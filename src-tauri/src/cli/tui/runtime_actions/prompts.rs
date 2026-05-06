use crate::cli::i18n::texts;
use crate::error::AppError;
use crate::services::PromptService;

use super::super::app::ToastKind;
use super::super::data::{load_state, UiData};
use super::helpers::select_prompt_by_id;
use super::RuntimeActionContext;

pub(super) fn activate(ctx: &mut RuntimeActionContext<'_>, id: String) -> Result<(), AppError> {
    let state = load_state()?;
    PromptService::enable_prompt(&state, ctx.app.app_type.clone(), &id)?;
    ctx.app
        .push_toast(texts::tui_toast_prompt_activated(), ToastKind::Success);
    *ctx.data = UiData::load(&ctx.app.app_type)?;
    Ok(())
}

pub(super) fn deactivate(ctx: &mut RuntimeActionContext<'_>, id: String) -> Result<(), AppError> {
    let state = load_state()?;
    PromptService::disable_prompt(&state, ctx.app.app_type.clone(), &id)?;
    ctx.app
        .push_toast(texts::tui_toast_prompt_deactivated(), ToastKind::Success);
    *ctx.data = UiData::load(&ctx.app.app_type)?;
    Ok(())
}

pub(super) fn rename(
    ctx: &mut RuntimeActionContext<'_>,
    id: String,
    name: String,
) -> Result<(), AppError> {
    let state = load_state()?;
    PromptService::rename_prompt(&state, ctx.app.app_type.clone(), &id, &name)?;
    ctx.app
        .push_toast(texts::tui_toast_prompt_renamed(), ToastKind::Success);
    *ctx.data = UiData::load(&ctx.app.app_type)?;
    select_prompt_by_id(ctx.app, ctx.data, &id);
    Ok(())
}

pub(super) fn delete(ctx: &mut RuntimeActionContext<'_>, id: String) -> Result<(), AppError> {
    let state = load_state()?;
    PromptService::delete_prompt(&state, ctx.app.app_type.clone(), &id)?;
    ctx.app
        .push_toast(texts::tui_toast_prompt_deleted(), ToastKind::Success);
    *ctx.data = UiData::load(&ctx.app.app_type)?;
    Ok(())
}
