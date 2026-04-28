use serde_json::{json, Value};

use crate::app_config::{AppType, McpServer};
use crate::cli::i18n::texts;
use crate::cli::tui::form::strip_common_config_from_settings;
use crate::commands::workspace;
use crate::error::AppError;
use crate::openclaw_config::{
    set_agents_defaults, set_env_config, set_tools_config, OpenClawAgentsDefaults,
    OpenClawEnvConfig, OpenClawToolsConfig,
};
use crate::provider::Provider;
use crate::services::{McpService, PromptService, ProviderService};
use crate::settings::{set_webdav_sync_settings, WebDavSyncSettings};

use super::super::app::{EditorSubmit, Overlay, TextViewState, ToastKind};
use super::super::data::{load_state, UiData};
use super::super::form::FormState;
use super::helpers::{refresh_openclaw_workspace_data, run_external_editor_for_current_editor};
use super::RuntimeActionContext;

fn is_codex_official_provider(provider: &Provider) -> bool {
    provider
        .meta
        .as_ref()
        .and_then(|meta| meta.codex_official)
        .unwrap_or(false)
        || provider.category.as_deref() == Some("official")
        || provider.website_url.as_deref() == Some("https://chatgpt.com/codex")
        || provider.name.trim().eq_ignore_ascii_case("OpenAI Official")
}

fn validate_provider_submit(
    app_type: &AppType,
    provider: &Provider,
    is_edit: bool,
) -> Option<&'static str> {
    if provider.name.trim().is_empty() {
        return Some(if is_edit {
            texts::tui_toast_provider_missing_name()
        } else {
            texts::tui_toast_provider_add_missing_fields()
        });
    }

    if matches!(app_type, AppType::Codex) && !is_codex_official_provider(provider) {
        let parsed = crate::cli::tui::form::parse_codex_config_snippet(
            provider
                .settings_config
                .get("config")
                .and_then(Value::as_str)
                .unwrap_or(""),
        );
        if parsed
            .base_url
            .as_deref()
            .is_none_or(|base_url| base_url.trim().is_empty())
        {
            return Some(texts::base_url_empty_error());
        }
    }

    None
}

pub(super) fn open_external(ctx: &mut RuntimeActionContext<'_>) -> Result<(), AppError> {
    ctx.terminal.with_terminal_restored(|| {
        run_external_editor_for_current_editor(ctx.app, crate::cli::editor::open_external_editor)
    })
}

pub(super) fn submit(
    ctx: &mut RuntimeActionContext<'_>,
    submit: EditorSubmit,
    content: String,
) -> Result<(), AppError> {
    match submit {
        EditorSubmit::PromptEdit { id } => submit_prompt_edit(ctx, id, content),
        EditorSubmit::ProviderFormApplyJson => submit_provider_form_apply_json(ctx, content),
        EditorSubmit::ProviderFormApplyOpenClawModels => {
            submit_provider_form_apply_openclaw_models(ctx, content)
        }
        EditorSubmit::ProviderFormApplyCodexAuth => {
            submit_provider_form_apply_codex_auth(ctx, content)
        }
        EditorSubmit::ProviderFormApplyCodexConfigToml => {
            submit_provider_form_apply_codex_config_toml(ctx, content)
        }
        EditorSubmit::ProviderAdd => submit_provider_add(ctx, content),
        EditorSubmit::ProviderEdit { id } => submit_provider_edit(ctx, id, content),
        EditorSubmit::McpAdd => submit_mcp_add(ctx, content),
        EditorSubmit::McpEdit { id } => submit_mcp_edit(ctx, id, content),
        EditorSubmit::ConfigCommonSnippet { app_type } => {
            submit_config_common_snippet(ctx, app_type, content)
        }
        EditorSubmit::OpenClawWorkspaceFile { filename } => {
            submit_openclaw_workspace_file(ctx, filename, content)
        }
        EditorSubmit::OpenClawDailyMemoryFile { filename } => {
            submit_openclaw_daily_memory_file(ctx, filename, content)
        }
        EditorSubmit::ConfigOpenClawEnv => submit_openclaw_env(ctx, content),
        EditorSubmit::ConfigOpenClawTools => submit_openclaw_tools(ctx, content),
        EditorSubmit::ConfigOpenClawAgents => submit_openclaw_agents(ctx, content),
        EditorSubmit::ConfigWebDavSettings => submit_webdav_settings(ctx, content),
    }
}

fn submit_openclaw_workspace_file(
    ctx: &mut RuntimeActionContext<'_>,
    filename: String,
    content: String,
) -> Result<(), AppError> {
    workspace::write_workspace_file(filename.clone(), content).map_err(|err| {
        AppError::Message(texts::tui_openclaw_workspace_save_failed(&filename, &err))
    })?;
    ctx.app.editor = None;
    ctx.app.push_toast(
        texts::tui_openclaw_workspace_saved(&filename),
        ToastKind::Success,
    );
    refresh_openclaw_workspace_data(ctx.app, ctx.data).map_err(|err| {
        AppError::Message(texts::tui_openclaw_workspace_refresh_failed(
            &err.to_string(),
        ))
    })
}

fn submit_openclaw_daily_memory_file(
    ctx: &mut RuntimeActionContext<'_>,
    filename: String,
    content: String,
) -> Result<(), AppError> {
    workspace::write_daily_memory_file(filename.clone(), content).map_err(|err| {
        AppError::Message(texts::tui_openclaw_daily_memory_save_failed(
            &filename, &err,
        ))
    })?;
    ctx.app.editor = None;
    ctx.app.push_toast(
        texts::tui_openclaw_daily_memory_saved(&filename),
        ToastKind::Success,
    );
    refresh_openclaw_workspace_data(ctx.app, ctx.data).map_err(|err| {
        AppError::Message(texts::tui_openclaw_daily_memory_refresh_failed(
            &err.to_string(),
        ))
    })
}

fn submit_openclaw_env(
    ctx: &mut RuntimeActionContext<'_>,
    content: String,
) -> Result<(), AppError> {
    let env: OpenClawEnvConfig = match serde_json::from_str(&content) {
        Ok(env) => env,
        Err(err) => {
            ctx.app.push_toast(
                texts::tui_toast_invalid_json(&err.to_string()),
                ToastKind::Error,
            );
            return Ok(());
        }
    };

    set_env_config(&env)?;
    ctx.app.editor = None;
    ctx.app.push_toast(
        texts::tui_toast_openclaw_config_saved(texts::tui_config_item_openclaw_env()),
        ToastKind::Success,
    );
    *ctx.data = UiData::load(&ctx.app.app_type)?;
    Ok(())
}

fn submit_openclaw_tools(
    ctx: &mut RuntimeActionContext<'_>,
    content: String,
) -> Result<(), AppError> {
    let tools: OpenClawToolsConfig = match serde_json::from_str(&content) {
        Ok(tools) => tools,
        Err(err) => {
            ctx.app.push_toast(
                texts::tui_toast_invalid_json(&err.to_string()),
                ToastKind::Error,
            );
            return Ok(());
        }
    };

    if let Err(err) = set_tools_config(&tools) {
        ctx.app.push_toast(
            texts::tui_toast_openclaw_tools_save_failed_detail(&err.to_string()),
            ToastKind::Error,
        );
        log::warn!("failed to save OpenClaw tools config: {err}");
        return Ok(());
    }

    let previous_form = ctx.app.openclaw_tools_form.clone();
    ctx.app.editor = None;
    *ctx.data = UiData::load(&ctx.app.app_type)?;
    let mut form = crate::cli::tui::app::OpenClawToolsFormState::from_snapshot(
        ctx.data.config.openclaw_tools.as_ref(),
    );
    if let Some(previous_form) = previous_form.as_ref() {
        form.restore_position(previous_form);
    }
    ctx.app.openclaw_tools_form = Some(form);
    if ctx
        .app
        .toast
        .as_ref()
        .is_some_and(|toast| toast.kind == ToastKind::Error)
    {
        ctx.app.toast = None;
    }
    Ok(())
}

fn submit_openclaw_agents(
    ctx: &mut RuntimeActionContext<'_>,
    content: String,
) -> Result<(), AppError> {
    let defaults: OpenClawAgentsDefaults = match serde_json::from_str(&content) {
        Ok(defaults) => defaults,
        Err(err) => {
            ctx.app.push_toast(
                texts::tui_toast_invalid_json(&err.to_string()),
                ToastKind::Error,
            );
            return Ok(());
        }
    };

    if let Err(err) = set_agents_defaults(&defaults) {
        ctx.app.push_toast(
            texts::tui_toast_openclaw_agents_save_failed_detail(&err.to_string()),
            ToastKind::Error,
        );
        log::warn!("failed to save OpenClaw agents config: {err}");
        return Ok(());
    }

    let previous_form = ctx.app.openclaw_agents_form.clone();
    ctx.app.editor = None;
    *ctx.data = UiData::load(&ctx.app.app_type)?;
    let mut form = crate::cli::tui::app::OpenClawAgentsFormState::from_snapshot(
        ctx.data.config.openclaw_agents_defaults.as_ref(),
    );
    if let Some(previous_form) = previous_form.as_ref() {
        form.restore_position(previous_form);
    }
    ctx.app.openclaw_agents_form = Some(form);
    if ctx
        .app
        .toast
        .as_ref()
        .is_some_and(|toast| toast.kind == ToastKind::Error)
    {
        ctx.app.toast = None;
    }
    Ok(())
}

