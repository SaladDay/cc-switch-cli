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

### Native import interoperability baseline

This test-only slice extends acceptance to native configuration outcomes. It
does not change Core, production imports, toggle policy, schemas or UI. CLI stays
on its migration branch; the full desktop repository is not accessed or changed.

Run the nine opt-in cases with the same isolated build and peer-binary setup:

```sh
cargo test --locked --lib services::mcp::consumer_tests::imports::native \
  -- --ignored --test-threads=1 --nocapture
```

The new Lite worker is
`consumer_coordination::mcp_lifecycle::mcp_lifecycle_in_cli_fixture`. It uses the
real import, toggle and delete services against the marked CLI fixture. The
five lifecycle cases import through CLI, disable and enable through Lite,
repeat CLI import, delete through Lite, then import again through CLI. They
check catalog flags, deletion, source-byte preservation on import, native-only
fields and root settings, and retention of a separately created Lite catalog
row. Every source also contains a second native MCP entry with a distinct
extension; its complete content must survive each Lite mutation unchanged.
Native entry comparisons allow only the existing Codex `type`/implicit
enablement and Gemini timeout normalization. They do not require formatting
identity after a native write.

The peer launcher binds all supported environment path overrides to the fixture,
including Hermes, Pi and Windows local application data. Before MCP observation
or mutation, the new Lite worker checks its resolved MCP files and installation
markers against the canonical fixture root. A separate case seeds a Lite command
with outside synthetic path overrides before applying the fixture bindings,
disables Hermes, and requires that the outside file and directory remain unchanged.
It uses one Lite child and the existing bounded wait/reap path, without a nested
CLI process or changing the test runner's ambient overrides. No real profile is used.

Three further cases start with disabled Codex, OpenCode and Hermes entries.
Lite imports their disabled state; CLI then imports the same source without
rewriting it; a subsequent explicit Lite enable must activate the native entry.
Codex may omit `enabled` to mean true. These cases are intentionally red until
the shared-state discrepancy is resolved, not inverted into compatibility tests.

Production baselines are CLI `6958d9ed205d6a715acb205d54a499fb13773773`
(Core/Store `b2adcdd71e928eeea9e3eab900b67bc5d8f7c28e`) and Lite
`fec310fade692c2bbe6d6ab19788e91bb2ee47f5` (tree equal to main `52210e96`,
Core/Store `7b7cfae53d997d7dd686427c3537dd359e560fe7`), with these test additions.
All five enabled-entry lifecycles and the path-isolation case pass. All three
disabled-entry cases fail at
the native activation assertion: CLI import marks the shared App flag true,
while the native entry remains disabled; Lite's same-state toggle returns
without applying native changes. Ordinary one-repository test success does
not pass this interoperability gate. These sequential cases do not establish
simultaneous-writer safety, HTTP transport coverage or full-product adoption.

The next production slice must define how native observation, catalog selection
and an explicit activation request relate before changing either consumer.
Import must not silently activate a native entry. A successful activation must
not rely solely on a cached catalog flag. Preserve each host's parser tolerance,
same-ID conflict policy and unrelated fields. Any shared contract belongs in
Core/Store and must also suit a future desktop consumer; Lite-specific UI
restrictions must not enter Core. Test new and existing disabled entries, repeated
requests, conflicting native connections, rollback and peer-row retention.
Native observation drift and remaining writers still need separate gates.

### Bounded import timing probe

Run `services::mcp::consumer_tests::imports::scale::measure_import_scaling` with
`--ignored --exact --test-threads=1 --nocapture` in the isolated CLI environment.
It needs no Lite binary. The probe compares the guarded Gemini import with the
former snapshot-save boundary, reproduced only inside the test. The latter is
an unsafe shared-consumer baseline, never a production fallback. It measures
new and repeated imports at 10, 100 and 500 records, excluding fixture creation
and database startup, and checks counts, catalog size and unchanged native bytes.
Results are single local debug-profile observations, not a benchmark distribution
or release-build performance guarantee. No timing threshold is part of CI.

One local run on the production revisions above measured milliseconds below
(Rust 1.91.1, unoptimized test profile, debug info and incremental builds off):

| Records | Guarded new / repeated | Former boundary new / repeated |
| --- | --- | --- |
| 10 | 4.95 / 1.01 | 4.90 / 3.97 |
| 100 | 92.81 / 4.70 | 35.14 / 30.65 |
| 500 | 1671.58 / 22.64 | 158.47 / 161.18 |

Larger new imports require a separate Store batch-write design review: per-row
whole-catalog verification can make the work grow quadratically. Any optimization
must retain the initial two-table observation, unknown host fields, trigger-drift
detection, transaction poisoning and rollback. Do not reset the expected baseline
after writes or expose the raw connection to skip these guarantees.

### Native enablement import contract

This slice changes Codex, OpenCode and Hermes imports only. Import observes the
accepted native entry's enabled flag; it does not activate that entry. New rows
retain the observed state, and existing rows update only that App's selection in
either direction. The count is new rows plus rows whose App flag changed. An
unchanged repeated import returns zero. Missing entries do not clear selections.

Core supplies the entry-local flag codec. CLI keeps its existing non-boolean
Codex flag tolerance (accepted as enabled), parser bounds, extension selection
and skipped-entry rules. Existing IDs still retain their catalog connection and
metadata even when the source connection differs; observing that ID is not an
ownership or connection-equivalence claim. Codex keeps its first valid entry in
legacy-then-canonical order for both connection and flag, so conflicting copies
cannot make every repeated import count transient flag changes.

The guarded write updates the selected App column only. Unknown host columns,
other App selections and native links survive; cache publication follows commit.
Tests cover both flag directions, new/existing rows, stale cache, stdio/HTTP,
malformed flags, duplicate Codex IDs, failed selection writes and retry. The
opt-in disabled-entry cases exercise both CLI-first and Lite-first import orders,
then require explicit Lite enable to change the actual native entry while keeping
native siblings and extensions. Source bytes must remain unchanged by import.

This does not repair arbitrary pre-existing catalog/native drift without an
import, alter Lite's same-state toggle policy, claim simultaneous native-writer
safety, or migrate other MCP writers. Grok Build's document-level override stays
in Core's complete document importer; the entry-local codec is not a replacement
for that importer. No UI, schema, proxy, provider, Skill or full-product change is
included. CLI remains on its remote migration branch, outside `main`.

