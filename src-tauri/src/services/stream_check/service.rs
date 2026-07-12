use std::time::Instant;

use reqwest::header::{HeaderValue, USER_AGENT};

use crate::{app_config::AppType, error::AppError, provider::Provider};

use super::types::{HealthStatus, StreamCheckConfig, StreamCheckResult};

/// 连通性检查服务
pub struct StreamCheckService;

impl StreamCheckService {
    /// 执行连通性检查（仅对超时类失败重试）
    pub async fn check_with_retry(
        app_type: &AppType,
        provider: &Provider,
        config: &StreamCheckConfig,
    ) -> Result<StreamCheckResult, AppError> {
        let effective_config = Self::merge_provider_config(provider, config);
        let mut last_result = None;

        for attempt in 0..=effective_config.max_retries {
            let result = Self::check_once(app_type, provider, &effective_config).await?;

            if result.success {
                return Ok(StreamCheckResult {
                    retry_count: attempt,
                    ..result
                });
            }

            if Self::should_retry(&result.message) && attempt < effective_config.max_retries {
                last_result = Some(result);
                continue;
            }

            return Ok(StreamCheckResult {
                retry_count: attempt,
                ..result
            });
        }

        Ok(last_result.unwrap_or_else(|| StreamCheckResult {
            status: HealthStatus::Failed,
            success: false,
            message: "Check failed".to_string(),
            response_time_ms: None,
            http_status: None,
            model_used: String::new(),
            tested_at: chrono::Utc::now().timestamp(),
            retry_count: effective_config.max_retries,
            error_category: None,
        }))
    }

    pub(crate) fn merge_provider_config(
        provider: &Provider,
        global_config: &StreamCheckConfig,
    ) -> StreamCheckConfig {
        let test_config = provider
            .meta
            .as_ref()
            .and_then(|meta| meta.test_config.as_ref())
            .filter(|cfg| cfg.enabled);

        match test_config {
            Some(cfg) => StreamCheckConfig {
                timeout_secs: cfg.timeout_secs.unwrap_or(global_config.timeout_secs),
                max_retries: cfg.max_retries.unwrap_or(global_config.max_retries),
                degraded_threshold_ms: cfg
                    .degraded_threshold_ms
                    .unwrap_or(global_config.degraded_threshold_ms),
            },
            None => global_config.clone(),
        }
    }

    pub(crate) async fn check_once(
        app_type: &AppType,
        provider: &Provider,
        config: &StreamCheckConfig,
    ) -> Result<StreamCheckResult, AppError> {
        let start = Instant::now();
        let base_url = Self::extract_base_url(provider, app_type)?;
        let client = Self::build_client_for_provider(provider)?;
        let timeout = std::time::Duration::from_secs(config.timeout_secs);
        let custom_user_agent = Self::custom_user_agent(provider);

        let result = Self::probe_reachability(&client, &base_url, timeout, custom_user_agent).await;
        let response_time = start.elapsed().as_millis() as u64;

        Ok(Self::build_result(
            result,
            response_time,
            config.degraded_threshold_ms,
        ))
    }

    /// 轻量可达性探测：GET `base_url`，收到任意 HTTP 响应即可达。
    ///
    /// `send()` 在收到响应头时即返回，reqwest 对 4xx/5xx 仍返回 Ok。
    async fn probe_reachability(
        client: &reqwest::Client,
        base_url: &str,
        timeout: std::time::Duration,
        custom_user_agent: Option<HeaderValue>,
    ) -> Result<u16, AppError> {
        let url = base_url.trim();
        if url.is_empty() {
            return Err(AppError::Message("base_url 为空".to_string()));
        }

        let mut request = client
            .get(url)
            .timeout(timeout)
            .header("accept", "*/*")
            .header("accept-encoding", "identity");
        if let Some(user_agent) = custom_user_agent {
            request = request.header(USER_AGENT, user_agent);
        }

        let response = request.send().await.map_err(Self::map_request_error)?;

        Ok(response.status().as_u16())
    }

    pub(crate) fn build_result(
        result: Result<u16, AppError>,
        response_time: u64,
        degraded_threshold_ms: u64,
    ) -> StreamCheckResult {
        let tested_at = chrono::Utc::now().timestamp();
        match result {
            Ok(status) => StreamCheckResult {
                status: Self::determine_status(response_time, degraded_threshold_ms),
                success: true,
                message: "Reachable".to_string(),
                response_time_ms: Some(response_time),
                http_status: Some(status),
                model_used: String::new(),
                tested_at,
                retry_count: 0,
                error_category: None,
            },
            Err(err) => StreamCheckResult {
                status: HealthStatus::Failed,
                success: false,
                message: err.to_string(),
                response_time_ms: Some(response_time),
                http_status: None,
                model_used: String::new(),
                tested_at,
                retry_count: 0,
                error_category: None,
            },
        }
    }

    pub(crate) fn determine_status(latency_ms: u64, threshold: u64) -> HealthStatus {
        if latency_ms <= threshold {
            HealthStatus::Operational
        } else {
            HealthStatus::Degraded
        }
    }

    pub(crate) fn should_retry(msg: &str) -> bool {
        let lower = msg.to_lowercase();
        lower.contains("timeout") || lower.contains("abort") || lower.contains("timed out")
    }

    pub(crate) fn map_request_error(err: reqwest::Error) -> AppError {
        if err.is_timeout() {
            AppError::Message("Request timeout".to_string())
        } else if err.is_connect() {
            AppError::Message(format!("Connection failed: {err}"))
        } else {
            AppError::Message(err.to_string())
        }
    }

    fn custom_user_agent(provider: &Provider) -> Option<HeaderValue> {
        provider
            .meta
            .as_ref()
            .and_then(|meta| meta.custom_user_agent_header().ok().flatten())
    }
}
