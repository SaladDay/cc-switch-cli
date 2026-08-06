# Codebase Structure

**Analysis Date:** 2026-06-12

## Directory Layout

```
cc-switch-cli/
├── AGENTS.md                 # Codex/agent guidance; mirrors CLAUDE.md
├── CLAUDE.md                 # Claude Code project guidance
├── README.md                 # English product documentation
├── README_ZH.md              # Chinese product documentation
├── CHANGELOG.md              # Release notes
├── LICENSE                   # MIT license
├── install.sh                # Install/update shell script
├── flake.nix                 # Nix flake definition
├── flake.lock                # Nix flake lockfile
├── .github/workflows/        # GitHub Actions workflows
├── .planning/codebase/       # Generated GSD codebase map documents
├── assets/                   # Product images, partner banners, screenshots
├── docs/                     # Additional project documentation
├── scripts/                  # Utility/build/release scripts
└── src-tauri/                # Main Rust crate
    ├── Cargo.toml            # Crate manifest for library and binary targets
    ├── Cargo.lock            # Locked Rust dependencies
    ├── rust-toolchain.toml   # Pinned Rust toolchain
    ├── build.rs              # Rust build script
    ├── icons/                # App/tray/platform icons
    ├── updater/              # Updater metadata/configuration
    ├── wix/                  # Windows installer files
    ├── src/                  # Rust source code
    └── tests/                # Integration test targets
```

## Directory Purposes

**Repository Root:**
- Purpose: Documentation, release/install metadata, GitHub workflows, planning artifacts, and the Rust crate wrapper.
- Contains: `README.md`, `README_ZH.md`, `CHANGELOG.md`, `install.sh`, `flake.nix`, `.github/workflows/`, `.planning/codebase/`, `assets/`, `docs/`, `scripts/`
- Key files: `AGENTS.md`, `CLAUDE.md`, `src-tauri/Cargo.toml`