Local acceptance for this diff on CLI `62bb54125d013738ccaa78a24f7a0427819ddebb`
uses Core/Store `637b06dfb27a44deb0e95395a35d516813301d77` and unchanged Lite
`4d0a77b3a7c675ef00cb84218d2cd1ae5120cde4` (tree equal to main `f65ad1b3`,
Core/Store `7b7cfae53d997d7dd686427c3537dd359e560fe7`). All 31 opt-in
`services::mcp::consumer_tests` cases, excluding the manual timing probe, pass.
This includes the three formerly failing disabled-entry cases in both import
orders. Ordinary CLI MCP tests pass (175), as do the database-filtered tests
(153, with overlap), Lite library tests (121), formatting and locked all-target
Clippy. CLI Clippy retains pre-existing warnings and the previously documented
allowance for unrelated `reversed_empty_ranges`; none comes from this diff.

### CLI toggle adoption

The next migration has three gates, each with local validation and fresh double
blind review:

1. Record actual CLI enablement and shared-catalog outcomes using real Lite
   services. This gate adds tests only; failing assertions remain requirements,
   not accepted compatibility behavior.
2. Move the affected single-App toggles onto shared Core native projection and
   guarded Store writes. Update only the requested server/App, preserve native
   siblings and unrelated catalog rows, and retain host parsing/path policies.
   Do not replace the global snapshot save or migrate upsert/delete/set-apps here.
3. Verify failure recovery, native contention and alternating CLI/Lite mutations
   for the migrated paths. Keep database and native-file protections through
   recovery, and remove replaced duplicate logic only within those paths.

Run the opt-in baseline with the independently built CLI and Lite library test
binaries and isolated environment described above:

```sh
cargo test --locked --lib \
  services::mcp::consumer_tests::imports::native::cli_toggle \
  -- --ignored --test-threads=1 --nocapture
```

Six cases start with disabled Codex/OpenCode/Hermes entries and import through
either CLI or Lite, then load a current CLI catalog and request CLI enable twice.
They check the actual native command and enabled flag, not only the database flag.
Five further cases let Lite commit a new MCP row after CLI has loaded its catalog,
then repeat CLI enable for Claude/Codex/Gemini/OpenCode/Hermes. The complete newer
Lite row must survive. Both families check root settings and a distinct native
sibling exactly. These stdio cases do not claim simultaneous writer safety,
HTTP coverage, preservation of every field on the edited native entry, or
compatibility for other workflows. No production, UI, schema, provider, Skill,
proxy or full-product changes belong to the baseline gate.

On CLI `8a53cf4543651f4b324b440078281756bfad34e1` (Core/Store `637b06df`)
and unchanged Lite `4d0a77b3a7c675ef00cb84218d2cd1ae5120cde4` (Core/Store
`7b7cfae5`), plus these tests, 3 cases pass and 8 fail. Codex activation after
CLI import and both Hermes activation cases leave the native entry disabled.
The four non-Gemini toggles delete the newer Lite row. Gemini retains that row
but adds `timeout: 60000` to the unrelated native sibling. These failures show
why updating only the catalog flag is insufficient: the migration must restrict
both catalog writes and native document changes to the requested operation.
The existing 31 peer tests and ordinary suites do not cover these failures.

#### Gemini single-entry follow-up

The Gemini part of gate 2 reuses the existing Core entry codec, snapshot restore,
operation receipts and Store transaction guard. Single-App toggle now projects
only the requested entry. Sibling values and their field order stay intact,
including metadata and unrecognized entries; the whole JSON file still uses the
host's existing pretty-printing. Full-map sync keeps its previous conversion and
replacement semantics. No public Core API, dependency pin, schema, path policy,
selection/link ownership rule, or other App writer changes in this step.

Regression cases cover stdio and HTTP, wrapped and unwrapped catalog entries,
repeated enable/disable/restore, opaque siblings, and invalid target input without
native/catalog/link publication. Existing timeout, large-field, legacy-ID and
failure-recovery cases remain acceptance requirements.

Against CLI `771b13f8` plus this change and the unchanged Lite build above, the
toggle baseline is now 4 passing and 7 failing: Gemini retains both the newer Lite
row and its native sibling. The other four Apps' catalog-write failures and the
three Codex/Hermes activation failures remain open. This is not completion of
gate 2 or the shared-Core migration; their single-App writers still need scoped
transactions, native projection and recovery before the whole gate can pass.

#### Codex single-entry adoption

The next slice migrates only standalone Codex toggles. It shares the existing
Gemini catalog/commit/recovery coordinator, with native handling behind a private
trait. Selection columns come from Core's App descriptor. The host keeps fresh
Store rows and its shared live-file lock through native publication, database
commit and failure recovery; only the target cache row is published afterward.

Codex uses Core's native TOML editor and MCP executor at `6e28a236`, retaining the
CLI's connection conversion, accepted grammar and file-size policy. Enable owns
the native `enabled` flag even when an imported entry contains `false`. Only the
selected legacy ID is removed; unrelated official and legacy entries survive.
Disable still removes that ID, leaves a missing file absent, and skips malformed
TOML as before. Existing UTF-8 errors remain errors. The operation never reads or
writes `auth.json`. Live projection rejects unexpected stored native snapshots
without discarding their payload. The existing path/privacy rules and Codex
process lock remain host-owned; file comparison detects observed conflicts but cannot exclude writers
that ignore the shared lock in the interval between comparison and replacement.

Acceptance includes repeated enable/disable, fresh peer rows and host columns,
native siblings/comments, uninitialized Apps, malformed input, large/missing
files, leaf links, database suppression/verification/commit failure, uncertain
native publication, external edits during recovery and real Lite lock probes.
The existing Gemini suite must still pass after coordinator extraction. Real
Codex CLI/Lite activation and peer-preservation cases verify adoption, not just
compilation against Core.

Claude/OpenCode/Hermes toggle migration remains open. Upsert, delete, set-apps,
whole-map sync, provider writes, UI, proxy, Skill, schema and the full product are
outside this slice. Their existing behavior is not changed or claimed as shared
execution acceptance. Both Core/Store pins move together; rollback restores the
host callers and pins together without a data migration.

Local acceptance against CLI `e9290c8d` plus this slice and unchanged Lite
`4d0a77b3` passes 187 ordinary MCP-related tests, 33 opt-in peer/lock tests,
9 Codex executor tests and 153 database-filtered tests (with overlap; one ignored).
Formatting and locked all-target Clippy pass with the existing warnings and
documented allowance for unrelated `reversed_empty_ranges`.
The real toggle baseline is 6 passing and 5 failing: all three Codex cases pass;
Claude/OpenCode/Hermes peer-row loss and the two Hermes activation failures remain.
This advances the Codex part of gates 2 and 3, not whole-gate completion.

