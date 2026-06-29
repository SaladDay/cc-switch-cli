use super::*;

impl App {
    pub(crate) fn on_editor_key(&mut self, key: KeyEvent) -> Action {
        let viewport = self.editor_viewport_size();

        let Some(editor) = &mut self.editor else {
            return Action::None;
        };

        if let EditorSubmit::ConfigCommonSnippet { app_type, source } = &editor.submit {
            match key.code {
                KeyCode::F(2) => {
                    return Action::EditorFormatCommonSnippet {
                        app_type: app_type.clone(),
                    };
                }
                KeyCode::F(4) if matches!(source, CommonSnippetViewSource::ProviderForm) => {
                    return Action::EditorExtractCommonSnippet {
                        app_type: app_type.clone(),
                    };
                }
                _ => {}
            }
        }

        if is_save_shortcut(key) {
            return Action::EditorSubmit {
                submit: editor.submit.clone(),
                content: editor.text(),
            };
        }

        if is_open_external_editor_shortcut(key) {
            return Action::EditorOpenExternal;
        }

        if editor.apply_editor_key(key, viewport) {
            return Action::None;
        }

        match key.code {
            KeyCode::Esc => {
                if editor.is_dirty() {
                    self.overlay = Overlay::Confirm(ConfirmOverlay {
                        title: texts::tui_editor_save_before_close_title().to_string(),
                        message: texts::tui_editor_save_before_close_message().to_string(),
                        action: ConfirmAction::EditorSaveBeforeClose,
                    });
                    Action::None
                } else {
                    self.editor = None;
                    Action::None
                }
            }
            _ => Action::None,
        }
    }

    pub(crate) fn editor_viewport_size(&self) -> Size {
        // Matches `render()` + `render_content()` + `render_editor()` layout math in `ui.rs`.
        let mut width = self.last_size.width.saturating_sub(30);
        let mut height = self.last_size.height.saturating_sub(3).saturating_sub(1);

        if self.filter.active || !self.filter.input.value.trim().is_empty() {
            height = height.saturating_sub(5);
        }

        // render_editor:
        // - outer borders (2)
        // - key bar row (1)
        // - field borders (2)
        width = width.saturating_sub(2).saturating_sub(2);
        height = height.saturating_sub(2).saturating_sub(1).saturating_sub(2);

        Size {
            width: width.max(1),
            height: height.max(1),
        }
    }
}
