use crate::error::AppError;

use super::super::app::ToastKind;
use super::super::data::UiData;
use super::RuntimeActionContext;

pub(super) fn delete_model(
    ctx: &mut RuntimeActionContext<'_>,
    provider_id: String,
    model_id: String,
    expected_revision: String,
) -> Result<(), AppError> {
    crate::omp_config::remove_omp_model_checked(&provider_id, &model_id, &expected_revision)?;
    ctx.app
        .push_toast(crate::t!("Model deleted", "模型已删除"), ToastKind::Success);
    *ctx.data = UiData::load(&ctx.app.app_type)?;
    ctx.app.clamp_selections(ctx.data);
    Ok(())
}

pub(super) fn delete_role(
    ctx: &mut RuntimeActionContext<'_>,
    role: String,
    expected_revision: String,
) -> Result<(), AppError> {
    crate::omp_config::set_omp_model_role(&role, None, Some(&expected_revision))?;
    ctx.app
        .push_toast(crate::t!("Role deleted", "角色已删除"), ToastKind::Success);
    *ctx.data = UiData::load(&ctx.app.app_type)?;
    ctx.app.clamp_selections(ctx.data);
    Ok(())
}

pub(super) fn delete_system_prompt(
    ctx: &mut RuntimeActionContext<'_>,
    kind: crate::services::pi_prompt_files::PiPromptFileKind,
    expected_revision: String,
) -> Result<(), AppError> {
    crate::services::pi_prompt_files::OmpPromptFileService::delete(kind, &expected_revision)?;
    ctx.app.push_toast(
        crate::t!("System prompt deleted", "系统提示词已删除"),
        ToastKind::Success,
    );
    *ctx.data = UiData::load(&ctx.app.app_type)?;
    ctx.app.clamp_selections(ctx.data);
    Ok(())
}
