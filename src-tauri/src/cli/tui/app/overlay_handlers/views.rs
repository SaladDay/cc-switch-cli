use super::*;

impl App {
    pub(super) fn handle_overlay_edit_shortcut(
        &mut self,
        key: KeyEvent,
        data: &UiData,
    ) -> Option<Action> {
        if !matches!(key.code, KeyCode::Char('e')) {
            return None;
        }

        match &self.overlay {
            Overlay::CommonSnippetPicker { selected } => {
                let app_type = snippet_picker_app_type(*selected);
                self.open_common_snippet_editor(
                    app_type,
                    data,
                    None,
                    CommonSnippetViewSource::Global,
                );
                Some(Action::None)
            }
            _ => None,
        }
    }

    pub(super) fn handle_view_overlay_key(
        &mut self,
        key: KeyEvent,
        data: &UiData,
    ) -> Option<Action> {
        if let Some(action) = self.handle_help_overlay_key(key) {
            return Some(action);
        }
        if let Some(action) = self.handle_backup_picker_key(key, data) {
            return Some(action);
        }
        if let Some(action) = self.handle_model_route_provider_picker_key(key, data) {
            return Some(action);
        }
        if let Some(action) = self.handle_text_view_overlay_key(key, data) {
            return Some(action);
        }
        if let Some(action) = self.handle_common_snippet_picker_key(key, data) {
            return Some(action);
        }
        if let Some(action) = self.handle_loading_overlay_key(key) {
            return Some(action);
        }
        if let Some(action) = self.handle_speedtest_overlay_key(key) {
            return Some(action);
        }
        if let Some(action) = self.handle_stream_check_overlay_key(key) {
            return Some(action);
        }
        if let Some(action) = self.handle_update_overlay_key(key) {
            return Some(action);
        }
        None
    }

    fn handle_help_overlay_key(&mut self, key: KeyEvent) -> Option<Action> {
        if !matches!(self.overlay, Overlay::Help(_)) {
            return None;
        }
        Some(match key.code {
            KeyCode::Esc | KeyCode::Char('?') => {
                self.close_overlay();
                Action::None
            }
            KeyCode::Up => {
                if let Overlay::Help(help) = &mut self.overlay {
                    help.scroll = help.scroll.saturating_sub(1);
                }
                Action::None
            }
            KeyCode::Down => {
                if let Overlay::Help(help) = &mut self.overlay {
                    if !help.content.lines.is_empty() {
                        help.scroll = (help.scroll + 1).min(help.content.lines.len() - 1);
                    }
                }
                Action::None
            }
            _ => Action::None,
        })
    }

