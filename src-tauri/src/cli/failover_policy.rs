use crate::app_config::AppType;
use crate::error::AppError;
use crate::proxy::types::ProxyTakeoverStatus;
use crate::store::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AutoFailoverGate {
    pub(crate) queue_empty: bool,
    pub(crate) proxy_running: bool,
    pub(crate) current_app_takeover_active: bool,
}

impl AutoFailoverGate {
    pub(crate) fn proxy_ready(&self) -> bool {
        self.proxy_running && self.current_app_takeover_active
    }

    pub(crate) fn needs_proxy_bootstrap(&self) -> bool {
        !self.proxy_ready()
    }
}

pub(crate) fn auto_failover_queue_empty_message() -> String {
    crate::t!(
        "Failover queue is empty. Add at least one provider before enabling automatic failover.",
        "故障转移队列为空，无法开启。请先添加至少一个供应商。"
    )
    .to_string()
}

pub(crate) fn auto_failover_proxy_required_message() -> String {
    crate::t!(
        "Automatic failover requires proxy routing for this app. Enable proxy takeover for the app first.",
        "故障转移需要当前应用走代理。请先开启代理并让当前应用接入代理。"
    )
    .to_string()
}

pub(crate) fn auto_failover_queue_empty_error() -> AppError {
    AppError::InvalidInput(auto_failover_queue_empty_message())
}

pub(crate) fn auto_failover_proxy_required_error() -> AppError {
    AppError::InvalidInput(auto_failover_proxy_required_message())
}

pub(crate) fn ensure_auto_failover_queue_ready(gate: &AutoFailoverGate) -> Result<(), AppError> {
    if gate.queue_empty {
        Err(auto_failover_queue_empty_error())
    } else {
        Ok(())
    }
}

pub(crate) fn ensure_auto_failover_ready(gate: &AutoFailoverGate) -> Result<(), AppError> {
    ensure_auto_failover_queue_ready(gate)?;
    if gate.needs_proxy_bootstrap() {
        Err(auto_failover_proxy_required_error())
    } else {
        Ok(())
    }
}

pub(crate) async fn inspect_auto_failover_gate(
    state: &AppState,
    app_type: &AppType,
) -> Result<AutoFailoverGate, AppError> {
    let queue_empty = state.db.get_failover_queue(app_type.as_str())?.is_empty();
    let status = state.proxy_service.get_status().await;
    let takeover = state
        .proxy_service
        .get_takeover_status()
        .await
        .map_err(AppError::Message)?;

    Ok(AutoFailoverGate {
        queue_empty,
        proxy_running: status.running,
        current_app_takeover_active: takeover_enabled_for(&takeover, app_type),
    })
}

fn takeover_enabled_for(takeover: &ProxyTakeoverStatus, app_type: &AppType) -> bool {
    match app_type {
        AppType::Claude => takeover.claude,
        AppType::Codex => takeover.codex,
        AppType::Gemini => takeover.gemini,
        AppType::OpenCode | AppType::Hermes | AppType::OpenClaw | AppType::Pi => false,
    }
}
