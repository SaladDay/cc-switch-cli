# Testing Patterns

**Analysis Date:** 2026-06-12

## Test Framework

**Runner:**
- Rust built-in test harness through Cargo.
- Toolchain: Rust 1.91.1 from `src-tauri/rust-toolchain.toml`.
- Config: `src-tauri/Cargo.toml` declares the crate, binary, library, `test-hooks` feature, and dev dependencies.

**Assertion Library:**
- Standard Rust assertions: `assert!`, `assert_eq!`, `assert_ne!`, `matches!`, `expect_err`, and `unwrap_err`.
- Structured fixtures use `serde_json::json!` from `src-tauri/Cargo.toml`.

**Run Commands:**
```bash
cargo test                           # Run all tests from src-tauri/
cargo test --lib                     # Run library unit tests
cargo test --bin cc-switch           # Run binary unit tests
cargo test provider_switch           # Run tests whose names contain provider_switch
cargo test --test provider_commands  # Run one integration test target
cargo test --features test-hooks     # Run tests with the test-hooks feature enabled
cargo fmt --check                    # Required format check
```

## Test File Organization

**Location:**
- Module-local unit tests live beside implementation under `src-tauri/src/**` behind `#[cfg(test)]`, for example `src-tauri/src/main.rs`, `src-tauri/src/database/tests.rs`, `src-tauri/src/services/stream_check/tests.rs`, and `src-tauri/src/proxy/forwarder/tests/*.rs`.
- Integration tests live under `src-tauri/tests/`, for example `src-tauri/tests/provider_commands.rs`, `src-tauri/tests/mcp_commands.rs`, `src-tauri/tests/proxy_service.rs`, and `src-tauri/tests/proxy_database.rs`.
- Large integration suites can be split into a directory plus helper module, for example `src-tauri/tests/proxy_claude_streaming/*.rs` and `src-tauri/tests/proxy_claude_openai_chat/*.rs`.

**Naming:**
- Name test files by subsystem or command surface: `provider_commands.rs`, `prompt_commands.rs`, `proxy_daemon.rs`, `settings_visible_apps.rs`, and `workspace_commands.rs`.
- Name test functions as behavior statements in snake_case: `provider_usage_query_set_writes_upstream_defaults_and_preserves_meta`, `schema_migration_rejects_future_version`, and `default_cost_multiplier_rejects_non_numeric_values`.

**Structure:**
```text
src-tauri/
├── src/
│   ├── main.rs                         # binary helpers plus unit tests
│   ├── test_support.rs                 # crate-local test environment guards
│   ├── database/tests.rs               # module test suite
│   └── proxy/forwarder/tests/*.rs      # focused module test suite
└── tests/
    ├── support.rs                      # integration-test support
    ├── provider_commands.rs            # command integration tests
    ├── proxy_database.rs               # database/proxy integration tests
    └── proxy_claude_openai_chat/*.rs   # split proxy integration suite
```

## Test Structure

**Suite Organization:**
```rust
#[path = "support.rs"]
mod support;

use support::{ensure_test_home, lock_test_mutex, reset_test_fs};

#[test]
#[serial]
fn provider_usage_query_set_writes_upstream_defaults_and_preserves_meta() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    ensure_test_home();

    // arrange state
    // execute command/service
    // assert persisted state and user-visible behavior
}
```

**Patterns:**
- Use arrange/act/assert inside each test, even when not explicitly commented. `src-tauri/tests/provider_commands.rs` builds config state, executes a command, reloads state, and asserts persisted fields.
- Use `#[tokio::test]` for async database, proxy, HTTP, and stream behavior, as in `src-tauri/tests/proxy_database.rs`, `src-tauri/tests/proxy_service.rs`, and `src-tauri/src/proxy/forwarder/tests/request_building.rs`.
- Use `#[serial]` from `serial_test` for tests that mutate process-global environment, current directory, app settings, daemon state, or singleton config paths.
- Use RAII guards for environment and current-directory restoration: `ConfigDirEnvGuard` in `src-tauri/src/main.rs` and `src-tauri/src/database/tests.rs`, `CurrentDirGuard` in `src-tauri/tests/support.rs`, and `TestEnvGuard` in `src-tauri/src/test_support.rs`.

## Mocking

**Framework:** No dedicated mocking framework detected.

**Patterns:**
```rust
let db = Database::memory()?;
let provider = Provider::with_id(
    "claude-p1".to_string(),
    "claude-p1".to_string(),
    json!({"env": {"BASE_URL": "https://example.com"}}),
    None,
);
db.save_provider("claude", &provider)?;
```

```rust
let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
let app = axum::Router::new().route("/chat/completions", post(handle_chat_completions));
```