**`src-tauri/`:**
- Purpose: The only Rust crate in the repository; contains both `cc_switch_lib` library and `cc-switch` binary.
- Contains: `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `build.rs`, `src/`, `tests/`, packaging assets
- Key files: `src-tauri/Cargo.toml`, `src-tauri/rust-toolchain.toml`, `src-tauri/src/main.rs`, `src-tauri/src/lib.rs`

**`src-tauri/src/`:**
- Purpose: All production Rust source for CLI, TUI, services, persistence, proxy, daemon, live config adapters, and library helpers.
- Contains: Top-level domain modules plus feature directories such as `cli/`, `services/`, `database/`, `proxy/`, `daemon/`, `deeplink/`, `commands/`, `session_manager/`
- Key files: `src-tauri/src/main.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/store.rs`, `src-tauri/src/app_config.rs`

**`src-tauri/src/cli/`:**
- Purpose: Command-line interface and interactive terminal UI.
- Contains: Clap definitions, command modules, ratatui TUI modules, shared terminal UI helpers, i18n text, editor helpers
- Key files: `src-tauri/src/cli/mod.rs`, `src-tauri/src/cli/interactive/mod.rs`, `src-tauri/src/cli/tui/mod.rs`, `src-tauri/src/cli/ui/table.rs`

**`src-tauri/src/cli/commands/`:**
- Purpose: Direct Clap subcommand implementations.
- Contains: One module per command family plus helper modules.
- Key files: `src-tauri/src/cli/commands/provider.rs`, `src-tauri/src/cli/commands/proxy.rs`, `src-tauri/src/cli/commands/mcp.rs`, `src-tauri/src/cli/commands/prompts.rs`, `src-tauri/src/cli/commands/skills.rs`, `src-tauri/src/cli/commands/config.rs`, `src-tauri/src/cli/commands/daemon.rs`

**`src-tauri/src/cli/tui/`:**
- Purpose: Ratatui interactive UI.
- Contains: Event loop, route/data/theme/help, app state, form definitions, UI renderers, runtime actions, background systems, terminal setup, text editing helpers
- Key files: `src-tauri/src/cli/tui/mod.rs`, `src-tauri/src/cli/tui/app/app_state.rs`, `src-tauri/src/cli/tui/ui/mod.rs`, `src-tauri/src/cli/tui/runtime_actions/mod.rs`, `src-tauri/src/cli/tui/runtime_systems/mod.rs`

**`src-tauri/src/services/`:**
- Purpose: Reusable business logic shared by CLI commands, TUI actions, startup recovery, proxy lifecycle, and environment checks.
- Contains: Provider, MCP, prompt, skill, proxy, auth, WebDAV, usage, speed test, stream check, environment, subscription, and config services.
- Key files: `src-tauri/src/services/mod.rs`, `src-tauri/src/services/provider/mod.rs`, `src-tauri/src/services/proxy.rs`, `src-tauri/src/services/mcp.rs`, `src-tauri/src/services/skill.rs`, `src-tauri/src/services/prompt.rs`

**`src-tauri/src/database/`:**
- Purpose: SQLite persistence layer.
- Contains: `Database` connection wrapper, schema creation/migration, legacy JSON migration, backups, DAO modules, database tests
- Key files: `src-tauri/src/database/mod.rs`, `src-tauri/src/database/schema.rs`, `src-tauri/src/database/migration.rs`, `src-tauri/src/database/backup.rs`, `src-tauri/src/database/dao/mod.rs`

**`src-tauri/src/database/dao/`:**
- Purpose: Data access modules grouped by persisted entity.
- Contains: Provider, MCP, prompt, skill, settings, proxy, failover, stream check, model pricing, universal provider, usage rollup, and model route methods.
- Key files: `src-tauri/src/database/dao/providers.rs`, `src-tauri/src/database/dao/proxy.rs`, `src-tauri/src/database/dao/model_routes.rs`, `src-tauri/src/database/dao/failover.rs`, `src-tauri/src/database/dao/settings.rs`

**`src-tauri/src/proxy/`:**
- Purpose: Local multi-app HTTP proxy, routing/failover, transforms, response handling, usage logging, and provider adapters.
- Contains: Axum server, handlers, request context, model router, provider router, forwarder, adapters, response builders, circuit breaker, metrics, SSE utilities, usage modules
- Key files: `src-tauri/src/proxy/server.rs`, `src-tauri/src/proxy/handlers.rs`, `src-tauri/src/proxy/handler_context.rs`, `src-tauri/src/proxy/model_router.rs`, `src-tauri/src/proxy/provider_router.rs`, `src-tauri/src/proxy/forwarder.rs`

**`src-tauri/src/proxy/providers/`:**
- Purpose: Provider-specific proxy auth, endpoint, schema, streaming, and transform behavior.
- Contains: Generic adapter trait plus Claude, Codex/OpenAI, Gemini, Copilot, streaming, transform, history, OAuth, and shadow-store modules.
- Key files: `src-tauri/src/proxy/providers/adapter.rs`, `src-tauri/src/proxy/providers/claude.rs`, `src-tauri/src/proxy/providers/codex.rs`, `src-tauri/src/proxy/providers/gemini.rs`, `src-tauri/src/proxy/providers/auth.rs`

**`src-tauri/src/daemon/`:**
- Purpose: Unix supervisor daemon for proxy worker lifecycle and foreground IPC.
- Contains: Daemon entry point, supervisor, IPC client/server/protocol, pidfile, logging, paths, restart policy
- Key files: `src-tauri/src/daemon/mod.rs`, `src-tauri/src/daemon/supervisor.rs`, `src-tauri/src/daemon/ipc/protocol.rs`, `src-tauri/src/daemon/ipc/client.rs`, `src-tauri/src/daemon/ipc/server.rs`

**`src-tauri/src/deeplink/`:**
- Purpose: `ccswitch://v1/import?...` parsing and provider-resource import.
- Contains: Parser, provider importer, URL utility functions, request model
- Key files: `src-tauri/src/deeplink/mod.rs`, `src-tauri/src/deeplink/parser.rs`, `src-tauri/src/deeplink/provider.rs`

**`src-tauri/src/commands/`:**
- Purpose: Library-level command helpers that are not normal Clap subcommands.
- Contains: OpenClaw workspace file and daily memory operations.
- Key files: `src-tauri/src/commands/mod.rs`, `src-tauri/src/commands/workspace.rs`

