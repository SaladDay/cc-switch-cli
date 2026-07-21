# Architecture

**Analysis Date:** 2026-06-12

## Pattern Overview

**Overall:** Layered Rust CLI with SQLite-backed single source of truth, live app-config adapters, an async HTTP proxy subsystem, and a Unix supervisor daemon for managed proxy workers.

**Key Characteristics:**
- `src-tauri/src/main.rs` is a thin binary entry point: parse `Cli`, initialize logging, run startup state recovery for most commands, then dispatch into `src-tauri/src/cli/commands/`.
- `src-tauri/src/store.rs` centralizes durable runtime state in `AppState`, combining `Database`, an in-memory `MultiAppConfig` snapshot, and `ProxyService`.
- `src-tauri/src/database/mod.rs` owns the SQLite connection and schema lifecycle; DAO methods under `src-tauri/src/database/dao/` are the persistence boundary.
- `src-tauri/src/services/` holds durable business logic shared by direct CLI commands and the ratatui TUI.
- `src-tauri/src/proxy/` runs an Axum-based multi-app proxy with request routing, provider failover, model-based routing, response transforms, usage logging, and provider-specific adapters.
- `src-tauri/src/daemon/` supervises proxy worker processes on Unix and exposes JSON IPC over a Unix domain socket.

## Layers

**Binary Entry Layer:**
- Purpose: Parse process arguments, set logging behavior, decide whether startup state is required, and dispatch commands.
- Location: `src-tauri/src/main.rs`
- Contains: `main()`, `run()`, `command_requires_startup_state()`, `initialize_startup_state_if_needed()`
- Depends on: `src-tauri/src/cli/mod.rs`, `src-tauri/src/store.rs`, `src-tauri/src/error.rs`
- Used by: The `cc-switch` binary target declared in `src-tauri/Cargo.toml`

**CLI Command Layer:**
- Purpose: Define the Clap command surface and perform terminal command I/O.
- Location: `src-tauri/src/cli/`
- Contains: Top-level `Cli` and `Commands` in `src-tauri/src/cli/mod.rs`; command implementations in `src-tauri/src/cli/commands/`
- Depends on: `src-tauri/src/services/`, `src-tauri/src/store.rs`, `src-tauri/src/app_config.rs`, `src-tauri/src/provider.rs`
- Used by: `src-tauri/src/main.rs` and integration tests under `src-tauri/tests/`
- Pattern: Add a user-facing command by defining Clap shape in `src-tauri/src/cli/mod.rs` or a command module, implementing command I/O in `src-tauri/src/cli/commands/`, and moving reusable behavior into `src-tauri/src/services/`.

**Interactive TUI Layer:**
- Purpose: Provide interactive provider, MCP, prompt, skill, proxy, usage, session, pricing, and config workflows.
- Location: `src-tauri/src/cli/tui/`
- Contains: Event loop in `src-tauri/src/cli/tui/mod.rs`, state in `src-tauri/src/cli/tui/app/`, rendering in `src-tauri/src/cli/tui/ui/`, forms in `src-tauri/src/cli/tui/form/`, runtime actions in `src-tauri/src/cli/tui/runtime_actions/`, background systems in `src-tauri/src/cli/tui/runtime_systems/`
- Depends on: `src-tauri/src/services/`, `src-tauri/src/database/`, `src-tauri/src/proxy/`, `src-tauri/src/cli/ui/`
- Used by: `cc-switch` with no subcommand or `cc-switch interactive`

**Application State Layer:**
- Purpose: Coordinate database state, legacy migration, in-memory config snapshots, startup recovery, and proxy service construction.
- Location: `src-tauri/src/store.rs`
- Contains: `AppState`, `try_new()`, `try_new_with_startup_recovery()`, `try_open_snapshot()`, `save()`, live-provider import and recovery helpers
- Depends on: `src-tauri/src/database/`, `src-tauri/src/app_config.rs`, `src-tauri/src/services/proxy.rs`, live config adapters such as `src-tauri/src/codex_config.rs`
- Used by: Most CLI commands, TUI actions, startup initialization, integration tests

**Service Layer:**
- Purpose: Hold business logic that should not be tied to terminal prompts or rendering.
- Location: `src-tauri/src/services/`
- Contains: `ProviderService`, `McpService`, `PromptService`, `SkillService`, `ProxyService`, `ConfigService`, auth services, usage services, WebDAV sync, environment checks, speed tests, stream checks
- Depends on: `src-tauri/src/database/`, live config adapter modules, external HTTP/auth libraries where needed
- Used by: `src-tauri/src/cli/commands/`, `src-tauri/src/cli/tui/runtime_actions/`, startup recovery in `src-tauri/src/store.rs`
- Pattern: Put shared command/TUI behavior here; keep direct user prompts and formatted output in `src-tauri/src/cli/`.

