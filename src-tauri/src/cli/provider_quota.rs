use serde::Serialize;
use serde_json::Value;

use crate::app_config::AppType;
use crate::provider::{Provider, UsageData, UsageResult};
use crate::services::{ProviderService, SubscriptionQuota};
use crate::store::AppState;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub(crate) enum QuotaTargetKind {
    SubscriptionTool { tool: String },
    CodexOAuth { account_id: Option<String> },
    UsageScript,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuotaTarget {
    pub(crate) app_type: AppType,
    pub(crate) provider_id: String,
    pub(crate) provider_name: String,
    pub(crate) kind: QuotaTargetKind,
}

impl QuotaTarget {
    pub(crate) fn cache_key(&self) -> String {
        let kind = match &self.kind {
            QuotaTargetKind::SubscriptionTool { tool } => format!("subscription:{tool}"),
            QuotaTargetKind::CodexOAuth { account_id } => {
                format!("codex_oauth:{}", account_id.as_deref().unwrap_or("default"))
            }
            QuotaTargetKind::UsageScript => "usage_script".to_string(),
        };
        format!("{}:{}:{kind}", self.app_type.as_str(), self.provider_id)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "quota")]
pub(crate) enum ProviderUsageQuota {
    Subscription(SubscriptionQuota),
    Script(UsageResult),
}

pub(crate) fn provider_display_name(app_type: &AppType, id: &str, provider: &Provider) -> String {
    let name = provider.name.trim();
    if !name.is_empty() {
        return provider.name.clone();
    }

    if matches!(app_type, AppType::OpenClaw) {
        return id.to_string();
    }

    provider.name.clone()
}

pub(crate) fn quota_target_for_provider(
    app_type: &AppType,
    id: &str,
    provider: &Provider,
) -> Option<QuotaTarget> {
    let provider_name = provider_display_name(app_type, id, provider);

    if provider
        .meta
        .as_ref()
        .and_then(|meta| meta.usage_script.as_ref())
        .is_some_and(|script| script.enabled)
    {
        return Some(QuotaTarget {
            app_type: app_type.clone(),
            provider_id: id.to_string(),
            provider_name,
            kind: QuotaTargetKind::UsageScript,
        });
    }

    if is_codex_oauth_provider(provider) {
        return Some(QuotaTarget {
            app_type: app_type.clone(),
            provider_id: id.to_string(),
            provider_name,
            kind: QuotaTargetKind::CodexOAuth {
                account_id: provider
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.managed_account_id_for("codex_oauth")),
            },
        });
    }

    let tool = match app_type {
        AppType::Claude if is_claude_official_provider(provider) => "claude",
        AppType::Codex if is_codex_official_provider(provider) => "codex",
        AppType::Gemini if is_gemini_official_provider(provider) => "gemini",
        _ => return None,
    };

    Some(QuotaTarget {
        app_type: app_type.clone(),
        provider_id: id.to_string(),
        provider_name,
        kind: QuotaTargetKind::SubscriptionTool {
            tool: tool.to_string(),
        },
    })
}

pub(crate) async fn query_quota(target: &QuotaTarget) -> Result<ProviderUsageQuota, String> {
    match &target.kind {
        QuotaTargetKind::SubscriptionTool { tool } => {
            crate::services::subscription::get_subscription_quota(tool)
                .await
                .map(ProviderUsageQuota::Subscription)
        }
        QuotaTargetKind::CodexOAuth { account_id } => Ok(ProviderUsageQuota::Subscription(
            crate::services::CodexOAuthService::get_quota(account_id.as_deref()).await,
        )),
        QuotaTargetKind::UsageScript => {
            let state = AppState::try_new().map_err(|error| error.to_string())?;
            ProviderService::query_provider_usage(
                &state,
                target.app_type.clone(),
                &target.provider_id,
            )
            .await
            .map(ProviderUsageQuota::Script)
        }
    }
}

pub(crate) fn display_usage_plan_name(item: &UsageData) -> Option<&str> {
    item.plan_name.as_deref().filter(|value| {
        let trimmed = value.trim();
        !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("default")
    })
}