fn submit_prompt_edit(
    ctx: &mut RuntimeActionContext<'_>,
    id: String,
    content: String,
) -> Result<(), AppError> {
    let state = load_state()?;
    let prompts = PromptService::get_prompts(&state, ctx.app.app_type.clone())?;
    let Some(mut prompt) = prompts.get(&id).cloned() else {
        ctx.app
            .push_toast(texts::tui_toast_prompt_not_found(&id), ToastKind::Error);
        return Ok(());
    };

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    prompt.content = content;
    prompt.updated_at = Some(timestamp);

    if let Err(err) = PromptService::upsert_prompt(&state, ctx.app.app_type.clone(), &id, prompt) {
        ctx.app.push_toast(err.to_string(), ToastKind::Error);
        return Ok(());
    }

    ctx.app.editor = None;
    ctx.app
        .push_toast(texts::tui_toast_prompt_edit_finished(), ToastKind::Success);
    *ctx.data = UiData::load(&ctx.app.app_type)?;
    Ok(())
}

fn submit_provider_form_apply_json(
    ctx: &mut RuntimeActionContext<'_>,
    content: String,
) -> Result<(), AppError> {
    let mut settings_value: Value = match serde_json::from_str(&content) {
        Ok(value) => value,
        Err(e) => {
            ctx.app.push_toast(
                texts::tui_toast_invalid_json(&e.to_string()),
                ToastKind::Error,
            );
            return Ok(());
        }
    };

    if !settings_value.is_object() {
        ctx.app
            .push_toast(texts::tui_toast_json_must_be_object(), ToastKind::Error);
        return Ok(());
    }

    let provider_value = match ctx.app.form.as_ref() {
        Some(FormState::ProviderAdd(form)) => {
            if form.should_strip_common_config_from_applied_settings_json() {
                if let Err(err) = strip_common_config_from_settings(
                    &form.app_type,
                    &mut settings_value,
                    &ctx.data.config.common_snippet,
                ) {
                    ctx.app.push_toast(err, ToastKind::Error);
                    return Ok(());
                }
            }

            let mut provider_value = form.to_provider_json_value();
            if let Some(obj) = provider_value.as_object_mut() {
                obj.insert("settingsConfig".to_string(), settings_value.clone());
            }
            Some(provider_value)
        }
        _ => None,
    };

    if let Some(provider_value) = provider_value {
        let apply_result = match ctx.app.form.as_mut() {
            Some(FormState::ProviderAdd(form)) => {
                form.apply_provider_json_value_to_fields(provider_value)
            }
            _ => Ok(()),
        };

        if let Err(err) = apply_result {
            ctx.app.push_toast(err, ToastKind::Error);
            return Ok(());
        }
    }
    ctx.app.editor = None;
    Ok(())
}

fn submit_provider_form_apply_openclaw_models(
    ctx: &mut RuntimeActionContext<'_>,
    content: String,
) -> Result<(), AppError> {
    let models_value: Value = match serde_json::from_str(&content) {
        Ok(value) => value,
        Err(e) => {
            ctx.app.push_toast(
                texts::tui_toast_invalid_json(&e.to_string()),
                ToastKind::Error,
            );
            return Ok(());
        }
    };

    if !models_value.is_array() {
        ctx.app
            .push_toast(texts::tui_toast_json_must_be_array(), ToastKind::Error);
        return Ok(());
    }

    let apply_result = match ctx.app.form.as_mut() {
        Some(FormState::ProviderAdd(form)) => form.apply_openclaw_models_value(models_value),
        _ => Ok(()),
    };

    if let Err(err) = apply_result {
        ctx.app.push_toast(err, ToastKind::Error);
        return Ok(());
    }

    ctx.app.editor = None;
    Ok(())
}

fn submit_provider_form_apply_codex_auth(
    ctx: &mut RuntimeActionContext<'_>,
    content: String,
) -> Result<(), AppError> {
    let auth_value: Value = match serde_json::from_str(&content) {
        Ok(value) => value,
        Err(e) => {
            ctx.app.push_toast(
                texts::tui_toast_invalid_json(&e.to_string()),
                ToastKind::Error,
            );
            return Ok(());
        }
    };

    if !auth_value.is_object() {
        ctx.app
            .push_toast(texts::tui_toast_json_must_be_object(), ToastKind::Error);
        return Ok(());
    }

    let provider_value = match ctx.app.form.as_ref() {
        Some(FormState::ProviderAdd(form)) => {
            let mut provider_value = form.to_provider_json_value();
            if let Some(settings_value) = provider_value
                .as_object_mut()
                .and_then(|obj| obj.get_mut("settingsConfig"))
            {
                if !settings_value.is_object() {
                    *settings_value = json!({});
                }
                if let Some(settings_obj) = settings_value.as_object_mut() {
                    settings_obj.insert("auth".to_string(), auth_value);
                }
            }
            Some(provider_value)
        }
        _ => None,
    };

    if let Some(provider_value) = provider_value {
        let apply_result = match ctx.app.form.as_mut() {
            Some(FormState::ProviderAdd(form)) => {
                form.apply_provider_json_value_to_fields(provider_value)
            }
            _ => Ok(()),
        };

        if let Err(err) = apply_result {
            ctx.app.push_toast(err, ToastKind::Error);
            return Ok(());
        }
    }

    ctx.app.editor = None;
    Ok(())
}

fn submit_provider_form_apply_codex_config_toml(
    ctx: &mut RuntimeActionContext<'_>,
    content: String,
) -> Result<(), AppError> {
    use toml_edit::DocumentMut;

    let config_text = if content.trim().is_empty() {
        String::new()
    } else {
        let doc: DocumentMut = match content.parse() {
            Ok(doc) => doc,
            Err(e) => {
                ctx.app.push_toast(
                    texts::common_config_snippet_invalid_toml(&e.to_string()),
                    ToastKind::Error,
                );
                return Ok(());
            }
        };
        doc.to_string()
    };

    let provider_value = match ctx.app.form.as_ref() {
        Some(FormState::ProviderAdd(form)) => {
            let mut provider_value = form.to_provider_json_value();
            if let Some(settings_value) = provider_value
                .as_object_mut()
                .and_then(|obj| obj.get_mut("settingsConfig"))
            {
                if !settings_value.is_object() {
                    *settings_value = json!({});
                }
                if let Some(settings_obj) = settings_value.as_object_mut() {
                    settings_obj.insert("config".to_string(), Value::String(config_text));
                }
            }
            Some(provider_value)
        }
        _ => None,
    };

    if let Some(provider_value) = provider_value {
        let apply_result = match ctx.app.form.as_mut() {
            Some(FormState::ProviderAdd(form)) => {
                form.apply_provider_json_value_to_fields(provider_value)
            }
            _ => Ok(()),
        };

        if let Err(err) = apply_result {
            ctx.app.push_toast(err, ToastKind::Error);
            return Ok(());
        }
    }

    ctx.app.editor = None;
    Ok(())
}

