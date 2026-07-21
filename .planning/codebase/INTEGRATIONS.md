# External Integrations

**Analysis Date:** 2026-06-12

## APIs & External Services

**AI Provider APIs:**
- Anthropic/Claude API - Claude provider routing, stream checks, proxy forwarding, and quota checks.
  - SDK/Client: `reqwest` through `src-tauri/src/proxy/http_client.rs`, adapters in `src-tauri/src/proxy/providers/claude.rs`, and quota code in `src-tauri/src/services/subscription.rs`.
  - Auth: `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, Claude OAuth credential files/keychain.
- OpenAI-compatible/Codex APIs - Codex, OpenCode, Hermes, OpenClaw, OpenAI Responses, and OpenAI Chat Completions compatible routing.
  - SDK/Client: `reqwest`; adapter code in `src-tauri/src/proxy/providers/codex.rs`, transform code in `src-tauri/src/proxy/providers/transform_responses.rs`, and live Codex config code in `src-tauri/src/codex_config.rs`.
  - Auth: `OPENAI_API_KEY`, Codex `auth.json`, provider TOML config, or managed Codex OAuth.
- Google Gemini API - Gemini provider routing and Claude-to-Gemini transformation.
  - SDK/Client: `reqwest`; adapter and transform code in `src-tauri/src/proxy/providers/gemini.rs`, `src-tauri/src/proxy/providers/transform_gemini.rs`, and `src-tauri/src/gemini_config.rs`.
  - Auth: `GEMINI_API_KEY`, `GOOGLE_API_KEY`, `GOOGLE_GEMINI_BASE_URL`, or OAuth token (`ya29.*`) written through Gemini settings.
- OpenRouter and OpenAI-compatible relay providers - user-configured provider endpoints and shipped templates.
  - SDK/Client: generic `reqwest` forwarding in `src-tauri/src/proxy/forwarder.rs` and provider templates in `src-tauri/src/cli/tui/form/provider_templates.rs`.
  - Auth: provider API keys stored in SQLite-backed provider settings and live app configs.

**Managed OAuth Services:**
- OpenAI/Codex OAuth - managed ChatGPT/Codex login for Codex OAuth providers.
  - SDK/Client: `reqwest` in `src-tauri/src/proxy/providers/codex_oauth_auth.rs` and wrapper service `src-tauri/src/services/codex_oauth.rs`.
  - Auth: OpenAI device flow endpoints `https://auth.openai.com/api/accounts/deviceauth/usercode`, `https://auth.openai.com/api/accounts/deviceauth/token`, `https://auth.openai.com/oauth/token`; credentials persist to `codex_oauth_auth.json` under the CC-Switch config dir.
- GitHub Copilot OAuth - GitHub device flow plus Copilot token exchange.
  - SDK/Client: `reqwest` in `src-tauri/src/proxy/providers/copilot_auth.rs` and wrapper service `src-tauri/src/services/copilot_auth.rs`.
  - Auth: GitHub device flow at `https://github.com/login/device/code`, token exchange at `https://github.com/login/oauth/access_token`, Copilot token at `https://api.github.com/copilot_internal/v2/token`, Copilot API at `https://api.githubcopilot.com`; GHES domains are also supported.

**Sync & Distribution:**
- WebDAV - cross-device sync for SQLite state and skills.
  - SDK/Client: `reqwest` in `src-tauri/src/services/webdav.rs`; archive protocol in `src-tauri/src/services/webdav_sync/mod.rs`.
  - Auth: basic auth username/password stored in `WebDavSyncSettings` in `src-tauri/src/settings.rs`; preset URL for Jianguoyun is `https://dav.jianguoyun.com/dav`.
- GitHub Releases - installer, self-update, release assets, checksums, and `latest.json` update manifest.
  - SDK/Client: `reqwest` updater in `src-tauri/src/cli/commands/update.rs`; release publishing in `.github/workflows/release.yml`.
  - Auth: GitHub Actions `GITHUB_TOKEN`; updater asset signing uses `CC_SWITCH_MINISIGN_SECRET_KEY` in `.github/workflows/release.yml`.

**Local Assistant Apps:**
- Claude Code - live config sync, prompts, MCP, OAuth quota reading, proxy takeover.
  - SDK/Client: filesystem adapters in `src-tauri/src/config.rs`, `src-tauri/src/claude_*`, `src-tauri/src/services/provider/claude.rs`, and `src-tauri/src/services/proxy.rs`.
  - Auth: `~/.claude/settings.json`, `~/.claude.json`, Claude credentials file/keychain.
