use super::*;

impl App {
    pub(crate) fn move_sessions_focus_left(&mut self) -> Action {
        if self.focus == Focus::Nav {
            return Action::None;
        }

        match self.sessions.pane {
            SessionsPane::List => {
                self.focus = Focus::Nav;
            }
            SessionsPane::Detail => {
                self.sessions.pane = SessionsPane::List;
                self.sessions.clear_detail();
            }
        }
        Action::None
    }

    pub(crate) fn move_sessions_focus_right(&mut self, data: &UiData) -> Action {
        if self.focus == Focus::Nav {
            self.focus = Focus::Content;
            self.sessions.pane = SessionsPane::List;
            return Action::None;
        }

        match self.sessions.pane {
            SessionsPane::List => self.open_selected_session_detail(data),
            SessionsPane::Detail => Action::None,
        }
    }

    fn open_selected_session_detail(&mut self, data: &UiData) -> Action {
        let visible = visible_sessions_for_state(
            &self.filter,
            &self.app_type,
            self.sessions.provider_id.as_deref(),
            &self.sessions.project_scope,
            &self.sessions.rows,
            self.sessions.detail_key.as_deref(),
            self.sessions.messages_loaded,
            &self.sessions.messages,
            self.sessions.deep_search_query.as_deref(),
            &self.sessions.deep_search_results,
            self.sessions
                .materialized_view_is_current(self.filter.query_lower().as_deref()),
            self.sessions.rows_revision,
            self.sessions.messages_revision,
            self.sessions.deep_search_seq,
            &self.sessions.visibility_cache,
        );
        let Some(session) = visible.get(self.sessions.selected_idx) else {
            return Action::None;
        };
        let key = session_key(session);
        let provider_id = session.provider_id.clone();
        let source_path = session.source_path.clone();
        self.sessions.open_detail(key.clone());
        self.sessions.pane = SessionsPane::Detail;
        self.clamp_selections(data);
        match source_path {
            Some(source_path) => Action::SessionMessagesLoad {
                key,
                provider_id,
                source_path,
            },
            None => {
                self.push_toast(
                    texts::tui_sessions_toast_source_missing(),
                    ToastKind::Warning,
                );
                Action::None
            }
        }
    }

    fn is_provider_read_only(&self, row: &super::data::ProviderRow) -> bool {
        super::data::provider_is_read_only(&self.app_type, row)
    }

    fn can_delete_provider(&self, row: &super::data::ProviderRow) -> bool {
        !self.is_provider_read_only(row) && (self.app_type.is_additive_mode() || !row.is_current)
    }

    fn show_provider_read_only_toast(&mut self) {
        self.push_toast(
            texts::tui_toast_provider_managed_by_hermes(),
            ToastKind::Info,
        );
    }

    fn open_provider_delete_confirm(&mut self, row: &super::data::ProviderRow) {
        self.overlay = Overlay::Confirm(ConfirmOverlay {
            title: texts::tui_confirm_delete_provider_title().to_string(),
            message: texts::tui_confirm_delete_provider_message(
                &super::data::provider_display_name(&self.app_type, row),
                &row.id,
            ),
            action: ConfirmAction::ProviderDelete { id: row.id.clone() },
        });
    }

    fn open_provider_remove_confirm(&mut self, row: &super::data::ProviderRow) {
        self.overlay = Overlay::Confirm(ConfirmOverlay {
            title: texts::tui_confirm_remove_provider_title().to_string(),
            message: texts::tui_confirm_remove_provider_message(
                &super::data::provider_display_name(&self.app_type, row),
            ),
            action: ConfirmAction::ProviderRemoveFromConfig { id: row.id.clone() },
        });
    }

    fn open_provider_copy_confirm(&mut self, row: &super::data::ProviderRow) {
        self.overlay = Overlay::Confirm(ConfirmOverlay {
            title: texts::tui_confirm_copy_provider_title().to_string(),
            message: texts::tui_confirm_copy_provider_message(
                &super::data::provider_display_name(&self.app_type, row),
                &row.id,
            ),
            action: ConfirmAction::ProviderCopy { id: row.id.clone() },
        });
    }

