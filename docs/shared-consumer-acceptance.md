# Shared-consumer migration acceptance

The CLI migration branch must preserve records written by Lite in their shared
database. A common Core dependency does not by itself coordinate host snapshots,
database transactions, and native files. This gate also records requirements for
a future full desktop consumer; it does not claim full-product compatibility or
authorize accessing, changing, or opening PRs in that repository.

## Steps and boundaries

1. **Reproduce with real consumers.** Add an opt-in library test that lets Lite
   create a provider after CLI has loaded its in-memory snapshot, then performs
   an ordinary CLI Gemini switch. Require both a successful switch and retention
   of the peer record. Use only temporary profiles and fake credentials. This
   step changes tests and this plan, not production behavior.
2. **Limit the database write set.** Ordinary Gemini switching may update its
   selection and the provider snapshots it actually owns. It must not save or
   delete unrelated providers, MCP entries, common snippets, or extension data.
   Read operation inputs from a coherent database observation and guard writes
   against changes to those inputs. Cover success, stale input, and later failure;
   rollback must not replay an obsolete whole-catalog snapshot. Reuse Store's
   existing guarded operations before considering a new shared API. Do not
   change the global `AppState::save` behavior or migrate other workflows here.
3. **Coordinate database and native-file outcomes.** Match the shared contract:
   database write transaction, then shared filesystem lock, then native work,
   then commit or compensation while both protections remain held. Verify actual
   CLI/Lite contention and lock release on success and failure. Audit nested DAO,
   MCP, Skill, and host-settings calls before extending a lock lifetime; a lock
   around the existing broad saves is not sufficient. Core supplies common
   contracts and execution; the host owns its transaction and side-effect order.

Each step needs local validation and two fresh independent blind reviews of all
changes. Reviewers receive the goal, expected behavior, acceptance criteria and
boundaries, not implementation hints or earlier findings. Confirm findings before
fixing them and repeat the gate. Reduce to one fresh reviewer only for converged
small refinements; reconsider the design if fixes keep cycling.

Force writes, proxy/takeover, other Apps, standalone MCP/Skill operations, UI,
schema migrations and full-product adoption are separate slices. No all-writer
concurrency claim is allowed while those paths remain unverified. CLI work stays
on its migration branch and must not be merged into `main`.

## Opt-in cross-repository test

The tests use independently compiled library test binaries, not installed apps,
mock adapters, or direct SQL in place of Lite's service. Build the CLI and Lite
from their respective `src-tauri/` directories, using their supported toolchains
(CLI 1.91.1, Lite 1.88.0):

```sh
cargo test --locked --lib --no-run --message-format=json
```

Use separate, newly created temporary `CARGO_TARGET_DIR` values. Set `TMPDIR` and
`CC_SWITCH_CONFIG_DIR` to owned temporary directories too; do not override the
shell's `HOME` or `CODEX_HOME`. The CLI test's `TestEnvGuard` owns profile isolation.
The Lite child inherits that test environment and additionally requires a marker
in the fixture. Extract the Lite test executable from Cargo's JSON records where
`reason == "compiler-artifact"`, `profile.test == true`, and `executable != null`;
do not guess a binary hash or point at a normal application executable.

Run from CLI's `src-tauri/`, with those same isolated environment settings:

```sh
CC_SWITCH_LITE_TEST_BINARY="/absolute/path/to/built/lite-test-binary" \
  cargo test --locked --lib \
  services::provider::gemini::execution_tests::gemini_switch_preserves_a_provider_committed_by_lite_after_cli_observation \
  -- --ignored --exact --test-threads=1 --nocapture
```

The test starts exactly Lite's ignored
`consumer_coordination::create_provider_in_cli_fixture` worker. Missing or
incorrect executables/fixtures fail the test; they do not silently skip it. The
child has a 30-second timeout and is reaped before the profile is released.
Keep Lite's target until the child has finished, then run `cargo clean` against
each exact owned target. Remove only owned synthetic fixtures after all test
processes have exited. Never use the normal applications or real profiles for
this test.

## Initial observation

With production CLI `b71916bddd68aa2656c99a9d69050a9a16d6873f` (Core/Store
`2bd92f0062f1e42bcc93336c5664f3b3d944f44c`) and Lite
`9dd59c7ca44fcdd882ae9670fae4c22e2ada8a13` (Core/Store
`743b1a963cda31be8cbbc55786a39488bb49c5a7`), plus these test-only additions,
the worker creates the provider successfully. CLI switching then succeeds but
removes the peer provider. The regression test is intentionally failing on this
baseline and ignored by ordinary one-repository test runs because it needs the
other consumer's binary. Passing the ordinary suite is **not** passing this gate.
Do not invert the assertion or mark the loss as accepted behavior to make it green.

