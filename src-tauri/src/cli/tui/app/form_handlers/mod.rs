use super::*;

mod mcp;
mod prompt;
mod provider;
mod tab;

impl App {
    pub(crate) fn on_form_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        if is_save_shortcut(key) {
            return self.handle_form_save_shortcut(data);
        }

        if self.handle_form_tab_key(key) {
            return Action::None;
        }

        if let Some(action) = self.handle_provider_template_key(key, data) {
            return action;
        }

        if let Some(action) = self.handle_mcp_template_key(key) {
            return action;
        }

        if let Some(action) = self.handle_provider_focus_key(key, data) {
            return action;
        }

        if let Some(action) = self.handle_mcp_focus_key(key) {
            return action;
        }

        if let Some(action) = self.handle_prompt_meta_focus_key(key) {
            return action;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.handle_form_exit_key(),
            _ => Action::None,
        }
    }

    fn handle_form_exit_key(&mut self) -> Action {
        if let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() {
            if matches!(provider.page, form::ProviderFormPage::CodexModelCatalog) {
                provider.close_codex_model_catalog_page();
                return Action::None;
            }
            if matches!(provider.page, form::ProviderFormPage::CodexLocalRouting) {
                provider.close_codex_local_routing_page();
                return Action::None;
            }
            if matches!(provider.page, form::ProviderFormPage::UsageQuery) {
                provider.close_usage_query_page();
                return Action::None;
            }
        }

        let has_unsaved_changes = self
            .form
            .as_ref()
            .is_some_and(FormState::has_unsaved_changes);
        if has_unsaved_changes {
            self.overlay = Overlay::Confirm(ConfirmOverlay {
                title: texts::tui_editor_save_before_close_title().to_string(),
                message: texts::tui_editor_save_before_close_message().to_string(),
                action: ConfirmAction::FormSaveBeforeClose,
            });
            return Action::None;
        }

        self.form = None;
        Action::None
    }

    pub(super) fn handle_form_save_shortcut(&mut self, data: &UiData) -> Action {
        match self.form.as_ref() {
            Some(FormState::ProviderAdd(_)) => self.build_provider_form_save_action(data),
            Some(FormState::McpAdd(_)) => self.build_mcp_form_save_action(),
            Some(FormState::PromptMeta(_)) => self.build_prompt_meta_form_save_action(),
            None => Action::None,
        }
    }
}
