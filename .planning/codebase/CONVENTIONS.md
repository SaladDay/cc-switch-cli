# Coding Conventions

**Analysis Date:** 2026-06-12

## Naming Patterns

**Files:**
- Use Rust snake_case module filenames under `src-tauri/src/`, for example `src-tauri/src/model_route.rs`, `src-tauri/src/usage_events.rs`, and `src-tauri/src/proxy/model_router.rs`.
- Use `mod.rs` to anchor larger module directories such as `src-tauri/src/cli/mod.rs`, `src-tauri/src/services/provider/mod.rs`, `src-tauri/src/proxy/mod.rs`, and `src-tauri/src/database/mod.rs`.
- Split large subsystems into named submodules under domain directories: CLI command modules in `src-tauri/src/cli/commands/*.rs`, TUI runtime handlers in `src-tauri/src/cli/tui/runtime_actions/*.rs`, proxy adapters in `src-tauri/src/proxy/providers/*.rs`, and DAOs in `src-tauri/src/database/dao/*.rs`.
- Integration test filenames describe the command or subsystem under test: `src-tauri/tests/provider_commands.rs`, `src-tauri/tests/mcp_commands.rs`, `src-tauri/tests/proxy_daemon.rs`, and nested proxy suites such as `src-tauri/tests/proxy_claude_openai_chat/transform_cases.rs`.

**Functions:**
- Use snake_case with behavior-first names: `command_requires_startup_state` in `src-tauri/src/main.rs`, `generate_provider_key` in `src-tauri/src/services/provider/mod.rs`, and `set_claude_proxy_port_to_ephemeral` in `src-tauri/tests/proxy_claude_streaming/helpers.rs`.
- Prefix small test fixture builders with the object they create, for example `usage_script_fixture` and `saved_provider` in `src-tauri/tests/provider_commands.rs`, and `claude_gemini_native_provider` in `src-tauri/src/services/stream_check/tests.rs`.
- Use `execute` as the standard top-level command entry function in command modules called from `src-tauri/src/main.rs`, for example `cc_switch_lib::cli::commands::provider::execute` and `cc_switch_lib::cli::commands::proxy::execute`.

**Variables:**
- Use snake_case locals and explicit domain names: `listen_port`, `takeovers`, `provider_id`, `app_type`, `request_timeout`, and `bypass_circuit_breaker` appear across `src-tauri/src/cli/mod.rs`, `src-tauri/src/services/provider/mod.rs`, and `src-tauri/src/proxy/forwarder/tests/request_building.rs`.
- Prefer meaningful temporary names in tests: `state`, `provider`, `meta`, `script`, `response`, `sent`, and `err` in `src-tauri/tests/provider_commands.rs` and `src-tauri/src/proxy/forwarder/tests/request_building.rs`.
- Use guard variables prefixed with `_guard` or `_lock` when the value is held for RAII side effects, as in `src-tauri/tests/provider_commands.rs`, `src-tauri/src/main.rs`, and `src-tauri/src/database/tests.rs`.

**Types:**
- Use PascalCase for structs, enums, and guards: `Cli`, `Commands`, `AppError`, `ProviderService`, `PostCommitAction`, `ConfigDirEnvGuard`, and `TestEnvGuard`.
- Use service structs as stateless namespaces for durable business behavior, for example `ProviderService` in `src-tauri/src/services/provider/mod.rs` and `StreamCheckService` in `src-tauri/src/services/stream_check/service.rs`.
- Use enum variants for command trees and structured errors: `Commands::Provider`, `Commands::Proxy`, and `AppError::Localized` in `src-tauri/src/cli/mod.rs` and `src-tauri/src/error.rs`.

## Code Style

**Formatting:**
- Use `rustfmt` from the pinned toolchain in `src-tauri/rust-toolchain.toml`.
- Run formatting from `src-tauri/` with:
```bash
cargo fmt
cargo fmt --check
```
- CI enforces `cargo fmt --check` for `src-tauri/**` and workflow changes in `.github/workflows/rust-ci.yml`.
- No repository-level `rustfmt.toml` or `.rustfmt.toml` is detected, so use default Rust formatting.

**Linting:**
- `clippy` is part of the pinned toolchain in `src-tauri/rust-toolchain.toml`, and documented local usage is:
```bash
cargo clippy
```
- No `clippy.toml` is detected; follow standard Clippy guidance and local patterns.
- CI currently runs format and selected tests in `.github/workflows/rust-ci.yml`; `cargo clippy` is documented in `AGENTS.md`, `CLAUDE.md`, and `README.md` as a local quality gate.

## Import Organization

**Order:**
1. Standard library imports first, usually grouped by module path, as in `src-tauri/src/main.rs` and `src-tauri/tests/provider_commands.rs`.
2. External crates next, such as `clap`, `serde_json`, `serial_test`, `tokio`, `axum`, `rusqlite`, and `tempfile`.
3. Crate-local imports after external crates, using `crate::...` inside the library and `cc_switch_lib::...` in integration tests.
4. Local test support imports after `#[path = "support.rs"] mod support;`, as in `src-tauri/tests/provider_commands.rs`.