    fn provider_switch_action(&mut self, row: &super::data::ProviderRow) -> Action {
        if self.app_type.is_additive_mode() {
            if row.is_in_config {
                if matches!(self.app_type, AppType::OpenClaw | AppType::Hermes)
                    && row.is_default_model
                {
                    self.push_toast(
                        texts::tui_toast_provider_cannot_remove_default_model(),
                        ToastKind::Warning,
                    );
                    return Action::None;
                }
                self.open_provider_remove_confirm(row);
                return Action::None;
            }

            return Action::ProviderSwitch { id: row.id.clone() };
        }
        if row.is_current {
            self.push_toast(texts::tui_toast_provider_already_in_use(), ToastKind::Info);
            return Action::None;
        }
        Action::ProviderSwitch { id: row.id.clone() }
    }

    pub(crate) fn provider_speedtest_action(&mut self, row: &super::data::ProviderRow) -> Action {
        let Some(url) = row.api_url.clone() else {
            self.push_toast(texts::tui_toast_provider_no_api_url(), ToastKind::Warning);
            return Action::None;
        };
        self.overlay = Overlay::SpeedtestRunning { url: url.clone() };
        Action::ProviderSpeedtest { url }
    }

    pub(crate) fn provider_stream_check_action(
        &mut self,
        row: &super::data::ProviderRow,
    ) -> Action {
        if !supports_provider_stream_check(&self.app_type) {
            return Action::None;
        }
        self.overlay = Overlay::StreamCheckRunning {
            provider_id: row.id.clone(),
            provider_name: super::data::provider_display_name(&self.app_type, row),
        };
        Action::ProviderStreamCheck { id: row.id.clone() }
    }

    pub(crate) fn open_provider_test_menu(&mut self, row: &super::data::ProviderRow) {
        self.overlay = Overlay::ProviderTestMenu {
            provider_id: row.id.clone(),
            selected: 0,
        };
    }

    fn provider_set_default_action(&mut self, row: &super::data::ProviderRow) -> Action {
        if !matches!(self.app_type, AppType::OpenClaw | AppType::Hermes) {
            return Action::None;
        }
        if !row.is_in_config {
            self.push_toast(
                texts::tui_toast_provider_default_requires_live_config(),
                ToastKind::Warning,
            );
            return Action::None;
        }
        let model_id = row.primary_model_id.clone().unwrap_or_default();
        if matches!(self.app_type, AppType::OpenClaw) && model_id.is_empty() {
            self.push_toast(
                texts::tui_toast_provider_default_model_missing(),
                ToastKind::Warning,
            );
            return Action::None;
        }
        Action::ProviderSetDefaultModel {
            provider_id: row.id.clone(),
            model_id,
        }
    }

    pub(crate) fn on_providers_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        use crate::cli::tui::keymap::providers::Intent;

        let visible = visible_providers(&self.app_type, &self.filter, data);
        match key.code {
            KeyCode::Up => {
                self.provider_idx = self.provider_idx.saturating_sub(1);
                return Action::None;
            }
            KeyCode::Down => {
                if !visible.is_empty() {
                    self.provider_idx = (self.provider_idx + 1).min(visible.len() - 1);
                }
                return Action::None;
            }
            _ => {}
        }

        let Some(intent) = crate::cli::tui::keymap::providers::intent_for(key.code) else {
            return Action::None;
        };

