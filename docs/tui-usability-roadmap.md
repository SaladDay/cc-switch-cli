# TUI Usability Roadmap

This note tracks the ongoing TUI usability overhaul: what has landed, and the
remaining work in priority order. Update it as items complete.

## Landed (2026-07)

For context — the foundations the remaining items build on:

- **Keymap registry** (`src/cli/tui/keymap.rs`): one binding table per page
  drives both key dispatch and the page key bar, so hints can never drift
  from handlers. Migrated: Providers, MCP, Prompts, Skills (installed),
  Usage, Sessions. Sessions binds only its action keys (Enter/R/d/r/a);
  pane/list navigation stays explicit in the handler (pane-dependent and
  reused by the filter path), with a static nav-hint prefix on the bar.
- **Overlay frame** (`src/cli/tui/ui/overlay/frame.rs`): all ~24 dialogs
  render through `overlay_frame`/`overlay_frame_at` with unified body
  padding; fixed-count pickers size to their options (`FitRows`).
- **Page frame** (`ui/shared.rs::render_page_frame`): shared page shell
  (bordered padded title + always-visible key bar + summary bar) for the
  five main list screens.
- **Theme system** (`src/cli/tui/theme.rs`): dark/light palettes with
  semantic colors (`fg_strong`, `on_accent`, `on_comment`), Settings ›
  Theme (Auto/Dark/Light, persisted), COLORFGBG auto-detection, curated
  ansi256 pins for both palettes.
- **Help sheet generation** (`src/cli/tui/help.rs::global_help_lines`): the
  MCP/Prompts/Sessions/Skills/Usage page lines are generated from
  `keymap::<page>::help_items` (a `never` sentinel + `fn_addr_eq` skips
  hidden aliases like Usage's reverse-Tab), so those hints track dispatch.
  Providers/Config/Settings and the Hermes-only Memory line stay
  hand-written (`texts::tui_help_line_*`) for their app-scope prose; the
  static prelude is `texts::tui_help_prelude`. `context_help_for_app` now
  takes `&UiData` to evaluate the keymap labels.
- **Icon fallback** (`src/cli/tui/icons.rs`): `CC_SWITCH_ICONS=auto|emoji|
  ascii` env override + a persisted Settings › Icons row, mirroring the
  color-mode philosophy. `Auto` keeps emoji unless the locale is clearly
  not UTF-8 (never flips the default when locale info is absent). The nav
  sidebar collapses its emoji column to zero width in ASCII mode and
  page/overlay titles strip a leading emoji via `icons::strip_icon`, so
  wide glyphs can no longer break border alignment on legacy terminals.
  Also accepts the full-width `？` as the help hotkey.
- Word-wrapped, message-adaptive dialogs; breadcrumb titles on sub-pages;
  empty-state guidance on empty lists; `? more` degradation for
  overflowing key bars.

## Remaining work

### 1. Finish the mechanical migrations (low risk, mostly delegatable)

- **config.rs sub-pages → `render_page_frame`**: **done** for Config,
  WebDAV, Settings, and Managed Accounts (the clean 1:1 fits — the last
  uses the frame's `Some(summary)` path then splits the body into two
  columns). Added `shared::breadcrumb_path` (unpadded) for frame callers,
  since the frame wraps the title itself. Still hand-rolled — and
  deliberately skipped because their layouts don't match the frame:
  - **Settings › Proxy** (`render_settings_proxy`): trailing 2-line
    footer (`[1, Min, 2]`) and a *conditional* key bar.
  - **Hermes Memory** (`render_hermes_memory`): a custom info-row
    paragraph (`[1, 2, Min]`), not a summary bar.
  - **OpenClaw** Env/Tools/Agents routes and Workspace/Daily Memory: a
    section-scroll layout, not a table body.
  These need `render_page_frame` variants (footer slot / info-row slot)
  before they can migrate; not maintenance-only.

Help-sheet generation (was #2) and the icon fallback (was #3, issue #314
class) are both done — see the Landed section. A possible follow-up on the
icon work: per-item ASCII nav markers instead of the current text-only
collapse, if a visible glyph in ASCII mode is wanted.

### 2. Provider form decomposition (largest remaining UX item)

The add/edit form spans ~60 fields across six apps in one scrolling
table. Plan:

- **Add flow**: show only the essentials (Name / Base URL / API Key +
  template row); collapse the rest behind an "Advanced" section header
  (the current divider rows become collapsible groups).
- **JSON preview on demand** (e.g. `F3`) instead of a permanent 45%
  column, returning width to the fields table.
- Sub-pages already show breadcrumbs; also surface a toast when `Ctrl+S`
  is ignored on a sub-page (`form_handlers/mod.rs` refuses silently).

### 3. Command palette (optional, largest discoverability win)

`Ctrl+P` (or `:`) fuzzy palette over the 24 routes plus per-page intents.
The `Route` enum and keymap intent tables make the candidate list nearly
free; the work is the overlay UX and dispatch plumbing.

### 4. Key vocabulary leftovers

- Case-pair traps kept for now: Sessions `R` (restore) vs `r` (refresh),
  Skill detail `s` (sync) vs `S` (sync all). If they cause real
  mis-presses, prefer confirm dialogs over rebinding.
- Usage `P` (pricing) vs Main `p` (proxy) cross-screen overload:
  tolerated because both are chip-labeled.

### 5. Upstream housekeeping (not TUI, found along the way) — done

- **done** — `.gitignore` `skills/` anchored to `/skills/` so it no
  longer swallows `src/cli/tui/ui/skills/`.
- **done** — deleted `src/cli/i18n/texts/` (the uncompiled divergent copy
  of the inline `texts` module in `i18n.rs`).

## Conventions for new work

- New dialogs: describe size/title/keys/body via `overlay_frame` — do
  not hand-roll chrome. Fixed-option pickers use `FitRows`.
- New pages: `render_page_frame` + a `keymap` module binding table.
- New palette colors: add both dark and light RGBs plus a curated
  ansi256 pin test in `theme.rs`.
- Every key visible in a bar must resolve through the same table the
  handler reads.
