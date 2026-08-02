# Canonical Restore Redesign Handoff

Status: **work in progress; do not merge as-is**

Prepared: 2026-08-02

Target branch: `main`

Issue: [#391](https://github.com/SaladDay/cc-switch-cli/issues/391)

## Why this branch exists

Large SQL imports can keep the CLI busy for minutes and block the TUI. The immediate report in #391 used a 34.6 MiB export and observed one CPU core at 100% for about ten minutes. Investigation also showed that the old restore path had security and consistency problems that could not be fixed safely by optimizing its statement loop alone.

This branch therefore replaces the restore boundary instead of applying another parser micro-optimization. It rebuilds untrusted input into a canonical database, validates it using the real runtime domains, publishes it through SQLite's backup API, coordinates database and Skills state, and runs restore work outside the TUI thread.

The work was intentionally stopped before final convergence. The current tree compiles, but the latest capability-gate changes have not received a complete regression run or the required fresh blind-review round.

## Upstream reference

The restore trust-boundary work was ported and adapted from the sibling `../cc-switch` repository at:

```text
feat/pi-native-support
3fa6b1f158b4e609b1588affb366bd90fb11878e
```

That pin includes `28530ff6` (`enforce canonical restore trust boundaries`) plus the later migration-source specifications. Do not silently rebase the design onto a moving upstream branch; compare and record any newer pin explicitly.

The CLI-specific policy is intentionally not a byte-for-byte copy. In particular:

- `usage_daily_rollups.avg_latency_ms` accepts numeric REAL values such as `1.5` despite its INTEGER declaration.
- `model_pricing` is user data and must not be overwritten as seed data.
- Hermes and OpenClaw tables have explicit CLI policies.
- `proxy_failover_live_snapshots` is handled as a child of `providers`; invalid overlay rows are dropped rather than inserted with foreign keys disabled.

## Intended invariants

1. Candidate schema is never published. A clean current-schema stage is created by trusted schema code, and rows are copied through an exhaustive table/column policy.
2. Values are read into Rust `rusqlite::types::Value`, checked for storage class and runtime domain, and only then inserted. SQLite affinity must not get a chance to normalize invalid input before validation.
3. The stage keeps foreign keys enabled, inserts parents before children, and must pass `PRAGMA foreign_key_check`.
4. The only database publication boundary is `rusqlite::backup::Backup::new(canonical_stage, live)` driven to completion. Do not restore by deleting/copying live rows, renaming database files, or retaining the old live schema.
5. Candidate hydration is pure. Host-dependent semantic migrations and live projection run only after publication, under the restore-exclusive capability.
6. Local SQL/database restore preserves live Skills. Cloud bundles replace Skills exactly and reject a database/ZIP mismatch in either direction.
7. Skills replacement is prepared on the same volume under `.restore/<operation-id>/`, installed with two renames, and made durable before SQLite publication. Recovery uses database intent/generation markers, never an external mutable JSON journal.
8. Normal mutations hold a shared capability for their whole read/compute/write/live-projection workflow. Restore holds the exclusive capability. Daemon, session-usage, and maintenance writers must participate.
9. Restore completion carries a publication-generation token. TUI handlers install results only when the token still matches the live database.
10. Untrusted input is bounded: regular files only, no symlinks/FIFOs/devices, 256 MiB-class artifact limits, SQL value/page/VM-step/cancellation budgets, NUL-delimited execution, and a restrictive SQLite authorizer.

## Implemented shape

### Canonical database import

- `src-tauri/src/database/sql_import.rs`
  - snapshots validated regular files;
  - rejects symlinks, FIFOs, devices, oversized input, and embedded NUL data;
  - executes untrusted SQL with authorizer and resource budgets;
  - removes external triggers/views/indexes before migration execution.
- `src-tauri/src/database/migration_source.rs`
  - recognizes the declared historical schema before creating missing tables;
  - checks `table_xinfo` and rejects hidden/generated columns;
  - rejects future versions and dishonest current-version stamps.
- `src-tauri/src/database/restore_policy.rs`
  - exhaustive CLI table policy with a coverage assertion;
  - typed storage/value-domain validation;
  - explicit PreserveLive/RebuildRuntime behavior.
- `src-tauri/src/database/canonical_import.rs`
  - owns the private `CanonicalStage` construction barrier;
  - applies typed live overlays while foreign keys remain enabled;
  - publishes the whole canonical database through SQLite Backup.
- `src-tauri/src/database/backup.rs`
  - routes SQL, binary backup, local restore, and cloud restore preparation through the canonical path;
  - creates bounded pre-restore backups and clears imported restore metadata.

### Restore transaction and recovery

- `src-tauri/src/restore_protocol.rs` defines operation IDs, Skills mode, generation keys, and durable intent metadata.
- `src-tauri/src/database/restore_state.rs` validates complete, matching restore metadata.
- `src-tauri/src/services/skills_restore.rs` implements same-volume staging, generation markers, two-rename installation, rollback, finalize, and crash recovery.
- `src-tauri/src/services/restore.rs` owns the exclusive restore session and the sequence:

```text
recover pending intent
  -> load state without startup side effects
  -> preflight proxy/takeover state
  -> arm canonical stage with new generation
  -> persist old-live intent
  -> install Skills if applicable
  -> publish canonical stage through SQLite Backup
  -> finalize Skills
  -> run post-commit semantic migrations
  -> pure hydrate
  -> install in-memory snapshot
  -> project providers/prompts/MCP/Skills to live files
  -> mark post-commit applied
```

Post-publication failures are reported as `Restore committed; <phase> is pending retry`, not as a generic failed restore.

### TUI and sync paths

- Local import, local backup restore, WebDAV download, and S3 download use the shared restore coordinator.
- Restore work runs in background workers.
- Completion messages carry the publication token and a fresh snapshot.
- Cloud snapshot creation uses a coherent state barrier and deterministic Skills ZIP generation.
- ZIP decoding rejects the entire archive when an entry has no safe enclosed name.

### Skills path hardening

`src-tauri/src/skill_directory.rs` defines a portable single-component directory type. It rejects separators, dot components, drive/UNC/colon forms, Windows reserved names, trailing dots/spaces, and normalized collisions. Validation is applied at restore decode, save, and runtime remove/sync boundaries.

### Related session scan fix

The branch also carries a sidecar scan-cache correction: file size is stored with the cached identity so a Codex JSONL append with an unchanged timestamp is still detected. This does not alter the main application database schema.

## Compatibility and fault tests added

The current changes include coverage for:

- injected triggers and schema objects not surviving canonical publication;
- hidden/generated columns and incomplete source schemas;
- dishonest `user_version` values and future schema versions;
- invalid storage classes, out-of-range proxy ports/bind values, overflowing sort indices, and fractional rollup latency;
- foreign-key orphans and overlay rows whose parent is absent;
- local Skills preservation and exact cloud DB/ZIP Skills matching;
- unsafe or colliding Skills directories and invalid ZIP paths;
- regular-file, symlink, FIFO, NUL, page, value, and VM-step limits;
- publication failure rollback and both database-intent crash states;
- post-publication semantic/Gemini scrub failure becoming a retryable committed state;
- a long-lived WAL connection continuing on the published database;
- a real v13 fixture generated from CLI commit `3c3a7f9` (`src-tauri/tests/fixtures/restore/v13-from-3c3a7f9.sql`).

## Validation evidence

All Cargo commands below were run from `src-tauri/` with isolated `HOME`, `CC_SWITCH_CONFIG_DIR`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, and XDG directories.

Current tree, after freezing the WIP:

- `cargo fmt --check`: passed on 2026-08-02.
- `cargo check --tests`: passed on 2026-08-02 (existing dead-code warnings remain).
- `git diff --check`: passed.

Earlier stable points in the same worktree, before the latest operation-scoped capability refactor:

- `cargo test --lib --quiet -- --test-threads=1`: 4,134 passed, 0 failed, 2 ignored, 73.82 s.
- database backup tests: 38 passed.
- restore-service tests: 6 passed.
- provider-service tests: 69 passed.
- WebDAV sync service: 7 passed in 4.01 s.
- import/export sync: 31 passed.
- daemon supervisor: 18 passed.
- prompt commands: 9 passed.
- provider commands: 29 passed.
- settings visible-apps/current-provider shims: 22 and 12 passed.
- Windows `x86_64-pc-windows-gnu` full library check: passed.

These earlier results must not be presented as final validation of the current head.

## Performance evidence

The local sample is intentionally not committed:

```text
/home/fanjingluo/cc-switch-export-20260801_034514.sql
57,048,540 bytes
```

Observed prototype results before the latest capability changes:

| Implementation point | Wall time | Peak RSS |
| --- | ---: | ---: |
| Earlier WIP importer | about 7.98 s | 76,640 KiB |
| Canonical redesign | about 10.5-11.3 s | about 74 MiB |

This confirms a seconds-scale path on a sample larger than the issue report and no peak-memory regression in that comparison, but the numbers are not a final benchmark and were not collected from `origin/main` under identical conditions. Re-run an apples-to-apples benchmark before claiming a final speedup.

## Known unfinished work

The capability-gate refactor was the active task when work stopped. The previous design kept a shared permit for the entire `AppState` lifetime, which deadlocked restore preflight whenever a caller retained an `AppState` while the proxy was running. The replacement makes permits operation-scoped and has already fixed the reproducer `webdav_download_rejects_when_proxy_running` (1.00 s), but the audit is not complete.

Concrete next checks:

1. Finish auditing command/TUI entries that acquire a shared permit and then call `AppState::try_new()` or another permit-acquiring wrapper. `src-tauri/src/cli/commands/proxy.rs` was still known to contain this shape when the handoff was written.
2. Finish explicit-capability plumbing for direct failover/settings/history/provider mutation paths. Avoid nested Tokio `RwLock` read acquisition: once an exclusive waiter is queued, a second read acquisition by the same workflow can self-deadlock.
3. Keep public convenience wrappers that acquire a permit, but make already-coordinated internal paths call `*_with_permit` variants.
4. Re-run focused MCP, prompt, provider, proxy, daemon, WebDAV/S3, Skills, and restore tests after the gate audit.
5. Run the complete suite with one test thread, then `cargo test --features test-hooks`, `cargo clippy`, the Windows check, WAL tests, and the 57 MiB benchmark.
6. Run two fresh independent blind reviews under `AGENTS.md`. No reviewer should see this implementation summary or any earlier findings before producing a report. Validate every finding, fix confirmed issues, and start a fresh round. If that round does not converge, stop rather than layering more patches onto the design.

## Safe test shell

Do not run mutation-capable tests against host configuration. A suitable shell setup is:

```bash
cd src-tauri
set -euo pipefail
umask 077
unset NO_COLOR CLICOLOR CLICOLOR_FORCE CC_SWITCH_COLOR_MODE
restore_test_root=$(mktemp -d /tmp/cc-switch-restore.XXXXXX)
mkdir -p "$restore_test_root/home" "$restore_test_root/runtime"
export HOME="$restore_test_root/home"
export USERPROFILE="$restore_test_root/home"
export CARGO_HOME=/home/fanjingluo/.cargo
export RUSTUP_HOME=/home/fanjingluo/.rustup
export CC_SWITCH_CONFIG_DIR="$restore_test_root/cc-switch"
export CLAUDE_CONFIG_DIR="$restore_test_root/claude"
export CODEX_HOME="$restore_test_root/codex"
export XDG_CONFIG_HOME="$restore_test_root/xdg-config"
export XDG_RUNTIME_DIR="$restore_test_root/runtime"
export XDG_STATE_HOME="$restore_test_root/xdg-state"
cargo test --quiet -- --test-threads=1
```

Never modify the host's configured CC-Switch, Claude, or Codex directories while continuing this work.

## Suggested review order

1. Read `docs/session-cost-v3-plan.md`, `AGENTS.md`, and `CLAUDE.md`.
2. Review the stage/type barriers and publication boundary before reviewing individual validators.
3. Review the database-intent/Skills crash state machine.
4. Review migration-source recognition and the real v13 fixture.
5. Review local-versus-cloud Skills policy and overlay semantics.
6. Complete the operation-scoped capability audit.
7. Only then evaluate TUI behavior and performance.
