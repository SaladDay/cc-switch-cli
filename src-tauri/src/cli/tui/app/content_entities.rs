use super::*;

impl App {
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
        let visible = visible_providers(&self.app_type, &self.filter, data);
        match key.code {
            KeyCode::Up => {
                self.provider_idx = self.provider_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                if !visible.is_empty() {
                    self.provider_idx = (self.provider_idx + 1).min(visible.len() - 1);
                }
                Action::None
            }
            KeyCode::Enter => {
                if data.providers.rows.is_empty() {
                    return Action::ProviderImportLiveConfig;
                }
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                self.push_route_and_switch(Route::ProviderDetail { id: row.id.clone() })
            }
            KeyCode::Char('a') => {
                self.open_provider_add_form(data);
                Action::None
            }
            KeyCode::Char('e') => {
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
            KeyCode::Char('s') | KeyCode::Char(' ') => {
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                self.provider_switch_action(row)
            }
            KeyCode::Char('x') => {
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                self.provider_set_default_action(row)
            }
            KeyCode::Char('d') => {
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
            KeyCode::Char('t') => {
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                self.open_provider_test_menu(row);
                Action::None
            }
            KeyCode::Char('o') => {
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                if !supports_temporary_provider_launch(&self.app_type) {
                    return Action::None;
                }
                Action::ProviderLaunchTemporary { id: row.id.clone() }
            }
            KeyCode::Char('f') => {
                if !supports_failover_controls(&self.app_type) {
                    return Action::None;
                }
                let selected = visible
                    .get(self.provider_idx)
                    .and_then(|row| {
                        failover_queue_rows(data)
                            .iter()
                            .position(|provider_row| provider_row.id == row.id)
                    })
                    .unwrap_or(0);
                self.overlay = Overlay::FailoverQueueManager { selected };
                Action::None
            }
            KeyCode::Char('<') | KeyCode::Char('>') => Action::None,
            KeyCode::Char('r') => {
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                if data::quota_target_for_provider(&self.app_type, row).is_none() {
                    self.push_toast(texts::tui_toast_quota_not_available(), ToastKind::Info);
                    return Action::None;
                }
                Action::ProviderQuotaRefresh { id: row.id.clone() }
            }
            _ => Action::None,
        }
    }

    pub(crate) fn on_provider_detail_key(
        &mut self,
        key: KeyEvent,
        data: &UiData,
        id: &str,
    ) -> Action {
        let Some(row) = data.providers.rows.iter().find(|p| p.id == id) else {
            return Action::None;
        };

        match key.code {
            KeyCode::Char('e') => {
                if self.is_provider_read_only(row) {
                    self.show_provider_read_only_toast();
                    return Action::None;
                }
                self.open_provider_edit_form(row, data);
                Action::None
            }
            KeyCode::Enter => Action::None,
            KeyCode::Char('s') | KeyCode::Char(' ') => self.provider_switch_action(row),
            KeyCode::Char('x') => self.provider_set_default_action(row),
            KeyCode::Char('t') => {
                self.open_provider_test_menu(row);
                Action::None
            }
            KeyCode::Char('o') => {
                if !supports_temporary_provider_launch(&self.app_type) {
                    return Action::None;
                }
                Action::ProviderLaunchTemporary { id: row.id.clone() }
            }
            KeyCode::Char('f') => {
                if !supports_failover_controls(&self.app_type) {
                    return Action::None;
                }
                let selected = failover_queue_rows(data)
                    .iter()
                    .position(|provider_row| provider_row.id == row.id)
                    .unwrap_or(0);
                self.overlay = Overlay::FailoverQueueManager { selected };
                Action::None
            }
            KeyCode::Char('<') | KeyCode::Char('>') => Action::None,
            KeyCode::Char('r') => {
                if data::quota_target_for_provider(&self.app_type, row).is_none() {
                    self.push_toast(texts::tui_toast_quota_not_available(), ToastKind::Info);
                    return Action::None;
                }
                Action::ProviderQuotaRefresh { id: row.id.clone() }
            }
            _ => Action::None,
        }
    }

    pub(crate) fn on_mcp_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        let visible = visible_mcp(&self.filter, data);
        match key.code {
            KeyCode::Up => {
                self.mcp_idx = self.mcp_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                if !visible.is_empty() {
                    self.mcp_idx = (self.mcp_idx + 1).min(visible.len() - 1);
                }
                Action::None
            }
            KeyCode::Char('a') => {
                self.open_mcp_add_form();
                Action::None
            }
            KeyCode::Char('e') => {
                let Some(row) = visible.get(self.mcp_idx) else {
                    return Action::None;
                };
                self.open_mcp_edit_form(row);
                Action::None
            }
            KeyCode::Char(' ') => {
                let Some(row) = visible.get(self.mcp_idx) else {
                    return Action::None;
                };
                let enabled = row.server.apps.is_enabled_for(&self.app_type);
                Action::McpToggle {
                    id: row.id.clone(),
                    enabled: !enabled,
                }
            }
            KeyCode::Char('m') => {
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
            KeyCode::Char('i') => Action::McpImport,
            KeyCode::Char('d') => {
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
            _ => Action::None,
        }
    }

    pub(crate) fn on_prompts_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        let visible = visible_prompts(&self.filter, data);
        match key.code {
            KeyCode::Char('a') => {
                self.open_prompt_create_form(data);
                Action::None
            }
            KeyCode::Up => {
                self.prompt_idx = self.prompt_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                if !visible.is_empty() {
                    self.prompt_idx = (self.prompt_idx + 1).min(visible.len() - 1);
                }
                Action::None
            }
            KeyCode::Enter => {
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
            KeyCode::Char(' ') => {
                let Some(row) = visible.get(self.prompt_idx) else {
                    return Action::None;
                };
                if row.prompt.enabled {
                    Action::PromptDeactivate { id: row.id.clone() }
                } else {
                    Action::PromptActivate { id: row.id.clone() }
                }
            }
            KeyCode::Char('d') => {
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
            KeyCode::Char('e') => {
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
            _ => Action::None,
        }
    }

    pub(crate) fn on_sessions_key(&mut self, key: KeyEvent) -> Action {
        let visible = visible_sessions(&self.filter, &self.app_type, &self.sessions.rows);
        match key.code {
            KeyCode::Left => {
                self.sessions.pane = match self.sessions.pane {
                    SessionsPane::List => SessionsPane::List,
                    SessionsPane::Detail => SessionsPane::List,
                };
                Action::None
            }
            KeyCode::Right => {
                self.sessions.pane = match self.sessions.pane {
                    SessionsPane::List => SessionsPane::Detail,
                    SessionsPane::Detail => SessionsPane::Detail,
                };
                Action::None
            }
            KeyCode::Up => {
                match self.sessions.pane {
                    SessionsPane::List => {
                        self.sessions.selected_idx = self.sessions.selected_idx.saturating_sub(1);
                    }
                    SessionsPane::Detail => {
                        self.sessions.message_idx = self.sessions.message_idx.saturating_sub(1);
                    }
                }
                Action::None
            }
            KeyCode::Down => {
                match self.sessions.pane {
                    SessionsPane::List => {
                        if !visible.is_empty() {
                            self.sessions.selected_idx =
                                (self.sessions.selected_idx + 1).min(visible.len() - 1);
                        }
                    }
                    SessionsPane::Detail => {
                        if !self.sessions.messages.is_empty() {
                            self.sessions.message_idx = (self.sessions.message_idx + 1)
                                .min(self.sessions.messages.len() - 1);
                        }
                    }
                }
                Action::None
            }
            KeyCode::Enter => match self.sessions.pane {
                SessionsPane::List => {
                    let Some(session) = visible.get(self.sessions.selected_idx) else {
                        return Action::None;
                    };
                    let key = session_key(session);
                    let provider_id = session.provider_id.clone();
                    let source_path = session.source_path.clone();
                    self.sessions.open_detail(key.clone());
                    self.sessions.pane = SessionsPane::Detail;
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
                SessionsPane::Detail => {
                    let Some(message) = self.sessions.messages.get(self.sessions.message_idx)
                    else {
                        return Action::None;
                    };
                    self.overlay = Overlay::TextView(TextViewState {
                        title: texts::tui_sessions_message_detail_title(&message.role),
                        lines: message
                            .content
                            .lines()
                            .map(|line| line.to_string())
                            .collect(),
                        scroll: 0,
                        action: None,
                    });
                    Action::None
                }
            },
            KeyCode::Char('R') => {
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
            KeyCode::Char('d') => {
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
                    message: texts::tui_sessions_delete_confirm_message(&session_title(session)),
                    action: ConfirmAction::SessionDelete {
                        key,
                        provider_id: session.provider_id.clone(),
                        session_id: session.session_id.clone(),
                        source_path,
                    },
                });
                Action::None
            }
            KeyCode::Char('r') => Action::SessionsRefresh,
            _ => Action::None,
        }
    }

    fn selected_session_from_visible<'a>(
        &self,
        visible: &'a [&'a crate::session_manager::SessionMeta],
    ) -> Option<&'a crate::session_manager::SessionMeta> {
        match self.sessions.pane {
            SessionsPane::List => visible.get(self.sessions.selected_idx).copied(),
            SessionsPane::Detail => self
                .sessions
                .detail_key
                .as_deref()
                .and_then(|key| {
                    visible
                        .iter()
                        .copied()
                        .find(|session| session_key(session) == key)
                })
                .or_else(|| visible.get(self.sessions.selected_idx).copied()),
        }
    }
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
    fn claude_provider_detail_o_key_requests_temporary_launch() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::ProviderDetail {
            id: "p1".to_string(),
        };
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

    #[test]
    fn codex_provider_detail_o_key_requests_temporary_launch() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::ProviderDetail {
            id: "p1".to_string(),
        };
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

    #[test]
    fn non_claude_provider_detail_o_key_is_noop() {
        let mut app = App::new(Some(AppType::Gemini));
        app.route = Route::ProviderDetail {
            id: "p1".to_string(),
        };
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(provider_row("p1"));

        let action = app.on_key(key(KeyCode::Char('o')), &data);
        assert!(matches!(action, Action::None));
    }
}
