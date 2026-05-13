# Changelog

## [0.1.2] - 2026-05-13

### Added

- Add OpenClaw MCP management support across the CLI/TUI app model.
- Show the installed OpenClaw CLI version in the TUI home local environment check.
- Add visual selection mode for skills management.
- Add OpenClaw skill support and align agent app columns.

### Fixed

- Keep OpenClaw and Hermes app switches persisted in TUI state.
- Prune stale OpenClaw agent model catalog entries when providers are removed.
- Align the OpenClaw current provider marker and default provider keyboard handling.
- Reconcile live app skill enablement and skip managed or bundled skills during agent import.
- Adapt upstream sync changes for cc-switch-tui.

## [0.1.1] — 2026-05-11

### Added

- Publish the Rust crate to crates.io during tagged release workflows.

### Fixed

- Fix OpenClaw provider switching and default model writes when valid upstream config uses flexible default model shapes or empty object values.
- Keep TUI app switching responsive during startup and accept localized app switch hotkey labels.
- Run legacy config directory migration before startup database initialization.

## [0.1.0] — 2026-05-10

Initial release of the renamed cc-switch-tui fork.

### Added

- CC_SWITCH_TUI_CONFIG_DIR env var to override config directory (with `~` expansion)
- Auto-migration from legacy `~/.cc-switch/` to `~/.cc-switch-tui/`
- Hermes support: provider management, MCP, skills, prompts, proxy
- OpenClaw support: provider management, MCP, prompts, proxy
- Interactive prompt for legacy config directory migration

### Changed

- Rename project from cc-switch-cli to cc-switch-tui (package, binaries, config paths)
- Repository URL updated to github.com/handy-sun/cc-switch-tui
- Description updated to include Hermes and OpenClaw

### Fixed

- Embedded line numbers in flake.nix and generate_latest_json.py
- MCP table rendering for Hermes column
- TUI picker navigation bounds for 6-app layout

### Removed

- Sponsor section from README files and partner assets

[0.1.2]: https://github.com/handy-sun/cc-switch-tui/releases/tag/v0.1.2
[0.1.1]: https://github.com/handy-sun/cc-switch-tui/releases/tag/v0.1.1
[0.1.0]: https://github.com/handy-sun/cc-switch-tui/releases/tag/v0.1.0
