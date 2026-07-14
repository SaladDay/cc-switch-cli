# Sessions & Usage Scan Performance Plan

This note is the implementation plan for speeding up the two slowest data
paths in the TUI: the Sessions page scan and the Usage statistics session-log
import. The target user runs the TUI on demand (start, look, quit) rather than
keeping it resident, so every optimization below is judged by what it does for
a fresh process on an ordinary machine (4–8 cores, SSD/HDD laptop, expensive
fsync on macOS, antivirus-taxed file opens on Windows) — not by what it does
for a large workstation.

## Measured baselines

Measured with a release build against a real history (Claude: 17,636 session
JSONL files, 2.45 GB, largest single file 232 MB; Codex: 821 files, 0.85 GB),
using a sandboxed `CC_SWITCH_CONFIG_DIR`. The test machine has a large page
cache, so cold-start numbers on ordinary hardware are strictly worse than
these:

| Scenario | Result |
| --- | --- |
| `sessions list --all`, warm cache | 0.77 s (4 scan threads) |
| `sessions sync-usage --all`, first full import | 133 s, 301,975 rows, single-threaded, 57% CPU |
| `sessions sync-usage --all`, second run (incremental) | 1.7 s |

## Bottlenecks

Usage import (`src/services/session_usage*.rs`):

1. Every message is inserted through `insert_session_log_entry`
   (`session_usage.rs:569`): per message it locks the connection, runs two
   dedup lookups plus a pricing lookup, then a single auto-commit `INSERT`.
   300k messages ≈ 1.2M queries and 300k commits. WAL is enabled
   (`database/mod.rs:518`) but `synchronous` stays at the default `FULL`, so
   every commit pays an fsync.
2. The Claude scanner parses every JSONL line into a full
   `serde_json::Value` (`session_usage.rs:350`) with no cheap prefilter, even
   though only `"type":"assistant"` lines matter. Multi-megabyte tool_result
   lines get fully deserialized and dropped. (The Codex scanner already
   demonstrates the prefilter pattern, `session_usage_codex.rs:277`.)
3. Incremental resume is line-based, not byte-based: a file whose mtime moved
   is re-read from byte 0 (`session_usage.rs:333`), so an active session file
   (232 MB here) is re-read on every sync cycle. Codex is worse: the offset
   skip happens *after* parsing (`session_usage_codex.rs:389`) because the
   cumulative-token delta state must be rebuilt, so a growing Codex session is
   fully re-parsed every cycle.

Sessions scan (`src/session_manager/`):

4. The scan cache lives only in TUI process memory
   (`cli/tui/app/types.rs:160`), so every TUI launch re-reads head/tail of
   every session file. Warm that is ~0.8 s; cold on an HDD or behind antivirus
   it is tens of seconds.
5. The UI paints only after all providers finish scanning, merging, and
   sorting (single `ScanFinished` message, `session_manager/mod.rs:61-94`), so
   time-to-first-paint equals the slowest provider.
6. Provider-specific waste: Gemini reads and parses each whole session JSON
   for four metadata fields (`gemini.rs:200`), serially; OpenCode falls back
   to reading every message/part file when a session has no title
   (`opencode.rs:648`); `read_head_tail_lines` opens large files twice
   (`utils.rs:63`, `utils.rs:82`).

## Plan

### P0 — Batch the usage import write path

Scope: `services/session_usage.rs`, `session_usage_codex.rs`,
`session_usage_gemini.rs`, `session_usage_opencode.rs`, `database/mod.rs`.

- Wrap each file's inserts in one transaction (`BEGIN`/`COMMIT` per file, or
  per N=500 rows for very large files). Reuse prepared statements across rows
  via `Connection::prepare_cached`. Update the file's
  `session_log_sync` row inside the same transaction.
- Cache pricing lookups in a per-sync `HashMap<String, Option<ModelPricing>>`
  keyed by model name instead of querying `model_pricing` per message.
