use super::super::types::{RouteMappingEditorField, RouteMappingEntryEditingState};
use super::*;
use crate::cli::tui::form::TextInput;

impl App {
    pub(super) fn handle_route_mappings_editor_key(
        &mut self,
        key: KeyEvent,
        _data: &UiData,
    ) -> Option<Action> {
        let Overlay::RouteMappingsEditor(editor) = &mut self.overlay else {
            return None;
        };

        if editor.editing.is_some() {
            return Some(self.handle_route_mapping_entry_editor_key(key));
        }

        let rows_len = editor.rows.len();
        Some(match key.code {
            KeyCode::Esc => self.apply_route_mappings_editor(),
            KeyCode::Up => {
                editor.selected = editor.selected.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                if rows_len > 0 {
                    editor.selected = (editor.selected + 1).min(rows_len - 1);
                }
                Action::None
            }
            KeyCode::Char('a') => {
                editor.editing = Some(RouteMappingEntryEditingState {
                    row: None,
                    field: RouteMappingEditorField::Request,
                    request: TextInput::new(""),
                    actual: TextInput::new(""),
                });
                Action::None
            }
            KeyCode::Enter => {
                let Some((request, actual)) = editor.rows.get(editor.selected).cloned() else {
                    return Some(Action::None);
                };
                let row = editor.selected;
                editor.editing = Some(RouteMappingEntryEditingState {
                    row: Some(row),
                    field: RouteMappingEditorField::Request,
                    request: TextInput::new(request),
                    actual: TextInput::new(actual),
                });
                Action::None
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                if rows_len > 0 {
                    editor.rows.remove(editor.selected);
                    editor.selected = editor.selected.min(editor.rows.len().saturating_sub(1));
                }
                Action::None
            }
            _ => Action::None,
        })
    }

    fn handle_route_mapping_entry_editor_key(&mut self, key: KeyEvent) -> Action {
        let Overlay::RouteMappingsEditor(editor) = &mut self.overlay else {
            return Action::None;
        };

        // Ctrl+F fetches upstream models for the actual-model field; the
        // runtime action parks this editor in `pending_overlay` while the
        // model picker runs and restores it afterwards. A modifier is
        // required because the row editor is live-typing: a plain `f` must
        // stay a letter so names like "gemini-2.0-flash" stay typeable.
        let fetch_actual = matches!(key.code, KeyCode::Char('f' | 'F'))
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && editor
                .editing
                .as_ref()
                .is_some_and(|edit| edit.actual_active());
        if fetch_actual {
            return Action::RouteMemberModelFetch {
                provider_id: editor.provider_id.clone(),
            };
        }

        // Ctrl+R opens the env-sourced request-model picker on the request
        // field. Same modifier rationale as Ctrl+F above: the row editor is
        // live-typing, so a plain `r` must stay a letter.
        let pick_request = matches!(key.code, KeyCode::Char('r' | 'R'))
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && editor
                .editing
                .as_ref()
                .is_some_and(|edit| edit.request_active());
        if pick_request {
            return Action::RouteRequestModelsFetch;
        }

        match key.code {
            KeyCode::Esc => {
                if let Some(editor) = self.route_mappings_editor_mut() {
                    editor.editing = None;
                }
                Action::None
            }
            KeyCode::Tab | KeyCode::BackTab => {
                // Two fields only, so both directions are a toggle.
                if let Some(editor) = self.route_mappings_editor_mut() {
                    if let Some(edit) = editor.editing.as_mut() {
                        edit.field = match edit.field {
                            RouteMappingEditorField::Request => RouteMappingEditorField::Actual,
                            RouteMappingEditorField::Actual => RouteMappingEditorField::Request,
                        };
                    }
                }
                Action::None
            }
            KeyCode::Enter => self.commit_route_mapping_row(),
            _ => {
                if let Some(editor) = self.route_mappings_editor_mut() {
                    if let Some(edit) = editor.editing.as_mut() {
                        let input = match edit.field {
                            RouteMappingEditorField::Request => &mut edit.request,
                            RouteMappingEditorField::Actual => &mut edit.actual,
                        };
                        let _ = input.apply_key(key);
                    }
                }
                Action::None
            }
        }
    }