        match intent {
            Intent::Primary => {
                if data.providers.rows.is_empty() {
                    return Action::ProviderImportLiveConfig;
                }
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                // Enter opens the edit form directly (same as `e`).
                if self.is_provider_read_only(row) {
                    self.show_provider_read_only_toast();
                    return Action::None;
                }
                self.open_provider_edit_form(row, data);
                Action::None
            }
            Intent::Add => {
                self.open_provider_add_form(data);
                Action::None
            }
            Intent::Copy => {
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                self.open_provider_copy_confirm(row);
                Action::None
            }
            Intent::Edit => {
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                if self.is_provider_read_only(row) {
                    self.show_provider_read_only_toast();
                    return Action::None;
                }
                self.open_provider_edit_form(row, data);
                Action::None
            }
            Intent::Switch => {
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                if supports_failover_controls(&self.app_type) && data.proxy.auto_failover_enabled {
                    self.push_toast(
                        crate::t!(
                            "Automatic failover is on. Routing follows queue priority; press f to manage it.",
                            "自动故障转移已开启，路由由队列优先级决定；按 f 管理队列。"
                        ),
                        ToastKind::Info,
                    );
                    return Action::None;
                }
                self.provider_switch_action(row)
            }
            Intent::SetDefault => {
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                self.provider_set_default_action(row)
            }
            Intent::Delete => {
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                if self.is_provider_read_only(row) {
                    self.show_provider_read_only_toast();
                    return Action::None;
                }
                if !self.can_delete_provider(row) {
                    self.push_toast(
                        texts::tui_toast_provider_cannot_delete_current(),
                        ToastKind::Warning,
                    );
                    return Action::None;
                }
                self.open_provider_delete_confirm(row);
                Action::None
            }
            Intent::Test => {
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                self.open_provider_test_menu(row);
                Action::None
            }
            Intent::LaunchTemp => {
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                if !supports_temporary_provider_launch(&self.app_type) {
                    return Action::None;
                }
                Action::ProviderLaunchTemporary { id: row.id.clone() }
            }
            Intent::Failover => {
                if !supports_failover_controls(&self.app_type) {
                    return Action::None;
                }
                let selected_provider_id = visible
                    .get(self.provider_idx)
                    .map(|row| row.id.clone())
                    .or_else(|| failover_queue_rows(data).first().map(|row| row.id.clone()));
                self.overlay = Overlay::FailoverQueueManager {
                    selected_provider_id,
                };
                Action::None
            }
            Intent::RefreshQuota => {
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                if data::quota_target_for_provider(&self.app_type, row).is_none() {
                    self.push_toast(texts::tui_toast_quota_not_available(), ToastKind::Info);
                    return Action::None;
                }
                Action::ProviderQuotaRefresh { id: row.id.clone() }
            }
        }
    }

    pub(crate) fn on_mcp_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        use crate::cli::tui::keymap::mcp::Intent;

        let visible = visible_mcp(&self.filter, data);
        match key.code {
            KeyCode::Up => {
                self.mcp_idx = self.mcp_idx.saturating_sub(1);
                return Action::None;
            }
            KeyCode::Down => {
                if !visible.is_empty() {
                    self.mcp_idx = (self.mcp_idx + 1).min(visible.len() - 1);
                }
                return Action::None;
            }
            _ => {}
        }

        let Some(intent) = crate::cli::tui::keymap::mcp::intent_for(key.code) else {
            return Action::None;
        };

        match intent {
            Intent::Add => {
                self.open_mcp_add_form();
                Action::None
            }
            Intent::Edit => {
                let Some(row) = visible.get(self.mcp_idx) else {
                    return Action::None;
                };
                self.open_mcp_edit_form(row);
                Action::None
            }
            Intent::Toggle => {
                let Some(row) = visible.get(self.mcp_idx) else {
                    return Action::None;
                };
                let enabled = row.server.apps.is_enabled_for(&self.app_type);
                Action::McpToggle {
                    id: row.id.clone(),
                    enabled: !enabled,
                }
            }
            Intent::Apps => {
                let Some(row) = visible.get(self.mcp_idx) else {
                    return Action::None;
                };
                self.overlay = Overlay::McpAppsPicker {
                    id: row.id.clone(),
                    name: row.server.name.clone(),
                    selected: four_app_picker_index(&self.app_type),
                    apps: row.server.apps.clone(),
                };
                Action::None
            }
            Intent::Import => Action::McpImport,
            Intent::Delete => {
                let Some(row) = visible.get(self.mcp_idx) else {
                    return Action::None;
                };
                self.overlay = Overlay::Confirm(ConfirmOverlay {
                    title: texts::tui_confirm_delete_mcp_title().to_string(),
                    message: texts::tui_confirm_delete_mcp_message(&row.server.name, &row.id),
                    action: ConfirmAction::McpDelete { id: row.id.clone() },
                });
                Action::None
            }
        }
    }

    pub(crate) fn on_prompts_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        use crate::cli::tui::keymap::prompts::Intent;

        let visible = visible_prompts(&self.filter, data);
        match key.code {
            KeyCode::Up => {
                self.prompt_idx = self.prompt_idx.saturating_sub(1);
                return Action::None;
            }
            KeyCode::Down => {
                if !visible.is_empty() {
                    self.prompt_idx = (self.prompt_idx + 1).min(visible.len() - 1);
                }
                return Action::None;
            }
            _ => {}
        }

        let Some(intent) = crate::cli::tui::keymap::prompts::intent_for(key.code) else {
            return Action::None;
        };

        match intent {
            Intent::Add => {
                self.open_prompt_create_form(data);
                Action::None
            }
            Intent::View => {
                let Some(row) = visible.get(self.prompt_idx) else {
                    return Action::None;
                };
                self.overlay = Overlay::TextView(TextViewState {
                    title: texts::tui_prompt_title(&row.prompt.name),
                    lines: row.prompt.content.lines().map(|s| s.to_string()).collect(),
                    scroll: 0,
                    action: None,
                });
                Action::None
            }
            Intent::Toggle => {
                let Some(row) = visible.get(self.prompt_idx) else {
                    return Action::None;
                };
                if row.prompt.enabled {
                    Action::PromptDeactivate { id: row.id.clone() }
                } else {
                    Action::PromptActivate { id: row.id.clone() }
                }
            }
            Intent::Delete => {
                let Some(row) = visible.get(self.prompt_idx) else {
                    return Action::None;
                };
                if row.prompt.enabled {
                    self.push_toast(
                        texts::tui_toast_prompt_cannot_delete_active(),
                        ToastKind::Warning,
                    );
                    return Action::None;
                }
                self.overlay = Overlay::Confirm(ConfirmOverlay {
                    title: texts::tui_confirm_delete_prompt_title().to_string(),
                    message: texts::tui_confirm_delete_prompt_message(&row.prompt.name, &row.id),
                    action: ConfirmAction::PromptDelete { id: row.id.clone() },
                });
                Action::None
            }
            Intent::Edit => {
                let Some(row) = visible.get(self.prompt_idx) else {
                    return Action::None;
                };
                self.filter.active = false;
                self.editor = None;
                self.overlay = Overlay::None;
                self.form = Some(FormState::PromptMeta(PromptMetaFormState::from_prompt(
                    &row.prompt,
                )));
                Action::None
            }
        }
    }

    pub(crate) fn on_sessions_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        use super::paged_list::PageDirection;
        use crate::cli::tui::keymap::sessions::Intent;

        let visible = visible_sessions_for_state(
            &self.filter,
            &self.app_type,
            self.sessions.provider_id.as_deref(),
            &self.sessions.project_scope,
            &self.sessions.rows,
            self.sessions.detail_key.as_deref(),
            self.sessions.messages_loaded,
            &self.sessions.messages,
            self.sessions.deep_search_query.as_deref(),
            &self.sessions.deep_search_results,
            self.sessions
                .materialized_view_is_current(self.filter.query_lower().as_deref()),
            self.sessions.rows_revision,
            self.sessions.messages_revision,
            self.sessions.deep_search_seq,
            &self.sessions.visibility_cache,
        );
        let visible_len = visible.len();
        self.sessions.selected_idx = self
            .sessions
            .selected_idx
            .min(visible_len.saturating_sub(1));
        self.sessions
            .pagination
            .sync_len(self.sessions.logical_total_rows());
        let page_start = self
            .sessions
            .remote
            .current_page()
            .saturating_mul(crate::session_manager::paged_manifest::PAGE_SIZE);
        let explicit_direction = match key.code {
            KeyCode::Up | KeyCode::PageUp => Some(PageDirection::Previous),
            KeyCode::Down | KeyCode::PageDown => Some(PageDirection::Next),
            _ => None,
        };
        let selected_absolute = if explicit_direction.is_none()
            && matches!(
                self.sessions.pagination.focus(),
                super::paged_list::PagedListFocus::Boundary(_)
            ) {
            self.sessions.pagination.selected_index().unwrap_or(0)
        } else {
            self.sessions.selected_source_absolute(
                visible_len,
                explicit_direction == Some(PageDirection::Next),
            )
        };
        if visible_len > 0
            && self.sessions.pagination.selected_index() != Some(selected_absolute)
            && !self.sessions.remote.input_is_blocked()
        {
            self.sessions.pagination.select(selected_absolute);
        }
        let gate_before_input = self.sessions.pagination.clone();
        macro_rules! apply_paged {
            ($outcome:expr) => {{
                let previous_gate = gate_before_input.clone();
                let outcome = $outcome;
                let selected_absolute = self.sessions.pagination.selected_index().unwrap_or(0);
                self.apply_sessions_paged_outcome(
                    outcome,
                    previous_gate,
                    selected_absolute,
                    visible_len,
                    None,
                )
            }};
        }
        if self.sessions.remote.input_is_blocked() {
            let reverse = match key.code {
                KeyCode::Up | KeyCode::PageUp => Some(PageDirection::Previous),
                KeyCode::Down | KeyCode::PageDown => Some(PageDirection::Next),
                _ => None,
            };
            if reverse.is_some_and(|direction| self.sessions.cancel_page_cross(direction)) {
                return Action::None;
            }
            return Action::None;
        }
        match key.code {
            KeyCode::Left => {
                self.dismiss_sessions_boundary();
                self.move_sessions_focus_left()
            }
            KeyCode::Right => {
                self.dismiss_sessions_boundary();
                self.move_sessions_focus_right(data)
            }
            KeyCode::Up => {
                match self.sessions.pane {
                    SessionsPane::List => {
                        let outcome = self.sessions.pagination.line(PageDirection::Previous);
                        return apply_paged!(outcome);
                    }
                    SessionsPane::Detail => {
                        let next_idx = {
                            let messages = visible_session_messages(&self.sessions);
                            if messages.is_empty() {
                                Some(0)
                            } else {
                                let current = messages
                                    .visible_index_of(self.sessions.message_idx)
                                    .unwrap_or(0);
                                messages
                                    .get(current.saturating_sub(1))
                                    .map(|(index, _)| index)
                            }
                        };
                        if let Some(next_idx) = next_idx {
                            self.sessions.message_idx = next_idx;
                        }
                    }
                }
                Action::None
            }
            KeyCode::Down => {
                match self.sessions.pane {
                    SessionsPane::List => {
                        let outcome = self.sessions.pagination.line(PageDirection::Next);
                        return apply_paged!(outcome);
                    }
                    SessionsPane::Detail => {
                        let next_idx = {
                            let messages = visible_session_messages(&self.sessions);
                            if messages.is_empty() {
                                Some(0)
                            } else {
                                let current = messages
                                    .visible_index_of(self.sessions.message_idx)
                                    .unwrap_or(0);
                                messages
                                    .get((current + 1).min(messages.len() - 1))
                                    .map(|(index, _)| index)
                            }
                        };
                        if let Some(next_idx) = next_idx {
                            self.sessions.message_idx = next_idx;
                        }
                    }
                }
                Action::None
            }
            KeyCode::PageDown => {
                if matches!(self.sessions.pane, SessionsPane::List) && !visible.is_empty() {
                    let page = self.sessions_list_page_size();
                    let outcome = self.sessions.pagination.lines(PageDirection::Next, page);
                    return apply_paged!(outcome);
                }
                Action::None
            }
            KeyCode::PageUp => {
                if matches!(self.sessions.pane, SessionsPane::List) {
                    let page = self.sessions_list_page_size();
                    let outcome = self
                        .sessions
                        .pagination
                        .lines(PageDirection::Previous, page);
                    return apply_paged!(outcome);
                }
                Action::None
            }
            KeyCode::Home => {
                if matches!(self.sessions.pane, SessionsPane::List) {
                    let outcome = self.sessions.pagination.select(page_start);
                    return apply_paged!(outcome);
                }
                Action::None
            }
            KeyCode::End => {
                if matches!(self.sessions.pane, SessionsPane::List) && !visible.is_empty() {
                    let outcome = self
                        .sessions
                        .pagination
                        .select(page_start.saturating_add(visible.len() - 1));
                    return apply_paged!(outcome);
                }
                Action::None
            }
            _ => {
                let Some(intent) = crate::cli::tui::keymap::sessions::intent_for(key.code) else {
                    return Action::None;
                };
                match intent {
                    Intent::View => match self.sessions.pane {
                        SessionsPane::List => {
                            let outcome = self.sessions.pagination.enter();
                            if outcome.crossed_page() {
                                apply_paged!(outcome)
                            } else {
                                self.open_selected_session_detail(data)
                            }
                        }
                        SessionsPane::Detail => {
                            let messages = visible_session_messages(&self.sessions);
                            let message = messages
                                .by_message_index(self.sessions.message_idx)
                                .or_else(|| messages.first().map(|(_, message)| message));
                            let Some(message) = message else {
                                return Action::None;
                            };
                            self.overlay = Overlay::TextView(TextViewState {
                                title: texts::tui_sessions_message_detail_title(&message.role),
                                lines: bounded_session_message_overlay_lines(&message.content),
                                scroll: 0,
                                action: None,
                            });
                            Action::None
                        }
                    },
                    Intent::Restore => {
                        let Some(session) = self.selected_session_from_visible(&visible) else {
                            return Action::None;
                        };
                        let Some(command) = session
                            .resume_command
                            .clone()
                            .filter(|value| !value.trim().is_empty())
                        else {
                            self.push_toast(
                                texts::tui_sessions_toast_action_unavailable(),
                                ToastKind::Info,
                            );
                            return Action::None;
                        };
                        Action::SessionResume {
                            command,
                            cwd: session.project_dir.clone(),
                        }
                    }
                    Intent::Delete => {
                        let Some(session) = self.selected_session_from_visible(&visible) else {
                            return Action::None;
                        };
                        let Some(source_path) = session
                            .source_path
                            .clone()
                            .filter(|value| !value.trim().is_empty())
                        else {
                            self.push_toast(
                                texts::tui_sessions_toast_source_missing(),
                                ToastKind::Warning,
                            );
                            return Action::None;
                        };
                        let key = session_key(session);
                        self.overlay = Overlay::Confirm(ConfirmOverlay {
                            title: texts::tui_sessions_delete_confirm_title().to_string(),
                            message: texts::tui_sessions_delete_confirm_message(&session_title(
                                session,
                            )),
                            action: ConfirmAction::SessionDelete {
                                key,
                                provider_id: session.provider_id.clone(),
                                session_id: session.session_id.clone(),
                                source_path,
                            },
                        });
                        Action::None
                    }
                    Intent::Refresh => Action::SessionsRefresh,
                    Intent::Project => Action::SessionsProjectCatalogLoad,
                }
            }
        }
    }

    pub(crate) fn on_sessions_wheel(
        &mut self,
        direction: crate::cli::tui::input::ScrollDirection,
        steps: u32,
        gesture: crate::cli::tui::input::WheelGestureId,
        _data: &UiData,
    ) -> Action {
        use super::paged_list::PageDirection;

        if matches!(self.sessions.pane, SessionsPane::Detail) {
            let messages = visible_session_messages(&self.sessions);
            if messages.is_empty() {
                self.sessions.message_idx = 0;
                return Action::None;
            }
            let current = messages
                .visible_index_of(self.sessions.message_idx)
                .unwrap_or(0);
            let steps = usize::try_from(steps).unwrap_or(usize::MAX);
            let target = match direction {
                crate::cli::tui::input::ScrollDirection::Up => current.saturating_sub(steps),
                crate::cli::tui::input::ScrollDirection::Down => {
                    current.saturating_add(steps).min(messages.len() - 1)
                }
            };
            self.sessions.message_idx = messages.get(target).map(|(index, _)| index).unwrap_or(0);
            return Action::None;
        }

        let visible = visible_sessions_for_state(
            &self.filter,
            &self.app_type,
            self.sessions.provider_id.as_deref(),
            &self.sessions.project_scope,
            &self.sessions.rows,
            self.sessions.detail_key.as_deref(),
            self.sessions.messages_loaded,
            &self.sessions.messages,
            self.sessions.deep_search_query.as_deref(),
            &self.sessions.deep_search_results,
            self.sessions
                .materialized_view_is_current(self.filter.query_lower().as_deref()),
            self.sessions.rows_revision,
            self.sessions.messages_revision,
            self.sessions.deep_search_seq,
            &self.sessions.visibility_cache,
        );
        let visible_len = visible.len();
        if self.sessions.remote.input_is_blocked() {
            let direction = PageDirection::from(direction);
            let _ = self.sessions.cancel_page_cross(direction);
            return Action::None;
        }
        self.sessions.selected_idx = self
            .sessions
            .selected_idx
            .min(visible_len.saturating_sub(1));
        self.sessions
            .pagination
            .sync_len(self.sessions.logical_total_rows());
        let absolute = self.sessions.selected_source_absolute(
            visible_len,
            direction == crate::cli::tui::input::ScrollDirection::Down,
        );
        if visible_len > 0 && self.sessions.pagination.selected_index() != Some(absolute) {
            self.sessions.pagination.select(absolute);
        }
        let previous_gate = self.sessions.pagination.clone();
        let outcome = self.sessions.pagination.wheel(
            PageDirection::from(direction),
            gesture,
            usize::try_from(steps).unwrap_or(usize::MAX),
        );
        let selected_absolute = self.sessions.pagination.selected_index().unwrap_or(0);
        self.apply_sessions_paged_outcome(
            outcome,
            previous_gate,
            selected_absolute,
            visible_len,
            Some(gesture),
        )
    }

    fn dismiss_sessions_boundary(&mut self) {
        if !self.sessions.pagination.is_row_focused() {
            let absolute = self.sessions.selected_collapsed_absolute();
            self.sessions.pagination.select(absolute);
        }
        self.sessions.remote.dismiss_page_error();
    }

    fn apply_sessions_paged_outcome(
        &mut self,
        outcome: super::paged_list::PagedListOutcome,
        previous_gate: super::paged_list::PagedListState,
        selected_absolute: usize,
        visible_len: usize,
        wheel_gesture: Option<crate::cli::tui::input::WheelGestureId>,
    ) -> Action {
        if outcome.crossed_page() {
            let target_page = selected_absolute / crate::session_manager::paged_manifest::PAGE_SIZE;
            if self
                .sessions
                .begin_page_cross(target_page, previous_gate, wheel_gesture)
            {
                return Action::None;
            }
            return Action::None;
        }
        if !matches!(
            self.sessions.pagination.focus(),
            super::paged_list::PagedListFocus::Boundary(_)
        ) {
            self.sessions.remote.dismiss_page_error();
        }
        let selected = selected_absolute.saturating_sub(
            self.sessions
                .remote
                .current_page()
                .saturating_mul(crate::session_manager::paged_manifest::PAGE_SIZE),
        );
        // The gate tracks immutable source coordinates, while `selected_idx`
        // indexes the current tombstone/filter-collapsed rows. A source page
        // can therefore end at 99 while only 90 rows remain visible.
        self.sessions.selected_idx = selected.min(visible_len.saturating_sub(1));
        Action::None
    }

    /// Approximate the session list viewport height for PageUp/PageDown, derived
    /// from the last rendered terminal size minus the surrounding chrome.
    fn sessions_list_page_size(&self) -> usize {
        // Rows consumed around the list body: outer border (2) + key bar (1) +
        // summary bar (3) + list border (2) + table header (1).
        const SESSIONS_LIST_CHROME_ROWS: usize = 9;
        (self.last_size.height as usize)
            .saturating_sub(SESSIONS_LIST_CHROME_ROWS)
            .max(1)
    }

    fn selected_session_from_visible<'a>(
        &self,
        visible: &SessionRowsView<'a>,
    ) -> Option<&'a crate::session_manager::SessionMeta> {
        match self.sessions.pane {
            SessionsPane::List => visible.get(self.sessions.selected_idx),
            SessionsPane::Detail => {
                let selected = visible.get(self.sessions.selected_idx);
                let Some(key) = self.sessions.detail_key.as_deref() else {
                    return selected;
                };
                if selected.is_some_and(|session| session_key_matches(session, key)) {
                    selected
                } else {
                    visible
                        .iter()
                        .find(|session| session_key_matches(session, key))
                        .or(selected)
                }
            }
        }
    }
}