pub(crate) fn usage_value_summary(item: &UsageData) -> Option<String> {
    let unit = item.unit.as_deref().unwrap_or("");
    match (item.remaining, item.total, item.used) {
        (Some(remaining), Some(total), Some(used)) => Some(format!(
            "{} / {} {} left, {} used",
            usage_number(remaining),
            usage_number(total),
            unit,
            usage_number(used)
        )),
        (Some(remaining), Some(total), None) => Some(format!(
            "{} / {} {} left",
            usage_number(remaining),
            usage_number(total),
            unit
        )),
        (Some(remaining), None, _) => Some(format!("{} {}", usage_number(remaining), unit)),
        (None, Some(total), Some(used)) => Some(format!(
            "{} / {} {} used",
            usage_number(used),
            usage_number(total),
            unit
        )),
        (None, Some(total), None) => Some(format!("total {} {}", usage_number(total), unit)),
        (None, None, Some(used)) => Some(format!("used {} {}", usage_number(used), unit)),
        _ => None,
    }
    .map(|value| value.trim().to_string())
}

pub(crate) fn usage_number(value: f64) -> String {
    if (value.fract()).abs() < f64::EPSILON {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

fn is_codex_oauth_provider(provider: &Provider) -> bool {
    provider
        .meta
        .as_ref()
        .and_then(|meta| meta.provider_type.as_deref())
        .is_some_and(|value| value == "codex_oauth")
}

fn is_claude_official_provider(provider: &Provider) -> bool {
    provider
        .category
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("official"))
}

fn is_codex_official_provider(provider: &Provider) -> bool {
    if provider
        .meta
        .as_ref()
        .and_then(|meta| meta.codex_official)
        .unwrap_or(false)
    {
        return true;
    }

    if provider
        .category
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("official"))
    {
        return true;
    }

    let legacy_official_identity = provider
        .website_url
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("https://chatgpt.com/codex"))
        || provider.name.trim().eq_ignore_ascii_case("OpenAI Official");

    if !legacy_official_identity {
        return false;
    }

    let api_key_blank = provider
        .settings_config
        .get("auth")
        .and_then(|auth| auth.get("OPENAI_API_KEY"))
        .and_then(Value::as_str)
        .is_none_or(|value| value.trim().is_empty());

    api_key_blank && !codex_config_has_base_url(&provider.settings_config)
}

fn is_gemini_official_provider(provider: &Provider) -> bool {
    if provider
        .category
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("official"))
    {
        return true;
    }

    if provider
        .meta
        .as_ref()
        .and_then(|meta| meta.partner_promotion_key.as_deref())
        .is_some_and(|value| value.eq_ignore_ascii_case("google-official"))
    {
        return true;
    }

    let legacy_google_identity = provider.website_url.as_deref().is_some_and(|value| {
        let value = value.trim_end_matches('/');
        value.eq_ignore_ascii_case("https://ai.google.dev")
            || value.eq_ignore_ascii_case("https://aistudio.google.com")
    }) || matches!(
        provider.name.trim().to_ascii_lowercase().as_str(),
        "google" | "google official" | "google oauth"
    );

    if !legacy_google_identity {
        return false;
    }

    let api_key_blank = provider
        .settings_config
        .get("env")
        .and_then(|env| env.get("GEMINI_API_KEY"))
        .and_then(Value::as_str)
        .is_none_or(|value| value.trim().is_empty());
    let base_url_blank = extract_api_url(&provider.settings_config, &AppType::Gemini)
        .is_none_or(|value| value.trim().is_empty());

    api_key_blank && base_url_blank
}

fn codex_config_has_base_url(settings_config: &Value) -> bool {
    let Some(config_text) = settings_config
        .get("config")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };

    let Ok(table) = toml::from_str::<toml::Table>(config_text) else {
        return false;
    };

    if table
        .get("base_url")
        .and_then(|value| value.as_str())
        .is_some_and(|value| !value.trim().is_empty())
    {
        return true;
    }

    let Some(provider_key) = table.get("model_provider").and_then(|value| value.as_str()) else {
        return false;
    };

    table
        .get("model_providers")
        .and_then(|value| value.as_table())
        .and_then(|providers| providers.get(provider_key))
        .and_then(|value| value.as_table())
        .and_then(|provider| provider.get("base_url"))
        .and_then(|value| value.as_str())
        .is_some_and(|value| !value.trim().is_empty())
}