**What to Mock:**
- Use in-memory SQLite through `Database::memory()` for DAO and proxy/database behavior where disk persistence is not under test, as in `src-tauri/tests/proxy_database.rs`.
- Use temporary HOME/config directories for stateful command and live-config paths, via `src-tauri/tests/support.rs` and `src-tauri/src/test_support.rs`.
- Use local Axum upstream servers for proxy request/response tests, as in `src-tauri/tests/proxy_claude_openai_chat/helpers.rs` and `src-tauri/tests/proxy_claude_streaming/helpers.rs`.
- Use scripted upstream helpers for lower-level forwarder tests, as in `src-tauri/src/proxy/forwarder/tests/request_building.rs`.

**What NOT to Mock:**
- Do not touch real `HOME`, `CC_SWITCH_CONFIG_DIR`, `CLAUDE_CONFIG_DIR`, or `CODEX_HOME`. Use the helper isolation layer instead.
- Do not replace SQLite with ad hoc structs when testing persistence semantics; use `Database::memory()` or isolated real config directories.
- Do not call external provider APIs for normal tests. Use local listeners and synthetic JSON/SSE fixtures.

## Fixtures and Factories

**Test Data:**
```rust
fn usage_script_fixture() -> UsageScript {
    UsageScript {
        enabled: true,
        language: "javascript".to_string(),
        code: "return { remaining: 1, unit: 'USD' };".to_string(),
        timeout: Some(9),
        api_key: Some("sk-old".to_string()),
        base_url: Some("https://old.example.com".to_string()),
        access_token: None,
        user_id: None,
        template_type: Some("general".to_string()),
        auto_query_interval: Some(30),
        coding_plan_provider: None,
    }
}
```

**Location:**
- Shared integration helpers live in `src-tauri/tests/support.rs`.
- Crate-local test helpers live in `src-tauri/src/test_support.rs`.
- Suite-specific proxy helpers live in `src-tauri/tests/proxy_claude_openai_chat/helpers.rs` and `src-tauri/tests/proxy_claude_streaming/helpers.rs`.
- Module-specific fixtures stay near tests, for example `usage_script_fixture` in `src-tauri/tests/provider_commands.rs` and `claude_gemini_native_provider` in `src-tauri/src/services/stream_check/tests.rs`.

## Coverage

**Requirements:** No enforced coverage threshold or coverage configuration detected.

**View Coverage:**
```bash
# Not configured in the repository. Add cargo-llvm-cov or tarpaulin only if a phase explicitly requires coverage reporting.
```

## Test Types

**Unit Tests:**
- Scope pure helpers, parsers, command parsing, schema migration functions, stream-check configuration, and transformation logic.
- Examples: `src-tauri/src/cli/mod.rs`, `src-tauri/src/main.rs`, `src-tauri/src/services/stream_check/tests.rs`, `src-tauri/src/proxy/providers/transform.rs`, and `src-tauri/src/proxy/providers/transform_codex_chat.rs`.

**Integration Tests:**
- Scope command execution, persisted state, live-config import/export, proxy HTTP behavior, daemon lifecycle, settings, WebDAV sync, and multi-app behavior.
- Examples: `src-tauri/tests/provider_commands.rs`, `src-tauri/tests/import_export_sync.rs`, `src-tauri/tests/proxy_service.rs`, `src-tauri/tests/proxy_daemon.rs`, and `src-tauri/tests/webdav_sync_service.rs`.

**E2E Tests:**
- No browser or full packaged-app E2E framework detected.
- The closest E2E coverage is command-level and proxy-level integration testing through Cargo integration tests under `src-tauri/tests/`.

## Common Patterns

**Async Testing:**
```rust
#[tokio::test]
async fn default_cost_multiplier_round_trips() -> Result<(), AppError> {
    let db = Database::memory()?;
    db.set_default_cost_multiplier("claude", "1.5").await?;
    let updated = db.get_default_cost_multiplier("claude").await?;
    assert_eq!(updated, "1.5");
    Ok(())
}
```

**Error Testing:**
```rust
let err = db
    .set_default_cost_multiplier("claude", "not-a-number")
    .await
    .unwrap_err();
assert!(matches!(
    err,
    AppError::Localized {
        key: "error.invalidMultiplier",
        ..
    }
));
```

**Environment Isolation:**
```rust
let _guard = lock_test_mutex();
reset_test_fs();
ensure_test_home();
```

**CI Test Isolation:**
- `.github/workflows/rust-ci.yml` creates a `mktemp -d` sandbox for `HOME`, `USERPROFILE`, `CC_SWITCH_CONFIG_DIR`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `XDG_CONFIG_HOME`, `XDG_RUNTIME_DIR`, and `XDG_STATE_HOME`.
- CI sets `RUST_TEST_THREADS=1` for unit and selected integration jobs because many tests manipulate process-global config state.

**Targeted CI Jobs:**
- Format job: `cargo fmt --check` from `src-tauri/`.
- Unit jobs: `cargo test --lib -- --nocapture` and `cargo test --bin cc-switch -- --nocapture`.
- Integration jobs: selected proxy/failover/database tests in `.github/workflows/rust-ci.yml`.

---

*Testing analysis: 2026-06-12*