This sequential interleaving proves stale-snapshot data loss, not simultaneous
writer behavior. Step 2 must make this test pass; step 3 still needs concurrent
native-write and recovery evidence. Record both exact consumer revisions and
Core/Store pins again when those steps pass.

## Scoped ordinary Gemini switching

The second slice replaces ordinary Gemini switching's whole-snapshot saves with
one host-owned immediate transaction. It reads the Gemini provider catalog,
common snippet and MCP catalog from that transaction. Store's guarded updates
write only the previous/target provider settings and Gemini selection flags;
metadata, endpoints, unknown columns, other Apps and the MCP/settings catalogs
are not rewritten. Old selection flags are cleared before the new one is set.
Provider settings retain unknown root fields alongside `env` and `config` in
both the stored row and the published cache. The native-owned `env`/`config`
sections keep their existing replacement and normalization behavior. This fixes
root-field loss in ordinary Gemini switching only; other snapshot paths are not
changed here.

The native work still uses Core receipts and CLI's process-local Gemini session.
The transaction stays open through native publication, the existing all-App MCP
tail and target snapshot persistence. On failure, native compensation runs before
database rollback; a failed commit is included. SQLite can itself abort a
transaction, so this is not a SQLite/filesystem atomicity guarantee. The in-memory
snapshot is published only after commit. Existing local-settings selection
precedence and authentication policy are unchanged; the final local selection
save remains outside this recovery boundary. Best-effort Skill sync runs after
the provider transaction because it opens its own database and may migrate state.

The CLI lock order is its in-memory config lock, database transaction, then
process-local native session. Transaction work uses explicit snapshot/connection
arguments and must not re-enter AppState or DAO locks. The old staged transaction
no longer carries an unused Gemini operation option. Other workflows retain their
existing save, projection and compensation paths.

Acceptance includes the real Lite worker at
`ccec9bc0b9f3a4add4e1464e32f78a96b9f2d52d` (Core/Store
`743b1a963cda31be8cbbc55786a39488bb49c5a7`) against CLI's second-slice changes on
`1dd4854f6679773300d462f9b3af8a8540aea365` (Core/Store
`2bd92f0062f1e42bcc93336c5664f3b3d944f44c`). The previously red opt-in test now
passes. Local regressions also cover fresh target/selection, deleted targets,
unchanged peer and extension data after success or MCP failure, a unique-current
index, and deferred foreign-key failure at commit with native recovery. The CLI
commit carrying this record identifies the reviewed diff and validation results.

This does not complete step 3: actual cross-process file locking, contention and
release/recovery evidence remain required. No Core API, schema, UI, force-write,
proxy, standalone MCP/Skill or full-product migration is included in this slice.

## Shared lock adoption in the provider workflow

The next slice pins CLI and Lite to Core/Store
`d3570e075fd245e8bcbfd8f0ef364cbf730b5701` and uses Core's
`SharedLiveConfigLock`. Ordinary CLI Gemini switching acquires it after the
database write transaction and before observing native files. It retains both
protections through publication and native compensation, including failed commit;
early validation errors release them too. The file guard is released before the
independent best-effort Skill tail. Local selection/authentication policy is not
made transactional by this change.

Lite replaces its local file-lock implementation while keeping its process mutex,
error codes and receipt lifetime. Its `switch_with_provider` keeps the database
transaction alive on commit failure until native compensation finishes. The shared
method retains existing selection and additive metadata rules; no App-specific
switch policy is added. Standalone import, MCP, Skill and deletion transaction
lifetimes are not migrated in this slice, even though their existing file guard
now comes from Core.

The opt-in `execution_tests::coordination` tests add three real-process cases:

- A Lite native switch retains its receipt while CLI attempts an ordinary Gemini
  switch. CLI must refuse without changing native files or catalog rows, and must
  switch successfully after Lite restores its files and releases the receipt.
- During CLI publication and compensation, a separate process calls Lite's real
  native switch service. It must report file-lock contention, not a database-busy
  error or an unrelated failure. It must succeed after CLI releases the lock.
  Cases include success, native publication failure, MCP failure and failed commit.
- Lite's real Store/native switching path and CLI's ordinary switch operate in
  turn on the same catalog and profile, retaining the expected selection and
  credentials. This checks the full service path separately from native probes.

Run the three ignored tests together, using the isolated environment and
independently built Lite binary described above:

```sh
cargo test --locked --lib \
  services::provider::gemini::execution_tests::coordination \
  -- --ignored --test-threads=1 --nocapture
```

