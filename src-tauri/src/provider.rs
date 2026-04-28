use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

// SSOT 模式：不再写供应商副本文件

/// 供应商结构体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub name: String,
    #[serde(rename = "settingsConfig")]
    pub settings_config: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "websiteUrl")]
    pub website_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "createdAt")]
    pub created_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "sortIndex")]
    pub sort_index: Option<usize>,
    /// 备注信息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// 供应商元数据（不写入 live 配置，仅存于 ~/.cc-switch/config.json）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<ProviderMeta>,
    /// 图标名称（如 "openai", "anthropic"）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// 图标颜色（Hex 格式，如 "#00A67E"）
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "iconColor")]
    pub icon_color: Option<String>,
    /// 是否加入故障转移队列
    #[serde(default)]
    #[serde(rename = "inFailoverQueue")]
    pub in_failover_queue: bool,
}

impl Provider {
    /// 从现有ID创建供应商
    pub fn with_id(
        id: String,
        name: String,
        settings_config: Value,
        website_url: Option<String>,
    ) -> Self {
        Self {
            id,
            name,
            settings_config,
            website_url,
            category: None,
            created_at: None,
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        }
    }
}

/// 供应商管理器
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderManager {
    pub providers: IndexMap<String, Provider>,
    pub current: String,
}

/// 用量查询脚本配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageScript {
    pub enabled: bool,
    pub language: String,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    /// 用量查询专用的 API Key（通用模板使用）
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "apiKey")]
    pub api_key: Option<String>,
    /// 用量查询专用的 Base URL（通用和 NewAPI 模板使用）
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "baseUrl")]
    pub base_url: Option<String>,
    /// 访问令牌（用于需要登录的接口，NewAPI 模板使用）
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "accessToken")]
    pub access_token: Option<String>,
    /// 用户ID（用于需要用户标识的接口，NewAPI 模板使用）
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "userId")]
    pub user_id: Option<String>,
    /// 模板类型（用于后端判断验证规则）
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "templateType")]
    pub template_type: Option<String>,
    /// 自动查询间隔（单位：分钟，0 表示禁用自动查询）
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "autoQueryInterval")]
    pub auto_query_interval: Option<u64>,
}

/// 用量数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageData {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "planName")]
    pub plan_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "isValid")]
    pub is_valid: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "invalidMessage")]
    pub invalid_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// 用量查询结果（支持多套餐）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Vec<UsageData>>, // 支持返回多个套餐
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 供应商单独的模型测试配置
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderTestConfig {
    /// 是否启用单独配置（false 时使用全局配置）
    #[serde(default)]
    pub enabled: bool,
    /// 测试用的模型名称（覆盖全局配置）
    #[serde(rename = "testModel", skip_serializing_if = "Option::is_none")]
    pub test_model: Option<String>,
    /// 超时时间（秒）
    #[serde(rename = "timeoutSecs", skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// 测试提示词
    #[serde(rename = "testPrompt", skip_serializing_if = "Option::is_none")]
    pub test_prompt: Option<String>,
    /// 降级阈值（毫秒）
    #[serde(
        rename = "degradedThresholdMs",
        skip_serializing_if = "Option::is_none"
    )]
    pub degraded_threshold_ms: Option<u64>,
    /// 最大重试次数
    #[serde(rename = "maxRetries", skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
}

/// 供应商单独的代理配置
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderProxyConfig {
    /// 是否启用单独配置（false 时使用全局/系统代理）
    #[serde(default)]
    pub enabled: bool,
    /// 代理类型：http, https, socks5
    #[serde(rename = "proxyType", skip_serializing_if = "Option::is_none")]
    pub proxy_type: Option<String>,
    /// 代理主机
    #[serde(rename = "proxyHost", skip_serializing_if = "Option::is_none")]
    pub proxy_host: Option<String>,
    /// 代理端口
    #[serde(rename = "proxyPort", skip_serializing_if = "Option::is_none")]
    pub proxy_port: Option<u16>,
    /// 代理用户名（可选）
    #[serde(rename = "proxyUsername", skip_serializing_if = "Option::is_none")]
    pub proxy_username: Option<String>,
    /// 代理密码（可选）
    #[serde(rename = "proxyPassword", skip_serializing_if = "Option::is_none")]
    pub proxy_password: Option<String>,
}

