//! Terminal-icon fallback, mirroring the color-mode philosophy in `theme.rs`.
//!
//! Decorative emoji glyphs (🏠 🔑 …) render double-width on some SSH and
//! legacy terminals and break border alignment. `CC_SWITCH_ICONS` and the
//! Settings › Icons row select the mode; `Auto` keeps emoji unless the locale
//! is clearly not UTF-8 (mirroring how COLORFGBG drives the theme). As with
//! color mode, `Auto` never flips the default blindly — it only downgrades for
//! a locale that cannot render wide glyphs; absent locale info stays emoji.

const ICON_MODE_ENV: &str = "CC_SWITCH_ICONS";

/// User-selectable icon rendering. `Auto` keeps emoji unless the locale is
/// not UTF-8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IconMode {
    #[default]
    Auto,
    Emoji,
    Ascii,
}

impl IconMode {
    pub fn code(&self) -> &'static str {
        match self {
            IconMode::Auto => "auto",
            IconMode::Emoji => "emoji",
            IconMode::Ascii => "ascii",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(IconMode::Auto),
            "emoji" => Some(IconMode::Emoji),
            "ascii" | "text" => Some(IconMode::Ascii),
            _ => None,
        }
    }

    pub fn next(&self) -> Self {
        match self {
            IconMode::Auto => IconMode::Emoji,
            IconMode::Emoji => IconMode::Ascii,
            IconMode::Ascii => IconMode::Auto,
        }
    }
}

fn icon_mode_override() -> Option<IconMode> {
    IconMode::parse(&std::env::var(ICON_MODE_ENV).ok()?)
}

/// The configured icon mode: the `CC_SWITCH_ICONS` override wins, then the
/// persisted Settings value, else `Auto`.
pub fn configured_icon_mode() -> IconMode {
    if let Some(mode) = icon_mode_override() {
        return mode;
    }
    crate::settings::get_icon_mode()
        .as_deref()
        .and_then(IconMode::parse)
        .unwrap_or_default()
}

/// Whether emoji glyphs should be emitted. `Auto` keeps them only when the
/// locale advertises UTF-8, mirroring the color-mode auto-detection.
pub fn use_emoji() -> bool {
    match configured_icon_mode() {
        IconMode::Emoji => true,
        IconMode::Ascii => false,
        IconMode::Auto => locale_is_utf8(),
    }
}

/// True when the first non-empty locale variable advertises UTF-8. Absent
/// locale info is treated as UTF-8 (the historical default) so `Auto` never
/// flips the default blindly — only a locale that is clearly not UTF-8
/// downgrades to ASCII.
fn locale_is_utf8() -> bool {
    for key in ["LC_ALL", "LC_CTYPE", "LANG"] {
        match std::env::var(key) {
            Ok(value) if !value.is_empty() => {
                let lower = value.to_ascii_lowercase();
                return lower.contains("utf-8") || lower.contains("utf8");
            }
            _ => continue,
        }
    }
    true
}

/// True for the decorative emoji glyphs used as label icons — the pictograph
/// planes plus the dingbats/symbols blocks and variation selectors, but NOT
/// CJK text (which terminals size consistently and must not be stripped).
pub fn is_emoji(c: char) -> bool {
    matches!(
        c as u32,
        0x2600..=0x27BF        // Misc symbols + Dingbats (⚙ ⚡ …)
        | 0xFE00..=0xFE0F      // Variation selectors
        | 0x1F000..=0x1FAFF    // Emoji / pictograph planes
    )
}

/// Strip a leading emoji marker (`"🔑 Providers"` → `"Providers"`) when icons
/// are disabled. Labels without an emoji prefix are returned unchanged, so
/// this is safe to apply to any title or menu label.
pub fn strip_icon(label: &str) -> &str {
    if use_emoji() {
        return label;
    }
    strip_leading_emoji(label)
}