The ordinary suite also checks CLI validation-error release. Lite's
`switch_commit_failure_keeps_database_and_native_locks_through_recovery` uses
a deferred foreign key to fail COMMIT, verifies both protections in the native
recovery callback, checks exact restored bytes, then retries successfully. Before
the production changes, the Lite-held/CLI-refusal test failed because CLI wrote
through the lock, and Lite's commit-failure test found the database unlocked during
native compensation. Commit records contain the final validation and review results.

This establishes only the tested provider workflow boundary. CLI's other Apps,
force/proxy paths and standalone writers still need their own adoption gates;
no all-writer or full-product concurrency guarantee follows. The lock is advisory
and requires a stable shared lock file and aliases, as documented by Core.

## Standalone MCP adoption gates

The next work is split into three reviewed slices:

1. Add an opt-in regression using a real Lite MCP service in a separate process.
   Initialize CLI's state first, commit a different MCP record through Lite, then
   enable CLI's Gemini MCP target. Both catalog records and unrelated native
   settings/servers must survive. This slice changes only tests and this plan;
   it does not migrate a writer or treat an expected baseline failure as a pass.
2. Migrate the demonstrated CLI operation to fresh, scoped Store transactions and
   Core native coordination. Validate success, stale state, contention and recovery
   before publishing. Do not rewrite unrelated catalogs or replace AppState::save
   globally. Preserve existing missing-target and repeated-toggle behavior.
3. Extend adoption to the remaining standalone MCP mutations/imports in bounded
   slices, with the same recovery and real-consumer checks for each write path.
   Other provider workflows, Skill, proxy, UI and full-product integration remain
   separate work; the first green MCP case will not establish all-App adoption.

Build both library test binaries independently using their pinned toolchains and
locked dependencies. Set `CC_SWITCH_LITE_TEST_BINARY` to the Lite test executable
reported by Cargo's JSON compiler-artifact output, then run from CLI `src-tauri/`:

```sh
cargo test --locked --lib \
  services::mcp::consumer_tests::gemini_toggle_preserves_mcp_created_by_lite_after_cli_startup \
  -- --ignored --exact --test-threads=1 --nocapture
```

The test uses `TestEnvGuard` and a marked temporary file database. Its fake MCP
commands are never executed. The Lite worker rejects profiles outside the
temporary directory. Without the external test binary, the case remains ignored;
ordinary one-repository test success is not evidence for this acceptance gate.

The initial run against CLI production `168df3c3f8833d256968d22caea0b35e268036e7`
and Lite production `a0d1f7605aa7ad8c27393d614599bfb1f44d037a` reproduces record
loss: Lite commits successfully, CLI enables its target and preserves the checked
native fields, but the independently committed `lite-peer` catalog row is absent
after CLI returns. The acceptance test exits 101 at its retention assertion.
This is the red pre-migration baseline, not a passing compatibility result.

## Standalone Gemini MCP toggle

This slice moves only `McpService::toggle_app` for Gemini to the common guarded
MCP transaction. It reads the target from the database, changes its Gemini flag
and native link, and publishes the target cache row only after commit. Other
servers, App flags, host columns and catalogs are not rewritten. Missing targets
remain a successful no-op, with a stale target removed from the local cache.
Repeated toggles still repair the native entry; uninitialized Apps remain DB-only.

CLI initialization ensures Core's existing native-link schema and delete trigger
alongside its MCP catalog schema, without a new host schema version. This works
without Lite having opened the database first. Core owns the schema's legacy
ownership backfill and orphan cleanup, not the toggle transaction. Once started,
the guard captures both MCP tables before any operation write.

Lock order is config, database, shared file lock, Gemini native session. No DAO or
AppState lock is re-entered during execution. Native compensation precedes database
rollback, including failed COMMIT. SQLite-triggered transaction aborts cannot retain
the database lock; native recovery still runs under the file lock. External file
changes are preserved and incomplete recovery is reported, not silently replaced.

CLI and Lite share the opaque native snapshot for removed Gemini entries. CLI keeps
its document parser, legacy IDs, content limits, timeout defaults, catalog wrappers
and metadata filtering. Restoring a captured native entry does not reinterpret its
extensions as catalog wrappers. Core supplies entry restoration and receipts;
hosts still choose their document and execution policies.

The acceptance cases cover peer-record retention, snapshot round trips through
both real services, and a real Lite native writer excluded during CLI publication
and failed-commit recovery. Local cases cover fresh targets, host extensions,
missing/uninitialized/repeated toggles, write suppression, cross-row drift, failed
COMMIT, SQLite abort, recovery conflict and lock release. The legacy uncoordinated
MCP-service lock test remains for upsert/sync; coordinated toggle tests check its
new lock order explicitly.

