# Technology Stack

**Analysis Date:** 2026-06-12

## Languages

**Primary:**
- Rust 1.91.1, edition 2021 - native CLI, TUI, daemon, proxy, database, and service logic in `src-tauri/src/`; pinned by `src-tauri/rust-toolchain.toml` and `src-tauri/Cargo.toml`.

**Secondary:**
- Python 3 - benchmark and release-helper scripts in `scripts/benchmark_cc_switch.py`, `scripts/check_benchmark_thresholds.py`, and `scripts/generate_latest_json.py`.
- Shell - installer and release/download workflow glue in `install.sh` and `.github/workflows/release.yml`.
- Nix - package definition in `flake.nix`.
- Markdown - user docs and prompt/live-memory files such as `README.md`, `README_ZH.md`, `AGENTS.md`, and generated planning docs under `.planning/`.

## Runtime

**Environment:**
- Native Rust binary named `cc-switch`, with async work on Tokio 1.x. The binary entry point is `src-tauri/src/main.rs`; the reusable library crate is exported from `src-tauri/src/lib.rs`.
- The repository directory is named `src-tauri/`, but the current manifest defines a Rust `rlib` and `cc-switch` binary only; no Tauri GUI crate metadata is present in `src-tauri/Cargo.toml`.
- Local HTTP proxy runtime uses Axum/Tokio in `src-tauri/src/proxy/server.rs` and `src-tauri/src/proxy/handlers.rs`.
- Unix daemon/supervisor runtime lives in `src-tauri/src/daemon/`.

**Package Manager:**
- Cargo 1.91.1.
- Lockfile: present at `src-tauri/Cargo.lock`.

## Frameworks

**Core:**
- Clap 4.5 - top-level CLI parsing in `src-tauri/src/cli/mod.rs` and command modules under `src-tauri/src/cli/commands/`.
- Ratatui 0.30 + Crossterm 0.29 - interactive terminal UI under `src-tauri/src/cli/tui/` and `src-tauri/src/cli/interactive/`.
- Axum 0.7 + Tower 0.5 + Tower HTTP 0.5 - local proxy server, route handlers, and CORS in `src-tauri/src/proxy/server.rs`.
- Tokio 1.x - async runtime, networking, process, signal, sync, and timers across proxy, WebDAV, OAuth, daemon, and service code.
- rusqlite 0.31 with `bundled`, `backup`, and `hooks` - SQLite persistence in `src-tauri/src/database/`.

**Testing:**
- Cargo test - unit and integration tests under module-local `#[cfg(test)]` blocks and `src-tauri/tests/`.
- serial_test 3 - serializes tests that mutate process environment or filesystem state.
- tempfile 3 - isolated temporary homes/config roots in `src-tauri/tests/support.rs` and module tests.

**Build/Dev:**
- rustfmt and clippy - pinned as toolchain components in `src-tauri/rust-toolchain.toml`; CI enforces `cargo fmt --check`.
- GitHub Actions - Rust CI, benchmark CI, and release workflows in `.github/workflows/rust-ci.yml`, `.github/workflows/benchmark.yml`, and `.github/workflows/release.yml`.
- Nix flakes - `flake.nix` builds the Cargo crate from `src-tauri/` and disables test execution for packaging.
- cross-rs/cross - release workflow uses cross compilation for Linux MUSL and ARM targets in `.github/workflows/release.yml`.

## Key Dependencies

**Critical:**
- `serde`, `serde_json`, `toml`, `toml_edit`, `serde_yaml`, `json5`, `json-five` - parse and write live config formats for Claude, Codex, Gemini, OpenCode, Hermes, and OpenClaw across `src-tauri/src/*config*.rs`.
- `reqwest` 0.12 with `rustls-tls`, `json`, `stream`, and `socks` - external API calls, WebDAV transport, proxy forwarding, model fetching, OAuth, update downloads, and usage checks.
- `rusqlite` 0.31 - durable state store for providers, MCP, prompts, skills, proxy state, usage logs, model pricing, model routes, failover queues, and settings.
- `rquickjs` 0.8 - executes JavaScript usage scripts for provider quota/coding-plan integrations in `src-tauri/src/usage_script.rs`.
- `minisign-verify`, `sha2`, `semver`, `flate2`, `tar`, and `zip` - signed self-update, checksum, archive, and release asset handling in `src-tauri/src/cli/commands/update.rs`.