**Persistence Layer:**
- Purpose: Persist providers, MCP servers, prompts, skills, settings, proxy state, failover queues, usage rollups, stream checks, model pricing, universal providers, and model routes.
- Location: `src-tauri/src/database/`
- Contains: `Database` in `src-tauri/src/database/mod.rs`, schema/migration in `src-tauri/src/database/schema.rs` and `src-tauri/src/database/migration.rs`, DAOs in `src-tauri/src/database/dao/`
- Depends on: `rusqlite`, `serde_json`, domain models such as `src-tauri/src/provider.rs` and `src-tauri/src/model_route.rs`
- Used by: `AppState`, services, proxy runtime, daemon, integration tests
- Pattern: Add new persisted entities as domain model plus DAO methods under `src-tauri/src/database/dao/`; update schema creation/migration in `src-tauri/src/database/schema.rs`.

**Live Config Adapter Layer:**
- Purpose: Translate CC-Switch database/provider state to and from supported assistant app config files.
- Location: `src-tauri/src/*_config.rs`, `src-tauri/src/*_mcp.rs`, `src-tauri/src/prompt_files.rs`
- Contains: App-specific adapters including `src-tauri/src/codex_config.rs`, `src-tauri/src/gemini_config.rs`, `src-tauri/src/opencode_config.rs`, `src-tauri/src/openclaw_config.rs`, `src-tauri/src/hermes_config.rs`, `src-tauri/src/claude_mcp.rs`, `src-tauri/src/gemini_mcp.rs`
- Depends on: `src-tauri/src/config.rs`, `src-tauri/src/app_config.rs`, `src-tauri/src/provider.rs`
- Used by: Provider, MCP, prompt, proxy takeover, and startup recovery workflows

**Proxy Runtime Layer:**
- Purpose: Accept local HTTP traffic, select providers, transform requests/responses, forward upstream calls, track usage, and maintain live proxy status.
- Location: `src-tauri/src/proxy/`
- Contains: Axum server in `src-tauri/src/proxy/server.rs`, route handlers in `src-tauri/src/proxy/handlers.rs`, per-request context in `src-tauri/src/proxy/handler_context.rs`, failover routing in `src-tauri/src/proxy/provider_router.rs`, model routing in `src-tauri/src/proxy/model_router.rs`, forwarding in `src-tauri/src/proxy/forwarder.rs`, adapters in `src-tauri/src/proxy/providers/`
- Depends on: `src-tauri/src/database/`, `src-tauri/src/provider.rs`, `src-tauri/src/model_route.rs`, `reqwest`, `axum`, `tokio`
- Used by: `ProxyService`, proxy CLI commands, daemon worker processes, proxy tests

**Daemon Layer:**
- Purpose: Own and supervise proxy worker processes, align worker state with persisted proxy state, and expose foreground control over IPC.
- Location: `src-tauri/src/daemon/`
- Contains: `run()` and `notify_global_switch()` in `src-tauri/src/daemon/mod.rs`, supervisor in `src-tauri/src/daemon/supervisor.rs`, IPC protocol/client/server in `src-tauri/src/daemon/ipc/`, pidfile and path helpers
- Depends on: `src-tauri/src/database/`, `src-tauri/src/services/proxy.rs`, Unix sockets and Tokio
- Used by: Unix `daemon` and proxy-management commands

**Library Command Layer:**
- Purpose: Expose non-Clap command helpers for embedded or app-like callers.
- Location: `src-tauri/src/commands/`
- Contains: OpenClaw workspace file and daily memory operations in `src-tauri/src/commands/workspace.rs`
- Depends on: `src-tauri/src/openclaw_config.rs`, `src-tauri/src/config.rs`
- Used by: Library callers and tests; not wired as normal Clap subcommands

**Deep Link Layer:**
- Purpose: Parse and import `ccswitch://v1/import?...` resources.
- Location: `src-tauri/src/deeplink/`
- Contains: Request model in `src-tauri/src/deeplink/mod.rs`, URL parsing in `src-tauri/src/deeplink/parser.rs`, provider import in `src-tauri/src/deeplink/provider.rs`
- Depends on: `src-tauri/src/provider.rs`, `src-tauri/src/app_config.rs`, `serde_json`
- Used by: Public library exports from `src-tauri/src/lib.rs` and deep-link integration tests

## Data Flow

