use super::*;

mod mcp;
mod prompt;
mod provider;
mod s3;
mod tab;
mod webdav;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveTextField {
    Provider(form::ProviderTextField),
    Mcp(form::McpAddField),
    Prompt(form::PromptMetaField),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveTextCommit {
    field: ActiveTextField,
    valid: bool,
}

impl App {
    pub(crate) fn on_form_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        if matches!(self.form, Some(FormState::S3Sync(_))) {
            return self.on_s3_sync_form_key(key);
        }
        if matches!(self.form, Some(FormState::WebDavSync(_))) {
            return self.on_webdav_sync_form_key(key);
        }

        if is_save_shortcut(key) {
            self.commit_active_text_edit(data);
            return self.handle_form_save_shortcut(data);
        }

        if self.handle_form_tab_key(key, data) {
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

        if let Some(action) = self.handle_mcp_focus_key(key, data) {
            return action;
        }

        if let Some(action) = self.handle_prompt_meta_focus_key(key, data) {
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
            if matches!(provider.page, form::ProviderFormPage::LocalProxySettings) {
                provider.close_local_proxy_settings_page();
                return Action::None;
            }
            if matches!(provider.page, form::ProviderFormPage::UsageQuery) {
                provider.close_usage_query_page();
                return Action::None;
            }
            if matches!(provider.page, form::ProviderFormPage::ClaudeQuickConfig) {
                provider.close_claude_quick_config_page();
                return Action::None;
            }
            if matches!(provider.page, form::ProviderFormPage::CodexQuickConfig) {
                provider.close_codex_quick_config_page();
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
            Some(FormState::McpAdd(_)) => self.build_mcp_form_save_action(data),
            Some(FormState::PromptMeta(_)) => self.build_prompt_meta_form_save_action(),
            Some(FormState::S3Sync(_)) => self.build_s3_sync_form_save_action(),
            Some(FormState::WebDavSync(_)) => self.build_webdav_sync_form_save_action(),
            None => Action::None,
        }
    }

    fn commit_active_text_edit(&mut self, data: &UiData) -> Option<ActiveTextCommit> {
        let provider_validation = match self.form.as_ref() {
            Some(FormState::ProviderAdd(provider)) => {
                provider.text_edit.as_ref().and_then(|edit| {
                    let form::ProviderTextField::Main(field) = edit.target() else {
                        return None;
                    };
                    provider::validate_provider_inline_field(provider, field)
                        .map(|message| (field, message))
                })
            }
            _ => None,
        };
        if let Some((field, message)) = provider_validation {
            if let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() {
                provider.set_main_field_error(field, message);
            }
            return Some(ActiveTextCommit {
                field: ActiveTextField::Provider(form::ProviderTextField::Main(field)),
                valid: false,
            });
        }

        let provider_commit = match self.form.as_mut() {
            Some(FormState::ProviderAdd(provider)) => provider.take_text_edit(),
            _ => None,
        };
        if let Some(session) = provider_commit {
            let (target, original, _original_error) = session.into_parts();
            match target {
                form::ProviderTextField::Main(field) => {
                    let changed = self.form.as_ref().is_some_and(|form| {
                        matches!(form, FormState::ProviderAdd(provider)
                            if provider.input(field)
                                .is_some_and(|input| input.value != original.value))
                    });
                    self.finish_provider_input_change(field, changed, data);
                    if let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() {
                        provider.sync_main_field_index(field);
                    }
                }
                form::ProviderTextField::UsageQuery(field) => {
                    if let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() {
                        if matches!(
                            field,
                            form::UsageQueryField::Timeout | form::UsageQueryField::AutoInterval
                        ) {
                            normalize_usage_query_numeric_field(provider, field);
                        }
                        let changed = provider
                            .usage_query_input(field)
                            .is_some_and(|input| input.value != original.value);
                        if changed {
                            provider.touch_usage_query();
                        }
                        provider.sync_usage_query_field_index(field);
                    }
                }
            }
            return Some(ActiveTextCommit {
                field: ActiveTextField::Provider(target),
                valid: true,
            });
        }

        let mcp_validation = match self.form.as_ref() {
            Some(FormState::McpAdd(mcp)) => mcp.text_edit_target().and_then(|field| {
                validate_mcp_inline_field(mcp, field).map(|message| (field, message))
            }),
            _ => None,
        };
        if let Some((field, message)) = mcp_validation {
            if let Some(FormState::McpAdd(mcp)) = self.form.as_mut() {
                mcp.set_field_error(field, message);
            }
            return Some(ActiveTextCommit {
                field: ActiveTextField::Mcp(field),
                valid: false,
            });
        }

        let mcp_commit = match self.form.as_mut() {
            Some(FormState::McpAdd(mcp)) => mcp.take_text_edit(),
            _ => None,
        };
        if let Some(session) = mcp_commit {
            let (field, _original, _original_error) = session.into_parts();
            let mut valid = true;
            if let Some(FormState::McpAdd(mcp)) = self.form.as_mut() {
                valid = field != form::McpAddField::Args || mcp.commit_args_input();
                if valid {
                    mcp.clear_field_error(field);
                } else {
                    mcp.set_field_error(field, texts::tui_mcp_args_invalid());
                }
            }
            return Some(ActiveTextCommit {
                field: ActiveTextField::Mcp(field),
                valid,
            });
        }

        let prompt_validation = match self.form.as_ref() {
            Some(FormState::PromptMeta(prompt)) => prompt.text_edit_target().and_then(|field| {
                validate_prompt_inline_field(prompt, field).map(|message| (field, message))
            }),
            _ => None,
        };
        if let Some((field, message)) = prompt_validation {
            if let Some(FormState::PromptMeta(prompt)) = self.form.as_mut() {
                prompt.set_field_error(field, message);
            }
            return Some(ActiveTextCommit {
                field: ActiveTextField::Prompt(field),
                valid: false,
            });
        }

        let prompt_commit = match self.form.as_mut() {
            Some(FormState::PromptMeta(prompt)) => prompt.take_text_edit(),
            _ => None,
        };
        if let Some(session) = prompt_commit {
            let (field, _original, _original_error) = session.into_parts();
            if let Some(FormState::PromptMeta(prompt)) = self.form.as_mut() {
                prompt.clear_field_error(field);
            }
            return Some(ActiveTextCommit {
                field: ActiveTextField::Prompt(field),
                valid: true,
            });
        }

        None
    }
}

fn validate_mcp_inline_field(
    mcp: &form::McpAddFormState,
    field: form::McpAddField,
) -> Option<&'static str> {
    match field {
        form::McpAddField::Id if mcp.id.is_blank() => Some(texts::tui_toast_mcp_missing_fields()),
        form::McpAddField::Name if mcp.name.is_blank() => {
            Some(texts::tui_toast_mcp_missing_fields())
        }
        form::McpAddField::Url if mcp.server_type.is_remote() && mcp.url.is_blank() => {
            Some(texts::tui_toast_url_empty())
        }
        form::McpAddField::Command if !mcp.server_type.is_remote() && mcp.command.is_blank() => {
            Some(texts::tui_toast_command_empty())
        }
        form::McpAddField::Args if !mcp.args_input_is_valid() => {
            Some(texts::tui_mcp_args_invalid())
        }
        _ => None,
    }
}

fn validate_prompt_inline_field(
    prompt: &form::PromptMetaFormState,
    field: form::PromptMetaField,
) -> Option<String> {
    match field {
        form::PromptMetaField::Id => {
            crate::services::PromptService::validate_prompt_id(&prompt.id_value())
                .err()
                .map(|error| error.to_string())
        }
        form::PromptMetaField::Name if prompt.name_value().is_empty() => {
            Some(texts::tui_toast_prompt_name_empty().to_string())
        }
        _ => None,
    }
}

fn normalize_usage_query_numeric_field(
    provider: &mut form::ProviderAddFormState,
    field: form::UsageQueryField,
) {
    match field {
        form::UsageQueryField::Timeout => {
            let value = form::normalize_usage_timeout(&provider.usage_query_timeout.value);
            provider.usage_query_timeout.set(value.to_string());
        }
        form::UsageQueryField::AutoInterval => {
            let value = form::normalize_usage_interval(&provider.usage_query_auto_interval.value);
            provider.usage_query_auto_interval.set(value.to_string());
        }
        _ => {}
    }
}
