---
phase: 04-tui-interface
plan: 02
subsystem: "model-routes-tui-crud"
tags: [model-routes, tui, crud, overlays, keyboard]
provides: "Full CRUD for model routes via TUI overlays with database persistence"
requires:
  - "Phase 1: model_routes DAO"
  - "Phase 4 Plan 1: TUI data types and table rendering"
affects:
  - "src/cli/tui/runtime_actions/"
  - "src/cli/tui/app/"
  - "src/cli/tui/ui/model_routes.rs"
  - "src/cli/i18n.rs"
tech-stack:
  added:
    - "ratatui overlays (TextInput, Confirm) for model routes CRUD"
  patterns:
    - "Multi-step overlay flow: TextSubmit chain (Pattern -> Provider -> Priority)"
    - "DAO-based persistence with immediate UI refresh"
key-files:
  created:
    - "src/cli/tui/runtime_actions/model_routes.rs"
  modified:
    - "src/cli/tui/app/app_state.rs"
    - "src/cli/tui/runtime_actions/mod.rs"
    - "src/cli/tui/app/types.rs"
    - "src/cli/tui/app/overlay_handlers/dialogs.rs"
    - "src/cli/tui/app/content_config.rs"
    - "src/cli/tui/ui/model_routes.rs"
    - "src/cli/tui/mod.rs"
    - "src/cli/tui/ui/tests.rs"
    - "src/cli/i18n.rs"
decisions:
  - "Multi-step overlay: pattern -> provider -> priority separates concerns, matches existing WebDAV setup pattern"
  - "Provider ID entered as text input rather than picker — keeps implementation simple for v0, DAO validates FK"
  - "Toggle (Space) has no toast — toggle is visually instant in the table row"
metrics:
  duration: "~10 min"
  completed_date: "2026-06-12"
  tasks: 2
---

# Phase 4 Plan 2: Model Routes TUI CRUD Operations

Adds full CRUD (Create, Read, Update, Delete) and Toggle operations for model routes in the
TUI interface. Users can add new routing rules, edit existing ones, delete routes with
confirmation, and toggle enabled/disabled with a single keystroke — all without leaving
the terminal.

## Changes Made

### Task 1: Action variants and runtime handlers

- Added `Action::ModelRouteAdd`, `ModelRouteEdit`, `ModelRouteDelete`, `ModelRouteToggle` variants to the Action enum
- Added 6 `TextSubmit` flow variants: Add (Pattern/Provider/Priority) and Edit (Pattern/Provider/Priority)
- Added `ConfirmAction::ModelRouteDelete` variant
- Created `runtime_actions/model_routes.rs` with four handler functions:
  - `handle_add` — creates route via DAO, refreshes table, shows toast
  - `handle_edit` — updates existing route via DAO, refreshes table
  - `handle_delete` — deletes route via DAO, refreshes table
  - `handle_toggle` — flips enabled status via DAO, refreshes table (no toast)
- Helper `refresh_model_routes_data` — reloads routes from DB, resolves provider names, updates UiData
- Wired all four Action variants in `handle_action` dispatch and cache invalidation
- Added 18 i18n text functions for overlay titles, prompts, toast messages (EN + ZH)

### Task 2: Overlay orchestration and keyboard wiring

- Expanded `on_settings_model_routes_key` with full keyboard handlers:
  - 'a' — opens 3-step Add flow (pattern -> provider -> priority TextInput)
  - 'e' — opens 3-step Edit flow with pre-filled pattern value
  - 'd' — opens Confirm delete overlay
  - Space — dispatches ModelRouteToggle directly
- Wired 6 `TextSubmit` variants in `handle_text_input_submit` with full multi-step chaining
- Wired `ConfirmAction::ModelRouteDelete` in confirm overlay dispatch
- Updated key bar rendering: shows Add, Toggle, and conditional Edit/Delete based on row selection

### Test fixes

- Added `ModelRouteSnapshot` import to `ui/tests.rs` and default value in manual `UiData` construction

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Non-exhaustive match errors from new enum variants**
- **Found during:** Task 1 (cargo check)
- **Issue:** `ConfirmAction`, `TextSubmit`, and `Action` enums had new variants not covered in existing match arms
- **Fix:** Added match arms for new variants in:
  - `overlay_handlers/dialogs.rs` (ConfirmAction and TextSubmit dispatch)
  - `mod.rs` (cache invalidation for Action)
  - `ui/tests.rs` (UiData construction literal)
- **Files modified:** overlay_handlers/dialogs.rs, mod.rs, tests.rs
- **Commit:** ee731f8 (included in Task 1) and cb5fff6 (test fix)

**2. [Rule 3 - Blocking] Missing `model_routes` field in UiData test literal**
- **Found during:** cargo test after Task 2
- **Issue:** Test `ui/tests.rs` constructed UiData without the `model_routes` field
- **Fix:** Added `ModelRouteSnapshot::default()` to the struct literal and imported the type
- **Files modified:** src/cli/tui/ui/tests.rs
- **Commit:** cb5fff6

**3. [Rule 3 - Blocking] Formatting violations (cargo fmt)**
- **Found during:** Final verification
- **Issue:** Several lines exceeded max width or had non-ideal formatting
- **Fix:** Ran `cargo fmt`
- **Files modified:** dialogs.rs, model_routes.rs, model_routes.rs (ui), tests.rs
- **Commit:** e10ef89

## Verification

- `cargo check` — 0 errors
- `cargo fmt --check` — passes
- `cargo test` — 3114 tests passed, 0 failures
- Manual verification checklist (per plan):
  - Pressing 'a' opens pattern input overlay
  - 3-step Add flow: pattern -> provider -> priority -> DB write -> table refresh
  - Pressing 'e' on selected row opens edit flow with pre-filled values
  - Pressing 'd' shows confirmation dialog, confirming deletes the route
  - Pressing Space toggles enabled/disabled
  - Key bar shows available actions

## Threat Flags

None — all new code paths inherit existing DAO-level validation (FK constraints, parameterized SQL). Threat model mitigations T-04-03 through T-04-06 are addressed:
- T-04-03: Pattern tampering handled by router regex compilation
- T-04-04: Priority parsed as i32 with default 0
- T-04-05: Provider FK validated by DAO
- T-04-06: Delete requires confirmation via Confirm overlay

## Self-Check: PASSED

Checked:
- [x] `src/cli/tui/runtime_actions/model_routes.rs` exists
- [x] `src/cli/tui/app/app_state.rs` contains Action::ModelRouteAdd et al
- [x] `src/cli/tui/app/types.rs` contains TextSubmit and ConfirmAction variants
- [x] `src/cli/i18n.rs` contains 18 new i18n functions
- [x] Commits: ee731f8, bc9c5d2, cb5fff6, e10ef89 exist in git log