**Normal CLI Command Startup:**

1. `src-tauri/src/main.rs` parses arguments into `Cli` from `src-tauri/src/cli/mod.rs`.
2. `command_requires_startup_state()` skips startup state for commands such as `update`, `auth`, `sessions`, `completions`, `internal`, and Unix `daemon`.
3. Most commands call `AppState::try_new_with_startup_recovery()` in `src-tauri/src/store.rs`.
4. `AppState` initializes `Database`, migrates legacy `config.json`/`skills.json` if needed, exports DB rows into `MultiAppConfig`, imports live provider configs where appropriate, recovers proxy takeovers, migrates Codex history buckets, and syncs session usage.
5. `run()` dispatches to a command module under `src-tauri/src/cli/commands/`.
6. Command modules call service-layer methods and print command-specific output.

**Provider Switch:**

1. Provider commands in `src-tauri/src/cli/commands/provider.rs` or TUI actions in `src-tauri/src/cli/tui/runtime_actions/providers.rs` collect a target provider and app.
2. `ProviderService` in `src-tauri/src/services/provider/` validates and persists current-provider state through `Database`.
3. Live config adapters such as `src-tauri/src/codex_config.rs`, `src-tauri/src/gemini_config.rs`, and `src-tauri/src/opencode_config.rs` write app-specific config files when that app is initialized and the workflow requires live sync.
4. Proxy/takeover paths coordinate with `ProxyService` in `src-tauri/src/services/proxy.rs` to update backup or takeover state.
5. `AppState::refresh_config_from_db()` keeps the in-memory `MultiAppConfig` snapshot aligned after DB changes.

**Proxy Request Routing:**

1. `ProxyServer` in `src-tauri/src/proxy/server.rs` routes HTTP endpoints to handlers in `src-tauri/src/proxy/handlers.rs`.
2. `HandlerContext::load()` in `src-tauri/src/proxy/handler_context.rs` records request start, extracts the requested `model`, reads app proxy and optimizer settings, and builds a provider candidate list.
3. `ModelRouter` in `src-tauri/src/proxy/model_router.rs` first checks enabled `ModelRoute` records from `src-tauri/src/model_route.rs`; a match selects a single route-targeted provider and records route hit count.
4. If no model route matches, `ProviderRouter` in `src-tauri/src/proxy/provider_router.rs` selects the current provider or the failover queue, respecting circuit-breaker state.
5. `RequestForwarder` in `src-tauri/src/proxy/forwarder.rs` sends the request to upstream providers through provider adapters under `src-tauri/src/proxy/providers/`.
6. Response helpers in `src-tauri/src/proxy/response.rs` and `src-tauri/src/proxy/response_handler.rs` build buffered, passthrough, transformed, SSE, or error responses.
7. Usage logging under `src-tauri/src/proxy/usage/` records request metadata and costs, and successful failover updates current provider state.

**Daemon-Managed Proxy:**

1. CLI daemon commands in `src-tauri/src/cli/commands/daemon.rs` call daemon entry points in `src-tauri/src/daemon/mod.rs`.
2. `daemon::run()` opens `Database`, starts periodic usage maintenance, creates `Supervisor`, runs startup recovery, binds the Unix IPC socket, and handles shutdown signals.
3. Foreground commands use `src-tauri/src/daemon/ipc/client.rs` to send requests defined in `src-tauri/src/daemon/ipc/protocol.rs`.
4. `src-tauri/src/daemon/supervisor.rs` starts, stops, restarts, and monitors proxy worker processes according to persisted desired state.
5. `ProxyService` stores runtime-session metadata so foreground/TUI processes can probe or recover managed proxy workers.

**OpenClaw Workspace File Flow:**

1. Callers use helper functions in `src-tauri/src/commands/workspace.rs`.
2. Filenames are restricted to `ALLOWED_FILES` or daily memory filename patterns.
3. The module validates workspace roots, rejects symlinks/path traversal, and reads/writes through helpers from `src-tauri/src/config.rs`.
4. Tests for this behavior live in `src-tauri/tests/workspace_commands.rs`.

**State Management:**
- SQLite at `cc-switch.db` is the durable source of truth, accessed through `Database` in `src-tauri/src/database/mod.rs`.
- `MultiAppConfig` in `src-tauri/src/app_config.rs` is an in-memory snapshot used by command and service code; refresh it from DB after persistence changes.
- `settings.json` values accessed through `src-tauri/src/settings.rs` hold app settings and some current-provider/UI integration settings.
- Proxy runtime state uses async `RwLock` fields in `ProxyServerState` (`src-tauri/src/proxy/server.rs`) and per-database shared runtime state in `ProxyService` (`src-tauri/src/services/proxy.rs`).