**Path Aliases:**
- Library code uses `crate::...` for internal modules, for example `crate::app_config::AppType` and `crate::error::AppError` in `src-tauri/src/services/provider/mod.rs`.
- Integration tests import public API through `cc_switch_lib::...`, for example `cc_switch_lib::{AppState, Database, ProviderService}` in `src-tauri/tests/support.rs`.
- Tests use `#[path = "support.rs"] mod support;` for shared integration-test helpers in `src-tauri/tests/provider_commands.rs`.

## Error Handling

**Patterns:**
- Use the central `AppError` enum in `src-tauri/src/error.rs` for product errors that cross CLI, service, database, and config boundaries.
- Return `Result<(), AppError>` or `Result<T, AppError>` from command and service code; `src-tauri/src/main.rs` prints `Error: {}` and exits with status 1 at the binary boundary.
- Prefer typed error variants with context over raw strings: `AppError::Io`, `AppError::Json`, `AppError::Toml`, `AppError::Database`, and `AppError::Localized` in `src-tauri/src/error.rs`.
- Use `AppError::localized(key, zh, en)` for user-visible validation or policy errors that need stable localization keys, for example `provider.key.invalid` in `src-tauri/src/services/provider/mod.rs`.
- Use `expect` freely in tests for setup failures with clear messages, for example `expect("create temp dir")`, `expect("seed database from config")`, and `expect("provider command should succeed")`.
- Use `matches!` for structured error assertions, as in `src-tauri/tests/proxy_database.rs` when checking `AppError::Localized` keys.

## Logging

**Framework:** `env_logger` plus `log`

**Patterns:**
- Initialize logging in `src-tauri/src/main.rs` with `env_logger::Builder::from_env`; default CLI logging is `error`, and `--verbose` switches to `debug`.
- Commands with their own logger opt out through `command_uses_own_logger` in `src-tauri/src/main.rs`; Unix daemon start is the visible special case.
- Tests typically assert behavior directly instead of checking logs. Proxy helpers capture request bodies, headers, and database log rows through helpers such as `request_log_insert_lines` in `src-tauri/tests/proxy_claude_openai_chat/helpers.rs`.
- Use `eprintln!` sparingly in test cleanup paths where cleanup failure should be visible but should not mask the test result, as in `src-tauri/tests/support.rs`.

## Comments

**When to Comment:**
- Keep comments short and explanatory for operational constraints, startup behavior, or migration snapshots.
- Chinese comments are present and acceptable in existing source, especially around test isolation and user-facing configuration behavior, for example `src-tauri/tests/support.rs`, `src-tauri/src/main.rs`, and `src-tauri/src/database/tests.rs`.
- Use module-level comments for test modules where they describe the suite purpose, as in `src-tauri/src/database/tests.rs`.

**JSDoc/TSDoc:**
- Not applicable; this is a Rust crate.
- Use Rust doc comments only where a public API or module needs documentation. Most internal helpers use ordinary comments or self-descriptive names.

## Function Design

**Size:** Keep command dispatch and command-shape code thin; move reusable behavior into services or helpers. `src-tauri/src/main.rs` dispatches to command modules, while durable behavior lives in `src-tauri/src/services/`, `src-tauri/src/database/`, and `src-tauri/src/proxy/`.

**Parameters:** Prefer explicit typed parameters over loosely structured maps for command/service boundaries. Examples include `ProviderCommand` in `src-tauri/src/cli/commands/provider.rs`, `AppType` arguments in `src-tauri/src/main.rs`, and `ForwardOptions` in `src-tauri/src/proxy/forwarder/tests/request_building.rs`.

**Return Values:** Use `Result<T, AppError>` for fallible product paths and direct values for pure helpers. Tests can return `Result<(), AppError>` when `?` improves readability, as in `src-tauri/tests/proxy_database.rs`.

## Module Design

**Exports:** Re-export public API from `src-tauri/src/lib.rs` for integration tests and command code. Keep most subsystem internals private or `pub(crate)` unless they are intentionally used across modules.

**Barrel Files:** Use Rust module barrels through `mod.rs` files. Examples include `src-tauri/src/cli/mod.rs`, `src-tauri/src/services/mod.rs`, `src-tauri/src/services/provider/mod.rs`, `src-tauri/src/proxy/mod.rs`, and `src-tauri/src/database/dao/mod.rs`.

**Command Modules:** Add user-facing CLI shape in `src-tauri/src/cli/mod.rs` or a relevant file under `src-tauri/src/cli/commands/`, implement command I/O in that command module, and put shared behavior in `src-tauri/src/services/`.

**Test-Only Modules:** Gate module-local test support with `#[cfg(test)]`, as in `src-tauri/src/services/provider/mod.rs`, `src-tauri/src/database/tests.rs`, and `src-tauri/src/proxy/forwarder/tests/mod.rs`.

---

*Convention analysis: 2026-06-12*