    fn commit_route_mapping_row(&mut self) -> Action {
        let Overlay::RouteMappingsEditor(editor) = &mut self.overlay else {
            return Action::None;
        };
        let (row, request_text, actual_text) = match editor.editing.as_ref() {
            Some(edit) => (
                edit.row,
                edit.request.value.trim().to_string(),
                edit.actual.value.trim().to_string(),
            ),
            None => return Action::None,
        };

        // 1-based row number for validation messages; a new row appends
        // after the current last one.
        let row_display = row
            .map(|existing| existing + 1)
            .unwrap_or(editor.rows.len() + 1);

        if request_text.is_empty() {
            self.push_toast(
                texts::tui_route_mappings_request_empty(row_display),
                ToastKind::Warning,
            );
            return Action::None;
        }
        if actual_text.is_empty() {
            self.push_toast(
                texts::tui_route_mappings_actual_empty(row_display),
                ToastKind::Warning,
            );
            return Action::None;
        }
        let duplicate_row = editor
            .rows
            .iter()
            .enumerate()
            .position(|(idx, (request, _))| Some(idx) != row && request == &request_text);
        if duplicate_row.is_some() {
            // Cite the row being edited (the offending duplicate), matching
            // the apply-side validator.
            self.push_toast(
                texts::tui_route_mappings_duplicate(&request_text, row_display),
                ToastKind::Warning,
            );
            return Action::None;
        }

        let (rows, selected) = (&mut editor.rows, &mut editor.selected);
        match row {
            Some(idx) => {
                if let Some(target) = rows.get_mut(idx) {
                    *target = (request_text.clone(), actual_text);
                }
            }
            None => rows.push((request_text.clone(), actual_text)),
        }
        if let Some(position) = rows
            .iter()
            .position(|(request, _)| request == &request_text)
        {
            *selected = position;
        }
        if let Some(editor) = self.route_mappings_editor_mut() {
            editor.editing = None;
        }
        Action::None
    }

    /// Esc on the list: validates the local table and applies it as one
    /// `SetRouteModeMemberMappings`. Empty rows clear the member's mappings.
    fn apply_route_mappings_editor(&mut self) -> Action {
        let (problem, provider_id, mappings): (Option<String>, _, _) = {
            let Overlay::RouteMappingsEditor(editor) = &self.overlay else {
                return Action::None;
            };
            let mut mappings = std::collections::BTreeMap::new();
            let mut problem: Option<String> = None;
            for (idx, (request, actual)) in editor.rows.iter().enumerate() {
                let request = request.trim();
                let actual = actual.trim();
                if request.is_empty() {
                    problem = Some(texts::tui_route_mappings_request_empty(idx + 1));
                    break;
                }
                if actual.is_empty() {
                    problem = Some(texts::tui_route_mappings_actual_empty(idx + 1));
                    break;
                }
                if mappings
                    .insert(request.to_string(), actual.to_string())
                    .is_some()
                {
                    problem = Some(texts::tui_route_mappings_duplicate(request, idx + 1));
                    break;
                }
            }
            (problem, editor.provider_id.clone(), mappings)
        };
        if let Some(message) = problem {
            self.push_toast(message, ToastKind::Warning);
            return Action::None;
        }
        self.overlay = Overlay::None;
        Action::SetRouteModeMemberMappings {
            app_type: self.app_type.clone(),
            provider_id,
            mappings,
        }
    }

    fn route_mappings_editor_mut(&mut self) -> Option<&mut RouteMappingsEditorState> {
        match &mut self.overlay {
            Overlay::RouteMappingsEditor(editor) => Some(editor),
            _ => None,
        }
    }
}
