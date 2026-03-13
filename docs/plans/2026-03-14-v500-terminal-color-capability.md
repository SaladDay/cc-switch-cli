# v5.0.0 Terminal Color Capability Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Keep the v5.0.0 TUI color semantics unchanged while adding a minimal, centralized terminal capability layer that preserves RGB on capable terminals and automatically falls back to ansi256 on known non-truecolor terminals such as Apple Terminal.

**Architecture:** Treat the v5.0.0 Dracula RGB palette as the only design source of truth. Implement all compatibility behavior inside `src-tauri/src/cli/tui/theme.rs` so UI components continue to consume the same `Theme` tokens and `terminal_palette_color()` helper without per-screen hacks. Prefer explicit capability detection plus a small token-aware ansi256 mapping table for the fixed palette, then fall back to generic nearest-color mapping for any remaining ad-hoc RGB values.

**Tech Stack:** Rust, ratatui, crossterm, Rust unit tests in `src-tauri/src/cli/tui/theme.rs`, existing TUI rendering tests in `src-tauri/src/cli/tui/ui/tests.rs`

---

### Task 1: Lock the capability policy with failing tests first

**Files:**
- Modify: `src-tauri/src/cli/tui/theme.rs`
- Test: `src-tauri/src/cli/tui/theme.rs`

**Step 1: Write the failing test for Apple Terminal auto-fallback**

Add a test that sets:

```rust
let _no_color = EnvGuard::remove("NO_COLOR");
let _color_mode = EnvGuard::remove("CC_SWITCH_COLOR_MODE");
let _colorterm = EnvGuard::remove("COLORTERM");
let _term = EnvGuard::set("TERM", "xterm-256color");
let _term_program = EnvGuard::set("TERM_PROGRAM", "Apple_Terminal");

let theme = theme_for(&AppType::Claude);

assert_eq!(detected_color_mode(), ColorMode::Ansi256);
assert!(matches!(theme.accent, Color::Indexed(_)));
```

Use a precise name like:

```rust
fn theme_uses_ansi256_in_apple_terminal_without_truecolor_signal()
```

**Step 2: Run the test to verify it fails**

Run:

```bash
cargo test cli::tui::theme::tests::theme_uses_ansi256_in_apple_terminal_without_truecolor_signal
```

Expected: FAIL because current auto-detection still returns `TrueColor`.

**Step 3: Add a second policy test for the default modern-terminal path**

Keep a separate test that removes `TERM_PROGRAM`, `TERM`, and `COLORTERM` and asserts the default path remains RGB:

```rust
assert_eq!(detected_color_mode(), ColorMode::TrueColor);
assert_eq!(theme.accent, Color::Rgb(255, 184, 108));
```

This prevents Apple Terminal fallback from accidentally downgrading the general default behavior.

**Step 4: Add an explicit override precedence test if missing**

Ensure one test proves explicit user choice still wins over auto-detection:

```rust
let _color_mode = EnvGuard::set("CC_SWITCH_COLOR_MODE", "truecolor");
let _term_program = EnvGuard::set("TERM_PROGRAM", "Apple_Terminal");
assert_eq!(detected_color_mode(), ColorMode::TrueColor);
```

If the project already has enough override coverage, keep the smallest additional assertion set that protects this rule.

**Step 5: Do not commit**

Per repo instructions, do not create a git commit unless the user explicitly asks.

### Task 2: Implement a centralized, minimal-intrusion capability layer

**Files:**
- Modify: `src-tauri/src/cli/tui/theme.rs:39-180`
- Test: `src-tauri/src/cli/tui/theme.rs`

**Step 1: Add a helper for known non-truecolor terminals**

Add a helper near the other capability functions:

```rust
fn known_ansi256_terminal() -> bool {
    std::env::var("TERM_PROGRAM")
        .map(|value| value == "Apple_Terminal")
        .unwrap_or(false)
}
```

Keep it intentionally small. Only encode terminals we have evidence for. Do not add speculative vendor checks.

**Step 2: Enforce the capability decision order**

Update `detected_color_mode()` to use this order:

```rust
if no_color() {
    return ColorMode::NoColor;
}
if let Some(mode) = color_mode_override() {
    return mode;
}
if known_ansi256_terminal() {
    return ColorMode::Ansi256;
}
if env_supports_truecolor("COLORTERM") || env_supports_truecolor("TERM") {
    return ColorMode::TrueColor;
}
ColorMode::TrueColor
```

