use ratatui::style::Color;

use crate::app_config::AppType;

const COLOR_MODE_ENV: &str = "CC_SWITCH_COLOR_MODE";

const DRACULA_GREEN: (u8, u8, u8) = (80, 250, 123);
const DRACULA_CYAN: (u8, u8, u8) = (139, 233, 253);
const DRACULA_PINK: (u8, u8, u8) = (255, 121, 198);
const DRACULA_ORANGE: (u8, u8, u8) = (255, 184, 108);
const DRACULA_YELLOW: (u8, u8, u8) = (241, 250, 140);
const DRACULA_RED: (u8, u8, u8) = (255, 85, 85);
const DRACULA_COMMENT: (u8, u8, u8) = (98, 114, 164);
const DRACULA_SURFACE: (u8, u8, u8) = (68, 71, 90);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    NoColor,
    TrueColor,
    Ansi256,
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub accent: Color,
    pub ok: Color,
    pub warn: Color,
    pub err: Color,
    pub dim: Color,
    /// Muted text / secondary info (Dracula comment #6272a4)
    pub comment: Color,
    /// Highlighted values (Dracula cyan #8be9fd)
    pub cyan: Color,
    /// Subtle background / surface (Dracula current-line #44475a)
    pub surface: Color,
    pub no_color: bool,
}

pub fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some()
}

fn parse_color_mode(value: &str) -> Option<ColorMode> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "" | "auto" => None,
        "none" | "no-color" => Some(ColorMode::NoColor),
        "rgb" | "truecolor" | "24bit" | "24-bit" => Some(ColorMode::TrueColor),
        "ansi256" | "ansi-256" | "256" | "256color" | "256-color" => Some(ColorMode::Ansi256),
        _ => None,
    }
}

fn color_mode_override() -> Option<ColorMode> {
    parse_color_mode(&std::env::var(COLOR_MODE_ENV).ok()?)
}

fn env_supports_truecolor(key: &str) -> bool {
    std::env::var(key)
        .map(|value| {
            let normalized = value.to_ascii_lowercase();
            normalized.contains("truecolor")
                || normalized.contains("24bit")
                || normalized.contains("24-bit")
                || normalized.contains("-direct")
                || normalized.ends_with("direct")
        })
        .unwrap_or(false)
}

fn env_supports_ansi256(key: &str) -> bool {
    std::env::var(key)
        .map(|value| {
            let normalized = value.to_ascii_lowercase();
            normalized.contains("256color") || normalized.contains("256-color")
        })
        .unwrap_or(false)
}

fn known_ansi256_terminal() -> bool {
    std::env::var("TERM_PROGRAM")
        .map(|value| value == "Apple_Terminal")
        .unwrap_or(false)
}

fn ssh_plain_xterm_prefers_ansi256() -> bool {
    std::env::var_os("SSH_TTY").is_some()
        && std::env::var("TERM")
            .map(|value| value.eq_ignore_ascii_case("xterm"))
            .unwrap_or(false)
}

fn detected_color_mode() -> ColorMode {
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

    if env_supports_ansi256("TERM") {
        return ColorMode::Ansi256;
    }

    if ssh_plain_xterm_prefers_ansi256() {
        return ColorMode::Ansi256;
    }

    ColorMode::TrueColor
}

fn cube_index(value: u8) -> u8 {
    match value {
        0..=47 => 0,
        48..=114 => 1,
        _ => ((value - 35) / 40).min(5),
    }
}

fn cube_level(index: u8) -> u8 {
    [0, 95, 135, 175, 215, 255][index as usize]
}

fn ansi256_cube(r: u8, g: u8, b: u8) -> (u8, u8, u8, u8) {
    let ri = cube_index(r);
    let gi = cube_index(g);
    let bi = cube_index(b);
    (
        16 + (36 * ri) + (6 * gi) + bi,
        cube_level(ri),
        cube_level(gi),
        cube_level(bi),
    )
}