## Key Abstractions

**`AppState`:**
- Purpose: Process-local coordination object for DB, in-memory config, and proxy service.
- Examples: `src-tauri/src/store.rs`, public export in `src-tauri/src/lib.rs`
- Pattern: Construct with `try_new_with_startup_recovery()` on normal process startup; use `try_open_snapshot()` for read-only TUI refresh paths.

**`Database`:**
- Purpose: SQLite connection wrapper, schema migrator, backup manager, and DAO host.
- Examples: `src-tauri/src/database/mod.rs`, `src-tauri/src/database/schema.rs`, `src-tauri/src/database/dao/providers.rs`, `src-tauri/src/database/dao/model_routes.rs`
- Pattern: Keep direct SQL in DAO/schema modules; call `Database` methods from services and proxy code.

**`MultiAppConfig` and `AppType`:**
- Purpose: Shared in-memory representation of supported apps and their provider/MCP/prompt/skill state.
- Examples: `src-tauri/src/app_config.rs`, `src-tauri/src/store.rs`
- Pattern: Use `AppType` labels (`claude`, `codex`, `gemini`, `opencode`, `hermes`, `openclaw`) at command boundaries and database rows.

**`Provider` and Provider Services:**
- Purpose: Represent provider config, metadata, usage scripts, current-provider selection, import/export, and live sync.
- Examples: `src-tauri/src/provider.rs`, `src-tauri/src/services/provider/mod.rs`, `src-tauri/src/services/provider/live.rs`, `src-tauri/src/cli/commands/provider.rs`
- Pattern: Put provider CRUD/switch semantics in `ProviderService`; keep formatting and prompts in command/TUI modules.

**`ModelRoute` and `ModelRouter`:**
- Purpose: Route proxy requests to provider IDs based on model-name patterns before normal provider/failover routing.
- Examples: `src-tauri/src/model_route.rs`, `src-tauri/src/proxy/model_router.rs`, `src-tauri/src/database/dao/model_routes.rs`, `src-tauri/src/proxy/handler_context.rs`
- Pattern: Routes are app-scoped, enabled/disabled, priority ordered, wildcard-enabled, and record hit counts asynchronously.

**`ProxyService`:**
- Purpose: Manage proxy lifecycle, takeover mode, live config rewrites, runtime session persistence, hot switching, and daemon/foreground coordination.
- Examples: `src-tauri/src/services/proxy.rs`, command surface in `src-tauri/src/cli/commands/proxy.rs`
- Pattern: Use service methods for lifecycle and live-config mutation; `src-tauri/src/proxy/` owns HTTP request processing.

**`ProxyServerState` and `HandlerContext`:**
- Purpose: Carry async proxy runtime state and per-request routing/timeout/config decisions.
- Examples: `src-tauri/src/proxy/server.rs`, `src-tauri/src/proxy/handler_context.rs`
- Pattern: `HandlerContext::load()` is the request setup boundary before forwarding logic.

**Provider Adapters:**
- Purpose: Encapsulate upstream-specific auth, endpoint, schema, streaming, and transform behavior.
- Examples: `src-tauri/src/proxy/providers/adapter.rs`, `src-tauri/src/proxy/providers/claude.rs`, `src-tauri/src/proxy/providers/codex.rs`, `src-tauri/src/proxy/providers/gemini.rs`
- Pattern: Add provider-specific proxy behavior under `src-tauri/src/proxy/providers/`, not in generic handlers.

**TUI Runtime Split:**
- Purpose: Keep interactive state, rendering, forms, actions, and background workers separate.
- Examples: `src-tauri/src/cli/tui/app/`, `src-tauri/src/cli/tui/ui/`, `src-tauri/src/cli/tui/form/`, `src-tauri/src/cli/tui/runtime_actions/`, `src-tauri/src/cli/tui/runtime_systems/`
- Pattern: UI modules render state; runtime action modules mutate state or call services; background systems handle async refresh/workers.

## Entry Points

**Binary Process:**
- Location: `src-tauri/src/main.rs`
- Triggers: Running `cc-switch` or `cargo run` from `src-tauri/`
- Responsibilities: Parse CLI, configure logging, initialize startup state when required, dispatch commands

**Library Root:**
- Location: `src-tauri/src/lib.rs`
- Triggers: Integration tests and external Rust callers importing `cc_switch_lib`
- Responsibilities: Declare internal modules and re-export public types/services such as `AppState`, `Database`, `ProviderService`, `ProxyService`, `ModelRoute`, and deep-link helpers

