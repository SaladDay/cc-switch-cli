# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

The Rust crate lives in `src-tauri/`. Run Cargo commands from that directory unless a command explicitly targets repository-root assets or scripts.

```bash
cd src-tauri

cargo run                                  # Run cc-switch in interactive mode
cargo run -- provider list                 # Run a specific CLI command
cargo run -- --app codex provider list     # Run a command for a specific app
cargo run -- proxy show                    # Inspect proxy state
cargo run -- env tools                     # Check local CLI tools
cargo build --release                      # Build release binary at target/release/cc-switch

cargo fmt                                  # Format Rust code
cargo fmt --check                          # Check formatting, matching CI
cargo clippy                               # Run lints
cargo test                                 # Run all tests
cargo test provider_switch                 # Run tests whose names contain provider_switch
cargo test --test provider_commands        # Run a single integration test target
cargo test --features test-hooks           # Run tests with the test-hooks feature enabled
```

The repository pins Rust through `src-tauri/rust-toolchain.toml` to Rust 1.91.1 with `rustfmt` and `clippy`. CI currently runs `cargo fmt --check` for changes under `src-tauri/**`.

## Project overview

CC-Switch CLI is a Rust command-line management tool for Claude Code, Codex, Gemini, OpenCode, Hermes, and OpenClaw. It manages provider configurations, MCP servers, prompts, skills, WebDAV sync, local proxy routes, failover, daemon/start flows, deep-link imports, workspace memory files, and environment checks.

The main crate is `src-tauri/`; the repository root contains docs, assets, install/update scripts, packaging metadata, and Nix files.

Key Rust entry points:

- `src/main.rs` parses CLI arguments, initializes logging, creates startup state for most commands, and dispatches to command handlers.
- `src/lib.rs` declares crate modules and re-exports public types used by integration tests and command code.
- `src/cli/mod.rs` defines the top-level Clap CLI, global `--app` flag, and command enum.
- `src/cli/commands/` contains direct command implementations for providers, MCP, prompts, skills, config/WebDAV, proxy, failover, Hermes, start, daemon, env, update, completions, and internal commands.
- `src/commands/` contains library command helpers that are not top-level Clap subcommands, including OpenClaw workspace file and daily memory operations.
- `src/cli/interactive/` and `src/cli/tui/` contain the interactive ratatui UI, runtime action handlers, forms, overlays, route state, and UI rendering.
- `src/services/` contains durable business logic used by commands and the TUI: providers, auth, MCP, prompts, skills, proxy, WebDAV sync, stream checks, speed tests, environment checks, visible apps, subscription/coding-plan quota checks, and state coordination.
- `src/database/` is the SQLite persistence layer. `Database` owns a mutex-wrapped rusqlite connection, schema creation/migration, backups, and DAO modules for providers, MCP, prompts, skills, settings, proxy state, stream checks, universal providers, and failover queues.
- `src/app_config.rs`, `src/provider.rs`, and app-specific config modules (`claude_*`, `codex_config.rs`, `gemini_*`, `hermes_config.rs`, `opencode_config.rs`, `openclaw_config.rs`) define the shared configuration model and live-file adapters for supported apps.
- `src/deeplink/` implements the `ccswitch://v1/import?...` import protocol for provider resources and is exported through `lib.rs` for tests and callers.
- `src/proxy/` implements the local multi-app proxy with Axum handlers, request forwarding, provider routing/failover, provider-specific transformations, response/stream handling, usage logging, model mapping, cache/thinking rectifiers, circuit breaking, and metrics.
- `src/daemon/` implements the Unix supervisor daemon, IPC protocol, logging, pidfile, and restart support.
- `src/store.rs` defines `AppState`, which ties together the database, an in-memory `MultiAppConfig` snapshot, startup live-config imports/recovery, and `ProxyService`.

## State and configuration model

CC-Switch stores core state in SQLite at `~/.cc-switch/cc-switch.db` by default, or under `$CC_SWITCH_CONFIG_DIR/cc-switch.db` when `CC_SWITCH_CONFIG_DIR` is set. `~/.cc-switch/settings.json` stores app settings, `~/.cc-switch/skills/` stores installed skill source files, and `~/.cc-switch/backups/` holds rotating backups.

Legacy `config.json` and `skills.json` are migration/import sources only. `AppState::try_new()` validates and migrates legacy files into SQLite when needed, exports database state into a `MultiAppConfig` snapshot, seeds defaults, migrates old common-config semantics, and constructs `ProxyService`. `AppState::try_new_with_startup_recovery()` also imports live provider configs and recovers proxy takeovers when needed. `AppState::save()` persists the in-memory snapshot back to SQLite.

