use std::time::Instant;

use crate::{app_config::AppType, error::AppError, provider::Provider};

use super::types::{HealthStatus, StreamCheckConfig, StreamCheckResult};

/// 流式健康检查服务
pub struct StreamCheckService;

impl StreamCheckService {
    /// 执行流式健康检查（带重试）
    pub async fn check_with_retry(
        app_type: &AppType,
        provider: &Provider,
        config: &StreamCheckConfig,
    ) -> Result<StreamCheckResult, AppError> {
        let effective_config = Self::merge_provider_config(provider, config);
        let mut last_result = None;

        for attempt in 0..=effective_config.max_retries {
            let result = Self::check_once(app_type, provider, &effective_config).await;

            match &result {
                Ok(r) if r.success => {
                    return Ok(StreamCheckResult {
                        retry_count: attempt,
                        ..r.clone()
                    });
                }
                Ok(r) => {
                    if Self::should_retry(&r.message) && attempt < effective_config.max_retries {
                        last_result = Some(r.clone());
                        continue;
                    }
                    return Ok(StreamCheckResult {
                        retry_count: attempt,
                        ..r.clone()
                    });
                }
                Err(err) => {
                    if Self::should_retry(&err.to_string())
                        && attempt < effective_config.max_retries
                    {
                        continue;
                    }
                    return Err(AppError::Message(err.to_string()));
                }
            }
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
                claude_model: cfg
                    .test_model
                    .clone()
                    .unwrap_or_else(|| global_config.claude_model.clone()),
                codex_model: cfg
                    .test_model
                    .clone()
                    .unwrap_or_else(|| global_config.codex_model.clone()),
                gemini_model: cfg
                    .test_model
                    .clone()
                    .unwrap_or_else(|| global_config.gemini_model.clone()),
                test_prompt: cfg
                    .test_prompt
                    .clone()
                    .unwrap_or_else(|| global_config.test_prompt.clone()),
            },
            None => global_config.clone(),
        }
    }

    pub(crate) async fn check_once(
        app_type: &AppType,
        provider: &Provider,
        config: &StreamCheckConfig,
    ) -> Result<StreamCheckResult, AppError> {
        if matches!(app_type, AppType::OpenClaw) {
            return Err(AppError::Message("OpenClaw 暂不支持流式检查".to_string()));
        }

        let start = Instant::now();
        let base_url = Self::extract_base_url(provider, app_type)?;
        let auth = Self::extract_auth(provider, app_type, &base_url).await?;
        let client = Self::build_client_for_provider(provider)?;
        let request_timeout = std::time::Duration::from_secs(config.timeout_secs);
        let model_to_test = Self::resolve_test_model(app_type, provider, config);
        let test_prompt = &config.test_prompt;

        let result = match app_type {
            AppType::Claude => {
                Self::check_claude_stream(
                    &client,
                    &base_url,
                    &auth,
                    &model_to_test,
                    test_prompt,
                    request_timeout,
                    provider,
                )
                .await
            }
            AppType::Codex => {
                Self::check_codex_stream(
                    &client,
                    &base_url,
                    &auth,
                    &model_to_test,
                    test_prompt,
                    request_timeout,
                )
                .await
            }
            AppType::Gemini => {
                Self::check_gemini_stream(
                    &client,
                    &base_url,
                    &auth,
                    &model_to_test,
                    test_prompt,
                    request_timeout,
                )
                .await
            }
            AppType::OpenCode => {
                Self::check_codex_stream(
                    &client,
                    &base_url,
                    &auth,
                    &model_to_test,
                    test_prompt,
                    request_timeout,
                )
                .await
            }
            AppType::OpenClaw => unreachable!("OpenClaw should return unsupported earlier"),
        };

        let response_time = start.elapsed().as_millis() as u64;
        let tested_at = chrono::Utc::now().timestamp();

        match result {
            Ok((status_code, model)) => Ok(StreamCheckResult {
                status: Self::determine_status(response_time, config.degraded_threshold_ms),
                success: true,
                message: "Check succeeded".to_string(),
                response_time_ms: Some(response_time),
                http_status: Some(status_code),
                model_used: model,
                tested_at,
                retry_count: 0,
            }),
            Err(err) => Ok(StreamCheckResult {
                status: HealthStatus::Failed,
                success: false,
                message: err.to_string(),
                response_time_ms: Some(response_time),
                http_status: None,
                model_used: String::new(),
                tested_at,
                retry_count: 0,
            }),
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
}
