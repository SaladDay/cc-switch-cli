//! Route mode runtime actions: enable/disable, member mutations.
//!
//! Member edits load the stored group doc, apply the change by provider id,
//! and save through the validating DAO so per-member model mappings survive.

use std::collections::BTreeMap;

use crate::app_config::AppType;
use crate::database::{RouteModeGroup, RouteModeMember};
use crate::error::AppError;

use super::super::app::ToastKind;
use super::super::data::{load_state, UiData};
use super::RuntimeActionContext;

fn runtime() -> Result<tokio::runtime::Runtime, AppError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::Message(format!("failed to create async runtime: {e}")))
}

pub(super) fn set_route_mode_enabled(
    ctx: &mut RuntimeActionContext<'_>,
    app_type: AppType,
    enabled: bool,
) -> Result<(), AppError> {
    let state = load_state()?;
    let runtime = runtime()?;

    if enabled {
        runtime
            .block_on(
                state
                    .proxy_service
                    .enable_route_mode_for_app(app_type.as_str()),
            )
            .map_err(AppError::Message)?;
    } else {
        runtime
            .block_on(
                state
                    .proxy_service
                    .disable_route_mode_for_app(app_type.as_str()),
            )
            .map_err(AppError::Message)?;
    }

    *ctx.data = UiData::load(&ctx.app.app_type)?;
    ctx.app.push_toast(
        if enabled {
            crate::t!("Route mode enabled.", "路由模式已开启。")
        } else {
            crate::t!("Route mode disabled.", "路由模式已关闭。")
        },
        ToastKind::Success,
    );
    Ok(())
}

pub(super) fn enable_proxy_and_route_mode(
    ctx: &mut RuntimeActionContext<'_>,
    app_type: AppType,
) -> Result<(), AppError> {
    let state = load_state()?;
    let runtime = runtime()?;

    runtime
        .block_on(
            state
                .proxy_service
                .enable_proxy_and_route_mode_for_app(app_type.as_str()),
        )
        .map_err(AppError::Message)?;

    *ctx.data = UiData::load(&ctx.app.app_type)?;
    ctx.app.push_toast(
        crate::t!(
            "Proxy routing and route mode enabled.",
            "代理路由和路由模式已开启。"
        ),
        ToastKind::Success,
    );
    Ok(())
}

pub(super) fn add_route_mode_member(
    ctx: &mut RuntimeActionContext<'_>,
    app_type: AppType,
    provider_id: String,
) -> Result<(), AppError> {
    mutate_route_mode_group(ctx, &app_type, |group| {
        if group
            .members
            .iter()
            .any(|member| member.provider_id == provider_id)
        {
            return Ok(false);
        }
        group.members.push(RouteModeMember::new(provider_id, 1));
        Ok(true)
    })
}

pub(super) fn remove_route_mode_member(
    ctx: &mut RuntimeActionContext<'_>,
    app_type: AppType,
    provider_id: String,
) -> Result<(), AppError> {
    mutate_route_mode_group(ctx, &app_type, |group| {
        let before = group.members.len();
        group
            .members
            .retain(|member| member.provider_id != provider_id);
        Ok(group.members.len() != before)
    })
}

pub(super) fn toggle_route_mode_member(
    ctx: &mut RuntimeActionContext<'_>,
    app_type: AppType,
    provider_id: String,
) -> Result<(), AppError> {
    mutate_route_mode_group(ctx, &app_type, |group| {
        if let Some(member) = group
            .members
            .iter_mut()
            .find(|member| member.provider_id == provider_id)
        {
            member.enabled = !member.enabled;
            return Ok(true);
        }
        Ok(false)
    })
}

pub(super) fn set_route_mode_member_weight(
    ctx: &mut RuntimeActionContext<'_>,
    app_type: AppType,
    provider_id: String,
    weight: u32,
) -> Result<(), AppError> {
    mutate_route_mode_group(ctx, &app_type, |group| {
        if let Some(member) = group
            .members
            .iter_mut()
            .find(|member| member.provider_id == provider_id)
        {
            member.weight = weight;
            return Ok(true);
        }
        Ok(false)
    })
}

pub(super) fn set_route_mode_member_mappings(
    ctx: &mut RuntimeActionContext<'_>,
    app_type: AppType,
    provider_id: String,
    mappings: BTreeMap<String, String>,
) -> Result<(), AppError> {
    mutate_route_mode_group(ctx, &app_type, |group| {
        if let Some(member) = group
            .members
            .iter_mut()
            .find(|member| member.provider_id == provider_id)
        {
            member.model_mappings = mappings;
            return Ok(true);
        }
        Ok(false)
    })
}

/// Loads the stored group, applies `mutate`, and saves only when something
/// changed. Clamps the route-mode cursor when the member list shrank so it
/// never points past the last row, and warns when a saved group has no
/// enabled member left while the mode stays on (requests would fail).
fn mutate_route_mode_group(
    ctx: &mut RuntimeActionContext<'_>,
    app_type: &AppType,
    mutate: impl FnOnce(&mut RouteModeGroup) -> Result<bool, AppError>,
) -> Result<(), AppError> {
    let state = load_state()?;
    let mut group = state.db.get_route_mode_group(app_type.as_str())?;
    let changed = mutate(&mut group)?;
    if changed {
        state.db.save_route_mode_group(app_type.as_str(), &group)?;
        ctx.app.route_mode_idx = ctx.app.route_mode_idx.min(group.members.len());
        if group.enabled && !group.members.iter().any(|member| member.enabled) {
            ctx.app.push_toast(
                crate::t!(
                    "Route mode is on but no member is enabled; requests will fail until one is enabled.",
                    "路由模式已开启但没有启用的成员；在启用成员之前请求都会失败。"
                ),
                ToastKind::Warning,
            );
        }
    }

    *ctx.data = UiData::load(&ctx.app.app_type)?;
    Ok(())
}
