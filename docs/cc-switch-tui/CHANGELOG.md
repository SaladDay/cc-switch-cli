# Changelog

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

[0.1.0]: https://github.com/handy-sun/cc-switch-tui/releases/tag/v0.1.0