#### OpenCode single-entry adoption

This slice migrates standalone OpenCode toggles to the same catalog transaction
boundary. Core still owns entry conversion, file execution and retained recovery;
Store protects fresh catalog rows and native links. Codex and OpenCode share one
host file binding, while their format and initialization policies remain separate.
Codex's existing local lock still covers observation through commit or recovery.
No Core/Store API or dependency pin changes are needed.

OpenCode retains strict JSON, portable connection fields, missing-file schema
creation on either enable or disable, null-root promotion on enable, long legacy
IDs and large-file support. A malformed `mcp` collection or non-object/non-null
root now rejects enable without publishing file or catalog state, rather than
silently claiming activation or panicking. Disable retains its tolerant container
handling. Single-entry removal preserves sibling values and their order; JSON
still uses the existing pretty-printing. Unsupported stored snapshots are rejected
without discarding them. Path, permissions and leaf-link replacement keep the
host's existing rules. Writers that ignore the shared lock remain outside its
exclusion guarantee.

Acceptance covers fresh peer rows and host columns, stdio/HTTP/SSE conversion,
repeated selection, invalid input, missing/large files, long IDs, database failures,
external edits during publication/recovery, failed disable, leaf links and real
Lite activation, peer preservation and lock probes. The existing Codex and Gemini
tests remain required. Upsert, delete, set-apps, whole-map sync, other App writers,
provider behavior, UI, proxy, Skill and the full product remain outside this slice.

Local acceptance against CLI `544b153a` plus this slice and unchanged Lite
`4d0a77b3` passes 196 ordinary MCP-related tests, 34 opt-in peer/lock tests,
9 Codex executor tests, 139 database-module tests, 3 OpenCode config tests and
121 Lite tests (some suites overlap). Formatting and locked all-target Clippy
pass with existing warnings and the same unrelated `reversed_empty_ranges`
allowance. No warning points at a changed source file. Windows was not run locally.
The real toggle baseline is now 7 passing and 4 failing: all three OpenCode cases
pass; Claude/Hermes peer-row loss and the two Hermes activation failures remain.
This advances OpenCode's part of gates 2 and 3, not whole-gate completion.

#### Hermes single-entry adoption

This slice migrates standalone Hermes toggles to the shared catalog coordinator
and single-file Core executor. Core continues to encode portable MCP connections;
the CLI keeps its existing Hermes-private-field merge, YAML section renderer,
initialization, path, permission and backup policies. The Hermes process lock is
held from native observation through database commit or failure recovery. No
Core/Store API, dependency pin or Lite source changes are needed.

Enable now owns `enabled: true`; an older native disabled flag cannot override
selection. Other Hermes-private fields retain their previous precedence. Disable
still removes the selected entry. Sibling entries stay in YAML rather than being
converted through JSON, and removal preserves their order. The existing parser
continues to accept large files, long IDs, YAML-only values and tagged mappings.
Missing/empty files and non-mapping MCP collections keep their initialization
policy. Unsupported stored native snapshots are rejected without discarding them.

Before publication, the rendered YAML must parse and equal the intended document.
If text replacement cannot preserve that document, the toggle fails without
changing native or catalog state or creating a backup. This covers unsafe quoted
or flow sections, document markers and cross-section aliases, rather than writing
invalid or unrelated configuration. It is scoped to this toggle, not a rewrite
of the other Hermes writers. Changed nonempty source still receives the host's
pre-write backup with existing retention. Byte-identical output neither writes
nor backs up again; a failed publication or commit retains its recovery backup.

The shared single-file host also retains the original leaf-link relationship
for failure recovery. Codex, OpenCode and Hermes all use this host. Successful
writes still replace the link without changing its referent. A recovery link is
prepared before publication and renamed into place only if both the live file
and original referent still match the expected bytes. Relative and dangling
links are retained; changed links or referents cause an explicit recovery error
without overwriting external edits. Cyclic links and other read errors fail
before publication. Temporary recovery links are removed when the operation
ends. This is host filesystem policy; Core still owns execution and receipts.

Acceptance includes real CLI/Lite activation and peer preservation, backup
preimages and retention, YAML-only siblings, private fields, malformed input,
missing/large files, long IDs, database suppression/verification/commit failures,
uncertain publication, external edits, failed disable and leaf links. Existing
Codex, Gemini, OpenCode and Hermes config tests remain required. Other App writers,
upsert/delete/set-apps/whole-map sync, providers, UI, proxy, Skill and the full
product remain outside this slice. Noncooperating writers can still race the
host's file comparison and replacement; the shared lock is not an OS-wide CAS.

Local acceptance against CLI `fb171cb8` plus this slice and unchanged Lite
`4d0a77b3` passes 213 ordinary MCP-related tests, 35 opt-in peer/lock tests,
139 database-module tests, 11 Hermes config tests, 9 Codex executor tests,
3 OpenCode config tests and 121 Lite tests (some suites overlap). Formatting
and locked all-target Clippy pass with existing warnings
and the same unrelated `reversed_empty_ranges` allowance; no warning points at a
changed source file. Windows was not run locally. The real toggle baseline is
now 10 passing and 1 failing: all three Hermes cases pass, while Claude's stale
peer-row loss remains. This advances Hermes's part of gates 2 and 3, not whole-gate
or shared-Core migration completion.

#### Claude single-entry adoption

Standalone Claude toggles now use the same guarded catalog transaction and Core
MCP execution as the other migrated Apps. Both Core/Store pins move to
`1abff9e801895438cf41440852243e9a30cb3454`. The shared entry policy preserves
Claude-native fields; the CLI still owns one-level legacy wrapper selection,
catalog metadata filtering, strict JSON, pretty printing, paths and initialization.
The existing bulk writer reuses the same field selection without changing its
whole-map behavior. Only the selected entry is projected by a standalone toggle;
native siblings, unrelated top-level fields and newer shared catalog rows survive.

Disable retains a Core native snapshot, including across repeated removal of an
already absent entry. Enable restores it only when the entry is absent, using
the current catalog's connection fields, then clears the stored snapshot. Existing
native entries keep the previous enable/replacement policy. Real CLI/Lite tests
must verify snapshot exchange in both directions as well as peer-row retention.
Legacy IDs and large host-owned files do not acquire Core document-API limits.