fn ansi256_gray(r: u8, g: u8, b: u8) -> (u8, u8, u8, u8) {
    let avg = ((r as u16 + g as u16 + b as u16) / 3) as u8;
    let index = if avg <= 8 {
        0
    } else if avg >= 238 {
        23
    } else {
        (((avg as u16 - 8 + 5) / 10) as u8).min(23)
    };
    let level = 8 + index * 10;
    (232 + index, level, level, level)
}

fn color_distance_sq(lhs: (u8, u8, u8), rhs: (u8, u8, u8)) -> u32 {
    let dr = lhs.0 as i32 - rhs.0 as i32;
    let dg = lhs.1 as i32 - rhs.1 as i32;
    let db = lhs.2 as i32 - rhs.2 as i32;
    (dr * dr + dg * dg + db * db) as u32
}

fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    let source = (r, g, b);
    let cube = ansi256_cube(r, g, b);
    let gray = ansi256_gray(r, g, b);

    let cube_distance = color_distance_sq(source, (cube.1, cube.2, cube.3));
    let gray_distance = color_distance_sq(source, (gray.1, gray.2, gray.3));

    if cube_distance <= gray_distance {
        cube.0
    } else {
        gray.0
    }
}

fn terminal_color(color_mode: ColorMode, rgb: (u8, u8, u8)) -> Color {
    match color_mode {
        ColorMode::NoColor => Color::Reset,
        ColorMode::TrueColor => Color::Rgb(rgb.0, rgb.1, rgb.2),
        ColorMode::Ansi256 => Color::Indexed(rgb_to_ansi256(rgb.0, rgb.1, rgb.2)),
    }
}

pub(crate) fn terminal_palette_color(rgb: (u8, u8, u8)) -> Color {
    terminal_color(detected_color_mode(), rgb)
}

fn accent_rgb(app: &AppType) -> (u8, u8, u8) {
    match app {
        AppType::Codex => DRACULA_GREEN,
        AppType::Claude => DRACULA_CYAN,
        AppType::Gemini => DRACULA_PINK,
        AppType::OpenCode => DRACULA_ORANGE,
        AppType::OpenClaw => DRACULA_ORANGE,
    }
}

