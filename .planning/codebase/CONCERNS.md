# Codebase Concerns

**Analysis Date:** 2026-06-12

## Tech Debt

**Model route test migration is incomplete:**
- Issue: The model-route CLI tests still mix the old integer route ID API with the current UUID/string route ID API. `ModelRouteCommand::{Remove,Toggle,Update}` expects `String`, but several tests pass integers, and several tests use `route_id` without assigning it from `create_model_route()`.
- Files: `src-tauri/src/cli/commands/proxy.rs`
- Impact: The crate does not compile in the current worktree. `rtk cargo test --manifest-path src-tauri/Cargo.toml model_route_remove_deletes_by_id --no-run` fails with 18 errors, including undefined `route_id` at `src-tauri/src/cli/commands/proxy.rs:1104` and integer/string mismatches at `src-tauri/src/cli/commands/proxy.rs:1121`, `src-tauri/src/cli/commands/proxy.rs:1180`, `src-tauri/src/cli/commands/proxy.rs:1213`, `src-tauri/src/cli/commands/proxy.rs:1260`, `src-tauri/src/cli/commands/proxy.rs:1305`, and `src-tauri/src/cli/commands/proxy.rs:1334`.
- Fix approach: Capture the returned `ModelRoute` from `db.create_model_route()`, use `created.id.clone()` as the route ID, and keep update `provider_id` fields as `Option<String>`. Re-run the focused test with `--no-run` before broader test execution.

**Monolithic proxy service:**
- Issue: `src-tauri/src/services/proxy.rs` is 7,085 lines and combines proxy lifecycle management, app takeover, live config restore, runtime status assembly, process cleanup, startup recovery, and many unit tests.
- Files: `src-tauri/src/services/proxy.rs`
- Impact: Proxy changes have a high merge-conflict and regression surface. Related responsibilities are difficult to test independently, and behavior changes can accidentally cross live-config, daemon, and failover boundaries.
- Fix approach: Split durable domains into modules such as lifecycle, takeover, live config sync/restore, runtime status, and process supervision. Keep shared orchestration in `ProxyService`.

**Large TUI and CLI modules:**
- Issue: Several single files carry too much UI state, rendering, or form logic: `src-tauri/src/cli/tui/data.rs` is 4,603 lines, `src-tauri/src/cli/commands/provider_input.rs` is 4,339 lines, and `src-tauri/src/cli/i18n.rs` is 11,384 lines.
- Files: `src-tauri/src/cli/tui/data.rs`, `src-tauri/src/cli/commands/provider_input.rs`, `src-tauri/src/cli/i18n.rs`
- Impact: Small feature work requires navigating large files with mixed concerns. Compile times and review load increase, and narrow UI behavior changes can be hard to isolate.
- Fix approach: Extract TUI data builders by route/domain, split provider input flows by provider/app family, and move i18n test blocks into dedicated test modules while preserving public text helper names.

**Schema version compatibility is encoded as a special case:**
- Issue: `SCHEMA_VERSION` is `11`, while the initialization path explicitly allows database user version `12` as a compatible future/upstream version.
- Files: `src-tauri/src/database/mod.rs`, `src-tauri/src/database/schema.rs`
- Impact: The application can run with a DB version higher than its declared schema version, but only for the hardcoded `12` case. Future upstream schema drift can fail abruptly, and contributors may be unsure whether model-route changes belong to v11 or v12.
- Fix approach: Document the compatibility contract near `SCHEMA_VERSION`, add explicit migration tests for v11 and v12 compatibility, and avoid adding new schema behavior without a corresponding version strategy.

## Known Bugs

**Current worktree fails to compile:**
- Symptoms: `cargo test --no-run` fails before running tests because model-route test code has stale types and missing route ID variables.
- Files: `src-tauri/src/cli/commands/proxy.rs`
- Trigger: Run `rtk cargo test --manifest-path src-tauri/Cargo.toml model_route_remove_deletes_by_id --no-run` from the repository root.
- Workaround: None for the current worktree. Fix the test migration before treating the branch or PR as reviewable.

**PR lookup is not directly discoverable from the proxied remote:**
- Symptoms: `rtk gh pr status` fails because the remote URL uses `https://gh-proxy.com/https://github.com/SaladDay/cc-switch-cli.git`; `rtk gh pr list --repo SaladDay/cc-switch-cli --head feat/model-based-routing --state all --json ...` returned `[]` during this audit.
- Files: `.git/config`
- Trigger: Use GitHub CLI repo inference on this checkout.
- Workaround: Pass an explicit `--repo` for GitHub operations and verify the actual upstream/fork target before making PR-state claims.

## Security Considerations

