use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::Text,
    widgets::{Block, BorderType, Borders, Cell, Row, Table, TableState},
    Frame,
};

use super::{
    app::{App, Focus},
    shared::{highlight_symbol, inset_left, pane_border_style, selection_style, CONTENT_INSET_LEFT},
    theme::Theme,
};

use crate::cli::tui::data::UiData;

pub(super) fn render_settings_model_routes(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &Theme,
) {
    let header_cells = vec![
        Cell::from("Pattern"),
        Cell::from("Provider"),
        Cell::from("Priority"),
        Cell::from("Enabled"),
    ];
    let header = Row::new(header_cells)
        .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

    let rows = data.model_routes.rows.iter().map(|r| {
        Row::new(vec![
            Cell::from(r.pattern.clone()),
            Cell::from(r.provider_name.clone()),
            Cell::from(r.priority.to_string()),
            Cell::from(if r.enabled { "Yes" } else { "No" }),
        ])
    });

    let constraints = vec![
        Constraint::Percentage(30),
        Constraint::Percentage(35),
        Constraint::Length(10),
        Constraint::Length(8),
    ];

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title("Model Routes");
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    let table = Table::new(rows, constraints)
        .header(header)
        .block(Block::default().borders(Borders::NONE))
        .row_highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(app.model_routes_idx));
    frame.render_stateful_widget(table, inset_left(chunks[1], CONTENT_INSET_LEFT), &mut state);
}