For a missing custom-path destination, the legacy file is read as input and the
migration plus toggle is published once. Failure restores the original missing
leaf or leaf link, rather than retaining a preliminary copy. The source is never
written; later source edits do not change this operation's already-read input.
Existing destination permissions and managed-file privacy take precedence over
inherited Unix source permissions. Windows rejects a read-only migration source
before publication. A dangling destination link's referent is never created by
migration; successful writes replace the leaf, and failed writes restore its link.
Parent directories may remain after failure. Other Claude readers, onboarding,
imports and whole-map writers retain their existing eager migration behavior.

Acceptance covers malformed input/snapshots, missing and uninitialized targets,
native fields and siblings, permissions, custom-path migration, guarded database
updates and failed commit, uncertain publication, external changes and leaf-link
recovery. The Codex/OpenCode/Hermes shared file-host tests also cover inherited
creation permissions. Noncooperating writers can still race comparison and
replacement. Upsert, delete, set-apps, whole-map sync, other provider/Skill work,
UI, proxy, schema and full-product adoption remain separate slices. This is not
a claim that every CLI writer or the full product is coordinated.

Local acceptance against CLI `a959847a` plus this slice and unchanged Lite
`4d0a77b3` passes 223 ordinary MCP-related tests, 48 opt-in real-consumer tests,
14 MCP command tests and 121 Lite tests (some suites overlap). The former
11-case activation/peer baseline now passes in full; the opt-in set also covers
Claude snapshot exchange and lock probes. Formatting and locked all-target
Clippy pass with existing warnings and the same unrelated
`reversed_empty_ranges` allowance; none points at a changed source file.
Windows was not run locally. CLI remains on its migration branch, without a
main merge or a PR opened merely to obtain CI.

#### Multi-App MCP selection

`set_apps` now shares the standalone selection coordinator and native bindings.
It reads the current target from Store, compares the requested matrix with that
row, and writes only changed CLI-supported selection columns. Other consumers'
records, unknown columns and selections outside the CLI's App catalog survive.
Only the target cache row is refreshed after commit. A missing row returns false
and removes its stale cache entry; an unchanged matrix does not touch native
files or acquire their lock. Single toggles still repair repeated selections.

Changed native files are handled in the CLI's existing App order under one
database transaction and shared file lock. Each write observes the result of
earlier writes, including custom paths that name the same file. Receipts and
local App locks remain held until commit or recovery. A later observation,
publication, catalog update or commit failure recovers the writes in reverse
order; a recovery conflict is reported while other files still receive recovery
attempts. Initialization is decided before writing, so one App's directory
creation cannot initialize another App during the same operation.

All five standalone MCP bindings now use the common single-file host. Gemini
retains its JSON, field conversion and snapshot policy as a pure preparation
step, plus the same process-local settings lock. Its provider/whole-map workflows
keep their existing multi-file operation. The shared single-file host restores
leaf links as well as bytes, which is needed when one App's MCP path links to
another's. Gemini also retains its logical-path check before publication: a
parent-directory alias retargeted after observation is a conflict, not a successful
write to the old directory. Both public selection entry points test this case.
No Core API, dependency pin, schema or Lite source change is required.

Acceptance begins with CLI `d39b05cf`: two ordinary tests reproduce stale-target
overwrite and missing compensation after a later App fails; a real Lite peer
test reproduces deletion of its newly committed row. These are failing baseline
results, not accepted behavior. The implementation must pass these assertions,
all 32 supported selection matrices, fresh/missing targets, unchanged and
uninitialized Apps, unknown fields, database and uncertain-publication failures,
external recovery conflicts, shared destinations, cross-App links and real Lite
snapshot exchange and lock probes. Existing single-App regressions remain required.

The scope is matrix selection and the shared single-entry machinery it uses.
Upsert/delete/whole-map migration, other provider/Skill workflows, UI, proxy and
full-product integration remain separate. Parent directories and Hermes recovery
backups may remain after failure under existing host policy. This is cooperative
transaction/compensation, not crash-proof filesystem/SQLite atomicity; writers
that ignore the shared lock can still race comparison and replacement.

Local validation against CLI `d39b05cf` plus this slice and unchanged Lite
`4d0a77b3` passes 233 ordinary MCP-related tests, 51 real-consumer MCP tests,
14 MCP command tests, 11 Gemini operation tests, 190 config tests, 139 database
tests, 3 real-consumer Gemini provider tests and 121 Lite tests (some suites
overlap). The three baseline failures now
pass. Formatting and locked all-target Clippy pass with existing warnings and
the same unrelated `reversed_empty_ranges` allowance; no warning points at a
changed source file. Windows and abrupt-process/crash recovery were not run.

#### MCP catalog deletion

`delete_server` reads the current target through Store and removes only that row
and its canonical native links. It no longer saves the caller's whole cached
catalog. Native removal follows the current row's enabled CLI-supported Apps and
the existing initialization policy. Disabled or uninitialized Apps are not written.
The target row and all its links are explicitly owned by deletion, including links
from other consumers; native Apps unsupported by this CLI are not newly managed.
Their native cleanup remains outside this slice's App support boundary.

Deletion shares the existing native bindings, lock lifetime and retained recovery
with selection. Native entries are removed before the guarded catalog
delete and commit. Later failures recover native writes in reverse order and roll
back the database; conflicts preserve outside changes and do not skip recovery of
other files. Only a successful commit removes the target cache entry. An absent
database row returns false, removes its stale cache entry and leaves native files
untouched. Deletion does not deserialize obsolete or future native snapshots.

The host bindings preserve sibling entries and root fields rather than rebuilding
whole native MCP maps. Existing strict JSON, tolerant Codex removal, missing-file
rules, Claude read-only migration inputs and Hermes backups remain host-owned.
The Hermes binding also retains its existing protection against invalid or unrelated
YAML changes. A baseline comparison demonstrates one deliberate difference from the
legacy removal helper: raw `null` previously returned success while appending invalid
YAML. Shared removal reports an error and leaves the file and catalog intact. This
slice does not rewrite the YAML editor to accept additional layouts.

