//! Shared overlay chrome.
//!
//! Every dialog used to hand-roll the same six steps (compute rect, clear,
//! bordered block, split off a key-bar row, inset the body) with slightly
//! different spacing each time. `overlay_frame` renders that chrome once
//! and hands back the body rect with the unified padding, so dialogs only
//! describe their size, title, keys, and body.

use super::super::theme;
use super::super::*;

/// Rows consumed by the frame: top/bottom borders, the key bar, and the
/// blank row between the key bar and the body.
pub(crate) const OVERLAY_CHROME_ROWS: u16 = 4;
/// Columns consumed by the frame: left/right borders plus one column of
/// body padding on each side.
pub(crate) const OVERLAY_CHROME_COLS: u16 = 4;

/// How an overlay determines its rectangle within the content pane.
pub(super) enum OverlaySize {
    /// Percentage of the content pane; for large scrollable overlays.
    Percent(u16, u16),
    /// Fixed width and height, clamped to the pane.
    Fixed(u16, u16),
    /// Fixed width, height fitted to `body_rows` of content plus chrome —
    /// pickers size themselves to their options instead of leaving a
    /// half-empty box.
    FitRows { width: u16, body_rows: u16 },
}

impl OverlaySize {
    fn resolve(&self, content_area: Rect) -> Rect {
        match *self {
            OverlaySize::Percent(x, y) => centered_rect(x, y, content_area),
            OverlaySize::Fixed(width, height) => centered_rect_fixed(width, height, content_area),
            OverlaySize::FitRows { width, body_rows } => centered_rect_fixed(
                width,
                body_rows
                    .saturating_add(OVERLAY_CHROME_ROWS)
                    .min(content_area.height),
                content_area,
            ),
        }
    }
}

/// Render the shared chrome (clear, bordered block with a padded title,
/// centered key bar) and return the body rect: one blank row under the key
/// bar, one column of padding on each side.
pub(super) fn overlay_frame(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    title: &str,
    keys: &[(&str, &str)],
    size: OverlaySize,
    border: Style,
) -> Rect {
    overlay_frame_at(
        frame,
        size.resolve(content_area),
        theme,
        title,
        keys,
        border,
    )
}

/// Same as [`overlay_frame`], for dialogs whose rect is computed elsewhere
/// (e.g. the message-adaptive confirm sizing in `layout.rs`).
pub(super) fn overlay_frame_at(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &theme::Theme,
    title: &str,
    keys: &[(&str, &str)],
    border: Style,
) -> Rect {
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(border)
        .title(format!(" {} ", icons::strip_icon(title)));
    frame.render_widget(block.clone(), area);
    let inner = block.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);
    render_key_bar_center(frame, chunks[0], theme, keys);

    inset_horizontal(inset_top(chunks[1], 1), 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_rows_adds_exactly_the_chrome() {
        let content = Rect::new(0, 0, 100, 30);
        let area = OverlaySize::FitRows {
            width: 50,
            body_rows: 6,
        }
        .resolve(content);
        assert_eq!(area.width, 50);
        assert_eq!(area.height, 6 + OVERLAY_CHROME_ROWS);
    }

    #[test]
    fn fit_rows_clamps_to_the_content_pane() {
        let content = Rect::new(0, 0, 40, 8);
        let area = OverlaySize::FitRows {
            width: 50,
            body_rows: 20,
        }
        .resolve(content);
        assert!(area.width <= content.width);
        assert!(area.height <= content.height);
    }
}
