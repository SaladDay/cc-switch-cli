use ratatui::style::Color;

use crate::app_config::AppType;

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
    std::env::var("NO_COLOR").is_ok()
}

pub fn theme_for(app: &AppType) -> Theme {
    let no_color = no_color();
    let accent = if no_color {
        Color::Reset
    } else {
        match app {
            AppType::Codex => Color::Rgb(80, 250, 123),   // Dracula green
            AppType::Claude => Color::Rgb(139, 233, 253),  // Dracula cyan
            AppType::Gemini => Color::Rgb(255, 121, 198),  // Dracula pink
        }
    };

    Theme {
        accent,
        ok: if no_color {
            Color::Reset
        } else {
            Color::Rgb(80, 250, 123) // Dracula green
        },
        warn: if no_color {
            Color::Reset
        } else {
            Color::Rgb(241, 250, 140) // Dracula yellow
        },
        err: if no_color {
            Color::Reset
        } else {
            Color::Rgb(255, 85, 85) // Dracula red
        },
        dim: if no_color {
            Color::Reset
        } else {
            Color::Rgb(98, 114, 164) // Dracula comment
        },
        comment: if no_color {
            Color::Reset
        } else {
            Color::Rgb(98, 114, 164) // #6272a4
        },
        cyan: if no_color {
            Color::Reset
        } else {
            Color::Rgb(139, 233, 253) // #8be9fd
        },
        surface: if no_color {
            Color::Reset
        } else {
            Color::Rgb(68, 71, 90) // #44475a
        },
        no_color,
    }
}