Live config files are separate from CC-Switch storage and are only synced or imported for initialized apps:

- Claude: `~/.claude/settings.json`, `~/.claude.json`, `~/.claude/CLAUDE.md`
- Codex: `~/.codex/auth.json`, `~/.codex/config.toml`, `~/.codex/AGENTS.md`
- Gemini: `~/.gemini/.env`, `~/.gemini/settings.json`, `~/.gemini/GEMINI.md`
- OpenCode: `~/.config/opencode/opencode.json`, `~/.config/opencode/AGENTS.md`
- Hermes: Hermes config directory from settings or default app location, with app-specific provider/prompt/MCP handling
- OpenClaw: `~/.openclaw/openclaw.json`, `~/.openclaw/AGENTS.md`

Environment overrides matter when testing or running commands: `CC_SWITCH_CONFIG_DIR` controls CC-Switch storage, `CLAUDE_CONFIG_DIR` controls Claude config directory, and `CODEX_HOME` controls Codex config. Tests also commonly set `HOME`, `XDG_CONFIG_HOME`, `XDG_RUNTIME_DIR`, and `XDG_STATE_HOME`.

## CLI architecture

Adding or changing a user-facing command usually requires updates in three layers:

1. Define the Clap shape in `src/cli/mod.rs` or the relevant `src/cli/commands/*.rs` file.
2. Implement command I/O and prompts in `src/cli/commands/`, keeping durable logic in `src/services/` when behavior is shared with the TUI or other commands.
3. Add or update tests under `src-tauri/tests/` or module-local `#[cfg(test)]` tests.

The global `--app` flag selects an `AppType`; Claude is the default. Supported app labels are `claude`, `codex`, `gemini`, `opencode`, `hermes`, and `openclaw`. Some app modes differ: OpenCode, Hermes, and OpenClaw use additive live-config semantics in provider workflows, while Claude/Codex/Gemini primarily switch a current provider.

Commands that normally create startup state call `AppState::try_new_with_startup_recovery()` before dispatch. `update`, `completions`, `internal`, and Unix `daemon` commands intentionally bypass normal startup state so they can run even when the user database has a future schema version or daemon-specific logging needs apply. When commands run under the daemon socket environment, startup state is also skipped so the daemon-owned process can coordinate state.

OpenClaw workspace helpers live under `src/commands/workspace.rs`, not the Clap command tree. They restrict file access to the OpenClaw workspace allowlist (`AGENTS.md`, `SOUL.md`, `USER.md`, `IDENTITY.md`, `TOOLS.md`, `MEMORY.md`, `HEARTBEAT.md`, `BOOTSTRAP.md`, `BOOT.md`) and daily memory files, and deliberately reject symlinks/path traversal.

## Proxy architecture

The proxy command surface is in `src/cli/commands/proxy.rs`, orchestration lives in `src/services/proxy.rs`, and the HTTP server is in `src/proxy/server.rs` and `src/proxy/handlers.rs`.

Request handling flows through `HandlerContext`, `ProviderRouter`, `RequestForwarder`, provider adapters in `src/proxy/providers/`, and response builders/handlers in `src/proxy/response*.rs`. Claude `/v1/messages` traffic may be transformed between Anthropic and OpenAI-compatible formats; Codex/OpenAI, Gemini, Copilot, and streaming-response routes are handled by provider-specific adapters. Proxy tests are split across focused integration targets such as `proxy_claude_streaming`, `proxy_claude_openai_chat`, `proxy_claude_response_parity`, `proxy_claude_forwarder_alignment`, `proxy_multi_app_passthrough`, `proxy_takeover`, `proxy_service`, and `proxy_daemon`.

## Testing requirements

Al test cases should be executed under `src-tauri/`.

When adding integration tests that touch HOME, app config directories, or live config files, isolate filesystem state with helpers in `src-tauri/tests/support.rs`. Use `ensure_test_home()`, `reset_test_fs()`, and `lock_test_mutex()` patterns rather than writing to real user directories. Unit tests inside the crate can also use `src/test_support.rs` helpers for test home/settings isolation.

### IMPORTANT

- **NEVER** change the host configuration in `$CC_SWITCH_CONFIG_DIR/`.
- **NEVER** change the host configuration in `$CLAUDE_CONFIG_DIR/`.
- **NEVER** change the host configuration in `$CODEX_HOME/`.
- Create a sandbox before executing test cases or commands that write app configuration.
- Prefer temporary directories and explicit environment overrides for tests that exercise live config sync/import paths.