/// 认证绑定来源
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AuthBindingSource {
    /// 从 provider 自身配置读取认证信息（默认）
    #[default]
    ProviderConfig,
    /// 使用托管账号认证（如 GitHub Copilot OAuth）
    ManagedAccount,
}

/// 通用认证绑定
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuthBinding {
    /// 认证来源
    #[serde(default)]
    pub source: AuthBindingSource,
    /// 托管认证供应商标识（如 github_copilot）
    #[serde(rename = "authProvider", skip_serializing_if = "Option::is_none")]
    pub auth_provider: Option<String>,
    /// 托管账号 ID；为空表示跟随该认证供应商的默认账号
    #[serde(rename = "accountId", skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

/// 供应商元数据
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderMeta {
    /// 是否在写入 live 配置时合并通用配置片段
    #[serde(
        rename = "commonConfigEnabled",
        alias = "applyCommonConfig",
        skip_serializing_if = "Option::is_none"
    )]
    pub apply_common_config: Option<bool>,
    /// Codex 官方供应商标记（官方无需填写 API Key，使用 codex login 凭证）
    #[serde(rename = "codexOfficial", skip_serializing_if = "Option::is_none")]
    pub codex_official: Option<bool>,
    /// 自定义端点列表（按 URL 去重存储）
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub custom_endpoints: HashMap<String, crate::settings::CustomEndpoint>,
    /// 用量查询脚本配置
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_script: Option<UsageScript>,
    /// 请求地址管理：测速后自动选择最佳端点
    #[serde(rename = "endpointAutoSelect", skip_serializing_if = "Option::is_none")]
    pub endpoint_auto_select: Option<bool>,
    /// 合作伙伴标记（前端使用 isPartner，保持字段名一致）
    #[serde(rename = "isPartner", skip_serializing_if = "Option::is_none")]
    pub is_partner: Option<bool>,
    /// 合作伙伴促销 key，用于识别 PackyCode 等特殊供应商
    #[serde(
        rename = "partnerPromotionKey",
        skip_serializing_if = "Option::is_none"
    )]
    pub partner_promotion_key: Option<String>,
    /// 成本倍数（用于计算实际成本）
    #[serde(rename = "costMultiplier", skip_serializing_if = "Option::is_none")]
    pub cost_multiplier: Option<String>,
    /// 计费模式来源（response/request）
    #[serde(rename = "pricingModelSource", skip_serializing_if = "Option::is_none")]
    pub pricing_model_source: Option<String>,
    /// 每日消费限额（USD）
    #[serde(rename = "limitDailyUsd", skip_serializing_if = "Option::is_none")]
    pub limit_daily_usd: Option<String>,
    /// 每月消费限额（USD）
    #[serde(rename = "limitMonthlyUsd", skip_serializing_if = "Option::is_none")]
    pub limit_monthly_usd: Option<String>,
    /// 供应商单独的模型测试配置
    #[serde(rename = "testConfig", skip_serializing_if = "Option::is_none")]
    pub test_config: Option<ProviderTestConfig>,
    /// 供应商单独的代理配置
    #[serde(rename = "proxyConfig", skip_serializing_if = "Option::is_none")]
    pub proxy_config: Option<ProviderProxyConfig>,
    /// Claude API 格式（仅 Claude 供应商使用）
    /// - "anthropic": 原生 Anthropic Messages API，直接透传
    /// - "openai_chat": OpenAI Chat Completions 格式，需要转换
    /// - "openai_responses": OpenAI Responses API 格式，需要转换
    #[serde(rename = "apiFormat", skip_serializing_if = "Option::is_none")]
    pub api_format: Option<String>,
    /// OpenAI 兼容端点使用的 prompt cache key。
    #[serde(rename = "promptCacheKey", skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
    /// 通用认证绑定（provider_config / managed_account）
    #[serde(rename = "authBinding", skip_serializing_if = "Option::is_none")]
    pub auth_binding: Option<AuthBinding>,
    /// Claude 认证字段名（"ANTHROPIC_AUTH_TOKEN" 或 "ANTHROPIC_API_KEY"）
    #[serde(rename = "apiKeyField", skip_serializing_if = "Option::is_none")]
    pub api_key_field: Option<String>,
    /// 累加模式应用中，该 provider 是否已写入 live config。
    /// `None` 表示旧数据/未知状态，`Some(false)` 表示明确仅存在于数据库中。
    #[serde(rename = "liveConfigManaged", skip_serializing_if = "Option::is_none")]
    pub live_config_managed: Option<bool>,
    /// 供应商类型标识（用于特殊供应商检测）
    /// - "github_copilot": GitHub Copilot 供应商
    #[serde(rename = "providerType", skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    /// GitHub Copilot 关联账号 ID（仅 github_copilot 供应商使用）
    /// 用于多账号支持，关联到特定的 GitHub 账号
    #[serde(rename = "githubAccountId", skip_serializing_if = "Option::is_none")]
    pub github_account_id: Option<String>,
}

