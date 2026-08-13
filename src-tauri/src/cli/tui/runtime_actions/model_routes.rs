use crate::cli::i18n::texts;
use crate::error::AppError;
use crate::model_route::ModelRoute;

use super::super::app::ToastKind;
use super::super::data::{load_state, ModelRouteRow, ModelRouteSnapshot};
use super::RuntimeActionContext;

fn refresh_model_routes_data(ctx: &mut RuntimeActionContext<'_>) -> Result<(), AppError> {
    let state = load_state()?;
    let routes = state.db.list_model_routes(ctx.app.app_type.as_str())?;

    let rows: Vec<ModelRouteRow> = routes
        .into_iter()
        .map(|route| {
            let provider_name = ctx
                .data
                .providers
                .rows
                .iter()
                .find(|p| p.id == route.provider_id)
                .map(|p| super::super::data::provider_display_name(&ctx.app.app_type, p))
                .unwrap_or_else(|| route.provider_id.clone());

            ModelRouteRow {
                id: route.id,
                pattern: route.pattern,
                provider_id: route.provider_id,
                provider_name,
                priority: route.priority,
                enabled: route.enabled,
                hit_count: route.hit_count,
                last_hit_at: route.last_hit_at,
            }
        })
        .collect();

    ctx.data.model_routes = ModelRouteSnapshot { rows };
    ctx.app.clamp_selections(ctx.data);
    ctx.data.mark_current_app_data_changed();
    Ok(())
}

pub(super) fn handle_add(
    ctx: &mut RuntimeActionContext<'_>,
    pattern: String,
    provider_id: String,
    priority: i32,
) -> Result<(), AppError> {
    let state = load_state()?;
    let route = ModelRoute {
        id: String::new(),
        app_type: ctx.app.app_type.as_str().to_string(),
        pattern,
        provider_id,
        priority,
        enabled: true,
        created_at: None,

        hit_count: 0,

        last_hit_at: None,
        updated_at: None,
    };

    state.db.create_model_route(&route)?;
    refresh_model_routes_data(ctx)?;
    ctx.app
        .push_toast(texts::tui_toast_model_route_added(), ToastKind::Success);
    ctx.app.overlay = super::super::app::Overlay::None;
    Ok(())
}

pub(super) fn handle_edit(
    ctx: &mut RuntimeActionContext<'_>,
    id: String,
    pattern: String,
    provider_id: String,
    priority: i32,
) -> Result<(), AppError> {
    let state = load_state()?;
    // 保留已有的 enabled 状态，不因编辑而静默恢复已禁用的路由
    let enabled = state
        .db
        .get_model_route(&id)
        .ok()
        .flatten()
        .map(|existing| existing.enabled)
        .unwrap_or(true);
    let route = ModelRoute {
        id: String::new(),
        app_type: ctx.app.app_type.as_str().to_string(),
        pattern,
        provider_id,
        priority,
        enabled,
        created_at: None,

        hit_count: 0,

        last_hit_at: None,
        updated_at: None,
    };

    state.db.update_model_route(&id, &route)?;
    refresh_model_routes_data(ctx)?;
    ctx.app
        .push_toast(texts::tui_toast_model_route_updated(), ToastKind::Success);
    ctx.app.overlay = super::super::app::Overlay::None;
    Ok(())
}

pub(super) fn handle_delete(
    ctx: &mut RuntimeActionContext<'_>,
    id: String,
) -> Result<(), AppError> {
    let state = load_state()?;
    state.db.delete_model_route(&id)?;
    refresh_model_routes_data(ctx)?;
    ctx.app
        .push_toast(texts::tui_toast_model_route_deleted(), ToastKind::Success);
    Ok(())
}

pub(super) fn handle_toggle(
    ctx: &mut RuntimeActionContext<'_>,
    id: String,
) -> Result<(), AppError> {
    let state = load_state()?;
    state.db.toggle_model_route(&id)?;
    refresh_model_routes_data(ctx)?;
    Ok(())
}
