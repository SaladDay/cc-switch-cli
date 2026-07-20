use super::*;

pub(crate) fn render_s3_sync_form(
    frame: &mut Frame<'_>,
    app: &App,
    form: &form::S3SyncFormState,
    area: Rect,
    theme: &theme::Theme,
) {
    let title = breadcrumb_path(&[
        texts::tui_config_title(),
        texts::tui_config_cloud_sync_title(),
        texts::tui_config_s3_title(),
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

    let editing = form.text_edit_target().is_some();
    let enter_label = if form.selected_field() == form::S3SyncField::Preset {
        texts::tui_key_select()
    } else {
        texts::tui_key_edit_mode()
    };
    let keys = if editing {
        vec![
            ("Enter", texts::tui_key_apply()),
            ("Ctrl+S", texts::tui_key_save()),
            ("Esc", texts::tui_key_cancel()),
        ]
    } else {
        vec![
            ("↑↓", texts::tui_key_select()),
            ("Enter", enter_label),
            ("Ctrl+S", texts::tui_key_save()),
            ("?", texts::tui_key_help()),
            ("Esc", texts::tui_key_close()),
        ]
    };
    render_key_bar(frame, chunks[0], theme, &keys);
    render_s3_fields(frame, form, chunks[1], theme);
}

fn render_s3_fields(
    frame: &mut Frame<'_>,
    form: &form::S3SyncFormState,
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
        .map(|field| s3_field_label_and_value(form, *field))
        .collect::<Vec<_>>();
    let table_area = area;
    let raw_label_width = field_label_column_width(
        rows_data
            .iter()
            .map(|row| row.0.as_str())
            .chain(std::iter::once(texts::tui_header_field())),
        1,
    );
    let label_col_width = raw_label_width.min(
        table_area
            .width
            .saturating_sub(FORM_VALUE_MIN_WIDTH)
            .saturating_sub(1),
    );
    let value_width = form_value_width(table_area.width, label_col_width, theme);
    let mut cursor_x = None;
    let mut row_heights = Vec::with_capacity(fields.len());
    let rows = fields
        .iter()
        .zip(rows_data.iter())
        .enumerate()
        .map(|(idx, (field, (label, value)))| {
            let editing = form.text_edit_target() == Some(*field);
            let display = if editing {
                form.input(*field).map_or_else(
                    || truncated_value_cell(value, table_area.width, label_col_width, theme),
                    |input| {
                        let (visible, x) = inline_input_window(input, value_width);
                        if idx == selected_idx {
                            cursor_x = Some(x);
                        }
                        visible
                    },
                )
            } else {
                truncated_value_cell(value, table_area.width, label_col_width, theme)
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
    let header = Row::new(vec![
        Cell::from(cell_pad(texts::tui_header_field())),
        Cell::from(texts::tui_header_value()),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));
    let table = Table::new(
        rows,
        [Constraint::Length(label_col_width), Constraint::Min(1)],
    )
    .header(header)
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));
    let mut state = TableState::default();
    state.select(Some(selected_idx));
    frame.render_stateful_widget(table, table_area, &mut state);
    if let Some(cursor_x) = cursor_x {
        set_inline_table_cursor(
            frame,
            table_area,
            label_col_width,
            selected_idx,
            state.offset(),
            &row_heights,
            cursor_x,
            theme,
        );
    }
}

pub(crate) fn s3_field_label_and_value(
    form: &form::S3SyncFormState,
    field: form::S3SyncField,
) -> (String, String) {
    let label = match field {
        form::S3SyncField::Preset => texts::tui_s3_service_preset(),
        form::S3SyncField::Region => texts::tui_s3_region(),
        form::S3SyncField::Bucket => texts::tui_s3_bucket(),
        form::S3SyncField::AccessKeyId => texts::tui_s3_access_key_id(),
        form::S3SyncField::SecretAccessKey => texts::tui_s3_secret_access_key(),
        form::S3SyncField::Endpoint => texts::tui_s3_endpoint(),
        form::S3SyncField::RemoteRoot => texts::tui_s3_remote_root(),
        form::S3SyncField::Profile => texts::tui_s3_profile(),
    };
    let value = if field == form::S3SyncField::Preset {
        form.preset.label().to_string()
    } else {
        form.input(field)
            .map(|input| bounded_trimmed_text_for_display(&input.value))
            .unwrap_or_default()
    };
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
    fn secret_value_is_not_redacted() {
        let settings = crate::settings::S3SyncSettings {
            secret_access_key: "plain-secret".to_string(),
            ..crate::settings::S3SyncSettings::default()
        };
        let form = form::S3SyncFormState::from_settings(Some(&settings));
        let (_, value) = s3_field_label_and_value(&form, form::S3SyncField::SecretAccessKey);
        assert_eq!(value, "plain-secret");
    }
}
