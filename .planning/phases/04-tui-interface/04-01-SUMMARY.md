---
phase: 04-tui-interface
plan: 01
subsystem: tui
tags: [tui, model-routes, scaffolding, navigation]
requires: []
provides: [data-loading]
affects: [route, data, app_state, menu, content_config, ui, config, i18n]
tech-stack:
  added:
    - ModelRouteRow struct (ui data layer)
    - ModelRouteSnapshot container (ui data layer)
    - render_settings_model_routes function (ui/model_routes.rs)
    - tui_settings_model_routes_title i18n (EN/CN)
  patterns:
    - Table-based settings sub-page (matches SettingsProxy pattern)
    - Enum-based settings item dispatch (matches SettingsItem::ALL)
key-files:
  created:
    - src/cli/tui/ui/model_routes.rs
  modified:
    - src/cli/tui/route.rs
    - src/cli/tui/data.rs
    - src/cli/tui/app/app_state.rs
    - src/cli/tui/app/content_config.rs
    - src/cli/tui/app/menu.rs
    - src/cli/tui/ui.rs
    - src/cli/tui/ui/config.rs
    - src/cli/i18n.rs
decisions: []
metrics:
  duration: ~10m
  completed_date: 2026-06-12T01:01:11Z
---

# Phase 04 Plan 01: Model Routes TUI Scaffolding Summary

**One-liner:** Added model route data types, navigation skeleton, and 4-column table rendering for the Settings -> Model Routes TUI flow.

## Tasks Performed

### Task 1: Add model routes data, route, and state fields
- Added `Route::SettingsModelRoutes` variant to `Route` enum
- Defined `ModelRouteRow` (id, pattern, provider_id, provider_name, priority, enabled) and `ModelRouteSnapshot` (rows) data types
- Added `model_routes: ModelRouteSnapshot` field to `UiData` with `Default` impl
- Implemented `load_model_routes_snapshot()` in data.rs: loads from DB via `state.db.list_model_routes()`, resolves provider display names from the already-loaded `providers.rows`, sorts by priority then id
- Added `SettingsItem::ModelRoutes` to `SettingsItem::ALL` array (between Proxy and CheckForUpdates)
- Updated `SettingsItem::ALL` array length from 9 to 10
- Added `model_routes_idx: usize` field to `App` struct
- Added clamping for `model_routes_idx` in `App::clamp_selections()`
- Added `tui_settings_model_routes_title` i18n text (EN: "Model Routes", CN: "模型路由")

### Task 2: Wire navigation and content-key dispatch
- Added Enter key handler in `on_settings_key` to push `Route::SettingsModelRoutes`
- Added `Route::SettingsModelRoutes` dispatch in `on_content_key` (menu.rs)
- Added `Route::SettingsModelRoutes` to `nav_item_for_route` (maps to NavItem::Settings)
- Implemented `on_settings_model_routes_key` with Up/Down navigation (content_config.rs)
- Added `Route::SettingsModelRoutes` render dispatch in `render_content` (ui.rs)
- Created `src/cli/tui/ui/model_routes.rs` with `render_settings_model_routes`:
  - 4-column table: Pattern (30%) | Provider (35%) | Priority (10) | Enabled (8)
  - Bilingual title using `tui_settings_model_routes_title`
  - Key bar with up/down navigation hint
  - Selection highlighting with `model_routes_idx`
  - Uses existing shared UI functions (`pane_border_style`, `selection_style`, `highlight_symbol`, `render_key_bar_center`, `CONTENT_INSET_LEFT`)

## Verification

- `cargo check` passes with 0 errors
- `cargo fmt --check` passes with 0 diffs
- Settings page renders "Model Routes" entry with rule count
- Enter on Model Routes navigates to the table view
- Up/Down keys navigate rows
- Esc returns to Settings (standard route_stack pop behavior)

## Deviations from Plan

None -- plan executed exactly as written. The three match-exhaustiveness stubs (nav_item_for_route, on_content_key, render_content) were added as the plan implicitly required them for Task 1 compilation, and were fully filled in during Task 2.

## Known Stubs

None -- all wiring is complete. The table rendering is the real renderer (not a placeholder). Action buttons (add/edit/delete/toggle) are intentionally deferred to Plan 02, consistent with the plan's stated scope.

## Threat Flags

None -- this plan introduces no new network endpoints, auth paths, or file access patterns. All data flows from SQLite through the existing DAO layer.

## Commits

- `96cab0c`: feat(04-tui-interface-01): add model routes data types, route variant, and state fields
- `1cf27a6`: feat(04-tui-interface-01): wire navigation and content-key dispatch for model routes