**CLI Shape:**
- Location: `src-tauri/src/cli/mod.rs`
- Triggers: Clap parsing from `src-tauri/src/main.rs`
- Responsibilities: Define global `--app`, verbose flag, subcommands, shell completion generation

**Command Implementations:**
- Location: `src-tauri/src/cli/commands/`
- Triggers: Dispatch from `src-tauri/src/main.rs`
- Responsibilities: Implement terminal-facing command flows for providers, MCP, prompts, skills, config, proxy, settings, failover, sessions, Hermes, daemon, env, auth, update, completions, and internal commands

**Interactive Mode:**
- Location: `src-tauri/src/cli/interactive/mod.rs`, `src-tauri/src/cli/tui/mod.rs`
- Triggers: No subcommand, `interactive`, or `ui`
- Responsibilities: Start the ratatui event loop and coordinate TUI state/actions/rendering

**Proxy HTTP Server:**
- Location: `src-tauri/src/proxy/server.rs`, `src-tauri/src/proxy/handlers.rs`
- Triggers: Proxy service start or daemon-managed proxy worker
- Responsibilities: Bind Axum routes, serve health/status, handle Claude messages, Codex chat/responses, Gemini passthrough, forwarding, response transformation, and status tracking

**Unix Daemon:**
- Location: `src-tauri/src/daemon/mod.rs`
- Triggers: `cc-switch daemon start` on Unix
- Responsibilities: Own pidfile/socket/logging, supervise worker processes, handle IPC requests, recover desired proxy state

**Deep Link Import:**
- Location: `src-tauri/src/deeplink/`
- Triggers: Library callers/tests parsing `ccswitch://v1/import?...`
- Responsibilities: Parse URL query fields and import provider resources

**OpenClaw Workspace Helpers:**
- Location: `src-tauri/src/commands/workspace.rs`
- Triggers: Library command callers/tests
- Responsibilities: Read/write allowlisted workspace files and daily memory files safely

## Error Handling

**Strategy:** Typed errors at subsystem boundaries, command-level propagation, and explicit proxy error-to-response conversion.

**Patterns:**
- Use `AppError` from `src-tauri/src/error.rs` for application, database, config, and I/O errors outside the proxy HTTP boundary.
- Use `ProxyError` from `src-tauri/src/proxy/error.rs` inside proxy routing/forwarding code and convert to HTTP responses via `src-tauri/src/proxy/response_handler.rs`.
- Command handlers return `Result<(), AppError>` or command-specific `Result` values; `src-tauri/src/main.rs` prints `Error: ...` and exits with status 1.
- Startup recovery logs best-effort failures for non-fatal import/migration/sync work in `src-tauri/src/store.rs`.
- Daemon entry points return `Result<(), String>` in `src-tauri/src/daemon/mod.rs` because IPC/process supervision errors need user-readable messages.

## Cross-Cutting Concerns

**Logging:** `src-tauri/src/main.rs` initializes `env_logger` for normal commands; Unix daemon start uses its own file logger in `src-tauri/src/daemon/logging.rs`; proxy and startup paths use `log` macros.

**Validation:** Clap validates command shapes in `src-tauri/src/cli/mod.rs`; workspace filenames and symlinks are validated in `src-tauri/src/commands/workspace.rs`; live config adapters validate app-specific file formats; database schema version gates are enforced in `src-tauri/src/database/mod.rs`.

**Authentication:** Provider credentials live inside provider settings or managed account flows; auth services are in `src-tauri/src/services/auth.rs`, `src-tauri/src/services/codex_oauth.rs`, and `src-tauri/src/services/copilot_auth.rs`; proxy provider auth strategies are in `src-tauri/src/proxy/providers/auth.rs`.

**Configuration Safety:** Tests and commands that touch live app config paths should use explicit env overrides (`CC_SWITCH_CONFIG_DIR`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `HOME`, `XDG_CONFIG_HOME`, `XDG_RUNTIME_DIR`, `XDG_STATE_HOME`) and helpers from `src-tauri/tests/support.rs` or `src-tauri/src/test_support.rs`.

**Concurrency:** SQLite access is guarded by `Mutex<Connection>` in `src-tauri/src/database/mod.rs`; proxy runtime state uses Tokio `RwLock` in `src-tauri/src/proxy/server.rs` and `src-tauri/src/services/proxy.rs`; model-route hit recording uses `tokio::task::spawn_blocking()` in `src-tauri/src/proxy/model_router.rs`.

---

*Architecture analysis: 2026-06-12*
