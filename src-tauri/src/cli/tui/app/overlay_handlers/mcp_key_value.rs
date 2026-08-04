use super::super::types::{McpKeyValueEditorField, McpKeyValueEntryEditorState};
use super::*;
use crate::cli::tui::form::{
    is_valid_http_header_name, is_valid_http_header_value, McpKeyValueKind, TextInput,
};

impl App {
    pub(super) fn handle_mcp_key_value_overlay_key(&mut self, key: KeyEvent) -> Option<Action> {
        if let Some(action) = self.handle_mcp_key_value_picker_key(key) {
            return Some(action);
        }
        if let Some(action) = self.handle_mcp_key_value_entry_editor_key(key) {
            return Some(action);
        }
        None
    }

    fn handle_mcp_key_value_picker_key(&mut self, key: KeyEvent) -> Option<Action> {
        let Overlay::McpKeyValuePicker { kind, selected } = &mut self.overlay else {
            return None;
        };
        let Some(FormState::McpAdd(mcp)) = self.form.as_mut() else {
            self.overlay = Overlay::None;
            return Some(Action::None);
        };
        let rows = mcp.key_value_rows(*kind);

        Some(match key.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                Action::None
            }
            KeyCode::Up => {
                *selected = selected.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                if !rows.is_empty() {
                    *selected = selected.saturating_add(1).min(rows.len() - 1);
                }
                Action::None
            }
            KeyCode::Char('a') => {
                self.overlay = Overlay::McpKeyValueEntryEditor(McpKeyValueEntryEditorState {
                    kind: *kind,
                    row: None,
                    return_selected: *selected,
                    field: McpKeyValueEditorField::Key,
                    key: TextInput::new(""),
                    value: TextInput::new(""),
                });
                Action::None
            }
            KeyCode::Enter => {
                let Some(row) = rows.get(*selected).cloned() else {
                    return Some(Action::None);
                };
                self.overlay = Overlay::McpKeyValueEntryEditor(McpKeyValueEntryEditorState {
                    kind: *kind,
                    row: Some(*selected),
                    return_selected: *selected,
                    field: McpKeyValueEditorField::Key,
                    key: TextInput::new(row.key),
                    value: TextInput::new(row.value),
                });
                Action::None
            }
            KeyCode::Backspace | KeyCode::Delete => {
                let kind = *kind;
                mcp.remove_key_value_row(kind, *selected);
                *selected = (*selected).min(mcp.key_value_rows(kind).len().saturating_sub(1));
                Action::None
            }
            _ => Action::None,
        })
    }

    fn handle_mcp_key_value_entry_editor_key(&mut self, key: KeyEvent) -> Option<Action> {
        if !matches!(self.overlay, Overlay::McpKeyValueEntryEditor(_)) {
            return None;
        }

        match key.code {
            KeyCode::Esc => {
                let Some(FormState::McpAdd(mcp)) = self.form.as_ref() else {
                    self.overlay = Overlay::None;
                    return Some(Action::None);
                };

                let (kind, selected) = match &self.overlay {
                    Overlay::McpKeyValueEntryEditor(editor) => (
                        editor.kind,
                        editor
                            .return_selected
                            .min(mcp.key_value_rows(editor.kind).len().saturating_sub(1)),
                    ),
                    _ => (McpKeyValueKind::Env, 0),
                };
                self.overlay = Overlay::McpKeyValuePicker { kind, selected };
                Some(Action::None)
            }
            KeyCode::Tab => {
                if let Overlay::McpKeyValueEntryEditor(editor) = &mut self.overlay {
                    editor.field = match editor.field {
                        McpKeyValueEditorField::Key => McpKeyValueEditorField::Value,
                        McpKeyValueEditorField::Value => McpKeyValueEditorField::Key,
                    };
                }
                Some(Action::None)
            }
            KeyCode::Enter => {
                let (kind, row, key_text, value) = match &self.overlay {
                    Overlay::McpKeyValueEntryEditor(editor) => (
                        editor.kind,
                        editor.row,
                        editor.key.value.trim().to_string(),
                        editor.value.value.clone(),
                    ),
                    _ => return Some(Action::None),
                };

                if key_text.is_empty() {
                    let message = match kind {
                        McpKeyValueKind::Env => texts::tui_toast_mcp_env_key_empty(),
                        McpKeyValueKind::Headers => texts::tui_toast_mcp_header_key_empty(),
                    };
                    self.push_toast(message, ToastKind::Warning);
                    return Some(Action::None);
                }

                if matches!(kind, McpKeyValueKind::Headers) && !is_valid_http_header_name(&key_text)
                {
                    self.push_toast(
                        texts::tui_override_header_invalid_name(&key_text),
                        ToastKind::Warning,
                    );
                    return Some(Action::None);
                }
                if matches!(kind, McpKeyValueKind::Headers) && !is_valid_http_header_value(&value) {
                    self.push_toast(
                        texts::tui_override_header_control_chars(&key_text),
                        ToastKind::Warning,
                    );
                    return Some(Action::None);
                }

                let duplicate = match self.form.as_ref() {
                    Some(FormState::McpAdd(mcp)) => mcp
                        .key_value_rows(kind)
                        .iter()
                        .enumerate()
                        .any(|(idx, existing)| {
                            Some(idx) != row
                                && match kind {
                                    McpKeyValueKind::Env => existing.key.trim() == key_text,
                                    McpKeyValueKind::Headers => {
                                        existing.key.trim().eq_ignore_ascii_case(&key_text)
                                    }
                                }
                        }),
                    _ => false,
                };
                if duplicate {
                    let message = match kind {
                        McpKeyValueKind::Env => texts::tui_toast_mcp_env_duplicate_key(&key_text),
                        McpKeyValueKind::Headers => {
                            texts::tui_toast_mcp_header_duplicate_key(&key_text)
                        }
                    };
                    self.push_toast(message, ToastKind::Warning);
                    return Some(Action::None);
                }

                let Some(FormState::McpAdd(mcp)) = self.form.as_mut() else {
                    self.overlay = Overlay::None;
                    return Some(Action::None);
                };

                mcp.upsert_key_value_row(kind, row, key_text.clone(), value);
                let selected = mcp
                    .key_value_rows(kind)
                    .iter()
                    .position(|entry| match kind {
                        McpKeyValueKind::Env => entry.key == key_text,
                        McpKeyValueKind::Headers => entry.key.eq_ignore_ascii_case(&key_text),
                    })
                    .unwrap_or_else(|| mcp.key_value_rows(kind).len().saturating_sub(1));
                self.overlay = Overlay::McpKeyValuePicker { kind, selected };
                Some(Action::None)
            }
            _ => {
                if let Overlay::McpKeyValueEntryEditor(editor) = &mut self.overlay {
                    let input = match editor.field {
                        McpKeyValueEditorField::Key => &mut editor.key,
                        McpKeyValueEditorField::Value => &mut editor.value,
                    };
                    let _ = input.apply_key(key);
                }
                Some(Action::None)
            }
        }
    }
}