This preserves v5.0.0 RGB by default while automatically protecting Apple Terminal.

**Step 3: Stabilize the ansi256 mapping for fixed v5 colors**

First verify the current nearest-color mapping already lands on the desired indices for the fixed palette used by the TUI. If it does, keep the generic mapper and lock those outputs with tests because that is the smaller, more elegant change. Only add a token-aware lookup table if the generic mapping produces visibly worse indices.

If a table is needed, keep it tiny and place it before the generic `rgb_to_ansi256()` fallback:

```rust
fn known_ansi256_index(rgb: (u8, u8, u8)) -> Option<u8> {
    match rgb {
        DRACULA_GREEN => Some(84),
        DRACULA_CYAN => Some(117),
        DRACULA_PINK => Some(212),
        DRACULA_ORANGE => Some(215),
        DRACULA_YELLOW => Some(228),
        DRACULA_RED => Some(203),
        DRACULA_COMMENT => Some(61),
        DRACULA_SURFACE => Some(239),
        (101, 113, 160) => Some(61),
        (248, 248, 248) => Some(231),
        (108, 108, 108) => Some(242),
        (255, 255, 255) => Some(231),
        _ => None,
    }
}
```

Then use it inside `terminal_color()`:

```rust
ColorMode::Ansi256 => Color::Indexed(
    known_ansi256_index(rgb).unwrap_or_else(|| rgb_to_ansi256(rgb.0, rgb.1, rgb.2))
)
```

If the generic mapper already returns these same indices, do not add the table; add tests instead. In either case, the fallback colors must stay stable and as close as possible to the v5.0.0 design intent.

**Step 4: Verify the theme tests pass**

Run:

```bash
cargo test cli::tui::theme::tests::
```

Expected: all theme tests pass, including the new Apple Terminal regression test and the default RGB preservation test.

**Step 5: Do not edit UI color semantics**

Do not change:

- `theme.accent`
- `theme.dim`
- `theme.surface`
- border bold/focus semantics
- per-screen color choices in `ui/*.rs`

All compatibility should remain inside `theme.rs`.

### Task 3: Verify UI surfaces inherit the fix without per-widget hacks

**Files:**
- Modify: `src-tauri/src/cli/tui/ui/tests.rs` (only if a regression test is still missing)
- Verify: `src-tauri/src/cli/tui/ui/chrome.rs`
- Verify: `src-tauri/src/cli/tui/ui/shared.rs`

**Step 1: Keep the existing footer ansi256 regression test**

Use or preserve the existing footer test that asserts ansi256 mode does not emit raw RGB:

```rust
assert!(!matches!(cell.bg, ratatui::style::Color::Rgb(_, _, _)));
assert!(matches!(cell.bg, ratatui::style::Color::Indexed(_)));
```

If the assertion is incomplete, extend it minimally; do not rewrite unrelated UI tests.

**Step 2: Confirm UI semantic tests still match v5 behavior**

Run focused UI tests that already protect v5 semantics:

```bash
cargo test cli::tui::ui::tests::focused_pane_border_keeps_v500_bold_style_in_ansi256_mode
cargo test cli::tui::ui::tests::inactive_pane_border_keeps_v500_dim_color_in_ansi256_mode
cargo test cli::tui::ui::tests::informational_overlay_border_keeps_v500_dim_color_in_ansi256_mode
cargo test cli::tui::ui::tests::footer_uses_terminal_palette_in_ansi256_mode
```

Expected: PASS. This confirms the fix comes from the capability layer, not UI hacks.

**Step 3: Run the full targeted verification suite**

Run:

```bash
cargo fmt && cargo test cli::tui::theme::tests:: && cargo test cli::tui::ui::tests::
```

Expected: formatting succeeds, theme tests pass, and all TUI UI tests pass.

**Step 4: Perform a final review pass**

Review the diff and confirm these statements are true:

- Apple Terminal automatically falls back to ansi256.
- Explicit `CC_SWITCH_COLOR_MODE` still wins.
- Unknown/default terminals still keep v5 RGB.
- UI files did not gain terminal-vendor-specific hacks.

**Step 5: Do not commit**

Leave the branch uncommitted unless the user explicitly requests a commit.