fn extract_api_url(settings_config: &Value, app_type: &AppType) -> Option<String> {
    match app_type {
        AppType::Claude => settings_config
            .get("env")?
            .get("ANTHROPIC_BASE_URL")?
            .as_str()
            .map(|s| s.to_string()),
        AppType::Codex => {
            if let Some(config_str) = settings_config.get("config")?.as_str() {
                for line in config_str.lines() {
                    let line = line.trim();
                    if line.starts_with("base_url") {
                        if let Some(url_part) = line.split('=').nth(1) {
                            let url = url_part.trim().trim_matches('"').trim_matches('\'');
                            if !url.is_empty() {
                                return Some(url.to_string());
                            }
                        }
                    }
                }
            }
            None
        }
        AppType::Gemini => settings_config
            .get("env")
            .and_then(|env| {
                env.get("GOOGLE_GEMINI_BASE_URL")
                    .or_else(|| env.get("GEMINI_BASE_URL"))
                    .or_else(|| env.get("BASE_URL"))
            })?
            .as_str()
            .map(|s| s.to_string()),
        AppType::OpenCode => settings_config
            .get("options")?
            .get("baseURL")?
            .as_str()
            .map(|s| s.to_string()),
        AppType::Hermes => settings_config
            .get("base_url")
            .or_else(|| settings_config.get("baseUrl"))
            .or_else(|| settings_config.get("baseURL"))
            .or_else(|| settings_config.get("endpoint"))?
            .as_str()
            .map(|s| s.to_string()),
        AppType::OpenClaw => settings_config
            .get("baseUrl")
            .or_else(|| settings_config.get("base_url"))?
            .as_str()
            .map(|s| s.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::provider::{AuthBinding, AuthBindingSource, ProviderMeta, UsageScript};

    fn test_provider(id: &str, name: &str, settings_config: Value) -> Provider {
        Provider::with_id(id.to_string(), name.to_string(), settings_config, None)
    }

    #[test]
    fn quota_target_detects_usage_script_before_official_provider() {
        let mut provider = test_provider("official", "Claude Official", json!({"env": {}}));
        provider.category = Some("official".to_string());
        provider.meta = Some(ProviderMeta {
            usage_script: Some(UsageScript {
                enabled: true,
                language: "javascript".to_string(),
                code: "return { data: [] }".to_string(),
                timeout: Some(10),
                api_key: None,
                base_url: None,
                access_token: None,
                user_id: None,
                template_type: Some("general".to_string()),
                auto_query_interval: Some(5),
                coding_plan_provider: None,
            }),
            ..ProviderMeta::default()
        });

        let target = quota_target_for_provider(&AppType::Claude, "official", &provider)
            .expect("usage script target");

        assert!(matches!(target.kind, QuotaTargetKind::UsageScript));
    }

    #[test]
    fn quota_target_detects_official_claude_by_explicit_category() {
        let mut official = test_provider("official", "Claude Official", json!({"env": {}}));
        official.category = Some("official".to_string());
        let stripped_custom = test_provider("stripped-custom", "Custom", json!({"env": {}}));
        let custom = test_provider(
            "custom",
            "Claude Custom",
            json!({"env": {"ANTHROPIC_BASE_URL": "https://api.example.com"}}),
        );

        assert!(matches!(
            quota_target_for_provider(&AppType::Claude, "official", &official)
                .map(|target| target.kind),
            Some(QuotaTargetKind::SubscriptionTool { tool }) if tool == "claude"
        ));
        assert!(
            quota_target_for_provider(&AppType::Claude, "stripped-custom", &stripped_custom)
                .is_none()
        );
        assert!(quota_target_for_provider(&AppType::Claude, "custom", &custom).is_none());
    }

    #[test]
    fn quota_target_detects_codex_official_and_skips_api_key_providers() {
        let missing_key = test_provider("official", "OpenAI Official", json!({"auth": {}}));
        let no_key_custom_base_url = test_provider(
            "base-url",
            "OpenAI Official",
            json!({
                "auth": {},
                "config": r#"base_url = "https://api.example.com/v1""#
            }),
        );
        let mut metadata_official = test_provider(
            "metadata",
            "Codex Official",
            json!({"auth": {"OPENAI_API_KEY": "sk-custom"}}),
        );
        metadata_official.meta = Some(ProviderMeta {
            codex_official: Some(true),
            ..ProviderMeta::default()
        });
        let api_key = test_provider(
            "api-key",
            "Custom OpenAI",
            json!({"auth": {"OPENAI_API_KEY": "sk-custom"}}),
        );

        assert!(matches!(
            quota_target_for_provider(&AppType::Codex, "official", &missing_key)
                .map(|target| target.kind),
            Some(QuotaTargetKind::SubscriptionTool { tool }) if tool == "codex"
        ));
        assert!(matches!(
            quota_target_for_provider(&AppType::Codex, "metadata", &metadata_official)
                .map(|target| target.kind),
            Some(QuotaTargetKind::SubscriptionTool { tool }) if tool == "codex"
        ));
        assert!(
            quota_target_for_provider(&AppType::Codex, "base-url", &no_key_custom_base_url)
                .is_none()
        );
        assert!(quota_target_for_provider(&AppType::Codex, "api-key", &api_key).is_none());
    }

    #[test]
    fn quota_target_detects_gemini_official_and_skips_api_key_providers() {
        let mut explicit_official =
            test_provider("google-official", "Google Official", json!({"env": {}}));
        explicit_official.category = Some("official".to_string());

        let google_oauth = test_provider(
            "google-oauth",
            "Google OAuth",
            json!({"env": {}, "config": {}}),
        );
        let mut partner_official = test_provider(
            "partner",
            "Gemini",
            json!({"env": {"GEMINI_API_KEY": "sk"}}),
        );
        partner_official.meta = Some(ProviderMeta {
            partner_promotion_key: Some("google-official".to_string()),
            ..ProviderMeta::default()
        });
        let api_key = test_provider(
            "api-key",
            "Google OAuth",
            json!({"env": {"GEMINI_API_KEY": "sk-custom"}}),
        );
        let base_url = test_provider(
            "base-url",
            "Google OAuth",
            json!({"env": {"GOOGLE_GEMINI_BASE_URL": "https://api.example.com"}}),
        );
        let stripped_custom = test_provider("custom", "Custom Gemini", json!({"env": {}}));

        assert!(matches!(
            quota_target_for_provider(&AppType::Gemini, "google-official", &explicit_official)
                .map(|target| target.kind),
            Some(QuotaTargetKind::SubscriptionTool { tool }) if tool == "gemini"
        ));
        assert!(matches!(
            quota_target_for_provider(&AppType::Gemini, "google-oauth", &google_oauth)
                .map(|target| target.kind),
            Some(QuotaTargetKind::SubscriptionTool { tool }) if tool == "gemini"
        ));
        assert!(matches!(
            quota_target_for_provider(&AppType::Gemini, "partner", &partner_official)
                .map(|target| target.kind),
            Some(QuotaTargetKind::SubscriptionTool { tool }) if tool == "gemini"
        ));
        assert!(quota_target_for_provider(&AppType::Gemini, "api-key", &api_key).is_none());
        assert!(quota_target_for_provider(&AppType::Gemini, "base-url", &base_url).is_none());
        assert!(quota_target_for_provider(&AppType::Gemini, "custom", &stripped_custom).is_none());
    }

    #[test]
    fn quota_target_detects_codex_oauth_managed_account() {
        let mut provider = test_provider("codex-oauth", "Codex OAuth", json!({}));
        provider.meta = Some(ProviderMeta {
            provider_type: Some("codex_oauth".to_string()),
            auth_binding: Some(AuthBinding {
                source: AuthBindingSource::ManagedAccount,
                auth_provider: Some("codex_oauth".to_string()),
                account_id: Some("acct-1".to_string()),
            }),
            ..ProviderMeta::default()
        });

        let target = quota_target_for_provider(&AppType::Claude, "codex-oauth", &provider)
            .expect("codex oauth quota target");

        assert_eq!(target.provider_id, "codex-oauth");
        assert!(matches!(
            target.kind,
            QuotaTargetKind::CodexOAuth { account_id } if account_id.as_deref() == Some("acct-1")
        ));
    }

    #[test]
    fn usage_display_hides_default_plan_name() {
        let item = UsageData {
            plan_name: Some("default".to_string()),
            remaining: Some(2.0),
            total: None,
            used: None,
            unit: Some("USD".to_string()),
            extra: None,
            is_valid: None,
            invalid_message: None,
        };

        assert_eq!(display_usage_plan_name(&item), None);
        assert_eq!(usage_value_summary(&item).as_deref(), Some("2 USD"));
    }
}
