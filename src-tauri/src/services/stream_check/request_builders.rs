use futures::StreamExt;
use reqwest::Client;
use serde_json::json;

use crate::error::AppError;
use crate::provider::Provider;
use crate::proxy::providers::get_claude_api_format;

use super::service::StreamCheckService;
use super::types::{AuthInfo, AuthStrategy};

impl StreamCheckService {
    pub(crate) async fn check_claude_stream(
        client: &Client,
        base_url: &str,
        auth: &AuthInfo,
        model: &str,
        test_prompt: &str,
        timeout: std::time::Duration,
        provider: &Provider,
    ) -> Result<(u16, String), AppError> {
        let base = base_url.trim_end_matches('/');
        let api_format = get_claude_api_format(provider);
        let is_openai_compatible = matches!(api_format, "openai_chat" | "openai_responses");
        let url = match api_format {
            "openai_chat" => {
                if base.ends_with("/v1") {
                    format!("{base}/chat/completions")
                } else {
                    format!("{base}/v1/chat/completions")
                }
            }
            "openai_responses" => {
                if base.ends_with("/v1") {
                    format!("{base}/responses")
                } else {
                    format!("{base}/v1/responses")
                }
            }
            _ => {
                if base.ends_with("/v1") {
                    format!("{base}/messages?beta=true")
                } else {
                    format!("{base}/v1/messages?beta=true")
                }
            }
        };

        let stream_include_usage = provider
            .settings_config
            .get("stream_include_usage")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let body = if api_format == "openai_responses" {
            json!({
                "model": model,
                "max_output_tokens": 1,
                "input": [{
                    "role": "user",
                    "content": [{ "type": "input_text", "text": test_prompt }]
                }],
                "stream": true,
            })
        } else {
            let mut b = json!({
                "model": model,
                "max_tokens": 1,
                "messages": [{ "role": "user", "content": test_prompt }],
                "stream": true,
            });
            if stream_include_usage {
                b["stream_options"] = json!({"include_usage": true});
            }
            b
        };

        let mut request = client
            .post(&url)
            .header("authorization", format!("Bearer {}", auth.api_key));

        let response = if is_openai_compatible {
            request
                .header("content-type", "application/json")
                .header("accept", "application/json")
                .timeout(timeout)
                .json(&body)
                .send()
                .await
                .map_err(Self::map_request_error)?
        } else {
            let os_name = Self::get_os_name();
            let arch_name = Self::get_arch_name();

            if auth.strategy == AuthStrategy::Anthropic {
                request = request.header("x-api-key", &auth.api_key);
            }

            request
                .header("anthropic-version", "2023-06-01")
                .header(
                    "anthropic-beta",
                    "claude-code-20250219,interleaved-thinking-2025-05-14",
                )
                .header("anthropic-dangerous-direct-browser-access", "true")
                .header("content-type", "application/json")
                .header("accept", "application/json")
                .header("accept-encoding", "identity")
                .header("accept-language", "*")
                .header("user-agent", "claude-cli/2.1.2 (external, cli)")
                .header("x-app", "cli")
                .header("x-stainless-lang", "js")
                .header("x-stainless-package-version", "0.70.0")
                .header("x-stainless-os", os_name)
                .header("x-stainless-arch", arch_name)
                .header("x-stainless-runtime", "node")
                .header("x-stainless-runtime-version", "v22.20.0")
                .header("x-stainless-retry-count", "0")
                .header("x-stainless-timeout", "600")
                .header("sec-fetch-mode", "cors")
                .header("connection", "keep-alive")
                .timeout(timeout)
                .json(&body)
                .send()
                .await
                .map_err(Self::map_request_error)?
        };

        let status = response.status().as_u16();
        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(AppError::Message(format!("HTTP {status}: {error_text}")));
        }

