use super::super::types::{McpEnvEditorField, McpEnvEntryEditorState};
use super::*;
use crate::cli::tui::form::TextInput;

impl App {
    pub(super) fn handle_mcp_env_overlay_key(&mut self, key: KeyEvent) -> Option<Action> {
        if let Some(action) = self.handle_mcp_env_picker_key(key) {
            return Some(action);
        }
        if let Some(action) = self.handle_mcp_env_entry_editor_key(key) {
            return Some(action);
        }
        None
    }

    fn handle_mcp_env_picker_key(&mut self, key: KeyEvent) -> Option<Action> {
        let Overlay::McpEnvPicker { selected } = &mut self.overlay else {
            return None;
        };
        let Some(FormState::McpAdd(mcp)) = self.form.as_mut() else {
            self.overlay = Overlay::None;
            return Some(Action::None);
        };

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
                if !mcp.env_rows.is_empty() {
                    *selected = selected.saturating_add(1).min(mcp.env_rows.len() - 1);
                }
                Action::None
            }
            KeyCode::Char('a') => {
                self.overlay = Overlay::McpEnvEntryEditor(McpEnvEntryEditorState {
                    row: None,
                    return_selected: *selected,
                    field: McpEnvEditorField::Key,
                    key: TextInput::new(""),
                    value: TextInput::new(""),
                });
                Action::None
            }
            KeyCode::Enter => {
                let Some(row) = mcp.env_rows.get(*selected).cloned() else {
                    return Some(Action::None);
                };
                self.overlay = Overlay::McpEnvEntryEditor(McpEnvEntryEditorState {
                    row: Some(*selected),
                    return_selected: *selected,
                    field: McpEnvEditorField::Key,
                    key: TextInput::new(row.key),
                    value: TextInput::new(row.value),
                });
                Action::None
            }
            KeyCode::Delete => {
                mcp.remove_env_row(*selected);
                *selected = (*selected).min(mcp.env_rows.len().saturating_sub(1));
                Action::None
            }
            _ => Action::None,
        })
    }

    fn handle_mcp_env_entry_editor_key(&mut self, key: KeyEvent) -> Option<Action> {
        if !matches!(self.overlay, Overlay::McpEnvEntryEditor(_)) {
            return None;
        }

        match key.code {
            KeyCode::Esc => {
                let Some(FormState::McpAdd(mcp)) = self.form.as_ref() else {
                    self.overlay = Overlay::None;
                    return Some(Action::None);
                };

                let selected = match &self.overlay {
                    Overlay::McpEnvEntryEditor(editor) => editor
                        .return_selected
                        .min(mcp.env_rows.len().saturating_sub(1)),
                    _ => 0,
                };
                self.overlay = Overlay::McpEnvPicker { selected };
                Some(Action::None)
            }
            KeyCode::Tab => {
                if let Overlay::McpEnvEntryEditor(editor) = &mut self.overlay {
                    editor.field = match editor.field {
                        McpEnvEditorField::Key => McpEnvEditorField::Value,
                        McpEnvEditorField::Value => McpEnvEditorField::Key,
                    };
                }
                Some(Action::None)
            }
            KeyCode::Enter => {
                let (row, key_text, value) = match &self.overlay {
                    Overlay::McpEnvEntryEditor(editor) => (
                        editor.row,
                        editor.key.value.trim().to_string(),
                        editor.value.value.clone(),
                    ),
                    _ => return Some(Action::None),
                };

                if key_text.is_empty() {
                    self.push_toast(texts::tui_toast_mcp_env_key_empty(), ToastKind::Warning);
                    return Some(Action::None);
                }

                let duplicate = match self.form.as_ref() {
                    Some(FormState::McpAdd(mcp)) => {
                        mcp.env_rows.iter().enumerate().any(|(idx, existing)| {
                            Some(idx) != row && existing.key.trim() == key_text
                        })
                    }
                    _ => false,
                };
                if duplicate {
                    self.push_toast(
                        texts::tui_toast_mcp_env_duplicate_key(&key_text),
                        ToastKind::Warning,
                    );
                    return Some(Action::None);
                }

                let Some(FormState::McpAdd(mcp)) = self.form.as_mut() else {
                    self.overlay = Overlay::None;
                    return Some(Action::None);
                };

                mcp.upsert_env_row(row, key_text.clone(), value);
                let selected = mcp
                    .env_rows
                    .iter()
                    .position(|entry| entry.key == key_text)
                    .unwrap_or_else(|| mcp.env_rows.len().saturating_sub(1));
                self.overlay = Overlay::McpEnvPicker { selected };
                Some(Action::None)
            }
            _ => {
                if let Overlay::McpEnvEntryEditor(editor) = &mut self.overlay {
                    let input = match editor.field {
                        McpEnvEditorField::Key => &mut editor.key,
                        McpEnvEditorField::Value => &mut editor.value,
                    };
                    match key.code {
                        KeyCode::Left => input.move_left(),
                        KeyCode::Right => input.move_right(),
                        KeyCode::Home => input.move_home(),
                        KeyCode::End => input.move_end(),
                        KeyCode::Backspace => {
                            input.backspace();
                        }
                        KeyCode::Delete => {
                            input.delete();
                        }
                        KeyCode::Char(c) if !c.is_control() => {
                            input.insert_char(c);
                        }
                        _ => {}
                    }
                }
                Some(Action::None)
            }
        }
    }
}