impl ProviderMeta {
    pub fn managed_account_id_for(&self, auth_provider: &str) -> Option<String> {
        if let Some(binding) = self.auth_binding.as_ref() {
            if binding.source == AuthBindingSource::ManagedAccount
                && binding.auth_provider.as_deref() == Some(auth_provider)
            {
                return binding.account_id.clone();
            }
        }

        if auth_provider == "github_copilot" {
            return self.github_account_id.clone();
        }

        None
    }
}

impl ProviderManager {
    /// 获取所有供应商
    pub fn get_all_providers(&self) -> &IndexMap<String, Provider> {
        &self.providers
    }
}

/// OpenCode 供应商配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenCodeProviderConfig {
    pub npm: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub options: OpenCodeProviderOptions,
    #[serde(default)]
    pub models: HashMap<String, OpenCodeModel>,
}

impl Default for OpenCodeProviderConfig {
    fn default() -> Self {
        Self {
            npm: "@ai-sdk/openai-compatible".to_string(),
            name: None,
            options: OpenCodeProviderOptions::default(),
            models: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OpenCodeProviderOptions {
    #[serde(rename = "baseURL", skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(rename = "apiKey", skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
    #[serde(flatten, default, skip_serializing_if = "HashMap::is_empty")]
    pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenCodeModel {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<OpenCodeModelLimit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<HashMap<String, Value>>,
    #[serde(flatten, default, skip_serializing_if = "HashMap::is_empty")]
    pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OpenCodeModelLimit {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OpenClawProviderConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<OpenClawModelEntry>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
    #[serde(flatten, default, skip_serializing_if = "HashMap::is_empty")]
    pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OpenClawModelEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<OpenClawModelCost>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(flatten, default, skip_serializing_if = "HashMap::is_empty")]
    pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OpenClawModelCost {
    pub input: f64,
    pub output: f64,
    #[serde(flatten, default, skip_serializing_if = "HashMap::is_empty")]
    pub extra: HashMap<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::{AuthBinding, AuthBindingSource, ProviderMeta};

    #[test]
    fn provider_meta_serializes_upstream_common_config_key_and_accepts_legacy_alias() {
        let meta = ProviderMeta {
            apply_common_config: Some(true),
            ..Default::default()
        };

        let serialized = serde_json::to_value(&meta).expect("serialize provider meta");
        assert_eq!(serialized["commonConfigEnabled"], true);
        assert!(
            serialized.get("applyCommonConfig").is_none(),
            "serialization should prefer the upstream key name"
        );

        let deserialized: ProviderMeta = serde_json::from_value(serde_json::json!({
            "applyCommonConfig": false
        }))
        .expect("deserialize legacy alias");
        assert_eq!(deserialized.apply_common_config, Some(false));
    }

    #[test]
    fn provider_meta_managed_account_id_for_prefers_binding_and_falls_back_to_legacy_github_id() {
        let managed_meta = ProviderMeta {
            auth_binding: Some(AuthBinding {
                source: AuthBindingSource::ManagedAccount,
                auth_provider: Some("github_copilot".to_string()),
                account_id: Some("binding-account".to_string()),
            }),
            github_account_id: Some("legacy-account".to_string()),
            ..Default::default()
        };
        assert_eq!(
            managed_meta.managed_account_id_for("github_copilot"),
            Some("binding-account".to_string())
        );

        let legacy_meta = ProviderMeta {
            github_account_id: Some("legacy-account".to_string()),
            ..Default::default()
        };
        assert_eq!(
            legacy_meta.managed_account_id_for("github_copilot"),
            Some("legacy-account".to_string())
        );
        assert_eq!(legacy_meta.managed_account_id_for("other_provider"), None);

        let provider_config_meta = ProviderMeta {
            auth_binding: Some(AuthBinding {
                source: AuthBindingSource::ProviderConfig,
                auth_provider: Some("github_copilot".to_string()),
                account_id: Some("provider-config-account".to_string()),
            }),
            github_account_id: Some("legacy-account".to_string()),
            ..Default::default()
        };
        assert_eq!(
            provider_config_meta.managed_account_id_for("github_copilot"),
            Some("legacy-account".to_string())
        );
    }
}

#[cfg(test)]
mod issue_71_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn meta_with_both_common_config_keys_causes_duplicate_field_error() {
        // When both commonConfigEnabled (rename) and applyCommonConfig (alias)
        // are present, serde rejects as duplicate field.
        let meta_json = json!({
            "commonConfigEnabled": true,
            "applyCommonConfig": false
        });

        let result: Result<ProviderMeta, _> = serde_json::from_value(meta_json);
        assert!(
            result.is_err(),
            "serde should reject duplicate rename+alias keys"
        );
        assert!(
            result.unwrap_err().to_string().contains("duplicate field"),
            "error should mention duplicate field"
        );
    }

    #[test]
    fn meta_with_only_alias_key_deserializes_ok() {
        // After the fix, to_provider_json_value removes commonConfigEnabled
        // before inserting applyCommonConfig, so only one key remains.
        let meta_json = json!({
            "applyCommonConfig": false
        });

        let result: ProviderMeta =
            serde_json::from_value(meta_json).expect("should deserialize with alias only");
        assert_eq!(result.apply_common_config, Some(false));
    }

    #[test]
    fn provider_with_only_alias_in_meta_deserializes_ok() {
        let provider_json = json!({
            "id": "test",
            "name": "Test",
            "settingsConfig": {},
            "meta": {
                "applyCommonConfig": true
            }
        });
        let provider: Provider =
            serde_json::from_value(provider_json).expect("should deserialize provider");
        assert_eq!(provider.meta.unwrap().apply_common_config, Some(true));
    }

    #[test]
    fn provider_roundtrip_preserves_upstream_meta_fields_for_auth_and_type() {
        let provider_json = json!({
            "id": "test",
            "name": "Test",
            "settingsConfig": {},
            "meta": {
                "authBinding": {
                    "source": "managed_account",
                    "authProvider": "github_copilot",
                    "accountId": "acc-1"
                },
                "apiKeyField": "ANTHROPIC_AUTH_TOKEN",
                "providerType": "github_copilot",
                "githubAccountId": "gh-123"
            }
        });

        let provider: Provider =
            serde_json::from_value(provider_json).expect("provider should deserialize");
        let serialized = serde_json::to_value(&provider).expect("provider should serialize");
        let meta = serialized
            .get("meta")
            .and_then(|value| value.as_object())
            .expect("meta should exist after roundtrip");

        assert_eq!(
            meta.get("authBinding")
                .and_then(|value| value.get("source"))
                .and_then(|value| value.as_str()),
            Some("managed_account")
        );
        assert_eq!(
            meta.get("authBinding")
                .and_then(|value| value.get("authProvider"))
                .and_then(|value| value.as_str()),
            Some("github_copilot")
        );
        assert_eq!(
            meta.get("authBinding")
                .and_then(|value| value.get("accountId"))
                .and_then(|value| value.as_str()),
            Some("acc-1")
        );
        assert_eq!(
            meta.get("apiKeyField").and_then(|value| value.as_str()),
            Some("ANTHROPIC_AUTH_TOKEN")
        );
        assert_eq!(
            meta.get("providerType").and_then(|value| value.as_str()),
            Some("github_copilot")
        );
        assert_eq!(
            meta.get("githubAccountId").and_then(|value| value.as_str()),
            Some("gh-123")
        );
    }
}