- Set `PRAGMA synchronous = NORMAL` next to the existing WAL setup. In WAL
  mode this cannot corrupt the database; the worst case on power loss is
  losing the newest transactions, and usage rows are always re-importable
  from the source session files.
- Prefilter Claude lines with a substring check (e.g.
  `line.contains("\"assistant\"")`) before JSON parsing, then deserialize
  into a narrow `#[derive(Deserialize)]` struct rather than `Value`.
- Sort files by mtime descending before syncing so the most recent history
  lands in the database first; the Usage page defaults to Today/7d, so useful
  numbers appear within seconds even while a long first import continues.

Expected effect: first import drops from ~133 s to roughly 10–20 s on the
same data; the fsync reduction helps weakest disks the most.

### P1 — Persistent session-scan cache with stale-while-revalidate

Scope: new sidecar cache store, `session_manager/`,
`cli/tui/runtime_systems/workers.rs`, `cli/tui/app/types.rs`, TUI handlers.

- New table `session_scan_cache`
  `(provider TEXT, file_path TEXT PRIMARY KEY, mtime_ns INTEGER, size INTEGER,
  meta TEXT, cache_version INTEGER)` — stored in a **separate local sidecar
  database** (`$CC_SWITCH_CONFIG_DIR/session-scan-cache.db`), NOT in
  cc-switch.db: the main database's schema is locked to the upstream project
  (see Constraints) and WebDAV syncs it as a whole, while this cache is
  machine-local (absolute paths) and fully rebuildable. The sidecar opens
  with WAL + synchronous=NORMAL and degrades gracefully: any open/read error
  behaves as an empty cache.
- Scan flow becomes: load cached rows and render immediately; in the
  background walk the session directories with readdir+stat only; parse
  head/tail only for new files or files whose `(mtime_ns, size)` changed;
  upsert those rows; delete rows whose files disappeared; then send the
  refreshed list to the UI.
- Unchanged files cost one `stat` instead of open+read: a cold-cache launch
  goes from "open and read 17k files" to "stat 17k files".
- First-ever run (empty cache) behaves like today's full scan, then seeds the
  cache. Manual reload (`r`) keeps forcing a full re-parse.
- Fix `read_head_tail_lines` to open the file once (seek instead of reopen).

Expected effect: first paint of the Sessions page under ~100 ms on any
machine after the first run; disk work proportional to changed files only.

### P2 — A shared incremental-sync driver with byte-offset resume

Scope: sidecar cache store, new `services/session_usage_driver.rs`,
`services/session_usage.rs`, `session_usage_codex.rs`.

Every app's usage import goes through one shared contract, designed so new
apps can be added without re-inventing the machinery:

- **Authoritative progress** stays in cc-switch.db's `session_log_sync`
  (`last_modified` + `last_line_offset`, upstream-compatible shape).
- **Acceleration hints** live in the sidecar (`session_sync_resume`):
  per-file byte offset plus a serialized parser state, a Unix inode identity,
  and a tail fingerprint (FNV-1a over the ≤64 bytes before the offset). A hint
  is honored only when its `(last_modified, last_line_offset)` snapshot exactly
  matches the authoritative row, the file has not shrunk, the recorded Unix
  inode still matches, and the tail fingerprint still matches the file's bytes.
  The inode check closes rename-replace rewrites and cross-machine same-path
  files (a different physical file at the same path has a different inode); it
  is skipped when the hint predates this column or the platform is non-Unix.
  The tail fingerprint is an **append-only heuristic, not a strict guarantee**:
  it only identifies an in-place rewrite that changed the ≤64 bytes before the
  offset (inode unchanged) — an in-place truncate+rewrite to a *larger* file
  whose first bytes happen to be byte-identical would slip past both the
  size/mtime checks and the fingerprint. That case is effectively unreachable
  for session logs (each JSONL line is an independent record and log apps only
  append, never rewrite their own history), so we deliberately do not chase a
  perfect cross-platform scheme. Any mismatch (database synced in from another
  machine, rotated file, changed inode, missing hint) falls back to today's
  read-from-zero line-counted pass and records a fresh hint.