**Infrastructure:**
- `dirs` - resolves user directories in `src-tauri/src/config.rs` and app-specific config adapters.
- `which` - detects local assistant CLIs and tools in environment-check code under `src-tauri/src/services/env_checker.rs` and `src-tauri/src/services/local_env_check.rs`.
- `chrono` and `rust_decimal` - timestamps, usage windows, pricing, and usage rollups in `src-tauri/src/database/dao/` and `src-tauri/src/services/`.
- `uuid`, `indexmap`, `regex`, `base64`, `url`, and `bytes` - identifiers, ordered maps, matching, OAuth token parsing, URL handling, and proxy body handling.
- Windows-only `winreg` and `self-replace` - registry and binary replacement support declared in `src-tauri/Cargo.toml`.

## Configuration

**Environment:**
- `CC_SWITCH_CONFIG_DIR` controls CC-Switch storage root; default is `$HOME/.cc-switch` from `src-tauri/src/config.rs`.
- `CLAUDE_CONFIG_DIR` overrides Claude config directory; default is `$HOME/.claude` from `src-tauri/src/config.rs`.
- `CODEX_HOME` controls Codex config directory when it exists; fallback is `$HOME/.codex` from `src-tauri/src/codex_config.rs`.
- Tests and CI set sandboxed `HOME`, `USERPROFILE`, `XDG_CONFIG_HOME`, `XDG_RUNTIME_DIR`, `XDG_STATE_HOME`, `CC_SWITCH_CONFIG_DIR`, `CLAUDE_CONFIG_DIR`, and `CODEX_HOME`; see `src-tauri/tests/support.rs` and `.github/workflows/rust-ci.yml`.
- Never run commands that write live app configuration without temporary environment overrides; this is required by `AGENTS.md`.

**Build:**
- `src-tauri/Cargo.toml` is the crate manifest; run Cargo commands from `src-tauri/`.
- `src-tauri/rust-toolchain.toml` pins Rust 1.91.1 with `rustfmt` and `clippy`.
- `src-tauri/Cargo.lock` locks Rust dependencies.
- `flake.nix` packages the crate for x86_64/aarch64 Linux and Darwin.
- `.github/workflows/rust-ci.yml` runs format and selected test targets in sandboxed homes.
- `.github/workflows/benchmark.yml` builds release binary and runs benchmark thresholds through `scripts/benchmark_cc_switch.py` and `scripts/check_benchmark_thresholds.py`.
- `.github/workflows/release.yml` builds multi-platform artifacts, signs updater assets, generates `latest.json`, and creates GitHub releases.

## Platform Requirements

**Development:**
- Rust 1.91.1 via rustup/toolchain file.
- Cargo commands should be run from `src-tauri/`, for example `cargo fmt --check`, `cargo test`, and `cargo build --release`.
- Local repository shell commands should use the `rtk` prefix per `AGENTS.md`.
- Current checkout uses a proxied remote (`https://gh-proxy.com/https://github.com/SaladDay/cc-switch-cli.git`); `gh pr status` cannot infer a GitHub repo from this remote, so PR inspection needs an explicit `--repo` target or remote adjustment outside this mapping task.

**Production:**
- Distributed as a native `cc-switch` binary for macOS, Linux, and Windows via GitHub Releases.
- Release targets in `.github/workflows/release.yml`: macOS x64/ARM64/universal, Windows x64, Linux x64/ARM64 MUSL, and Linux x64/ARM64 GLIBC.
- Install/update surface includes `install.sh`, release archives, `checksums.txt`, `latest.json`, Minisign public key `src-tauri/updater/minisign.pub`, and updater code in `src-tauri/src/cli/commands/update.rs`.
- Nix package output is defined in `flake.nix` as `cc-switch`, `cc-switch-cli`, and `default`.

---

*Stack analysis: 2026-06-12*