fn bounded_session_message_overlay_lines(content: &str) -> Vec<String> {
    const MAX_LINES: usize = 512;

    let byte_limit = crate::session_manager::SESSION_MESSAGE_PREVIEW_MAX_MESSAGE_BYTES;
    let mut end = content.len().min(byte_limit);
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }
    let bounded = &content[..end];
    let mut lines = Vec::with_capacity(32);
    let mut source = bounded.lines();
    for _ in 0..MAX_LINES {
        let Some(line) = source.next() else {
            break;
        };
        lines.push(line.to_string());
    }
    if content.len() > end || source.next().is_some() {
        lines.push("…".to_string());
    }
    lines
}

fn session_title(session: &crate::session_manager::SessionMeta) -> String {
    session
        .title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            session.project_dir.as_deref().and_then(|path| {
                std::path::Path::new(path.trim().trim_end_matches(['/', '\\']))
                    .file_name()
                    .and_then(|value| value.to_str())
                    .map(str::to_string)
            })
        })
        .unwrap_or_else(|| session.session_id.chars().take(8).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn provider_row(id: &str) -> super::data::ProviderRow {
        super::data::ProviderRow {
            id: id.to_string(),
            provider: crate::provider::Provider::with_id(
                id.to_string(),
                "Provider One".to_string(),
                json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: None,
            default_model_id: None,
        }
    }

    #[test]
    fn claude_provider_list_o_key_requests_temporary_launch() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(provider_row("p1"));

        let action = app.on_key(key(KeyCode::Char('o')), &data);
        assert!(matches!(
            action,
            Action::ProviderLaunchTemporary { id } if id == "p1"
        ));
    }

    #[test]
    fn codex_provider_o_key_requests_temporary_launch() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(provider_row("p1"));

        let action = app.on_key(key(KeyCode::Char('o')), &data);
        assert!(matches!(
            action,
            Action::ProviderLaunchTemporary { id } if id == "p1"
        ));
    }

    #[cfg(not(unix))]
    #[test]
    fn claude_provider_o_key_is_noop_on_non_unix() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(provider_row("p1"));

        let action = app.on_key(key(KeyCode::Char('o')), &data);
        assert!(matches!(action, Action::None));
    }

    #[cfg(not(unix))]
    #[test]
    fn codex_provider_o_key_is_noop_on_non_unix() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(provider_row("p1"));

        let action = app.on_key(key(KeyCode::Char('o')), &data);
        assert!(matches!(action, Action::None));
    }

    #[test]
    fn non_claude_provider_o_key_is_noop() {
        let mut app = App::new(Some(AppType::Gemini));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(provider_row("p1"));

        let action = app.on_key(key(KeyCode::Char('o')), &data);
        assert!(matches!(action, Action::None));
    }
}
