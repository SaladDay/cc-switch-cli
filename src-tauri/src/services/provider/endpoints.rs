use std::time::{SystemTime, UNIX_EPOCH};

use crate::app_config::AppType;
use crate::error::AppError;
use crate::provider::ProviderMeta;
use crate::settings::CustomEndpoint;
use crate::store::AppState;

use super::ProviderService;

impl ProviderService {
    /// 获取自定义端点列表
    pub fn get_custom_endpoints(
        state: &AppState,
        app_type: AppType,
        provider_id: &str,
    ) -> Result<Vec<CustomEndpoint>, AppError> {
        let cfg = state.config.read().map_err(AppError::from)?;
        let manager = cfg
            .get_manager(&app_type)
            .ok_or_else(|| Self::app_not_found(&app_type))?;

        let Some(provider) = manager.providers.get(provider_id) else {
            return Ok(vec![]);
        };
        let Some(meta) = provider.meta.as_ref() else {
            return Ok(vec![]);
        };
        if meta.custom_endpoints.is_empty() {
            return Ok(vec![]);
        }

        let mut result: Vec<_> = meta.custom_endpoints.values().cloned().collect();
        result.sort_by(|a, b| b.added_at.cmp(&a.added_at));
        Ok(result)
    }

    /// 新增自定义端点
    pub fn add_custom_endpoint(
        state: &AppState,
        app_type: AppType,
        provider_id: &str,
        url: String,
    ) -> Result<(), AppError> {
        let permit =
            crate::services::state_coordination::acquire_ordinary_mutation_permit_blocking()
                .map_err(AppError::Message)?;
        let normalized = url.trim().trim_end_matches('/').to_string();
        if normalized.is_empty() {
            return Err(AppError::localized(
                "provider.endpoint.url_required",
                "URL 不能为空",
                "URL cannot be empty",
            ));
        }

        {
            let mut cfg = state.config.write().map_err(AppError::from)?;
            let manager = cfg
                .get_manager_mut(&app_type)
                .ok_or_else(|| Self::app_not_found(&app_type))?;
            let provider = manager.providers.get_mut(provider_id).ok_or_else(|| {
                AppError::localized(
                    "provider.not_found",
                    format!("供应商不存在: {provider_id}"),
                    format!("Provider not found: {provider_id}"),
                )
            })?;
            let meta = provider.meta.get_or_insert_with(ProviderMeta::default);

            let endpoint = CustomEndpoint {
                url: normalized.clone(),
                added_at: now_millis(),
                last_used: None,
            };
            meta.custom_endpoints.insert(normalized, endpoint);
        }

        state.save_with_permit(&permit)?;
        Ok(())
    }

    /// 删除自定义端点
    pub fn remove_custom_endpoint(
        state: &AppState,
        app_type: AppType,
        provider_id: &str,
        url: String,
    ) -> Result<(), AppError> {
        let permit =
            crate::services::state_coordination::acquire_ordinary_mutation_permit_blocking()
                .map_err(AppError::Message)?;
        let normalized = url.trim().trim_end_matches('/').to_string();

        {
            let mut cfg = state.config.write().map_err(AppError::from)?;
            if let Some(manager) = cfg.get_manager_mut(&app_type) {
                if let Some(provider) = manager.providers.get_mut(provider_id) {
                    if let Some(meta) = provider.meta.as_mut() {
                        meta.custom_endpoints.remove(&normalized);
                    }
                }
            }
        }

        state.save_with_permit(&permit)?;
        Ok(())
    }

    /// 更新端点最后使用时间
    pub fn update_endpoint_last_used(
        state: &AppState,
        app_type: AppType,
        provider_id: &str,
        url: String,
    ) -> Result<(), AppError> {
        let permit =
            crate::services::state_coordination::acquire_ordinary_mutation_permit_blocking()
                .map_err(AppError::Message)?;
        let normalized = url.trim().trim_end_matches('/').to_string();

        {
            let mut cfg = state.config.write().map_err(AppError::from)?;
            if let Some(manager) = cfg.get_manager_mut(&app_type) {
                if let Some(provider) = manager.providers.get_mut(provider_id) {
                    if let Some(meta) = provider.meta.as_mut() {
                        if let Some(endpoint) = meta.custom_endpoints.get_mut(&normalized) {
                            endpoint.last_used = Some(now_millis());
                        }
                    }
                }
            }
        }

        state.save_with_permit(&permit)?;
        Ok(())
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
