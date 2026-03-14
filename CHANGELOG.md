# Changelog

All notable changes to CC Switch CLI will be documented in this file.

**Note:** This is a CLI fork of the original [CC-Switch](https://github.com/farion1231/cc-switch) project, maintained by [saladday](https://github.com/saladday).

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [5.0.1] - 2026-03-15

### Added

- **Claude / Codex (TUI)**: Add first-switch safety prompts that detect existing live configs, offer import-before-overwrite, and show a one-time common-config tip after the first real provider switch.

### Fixed

- **Update**: Align self-update target resolution with upstream release metadata, harden signed self-update and installer upgrades, and avoid noisy archive extraction locale issues in the install script.
- **TUI**: Preserve the v5 palette across terminal capabilities, including ansi256 fallback paths such as Apple Terminal compatibility mode.
- **Codex**: Allow importing the current live config when `config.toml` exists without `auth.json`, while still avoiding first-use prompts for auth-only states.

### Docs

- **README**: Refresh the version badges and add a short 5.0.1 highlights section in both English and Chinese docs.

### Commits (since v5.0.0)

- 3ee87c4 fix: align update flow with upstream release resolution
- a0ebe7f fix(tui): preserve v5 colors across terminal capabilities
- 060cd7a fix(update): harden signed self-update and installer upgrades
- d3ff0a7 feat(tui): add Claude provider switch safety prompts
- 5a8d227 feat(tui): add Codex provider switch safety prompts

### Thanks

- Thanks `@saladday` for the 5.0.1 self-update hardening, terminal color compatibility work, and Claude/Codex switch safety UX.

## [5.0.0] - 2026-03-13

### Added

- **OpenCode**: Add first-class provider support plus TUI MCP and skills flows.
- **Proxy (TUI)**: Add multi-app proxy management, token telemetry on the dashboard, and restore the local proxy settings flow.
- **TUI**: Add an external editor shortcut for prompt, provider, and MCP editing.
- **Claude**: Align API format support with upstream.

### Changed

- **TUI**: Remove the legacy TUI and consolidate the interactive experience around the ratatui flow, including shared visual tokens, overlay rules, footer blocks, and connection card spacing.
- **Skills / MCP**: Refine dialogs and align skill import and scan behavior with upstream.
- **Stream Check**: Split the backend module for cleaner maintenance.

### Fixed

- **TUI**: Unify MCP and skill import UX, stop showing the restart notice after provider switch, and polish the Claude API format picker.
- **i18n**: Localize the remaining Chinese TUI copy.
- **Claude / Sync**: Align official provider behavior, db-v6 schema, and WebDAV format with upstream.

### Docs & Chore

- **Repo Hygiene**: Ignore local agent artifacts, worktrees, and workspace docs.
- **Docs / Tests**: Refresh agent guidance, move homepage gifs, and align proxy waveform assertions with baseline rendering.

### Commits (since v4.8.0)

- cb7e878 refactor(tui): unify visual style tokens and overlay rules
- b8a2d3a feat(tui): add external editor shortcut
- 64637f2 refactor(stream-check): split backend module
- 64da960 feat(provider): add OpenCode support
- 65d63c9 feat(opencode): add MCP and skills support to TUI
- e6ab625 feat(tui): refine skill and mcp dialogs
- a107593 fix(tui): unify mcp and skill import ux
- 7eabc5e feat(skill): align import and scan with upstream
- 1d6c54b chore: ignore local agent artifacts
- a20adbb chore: ignore worktrees and lock Chinese TUI copy
- 8c39a3d feat(proxy): add multi-app proxy management to TUI
- 1613650 fix(i18n): localize remaining Chinese TUI copy
- e925eec feat(proxy): add token telemetry to the TUI dashboard
- 2817ba1 refactor(cli): remove legacy TUI and align CLI with interactive flows
- 338e621 test(proxy): align waveform assertions with baseline rendering
- a7b593c fix(claude): align official provider behavior with upstream
- 0438577 fix(sync): align db-v6 schema and WebDAV format with upstream
- 9b6ea26 fix(tui): stop showing restart notice after provider switch
- e2c1cc8 feat(claude): align api format support with upstream
- 55792bb feat(tui): redesign footer with NAV/ACT color blocks and add provider Online indicator
- 0b9c077 style(tui): add top padding before connection info card
- 54baf70 fix(tui): polish claude api format picker ux
- 43e634f feat(tui): restore local proxy settings flow and proxy-required switch notice
- c8c86a5 docs: refresh agent guidance and move homepage gifs
- 4235820 chore: stop tracking local workspace docs

### Thanks

- Thanks `@saladday` for driving the v5.0.0 TUI consolidation, multi-app proxy workflow, and OpenCode support.

## [4.8.0] - 2026-02-28

### Added

- **Install Script**: One-liner `curl | sh` installation for macOS and Linux. (#41)
- **TUI**: Claude VSCode plugin takeover toggle and sync. (#47)
- **TUI**: Model auto-fetch picker with search filtering. (#45)

### Changed

- **Docs**: Revamp installation section with quick install as primary method, manual install in collapsible details.
- **Docs**: Update code structure to reflect current architecture (tui, database, ui directory).
- **Docs**: Fix `skills search` → `skills discover` command name.

### Commits (since v4.7.4)

- bedd43e docs: fix skills search -> discover in README
- a5ce614 feat: add install.sh for one-liner curl | sh installation (#41)
- 3249c29 merge: issue #47 Claude VSCode plugin takeover
- 83212f0 feat(tui): add Claude VSCode plugin takeover toggle and sync
- 20e81a7 feat(tui): add model auto-fetch picker with search filtering (#45)

## [4.7.4] - 2026-02-28

### Added

- **WebDAV Sync**: Add automatic V1 -> V2 migration with legacy artifact cleanup.

### Changed

- **WebDAV Sync**: Align sync protocol implementation with upstream v2.
- **Codex**: Migrate legacy config at startup instead of runtime compatibility fallback.
- **Sponsors**: Update RightCode offer to 25% bonus pay-as-you-go credits and update affiliate link.
- **Docs**: Refresh WebDAV compatibility notes and remove duplicated sponsor descriptions.

### Fixed

- **WebDAV Sync**: Sync live config files after WebDAV download, backup restore, and config import.
- **Codex**: Align provider keys with upstream `model_provider` + `model_providers` format.
- **MCP**: Skip MCP sync when target app is not installed. (#43)
- **Deeplink/Codex**: Fix missing `requires_openai_auth`, strip loop bug, and deduplicate `clean_codex_provider_key`.
- **WebDAV Sync**: Correct `webdav_status_error` parameter order in `delete_collection`.

### Commits (since v4.7.3)

- 16b556f fix: update RightCode affiliate link to ccswitch-cli
- b883a28 feat: update RightCode promotion to 25% bonus credits, remove promo code
- 67f8262 fix: sync live config files after WebDAV download, backup restore, and config import
- faab17f fix: align Codex config with upstream model_provider + model_providers format
- 29b9bb3 fix: skip MCP sync when target app is not installed (#43)
- 5ac3fa5 refactor: align WebDAV sync with upstream v2 protocol
- 6a8ca50 feat: add V1 → V2 WebDAV sync migration with automatic detection and cleanup
- df0fb0e Update README.md
- 5d48583 Update README with WebDAV sync compatibility
- 2c1cfe5 refactor: migrate legacy Codex config at startup instead of runtime compat
- a5d1de9 fix: deeplink missing requires_openai_auth, strip loop bug, dedup clean_codex_provider_key
- 53d5139 fix: correct webdav_status_error param order in delete_collection
- 81eec41 docs: remove duplicate PackyCode README descriptions

## [4.7.3] - 2026-02-24

### Fixed

- **TUI**: Normalize Ctrl+H to Backspace for SSH terminal compatibility (e.g. Xshell sends `\x08` instead of `\x7F`). Fixes #38.

### Changed

- **Providers (TUI)**: Promote RightCode from built-in template to sponsor preset with partner meta, now available for all app types (Claude/Codex/Gemini) with `* ` prefix.
- **Docs**: Add RightCode sponsor section to README (EN/ZH).

## [4.7.2] - 2026-02-08

### Added

- **Settings (TUI)**: Add self-update check/download/apply flow in Settings.

### Fixed

- **Settings (TUI)**: Make update-check cancelable; use ←/→ to switch Update/Cancel; improve update overlay layout and key hints.

## [4.7.1] - 2026-02-07

### Added

- **Providers (TUI)**: Add RightCode provider template.

### Changed

- **TUI**: Tighten layout spacing and reduce list left padding.
- **Codex (TUI)**: Split auth/config previews for clearer navigation.
- **Docs**: Update home screenshots (EN/ZH).

### Fixed

- **Codex**: Align provider config and auth handling with upstream.
- **TUI**: Add left padding between nav border and icons.

## [4.7.0] - 2026-02-07

### Added

- **WebDAV Sync (TUI)**: Add WebDAV sync workflow with TUI controls and home status.
- **CLI**: Add hardened self-update command.
- **Settings**: Add Claude Code onboarding skip toggle.
- **Providers (TUI)**: Add Claude model popup config in provider form.

### Changed

- **Providers (TUI)**: Improve provider JSON editor and modal UX.

### Fixed

- **TUI**: Fix general config editor scroll offset; align editor cursor under soft-wrap; use ASCII PackyCode chip label.
- **Codex**: Stabilize OpenAI auth switching; use TOML snippets and avoid `auth.json`.
- **CLI**: Remove unused global `--json` flag.
- **i18n**: Codex snippet prompt uses TOML.

### Commits (since v4.6.2)

- ec78dad fix(tui): use ASCII PackyCode chip label
- f6ba975 feat(provider): support notes field across cli and tui
- 2f1e516 feat(tui): add Claude model popup config in provider form
- be05cbf feat(tui): improve provider JSON editor and modal UX
- 01897e3 feat(settings): add Claude Code onboarding skip toggle
- 71374dd Merge pull request #25 from SaladDay/sync-upstream-skip-claude-onboarding
- 64dbff8 fix(cli): remove unused global --json flag
- 672299c fix(codex): stabilize OpenAI auth switching
- ada90fc fix(tui): align editor cursor under soft-wrap
- b69ced1 feat(cli): add hardened self-update command
- 108e291 feat(webdav): add sync workflow, tui controls, and home status
- 0624d01 Merge pull request #26 from SaladDay/bugfix-general-config-editor-scroll-offset
- 50fbe19 fix(codex): use TOML snippets and avoid auth.json
- 590c461 fix(i18n): codex snippet prompt uses TOML
- a9d63b5 Merge pull request #27 from skyswordw/codex/add-self-update-command-clean-upstream
- b3cd0ef Merge origin/main into feat/webdav-sync-v1
- 1114997 Merge pull request #28 from SaladDay/feat/webdav-sync-v1

## [4.6.2] - 2026-02-05

### Changed

- **Providers (TUI/CLI)**: Hide the `Notes` field from add/edit flows; optional fields now only include sort index.
- **TUI**: Shorten the Website URL label (remove "(optional)") to avoid layout truncation.

## [4.6.1] - 2026-02-05

### Added

- **Providers (TUI)**: Add sponsor provider presets (PackyCode) for Claude/Codex/Gemini in the "Add Provider" form.
- **Docs**: Add PackyCode sponsor section to README (EN/ZH), including website, registration link, and promo code `cc-switch-cli` (10% off).

## [4.6.0] - 2026-02-05

### Added

- **Interactive (TUI)**: Home now includes a local environment check for installed tool versions (Claude/Codex/Gemini/OpenCode), with `r` to refresh.
- **Providers (TUI)**: Gemini provider forms now support configuring `GEMINI_MODEL` (API Key auth).

### Changed

- **Release**: Publish versionless GitHub Release assets; README download links updated accordingly.
- **Storage**: Align SQLite store implementation with upstream for parity.

### Fixed

- **Codex**: Support provider configs that contain a full `config.toml` (extract provider settings from `model_provider`/`model_providers`).

## [4.5.0] - 2026-02-01

### Added

- **Interactive (TUI)**: New provider/MCP add & edit forms (templates + fields + JSON preview).
- **Providers (TUI)**: Per-provider toggle to attach (or skip) the common config snippet when applying.

### Changed

- **Interactive (TUI)**: API keys are shown in add/edit forms (masked only in provider detail view).
- **Interactive (TUI)**: JSON preview hides cc-switch internal metadata fields.

### Fixed

- **Interactive (TUI)**: Switching from an official template back to Custom now clears template-filled values.

## [4.4.0] - 2026-01-31

### Added

- **Interactive (TUI)**: New ratatui-based TUI (recommended).
- **Interactive (MCP)**: Multi-app selector (`m`) to enable/disable MCP servers per app.
- **Sync**: Safe auto live-sync policy (skip live writes/deletes when the target app is uninitialized).
- **Skills**: Align SSOT skills system with upstream; auto-migrate SSOT on list.

### Changed

- **Alignment**: Upstream backend parity Phase 1/2 (Provider + MCP behavior and data model alignment).

### Fixed

- **Deeplink**: Validate endpoints and homepage during provider import.

## [4.3.1] - 2026-01-29

### Fixed

- **Codex**: Fix provider switching where `base_url` could be lost and `wire_api` could reset to `chat` after switching providers multiple times. Fixes #15.

## [4.3.0] - 2026-01-17

### Added

- **Interactive**: Search/filter functionality for menus:
  - Main menu: Press `/` to enter search mode, filter menu items by keyword
  - All select menus (providers, MCP servers, prompts): Type to filter (fuzzy matching enabled)
  - Help text updated to inform users about filtering capability
  - Esc clears active filter before exiting

## [4.2.11] - 2026-01-17

### Fixed

- **Interactive**: Enable paste functionality in provider add/edit prompts by disabling bracketed paste mode. Fixes issue where paste appeared broken in terminals with bracketed paste enabled (zsh/fish, tmux/zellij).

## [4.2.10] - 2026-01-08

### Added

- **Interactive**: Per-app theme colors in TUI: Codex (green), Claude (cyan), Gemini (magenta), including submenu prompts.

## [4.2.9] - 2026-01-08

### Fixed

- **Codex**: Update the default model to `gpt-5.2-codex` (aligns with latest Codex config docs).

## [4.2.8] - 2026-01-08

### Fixed

- **Providers**: The “Official provider” add mode is now Codex-only (Claude Code uses third-party flow only).
- **Codex (Official provider)**: Skip prompts for website URL / model / wire API; uses `requires_openai_auth = true` and `wire_api = "responses"` by default.

## [4.2.7] - 2026-01-08

### Added

- **Providers (Codex/Claude)**: Under `➕ Add Provider`, add a submenu to choose **Official** vs **Third-Party** providers.

## [4.2.6] - 2026-01-08

### Fixed

- **Codex**: Allow switching providers when `~/.codex/auth.json` is absent (credential store / keyring mode).

## [4.2.5] - 2026-01-08

### Added

- **Interactive**: Use `←/→` on the main menu to switch the current application.

## [4.2.4] - 2026-01-06

### Added

- **Interactive**: Press `Esc` to go back to the previous step (no more “Selection cancelled” errors).

## [4.2.3] - 2026-01-06

### Fixed

- **Interactive**: Clear the terminal between screens/menus to prevent ghosting (“拖影”) and keep the UI clean.

## [4.2.2] - 2026-01-06

### Fixed

- **Interactive**: The common config snippet editor now uses the same external editor flow as provider JSON editing (opens your `$EDITOR` via external editor). Fixes #11.

## [4.2.1] - 2026-01-06

### Added

- **Interactive**: Add a JSON editor in `⚙️ Configuration Management → 🧩 Common Config Snippet` to edit/apply per-app common config snippets (Claude use-case: shared env like `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC`). Fixes #11.

## [4.2.0] - 2026-01-06

### Added

- **Common Config Snippet**: Add `cc-switch config common` to manage per-app common config snippets (useful for shared Claude settings like `env.CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` and `includeCoAuthoredBy`). Fixes #11.

### Changed

- **Claude**: Merge the common config snippet into the live `~/.claude/settings.json` when switching providers.
- **Claude**: Strip values matching the common snippet when backfilling provider snapshots, so common settings stay global across providers.

## [4.1.4] - 2026-01-06

### Fixed

- **Providers (Interactive)**: When adding the first provider for an app, auto-set it as the current provider (prevents “current provider is empty” and unlocks switching). Fixes #10.

## [4.1.3] - 2026-01-06

### Fixed

- **Codex (0.63+)**: Avoid writing `env_key = "OPENAI_API_KEY"` into `~/.codex/config.toml` by default (prevents `Missing environment variable: OPENAI_API_KEY`).
- **Codex**: Generate provider config using `requires_openai_auth = true` for OpenAI-auth flows; interactive provider add/edit now lets you choose auth mode.

## [4.1.0] - 2025-11-25

### Added

- **Interactive Provider Management**: Complete implementation of add/edit provider flows in interactive mode
  - Full-featured provider creation with validation
  - In-place provider editing with current values pre-filled
  - ID column display in provider tables for easier reference
- **Port Testing**: Added endpoint connectivity testing for API providers
  - Test reachability of API endpoints before switching
  - Validates base URLs and ports are accessible
- **Prompts Deactivate Command**: New `prompts deactivate` command to disable active prompts
  - Supports multi-app deactivation (Claude/Codex/Gemini)
  - Removes active prompt files from app directories
- **Toggle Prompt Mode**: Added ability to switch between prompt switching modes
  - Configure how prompts are activated and managed
  - Interactive mode support for toggling settings
- **Environment Management Commands**: Full implementation of environment variable detection
  - `env check`: Detect conflicting API keys in system environment
  - `env list`: List all relevant environment variables by app
  - Helps identify issues when provider switching doesn't take effect
- **Delete Commands for Prompts**: Multi-app support for deleting prompts
  - Delete prompts from all configured apps at once
  - Proper cleanup of prompt files and configuration

### Changed

- **Interactive Mode Refactoring**: Reorganized into modular structure (~1,254 lines reorganized)
  - Split into 6 focused submodules: `provider.rs`, `mcp.rs`, `prompts.rs`, `config.rs`, `settings.rs`, `utils.rs`
  - Improved code maintainability and separation of concerns
  - Better error handling and user feedback
- **Command Output Enhancement**: Improved formatting and alignment in command mode
  - Better table formatting for command-line output
  - Consistent status indicators and color coding
- **Backup Management**: Enhanced interactive backup selection and management
  - Improved backup listing with timestamps
  - Better restore flow with confirmation prompts

### Fixed

- Command mode table alignment issues in provider display
- ID column visibility in interactive provider lists
- Provider add/edit validation edge cases

### Removed

- Environment variable set/unset features (removed for safety)
  - Users must manually manage environment variables
  - Tool now focuses on detection only to prevent accidental overwrites

### Technical

- 15 commits since v4.0.1
- Cargo.toml version updated to 4.1.0
- Core business logic preserved at 100%
- All changes maintain backward compatibility with existing configs

---

## [4.0.2-cli] - 2025-11-24

### Changed

- **Interactive CLI Refactoring**: Reorganized interactive mode into modular structure (~1,254 lines)
  - Split functionality into 6 focused submodules: `provider.rs`, `mcp.rs`, `prompts.rs`, `config.rs`, `settings.rs`, `utils.rs`
  - Improved code maintainability and separation of concerns
- **Provider Display Enhancement**: Replaced "Category" field with "API URL" in interactive mode
  - Provider list now shows actual API endpoints instead of category labels
  - Detail view displays full API URL with app-specific extraction logic
  - Added support for Claude (`ANTHROPIC_BASE_URL`), Codex (`base_url` from TOML), Gemini (`GEMINI_BASE_URL`)

### Added

- Configuration management menu with 8 operations (export, import, backup, restore, validate, reset, show full, show path)
- Enhanced MCP management options (delete, enable/disable servers, import from live config, validate command)
- Extended prompts management (view full content, delete prompts, view current prompt)
- ~395 lines of new i18n strings for configuration, MCP, and prompts operations

### Removed

- Category selection prompt in "Add Provider" interactive flow
- Category column from provider list tables in interactive mode

---

## [4.0.1-cli] - 2025-11-24

### Fixed

- Documentation updates and corrections

---

## [4.0.0-cli] - 2025-11-23 (CLI Edition Fork)

### Overview

Complete migration from Tauri GUI application to standalone CLI tool. This is a **CLI-focused fork** of the original CC-Switch project.

### Breaking Changes

- Removed Tauri GUI (desktop window, React frontend, WebView runtime)
- Removed system tray menu, auto-updater, deep link protocol (`ccswitch://`)
- Users must transition to command-line or interactive mode

### New Features

- **Dual Interface Modes**: Command-line mode + Interactive TUI mode
- **Default Interactive Mode**: Run `cc-switch` without arguments to enter interactive mode
- **Provider Management**: list, add, edit, delete, switch, duplicate, speedtest
- **MCP Server Management**: list, add, edit, delete, enable/disable, sync, import/export
- **Prompts Management**: list, activate, show, create, edit, delete
- **Configuration Management**: show, export, import, backup, restore, validate, reset
- **Utilities**: shell completions (bash/zsh/fish/powershell), env check, app switch

### Removed

- Complete React 18 + TypeScript frontend (~50,000 lines)
- Tauri 2.8 desktop runtime and all GUI-specific features
- 200+ npm dependencies

### Preserved

- 100% core business logic (ProviderService, McpService, PromptService, ConfigService)
- Configuration format and file locations
- Multi-app support (Claude/Codex/Gemini)

### Technical

- **New Stack**: clap v4.5, inquire v0.7, comfy-table v7.1, colored v2.1
- **Binary Size**: ~5-8 MB (vs ~15-20 MB GUI)
- **Startup Time**: <50ms (vs 500-1000ms GUI)
- **Dependencies**: ~20 Rust crates (vs 200+ npm + 50+ Rust)

### Credits

- Original Project: [CC-Switch](https://github.com/farion1231/cc-switch) by Jason Young
- CLI Fork Maintainer: saladday

---

## [3.7.1] - 2025-11-22

### Fixed

- Skills third-party repository installation (#268)
- Gemini configuration persistence
- Dialog overlay click protection

### Added

- Gemini configuration directory support (#255)
- ArchLinux installation support (#259)

### Improved

- Skills error messages i18n (28+ messages)
- Download timeout extended to 60s

---

## [3.7.0] - 2025-11-19

### Major Features

- **Gemini CLI Integration** - Third major application support
- **MCP v3.7.0 Unified Architecture** - Single interface for Claude/Codex/Gemini
- **Claude Skills Management System** - GitHub repository integration
- **Prompts Management** - Multi-preset system prompts
- **Deep Link Protocol** - `ccswitch://` URL scheme
- **Environment Variable Conflict Detection**

### New Features

- Provider presets: DouBaoSeed, Kimi For Coding, BaiLing
- Common config migration to `config.json`
- macOS native design color scheme

### Statistics

- 85 commits, 152 files changed
- Skills: 2,034 lines, Prompts: 1,302 lines, Gemini: ~1,000 lines

---

## [3.6.0] - 2025-11-07

### New Features

- Provider Duplicate
- Edit Mode Toggle
- Custom Endpoint Management
- Usage Query Enhancements
- Auto-sync on Directory Change
- New Provider Presets: DMXAPI, Azure Codex, AnyRouter, AiHubMix, MiniMax

### Technical Improvements

- Backend: 5-phase refactoring (error handling, commands, services, concurrency)
- Frontend: 4-stage refactoring (tests, hooks, components, cleanup)
- Hooks unit tests 100% coverage

---

## [3.5.0] - 2025-01-15

### Breaking Changes

- Tauri commands only accept `app` parameter (values: `claude`/`codex`)
- Frontend type unified to `AppId`

### New Features

- MCP (Model Context Protocol) Management
- Configuration Import/Export
- Endpoint Speed Testing

---

## [3.4.0] - 2025-10-01

### Features

- Internationalization (i18next) with Chinese default
- Claude plugin sync
- Extended provider presets
- Portable mode and single instance enforcement

---

## [3.3.0] - 2025-09-22

### Features

- VS Code integration for provider sync _(Removed in 3.4.x)_
- Codex provider wizard enhancements
- Shared common config snippets

---

## [3.2.0] - 2025-09-13

### New Features

- System tray provider switching
- Built-in update flow via Tauri Updater
- Single source of truth for provider configs
- One-time migration from v1 to v2

---

## [3.1.0] - 2025-09-01

### New Features

- **Codex application support** - Manage auth.json and config.toml
- Multi-app config v2 structure
- Automatic v1→v2 migration

---

## [3.0.0] - 2025-08-27

### Major Changes

- **Complete migration from Electron to Tauri 2.0**
- 90% reduction in bundle size (~150MB → ~15MB)
- Significantly improved startup performance

---

## [2.0.0] - Previous Electron Release

- Multi-provider configuration management
- Quick provider switching
- Import/export configurations

---

## [1.0.0] - Initial Release

- Basic provider management
- Claude Code integration