- **Persisted progress stops at the last newline boundary.** A trailing line
  without a newline is still fed to the parser (a finished file's last line
  may legitimately never get one) but is not counted into
  `session_log_sync`/hint progress, so a line that was caught mid-append is
  re-read next cycle instead of being skipped forever (per-app request-id
  dedup makes the re-read safe). This also fixes the pre-existing loss in
  the line-offset path.
- **The generic JSONL driver** (`scan_jsonl_incremental`) owns: the mtime
  skip, hint validation, seek-or-fallback, byte-exact line reading
  (`read_until`), and line/byte bookkeeping. An app plugs in two things: a
  serde-serializable parser state (Claude: `{session_id}`; Codex: the full
  cumulative-token state machine) and a per-line callback. Parsing belongs to
  the app; file driving belongs to the driver.
- **Write semantics stay per-app** (Claude uses INSERT OR IGNORE with its
  dedup key, Gemini upserts, Codex allows missing cache_creation): unifying
  them would risk parity bugs for no scan-cost win. All apps follow the same
  two-phase shape — scan/collect first, then batch-write in one transaction —
  so the connection lock is never held while reading files.
- Non-JSONL sources cannot byte-resume by nature and use the mtime-skip
  contract only: Gemini re-reads its whole session JSON when changed;
  OpenCode queries its external SQLite. They still share the sync-state
  helpers and result shape.

Adding a new app later means implementing: a file collector (walk + mtime,
newest first), a parser state + line callback for the driver (if JSONL), and
an insert function with the app's dedup semantics. Everything else — resume,
batching, progress bookkeeping — is inherited.
- Guard against truncation/rotation: if current file size < stored offset,
  reset and re-import that file from zero (dedup by request_id makes this
  safe).

Expected effect: steady-state sync cost becomes proportional to appended
bytes, not file size; a TUI left open next to an active 200 MB session stops
re-reading it every cycle.

### P3 — Perceived-latency polish

Scope: TUI only.

- The "all providers" session scan emits a partial result after each provider
  finishes revalidating, so a genuine full scan (first-ever run, manual
  reload) fills the list progressively instead of appearing all at once.
- The Usage summary bar shows import progress (files processed / total) while
  the background sync runs, fed by a process-global atomic tracker in
  `session_usage::sync_progress` (the CLI build has no per-row notification
  channel — `notify_log_recorded` is a no-op stub here).
- While that sync is active and a Usage route is visible, the main loop
  re-aggregates the current range every ~2.5 s, so the numbers fill in live;
  combined with P0's newest-first import order, Today/7d becomes meaningful
  within seconds of a long first import.

### Optional follow-ups (not scheduled)

- Parallel parse pipeline for the usage import (3–4 reader threads feeding a
  single writer thread); keep the adaptive conservative thread cap.
- Parallelize Gemini/OpenCode session scans and make Gemini parse partially.

## Constraints

- Never touch host configuration (`$CC_SWITCH_CONFIG_DIR`, Claude/Codex/...
  live config dirs); all tests must isolate via `tests/support.rs` helpers.
- **cc-switch.db's schema is locked to the upstream project**: no new tables,
  no new columns, no `SCHEMA_VERSION` bump — the database syncs with upstream
  builds (and via WebDAV across machines), so schema drift breaks
  interoperability. Anything this plan needs to persist lives in a separate
  machine-local sidecar store instead. Connection-level PRAGMAs and ordinary
  data rows are fine.
- Behavior parity: imported row contents, dedup semantics
  (`should_skip_session_insert`), and Usage aggregates must not change; only
  cost and ordering of work may change.

## Landing order

P0 and P1 are independent and land as separate commits. P2 builds on P0's
final shape of the sync loop. P3 lands last and is optional if the earlier
phases already make the pages feel instant.
