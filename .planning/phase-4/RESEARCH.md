# Phase 4 Research: TUI Interface

**Date:** 2026-06-12
**Status:** Complete

## Key Findings

### TUI Architecture
- **Table rendering**: ratatui `Table::new(rows, constraints)` with `Row::new(cells)` — see `ui/providers.rs:290-339`
- **Actions**: `Action` enum variants in `runtime_actions/mod.rs` dispatch to handler functions
- **Config page**: `app/content_config.rs` — Proxy settings area with `LocalProxySettingsItem`
- **Overlays**: `ui/overlay/` — forms (`basic.rs`, `pickers.rs`) and modals
- **Data**: `UiData` struct in `data.rs:832` — TUI data snapshot
- **No React/svelte**: everything is ratatui + crossterm

### Integration Points
1. **New file**: `ui/model_routes.rs` — table rendering + key handling
2. **Runtime actions**: `runtime_actions/model_routes.rs` (NEW) — CRUD handlers
3. **Config page**: Add model routes section to `content_config.rs`
4. **App state**: Add model routes state to `app/app_state.rs`
5. **Data**: Add model routes data to `data.rs` `UiData`

### Patterns to Follow
- Table: `ui/providers.rs` (266 lines) — header, rows, highlight, key bar
- Actions: `runtime_actions/providers.rs` — CRUD via DB calls
- Form: `ui/overlay/basic.rs` — text input overlays

### Simpler Approach
Given the TUI complexity (4545-line data.rs), **embed model routes table directly into the existing proxy settings section** rather than creating a new top-level tab. This minimizes changes:
- Add a "Model Routes" section below proxy settings in `content_config.rs`
- Render a compact 4-column table (Pattern | Provider | Priority | Enabled)
- Add/Edit via simple text input overlays (reuse existing overlay system)
- Delete via confirmation dialog
- Toggle via keyboard shortcut

### Files (estimated)
- `ui/model_routes.rs` — NEW, ~250 lines (table render + key handler)
- `runtime_actions/model_routes.rs` — NEW, ~150 lines (5 handlers)
- `app/content_config.rs` — modify, ~+50 lines (integration)
- `app/app_state.rs` — modify, ~+30 lines (state fields)
- `data.rs` — modify, ~+50 lines (data loading)
- `runtime_actions/mod.rs` — modify, ~+10 lines (action enum + dispatch)
