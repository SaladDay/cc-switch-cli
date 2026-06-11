use crate::error::AppError;

use super::super::app::ToastKind;
use super::super::data::{load_state, UiData};
use super::RuntimeActionContext;

pub(super) fn delete(ctx: &mut RuntimeActionContext<'_>, model_id: String) -> Result<(), AppError> {
    let state = load_state()?;
    match state.db.delete_model_pricing(&model_id) {
        Ok(true) => {
            ctx.app.push_toast(
                crate::t!("Model pricing deleted.", "模型定价已删除。"),
                ToastKind::Success,
            );
        }
        Ok(false) => {
            ctx.app.push_toast(
                crate::t!("Model pricing not found.", "未找到模型定价。"),
                ToastKind::Warning,
            );
        }
        Err(err) => {
            ctx.app.push_toast(err.to_string(), ToastKind::Error);
            return Ok(());
        }
    }

    *ctx.data = UiData::load(&ctx.app.app_type)?;
    ctx.app.clamp_selections(ctx.data);
    Ok(())
}