**Release profile aborts on panic:**
- Risk: Release builds use `panic = "abort"`, so any unrecovered panic terminates the process instead of unwinding a task.
- Files: `src-tauri/Cargo.toml`
- Current mitigation: Many database paths use `Result` and the `lock_conn!` macro instead of direct `unwrap()`, and request handlers generally propagate `ProxyError`.
- Recommendations: Keep panic-prone code out of proxy hot paths. Convert production `unwrap()`/`expect()` sites to structured errors before enabling new proxy features.

**Environment variable mutation uses unsafe blocks:**
- Risk: `std::env::set_var()` and `std::env::remove_var()` are unsafe in current Rust because global process environment mutation can race with concurrent readers.
- Files: `src-tauri/src/main.rs`, `src-tauri/src/config.rs`, `src-tauri/src/cli/commands/completions.rs`
- Current mitigation: Most mutations are scoped to startup or temporary guard patterns.
- Recommendations: Keep env mutation outside concurrent runtime paths. Use explicit config structs for new behavior rather than adding more process-wide environment overrides.

**Dynamic SQL construction requires continued discipline:**
- Risk: SQL fragments are built with `format!()` in usage statistics, backup export, and helper modules. Current usage appears to use hardcoded columns/table names and parameterized user values, but the pattern can become injectable if user-controlled identifiers are added later.
- Files: `src-tauri/src/services/usage_stats.rs`, `src-tauri/src/services/sql_helpers.rs`, `src-tauri/src/database/backup.rs`, `src-tauri/src/database/dao/model_routes.rs`
- Current mitigation: User-facing values are generally bound with `?` parameters or `rusqlite::params![]`.
- Recommendations: Keep dynamic SQL helper inputs as enums/constants, not raw strings. Prefer query builders or explicit match arms when adding filter/sort fields.

## Performance Bottlenecks

**Model route matching does database reads and regex compilation per request:**
- Problem: `ModelRouter::match_route()` calls `db.list_model_routes(app_type)` for every request, then compiles each route pattern into a `Regex` inside the loop.
- Files: `src-tauri/src/proxy/model_router.rs`, `src-tauri/src/proxy/handler_context.rs`
- Cause: Routes are stored as raw patterns and there is no cache for enabled route lists or compiled regexes.
- Improvement path: Cache enabled routes per app with compiled regexes and invalidate on model-route CRUD operations. Keep the database as source of truth but avoid per-request full-list loads.

**Route hit tracking can amplify database contention:**
- Problem: A matched route spawns a blocking task that calls `record_model_route_hit()` for each hit, incrementing `hit_count` and updating `last_hit_at`.
- Files: `src-tauri/src/proxy/model_router.rs`, `src-tauri/src/database/dao/model_routes.rs`, `src-tauri/src/database/mod.rs`
- Cause: `Database` wraps a single `rusqlite::Connection` in `Mutex<Connection>`, so request logging, route hit writes, provider state reads, and config operations contend on one connection.
- Improvement path: Batch hit counters in memory and flush periodically, or use a separate write queue for telemetry updates. Avoid spawning one blocking task per hot-path match under high traffic.

**Large inline test files increase compile cost:**
- Problem: TUI tests are concentrated in very large files: `src-tauri/src/cli/tui/app/tests.rs` is 15,016 lines and `src-tauri/src/cli/tui/ui/tests.rs` is 9,331 lines.
- Files: `src-tauri/src/cli/tui/app/tests.rs`, `src-tauri/src/cli/tui/ui/tests.rs`
- Cause: Route, form, overlay, rendering, and settings tests are grouped by broad module rather than feature area.
- Improvement path: Split tests by domain and keep shared fixtures in support modules. Preserve existing serial filesystem isolation patterns when moving tests.

## Fragile Areas

**Model-routed providers interact with current-provider sync:**
- Files: `src-tauri/src/proxy/handler_context.rs`, `src-tauri/src/proxy/server.rs`, `src-tauri/src/proxy/response_handler.rs`, `src-tauri/src/proxy/model_router.rs`
- Why fragile: `HandlerContext::load()` can select a single provider from a model route, while successful proxy responses later call provider-selection sync paths. If model-routed providers are treated like failover-selected providers, a request can accidentally change the global current provider.
- Safe modification: Preserve tests around `route_source`, `current_provider_id_at_start`, and model-route bypass behavior. Add a regression test that a model-route hit does not mutate DB/settings current provider unless that is explicitly intended.
- Test coverage: Current coverage exists in `src-tauri/src/proxy/handler_context.rs`, but the active worktree does not compile because of `src-tauri/src/cli/commands/proxy.rs` tests.