    fn handle_backup_picker_key(&mut self, key: KeyEvent, data: &UiData) -> Option<Action> {
        let Overlay::BackupPicker { selected } = &mut self.overlay else {
            return None;
        };

        let backups = &data.config.backups;
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
                if !backups.is_empty() {
                    *selected = (*selected + 1).min(backups.len() - 1);
                }
                Action::None
            }
            KeyCode::Enter => {
                let Some(backup) = backups.get(*selected) else {
                    return Some(Action::None);
                };
                let id = backup.id.clone();
                self.overlay = Overlay::Confirm(ConfirmOverlay {
                    title: texts::tui_confirm_restore_backup_title().to_string(),
                    message: texts::tui_confirm_restore_backup_message(&backup.display_name),
                    action: ConfirmAction::ConfigRestoreBackup { id },
                });
                Action::None
            }
            _ => Action::None,
        })
    }

    fn handle_text_view_overlay_key(&mut self, key: KeyEvent, data: &UiData) -> Option<Action> {
        if !matches!(self.overlay, Overlay::TextView(_)) {
            return None;
        }

        Some(match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.overlay = Overlay::None;
                Action::None
            }
            KeyCode::Char('t') | KeyCode::Char('T') => {
                let has_action = matches!(
                    &self.overlay,
                    Overlay::TextView(TextViewState {
                        action: Some(TextViewAction::ProxyToggleManagedRoute),
                        ..
                    })
                );
                if has_action {
                    self.main_proxy_action(data)
                } else {
                    Action::None
                }
            }
            KeyCode::Up => {
                if let Overlay::TextView(view) = &mut self.overlay {
                    view.scroll = view.scroll.saturating_sub(1);
                }
                Action::None
            }
            KeyCode::Down => {
                if let Overlay::TextView(view) = &mut self.overlay {
                    if !view.lines.is_empty() {
                        view.scroll = (view.scroll + 1).min(view.lines.len() - 1);
                    }
                }
                Action::None
            }
            _ => Action::None,
        })
    }

    fn handle_common_snippet_picker_key(&mut self, key: KeyEvent, data: &UiData) -> Option<Action> {
        let Overlay::CommonSnippetPicker { selected } = &mut self.overlay else {
            return None;
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
                *selected = (*selected + 1).min(3);
                Action::None
            }
            KeyCode::Enter => {
                let app_type = snippet_picker_app_type(*selected);
                self.open_common_snippet_editor(
                    app_type,
                    data,
                    None,
                    CommonSnippetViewSource::Global,
                );
                Action::None
            }
            _ => Action::None,
        })
    }

    fn handle_loading_overlay_key(&mut self, key: KeyEvent) -> Option<Action> {
        let Overlay::Loading { kind, .. } = &self.overlay else {
            return None;
        };

        Some(match key.code {
            KeyCode::Esc => {
                let kind = *kind;
                self.overlay = Overlay::None;
                if kind == LoadingKind::UpdateCheck {
                    Action::CancelUpdateCheck
                } else {
                    Action::None
                }
            }
            _ => Action::None,
        })
    }

    fn handle_speedtest_overlay_key(&mut self, key: KeyEvent) -> Option<Action> {
        if matches!(self.overlay, Overlay::SpeedtestRunning { .. }) {
            return Some(match key.code {
                KeyCode::Esc => {
                    self.overlay = Overlay::None;
                    Action::None
                }
                _ => Action::None,
            });
        }

        let Overlay::SpeedtestResult { scroll, lines, .. } = &mut self.overlay else {
            return None;
        };
        Some(match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.overlay = Overlay::None;
                Action::None
            }
            KeyCode::Up => {
                *scroll = scroll.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                if !lines.is_empty() {
                    *scroll = (*scroll + 1).min(lines.len() - 1);
                }
                Action::None
            }
            _ => Action::None,
        })
    }

    fn handle_stream_check_overlay_key(&mut self, key: KeyEvent) -> Option<Action> {
        if matches!(self.overlay, Overlay::StreamCheckRunning { .. }) {
            return Some(match key.code {
                KeyCode::Esc => {
                    self.overlay = Overlay::None;
                    Action::None
                }
                _ => Action::None,
            });
        }

        let Overlay::StreamCheckResult { scroll, lines, .. } = &mut self.overlay else {
            return None;
        };
        Some(match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.overlay = Overlay::None;
                Action::None
            }
            KeyCode::Up => {
                *scroll = scroll.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                if !lines.is_empty() {
                    *scroll = (*scroll + 1).min(lines.len() - 1);
                }
                Action::None
            }
            _ => Action::None,
        })
    }

    fn handle_update_overlay_key(&mut self, key: KeyEvent) -> Option<Action> {
        if let Overlay::UpdateAvailable { selected, .. } = &mut self.overlay {
            return Some(match key.code {
                KeyCode::Left => {
                    *selected = 0;
                    Action::None
                }
                KeyCode::Right => {
                    *selected = 1;
                    Action::None
                }
                KeyCode::Enter => {
                    if *selected == 0 {
                        Action::ConfirmUpdate
                    } else {
                        Action::CancelUpdate
                    }
                }
                KeyCode::Esc => Action::CancelUpdate,
                _ => Action::None,
            });
        }

        if matches!(self.overlay, Overlay::UpdateDownloading { .. }) {
            return Some(match key.code {
                KeyCode::Esc => {
                    self.overlay = Overlay::None;
                    Action::None
                }
                _ => Action::None,
            });
        }

        let Overlay::UpdateResult { success, .. } = &self.overlay else {
            return None;
        };
        let should_exit = *success;
        Some(match key.code {
            KeyCode::Enter => {
                self.overlay = Overlay::None;
                if should_exit {
                    self.should_quit = true;
                }
                Action::None
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.overlay = Overlay::None;
                Action::None
            }
            _ => Action::None,
        })
    }

    fn handle_model_route_provider_picker_key(
        &mut self,
        key: KeyEvent,
        data: &UiData,
    ) -> Option<Action> {
        let Overlay::ModelRouteProviderPicker {
            pattern,
            selected,
            editing,
            existing_id,
        } = &mut self.overlay
        else {
            return None;
        };

        let providers = &data.providers.rows;

        Some(match key.code {
            KeyCode::Esc => {
                self.overlay = Overlay::TextInput(TextInputState {
                    title: if *editing {
                        texts::tui_model_route_edit_pattern_title().to_string()
                    } else {
                        texts::tui_model_route_add_pattern_title().to_string()
                    },
                    prompt: if *editing {
                        texts::tui_model_route_edit_pattern_prompt().to_string()
                    } else {
                        texts::tui_model_route_add_pattern_prompt().to_string()
                    },
                    input: TextInput::new(pattern.clone()),
                    submit: if *editing {
                        TextSubmit::ModelRouteEditPattern {
                            id: existing_id.clone().unwrap_or_default(),
                        }
                    } else {
                        TextSubmit::ModelRouteAddPattern
                    },
                    secret: false,
                });
                Action::None
            }
            KeyCode::Up => {
                *selected = selected.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                if !providers.is_empty() {
                    *selected = (*selected + 1).min(providers.len() - 1);
                }
                Action::None
            }
            KeyCode::Enter => {
                if let Some(provider_row) = providers.get(*selected) {
                    let provider_id = provider_row.id.clone();
                    let pattern = std::mem::take(pattern);
                    let is_editing = *editing;
                    let eid = existing_id.clone();
                    self.overlay = Overlay::TextInput(TextInputState {
                        title: texts::tui_model_route_add_priority_title().to_string(),
                        prompt: texts::tui_model_route_add_priority_prompt().to_string(),
                        input: TextInput::new("0".to_string()),
                        submit: if is_editing {
                            TextSubmit::ModelRouteEditPriority {
                                id: eid.unwrap_or_default(),
                                pattern,
                                provider_id,
                            }
                        } else {
                            TextSubmit::ModelRouteAddPriority {
                                pattern,
                                provider_id,
                            }
                        },
                        secret: false,
                    });
                }
                Action::None
            }
            _ => Action::None,
        })
    }
}
