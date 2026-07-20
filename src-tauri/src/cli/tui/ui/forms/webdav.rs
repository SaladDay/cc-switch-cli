use super::*;

pub(crate) fn render_webdav_sync_form(
    frame: &mut Frame<'_>,
    app: &App,
    form: &form::WebDavSyncFormState,
    area: Rect,
    theme: &theme::Theme,
) {
    let title = breadcrumb_path(&[
        texts::tui_config_title(),
        texts::tui_config_cloud_sync_title(),
        texts::tui_config_webdav_title(),
        texts::tui_configure(),
    ]);
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(format!(" {title} "));
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);
    let keys = if form.is_editing() {
        vec![
            ("Enter", texts::tui_key_apply()),
            ("Ctrl+S", texts::tui_key_save()),
            ("Esc", texts::tui_key_cancel()),
        ]
    } else {
        vec![
            ("↑↓", texts::tui_key_select()),
            ("Enter", texts::tui_key_edit_mode()),
            ("Ctrl+S", texts::tui_key_save()),
            ("?", texts::tui_key_help()),
            ("Esc", texts::tui_key_close()),
        ]
    };
    render_key_bar(frame, chunks[0], theme, &keys);
    render_webdav_fields(frame, form, chunks[1], theme);
}

fn render_webdav_fields(
    frame: &mut Frame<'_>,
    form: &form::WebDavSyncFormState,
    area: Rect,
    theme: &theme::Theme,
) {
    let fields = form.fields();
    let selected_idx = form
        .text_edit_target()
        .and_then(|field| fields.iter().position(|candidate| *candidate == field))
        .unwrap_or(form.field_idx.min(fields.len().saturating_sub(1)));
    let rows_data = fields
        .iter()
        .map(|field| webdav_field_label_and_value(form, *field))
        .collect::<Vec<_>>();
    let raw_label_width = field_label_column_width(
        rows_data
            .iter()
            .map(|row| row.0.as_str())
            .chain(std::iter::once(texts::tui_header_field())),
        1,
    );
    let label_col_width = raw_label_width.min(
        area.width
            .saturating_sub(FORM_VALUE_MIN_WIDTH)
            .saturating_sub(1),
    );
    let value_width = form_value_width(area.width, label_col_width, theme);
    let mut cursor_x = None;
    let mut row_heights = Vec::with_capacity(fields.len());
    let rows = fields
        .iter()
        .zip(rows_data.iter())
        .enumerate()
        .map(|(idx, (field, (label, value)))| {
            let editing = form.text_edit_target() == Some(*field);
            let display = if editing {
                let (visible, x) = inline_input_window(form.input(*field), value_width);
                if idx == selected_idx {
                    cursor_x = Some(x);
                }
                visible
            } else {
                truncated_value_cell(value, area.width, label_col_width, theme)
            };
            let error = form.field_error(*field);
            let height = inline_row_height(error, value_width);
            row_heights.push(height);
            Row::new(vec![
                Cell::from(cell_pad(label)),
                inline_field_cell(display, error, value_width, theme),
            ])
            .height(height)
        })
        .collect::<Vec<_>>();
    let table = Table::new(
        rows,
        [Constraint::Length(label_col_width), Constraint::Min(1)],
    )
    .header(
        Row::new(vec![
            Cell::from(cell_pad(texts::tui_header_field())),
            Cell::from(texts::tui_header_value()),
        ])
        .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD)),
    )
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));
    let mut state = TableState::default();
    state.select(Some(selected_idx));
    frame.render_stateful_widget(table, area, &mut state);
    if let Some(cursor_x) = cursor_x {
        set_inline_table_cursor(
            frame,
            area,
            label_col_width,
            selected_idx,
            state.offset(),
            &row_heights,
            cursor_x,
            theme,
        );
    }
}

pub(crate) fn webdav_field_label_and_value(
    form: &form::WebDavSyncFormState,
    field: form::WebDavSyncField,
) -> (String, String) {
    let label = match field {
        form::WebDavSyncField::BaseUrl => texts::tui_webdav_base_url(),
        form::WebDavSyncField::Username => texts::tui_webdav_username(),
        form::WebDavSyncField::Password => texts::tui_webdav_password(),
        form::WebDavSyncField::RemoteRoot => texts::tui_s3_remote_root(),
        form::WebDavSyncField::Profile => texts::tui_s3_profile(),
    };
    let value = bounded_trimmed_text_for_display(&form.input(field).value);
    (
        label.to_string(),
        if value.is_empty() {
            texts::tui_na().to_string()
        } else {
            value
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_is_displayed_as_plain_text() {
        let settings = crate::settings::WebDavSyncSettings {
            password: "plain-password".to_string(),
            ..crate::settings::WebDavSyncSettings::default()
        };
        let form = form::WebDavSyncFormState::from_settings(Some(&settings));
        let (_, value) = webdav_field_label_and_value(&form, form::WebDavSyncField::Password);
        assert_eq!(value, "plain-password");
    }
}