Two tests fail against CLI `1fd57961` before adoption: a stale caller deletes a new
peer row, and a later native failure leaves the target catalog row deleted. Required
acceptance includes all 32 supported selection matrices, peer fields and links,
unknown snapshots, missing/stale caches, uninitialized Apps, native parser baselines,
guarded deletion/cascade/verification/commit failures, lock retention and retry,
uncertain writes, external recovery conflicts, shared destinations and leaf links.
Real Lite tests cover new peer retention, deletion in both directions and writer
exclusion through publication/recovery. Selection fixtures are shared unchanged by
the two test modules; production initialization and App support are not broadened.

No Core API, dependency pin, schema, Lite source, UI or proxy change is needed.
Upsert, whole-map sync, other provider/Skill workflows and full-product migration
remain separate. Recovery is cooperative compensation, not crash-proof atomicity;
parent directories and host recovery backups may remain after failure. Core/Store
remains pinned to `1abff9e801895438cf41440852243e9a30cb3454`; rollback of this CLI
slice needs no data migration.

Local validation against CLI `1fd57961` plus this slice and unchanged Lite
`4d0a77b3` passes 241 ordinary MCP-related tests, 53 real-consumer MCP tests,
14 MCP command tests, 11 Gemini operation tests, 190 config tests, 139 database
tests, 3 real-consumer Gemini provider tests and 121 Lite tests (some suites
overlap). The two failing baselines now pass. Formatting and locked all-target
Clippy pass with existing warnings and the unrelated `reversed_empty_ranges`
allowance; none points at a changed source file. Windows, crash recovery and
noncooperating-writer races were not validated locally.

#### MCP catalog create and update

`upsert_server` now saves only its requested row through the existing Store guard.
The incoming fields and CLI-supported selection matrix replace that row's known
values; fresh peer rows, host-owned columns, non-CLI selections and unrelated
native links survive. The DAO's registry-driven value conversion is reused.
No Core API, dependency pin, schema or Lite source change is required.

Native work follows the current database row, not the caller's cached flags.
Previously enabled Apps that are now disabled are removed first, then every
incoming enabled App is refreshed, even when its flag did not change. Each group
retains the host's App order. Unchanged disabled Apps are untouched. Initialization
is decided before publication; uninitialized Apps only receive catalog changes
and retain opaque native snapshots without interpreting them.

Create/update, selection and deletion share the same transaction, filesystem lock
and retained native recovery. Catalog insertion/update precedes native-link writes
inside the uncommitted transaction. Native observation remains sequential for
aliased paths. Any later failure attempts reverse-order native recovery before
database rollback; only commit publishes the target cache row. Other cache rows
are not refreshed or saved. Unknown snapshots needed for a native write fail safely.

Editing retains the existing per-App conversion and activation policies. Codex's
incoming native `enabled` value and Hermes's merge-preserved native activation are
not forced on by an edit; explicit selection still activates them. Claude/Gemini
removal snapshots exchange with Lite and restore native extensions using current
catalog connection fields. Native siblings are not rebuilt or cleaned as part of
a target edit. Existing Hermes safe-YAML checks and recovery backups remain in use.

Two tests fail on CLI `be0d54bb` before adoption: whole-cache persistence removes a
new peer, and a later native failure leaves the catalog already changed. Acceptance
covers create/update across all 32 supported matrices, repeated edits, fresh flags,
metadata, unknown columns, uninitialized Apps, opaque snapshots, conversion parity,
catalog/link/verification/commit failures, lock retention, retry, uncertain writes,
external recovery conflicts, shared destinations and cross-App links. Real Lite
tests exchange snapshots and check peer preservation and writer exclusion through
commit and recovery. All fixtures use isolated directories and synthetic credentials.

Whole-map MCP sync, remaining provider/Skill workflows, UI, proxy and full-product
adoption remain separate. This is cooperative compensation, not crash-proof
filesystem/SQLite atomicity. Uncooperative writers can race comparison/replacement;
parent directories and host recovery backups may remain after failure. Unsupported
Apps do not acquire new native writers. CLI stays on its migration branch.

Local acceptance passes 250 ordinary MCP-related tests, 55 real-consumer MCP
tests, 14 MCP command tests, 11 Gemini operation tests, 190 config tests, 139
database tests, 3 real Gemini provider tests and 121 Lite tests (some suites
overlap). Formatting and
locked all-target Clippy pass with existing warnings and the unrelated
`reversed_empty_ranges` allowance; none points at a changed source file. Windows,
crash recovery and noncooperating-writer races were not run locally.

#### Public whole-catalog MCP synchronization

`sync_all_enabled`, `sync_enabled_for_app` and the deprecated `sync_enabled`
read current Store rows, not the caller's cached catalog. They do not save or
refresh that cache. Full and targeted synchronization include disabled rows for
native removal; the deprecated method keeps its enabled-only scope. Initialization
is decided before all-App publication. Unsupported or uninitialized Apps stay
untouched, and an empty applicable catalog does not open native files.

Each App owns one database transaction, shared file lock and native observation.
Its entries are prepared in Store order and published at most once. The existing
native bindings serve single-entry operations and batches; the per-entry public
sync loop is removed. A bad later entry cannot publish a valid prefix. Each App
retains only one whole-file recovery record rather than one per catalog row.
Catalog rows, unknown fields and other consumers' native links are not rewritten.

Native-link persistence and commit share the existing completion/recovery code
with selection, deletion and upsert. A failed App restores its own observation,
including earlier committed writes when paths alias. Other Apps still run and
their successful changes remain committed; this is not an all-App transaction.
Recovery conflicts preserve external edits and remain visible in the aggregate
error. Database and filesystem locks stay held through compensation. No AppState
lock is taken while those locks are held.

Per-App parsing, conversion, activation and unmanaged-field policies remain in
place. Claude/Gemini removal snapshots exchange with Lite, while Codex/Hermes
sync does not force native activation. Codex's tolerant null-entry conversion is
unchanged. An unchanged removal snapshot keeps its exact stored representation,
including unknown envelope fields; restoring its entry still clears the snapshot.
Hermes retains safe-YAML checks and its backup policy, now with one
pre-publication backup per batch. Large host documents do not acquire Core's
bounded whole-document limits.

Two tests fail against CLI `44eeb266` before adoption: fresh peer rows are omitted
and a later invalid Claude entry leaves a published prefix. Acceptance also covers
five native bindings, one-write batches, empty/uninitialized catalogs, targeted
versus enabled-only removal, unknown snapshots, native-link/verification/commit
failures, uncertain publication, external recovery conflicts and aliased paths.
Real Lite tests check snapshot exchange, fresh peer preservation and writer
exclusion through commit/recovery. The obsolete provider-side MCP hook test is
removed; bulk lock assertions now use the publication path actually exercised.

