use crate::app_config::{AppType, McpApps};
use crate::cli::i18n::texts;
use crate::error::AppError;
use crate::services::McpService;

use super::super::app::ToastKind;
use super::super::data::{load_state, UiData};
use super::helpers::import_mcp_from_supported_apps;
use super::RuntimeActionContext;

pub(super) fn toggle(
    ctx: &mut RuntimeActionContext<'_>,
    id: String,
    enabled: bool,
) -> Result<(), AppError> {
    let state = load_state()?;
    McpService::toggle_app(&state, &id, ctx.app.app_type.clone(), enabled)?;
    if !crate::sync_policy::should_sync_live(&ctx.app.app_type) {
        let mut message = texts::tui_toast_mcp_updated().to_string();
        message.push(' ');
        message.push_str(&texts::tui_toast_live_sync_skipped_uninitialized(
            ctx.app.app_type.as_str(),
        ));
        ctx.app.push_toast(message, ToastKind::Warning);
    } else {
        ctx.app
            .push_toast(texts::tui_toast_mcp_updated(), ToastKind::Success);
    }
    *ctx.data = UiData::load(&ctx.app.app_type)?;
    Ok(())
}

pub(super) fn set_apps(
    ctx: &mut RuntimeActionContext<'_>,
    id: String,
    apps: McpApps,
) -> Result<(), AppError> {
    let Some(before) = ctx
        .data
        .mcp
        .rows
        .iter()
        .find(|row| row.id == id)
        .map(|row| row.server.apps.clone())
    else {
        ctx.app
            .push_toast(texts::tui_toast_mcp_server_not_found(), ToastKind::Warning);
        return Ok(());
    };

    let state = load_state()?;
    let mut skipped: Vec<&str> = Vec::new();
    let mut changed = false;

    for app_type in [
        AppType::Claude,
        AppType::Codex,
        AppType::Gemini,
        AppType::OpenCode,
        AppType::Hermes,
    ] {
        let next_enabled = apps.is_enabled_for(&app_type);
        if before.is_enabled_for(&app_type) == next_enabled {
            continue;
        }

        changed = true;
        McpService::toggle_app(&state, &id, app_type.clone(), next_enabled)?;
        if !crate::sync_policy::should_sync_live(&app_type) {
            skipped.push(app_type.as_str());
        }
    }

    if !changed || skipped.is_empty() {
        ctx.app
            .push_toast(texts::tui_toast_mcp_updated(), ToastKind::Success);
    } else {
        ctx.app.push_toast(
            texts::tui_toast_mcp_updated_live_sync_skipped(&skipped),
            ToastKind::Warning,
        );
    }

    *ctx.data = UiData::load(&ctx.app.app_type)?;
    Ok(())
}

pub(super) fn delete(ctx: &mut RuntimeActionContext<'_>, id: String) -> Result<(), AppError> {
    let state = load_state()?;
    let deleted = McpService::delete_server(&state, &id)?;
    if deleted {
        ctx.app
            .push_toast(texts::tui_toast_mcp_server_deleted(), ToastKind::Success);
    } else {
        ctx.app
            .push_toast(texts::tui_toast_mcp_server_not_found(), ToastKind::Warning);
    }
    *ctx.data = UiData::load(&ctx.app.app_type)?;
    Ok(())
}

pub(super) fn import_supported_apps(ctx: &mut RuntimeActionContext<'_>) -> Result<(), AppError> {
    import_mcp_from_supported_apps(ctx.app, ctx.data)
}
