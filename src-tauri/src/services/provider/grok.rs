use super::PreparedLiveWrite;
use crate::app_config::{AppType, MultiAppConfig};
use crate::error::AppError;
use crate::provider::Provider;

pub(super) fn prepare_switch_grokbuild(
    config: &mut MultiAppConfig,
    provider_id: &str,
    _effective_current_provider: Option<&str>,
) -> Result<Provider, AppError> {
    let manager = config
        .get_manager_mut(&AppType::GrokBuild)
        .ok_or_else(|| AppError::Config("Grok Build manager not found".into()))?;
    let provider = manager
        .providers
        .get(provider_id)
        .cloned()
        .ok_or_else(|| AppError::InvalidInput(format!("Provider not found: {provider_id}")))?;
    manager.current = provider_id.to_string();
    Ok(provider)
}

pub(super) fn prepare_grok_live_write(provider: &Provider) -> Result<PreparedLiveWrite, AppError> {
    if !crate::sync_policy::should_sync_live(&AppType::GrokBuild) {
        return Ok(PreparedLiveWrite::Noop);
    }
    // 只做静态校验与输入提取；durable Written 的读取在应用锁内进行，
    // 避免跨进程并发切换时用过期的清理记录覆盖新记录。
    let inputs = crate::grok_config::GrokPatchInputs::of(provider)?;
    Ok(PreparedLiveWrite::GrokBuild { inputs })
}

pub(super) fn apply_grok_live_write(
    prepared: &PreparedLiveWrite,
    db_target: Option<(&crate::Database, Option<&str>)>,
) -> Result<(), AppError> {
    let PreparedLiveWrite::GrokBuild { inputs } = prepared else {
        return Ok(());
    };
    crate::grok_config::apply_provider_patch(inputs, db_target)
}