The provider-owned `sync_snapshot_with_operation` path remains separate because
its caller already owns the database and native operation. Its behavior and
provider/Skill tails are not migrated here. No Core API, dependency pin, schema,
Lite source, UI, proxy or full-product change is needed. Cooperative compensation
does not promise crash-proof atomicity; parent directories and recovery backups
may remain after failure. CLI stays on its migration branch.

Local acceptance against CLI `44eeb266` plus this slice and unchanged Lite
`4d0a77b3` passes 259 ordinary MCP-related tests, 57 real-consumer MCP tests,
14 MCP command tests, 543 config-related tests, 160 database-related tests,
25 Gemini provider tests, 11 Gemini operation tests, 3 real Gemini provider tests
and 121 Lite tests (some suites overlap). Formatting and locked all-target Clippy
pass with existing warnings and the unrelated `reversed_empty_ranges` allowance;
none points at a changed source file.

The wider provider command suite passes 26 tests and fails 3 usage-query tests.
All three fail with the same official-provider template diagnostic on unchanged
CLI `44eeb266`; they are recorded baseline failures, not fixed in this MCP slice.
Windows, abrupt-process recovery and noncooperating-writer races were not run.

#### MCP inside an ordinary provider switch

The public sync gate does not cover `sync_snapshot_with_operation`. Ordinary
Gemini switching already owns a provider transaction, the shared filesystem lock
and a Gemini native operation when it calls that helper. Starting the public
sync's independent transaction or reacquiring its native lock here is not valid.
The embedded MCP work must share the caller's eventual commit or recovery.

This migration has three reviewed steps:

1. Add opt-in acceptance tests through the real provider entry point and Lite
   service. Prove native removal snapshots survive across consumers and that
   both later MCP errors and failed provider commit recover every touched App.
   This step changes only tests and the plan, not production code or pins.
2. Establish shared transaction composition and native recovery ownership.
   Reuse Core/Store APIs where they enforce the required invariants; any new API
   must support caller-owned transactions without Gemini, CLI, UI or proxy policy.
   Validate success, failure, later writes and final commit, including a synthetic
   composite consumer suitable for future full-desktop integration. Do not merely
   nest the public sync entry point or expose an unchecked transaction escape.
3. Adopt that contract in the ordinary Gemini provider MCP tail and remove the
   replaced per-entry path. Preserve initialized-App support, error aggregation,
   native formats, unknown fields, shared snapshots and existing provider auth
   behavior. Keep receipts and locks through the caller's final decision, including
   aliased native paths. Verify real CLI/Lite interchange and existing regressions.

Each step requires two fresh independent blind reviews and cache cleanup. The
whole migration remains incomplete until the opt-in gates pass through the
provider-owned path. An ignored test or a passing public-sync control is not
evidence that the embedded path works.

Unlike independent public per-App synchronization, an abandoned provider
transaction must recover all native MCP changes it owns. Later Apps still receive
attempts when an earlier App fails, before aggregate failure triggers recovery.
This does not make filesystem/SQLite updates crash-atomic or change the existing
host-settings and best-effort Skill tail boundaries. Force writes, takeover/proxy,
other provider workflows, UI, schema redesign and full-product adoption remain
outside this slice. CLI changes stay on its migration branch.

The first-step gates are in `services::mcp::provider_sync_tests`. Run them with
the independently built Lite test binary and isolated environment described above:

```sh
cargo test --locked --lib services::mcp::provider_sync_tests \
  -- --ignored --test-threads=1
```

Against unchanged production CLI `fce63d97` (Core/Store `1abff9e8`) and Lite
`4d0a77b3` (Core/Store `7b7cfae5`), all three gates fail at their intended final
assertions. A later MCP error leaves Claude/OpenCode writes behind. A deferred
provider commit failure leaves Claude/Codex/OpenCode/Hermes writes behind, even
though provider selection, cache and Gemini files recover. The no-failure control
successfully projects the target to all five Apps before the failed-commit case.
The failed case also observes every native file at its own COMMIT preparation,
compares those bytes with the successful control and requires the deferred
foreign-key diagnostic before checking recovery.

The real Lite test first passes through public targeted sync, then fails through
an ordinary provider switch with an otherwise equivalent profile. Lite can enable
the removed entry in both cases, but its native `trust` extension is missing only
after provider-owned removal. Both cases retain a fresh peer committed by Lite.
No real commands or credentials are used. These failures are pending integration
requirements, not accepted behavior or a reason to weaken the assertions. Remove
the temporary ignore on the two single-repository gates after adoption; the
cross-repository gate remains opt-in and must also pass explicitly.

The test-only step also passes 259 existing MCP tests, 57 existing real-Lite MCP
gates, 25 Gemini provider tests, 11 Gemini operation tests and 121 unchanged Lite
tests. Formatting and locked all-target Clippy pass with the previously recorded
baseline lint warnings. The three expected failures above are reported separately
and do not count as passing coverage.

#### Provider-owned MCP adoption

CLI now pins Core/Store to `f5b6b4cd` and transfers its ordinary Gemini switch
transaction to the shared provider/MCP guard before publishing files. Provider
settings and selection, MCP catalog checks and native removal snapshots share
one final commit. Host extension settings are read before transfer; the caller
does not regain an unchecked connection afterward. Unknown provider fields,
catalog columns, unsupported App selections and unrelated links remain intact.
This uses the shared contract established in Core PR #18; it adds no Core API.

The old per-entry provider MCP path is removed. Public sync and the provider tail
reuse one batch/link implementation with the existing App-specific native bindings.
Each initialized App receives at most one MCP publication. Later Apps are still
attempted after an earlier native error, and their errors are aggregated. Public
sync keeps its independent per-App commits; a failed provider switch instead
recovers every native publication it owns before rolling back the database.

Bindings and locks remain held through the final decision. Gemini's provider and
MCP publications retain separate Core receipts so all Apps can recover in reverse
publication order, including leaf links that refer to Gemini from earlier or later
Apps. Recovery remains conditional and preserves external edits. SQLite-triggered
transaction aborts release the database lock themselves; the shared native lock
still spans recovery. Local selection/authentication and the post-commit best-effort
Skill tail retain their existing boundaries. This is not crash-atomic execution.