**Live config and host config safety:**
- Files: `src-tauri/src/store.rs`, `src-tauri/src/services/proxy.rs`, `src-tauri/src/config.rs`, `src-tauri/tests/support.rs`, `src-tauri/src/test_support.rs`
- Why fragile: Startup recovery, proxy takeover, and live config sync can read or write real app configuration unless tests isolate `HOME`, `CC_SWITCH_CONFIG_DIR`, `CLAUDE_CONFIG_DIR`, and `CODEX_HOME`.
- Safe modification: Always use `src-tauri/tests/support.rs` or `src-tauri/src/test_support.rs` helpers for tests touching app config directories. Never run exploratory product commands without explicit temporary config overrides.
- Test coverage: Many integration tests use temp homes, but new CLI or proxy tests need to follow the same pattern.

**SQLite migration and compatibility paths:**
- Files: `src-tauri/src/database/mod.rs`, `src-tauri/src/database/schema.rs`, `src-tauri/src/database/tests.rs`
- Why fragile: `create_tables_on_conn()` creates current tables, migrations mutate old schemas in place, and a special compatible future-version path skips migrations for user version `12`.
- Safe modification: Add targeted migration tests for every schema change. Keep `SCHEMA_VERSION`, migration function names, `PRAGMA user_version`, and compatibility exceptions aligned.
- Test coverage: Existing database tests cover model-route CRUD and migration cases, but current compile failure prevents validating them.

## Scaling Limits

**Single SQLite connection limits concurrent proxy throughput:**
- Current capacity: One `rusqlite::Connection` protected by `Mutex<Connection>` with WAL and a 5-second busy timeout.
- Limit: Concurrent proxy requests serialize whenever they touch the database, including provider lookup, proxy config reads, usage logging, route-hit updates, and health/circuit state changes.
- Scaling path: Introduce a read connection pool or dedicated read-only snapshot path for hot reads, and push telemetry writes through a bounded async queue.

**Per-request route list scans scale with number of routes:**
- Current capacity: Every model-routed request scans all routes for the app in priority order.
- Limit: Matching cost grows linearly with configured model routes and repeats regex compilation work.
- Scaling path: Precompile enabled route patterns and update the cache from model-route CRUD operations.

## Dependencies at Risk

**No dependency vulnerability scan is detected in CI:**
- Risk: `.github/workflows/` contains `benchmark.yml`, `release.yml`, and `rust-ci.yml`, but no detected `cargo audit`, `cargo deny`, advisory, or RustSec check.
- Impact: Vulnerable transitive dependencies can enter releases unnoticed.
- Migration plan: Add `cargo audit` or `cargo deny check advisories` to `rust-ci.yml` and make release workflow depend on it.

**Bundled SQLite requires application releases for SQLite fixes:**
- Risk: `rusqlite` uses the `bundled` feature.
- Impact: SQLite security or correctness fixes require dependency updates and a new cc-switch release rather than relying on system SQLite.
- Migration plan: Keep `rusqlite` current and include it in dependency audit/release checks.

**Embedded JavaScript engine increases review surface:**
- Risk: `rquickjs` brings a JavaScript runtime into the binary for usage-script behavior.
- Impact: Runtime script execution features have a larger security and maintenance surface than pure Rust parsing/configuration.
- Migration plan: Keep script inputs constrained and documented. Prefer declarative config for new usage features unless JS execution is required.

## Missing Critical Features

**No enforced PR-ready verification gate in this checkout:**
- Problem: Current branch has compile errors, and GitHub CLI cannot infer PR state from the proxied remote. No current PR was found for `feat/model-based-routing` under `SaladDay/cc-switch-cli`.
- Blocks: Treating the pushed PR as ready for review or merge.

**No model-route hot-path performance guardrails:**
- Problem: There are functional tests for matching patterns, but no benchmark or stress test for many routes or high request volume.
- Blocks: Confidently scaling model-route routing beyond small user-configured rule sets.

## Test Coverage Gaps

**Model-route CLI tests are stale and currently block compilation:**
- What's not tested: Remove, toggle, and update behavior with UUID/string route IDs.
- Files: `src-tauri/src/cli/commands/proxy.rs`
- Risk: The CLI surface can regress independently of DAO and proxy matching tests.
- Priority: High

**Model-route current-provider sync regression needs explicit coverage:**
- What's not tested: A full handler/response path proving that a successful model-route request does not mutate global current provider when the routed provider differs from the request-start current provider.
- Files: `src-tauri/src/proxy/handler_context.rs`, `src-tauri/src/proxy/server.rs`, `src-tauri/src/proxy/response_handler.rs`
- Risk: Route-specific providers can leak into global app state.
- Priority: High

**Remote PR state is not verified by local automation:**
- What's not tested: Whether the local branch has a matching upstream PR, and whether pushed PR checks pass.
- Files: `.git/config`, `.github/workflows/rust-ci.yml`
- Risk: Local work can be mistaken for a pushed/reviewable PR.
- Priority: Medium

---

*Concerns audit: 2026-06-12*
