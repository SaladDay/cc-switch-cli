use reqwest::Client;
use serde_json::Value;
use std::collections::HashSet;
use std::time::Duration;

use crate::error::AppError;

use super::ProviderService;

impl ProviderService {
    /// 尝试从远端拉取模型列表
    pub async fn fetch_provider_models(
        base_url: &str,
        api_key: Option<&str>,
    ) -> Result<Vec<String>, AppError> {
        let base_url = base_url.trim().trim_end_matches('/');
        if base_url.is_empty() {
            return Err(AppError::localized(
                "fetch.invalid_url",
                "URL 不能为空",
                "URL cannot be empty",
            ));
        }

        let mut candidate_urls = Vec::new();

        // 如果用户直接填了 /v1/models 或者 /models，我们就直接用
        if base_url.ends_with("/models") {
            candidate_urls.push(base_url.to_string());
        } else {
            // 智能适配：如果没带 /models，尝试追加
            candidate_urls.push(format!("{}/models", base_url));
            if !base_url.ends_with("/v1") && !base_url.ends_with("/v1beta") {
                candidate_urls.push(format!("{}/v1/models", base_url));
            }
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| AppError::Message(e.to_string()))?;

        let mut last_err_zh = None;
        let mut last_err_en = None;

        for url in candidate_urls {
            let mut req = client.get(&url);
            if let Some(key) = api_key {
                let key = key.trim();
                // 同时添加 OpenAI 的 Bearer 和 Anthropic 的 x-api-key 格式，代理服务通常会接受其中之一
                req = req
                    .header("Authorization", format!("Bearer {}", key))
                    .header("x-api-key", key);
            }

            match req.send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        if let Ok(json) = resp.json::<Value>().await {
                            let mut models = Vec::new();

                            // 测试格式 1: OpenAI 兼容格式 {"data": [{"id": "gpt-4o"}]}
                            if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
                                for item in data {
                                    if let Some(id) = item.get("id").and_then(|i| i.as_str()) {
                                        models.push(id.to_string());
                                    }
                                }
                            }

                            // 测试格式 2: Gemini 格式 {"models": [{"name": "models/gemini-pro"}]}
                            if models.is_empty() {
                                if let Some(data) = json.get("models").and_then(|d| d.as_array()) {
                                    for item in data {
                                        if let Some(name) =
                                            item.get("name").and_then(|i| i.as_str())
                                        {
                                            let id = name.strip_prefix("models/").unwrap_or(name);
                                            models.push(id.to_string());
                                        }
                                    }
                                }
                            }

                            // 测试格式 3: 直接的数组格式 [{"id": "llama-3"}]
                            if models.is_empty() {
                                if let Some(arr) = json.as_array() {
                                    for item in arr {
                                        if let Some(id) = item.get("id").and_then(|i| i.as_str()) {
                                            models.push(id.to_string());
                                        }
                                    }
                                }
                            }

                            if !models.is_empty() {
                                // 保序去重，避免非相邻重复项残留。
                                let mut seen = HashSet::new();
                                models.retain(|model| seen.insert(model.clone()));
                                return Ok(models);
                            } else {
                                last_err_zh =
                                    Some(format!("未能在响应中找到模型列表 (URL: {})", url));
                                last_err_en =
                                    Some(format!("No model list found in response (URL: {})", url));
                            }
                        } else {
                            last_err_zh = Some(format!("无法解析 JSON 响应 (URL: {})", url));
                            last_err_en =
                                Some(format!("Failed to parse JSON response (URL: {})", url));
                        }
                    } else {
                        let err = format!("HTTP {} (URL: {})", resp.status(), url);
                        last_err_zh = Some(err.clone());
                        last_err_en = Some(err);
                    }
                }
                Err(e) => {
                    let err = e.to_string();
                    last_err_zh = Some(err.clone());
                    last_err_en = Some(err);
                }
            }
        }

        let err_zh = last_err_zh.unwrap_or_else(|| "未知错误".to_string());
        let err_en = last_err_en.unwrap_or_else(|| "Unknown error".to_string());
        Err(AppError::localized(
            "fetch.failed",
            format!("拉取失败: {}", err_zh),
            format!("Fetch failed: {}", err_en),
        ))
    }
}