- Codex CLI - live config sync, MCP, auth, model catalog, prompt file, proxy takeover.
  - SDK/Client: `src-tauri/src/codex_config.rs`, `src-tauri/src/services/provider/codex.rs`, and `src-tauri/src/services/proxy/codex_toml.rs`.
  - Auth: `~/.codex/auth.json`, `~/.codex/config.toml`, or managed Codex OAuth.
- Gemini CLI, OpenCode, Hermes, OpenClaw - app-specific live config adapters.
  - SDK/Client: `src-tauri/src/gemini_config.rs`, `src-tauri/src/opencode_config.rs`, `src-tauri/src/hermes_config.rs`, and `src-tauri/src/openclaw_config.rs`.
  - Auth: app-specific live config files and provider settings persisted by CC-Switch.

## Data Storage

**Databases:**
- SQLite via `rusqlite`.
  - Connection: `CC_SWITCH_CONFIG_DIR/cc-switch.db`, defaulting to `$HOME/.cc-switch/cc-switch.db`; configured in `src-tauri/src/config.rs` and opened in `src-tauri/src/database/mod.rs`.
  - Client: `Database` wrapper in `src-tauri/src/database/mod.rs` with DAO modules under `src-tauri/src/database/dao/`.
  - Tables include providers, provider endpoints, MCP servers, prompts, skills, settings, proxy config, provider health, proxy request logs, model pricing, stream checks, proxy live backup, usage rollups, session log sync, and model routes in `src-tauri/src/database/schema.rs`.

**File Storage:**
- CC-Switch config root: `$HOME/.cc-switch` or `CC_SWITCH_CONFIG_DIR`, containing `cc-switch.db`, `settings.json`, `skills/`, backups, `codex_oauth_auth.json`, and `copilot_auth.json`.
- Live config files:
  - Claude: `~/.claude/settings.json`, `~/.claude.json`, `~/.claude/CLAUDE.md`.
  - Codex: `~/.codex/auth.json`, `~/.codex/config.toml`, `~/.codex/AGENTS.md`, `~/.codex/models_cache.json`.
  - Gemini: `~/.gemini/.env`, `~/.gemini/settings.json`, `~/.gemini/GEMINI.md`.
  - OpenCode: `~/.config/opencode/opencode.json`, `~/.config/opencode/AGENTS.md`.
  - Hermes: config path resolved by `src-tauri/src/hermes_config.rs`.
  - OpenClaw: `~/.openclaw/openclaw.json`, `~/.openclaw/AGENTS.md`.
- WebDAV sync artifacts: SQL dump, skills zip, and manifest generated by `src-tauri/src/services/webdav_sync/mod.rs`.

**Caching:**
- In-process caches for Codex OAuth access tokens in `src-tauri/src/proxy/providers/codex_oauth_auth.rs`.
- In-process caches for GitHub Copilot tokens, models, and API endpoints in `src-tauri/src/proxy/providers/copilot_auth.rs`.
- Codex model catalog cache at `~/.codex/models_cache.json`, read by `src-tauri/src/codex_config.rs`.
- Proxy runtime state and request counters in `ProxyServerState` in `src-tauri/src/proxy/server.rs`.

## Authentication & Identity

**Auth Provider:**
- Custom per-provider auth is the default model; provider credentials live in SQLite-backed `Provider.settings_config` and are exported to live assistant config files by service code under `src-tauri/src/services/provider/`.
  - Implementation: provider adapters choose auth strategies in `src-tauri/src/proxy/providers/auth.rs` and adapter files under `src-tauri/src/proxy/providers/`.
- OpenAI/Codex OAuth is managed in `src-tauri/src/proxy/providers/codex_oauth_auth.rs`.
  - Implementation: device code flow, refresh token storage, access-token cache, multi-account selection.
- GitHub Copilot OAuth is managed in `src-tauri/src/proxy/providers/copilot_auth.rs`.
  - Implementation: GitHub device code flow, GitHub token storage, Copilot token exchange, GHES domain support.
- Claude and Codex quota checks can read macOS Keychain when available and fallback to local credential files; see `src-tauri/src/services/subscription.rs`.

## Monitoring & Observability

**Error Tracking:**
- None external. Errors are represented through local `AppError` in `src-tauri/src/error.rs` and proxy-specific errors in `src-tauri/src/proxy/error.rs`.