/// Icon-mode-agnostic version of [`strip_icon`]: removes a leading emoji and
/// its trailing space, tolerating any leading indent spaces (e.g. the centred
/// home title `"    🎯 CC-Switch …"`). Kept separate so callers that already
/// resolved the mode (and the tests) don't re-read the environment.
pub fn strip_leading_emoji(label: &str) -> &str {
    let trimmed = label.trim_start_matches(' ');
    match trimmed.chars().next() {
        Some(c) if is_emoji(c) => trimmed
            .split_once(' ')
            .map(|(_, rest)| rest)
            .unwrap_or(label),
        _ => label,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::lock_test_home_and_settings;
    use std::ffi::OsString;

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
            match self.previous.take() {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    #[test]
    fn icon_mode_codes_round_trip() {
        for mode in [IconMode::Auto, IconMode::Emoji, IconMode::Ascii] {
            assert_eq!(IconMode::parse(mode.code()), Some(mode));
        }
        assert_eq!(IconMode::parse(" Ascii "), Some(IconMode::Ascii));
        assert_eq!(IconMode::parse("text"), Some(IconMode::Ascii));
        assert_eq!(IconMode::parse("nerdfont"), None);
    }

    #[test]
    fn icon_mode_cycles_through_all_variants() {
        let start = IconMode::Auto;
        assert_eq!(start.next(), IconMode::Emoji);
        assert_eq!(start.next().next(), IconMode::Ascii);
        assert_eq!(start.next().next().next(), IconMode::Auto);
    }

    #[test]
    fn env_override_forces_mode_regardless_of_locale() {
        let _lock = lock_test_home_and_settings();
        let _lang = EnvGuard::set("LANG", "en_US.UTF-8");
        let _lc_all = EnvGuard::remove("LC_ALL");
        let _lc_ctype = EnvGuard::remove("LC_CTYPE");

        let _icons = EnvGuard::set(ICON_MODE_ENV, "ascii");
        assert!(!use_emoji(), "explicit ascii override should disable emoji");

        let _icons = EnvGuard::set(ICON_MODE_ENV, "emoji");
        assert!(use_emoji(), "explicit emoji override should force emoji");
    }

    #[test]
    fn auto_downgrades_only_for_non_utf8_locales() {
        let _lock = lock_test_home_and_settings();
        let _icons = EnvGuard::set(ICON_MODE_ENV, "auto");
        let _lc_all = EnvGuard::remove("LC_ALL");
        let _lc_ctype = EnvGuard::remove("LC_CTYPE");

        let _lang = EnvGuard::set("LANG", "en_US.UTF-8");
        assert!(use_emoji(), "utf-8 locale keeps emoji under auto");

        let _lang = EnvGuard::set("LANG", "C");
        assert!(!use_emoji(), "non-utf-8 locale downgrades under auto");

        let _lang = EnvGuard::set("LANG", "POSIX");
        assert!(!use_emoji(), "POSIX locale downgrades under auto");
    }

    #[test]
    fn auto_keeps_emoji_when_locale_is_unset() {
        let _lock = lock_test_home_and_settings();
        let _icons = EnvGuard::set(ICON_MODE_ENV, "auto");
        let _lang = EnvGuard::remove("LANG");
        let _lc_all = EnvGuard::remove("LC_ALL");
        let _lc_ctype = EnvGuard::remove("LC_CTYPE");
        // Absent locale info must not flip the default.
        assert!(use_emoji());
    }

    #[test]
    fn strip_leading_emoji_handles_menu_labels() {
        // The real menu labels: emoji + space + text (some with a variation
        // selector). Only the leading emoji is removed; CJK text stays.
        assert_eq!(strip_leading_emoji("🏠 Home"), "Home");
        assert_eq!(strip_leading_emoji("🔑 供应商"), "供应商");
        assert_eq!(
            strip_leading_emoji("🛠️ MCP Server Management"),
            "MCP Server Management"
        );
        // Leading indent spaces before the emoji (the centred home title).
        assert_eq!(
            strip_leading_emoji("    🎯 CC-Switch 交互模式"),
            "CC-Switch 交互模式"
        );
        // No emoji prefix → unchanged, even with a leading indent or CJK text.
        assert_eq!(strip_leading_emoji("供应商管理"), "供应商管理");
        assert_eq!(strip_leading_emoji("Custom Provider"), "Custom Provider");
        assert_eq!(strip_leading_emoji("  Plain Title"), "  Plain Title");
    }

    #[test]
    fn cjk_is_not_treated_as_an_emoji() {
        assert!(!is_emoji('供'));
        assert!(!is_emoji('设'));
        assert!(!is_emoji('A'));
        assert!(is_emoji('🔑'));
        assert!(is_emoji('⚙'));
    }
}
