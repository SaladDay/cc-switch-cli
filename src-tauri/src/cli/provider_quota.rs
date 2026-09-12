use serde::Serialize;

use crate::app_config::AppType;
use crate::provider::{Provider, UsageData, UsageResult};
use crate::services::{ProviderService, SubscriptionQuota};
use crate::store::AppState;
use crate::usage_script::UsageQueryTemplate;

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
    /// Scheduling policy is local runtime metadata, not part of the public
    /// `provider quota --json` contract.
    #[serde(skip)]
    pub(crate) auto_query_interval_minutes: u64,
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

pub(crate) struct QuotaResetDisplay {
    pub(crate) at: chrono::DateTime<chrono::FixedOffset>,
    pub(crate) remaining: Option<String>,
}

pub(crate) fn quota_reset_display(
    resets_at: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<QuotaResetDisplay> {
    let at = chrono::DateTime::parse_from_rfc3339(resets_at?).ok()?;
    let diff = at.signed_duration_since(now);
    // Match upstream SubscriptionQuotaFooter.countdownStr: elapsed resets
    // have no countdown, and future windows use whole minutes/hours/days.
    let remaining = (diff > chrono::Duration::zero()).then(|| {
        let minutes = diff.num_minutes();
        let hours = minutes / 60;
        if hours > 24 {
            format!("{}d{}h", hours / 24, hours % 24)
        } else if hours > 0 {
            format!("{hours}h{}m", minutes % 60)
        } else {
            format!("{minutes}m")
        }
    });
    Some(QuotaResetDisplay { at, remaining })
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
    let usage_script = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.usage_script.as_ref());

    if let Some(script) = usage_script.filter(|script| script.enabled) {
        let template = script
            .template_type
            .as_deref()
            .and_then(UsageQueryTemplate::from_str);

        if template == Some(UsageQueryTemplate::OfficialSubscription) {
            if let Some(tool) = provider.official_subscription_tool(app_type) {
                return Some(QuotaTarget {
                    app_type: app_type.clone(),
                    provider_id: id.to_string(),
                    provider_name,
                    kind: QuotaTargetKind::SubscriptionTool {
                        tool: tool.to_string(),
                    },
                    auto_query_interval_minutes: script.auto_query_interval.unwrap_or(0),
                });
            }
            // Ignore an imported or hand-edited native template on a custom
            // provider. Managed OAuth providers may still use their own quota
            // path below.
        } else {
            // Official providers expose only the native subscription template.
            // A stale script template must not bypass the opt-in switch.
            if provider.official_subscription_tool(app_type).is_some() {
                return None;
            }

            return Some(QuotaTarget {
                app_type: app_type.clone(),
                provider_id: id.to_string(),
                provider_name,
                kind: QuotaTargetKind::UsageScript,
                auto_query_interval_minutes: script.auto_query_interval.unwrap_or(0),
            });
        }
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
            auto_query_interval_minutes: 5,
        });
    }

    None
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
            let state = AppState::try_open_snapshot().map_err(|error| error.to_string())?;
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

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;
    use crate::provider::{AuthBinding, AuthBindingSource, ProviderMeta, UsageScript};

    #[test]
    fn quota_reset_countdown_matches_upstream_windows() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-12T12:00:00Z")
            .unwrap()
            .to_utc();
        for (seconds, expected) in [
            (1, "0m"),
            (59, "0m"),
            (60, "1m"),
            (9_000, "2h30m"),
            (86_400, "24h0m"),
            (89_940, "24h59m"),
            (90_000, "1d1h"),
            (302_400, "3d12h"),
        ] {
            let resets_at = (now + chrono::Duration::seconds(seconds)).to_rfc3339();
            let reset = quota_reset_display(Some(&resets_at), now).unwrap();
            assert_eq!(reset.remaining.as_deref(), Some(expected), "{seconds}s");
        }
        let reset = quota_reset_display(Some("2026-09-12T20:43:00+08:00"), now).unwrap();
        assert_eq!(reset.remaining.as_deref(), Some("43m"));
        assert_eq!(reset.at.to_utc(), now + chrono::Duration::minutes(43));
    }

    #[test]
    fn quota_reset_display_handles_missing_invalid_and_elapsed_times() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-12T12:00:00Z")
            .unwrap()
            .to_utc();
        for resets_at in [
            None,
            Some(""),
            Some("invalid"),
            Some("2026-99-99T00:00:00Z"),
        ] {
            assert!(quota_reset_display(resets_at, now).is_none());
        }
        for resets_at in ["2026-09-12T12:00:00Z", "2026-09-11T12:00:00Z"] {
            let reset = quota_reset_display(Some(resets_at), now).unwrap();
            assert!(reset.remaining.is_none());
        }
    }

    fn test_provider(id: &str, name: &str, settings_config: Value) -> Provider {
        Provider::with_id(id.to_string(), name.to_string(), settings_config, None)
    }

    fn set_usage_script(provider: &mut Provider, enabled: bool, template_type: &str) {
        provider.meta = Some(ProviderMeta {
            usage_script: Some(UsageScript {
                enabled,
                language: "javascript".to_string(),
                code: "return { data: [] }".to_string(),
                timeout: Some(10),
                api_key: None,
                base_url: None,
                access_token: None,
                user_id: None,
                template_type: Some(template_type.to_string()),
                auto_query_interval: Some(5),
                coding_plan_provider: None,
            }),
            ..ProviderMeta::default()
        });
    }

    #[test]
    fn official_subscription_target_requires_enabled_official_template() {
        let mut provider = test_provider("official", "Claude Official", json!({"env": {}}));
        provider.category = Some("official".to_string());

        assert!(quota_target_for_provider(&AppType::Claude, "official", &provider).is_none());

        set_usage_script(&mut provider, false, "official_subscription");
        assert!(quota_target_for_provider(&AppType::Claude, "official", &provider).is_none());

        set_usage_script(&mut provider, true, "official_subscription");
        assert!(matches!(
            quota_target_for_provider(&AppType::Claude, "official", &provider)
                .map(|target| target.kind),
            Some(QuotaTargetKind::SubscriptionTool { tool }) if tool == "claude"
        ));

        set_usage_script(&mut provider, true, "general");
        assert!(quota_target_for_provider(&AppType::Claude, "official", &provider).is_none());
    }

    #[test]
    fn official_subscription_template_routes_supported_apps_to_native_quota() {
        for (app_type, tool, settings) in [
            (AppType::Claude, "claude", json!({"env": {}})),
            (AppType::Codex, "codex", json!({"auth": {}})),
            (AppType::Gemini, "gemini", json!({"env": {}})),
        ] {
            let mut provider = test_provider("official", "Official", settings);
            set_usage_script(&mut provider, true, "official_subscription");
            assert!(matches!(
                quota_target_for_provider(&app_type, "official", &provider)
                    .map(|target| target.kind),
                Some(QuotaTargetKind::SubscriptionTool { tool: actual }) if actual == tool
            ));
        }
    }

    #[test]
    fn non_official_enabled_script_keeps_script_route() {
        let mut provider = test_provider(
            "custom",
            "Custom",
            json!({"env": {"ANTHROPIC_BASE_URL": "https://api.example.com"}}),
        );
        set_usage_script(&mut provider, true, "general");
        assert!(matches!(
            quota_target_for_provider(&AppType::Claude, "custom", &provider)
                .map(|target| target.kind),
            Some(QuotaTargetKind::UsageScript)
        ));

        set_usage_script(&mut provider, false, "general");
        assert!(quota_target_for_provider(&AppType::Claude, "custom", &provider).is_none());

        set_usage_script(&mut provider, true, "official_subscription");
        assert!(
            quota_target_for_provider(&AppType::Claude, "custom", &provider).is_none(),
            "a raw native template must not read local OAuth for a custom provider"
        );
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