**`src-tauri/src/session_manager/`:**
- Purpose: Multi-app session listing and terminal/session provider helpers.
- Contains: Provider-specific session readers and terminal helpers.
- Key files: `src-tauri/src/session_manager/mod.rs`, `src-tauri/src/session_manager/providers/claude.rs`, `src-tauri/src/session_manager/providers/codex.rs`, `src-tauri/src/session_manager/terminal/mod.rs`

**`src-tauri/tests/`:**
- Purpose: Integration test targets for CLI commands, config isolation, proxy behavior, database behavior, providers, WebDAV, sessions, skills, deep links, and install script checks.
- Contains: Flat integration tests and grouped proxy test directories.
- Key files: `src-tauri/tests/support.rs`, `src-tauri/tests/provider_commands.rs`, `src-tauri/tests/proxy_service.rs`, `src-tauri/tests/proxy_claude_streaming.rs`, `src-tauri/tests/proxy_claude_openai_chat.rs`

## Key File Locations

**Entry Points:**
- `src-tauri/src/main.rs`: Binary entry point, startup-state gate, command dispatcher.
- `src-tauri/src/lib.rs`: Library root, module declarations, public re-exports for tests and callers.
- `src-tauri/src/cli/mod.rs`: Clap root command, global `--app`, top-level subcommands.
- `src-tauri/src/cli/interactive/mod.rs`: Interactive-mode dispatch.
- `src-tauri/src/proxy/server.rs`: Axum proxy server construction and runtime state.
- `src-tauri/src/daemon/mod.rs`: Unix daemon entry point and global proxy-switch notification.

**Configuration:**
- `src-tauri/Cargo.toml`: Crate metadata, library/binary targets, dependencies, features.
- `src-tauri/rust-toolchain.toml`: Pinned Rust toolchain.
- `src-tauri/src/config.rs`: CC-Switch config directory and JSON/text file helpers.
- `src-tauri/src/settings.rs`: App settings and `settings.json` accessors.
- `src-tauri/src/codex_config.rs`: Codex live config paths and TOML/auth writes.
- `src-tauri/src/gemini_config.rs`: Gemini `.env` and settings conversion.
- `src-tauri/src/opencode_config.rs`: OpenCode live config adapter.
- `src-tauri/src/openclaw_config.rs`: OpenClaw live config and workspace paths.
- `src-tauri/src/hermes_config.rs`: Hermes app config adapter.

**Core Logic:**
- `src-tauri/src/store.rs`: `AppState`, database/config snapshot coordination, startup recovery.
- `src-tauri/src/app_config.rs`: `AppType`, `MultiAppConfig`, MCP/prompt/skill app models.
- `src-tauri/src/provider.rs`: `Provider`, provider metadata, usage script model.
- `src-tauri/src/model_route.rs`: Model-based proxy routing record type.
- `src-tauri/src/services/provider/mod.rs`: Provider business logic facade.
- `src-tauri/src/services/proxy.rs`: Proxy lifecycle, takeover, hot-switch, runtime session coordination.
- `src-tauri/src/database/mod.rs`: `Database` connection, schema initialization, migration gate.
- `src-tauri/src/proxy/handler_context.rs`: Per-request proxy setup, model-route fallback to provider routing.
- `src-tauri/src/proxy/model_router.rs`: Model pattern matching and route hit recording.
- `src-tauri/src/proxy/provider_router.rs`: Current-provider/failover queue selection and circuit breaker coordination.

**Testing:**
- `src-tauri/tests/support.rs`: Shared integration-test filesystem/home isolation helpers.
- `src-tauri/src/test_support.rs`: Crate-local test helpers.
- `src-tauri/tests/provider_commands.rs`: Provider CLI command integration tests.
- `src-tauri/tests/mcp_commands.rs`: MCP command integration tests.
- `src-tauri/tests/prompt_commands.rs`: Prompt command integration tests.
- `src-tauri/tests/proxy_service.rs`: Proxy service integration tests.
- `src-tauri/tests/proxy_claude_streaming.rs`: Claude streaming proxy tests.
- `src-tauri/tests/proxy_claude_openai_chat.rs`: Claude-to-OpenAI chat transform tests.
- `src-tauri/tests/deeplink_import.rs`: Deep-link import tests.
- `src-tauri/tests/workspace_commands.rs`: OpenClaw workspace command tests.