The original three provider-entry gates now pass against unchanged Lite `4d0a77b3`
(Core/Store `7b7cfae5`), including the actual remove/enable snapshot round trip.
The two single-repository gates are no longer ignored. Added cases check shared
and database protection during every publication/recovery, suppressed writes,
provider-row drift, deferred commit failure, SQLite abort, uncertain publication,
external edits, linked paths and real Lite exclusion/release. The large-document
test checks one MCP publication for 64 entries, not one write per entry.

Local validation passes 264 ordinary MCP-filtered tests, 59 real-Lite MCP gates,
29 Gemini provider tests including real consumers, 11 Gemini operation tests,
139 database tests, 14 MCP command tests, 71 provider service tests, one provider
model round-trip test, one provider-selection settings test and 121 Lite tests.
Some suites overlap. Formatting and locked all-target Clippy pass with the recorded
baseline warnings and unrelated `reversed_empty_ranges` allowance; no warning
points at a changed source file. Windows was not run locally.

Only the CLI migration branch adopts this workflow. Lite source/pins, other provider
workflows, global snapshot persistence, Skill writers, proxy, UI and full-product
adoption are unchanged. Shared-Core migration remains incomplete outside these
verified paths. No CLI merge to `main` is authorized.

## Skill shared-consumer adoption

Skill catalog DAOs already use Store, but that does not establish that a whole
Skill operation coordinates its catalog observation, native deployment and final
decision. Continue in four separately reviewed slices:

1. **Establish real-consumer gates.** Use CLI's public toggle and Lite's actual
   Skill Store/native services in marked temporary profiles. Verify same-row
   selection retention, native restoration after database rejection, shared-lock
   exclusion/release and Lite's protection at commit-failure recovery entry.
   This slice adds tests, test-only observation points and this plan. It does not
   change production behavior, dependencies or schema.
2. **Confirm the shared contract.** Reuse Core's Skill registry, runtime, plans,
   receipts and Store's guarded catalog writes. Add a shared primitive only when
   the gates establish that the existing API cannot express the required lifetime
   or write set. Keep CLI/Lite settings, path resolution and deployment choices
   in their hosts. A future full-product host must not need CLI or Lite types.
3. **Adopt the complete toggle workflow.** Use fresh guarded observations and
   retain database/native protection through commit or compensation. Pass the
   gates with actual consumers, preserve unowned fields and other App selections,
   and extend coverage to every supported Skill App and deployment/storage mode
   before making an all-App claim. Add real-peer probes during CLI publication,
   commit and compensation, plus observations inside and at the end of Lite
   recovery; entry-only checks do not establish the full lock lifetime. Delete
   replaced toggle code, not unrelated installation or repository features.
4. **Migrate the remaining Skill writers.** Inventory set-apps, install/reuse,
   import, removal, synchronization and storage migration separately; migrate
   shared planning/execution and remove their replaced implementations in bounded
   follow-up changes. Keep repository acquisition, CLI presentation and product
   policy outside Core. A green toggle alone does not complete Skill adoption.

Each slice follows the independent two-reviewer protocol above. Production
provider/MCP workflows, proxy, UI, market features and full-product integration
are outside the first Skill slice. No normal app or real profile is started.

The CLI tests live under `services::skill::consumer_tests`. The ordinary
round-trip control covers Auto/Copy, repeated enable/disable, an unrelated row,
unknown binary data, an unsupported App selection and host metadata. The explicit
database-rejection gate exercises both enable and disable in both modes. The
same-row peer gate accepts either a completed Lite Gemini toggle that CLI must
preserve, or a specifically identified lock/busy refusal followed by successful
retry after CLI finishes; arbitrary peer errors do not qualify. The native-holder
gate deliberately holds no database write transaction, so a SQLite busy error
alone cannot establish file-lock participation.

Build independent CLI/Lite library test binaries as described above. With the
isolated environment and `CC_SWITCH_LITE_TEST_BINARY` set, run:

```sh
cargo test --locked --lib services::skill::consumer_tests -- --include-ignored --test-threads=1
```

From Lite's `src-tauri/`, run its recovery tests with the same
owned temporary `TMPDIR` and `CC_SWITCH_CONFIG_DIR`. The standalone test and the
Skill peer fix every App path under their checked temporary home, including
subsequent Skill runtime observations; ambient App overrides are not used:

```sh
cargo test --locked --lib skill::acceptance_tests -- --test-threads=1
```

The initial Lite gate used an actual deferred foreign-key failure and the real
native receipt. It recorded lock observations immediately before native recovery,
let restoration and retry finish, then checked that entry observation. It did not
prove protection inside or at the end of recovery. The CLI holder gate likewise
does not observe CLI-owned publication/commit/compensation. Those full-lifecycle
gates remain required in step 3 above. Acceptance failures
remain explicit pending requirements, not passing coverage. Remove the temporary
ignore on single-repository gates when their production adoption passes; real
cross-repository workers remain opt-in and must still be run explicitly.

### Initial Skill observations

With these test-only additions on CLI `20930a9f` (Core/Store `f5b6b4cd`) and Lite
`4d0a77b3` (Core/Store `7b7cfae5`), all four acceptance failures are reproduced:

- Lite commits Gemini's selection for the target Skill after CLI reads its index.
  CLI's Claude toggle succeeds and leaves both native Skills reachable, but
  overwrites the peer's Gemini catalog selection back to false.
- Lite holds a real Gemini native Skill receipt without a database write lock.
  CLI's Claude toggle still succeeds. Lite recovery and subsequent retries succeed;
  the gate fails because CLI did not refuse the protected native write.
- A selection-rejecting SQLite trigger makes CLI return a conflict and preserves
  the catalog, but leaves native changes in all four Auto/Copy enable/disable
  cases. Removing the trigger permits successful retries.
- Lite's deferred foreign-key failure reaches native recovery with the file lock
  held but the database write lock released. Restoration and retry finish; the
  gate then fails at its recorded lock-lifetime assertion.

The fixture starts Gemini's disabled list in agreement with its catalog flags;
it does not suppress Core's drift checks. Core may retain a disabled protected
public link, so the holder test checks that no Skill remains reachable after
recovery instead of requiring that internal reference to disappear. These gates
do not yet establish all-App, copy/link interoperability or crash recovery.

Local regression results: 54 CLI Skill tests pass, 139 database tests pass,
28 Skill service integration tests pass, and all 121 ordinary Lite tests pass.
The existing CLI `migration_tree_hash_preserves_non_utf8_names` test fails during
fixture creation with macOS error 92 (`Illegal byte sequence`), before calling
project logic. Its source is unchanged from the baseline; an isolated rerun has
the same result. It is neither fixed nor counted as passing here. Ordinary runs
ignore the explicit acceptance gates and real-consumer workers; those four
failures above are reported separately.