pub fn theme_for(app: &AppType) -> Theme {
    let color_mode = detected_color_mode();
    let no_color = matches!(color_mode, ColorMode::NoColor);

    Theme {
        accent: terminal_color(color_mode, accent_rgb(app)),
        ok: terminal_color(color_mode, DRACULA_GREEN),
        warn: terminal_color(color_mode, DRACULA_YELLOW),
        err: terminal_color(color_mode, DRACULA_RED),
        dim: terminal_color(color_mode, DRACULA_COMMENT),
        comment: terminal_color(color_mode, DRACULA_COMMENT),
        cyan: terminal_color(color_mode, DRACULA_CYAN),
        surface: terminal_color(color_mode, DRACULA_SURFACE),
        no_color,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct EnvGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(key);
            unsafe { std::env::set_var(key, value) };
            Self { key, previous }
        }

        fn remove(key: &'static str) -> Self {
            let previous = std::env::var_os(key);
            unsafe { std::env::remove_var(key) };
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = self.previous.take() {
                unsafe { std::env::set_var(self.key, value) };
            } else {
                unsafe { std::env::remove_var(self.key) };
            }
        }
    }

    #[test]
    fn opencode_theme_uses_distinct_accent_from_codex() {
        let _lock = env_lock().lock().expect("env lock poisoned");
        let _no_color = EnvGuard::remove("NO_COLOR");
        let _color_mode = EnvGuard::remove(COLOR_MODE_ENV);
        let _colorterm = EnvGuard::remove("COLORTERM");
        let _term = EnvGuard::remove("TERM");

        let opencode = theme_for(&AppType::OpenCode);
        let codex = theme_for(&AppType::Codex);

        assert_ne!(opencode.accent, codex.accent);
    }

    #[test]
    fn theme_keeps_rgb_colors_when_truecolor_is_available() {
        let _lock = env_lock().lock().expect("env lock poisoned");
        let _no_color = EnvGuard::remove("NO_COLOR");
        let _color_mode = EnvGuard::remove("CC_SWITCH_COLOR_MODE");
        let _colorterm = EnvGuard::set("COLORTERM", "truecolor");
        let _term = EnvGuard::set("TERM", "xterm-256color");

        let theme = theme_for(&AppType::Claude);

        assert_eq!(detected_color_mode(), ColorMode::TrueColor);
        assert_eq!(theme.accent, Color::Rgb(139, 233, 253));
        assert_eq!(theme.surface, Color::Rgb(68, 71, 90));
        assert!(!theme.no_color);
    }

    #[test]
    fn theme_defaults_to_rgb_when_terminal_capability_is_unknown() {
        let _lock = env_lock().lock().expect("env lock poisoned");
        let _no_color = EnvGuard::remove("NO_COLOR");
        let _color_mode = EnvGuard::remove("CC_SWITCH_COLOR_MODE");
        let _colorterm = EnvGuard::remove("COLORTERM");
        let _term = EnvGuard::remove("TERM");
        let _term_program = EnvGuard::remove("TERM_PROGRAM");

        let theme = theme_for(&AppType::OpenCode);

        assert_eq!(detected_color_mode(), ColorMode::TrueColor);
        assert_eq!(theme.accent, Color::Rgb(255, 184, 108));
        assert_eq!(theme.surface, Color::Rgb(68, 71, 90));
        assert!(!theme.no_color);
    }

    #[test]
    fn theme_uses_ansi256_when_term_advertises_xterm_256color() {
        let _lock = env_lock().lock().expect("env lock poisoned");
        let _no_color = EnvGuard::remove("NO_COLOR");
        let _color_mode = EnvGuard::remove("CC_SWITCH_COLOR_MODE");
        let _colorterm = EnvGuard::remove("COLORTERM");
        let _term = EnvGuard::set("TERM", "xterm-256color");
        let _term_program = EnvGuard::remove("TERM_PROGRAM");

        let theme = theme_for(&AppType::Claude);

        assert_eq!(detected_color_mode(), ColorMode::Ansi256);
        assert_eq!(theme.accent, Color::Indexed(rgb_to_ansi256(139, 233, 253)));
        assert_eq!(theme.surface, Color::Indexed(rgb_to_ansi256(68, 71, 90)));
        assert!(!theme.no_color);
    }

    #[test]
    fn theme_uses_ansi256_when_term_advertises_tmux_256color() {
        let _lock = env_lock().lock().expect("env lock poisoned");
        let _no_color = EnvGuard::remove("NO_COLOR");
        let _color_mode = EnvGuard::remove("CC_SWITCH_COLOR_MODE");
        let _colorterm = EnvGuard::remove("COLORTERM");
        let _term = EnvGuard::set("TERM", "tmux-256color");
        let _term_program = EnvGuard::remove("TERM_PROGRAM");

        let theme = theme_for(&AppType::Claude);

        assert_eq!(detected_color_mode(), ColorMode::Ansi256);
        assert_eq!(theme.accent, Color::Indexed(rgb_to_ansi256(139, 233, 253)));
        assert_eq!(theme.surface, Color::Indexed(rgb_to_ansi256(68, 71, 90)));
        assert!(!theme.no_color);
    }

    #[test]
    fn theme_uses_ansi256_for_plain_xterm_over_ssh_without_truecolor_signal() {
        let _lock = env_lock().lock().expect("env lock poisoned");
        let _no_color = EnvGuard::remove("NO_COLOR");
        let _color_mode = EnvGuard::remove("CC_SWITCH_COLOR_MODE");
        let _colorterm = EnvGuard::remove("COLORTERM");
        let _term = EnvGuard::set("TERM", "xterm");
        let _term_program = EnvGuard::remove("TERM_PROGRAM");
        let _ssh_tty = EnvGuard::set("SSH_TTY", "/dev/pts/0");

        let theme = theme_for(&AppType::Claude);

        assert_eq!(detected_color_mode(), ColorMode::Ansi256);
        assert_eq!(theme.accent, Color::Indexed(rgb_to_ansi256(139, 233, 253)));
        assert_eq!(theme.surface, Color::Indexed(rgb_to_ansi256(68, 71, 90)));
        assert!(!theme.no_color);
    }

    #[test]
    fn theme_keeps_truecolor_for_plain_xterm_without_ssh_signal() {
        let _lock = env_lock().lock().expect("env lock poisoned");
        let _no_color = EnvGuard::remove("NO_COLOR");
        let _color_mode = EnvGuard::remove("CC_SWITCH_COLOR_MODE");
        let _colorterm = EnvGuard::remove("COLORTERM");
        let _term = EnvGuard::set("TERM", "xterm");
        let _term_program = EnvGuard::remove("TERM_PROGRAM");
        let _ssh_tty = EnvGuard::remove("SSH_TTY");

        let theme = theme_for(&AppType::Claude);

        assert_eq!(detected_color_mode(), ColorMode::TrueColor);
        assert_eq!(theme.accent, Color::Rgb(139, 233, 253));
        assert_eq!(theme.surface, Color::Rgb(68, 71, 90));
        assert!(!theme.no_color);
    }

    #[test]
    fn theme_keeps_truecolor_for_plain_xterm_over_ssh_with_explicit_truecolor_signal() {
        let _lock = env_lock().lock().expect("env lock poisoned");
        let _no_color = EnvGuard::remove("NO_COLOR");
        let _color_mode = EnvGuard::remove("CC_SWITCH_COLOR_MODE");
        let _colorterm = EnvGuard::set("COLORTERM", "truecolor");
        let _term = EnvGuard::set("TERM", "xterm");
        let _term_program = EnvGuard::remove("TERM_PROGRAM");
        let _ssh_tty = EnvGuard::set("SSH_TTY", "/dev/pts/0");

        let theme = theme_for(&AppType::Claude);

        assert_eq!(detected_color_mode(), ColorMode::TrueColor);
        assert_eq!(theme.accent, Color::Rgb(139, 233, 253));
        assert_eq!(theme.surface, Color::Rgb(68, 71, 90));
        assert!(!theme.no_color);
    }

    #[test]
    fn theme_keeps_truecolor_for_term_direct_over_ssh() {
        let _lock = env_lock().lock().expect("env lock poisoned");
        let _no_color = EnvGuard::remove("NO_COLOR");
        let _color_mode = EnvGuard::remove("CC_SWITCH_COLOR_MODE");
        let _colorterm = EnvGuard::remove("COLORTERM");
        let _term = EnvGuard::set("TERM", "xterm-direct");
        let _term_program = EnvGuard::remove("TERM_PROGRAM");
        let _ssh_tty = EnvGuard::set("SSH_TTY", "/dev/pts/0");

        let theme = theme_for(&AppType::Claude);

        assert_eq!(detected_color_mode(), ColorMode::TrueColor);
        assert_eq!(theme.accent, Color::Rgb(139, 233, 253));
        assert_eq!(theme.surface, Color::Rgb(68, 71, 90));
        assert!(!theme.no_color);
    }

    #[test]
    fn theme_uses_ansi256_when_explicitly_requested() {
        let _lock = env_lock().lock().expect("env lock poisoned");
        let _no_color = EnvGuard::remove("NO_COLOR");
        let _color_mode = EnvGuard::set("CC_SWITCH_COLOR_MODE", "ansi256");
        let _colorterm = EnvGuard::remove("COLORTERM");
        let _term = EnvGuard::remove("TERM");
        let _term_program = EnvGuard::remove("TERM_PROGRAM");

        let theme = theme_for(&AppType::OpenCode);

        assert_eq!(detected_color_mode(), ColorMode::Ansi256);
        assert_eq!(theme.accent, Color::Indexed(215));
        assert_eq!(theme.surface, Color::Indexed(239));
        assert!(!theme.no_color);
    }

    #[test]
    fn no_color_has_priority_over_explicit_color_mode() {
        let _lock = env_lock().lock().expect("env lock poisoned");
        let _no_color = EnvGuard::set("NO_COLOR", "1");
        let _color_mode = EnvGuard::set("CC_SWITCH_COLOR_MODE", "truecolor");
        let _colorterm = EnvGuard::set("COLORTERM", "truecolor");
        let _term = EnvGuard::set("TERM", "xterm-256color");
        let _term_program = EnvGuard::remove("TERM_PROGRAM");

        let theme = theme_for(&AppType::Gemini);

        assert_eq!(detected_color_mode(), ColorMode::NoColor);
        assert_eq!(theme.accent, Color::Reset);
        assert_eq!(theme.surface, Color::Reset);
        assert!(theme.no_color);
    }

    #[test]
    fn theme_uses_ansi256_in_apple_terminal_without_truecolor_signal() {
        let _lock = env_lock().lock().expect("env lock poisoned");
        let _no_color = EnvGuard::remove("NO_COLOR");
        let _color_mode = EnvGuard::remove("CC_SWITCH_COLOR_MODE");
        let _colorterm = EnvGuard::remove("COLORTERM");
        let _term = EnvGuard::set("TERM", "xterm-256color");
        let _term_program = EnvGuard::set("TERM_PROGRAM", "Apple_Terminal");

        let theme = theme_for(&AppType::Claude);

        assert_eq!(detected_color_mode(), ColorMode::Ansi256);
        assert_eq!(theme.accent, Color::Indexed(rgb_to_ansi256(139, 233, 253)));
        assert_eq!(theme.surface, Color::Indexed(rgb_to_ansi256(68, 71, 90)));
        assert!(!theme.no_color);
    }

    #[test]
    fn explicit_truecolor_override_beats_apple_terminal_auto_fallback() {
        let _lock = env_lock().lock().expect("env lock poisoned");
        let _no_color = EnvGuard::remove("NO_COLOR");
        let _color_mode = EnvGuard::set("CC_SWITCH_COLOR_MODE", "truecolor");
        let _colorterm = EnvGuard::remove("COLORTERM");
        let _term = EnvGuard::set("TERM", "xterm-256color");
        let _term_program = EnvGuard::set("TERM_PROGRAM", "Apple_Terminal");

        let theme = theme_for(&AppType::Claude);

        assert_eq!(detected_color_mode(), ColorMode::TrueColor);
        assert_eq!(theme.accent, Color::Rgb(139, 233, 253));
        assert_eq!(theme.surface, Color::Rgb(68, 71, 90));
        assert!(!theme.no_color);
    }

    #[test]
    fn ansi256_mapping_keeps_curated_indices_for_fixed_v5_palette() {
        assert_eq!(rgb_to_ansi256(80, 250, 123), 84);
        assert_eq!(rgb_to_ansi256(139, 233, 253), 117);
        assert_eq!(rgb_to_ansi256(255, 121, 198), 212);
        assert_eq!(rgb_to_ansi256(255, 184, 108), 215);
        assert_eq!(rgb_to_ansi256(241, 250, 140), 228);
        assert_eq!(rgb_to_ansi256(255, 85, 85), 203);
        assert_eq!(rgb_to_ansi256(98, 114, 164), 61);
        assert_eq!(rgb_to_ansi256(68, 71, 90), 239);
        assert_eq!(rgb_to_ansi256(101, 113, 160), 61);
        assert_eq!(rgb_to_ansi256(248, 248, 248), 231);
        assert_eq!(rgb_to_ansi256(108, 108, 108), 242);
        assert_eq!(rgb_to_ansi256(255, 255, 255), 231);
    }
}