**CI and Release:**
- `.github/workflows/rust-ci.yml`: Rust CI workflow.
- `.github/workflows/release.yml`: Release workflow.
- `.github/workflows/benchmark.yml`: Benchmark workflow.
- `scripts/`: Repository utility scripts.
- `src-tauri/updater/`: Updater artifacts/configuration.
- `src-tauri/wix/`: Windows installer configuration.

## Naming Conventions

**Files:**
- Use Rust `snake_case.rs` module filenames: `src-tauri/src/model_route.rs`, `src-tauri/src/proxy/model_router.rs`, `src-tauri/src/cli/proxy_settings.rs`.
- Use one command family per file under `src-tauri/src/cli/commands/`: `provider.rs`, `proxy.rs`, `mcp.rs`, `prompts.rs`, `skills.rs`.
- Use directory `mod.rs` as the public module root: `src-tauri/src/services/mod.rs`, `src-tauri/src/proxy/providers/mod.rs`, `src-tauri/src/database/dao/mod.rs`.
- Use grouped test directories for large proxy suites: `src-tauri/tests/proxy_claude_streaming/`, `src-tauri/tests/proxy_claude_openai_chat/`.

**Directories:**
- Group by architectural layer first: `cli/`, `services/`, `database/`, `proxy/`, `daemon/`.
- Group TUI by responsibility: `src-tauri/src/cli/tui/app/`, `src-tauri/src/cli/tui/ui/`, `src-tauri/src/cli/tui/form/`, `src-tauri/src/cli/tui/runtime_actions/`, `src-tauri/src/cli/tui/runtime_systems/`.
- Group proxy provider specifics under `src-tauri/src/proxy/providers/` and keep generic routing/forwarding at `src-tauri/src/proxy/`.
- Group database persistence by entity under `src-tauri/src/database/dao/`.

**Rust Types and Functions:**
- Public service facades use `PascalCase` with `Service` suffix: `ProviderService`, `ProxyService`, `McpService`, `SkillService`.
- Domain records use `PascalCase`: `Provider`, `ProviderMeta`, `ModelRoute`, `MultiAppConfig`, `AppSettings`.
- Functions and methods use `snake_case`: `try_new_with_startup_recovery()`, `match_route()`, `select_providers()`, `record_model_route_hit()`.
- App labels and DB string values are lowercase identifiers such as `claude`, `codex`, `gemini`, `opencode`, `hermes`, `openclaw`.

## Where to Add New Code

**New User-Facing CLI Command:**
- Clap shape: `src-tauri/src/cli/mod.rs` or the relevant module under `src-tauri/src/cli/commands/`
- Command implementation: `src-tauri/src/cli/commands/<command>.rs`
- Shared logic: `src-tauri/src/services/<domain>.rs` or `src-tauri/src/services/<domain>/`
- Tests: `src-tauri/tests/<command>_commands.rs` for integration behavior or module-local `#[cfg(test)]` tests for pure parsing/formatting

**New Provider Workflow:**
- Domain model changes: `src-tauri/src/provider.rs` or `src-tauri/src/app_config.rs`
- Service logic: `src-tauri/src/services/provider/`
- Persistence: `src-tauri/src/database/dao/providers.rs` and `src-tauri/src/database/schema.rs`
- CLI command surface: `src-tauri/src/cli/commands/provider.rs`
- TUI actions/rendering: `src-tauri/src/cli/tui/runtime_actions/providers.rs`, `src-tauri/src/cli/tui/ui/providers.rs`, and related form files under `src-tauri/src/cli/tui/form/`
- Tests: `src-tauri/tests/provider_commands.rs`, `src-tauri/tests/provider_service.rs`, or service module tests

**New Proxy Routing Feature:**
- Request setup: `src-tauri/src/proxy/handler_context.rs`
- Routing engine: `src-tauri/src/proxy/provider_router.rs` or a focused module like `src-tauri/src/proxy/model_router.rs`
- Persistence: `src-tauri/src/database/dao/` and `src-tauri/src/database/schema.rs`
- CLI/TUI management: `src-tauri/src/cli/commands/proxy.rs` and `src-tauri/src/cli/tui/`
- Tests: focused integration tests under `src-tauri/tests/proxy_*.rs` and unit tests near the routing module