Both format checks and locked all-target Clippy pass. Lite uses `-D warnings`;
CLI retains its 63 baseline warnings and the previously recorded unrelated
`reversed_empty_ranges` allowance, with no new warning on the added test code.
Windows and the complete CLI test suite were not run locally.

The standalone Lite gate was also run with `CLAUDE_CONFIG_DIR` pointing at a
different owned temporary path. It reached the expected lock-entry assertion,
and the overridden path remained absent; no outside Skill was published.

### Shared contract check and Lite recovery adoption

Core's existing Skill runtime, plans and receipts, plus Store's column-scoped
catalog writes, already express the transaction lifetime needed by Lite. No new
Core API or dependency pin is required for this correction. The host owns the
database transaction and retains it until its Core native receipt has recovered.
A failed COMMIT with an active SQLite transaction is not an uncertain commit:
recover native state first, then roll back the database. If COMMIT reports an
error after SQLite has ended the transaction, keep the existing committed-catalog
decision path and native lock. Do not claim that an automatically aborted
database lock can still be retained.

Lite now keeps that transaction guard after statement or COMMIT failure. A native
recovery error does not skip the database rollback; the operation and native
recovery failures are both reported and
external native edits are preserved. Its ordinary recovery tests cover Claude
and Gemini, enable and disable, deferred COMMIT failure, statement ABORT, full
transaction ROLLBACK, release/retry, unchanged catalog rows/opaque fields, exact
Gemini configuration restoration and an external edit that prevents recovery.
Observations bracket the real Core rollback while its native lock remains owned;
they do not cover every filesystem instruction or the whole successful workflow.
The replacement recovery tests run in the ordinary suite. The extended tests failed
on the pre-fix production code and pass after this host-lifetime correction.

CLI deployment remains a separate shared-contract task. Its Auto mode preserves
existing copies and can fall back to copying; explicit Copy and Symlink modes
also remain supported. Core's current protected-reference implementation cannot
replace those operations unchanged. Define typed, product-neutral deployment and
ownership choices before adopting CLI toggles; do not silently convert copies to
links or wrap the old CLI planner and call that completed Core migration. The
intended full-product API must admit these choices without CLI/Lite types. No
full-product caller was inspected or changed; its compatibility remains unverified.

This slice changes only Lite Skill recovery and these acceptance records. Core,
CLI production code, provider/MCP workflows, UI, schema, pins and deployment modes
are unchanged. The three CLI Skill gates remain unresolved; all-App deployment,
successful-workflow lock lifetime and the remaining Skill writers still require
their planned adoption and independent reviews.

Local validation: 125 ordinary Lite tests pass (six opt-in workers remain ignored),
including 13 recovery cases in four tests. Formatting and locked all-target
Clippy with `-D warnings` pass. The four recovery tests also pass with Claude and
Windows AppData overrides pointing at separate owned paths; neither is created.
Against this Lite build, CLI's control passes and its three known Skill acceptance
failures remain reproducible. Core/Store pins remain Lite `7b7cfae5` and CLI
`f5b6b4cd`; the Skill implementation is identical at those Core revisions.
Windows and the full CLI suite were not run locally.

### Deployment composition follow-up

The next slices follow Core's
[Skill deployment plan](https://github.com/SaladDay/cc-switch-core/blob/main/docs/skill-deployment-composition.md).
It separates directory deployment from native controls, requires representation
and ownership compatibility, and starts production adoption with Claude toggles
in all three CLI deployment modes and real Lite interoperability. This is a plan,
not a passing gate: the three CLI failures above remain open. CLI work stays on
`refactor/core-migration-validation`; it must not be merged into `main`.

### Deployment representation baseline

The first composition gate adds tests only, against CLI `f9a06ffc` and Lite
`a69bfb2c` production code. Core/Store pins remain CLI `f5b6b4cd` and Lite
`7b7cfae5`. No native writer, dependency, ownership format, schema or UI changes.

Three ordinary CLI tests cover 36 fixture cases: Claude/Gemini/Hermes/Pi with
Auto/Copy/Symlink, fresh deployment and existing-copy refresh, repeated requests,
missing-source enable/removal, and Pi's externally modified copy protection.
For non-Pi existing copies, both source files change before the repeated enable;
the deployed content must refresh, retaining a copy under Auto/Copy.
They inspect directory/link type, exact link target, manifest and binary asset,
catalog flags, unknown fields and retained source data. Gemini/Hermes documents
must remain byte-identical after each toggle, even when native controls disagree
with the requested catalog flag. This records CLI's directory-only behavior; it
does not claim effective native enablement. Symlink cases need a filesystem and
account that permit directory symlinks. Auto's failure fallback is not forced.

Two opt-in tests cover nine Claude interoperation cases. CLI-first exercises all
three modes. Lite-first starts with either its enabled reference or its disabled,
dangling reference, then asks CLI to enable in each mode. Lite must subsequently
disable and reenable the same installed Skill; a copied deployment must remain a
copy. The peer runs Lite's real Skill Store and observation service and returns
both the operation error and observed state. Successful worker exit alone is not
a successful toggle. The existing bounded child wait/reap and isolated paths are
reused; all profiles and credentials are synthetic.

Run the new tests with the independent binaries and isolated environment above:

```sh
cargo test --locked --lib services::skill::consumer_tests::deployment \
  -- --include-ignored --test-threads=1 --nocapture
```

Local macOS results: all 36 ordinary cases pass. All nine cross-consumer cases
reach Lite's real disable service and fail with `skill_unavailable` /
`NativeConflict`; catalog selection remains true, effective state is unknown,
and Lite preserves the native entry and catalog on refusal. Thus both opt-in
tests are red. Reenable/copy-retention assertions remain requirements but are
not reached on this baseline. Running the whole Skill consumer group gives
four passing tests and five failures, including the three previously recorded
failures. These assertions must not be inverted to make the gate green.

Lite's 125 ordinary tests and strict all-target Clippy pass. Both formatting
checks pass. CLI all-target Clippy passes with its existing warnings and recorded
`reversed_empty_ranges` allowance; no diagnostic names a changed test file.
Windows/Linux execution, the full CLI suite, crash recovery and all-App
interoperation were not tested in this slice. The next production slice must
resolve representation ownership and deployment composition, not just locking.