fn submit_provider_add(
    ctx: &mut RuntimeActionContext<'_>,
    content: String,
) -> Result<(), AppError> {
    let mut provider: Provider = match serde_json::from_str(&content) {
        Ok(p) => p,
        Err(e) => {
            ctx.app.push_toast(
                texts::tui_toast_invalid_json(&e.to_string()),
                ToastKind::Error,
            );
            return Ok(());
        }
    };

    if let Some(message) = validate_provider_submit(&ctx.app.app_type, &provider, false) {
        ctx.app.push_toast(message, ToastKind::Warning);
        return Ok(());
    }

    let state = load_state()?;
    let existing_ids = {
        let config = state.config.read().map_err(AppError::from)?;
        config
            .get_manager(&ctx.app.app_type)
            .map(|manager| manager.providers.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default()
    };
    let Some(provider_id) = crate::cli::tui::form::resolve_provider_id_for_submit(
        &provider.name,
        &provider.id,
        &existing_ids,
    ) else {
        ctx.app.push_toast(
            texts::tui_toast_provider_add_missing_fields(),
            ToastKind::Warning,
        );
        return Ok(());
    };
    provider.id = provider_id;

    match ProviderService::add(&state, ctx.app.app_type.clone(), provider) {
        Ok(true) => {
            ctx.app.editor = None;
            ctx.app.form = None;
            ctx.app
                .push_toast(texts::tui_toast_provider_add_finished(), ToastKind::Success);
            *ctx.data = UiData::load(&ctx.app.app_type)?;
        }
        Ok(false) => {
            ctx.app
                .push_toast(texts::tui_toast_provider_add_failed(), ToastKind::Error);
        }
        Err(err) => {
            ctx.app.push_toast(err.to_string(), ToastKind::Error);
        }
    }

    Ok(())
}

fn submit_provider_edit(
    ctx: &mut RuntimeActionContext<'_>,
    id: String,
    content: String,
) -> Result<(), AppError> {
    let mut provider: Provider = match serde_json::from_str(&content) {
        Ok(p) => p,
        Err(e) => {
            ctx.app.push_toast(
                texts::tui_toast_invalid_json(&e.to_string()),
                ToastKind::Error,
            );
            return Ok(());
        }
    };
    provider.id = id.clone();

    if let Some(message) = validate_provider_submit(&ctx.app.app_type, &provider, true) {
        ctx.app.push_toast(message, ToastKind::Warning);
        return Ok(());
    }

    let state = load_state()?;
    let result = ProviderService::update(&state, ctx.app.app_type.clone(), provider);

    if let Err(err) = result {
        ctx.app.push_toast(err.to_string(), ToastKind::Error);
        return Ok(());
    }

    ctx.app.editor = None;
    ctx.app.form = None;
    ctx.app.push_toast(
        texts::tui_toast_provider_edit_finished(),
        ToastKind::Success,
    );
    *ctx.data = UiData::load(&ctx.app.app_type)?;
    Ok(())
}
fn submit_mcp_add(ctx: &mut RuntimeActionContext<'_>, content: String) -> Result<(), AppError> {
    let server: McpServer = match serde_json::from_str(&content) {
        Ok(s) => s,
        Err(e) => {
            ctx.app.push_toast(
                texts::tui_toast_invalid_json(&e.to_string()),
                ToastKind::Error,
            );
            return Ok(());
        }
    };

    if server.id.trim().is_empty() || server.name.trim().is_empty() {
        ctx.app
            .push_toast(texts::tui_toast_mcp_missing_fields(), ToastKind::Warning);
        return Ok(());
    }

    let state = load_state()?;
    if let Err(err) = McpService::upsert_server(&state, server) {
        ctx.app.push_toast(err.to_string(), ToastKind::Error);
        return Ok(());
    }

    ctx.app.editor = None;
    ctx.app.form = None;
    ctx.app
        .push_toast(texts::tui_toast_mcp_upserted(), ToastKind::Success);
    *ctx.data = UiData::load(&ctx.app.app_type)?;
    Ok(())
}

fn submit_mcp_edit(
    ctx: &mut RuntimeActionContext<'_>,
    id: String,
    content: String,
) -> Result<(), AppError> {
    let mut server: McpServer = match serde_json::from_str(&content) {
        Ok(s) => s,
        Err(e) => {
            ctx.app.push_toast(
                texts::tui_toast_invalid_json(&e.to_string()),
                ToastKind::Error,
            );
            return Ok(());
        }
    };
    server.id = id.clone();

    if server.name.trim().is_empty() {
        ctx.app
            .push_toast(texts::tui_toast_mcp_missing_fields(), ToastKind::Warning);
        return Ok(());
    }

    let state = load_state()?;
    if let Err(err) = McpService::upsert_server(&state, server) {
        ctx.app.push_toast(err.to_string(), ToastKind::Error);
        return Ok(());
    }

    ctx.app.editor = None;
    ctx.app.form = None;
    ctx.app
        .push_toast(texts::tui_toast_mcp_upserted(), ToastKind::Success);
    *ctx.data = UiData::load(&ctx.app.app_type)?;
    Ok(())
}

fn submit_config_common_snippet(
    ctx: &mut RuntimeActionContext<'_>,
    app_type: AppType,
    content: String,
) -> Result<(), AppError> {
    let edited = content.trim().to_string();
    let (next_snippet, toast) = if edited.is_empty() {
        (None, texts::common_config_snippet_cleared())
    } else if matches!(app_type, AppType::Codex) {
        let doc: toml_edit::DocumentMut = match edited.parse() {
            Ok(v) => v,
            Err(e) => {
                ctx.app.push_toast(
                    texts::common_config_snippet_invalid_toml(&e.to_string()),
                    ToastKind::Error,
                );
                return Ok(());
            }
        };
        let canonical = doc.to_string().trim().to_string();
        (Some(canonical), texts::common_config_snippet_saved())
    } else {
        let value: Value = match serde_json::from_str(&edited) {
            Ok(v) => v,
            Err(e) => {
                ctx.app.push_toast(
                    texts::common_config_snippet_invalid_json(&e.to_string()),
                    ToastKind::Error,
                );
                return Ok(());
            }
        };

        if !value.is_object() {
            ctx.app
                .push_toast(texts::common_config_snippet_not_object(), ToastKind::Error);
            return Ok(());
        }

        let pretty = match serde_json::to_string_pretty(&value) {
            Ok(v) => v,
            Err(e) => {
                ctx.app.push_toast(
                    texts::failed_to_serialize_json(&e.to_string()),
                    ToastKind::Error,
                );
                return Ok(());
            }
        };

        (Some(pretty), texts::common_config_snippet_saved())
    };

    let state = load_state()?;
    let service_result = if let Some(snippet) = next_snippet.clone() {
        ProviderService::set_common_config_snippet(&state, app_type.clone(), Some(snippet))
    } else {
        ProviderService::clear_common_config_snippet(&state, app_type.clone())
    };
    if let Err(err) = service_result {
        ctx.app.push_toast(err.to_string(), ToastKind::Error);
        return Ok(());
    }

    ctx.app.editor = None;
    ctx.app.push_toast(toast, ToastKind::Success);
    *ctx.data = UiData::load(&ctx.app.app_type)?;

    let snippet = next_snippet.unwrap_or_else(|| {
        texts::tui_default_common_snippet_for_app(app_type.as_str()).to_string()
    });
    ctx.app.overlay = Overlay::CommonSnippetView {
        app_type: app_type.clone(),
        view: TextViewState {
            title: texts::tui_common_snippet_title(app_type.as_str()),
            lines: snippet.lines().map(|s| s.to_string()).collect(),
            scroll: 0,
            action: None,
        },
    };
    Ok(())
}

fn submit_webdav_settings(
    ctx: &mut RuntimeActionContext<'_>,
    content: String,
) -> Result<(), AppError> {
    let edited = content.trim();
    if edited.is_empty() {
        set_webdav_sync_settings(None)?;
        ctx.app.editor = None;
        ctx.app.push_toast(
            texts::tui_toast_webdav_settings_cleared(),
            ToastKind::Success,
        );
        *ctx.data = UiData::load(&ctx.app.app_type)?;
        return Ok(());
    }

    let cfg: WebDavSyncSettings = serde_json::from_str(edited)
        .map_err(|e| AppError::Message(texts::tui_toast_invalid_json(&e.to_string())))?;
    set_webdav_sync_settings(Some(cfg))?;

    ctx.app.editor = None;
    ctx.app
        .push_toast(texts::tui_toast_webdav_settings_saved(), ToastKind::Success);
    *ctx.data = UiData::load(&ctx.app.app_type)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use serial_test::serial;
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::path::Path;
    use tempfile::{tempdir, TempDir};

    use crate::app_config::AppType;
    use crate::cli::tui::app::{Action, App, Focus, Toast};
    use crate::cli::tui::route::Route;
    use crate::cli::tui::runtime_systems::RequestTracker;
    use crate::cli::tui::terminal::TuiTerminal;
    use crate::openclaw_config::{write_openclaw_config_source, OpenClawModelCatalogEntry};
    use crate::settings::{get_settings, update_settings, AppSettings};
    use crate::test_support::{
        lock_test_home_and_settings, set_test_home_override, TestHomeSettingsLock,
    };

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    struct EnvGuard {
        _lock: TestHomeSettingsLock,
        old_home: Option<OsString>,
        old_userprofile: Option<OsString>,
    }

    impl EnvGuard {
        fn set_home(home: &Path) -> Self {
            let lock = lock_test_home_and_settings();
            let old_home = std::env::var_os("HOME");
            let old_userprofile = std::env::var_os("USERPROFILE");
            std::env::set_var("HOME", home);
            std::env::set_var("USERPROFILE", home);
            set_test_home_override(Some(home));
            crate::settings::reload_test_settings();
            Self {
                _lock: lock,
                old_home,
                old_userprofile,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.old_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match &self.old_userprofile {
                Some(value) => std::env::set_var("USERPROFILE", value),
                None => std::env::remove_var("USERPROFILE"),
            }
            set_test_home_override(self.old_home.as_deref().map(Path::new));
            crate::settings::reload_test_settings();
        }
    }

    struct SettingsGuard {
        previous: AppSettings,
    }

    impl SettingsGuard {
        fn with_openclaw_dir(path: &Path) -> Self {
            let previous = get_settings();
            let mut settings = AppSettings::default();
            settings.openclaw_config_dir = Some(path.display().to_string());
            update_settings(settings).expect("set openclaw override dir");
            Self { previous }
        }
    }

    impl Drop for SettingsGuard {
        fn drop(&mut self) {
            update_settings(self.previous.clone()).expect("restore previous settings");
        }
    }

    struct RuntimeCtxFixture {
        _temp_home: TempDir,
        _env: EnvGuard,
        terminal: TuiTerminal,
        app: App,
        data: UiData,
        proxy_loading: RequestTracker,
        webdav_loading: RequestTracker,
        update_check: RequestTracker,
    }

    fn runtime_ctx(app_type: AppType) -> RuntimeCtxFixture {
        let temp_home = TempDir::new().expect("create temp home");
        let env = EnvGuard::set_home(temp_home.path());

        let terminal = TuiTerminal::new_for_test().expect("create test terminal");
        let app = App::new(Some(app_type.clone()));
        let data = UiData::load(&app_type).expect("load ui data");
        RuntimeCtxFixture {
            _temp_home: temp_home,
            _env: env,
            terminal,
            app,
            data,
            proxy_loading: RequestTracker::default(),
            webdav_loading: RequestTracker::default(),
            update_check: RequestTracker::default(),
        }
    }

    #[test]
    #[serial(home_settings)]
    fn submit_provider_add_generates_id_when_name_is_valid() {
        let mut fixture = runtime_ctx(AppType::Claude);

        let mut ctx = RuntimeActionContext {
            terminal: &mut fixture.terminal,
            app: &mut fixture.app,
            data: &mut fixture.data,
            speedtest_req_tx: None,
            stream_check_req_tx: None,
            skills_req_tx: None,
            proxy_req_tx: None,
            proxy_loading: &mut fixture.proxy_loading,
            local_env_req_tx: None,
            webdav_req_tx: None,
            webdav_loading: &mut fixture.webdav_loading,
            update_req_tx: None,
            update_check: &mut fixture.update_check,
            model_fetch_req_tx: None,
        };

        submit_provider_add(
            &mut ctx,
            r#"{"id":"","name":"Provider One","settingsConfig":{"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}}"#
                .to_string(),
        )
        .expect("submit should succeed");

        let refreshed = UiData::load(&AppType::Claude).expect("reload ui data");
        assert!(
            refreshed
                .providers
                .rows
                .iter()
                .any(|row| row.id == "provider-one"),
            "runtime submit should auto-generate and persist an id"
        );
    }

    #[test]
    #[serial(home_settings)]
    fn submit_provider_add_rejects_name_that_cannot_generate_id() {
        let mut fixture = runtime_ctx(AppType::Claude);

        let mut ctx = RuntimeActionContext {
            terminal: &mut fixture.terminal,
            app: &mut fixture.app,
            data: &mut fixture.data,
            speedtest_req_tx: None,
            stream_check_req_tx: None,
            skills_req_tx: None,
            proxy_req_tx: None,
            proxy_loading: &mut fixture.proxy_loading,
            local_env_req_tx: None,
            webdav_req_tx: None,
            webdav_loading: &mut fixture.webdav_loading,
            update_req_tx: None,
            update_check: &mut fixture.update_check,
            model_fetch_req_tx: None,
        };

        submit_provider_add(
            &mut ctx,
            r#"{"id":"","name":"!!!","settingsConfig":{"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}}"#
                .to_string(),
        )
        .expect("submit should return without crashing");

        let refreshed = UiData::load(&AppType::Claude).expect("reload ui data");
        assert!(
            refreshed.providers.rows.is_empty(),
            "runtime submit should refuse names that still yield an empty id"
        );
        assert!(matches!(
            ctx.app.toast.as_ref(),
            Some(Toast {
                kind: ToastKind::Warning,
                ..
            })
        ));
    }

    #[test]
    #[serial(home_settings)]
    fn submit_provider_add_rejects_blank_codex_base_url() {
        let mut fixture = runtime_ctx(AppType::Codex);

        let mut ctx = RuntimeActionContext {
            terminal: &mut fixture.terminal,
            app: &mut fixture.app,
            data: &mut fixture.data,
            speedtest_req_tx: None,
            stream_check_req_tx: None,
            skills_req_tx: None,
            proxy_req_tx: None,
            proxy_loading: &mut fixture.proxy_loading,
            local_env_req_tx: None,
            webdav_req_tx: None,
            webdav_loading: &mut fixture.webdav_loading,
            update_req_tx: None,
            update_check: &mut fixture.update_check,
            model_fetch_req_tx: None,
        };

        submit_provider_add(
            &mut ctx,
            r#"{
  "id": "",
  "name": "Codex Provider",
  "settingsConfig": {
    "auth": {
      "OPENAI_API_KEY": "sk-test"
    },
    "config": "model_provider = \"custom\"\nmodel = \"gpt-5.4\"\nmodel_reasoning_effort = \"high\"\ndisable_response_storage = true\n\n[model_providers.custom]\nname = \"custom\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
  }
}"#
            .to_string(),
        )
        .expect("submit should return without crashing");

        let refreshed = UiData::load(&AppType::Codex).expect("reload ui data");
        assert!(
            refreshed.providers.rows.is_empty(),
            "runtime submit should reject Codex providers without a base_url"
        );
        assert!(matches!(
            ctx.app.toast.as_ref(),
            Some(Toast {
                kind: ToastKind::Warning,
                message,
                ..
            }) if message == texts::base_url_empty_error()
        ));
    }

    #[test]
    #[serial(home_settings)]
    fn submit_provider_add_preserves_custom_openclaw_name_after_reload() {
        let home_dir = tempdir().expect("create temp home");
        let openclaw_dir = tempdir().expect("create temp openclaw dir");
        let _home = EnvGuard::set_home(home_dir.path());
        let _settings = SettingsGuard::with_openclaw_dir(openclaw_dir.path());

        let mut terminal = TuiTerminal::new_for_test().expect("create test terminal");
        let mut app = App::new(Some(AppType::OpenClaw));
        let mut data = UiData::load(&AppType::OpenClaw).expect("load openclaw ui data");

        let mut proxy_loading = RequestTracker::default();
        let mut webdav_loading = RequestTracker::default();
        let mut update_check = RequestTracker::default();
        let mut ctx = RuntimeActionContext {
            terminal: &mut terminal,
            app: &mut app,
            data: &mut data,
            speedtest_req_tx: None,
            stream_check_req_tx: None,
            skills_req_tx: None,
            proxy_req_tx: None,
            proxy_loading: &mut proxy_loading,
            local_env_req_tx: None,
            webdav_req_tx: None,
            webdav_loading: &mut webdav_loading,
            update_req_tx: None,
            update_check: &mut update_check,
            model_fetch_req_tx: None,
        };

        submit_provider_add(
            &mut ctx,
            r#"{
  "id": "",
  "name": "Friendly OpenClaw",
  "settingsConfig": {
    "apiKey": "sk-friendly",
    "baseUrl": "https://friendly.example/v1",
    "models": [
      { "id": "friendly-model", "name": "Friendly Model" }
    ]
  }
}"#
            .to_string(),
        )
        .expect("submit should succeed");

        let refreshed = UiData::load(&AppType::OpenClaw).expect("reload openclaw ui data");
        let refreshed_row = refreshed
            .providers
            .rows
            .iter()
            .find(|row| row.id == "friendly-openclaw")
            .expect("provider row should still exist after reload");
        assert_eq!(refreshed_row.provider.name, "Friendly OpenClaw");
        assert!(
            refreshed_row.provider.created_at.is_some(),
            "adding an OpenClaw provider through the add flow should persist a user-touched marker"
        );
    }

    #[test]
    #[serial(home_settings)]
    fn submit_provider_edit_updates_openclaw_live_backed_provider() {
        let home_dir = tempdir().expect("create temp home");
        let openclaw_dir = tempdir().expect("create temp openclaw dir");
        let _home = EnvGuard::set_home(home_dir.path());
        let _settings = SettingsGuard::with_openclaw_dir(openclaw_dir.path());

        write_openclaw_config_source(
            r#"{
  models: {
    mode: 'merge',
    providers: {
      'live-only': {
        apiKey: 'sk-live-old',
        baseUrl: 'https://live.old.example/v1',
        models: [{ id: 'live-model-old', name: 'Live Model Old' }],
      },
    },
  },
}"#,
        )
        .expect("seed live-only openclaw provider");

        let mut terminal = TuiTerminal::new_for_test().expect("create test terminal");
        let mut app = App::new(Some(AppType::OpenClaw));
        let mut data = UiData::load(&AppType::OpenClaw).expect("load openclaw ui data");
        assert!(
            data.providers
                .rows
                .iter()
                .any(|row| row.id == "live-only" && row.is_saved && row.is_in_config),
            "precondition: UI should mirror the live provider into the local manager before edit submit"
        );

        let mut proxy_loading = RequestTracker::default();
        let mut webdav_loading = RequestTracker::default();
        let mut update_check = RequestTracker::default();
        let mut ctx = RuntimeActionContext {
            terminal: &mut terminal,
            app: &mut app,
            data: &mut data,
            speedtest_req_tx: None,
            stream_check_req_tx: None,
            skills_req_tx: None,
            proxy_req_tx: None,
            proxy_loading: &mut proxy_loading,
            local_env_req_tx: None,
            webdav_req_tx: None,
            webdav_loading: &mut webdav_loading,
            update_req_tx: None,
            update_check: &mut update_check,
            model_fetch_req_tx: None,
        };

        submit_provider_edit(
            &mut ctx,
            "live-only".to_string(),
            r#"{
  "id": "live-only",
  "name": "Live Only Imported",
  "settingsConfig": {
    "apiKey": "sk-live-new",
    "baseUrl": "https://live.new.example/v1",
    "models": [
      { "id": "live-model-new", "name": "Live Model New" }
    ]
  }
}"#
            .to_string(),
        )
        .expect("submit should succeed");

        let refreshed = UiData::load(&AppType::OpenClaw).expect("reload openclaw ui data");
        assert!(
            refreshed
                .providers
                .rows
                .iter()
                .any(|row| row.id == "live-only" && row.is_saved && row.is_in_config),
            "editing an OpenClaw provider should keep the mirrored row in sync"
        );
        assert_eq!(
            crate::openclaw_config::get_provider("live-only")
                .expect("read rewritten live provider")
                .and_then(|provider| provider
                    .get("baseUrl")
                    .and_then(Value::as_str)
                    .map(str::to_string)),
            Some("https://live.new.example/v1".to_string())
        );
    }

    #[test]
    #[serial(home_settings)]
    fn submit_provider_edit_rejects_blank_codex_base_url() {
        let mut fixture = runtime_ctx(AppType::Codex);
        let state = load_state().expect("load state");
        {
            let mut config = state.config.write().expect("lock config");
            let manager = config
                .get_manager_mut(&AppType::Codex)
                .expect("codex manager");
            manager.providers.insert(
                "codex-provider".to_string(),
                Provider::with_id(
                    "codex-provider".to_string(),
                    "Codex Provider".to_string(),
                    json!({
                        "auth": { "OPENAI_API_KEY": "sk-test" },
                        "config": "model_provider = \"custom\"\nmodel = \"gpt-5.4\"\nmodel_reasoning_effort = \"high\"\ndisable_response_storage = true\n\n[model_providers.custom]\nname = \"custom\"\nbase_url = \"https://api.example.com/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
                    }),
                    None,
                ),
            );
        }
        state.save().expect("persist codex provider");
        fixture.data = UiData::load(&AppType::Codex).expect("reload codex data");

        let mut ctx = RuntimeActionContext {
            terminal: &mut fixture.terminal,
            app: &mut fixture.app,
            data: &mut fixture.data,
            speedtest_req_tx: None,
            stream_check_req_tx: None,
            skills_req_tx: None,
            proxy_req_tx: None,
            proxy_loading: &mut fixture.proxy_loading,
            local_env_req_tx: None,
            webdav_req_tx: None,
            webdav_loading: &mut fixture.webdav_loading,
            update_req_tx: None,
            update_check: &mut fixture.update_check,
            model_fetch_req_tx: None,
        };

        submit_provider_edit(
            &mut ctx,
            "codex-provider".to_string(),
            r#"{
  "id": "codex-provider",
  "name": "Codex Provider",
  "settingsConfig": {
    "auth": {
      "OPENAI_API_KEY": "sk-test"
    },
    "config": "model_provider = \"custom\"\nmodel = \"gpt-5.4\"\nmodel_reasoning_effort = \"high\"\ndisable_response_storage = true\n\n[model_providers.custom]\nname = \"custom\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
  }
}"#
            .to_string(),
        )
        .expect("submit should return without crashing");

        let refreshed = UiData::load(&AppType::Codex).expect("reload ui data");
        let row = refreshed
            .providers
            .rows
            .iter()
            .find(|row| row.id == "codex-provider")
            .expect("provider should remain present");
        let config_text = row.provider.settings_config["config"]
            .as_str()
            .expect("codex config text should remain present");
        assert!(
            config_text.contains("base_url = \"https://api.example.com/v1\""),
            "failed edit should keep the existing base_url intact"
        );
        assert!(matches!(
            ctx.app.toast.as_ref(),
            Some(Toast {
                kind: ToastKind::Warning,
                message,
                ..
            }) if message == texts::base_url_empty_error()
        ));
    }

    #[test]
    #[serial(home_settings)]
    fn submit_provider_edit_preserves_custom_openclaw_name_after_reload() {
        let home_dir = tempdir().expect("create temp home");
        let openclaw_dir = tempdir().expect("create temp openclaw dir");
        let _home = EnvGuard::set_home(home_dir.path());
        let _settings = SettingsGuard::with_openclaw_dir(openclaw_dir.path());

        write_openclaw_config_source(
            r#"{
  models: {
    mode: 'merge',
    providers: {
      'live-only': {
        apiKey: 'sk-live',
        baseUrl: 'https://live.example/v1',
        models: [{ id: 'live-model', name: 'Live Model' }],
      },
    },
  },
}"#,
        )
        .expect("seed live-only openclaw provider");

        let mut terminal = TuiTerminal::new_for_test().expect("create test terminal");
        let mut app = App::new(Some(AppType::OpenClaw));
        let mut data = UiData::load(&AppType::OpenClaw).expect("load openclaw ui data");
        let initial_row = data
            .providers
            .rows
            .iter()
            .find(|row| row.id == "live-only")
            .expect("mirrored provider row should exist before edit");
        assert_eq!(initial_row.provider.name, "live-only");
        assert!(initial_row.provider.created_at.is_none());

        let mut proxy_loading = RequestTracker::default();
        let mut webdav_loading = RequestTracker::default();
        let mut update_check = RequestTracker::default();
        let mut ctx = RuntimeActionContext {
            terminal: &mut terminal,
            app: &mut app,
            data: &mut data,
            speedtest_req_tx: None,
            stream_check_req_tx: None,
            skills_req_tx: None,
            proxy_req_tx: None,
            proxy_loading: &mut proxy_loading,
            local_env_req_tx: None,
            webdav_req_tx: None,
            webdav_loading: &mut webdav_loading,
            update_req_tx: None,
            update_check: &mut update_check,
            model_fetch_req_tx: None,
        };

        submit_provider_edit(
            &mut ctx,
            "live-only".to_string(),
            r#"{
  "id": "live-only",
  "name": "Live Only Custom",
  "meta": {
    "applyCommonConfig": false
  },
  "settingsConfig": {
    "apiKey": "sk-live",
    "baseUrl": "https://live.example/v1",
    "models": [
      { "id": "live-model", "name": "Live Model" }
    ]
  }
}"#
            .to_string(),
        )
        .expect("submit should succeed");

        let refreshed = UiData::load(&AppType::OpenClaw).expect("reload openclaw ui data");
        let refreshed_row = refreshed
            .providers
            .rows
            .iter()
            .find(|row| row.id == "live-only")
            .expect("provider row should still exist after reload");
        assert_eq!(refreshed_row.provider.name, "Live Only Custom");
        assert!(
            refreshed_row.provider.created_at.is_some(),
            "saving a mirrored OpenClaw provider through the edit flow should persist a user-touched marker"
        );
    }

    #[test]
    #[serial(home_settings)]
    fn submit_provider_edit_keeps_saved_only_openclaw_snapshot_rows_visible() {
        let home_dir = tempdir().expect("create temp home");
        let openclaw_dir = tempdir().expect("create temp openclaw dir");
        let _home = EnvGuard::set_home(home_dir.path());
        let _settings = SettingsGuard::with_openclaw_dir(openclaw_dir.path());

        write_openclaw_config_source(
            r#"{
  models: {
    mode: 'merge',
    providers: {
      keep: {
        apiKey: 'sk-keep',
        baseUrl: 'https://keep.example/v1',
        models: [{ id: 'keep-model' }],
      },
    },
  },
  agents: {
    defaults: {
      model: {
        primary: 'keep/keep-model',
      },
    },
  },
}"#,
        )
        .expect("seed unrelated live openclaw provider");

        let state = load_state().expect("load state");
        {
            let mut config = state.config.write().expect("lock config");
            let manager = config
                .get_manager_mut(&AppType::OpenClaw)
                .expect("openclaw manager");
            manager.providers.insert(
                "saved-only".to_string(),
                Provider::with_id(
                    "saved-only".to_string(),
                    "Saved Only".to_string(),
                    json!({
                        "apiKey": "sk-saved-old",
                        "baseUrl": "https://saved.old.example/v1",
                        "models": [{ "id": "saved-model-old" }]
                    }),
                    None,
                ),
            );
        }
        state.save().expect("persist saved-only provider snapshot");

        let data = UiData::load(&AppType::OpenClaw).expect("load openclaw ui data");
        let row = data
            .providers
            .rows
            .iter()
            .find(|row| row.id == "saved-only")
            .expect("saved-only snapshot rows should remain visible after OpenClaw reload");
        assert!(!row.is_in_config);
    }

    #[test]
    #[serial(home_settings)]
    fn submit_provider_edit_rejects_invalid_usage_script_for_openclaw_provider() {
        let home_dir = tempdir().expect("create temp home");
        let openclaw_dir = tempdir().expect("create temp openclaw dir");
        let _home = EnvGuard::set_home(home_dir.path());
        let _settings = SettingsGuard::with_openclaw_dir(openclaw_dir.path());

        write_openclaw_config_source(
            r#"{
  models: {
    mode: 'merge',
    providers: {
      keep: {
        apiKey: 'sk-keep',
        baseUrl: 'https://keep.example/v1',
        models: [{ id: 'keep-model' }],
      },
    },
  },
}"#,
        )
        .expect("seed unrelated live openclaw provider");

        let state = load_state().expect("load state");
        {
            let mut config = state.config.write().expect("lock config");
            let manager = config
                .get_manager_mut(&AppType::OpenClaw)
                .expect("openclaw manager");
            manager.providers.insert(
                "keep".to_string(),
                Provider::with_id(
                    "keep".to_string(),
                    "Keep".to_string(),
                    json!({
                        "apiKey": "sk-keep",
                        "baseUrl": "https://keep.example/v1",
                        "models": [{ "id": "keep-model" }]
                    }),
                    None,
                ),
            );
        }
        state.save().expect("persist mirrored provider state");

        let original_live =
            std::fs::read_to_string(crate::openclaw_config::get_openclaw_config_path())
                .expect("read original live config");
        let mut terminal = TuiTerminal::new_for_test().expect("create test terminal");
        let mut app = App::new(Some(AppType::OpenClaw));
        let mut data = UiData::load(&AppType::OpenClaw).expect("load openclaw ui data");
        assert!(
            data.providers
                .rows
                .iter()
                .any(|row| row.id == "keep" && row.is_saved && row.is_in_config),
            "precondition: UI should expose the mirrored provider before edit submit"
        );

        let mut proxy_loading = RequestTracker::default();
        let mut webdav_loading = RequestTracker::default();
        let mut update_check = RequestTracker::default();
        let mut ctx = RuntimeActionContext {
            terminal: &mut terminal,
            app: &mut app,
            data: &mut data,
            speedtest_req_tx: None,
            stream_check_req_tx: None,
            skills_req_tx: None,
            proxy_req_tx: None,
            proxy_loading: &mut proxy_loading,
            local_env_req_tx: None,
            webdav_req_tx: None,
            webdav_loading: &mut webdav_loading,
            update_req_tx: None,
            update_check: &mut update_check,
            model_fetch_req_tx: None,
        };

        submit_provider_edit(
            &mut ctx,
            "keep".to_string(),
            r#"{
  "id": "keep",
  "name": "Keep Invalid",
  "settingsConfig": {
    "apiKey": "sk-keep-new",
    "baseUrl": "https://keep.new.example/v1",
    "models": [
      { "id": "keep-model-new" }
    ]
  },
  "meta": {
    "usage_script": {
      "enabled": true,
      "language": "javascript",
      "code": "return { success: true, data: [] };",
      "autoQueryInterval": 1441
    }
  }
}"#
            .to_string(),
        )
        .expect("submit should return without crashing");

        assert!(matches!(
            ctx.app.toast.as_ref(),
            Some(Toast {
                kind: ToastKind::Error,
                ..
            })
        ));
        assert!(
            ctx.app
                .toast
                .as_ref()
                .is_some_and(|toast| toast.message.contains("1440")),
            "OpenClaw edits should surface usage_script validation errors"
        );

        let refreshed = UiData::load(&AppType::OpenClaw).expect("reload openclaw ui data");
        let saved_row = refreshed
            .providers
            .rows
            .iter()
            .find(|row| row.id == "keep")
            .expect("provider row should still exist");
        assert_eq!(saved_row.provider.name, "Keep");
        assert_eq!(
            saved_row.provider.settings_config["baseUrl"],
            json!("https://keep.example/v1"),
            "invalid OpenClaw edits should not persist the attempted provider update"
        );
        let live_after =
            std::fs::read_to_string(crate::openclaw_config::get_openclaw_config_path())
                .expect("read live config after rejected saved-only edit");
        assert_eq!(
            live_after, original_live,
            "rejected OpenClaw edits should leave openclaw.json untouched"
        );
    }

    fn run_openclaw_editor_submit_flow(
        route: Route,
        open_key: KeyEvent,
        initial_source: &str,
        expected_submit: EditorSubmit,
        expected_initial_warning: &str,
        edited_content: &str,
    ) -> Result<(App, UiData), AppError> {
        let home_dir = tempdir().expect("create temp home");
        let openclaw_dir = tempdir().expect("create temp openclaw dir");
        let _home = EnvGuard::set_home(home_dir.path());
        let _settings = SettingsGuard::with_openclaw_dir(openclaw_dir.path());

        write_openclaw_config_source(initial_source)?;

        let mut terminal = TuiTerminal::new_for_test()?;
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = route;
        app.focus = Focus::Content;
        let mut data = UiData::load(&AppType::OpenClaw)?;
        assert!(data
            .config
            .openclaw_warnings
            .as_ref()
            .is_some_and(|warnings| warnings
                .iter()
                .any(|warning| warning.code == expected_initial_warning)));

        let open_action = app.on_key(open_key, &data);
        assert!(matches!(open_action, Action::None));
        assert!(matches!(
            app.editor.as_ref().map(|editor| editor.submit.clone()),
            Some(submit) if submit == expected_submit
        ));

        app.editor
            .as_mut()
            .expect("editor should be open")
            .replace_text(edited_content);

        let submit_action = app.on_key(ctrl(KeyCode::Char('s')), &data);
        let Action::EditorSubmit { submit, content } = submit_action else {
            panic!("expected EditorSubmit action");
        };

        let mut proxy_loading = RequestTracker::default();
        let mut webdav_loading = RequestTracker::default();
        let mut update_check = RequestTracker::default();
        let mut ctx = RuntimeActionContext {
            terminal: &mut terminal,
            app: &mut app,
            data: &mut data,
            speedtest_req_tx: None,
            stream_check_req_tx: None,
            skills_req_tx: None,
            proxy_req_tx: None,
            proxy_loading: &mut proxy_loading,
            local_env_req_tx: None,
            webdav_req_tx: None,
            webdav_loading: &mut webdav_loading,
            update_req_tx: None,
            update_check: &mut update_check,
            model_fetch_req_tx: None,
        };

        super::submit(&mut ctx, submit, content)?;
        Ok((app, data))
    }

    fn run_openclaw_tools_form_submit_flow(
        initial_source: &str,
        tools: &OpenClawToolsConfig,
    ) -> Result<(App, UiData), AppError> {
        let home_dir = tempdir().expect("create temp home");
        let openclaw_dir = tempdir().expect("create temp openclaw dir");
        let _home = EnvGuard::set_home(home_dir.path());
        let _settings = SettingsGuard::with_openclaw_dir(openclaw_dir.path());

        write_openclaw_config_source(initial_source)?;

        let mut terminal = TuiTerminal::new_for_test()?;
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;
        app.openclaw_tools_form = Some(
            crate::cli::tui::app::OpenClawToolsFormState::from_snapshot(Some(tools)),
        );
        let mut data = UiData::load(&AppType::OpenClaw)?;

        let content =
            serde_json::to_string_pretty(tools).expect("serialize structured tools form content");

        let mut proxy_loading = RequestTracker::default();
        let mut webdav_loading = RequestTracker::default();
        let mut update_check = RequestTracker::default();
        let mut ctx = RuntimeActionContext {
            terminal: &mut terminal,
            app: &mut app,
            data: &mut data,
            speedtest_req_tx: None,
            stream_check_req_tx: None,
            skills_req_tx: None,
            proxy_req_tx: None,
            proxy_loading: &mut proxy_loading,
            local_env_req_tx: None,
            webdav_req_tx: None,
            webdav_loading: &mut webdav_loading,
            update_req_tx: None,
            update_check: &mut update_check,
            model_fetch_req_tx: None,
        };

        super::submit(&mut ctx, EditorSubmit::ConfigOpenClawTools, content)?;
        Ok((app, data))
    }

    fn run_openclaw_tools_form_submit_flow_with_state(
        initial_source: &str,
        form: crate::cli::tui::app::OpenClawToolsFormState,
    ) -> Result<(App, UiData), AppError> {
        let home_dir = tempdir().expect("create temp home");
        let openclaw_dir = tempdir().expect("create temp openclaw dir");
        let _home = EnvGuard::set_home(home_dir.path());
        let _settings = SettingsGuard::with_openclaw_dir(openclaw_dir.path());

        write_openclaw_config_source(initial_source)?;

        let mut terminal = TuiTerminal::new_for_test()?;
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;
        app.openclaw_tools_form = Some(form.clone());
        let mut data = UiData::load(&AppType::OpenClaw)?;

        let content = serde_json::to_string_pretty(&form.to_config())
            .expect("serialize structured tools form content");

        let mut proxy_loading = RequestTracker::default();
        let mut webdav_loading = RequestTracker::default();
        let mut update_check = RequestTracker::default();
        let mut ctx = RuntimeActionContext {
            terminal: &mut terminal,
            app: &mut app,
            data: &mut data,
            speedtest_req_tx: None,
            stream_check_req_tx: None,
            skills_req_tx: None,
            proxy_req_tx: None,
            proxy_loading: &mut proxy_loading,
            local_env_req_tx: None,
            webdav_req_tx: None,
            webdav_loading: &mut webdav_loading,
            update_req_tx: None,
            update_check: &mut update_check,
            model_fetch_req_tx: None,
        };

        super::submit(&mut ctx, EditorSubmit::ConfigOpenClawTools, content)?;
        Ok((app, data))
    }

    fn run_openclaw_agents_form_submit_flow(
        initial_source: &str,
        defaults: &OpenClawAgentsDefaults,
    ) -> Result<(App, UiData), AppError> {
        let home_dir = tempdir().expect("create temp home");
        let openclaw_dir = tempdir().expect("create temp openclaw dir");
        let _home = EnvGuard::set_home(home_dir.path());
        let _settings = SettingsGuard::with_openclaw_dir(openclaw_dir.path());

        write_openclaw_config_source(initial_source)?;

        let mut terminal = TuiTerminal::new_for_test()?;
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;
        app.openclaw_agents_form =
            Some(crate::cli::tui::app::OpenClawAgentsFormState::from_snapshot(Some(defaults)));
        let mut data = UiData::load(&AppType::OpenClaw)?;

        let content = serde_json::to_string_pretty(defaults)
            .expect("serialize structured agents form content");

        let mut proxy_loading = RequestTracker::default();
        let mut webdav_loading = RequestTracker::default();
        let mut update_check = RequestTracker::default();
        let mut ctx = RuntimeActionContext {
            terminal: &mut terminal,
            app: &mut app,
            data: &mut data,
            speedtest_req_tx: None,
            stream_check_req_tx: None,
            skills_req_tx: None,
            proxy_req_tx: None,
            proxy_loading: &mut proxy_loading,
            local_env_req_tx: None,
            webdav_req_tx: None,
            webdav_loading: &mut webdav_loading,
            update_req_tx: None,
            update_check: &mut update_check,
            model_fetch_req_tx: None,
        };

        super::submit(&mut ctx, EditorSubmit::ConfigOpenClawAgents, content)?;
        Ok((app, data))
    }

    fn run_openclaw_agents_form_submit_flow_with_state(
        initial_source: &str,
        form: crate::cli::tui::app::OpenClawAgentsFormState,
    ) -> Result<(App, UiData), AppError> {
        let home_dir = tempdir().expect("create temp home");
        let openclaw_dir = tempdir().expect("create temp openclaw dir");
        let _home = EnvGuard::set_home(home_dir.path());
        let _settings = SettingsGuard::with_openclaw_dir(openclaw_dir.path());

        write_openclaw_config_source(initial_source)?;

        let mut terminal = TuiTerminal::new_for_test()?;
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;
        app.openclaw_agents_form = Some(form.clone());
        let mut data = UiData::load(&AppType::OpenClaw)?;

        let content = serde_json::to_string_pretty(&form.to_config())
            .expect("serialize structured agents form content");

        let mut proxy_loading = RequestTracker::default();
        let mut webdav_loading = RequestTracker::default();
        let mut update_check = RequestTracker::default();
        let mut ctx = RuntimeActionContext {
            terminal: &mut terminal,
            app: &mut app,
            data: &mut data,
            speedtest_req_tx: None,
            stream_check_req_tx: None,
            skills_req_tx: None,
            proxy_req_tx: None,
            proxy_loading: &mut proxy_loading,
            local_env_req_tx: None,
            webdav_req_tx: None,
            webdav_loading: &mut webdav_loading,
            update_req_tx: None,
            update_check: &mut update_check,
            model_fetch_req_tx: None,
        };

        super::submit(&mut ctx, EditorSubmit::ConfigOpenClawAgents, content)?;
        Ok((app, data))
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_config_route_env_editor_submit_saves_and_reloads_ui_data() {
        let initial_source = r#"{
  env: {
    vars: 'broken-env',
  },
}"#;
        let edited_content = r#"{
  "OPENCLAW_ENV_TOKEN": "fresh-token",
  "OPENCLAW_TIMEOUT": 30
}"#;

        let (app, data) = run_openclaw_editor_submit_flow(
            Route::ConfigOpenClawEnv,
            key(KeyCode::Enter),
            initial_source,
            EditorSubmit::ConfigOpenClawEnv,
            "stringified_env_vars",
            edited_content,
        )
        .expect("env submit flow should succeed");

        assert!(app.editor.is_none());
        assert_eq!(
            app.toast.as_ref().map(|toast| toast.message.as_str()),
            Some(
                texts::tui_toast_openclaw_config_saved(texts::tui_config_item_openclaw_env())
                    .as_str()
            )
        );
        assert_eq!(
            data.config
                .openclaw_env
                .as_ref()
                .and_then(|env| env.vars.get("OPENCLAW_ENV_TOKEN")),
            Some(&Value::String("fresh-token".to_string()))
        );
        let warnings = data.config.openclaw_warnings.clone().unwrap_or_default();
        assert!(!warnings
            .iter()
            .any(|warning| warning.code == "stringified_env_vars"));
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_config_route_tools_form_submit_saves_and_reloads_ui_data() {
        let initial_source = r#"{
  tools: {
    profile: 'dangerous',
    allow: ['Read'],
  },
}"#;
        let edited_tools = OpenClawToolsConfig {
            profile: Some("coding".to_string()),
            allow: vec!["Read".to_string(), "Write".to_string()],
            deny: vec!["Bash".to_string()],
            extra: std::collections::HashMap::new(),
        };

        let (app, data) = run_openclaw_tools_form_submit_flow(initial_source, &edited_tools)
            .expect("tools submit flow should succeed");

        assert!(app.editor.is_none());
        let form = app
            .openclaw_tools_form
            .as_ref()
            .expect("tools form should be reloaded after save");
        assert_eq!(
            form.section,
            crate::cli::tui::app::OpenClawToolsSection::Profile
        );
        assert_eq!(form.row, 0);
        assert!(
            app.toast.is_none(),
            "successful tools auto-save should stay silent"
        );
        assert_eq!(
            data.config
                .openclaw_tools
                .as_ref()
                .and_then(|tools| tools.profile.as_deref()),
            Some("coding")
        );
        assert_eq!(
            data.config
                .openclaw_tools
                .as_ref()
                .map(|tools| tools.allow.clone()),
            Some(vec!["Read".to_string(), "Write".to_string()])
        );
        let warnings = data.config.openclaw_warnings.clone().unwrap_or_default();
        assert!(!warnings
            .iter()
            .any(|warning| warning.code == "invalid_tools_profile"));
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_config_route_tools_form_submit_preserves_structured_focus_after_reload() {
        let initial_source = r#"{
  tools: {
    profile: 'dangerous',
    allow: ['Read'],
  },
}"#;
        let mut form = crate::cli::tui::app::OpenClawToolsFormState::from_snapshot(Some(
            &OpenClawToolsConfig {
                profile: Some("coding".to_string()),
                allow: vec!["Read".to_string(), "Write".to_string()],
                deny: vec!["Bash".to_string(), "Exec".to_string()],
                extra: HashMap::new(),
            },
        ));
        form.section = crate::cli::tui::app::OpenClawToolsSection::Deny;
        form.row = 1;

        let (app, data) = run_openclaw_tools_form_submit_flow_with_state(initial_source, form)
            .expect("tools submit flow should succeed");

        let form = app
            .openclaw_tools_form
            .as_ref()
            .expect("tools form should stay available after reload");
        assert_eq!(
            form.section,
            crate::cli::tui::app::OpenClawToolsSection::Deny
        );
        assert_eq!(form.row, 1);
        assert_eq!(form.deny, vec!["Bash".to_string(), "Exec".to_string()]);
        assert_eq!(
            data.config
                .openclaw_tools
                .as_ref()
                .map(|tools| tools.deny.clone()),
            Some(vec!["Bash".to_string(), "Exec".to_string()])
        );
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_config_route_agents_form_submit_saves_and_reloads_ui_data() {
        let initial_source = r#"{
  models: {
    mode: 'merge',
    providers: {
      demo: {
        models: [
          { id: 'gpt-4.1' },
          { id: 'gpt-4o-mini' },
        ],
      },
    },
  },
  agents: {
    defaults: {
      timeout: 42,
      model: {
        primary: 'legacy-model',
      },
    },
  },
}"#;
        let edited_defaults = OpenClawAgentsDefaults {
            model: Some(crate::openclaw_config::OpenClawDefaultModel {
                primary: "demo/gpt-4.1".to_string(),
                fallbacks: vec!["demo/gpt-4o-mini".to_string()],
                extra: HashMap::new(),
            }),
            models: Some(HashMap::from([(
                "demo/gpt-4.1".to_string(),
                OpenClawModelCatalogEntry {
                    alias: Some("General".to_string()),
                    extra: HashMap::new(),
                },
            )])),
            extra: HashMap::new(),
        };

        let (app, data) = run_openclaw_agents_form_submit_flow(initial_source, &edited_defaults)
            .expect("agents submit flow should succeed");

        assert!(app.editor.is_none());
        let form = app
            .openclaw_agents_form
            .as_ref()
            .expect("agents form should be reloaded after save");
        assert_eq!(
            form.section,
            crate::cli::tui::app::OpenClawAgentsSection::PrimaryModel
        );
        assert_eq!(form.row, 0);
        assert!(
            app.toast.is_none(),
            "successful agents auto-save should stay silent"
        );
        assert_eq!(
            data.config
                .openclaw_agents_defaults
                .as_ref()
                .and_then(|defaults| defaults.model.as_ref())
                .map(|model| model.primary.as_str()),
            Some("demo/gpt-4.1")
        );
        assert_eq!(
            data.config
                .openclaw_agents_defaults
                .as_ref()
                .and_then(|defaults| defaults.models.as_ref())
                .and_then(|models| models.get("demo/gpt-4.1")),
            Some(&OpenClawModelCatalogEntry {
                alias: Some("General".to_string()),
                extra: HashMap::new(),
            })
        );
        let warnings = data.config.openclaw_warnings.clone().unwrap_or_default();
        assert!(!warnings
            .iter()
            .any(|warning| warning.code == "legacy_agents_timeout"));
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_config_route_agents_form_submit_preserves_structured_focus_after_reload() {
        let initial_source = r#"{
  models: {
    mode: 'merge',
    providers: {
      demo: {
        models: [
          { id: 'gpt-4.1' },
          { id: 'gpt-4o-mini' },
        ],
      },
    },
  },
  agents: {
    defaults: {
      workspace: './workspace',
      contextTokens: 4096,
      maxConcurrent: 3,
      model: {
        primary: 'demo/gpt-4.1',
      },
    },
  },
}"#;
        let mut form = crate::cli::tui::app::OpenClawAgentsFormState::from_snapshot(Some(
            &OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "demo/gpt-4.1".to_string(),
                    fallbacks: vec!["demo/gpt-4o-mini".to_string()],
                    extra: HashMap::new(),
                }),
                models: None,
                extra: HashMap::from([
                    (
                        "workspace".to_string(),
                        Value::String("./workspace".to_string()),
                    ),
                    ("contextTokens".to_string(), Value::from(4096)),
                    ("maxConcurrent".to_string(), Value::from(3)),
                ]),
            },
        ));
        form.section = crate::cli::tui::app::OpenClawAgentsSection::Runtime;
        form.row = 2;

        let (app, data) = run_openclaw_agents_form_submit_flow_with_state(initial_source, form)
            .expect("agents submit flow should succeed");

        let form = app
            .openclaw_agents_form
            .as_ref()
            .expect("agents form should stay available after reload");
        assert_eq!(
            form.section,
            crate::cli::tui::app::OpenClawAgentsSection::Runtime
        );
        assert_eq!(form.row, 2);
        assert_eq!(form.context_tokens, "4096");
        assert_eq!(
            data.config
                .openclaw_agents_defaults
                .as_ref()
                .and_then(|defaults| defaults.extra.get("contextTokens")),
            Some(&Value::from(4096))
        );
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_config_route_agents_form_submit_clears_stale_error_toast_after_success() {
        let initial_source = r#"{
  agents: {
    defaults: {
      workspace: './workspace',
    },
  },
}"#;
        let home_dir = tempdir().expect("create temp home");
        let openclaw_dir = tempdir().expect("create temp openclaw dir");
        let _home = EnvGuard::set_home(home_dir.path());
        let _settings = SettingsGuard::with_openclaw_dir(openclaw_dir.path());

        write_openclaw_config_source(initial_source).expect("seed openclaw config");

        let mut terminal = TuiTerminal::new_for_test().expect("create test terminal");
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;
        app.openclaw_agents_form = Some(
            crate::cli::tui::app::OpenClawAgentsFormState::from_snapshot(Some(
                &OpenClawAgentsDefaults {
                    model: None,
                    models: None,
                    extra: HashMap::from([(
                        "workspace".to_string(),
                        Value::String("./workspace-next".to_string()),
                    )]),
                },
            )),
        );
        app.push_toast(
            texts::tui_toast_openclaw_agents_save_failed_detail("previous failure"),
            ToastKind::Error,
        );
        let mut data = UiData::load(&AppType::OpenClaw).expect("load openclaw ui data");

        let content = serde_json::to_string_pretty(
            &app.openclaw_agents_form
                .as_ref()
                .expect("agents form should be seeded")
                .to_config(),
        )
        .expect("serialize structured agents form content");

        let mut proxy_loading = RequestTracker::default();
        let mut webdav_loading = RequestTracker::default();
        let mut update_check = RequestTracker::default();
        let mut ctx = RuntimeActionContext {
            terminal: &mut terminal,
            app: &mut app,
            data: &mut data,
            speedtest_req_tx: None,
            stream_check_req_tx: None,
            skills_req_tx: None,
            proxy_req_tx: None,
            proxy_loading: &mut proxy_loading,
            local_env_req_tx: None,
            webdav_req_tx: None,
            webdav_loading: &mut webdav_loading,
            update_req_tx: None,
            update_check: &mut update_check,
            model_fetch_req_tx: None,
        };

        super::submit(&mut ctx, EditorSubmit::ConfigOpenClawAgents, content)
            .expect("agents submit flow should succeed");

        assert!(
            app.toast.is_none(),
            "successful agents auto-save should clear stale error toast without showing success"
        );
        assert_eq!(
            data.config
                .openclaw_agents_defaults
                .as_ref()
                .and_then(|defaults| defaults.extra.get("workspace")),
            Some(&Value::String("./workspace-next".to_string()))
        );
    }

    #[test]
    #[serial(home_settings)]
    fn submit_provider_form_apply_json_keeps_common_snippet_out_of_raw_submit_payload() {
        let mut fixture = runtime_ctx(AppType::Claude);

        fixture.data.config.common_snippet = r#"{
            "alwaysThinkingEnabled": false,
            "env": {
                "COMMON_FLAG": "1"
            }
        }"#
        .to_string();

        let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
        form.id.set("p1");
        form.name.set("Provider One");
        form.include_common_config = true;
        form.claude_base_url.set("https://provider.example");
        fixture.app.form = Some(FormState::ProviderAdd(form));

        let mut ctx = RuntimeActionContext {
            terminal: &mut fixture.terminal,
            app: &mut fixture.app,
            data: &mut fixture.data,
            speedtest_req_tx: None,
            stream_check_req_tx: None,
            skills_req_tx: None,
            proxy_req_tx: None,
            proxy_loading: &mut fixture.proxy_loading,
            local_env_req_tx: None,
            webdav_req_tx: None,
            webdav_loading: &mut fixture.webdav_loading,
            update_req_tx: None,
            update_check: &mut fixture.update_check,
            model_fetch_req_tx: None,
        };

        submit_provider_form_apply_json(
            &mut ctx,
            r#"{
                "alwaysThinkingEnabled": false,
                "env": {
                    "ANTHROPIC_BASE_URL": "https://edited.example",
                    "COMMON_FLAG": "1",
                    "EXTRA_FIELD": "kept"
                }
            }"#
            .to_string(),
        )
        .expect("apply should succeed");

        let FormState::ProviderAdd(form) = ctx
            .app
            .form
            .as_ref()
            .expect("provider form should remain open")
        else {
            panic!("expected provider form");
        };
        let settings = form
            .to_provider_json_value()
            .get("settingsConfig")
            .cloned()
            .expect("settingsConfig should exist");

        assert!(
            settings.get("alwaysThinkingEnabled").is_none(),
            "applying preview JSON should not persist top-level common snippet keys into raw form payload"
        );
        assert!(
            settings["env"].get("COMMON_FLAG").is_none(),
            "applying preview JSON should not persist nested common snippet keys into raw form payload"
        );
        assert_eq!(
            settings["env"]["ANTHROPIC_BASE_URL"], "https://edited.example",
            "provider-specific edits from the preview editor should still be preserved"
        );
        assert_eq!(
            settings["env"]["EXTRA_FIELD"], "kept",
            "non-common keys introduced in the preview editor should still be preserved"
        );
    }

    #[test]
    #[serial(home_settings)]
    fn submit_provider_form_apply_json_preserves_missing_meta_subset_detection() {
        let mut fixture = runtime_ctx(AppType::Claude);

        fixture.data.config.common_snippet = r#"{
            "alwaysThinkingEnabled": false,
            "env": {
                "COMMON_FLAG": "1"
            }
        }"#
        .to_string();

        let provider = Provider::with_id(
            "legacy-provider".to_string(),
            "Legacy Provider".to_string(),
            json!({
                "alwaysThinkingEnabled": false,
                "env": {
                    "ANTHROPIC_BASE_URL": "https://provider.example",
                    "COMMON_FLAG": "1"
                }
            }),
            None,
        );
        fixture.app.form = Some(FormState::ProviderAdd(
            crate::cli::tui::form::ProviderAddFormState::from_provider(AppType::Claude, &provider),
        ));

        let mut ctx = RuntimeActionContext {
            terminal: &mut fixture.terminal,
            app: &mut fixture.app,
            data: &mut fixture.data,
            speedtest_req_tx: None,
            stream_check_req_tx: None,
            skills_req_tx: None,
            proxy_req_tx: None,
            proxy_loading: &mut fixture.proxy_loading,
            local_env_req_tx: None,
            webdav_req_tx: None,
            webdav_loading: &mut fixture.webdav_loading,
            update_req_tx: None,
            update_check: &mut fixture.update_check,
            model_fetch_req_tx: None,
        };

        submit_provider_form_apply_json(
            &mut ctx,
            r#"{
                "alwaysThinkingEnabled": false,
                "env": {
                    "ANTHROPIC_BASE_URL": "https://edited.example",
                    "COMMON_FLAG": "1"
                }
            }"#
            .to_string(),
        )
        .expect("apply should succeed");

        let FormState::ProviderAdd(form) = ctx
            .app
            .form
            .as_ref()
            .expect("provider form should remain open")
        else {
            panic!("expected provider form");
        };
        let raw = form.to_provider_json_value();
        assert!(
            raw.get("meta")
                .and_then(|meta| meta.get("commonConfigEnabled"))
                .is_none(),
            "settings JSON edits on a missing-meta provider must not synthesize explicit common config metadata"
        );
        assert_eq!(
            raw["settingsConfig"]["alwaysThinkingEnabled"], false,
            "missing-meta providers must keep the common subset when metadata remains absent"
        );
        assert_eq!(
            raw["settingsConfig"]["env"]["COMMON_FLAG"], "1",
            "missing-meta providers need the common subset for backend subset detection"
        );
        assert_eq!(
            raw["settingsConfig"]["env"]["ANTHROPIC_BASE_URL"], "https://edited.example",
            "provider-specific edits from the settings JSON editor should still be preserved"
        );
    }
}
