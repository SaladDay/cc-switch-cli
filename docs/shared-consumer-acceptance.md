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