        let mut stream = response.bytes_stream();
        if let Some(chunk) = stream.next().await {
            match chunk {
                Ok(_) => Ok((status, model.to_string())),
                Err(err) => Err(AppError::Message(format!("Stream read failed: {err}"))),
            }
        } else {
            Err(AppError::Message("No response data received".to_string()))
        }
    }

    pub(crate) async fn check_codex_stream(
        client: &Client,
        base_url: &str,
        auth: &AuthInfo,
        model: &str,
        test_prompt: &str,
        timeout: std::time::Duration,
    ) -> Result<(u16, String), AppError> {
        let base = base_url.trim_end_matches('/');
        let urls = if base.ends_with("/v1") {
            vec![format!("{base}/responses")]
        } else {
            vec![format!("{base}/responses"), format!("{base}/v1/responses")]
        };

        let (actual_model, reasoning_effort) = Self::parse_model_with_effort(model);
        let os_name = Self::get_os_name();
        let arch_name = Self::get_arch_name();

        let mut body = json!({
            "model": actual_model,
            "input": [{ "role": "user", "content": test_prompt }],
            "stream": true,
        });

        if let Some(effort) = reasoning_effort {
            body["reasoning"] = json!({ "effort": effort });
        }

        for (index, url) in urls.iter().enumerate() {
            let response = client
                .post(url)
                .header("authorization", format!("Bearer {}", auth.api_key))
                .header("content-type", "application/json")
                .header("accept", "text/event-stream")
                .header("accept-encoding", "identity")
                .header(
                    "user-agent",
                    format!("codex_cli_rs/0.80.0 ({os_name} 15.7.2; {arch_name}) Terminal"),
                )
                .header("originator", "codex_cli_rs")
                .timeout(timeout)
                .json(&body)
                .send()
                .await
                .map_err(Self::map_request_error)?;

            let status = response.status().as_u16();
            if !response.status().is_success() {
                let error_text = response.text().await.unwrap_or_default();
                if index == 0 && status == 404 && urls.len() > 1 {
                    continue;
                }
                return Err(AppError::Message(format!("HTTP {status}: {error_text}")));
            }

            let mut stream = response.bytes_stream();
            if let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(_) => return Ok((status, actual_model.clone())),
                    Err(err) => {
                        return Err(AppError::Message(format!("Stream read failed: {err}")));
                    }
                }
            }

            return Err(AppError::Message("No response data received".to_string()));
        }

        Err(AppError::Message(
            "No valid Codex responses endpoint found".to_string(),
        ))
    }

    pub(crate) async fn check_gemini_stream(
        client: &Client,
        base_url: &str,
        auth: &AuthInfo,
        model: &str,
        test_prompt: &str,
        timeout: std::time::Duration,
    ) -> Result<(u16, String), AppError> {
        let base = base_url.trim_end_matches('/');
        let url = if base.contains("/v1beta") || base.contains("/v1/") {
            format!("{base}/models/{model}:streamGenerateContent?alt=sse")
        } else {
            format!("{base}/v1beta/models/{model}:streamGenerateContent?alt=sse")
        };

        let body = json!({
            "contents": [{
                "role": "user",
                "parts": [{ "text": test_prompt }],
            }]
        });

        let request = client
            .post(&url)
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .timeout(timeout)
            .json(&body);

        let request = match auth.strategy {
            AuthStrategy::GoogleOAuth => request
                .header(
                    "authorization",
                    format!(
                        "Bearer {}",
                        auth.access_token
                            .as_deref()
                            .unwrap_or(auth.api_key.as_str())
                    ),
                )
                .header("x-goog-api-client", "GeminiCLI/1.0"),
            _ => request.header("x-goog-api-key", &auth.api_key),
        };

        let response = request.send().await.map_err(Self::map_request_error)?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(AppError::Message(format!("HTTP {status}: {error_text}")));
        }

        let mut stream = response.bytes_stream();
        if let Some(chunk) = stream.next().await {
            match chunk {
                Ok(_) => Ok((status, model.to_string())),
                Err(err) => Err(AppError::Message(format!("Stream read failed: {err}"))),
            }
        } else {
            Err(AppError::Message("No response data received".to_string()))
        }
    }

    pub(crate) fn parse_model_with_effort(model: &str) -> (String, Option<String>) {
        if let Some(pos) = model.find('@').or_else(|| model.find('#')) {
            let actual_model = model[..pos].to_string();
            let effort = model[pos + 1..].to_string();
            if !effort.is_empty() {
                return (actual_model, Some(effort));
            }
        }
        (model.to_string(), None)
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

    pub(crate) fn get_os_name() -> &'static str {
        match std::env::consts::OS {
            "macos" => "MacOS",
            "linux" => "Linux",
            "windows" => "Windows",
            other => other,
        }
    }

    pub(crate) fn get_arch_name() -> &'static str {
        match std::env::consts::ARCH {
            "aarch64" => "arm64",
            "x86_64" => "x86_64",
            "x86" => "x86",
            other => other,
        }
    }
}