**New Proxy Provider Adapter:**
- Adapter implementation: `src-tauri/src/proxy/providers/<provider>.rs`
- Adapter trait updates: `src-tauri/src/proxy/providers/adapter.rs`
- Streaming/transform helpers: `src-tauri/src/proxy/providers/streaming*.rs` or `src-tauri/src/proxy/providers/transform*.rs`
- Endpoint mapping: `src-tauri/src/proxy/provider_router/upstream_endpoint.rs`
- Tests: `src-tauri/tests/proxy_<provider>*.rs` or module tests under `src-tauri/src/proxy/providers/`

**New Persisted Entity:**
- Domain type: top-level source file such as `src-tauri/src/model_route.rs`, or an existing domain module if scoped.
- Schema: `src-tauri/src/database/schema.rs`
- DAO: `src-tauri/src/database/dao/<entity>.rs`
- DAO registration: `src-tauri/src/database/dao/mod.rs`
- Public export if needed: `src-tauri/src/lib.rs`
- Tests: `src-tauri/src/database/tests.rs` and feature-specific tests

**New TUI View or Overlay:**
- Route/menu state: `src-tauri/src/cli/tui/route.rs` and `src-tauri/src/cli/tui/app/menu.rs`
- App state/types: `src-tauri/src/cli/tui/app/types.rs` or `src-tauri/src/cli/tui/app/app_state.rs`
- Rendering: `src-tauri/src/cli/tui/ui/` or `src-tauri/src/cli/tui/ui/overlay/`
- Actions: `src-tauri/src/cli/tui/runtime_actions/`
- Forms: `src-tauri/src/cli/tui/form/` and `src-tauri/src/cli/tui/ui/forms/`
- Tests: module-local tests in the affected TUI files

**New Live App Config Support:**
- Adapter module: `src-tauri/src/<app>_config.rs`
- MCP adapter if applicable: `src-tauri/src/<app>_mcp.rs`
- App enum/config model: `src-tauri/src/app_config.rs`
- Provider integration: `src-tauri/src/services/provider/`
- Proxy takeover integration: `src-tauri/src/services/proxy.rs`
- Tests: app-specific integration tests under `src-tauri/tests/`

**New OpenClaw Workspace Operation:**
- Implementation: `src-tauri/src/commands/workspace.rs`
- Config path helpers: `src-tauri/src/openclaw_config.rs`
- Tests: `src-tauri/tests/workspace_commands.rs`
- Rule: Preserve allowlist and symlink/path-traversal rejection patterns.

**New Utility Shared Across CLI and TUI:**
- Business logic: `src-tauri/src/services/`
- Terminal-only formatting: `src-tauri/src/cli/ui/`
- Low-level path or file helpers: `src-tauri/src/config.rs`
- Test helpers: `src-tauri/tests/support.rs` for integration tests or `src-tauri/src/test_support.rs` for unit tests

## Special Directories

**`.planning/codebase/`:**
- Purpose: GSD-generated codebase map consumed by planning/execution commands.
- Generated: Yes
- Committed: Project-dependent; this mapping task writes `ARCHITECTURE.md` and `STRUCTURE.md` here.

**`.github/workflows/`:**
- Purpose: CI, release, and benchmark workflows.
- Generated: No
- Committed: Yes

**`assets/`:**
- Purpose: Product images, screenshots, and partner assets.
- Generated: No
- Committed: Yes

**`docs/`:**
- Purpose: Human documentation beyond root README files.
- Generated: No
- Committed: Yes

**`scripts/`:**
- Purpose: Repository utility scripts for install/build/release/support tasks.
- Generated: No
- Committed: Yes

**`src-tauri/icons/`:**
- Purpose: Platform and app icon assets.
- Generated: Usually yes from design/icon sources, but stored as committed assets.
- Committed: Yes

**`src-tauri/updater/`:**
- Purpose: Update metadata/configuration used by updater/release flows.
- Generated: Partially
- Committed: Yes

**`src-tauri/wix/`:**
- Purpose: Windows installer configuration.
- Generated: Partially
- Committed: Yes

**`target/`:**
- Purpose: Cargo build output and rust-analyzer/flycheck artifacts.
- Generated: Yes
- Committed: No

---

*Structure analysis: 2026-06-12*
