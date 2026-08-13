# TUI Usability Roadmap

This note tracks the ongoing TUI usability overhaul: what has landed, and the
remaining work in priority order. Update it as items complete.

## Landed (2026-07)

For context — the foundations the remaining items build on:

- **Keymap registry** (`src/cli/tui/keymap.rs`): one binding table per page
  drives both key dispatch and the page key bar, so hints can never drift
  from handlers. Migrated: Providers, MCP, Prompts, Skills (installed),
  Usage.
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
- Word-wrapped, message-adaptive dialogs; breadcrumb titles on sub-pages;
  empty-state guidance on empty lists; `? more` degradation for
  overflowing key bars; help sheet synced with actual bindings.

## Remaining work

### 1. Finish the mechanical migrations (low risk, mostly delegatable)

- **config.rs sub-pages → `render_page_frame`**: Config, WebDAV, OpenClaw
  Workspace/Daily Memory/Env/Tools/Agents, Hermes Memory, Settings,
  Settings › Proxy, Managed Accounts still hand-roll the page shell.
  Visual output is already consistent (padded titles, persistent key
  bars); this is maintenance-only deduplication.
- **Sessions page → keymap registry**: the last main page still
  dispatching raw key codes. Its Enter/R/d/r/a actions are
  pane-dependent (`SessionsPane`), so the migration needs the intent
  handler to keep the pane checks — follow the Providers pattern where
  guards stay in the handler body.

### 2. Help sheet generated from the keymap registry

`texts::tui_help_text*` page-key lines are still hand-written prose. Once
Sessions is migrated, generate the per-page lines from
`keymap::<page>::BINDINGS` (display + label, skipping `shown == never`
aliases) so dispatch, key bars, and help share one source of truth. The
static text should keep the global-keys and text-editing sections.

### 3. Terminal compatibility: icon fallback (issue #314 class)

Nav/emoji glyphs (🏠 🔑 …) render double-width on some SSH/legacy
terminals and break border alignment. Plan:

- `CC_SWITCH_ICONS=ascii|emoji|auto` env override plus a Settings row;
- `auto`: fall back to ASCII markers when the locale is not UTF-8
  (`LC_ALL`/`LC_CTYPE`/`LANG` without `utf-8`), mirroring the
  color-mode philosophy — add per-terminal cases, never flip defaults
  (see the pinned tests in `theme.rs`).

### 4. Provider form decomposition (largest remaining UX item)

The add/edit form spans ~60 fields across six apps in one scrolling
table. Plan:

- **Add flow**: show only the essentials (Name / Base URL / API Key +
  template row); collapse the rest behind an "Advanced" section header
  (the current divider rows become collapsible groups).
- **JSON preview on demand** (e.g. `F3`) instead of a permanent 45%
  column, returning width to the fields table.
- Sub-pages already show breadcrumbs; also surface a toast when `Ctrl+S`
  is ignored on a sub-page (`form_handlers/mod.rs` refuses silently).

### 5. Command palette (optional, largest discoverability win)

`Ctrl+P` (or `:`) fuzzy palette over the 24 routes plus per-page intents.
The `Route` enum and keymap intent tables make the candidate list nearly
free; the work is the overlay UX and dispatch plumbing.

### 6. Key vocabulary leftovers

- Case-pair traps kept for now: Sessions `R` (restore) vs `r` (refresh),
  Skill detail `s` (sync) vs `S` (sync all). If they cause real
  mis-presses, prefer confirm dialogs over rebinding.
- Usage `P` (pricing) vs Main `p` (proxy) cross-screen overload:
  tolerated because both are chip-labeled.

### 7. Upstream housekeeping (not TUI, found along the way)

- `.gitignore` line `skills/` is unanchored and swallows
  `src/cli/tui/ui/skills/` — new files there are silently ignored
  (`git add` needs `-f`). Should be `/skills/`.
- `src/cli/i18n/texts/` is an uncompiled copy of the inline `texts`
  module (nothing declares `mod texts;` against the directory) and has
  already diverged from `i18n.rs`. Either finish that split or delete
  the directory.

## Conventions for new work

- New dialogs: describe size/title/keys/body via `overlay_frame` — do
  not hand-roll chrome. Fixed-option pickers use `FitRows`.
- New pages: `render_page_frame` + a `keymap` module binding table.
- New palette colors: add both dark and light RGBs plus a curated
  ansi256 pin test in `theme.rs`.
- Every key visible in a bar must resolve through the same table the
  handler reads.