Other Apps, upsert/delete/set-apps/import, provider/Skill workflows and full-product
adoption are not migrated here. The whole in-memory catalog is not refreshed by
this toggle, and remaining snapshot-save paths can still overwrite peer data.
Neither one green operation nor matching Core pins prove the complete goal.

## Standalone MCP import gates

1. Add a test-only real-consumer baseline for all five CLI MCP importers. Lite
   commits a peer after CLI loads its cache. Importing a new entry, enabling an
   existing entry, importing an already-enabled entry, and importing an empty
   document must retain that peer. Existing catalog connections and other App
   flags stay unchanged; the selected App is enabled and the count keeps its
   existing meaning. The fixture's source document must remain byte-identical.
2. Replace whole-snapshot persistence with one shared transaction boundary for
   these importers. Keep host parsing, diagnostics, skipped entries, merge policy
   and import order. Do not copy the transaction coordinator for every App or
   change global AppState::save. Claude's legacy path-copy behavior needs its own
   baseline before changing its read boundary; not every import is read-only.
3. Validate fresh targets, no-op imports, catalog extensions, failures and native
   ownership coexistence. CLI and Lite have different existing same-ID conflict
   policies; sharing Core must not silently replace either policy. Provider,
   Skill, other MCP writers and full-product adoption remain separate work.

Each step uses the independent double-blind gate above. The first changes tests
and this plan only. Run its opt-in cases with the independently built Lite binary
and isolated environment described above:

```sh
cargo test --locked --lib services::mcp::consumer_tests::imports \
  -- --ignored --test-threads=1
```

The fixture matrix covers Claude, Codex, Gemini, OpenCode and Hermes explicitly;
OpenClaw and Pi have no CLI MCP importer. Existing targets carry a host-owned
column value distinct from its SQL default, so rebuilding a row cannot pass its
retention check merely by restoring that default. These are service-level sequential
interleaving tests, not proof of concurrent native observation or full-product
compatibility.

Against CLI production `5520f3afa604ec23b035f038a0ef072e7b2a1fe6` and Lite
`fec310fade692c2bbe6d6ab19788e91bb2ee47f5` (the same source tree as merged
`52210e96c84300380bd72be60e07245733b07271`), both using Core/Store
`7b7cfae53d997d7dd686427c3537dd359e560fe7`, all 20 opt-in cases fail at peer
retention: the real Lite worker commits successfully, CLI import returns the
expected count and preserves the checked target fields and native bytes, but
the peer row is absent afterward. The command exits 101. This is the recorded
red baseline; the ordinary MCP suite passing does not satisfy the import gate.

### MCP import catalog migration

The five standalone import services now share a guarded MCP catalog transaction.
Core/Store `b2adcdd71e928eeea9e3eab900b67bc5d8f7c28e` supplies the fresh catalog
read; each unchanged host importer keeps its parsing, count, skipped-entry and
same-ID policy. Only new records and changed selected-App flags are written.
Other App flags, host fields, peer records, native links and unrelated catalogs
are not rewritten. Successful imports refresh only the in-memory MCP catalog;
parse, write and commit failures do not publish staged cache data.

Lock order is config, database transaction, shared live-file lock. The file lock
is held through database commit or rollback, including no-op imports. Contention
returns a conflict before calling the native reader. Import-all still runs Claude,
Codex, Gemini, OpenCode and Hermes in order; the first error stops later imports
without undoing earlier Apps' committed imports.

Claude's legacy override-copy policy was tested against the previous production
implementation before changing this boundary: missing target copies the default
file, existing target wins, malformed copied content reports an error but keeps
the copy, and missing source or blocked destination retains the old no-op result.
That compatibility copy remains a host path migration, not a reversible native
write in this transaction. Ordinary source documents are not modified.

This slice preserves existing native links without creating or refreshing them.
Native observation verification and imported-entry ownership remain the next
gate; noncooperating native writers, other MCP writes, provider/Skill workflows
and full-product adoption are not covered by this catalog migration. The global
AppState snapshot-save path is unchanged and is still unsafe for peer data where
other workflows use it. CLI stays on the remote migration branch, not main.

The 20 previously failing real-consumer import cases now pass against unchanged
Lite `fec310fade692c2bbe6d6ab19788e91bb2ee47f5` with Core/Store `7b7cfae`.
The local MCP suite passes 172 tests (23 external-consumer cases remain opt-in).
Local import cases cover the Claude copy baseline, fresh and repeated imports,
skipped invalid entries, preserved host/App fields, parse errors, suppressed writes,
cross-row drift, deferred COMMIT failure, lock contention and retry, and ordered
partial progress when import-all fails. These are catalog acceptance results,
not evidence for the deferred native-ownership gate.
