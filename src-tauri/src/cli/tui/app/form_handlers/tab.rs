use super::*;

impl App {
    pub(super) fn handle_form_tab_key(&mut self, key: KeyEvent) -> bool {
        let is_backtab = matches!(key.code, KeyCode::BackTab)
            || (matches!(key.code, KeyCode::Tab) && key.modifiers.contains(KeyModifiers::SHIFT));
        let is_tab = matches!(key.code, KeyCode::Tab) && !is_backtab;
        if !is_tab && !is_backtab {
            return false;
        }

        let Some(form) = self.form.as_mut() else {
            return false;
        };

        match form {
            FormState::ProviderAdd(provider) => {
                if matches!(provider.page, form::ProviderFormPage::CodexLocalRouting) {
                    if is_backtab {
                        return false;
                    }
                    provider.focus = FormFocus::Fields;
                    return true;
                }
                if matches!(provider.page, form::ProviderFormPage::CodexModelCatalog) {
                    if is_backtab {
                        return false;
                    }
                    provider.focus = FormFocus::Fields;
                    return true;
                }
                if matches!(provider.page, form::ProviderFormPage::UsageQuery) {
                    if is_backtab {
                        return false;
                    }
                    finish_usage_query_tab_editing(provider);
                    if !provider.usage_query_extractor_available() {
                        provider.focus = FormFocus::Fields;
                        return true;
                    }
                    provider.focus = match provider.focus {
                        FormFocus::Fields => FormFocus::JsonPreview,
                        FormFocus::JsonPreview => FormFocus::Content,
                        FormFocus::Content => FormFocus::Fields,
                        FormFocus::Templates => FormFocus::Fields,
                    };
                    return true;
                }
                if is_backtab {
                    return false;
                }
                if matches!(provider.app_type, AppType::Codex) {
                    match (
                        &provider.mode,
                        provider.focus,
                        provider.codex_preview_section,
                    ) {
                        (FormMode::Add, FormFocus::Templates, _) => {
                            provider.focus = FormFocus::Fields;
                        }
                        (FormMode::Add, FormFocus::Fields, _) => {
                            provider.focus = FormFocus::JsonPreview;
                            provider.codex_preview_section = form::CodexPreviewSection::Auth;
                        }
                        (
                            FormMode::Add,
                            FormFocus::JsonPreview,
                            form::CodexPreviewSection::Auth,
                        ) => {
                            provider.focus = FormFocus::JsonPreview;
                            provider.codex_preview_section = form::CodexPreviewSection::Config;
                        }
                        (
                            FormMode::Add,
                            FormFocus::JsonPreview,
                            form::CodexPreviewSection::Config,
                        ) => {
                            provider.focus = FormFocus::Templates;
                        }
                        (FormMode::Edit { .. }, FormFocus::Fields, _) => {
                            provider.focus = FormFocus::JsonPreview;
                            provider.codex_preview_section = form::CodexPreviewSection::Auth;
                        }
                        (
                            FormMode::Edit { .. },
                            FormFocus::JsonPreview,
                            form::CodexPreviewSection::Auth,
                        ) => {
                            provider.focus = FormFocus::JsonPreview;
                            provider.codex_preview_section = form::CodexPreviewSection::Config;
                        }
                        (
                            FormMode::Edit { .. },
                            FormFocus::JsonPreview,
                            form::CodexPreviewSection::Config,
                        ) => {
                            provider.focus = FormFocus::Fields;
                        }
                        (FormMode::Edit { .. }, FormFocus::Templates, _) => {
                            provider.focus = FormFocus::Fields;
                        }
                        (_, FormFocus::Content, _) => {
                            provider.focus = FormFocus::Fields;
                        }
                    }
                } else {
                    provider.focus = match (&provider.mode, provider.focus) {
                        (FormMode::Add, FormFocus::Templates) => FormFocus::Fields,
                        (FormMode::Add, FormFocus::Fields) => FormFocus::JsonPreview,
                        (FormMode::Add, FormFocus::JsonPreview) => FormFocus::Templates,
                        (FormMode::Add, FormFocus::Content) => FormFocus::Fields,
                        (FormMode::Edit { .. }, FormFocus::Fields) => FormFocus::JsonPreview,
                        (FormMode::Edit { .. }, FormFocus::JsonPreview) => FormFocus::Fields,
                        (FormMode::Edit { .. }, FormFocus::Templates) => FormFocus::Fields,
                        (FormMode::Edit { .. }, FormFocus::Content) => FormFocus::Fields,
                    };
                }
            }
            FormState::McpAdd(mcp) => {
                if is_backtab {
                    return false;
                }
                mcp.focus = match (&mcp.mode, mcp.focus) {
                    (FormMode::Add, FormFocus::Templates) => FormFocus::Fields,
                    (FormMode::Add, FormFocus::Fields) => FormFocus::JsonPreview,
                    (FormMode::Add, FormFocus::JsonPreview) => FormFocus::Templates,
                    (FormMode::Add, FormFocus::Content) => FormFocus::Fields,
                    (FormMode::Edit { .. }, FormFocus::Fields) => FormFocus::JsonPreview,
                    (FormMode::Edit { .. }, FormFocus::JsonPreview) => FormFocus::Fields,
                    (FormMode::Edit { .. }, FormFocus::Templates) => FormFocus::Fields,
                    (FormMode::Edit { .. }, FormFocus::Content) => FormFocus::Fields,
                };
            }
            FormState::PromptMeta(prompt) => {
                if is_backtab {
                    prompt.editing = false;
                    prompt.focus = FormFocus::Fields;
                    return true;
                }
                if is_tab && matches!(prompt.focus, FormFocus::Content) {
                    return false;
                }
                prompt.editing = false;
                prompt.focus = match prompt.focus {
                    FormFocus::Fields => FormFocus::Content,
                    FormFocus::Content => FormFocus::Fields,
                    FormFocus::Templates | FormFocus::JsonPreview => FormFocus::Fields,
                };
            }
        }

        true
    }
}

fn finish_usage_query_tab_editing(provider: &mut form::ProviderAddFormState) {
    if !provider.usage_query_editing {
        return;
    }

    if matches!(
        provider.selected_usage_query_field(),
        Some(form::UsageQueryField::Timeout | form::UsageQueryField::AutoInterval)
    ) {
        let timeout = form::normalize_usage_timeout(&provider.usage_query_timeout.value);
        provider.usage_query_timeout.set(timeout.to_string());

        let interval = form::normalize_usage_interval(&provider.usage_query_auto_interval.value);
        provider.usage_query_auto_interval.set(interval.to_string());
    }

    provider.usage_query_editing = false;
}