**Logs:**
- Rust `log` facade with `env_logger` declared in `src-tauri/Cargo.toml`.
- Proxy request and usage logs persist to SQLite tables through `src-tauri/src/proxy/usage/logger.rs` and DAO code under `src-tauri/src/database/dao/`.
- Daemon logging lives in `src-tauri/src/daemon/logging.rs`.
- Stream check results persist through `src-tauri/src/services/stream_check/` and `src-tauri/src/database/dao/stream_check.rs`.
- CI uploads benchmark artifacts from `.github/workflows/benchmark.yml` and `.github/workflows/release.yml`.

## CI/CD & Deployment

**Hosting:**
- Source and releases target GitHub. `src-tauri/Cargo.toml` declares repository `https://github.com/saladday/cc-switch-cli`; `README.md` and `install.sh` point to GitHub release downloads.
- Current checkout remote is proxied through `https://gh-proxy.com/https://github.com/SaladDay/cc-switch-cli.git`; `gh pr status` cannot infer a known GitHub host from that remote, so PR status lookup requires an explicit GitHub repo target.

**CI Pipeline:**
- GitHub Actions:
  - `.github/workflows/rust-ci.yml` runs `cargo fmt --check`, library unit tests, binary unit tests, and focused integration tests in sandboxed config directories.
  - `.github/workflows/benchmark.yml` builds the release binary and runs blocking operation benchmarks.
  - `.github/workflows/release.yml` builds multi-platform artifacts, creates macOS universal binary, signs update assets with Minisign, generates `latest.json`, computes `checksums.txt`, and publishes a GitHub Release with `softprops/action-gh-release@v2`.
- Nix package pipeline is defined in `flake.nix`; tests are disabled there because packaging should not depend on host assistant CLIs or live config fixtures.

## Environment Configuration

**Required env vars:**
- `CC_SWITCH_CONFIG_DIR` - optional override for CC-Switch state; required in tests/CI to avoid host config writes.
- `CLAUDE_CONFIG_DIR` - optional override for Claude config; required in tests/CI to avoid host config writes.
- `CODEX_HOME` - optional override for Codex config; required in tests/CI to avoid host config writes.
- `HOME` and `USERPROFILE` - used for default config roots and sandboxed in CI/tests.
- `XDG_CONFIG_HOME`, `XDG_RUNTIME_DIR`, `XDG_STATE_HOME` - used in tests and app config discovery.
- `CC_SWITCH_MINISIGN_SECRET_KEY` - GitHub Actions release signing secret.
- `GITHUB_TOKEN` - GitHub Actions release publishing token.

**Secrets location:**
- User/provider secrets may be stored in `cc-switch.db`, `settings.json`, app live config files, `codex_oauth_auth.json`, and `copilot_auth.json` under the configured CC-Switch/app config roots. These files must not be read or quoted in codebase maps.
- WebDAV credentials are stored in `WebDavSyncSettings` through `src-tauri/src/settings.rs`.
- Release signing secret is stored only in GitHub Actions secrets and referenced as `CC_SWITCH_MINISIGN_SECRET_KEY` in `.github/workflows/release.yml`.

## Webhooks & Callbacks

**Incoming:**
- Local proxy routes in `src-tauri/src/proxy/handlers.rs`:
  - `GET /health` and `GET /status`.
  - `POST /v1/messages` for Anthropic/Claude messages.
  - `POST /v1/messages/count_tokens` for Claude token counting.
  - `POST /chat/completions` for OpenAI-compatible chat completions.
  - `POST /responses` and `POST /responses/compact` for OpenAI Responses API.
  - Gemini passthrough routes under `/gemini`.
- Daemon IPC is local Unix supervisor/socket logic under `src-tauri/src/daemon/ipc/`.
- Deep-link import protocol `ccswitch://v1/import?...` is parsed under `src-tauri/src/deeplink/`.

**Outgoing:**
- Provider API calls to Anthropic, OpenAI-compatible providers, Google Gemini, OpenRouter/relay endpoints, ChatGPT Codex backend, and GitHub Copilot through `src-tauri/src/proxy/forwarder.rs` and provider adapters.
- OAuth device-code and token polling to OpenAI auth and GitHub/GHES endpoints from `src-tauri/src/proxy/providers/codex_oauth_auth.rs` and `src-tauri/src/proxy/providers/copilot_auth.rs`.
- WebDAV `PROPFIND`, `MKCOL`, `HEAD`, `GET`, and `PUT` requests from `src-tauri/src/services/webdav.rs`.
- GitHub release/update manifest and asset downloads from `src-tauri/src/cli/commands/update.rs`.
- Optional usage/quota probes through `src-tauri/src/services/subscription.rs`, `src-tauri/src/services/coding_plan.rs`, and `src-tauri/src/usage_script.rs`.

---

*Integration audit: 2026-06-12*
