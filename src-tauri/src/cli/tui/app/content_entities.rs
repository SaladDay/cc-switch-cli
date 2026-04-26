use super::*;

impl App {
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
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                self.push_route_and_switch(Route::ProviderDetail { id: row.id.clone() })
            }
            KeyCode::Char('a') => {
                self.open_provider_add_form();
                Action::None
            }
            KeyCode::Char('e') => {
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                self.open_provider_edit_form(row);
                Action::None
            }
            KeyCode::Char('s') => {
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                if matches!(self.app_type, AppType::OpenCode) {
                    if row.is_in_config {
                        return Action::ProviderRemoveFromConfig { id: row.id.clone() };
                    }

                    return Action::ProviderSwitch { id: row.id.clone() };
                }
                if matches!(self.app_type, AppType::OpenClaw) {
                    if row.is_in_config {
                        if row.is_default_model {
                            self.push_toast(
                                texts::tui_toast_provider_cannot_remove_default_model(),
                                ToastKind::Warning,
                            );
                            return Action::None;
                        }
                        return Action::ProviderRemoveFromConfig { id: row.id.clone() };
                    }

                    return Action::ProviderSwitch { id: row.id.clone() };
                }
                if row.is_current {
                    self.push_toast(texts::tui_toast_provider_already_in_use(), ToastKind::Info);
                    return Action::None;
                }
                Action::ProviderSwitch { id: row.id.clone() }
            }
            KeyCode::Char('x') => {
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                if !matches!(self.app_type, AppType::OpenClaw) {
                    return Action::None;
                }
                if !row.is_in_config {
                    self.push_toast(
                        texts::tui_toast_provider_default_requires_live_config(),
                        ToastKind::Warning,
                    );
                    return Action::None;
                }
                let Some(model_id) = row.primary_model_id.clone() else {
                    self.push_toast(
                        texts::tui_toast_provider_default_model_missing(),
                        ToastKind::Warning,
                    );
                    return Action::None;
                };
                Action::ProviderSetDefaultModel {
                    provider_id: row.id.clone(),
                    model_id,
                }
            }
            KeyCode::Char('d') => {
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                if row.is_current {
                    self.push_toast(
                        texts::tui_toast_provider_cannot_delete_current(),
                        ToastKind::Warning,
                    );
                    return Action::None;
                }
                self.overlay = Overlay::Confirm(ConfirmOverlay {
                    title: texts::tui_confirm_delete_provider_title().to_string(),
                    message: texts::tui_confirm_delete_provider_message(
                        &super::data::provider_display_name(&self.app_type, row),
                        &row.id,
                    ),
                    action: ConfirmAction::ProviderDelete { id: row.id.clone() },
                });
                Action::None
            }
            KeyCode::Char('t') => {
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                let Some(url) = row.api_url.clone() else {
                    self.push_toast(texts::tui_toast_provider_no_api_url(), ToastKind::Warning);
                    return Action::None;
                };
                self.overlay = Overlay::SpeedtestRunning { url: url.clone() };
                Action::ProviderSpeedtest { url }
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
            KeyCode::Char('c') => {
                if !supports_provider_stream_check(&self.app_type) {
                    return Action::None;
                }
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                self.overlay = Overlay::StreamCheckRunning {
                    provider_id: row.id.clone(),
                    provider_name: super::data::provider_display_name(&self.app_type, row),
                };
                Action::ProviderStreamCheck { id: row.id.clone() }
            }
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
                self.open_provider_edit_form(row);
                Action::None
            }
            KeyCode::Enter => Action::None,
            KeyCode::Char('s') => {
                if matches!(self.app_type, AppType::OpenCode) {
                    if row.is_in_config {
                        return Action::ProviderRemoveFromConfig { id: row.id.clone() };
                    }

                    return Action::ProviderSwitch { id: row.id.clone() };
                }
                if matches!(self.app_type, AppType::OpenClaw) {
                    if row.is_in_config {
                        if row.is_default_model {
                            self.push_toast(
                                texts::tui_toast_provider_cannot_remove_default_model(),
                                ToastKind::Warning,
                            );
                            return Action::None;
                        }
                        return Action::ProviderRemoveFromConfig { id: row.id.clone() };
                    }

                    return Action::ProviderSwitch { id: row.id.clone() };
                }
                if row.is_current {
                    self.push_toast(texts::tui_toast_provider_already_in_use(), ToastKind::Info);
                    return Action::None;
                }
                Action::ProviderSwitch { id: row.id.clone() }
            }
            KeyCode::Char('x') => {
                if !matches!(self.app_type, AppType::OpenClaw) {
                    return Action::None;
                }
                if !row.is_in_config {
                    self.push_toast(
                        texts::tui_toast_provider_default_requires_live_config(),
                        ToastKind::Warning,
                    );
                    return Action::None;
                }
                let Some(model_id) = row.primary_model_id.clone() else {
                    self.push_toast(
                        texts::tui_toast_provider_default_model_missing(),
                        ToastKind::Warning,
                    );
                    return Action::None;
                };
                Action::ProviderSetDefaultModel {
                    provider_id: row.id.clone(),
                    model_id,
                }
            }
            KeyCode::Char('t') => {
                let Some(url) = row.api_url.clone() else {
                    self.push_toast(texts::tui_toast_provider_no_api_url(), ToastKind::Warning);
                    return Action::None;
                };
                self.overlay = Overlay::SpeedtestRunning { url: url.clone() };
                Action::ProviderSpeedtest { url }
            }
            KeyCode::Char('o') => {
                if !supports_temporary_provider_launch(&self.app_type) {
                    return Action::None;
                }
                Action::ProviderLaunchTemporary { id: row.id.clone() }
            }
            KeyCode::Char('c') => {
                if !supports_provider_stream_check(&self.app_type) {
                    return Action::None;
                }
                self.overlay = Overlay::StreamCheckRunning {
                    provider_id: row.id.clone(),
                    provider_name: super::data::provider_display_name(&self.app_type, row),
                };
                Action::ProviderStreamCheck { id: row.id.clone() }
            }
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
            KeyCode::Char('x') => {
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
            KeyCode::Char('a') => {
                let Some(row) = visible.get(self.prompt_idx) else {
                    return Action::None;
                };
                Action::PromptActivate { id: row.id.clone() }
            }
            KeyCode::Char('x') => {
                let active = data.prompts.rows.iter().find(|p| p.prompt.enabled);
                let Some(active) = active else {
                    self.push_toast(
                        texts::tui_toast_prompt_no_active_to_deactivate(),
                        ToastKind::Info,
                    );
                    return Action::None;
                };
                Action::PromptDeactivate {
                    id: active.id.clone(),
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
                self.open_editor(
                    texts::tui_prompt_title(&row.prompt.name),
                    EditorKind::Plain,
                    row.prompt.content.clone(),
                    EditorSubmit::PromptEdit { id: row.id.clone() },
                );
                Action::None
            }
            _ => Action::None,
        }
    }
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

    #[cfg(not(any(unix, windows)))]
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

    #[cfg(not(any(unix, windows)))]
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
