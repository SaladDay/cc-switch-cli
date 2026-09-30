use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use uuid::Uuid;
use crate::config;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialStatus {
    Valid,
    Expired,
    NotFound,
    ParseError,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QuotaTier {
    pub name: String,
    pub utilization: f64,
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExtraUsage {
    pub is_enabled: bool,
    pub monthly_limit: Option<f64>,
    pub used_credits: Option<f64>,
    pub utilization: Option<f64>,
    pub currency: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodexResetCredit {
    pub id: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub reset_type: Option<String>,
    pub status: Option<String>,
    pub granted_at: Option<String>,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodexResetCreditsSummary {
    pub available_count: usize,
    pub applicable_available_count: Option<usize>,
    pub credits: Vec<CodexResetCredit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexResetConsumeResult {
    pub success: bool,
    pub code: String,
    pub credit_id: String,
    pub redeem_request_id: String,
    pub windows_reset: Option<i64>,
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_quota: Option<SubscriptionQuota>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionQuota {
    pub tool: String,
    pub credential_status: CredentialStatus,
    pub credential_message: Option<String>,
    pub success: bool,
    pub tiers: Vec<QuotaTier>,
    pub extra_usage: Option<ExtraUsage>,
    pub error: Option<String>,
    pub queried_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_credits: Option<CodexResetCreditsSummary>,
}

impl SubscriptionQuota {
    pub fn not_found(tool: &str) -> Self {
        Self {
            tool: tool.to_string(),
            credential_status: CredentialStatus::NotFound,
            credential_message: None,
            success: false,
            tiers: vec![],
            extra_usage: None,
            error: None,
            queried_at: None,
            reset_credits: None,
        }
    }

    pub(crate) fn error(tool: &str, status: CredentialStatus, message: String) -> Self {
        Self {
            tool: tool.to_string(),
            credential_status: status,
            credential_message: Some(message.clone()),
            success: false,
            tiers: vec![],
            extra_usage: None,
            error: Some(message),
            queried_at: Some(now_millis()),
            reset_credits: None,
        }
    }
}

#[derive(Deserialize)]
struct ClaudeOAuthEntry {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
    #[serde(rename = "expiresAt")]
    expires_at: Option<serde_json::Value>,
}

fn read_claude_credentials() -> (Option<String>, CredentialStatus, Option<String>) {
    #[cfg(target_os = "macos")]
    {
        if let Some(result) = read_claude_credentials_from_keychain() {
            return result;
        }
    }

    read_claude_credentials_from_file()
}

#[cfg(target_os = "macos")]
fn read_claude_credentials_from_keychain(
) -> Option<(Option<String>, CredentialStatus, Option<String>)> {
    let output = std::process::Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            "Claude Code-credentials",
            "-w",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let json_str = String::from_utf8(output.stdout).ok()?;
    let json_str = json_str.trim();
    if json_str.is_empty() {
        return None;
    }

    Some(parse_claude_credentials_json(json_str))
}

fn read_claude_credentials_from_file() -> (Option<String>, CredentialStatus, Option<String>) {
    let cred_path = config::get_claude_config_dir().join(".credentials.json");

    if !cred_path.exists() {
        return (None, CredentialStatus::NotFound, None);
    }

    let content = match std::fs::read_to_string(&cred_path) {
        Ok(content) => content,
        Err(error) => {
            return (
                None,
                CredentialStatus::ParseError,
                Some(format!("Failed to read credentials file: {error}")),
            );
        }
    };

    parse_claude_credentials_json(&content)
}

fn parse_claude_credentials_json(
    content: &str,
) -> (Option<String>, CredentialStatus, Option<String>) {
    let parsed: serde_json::Value = match serde_json::from_str(content) {
        Ok(value) => value,
        Err(error) => {
            return (
                None,
                CredentialStatus::ParseError,
                Some(format!("Failed to parse credentials JSON: {error}")),
            );
        }
    };

    let entry_value = parsed
        .get("claudeAiOauth")
        .or_else(|| parsed.get("claude.ai_oauth"));

    let Some(entry_value) = entry_value else {
        return (
            None,
            CredentialStatus::ParseError,
            Some("No OAuth entry found in credentials".to_string()),
        );
    };

    let entry: ClaudeOAuthEntry = match serde_json::from_value(entry_value.clone()) {
        Ok(entry) => entry,
        Err(error) => {
            return (
                None,
                CredentialStatus::ParseError,
                Some(format!("Failed to parse OAuth entry: {error}")),
            );
        }
    };

    let access_token = match entry.access_token {
        Some(token) if !token.is_empty() => token,
        _ => {
            return (
                None,
                CredentialStatus::ParseError,
                Some("accessToken is empty or missing".to_string()),
            );
        }
    };

    if let Some(expires_at) = entry.expires_at {
        if is_token_expired(&expires_at) {
            return (
                Some(access_token),
                CredentialStatus::Expired,
                Some("OAuth token has expired".to_string()),
            );
        }
    }

    (Some(access_token), CredentialStatus::Valid, None)
}

fn is_token_expired(expires_at: &serde_json::Value) -> bool {
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    match expires_at {
        serde_json::Value::Number(value) => value.as_u64().is_some_and(|timestamp| {
            let timestamp_secs = if timestamp > 1_000_000_000_000 {
                timestamp / 1000
            } else {
                timestamp
            };
            timestamp_secs < now_secs
        }),
        serde_json::Value::String(value) => {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(value) {
                (dt.timestamp() as u64) < now_secs
            } else if let Ok(dt) =
                chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
            {
                (dt.and_utc().timestamp() as u64) < now_secs
            } else {
                false
            }
        }
        _ => false,
    }
}

#[derive(Deserialize)]
struct ApiUsageWindow {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

#[derive(Deserialize)]
struct ApiExtraUsage {
    is_enabled: Option<bool>,
    monthly_limit: Option<f64>,
    used_credits: Option<f64>,
    utilization: Option<f64>,
    currency: Option<String>,
}

const KNOWN_TIERS: &[&str] = &[
    "five_hour",
    "seven_day",
    "seven_day_opus",
    "seven_day_sonnet",
];

async fn query_claude_quota(access_token: &str) -> SubscriptionQuota {
    let client = crate::proxy::http_client::get();

    let response = match client
        .get("https://api.anthropic.com/api/oauth/usage")
        .header("Authorization", format!("Bearer {access_token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return SubscriptionQuota::error(
                "claude",
                CredentialStatus::Valid,
                format!("Network error: {error}"),
            );
        }
    };

    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return SubscriptionQuota::error(
            "claude",
            CredentialStatus::Expired,
            format!("Authentication failed (HTTP {status}). Please re-login with Claude CLI."),
        );
    }

    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return SubscriptionQuota::error(
            "claude",
            CredentialStatus::Valid,
            format!("API error (HTTP {status}): {body}"),
        );
    }

    let body: serde_json::Value = match response.json().await {
        Ok(body) => body,
        Err(error) => {
            return SubscriptionQuota::error(
                "claude",
                CredentialStatus::Valid,
                format!("Failed to parse API response: {error}"),
            );
        }
    };

    let mut tiers = Vec::new();
    for &tier_name in KNOWN_TIERS {
        if let Some(window) = body.get(tier_name) {
            if let Ok(window) = serde_json::from_value::<ApiUsageWindow>(window.clone()) {
                if let Some(utilization) = window.utilization {
                    tiers.push(QuotaTier {
                        name: tier_name.to_string(),
                        utilization,
                        resets_at: window.resets_at,
                    });
                }
            }
        }
    }

    if let Some(object) = body.as_object() {
        for (key, value) in object {
            if key == "extra_usage" || KNOWN_TIERS.contains(&key.as_str()) {
                continue;
            }
            if let Ok(window) = serde_json::from_value::<ApiUsageWindow>(value.clone()) {
                if let Some(utilization) = window.utilization {
                    tiers.push(QuotaTier {
                        name: key.clone(),
                        utilization,
                        resets_at: window.resets_at,
                    });
                }
            }
        }
    }

    let extra_usage = body.get("extra_usage").and_then(|value| {
        serde_json::from_value::<ApiExtraUsage>(value.clone())
            .ok()
            .map(|extra| ExtraUsage {
                is_enabled: extra.is_enabled.unwrap_or(false),
                monthly_limit: extra.monthly_limit,
                used_credits: extra.used_credits,
                utilization: extra.utilization,
                currency: extra.currency,
            })
    });

    SubscriptionQuota {
        tool: "claude".to_string(),
        credential_status: CredentialStatus::Valid,
        credential_message: None,
        success: true,
        tiers,
        extra_usage,
        error: None,
        queried_at: Some(now_millis()),
        reset_credits: None,
    }
}

#[derive(Deserialize)]
struct CodexAuthJson {
    auth_mode: Option<String>,
    tokens: Option<CodexTokens>,
    last_refresh: Option<String>,
}

#[derive(Deserialize)]
struct CodexTokens {
    access_token: Option<String>,
    account_id: Option<String>,
}

type CodexCredentials = (
    Option<String>,
    Option<String>,
    CredentialStatus,
    Option<String>,
);

fn read_codex_credentials() -> CodexCredentials {
    #[cfg(target_os = "macos")]
    {
        if let Some(result) = read_codex_credentials_from_keychain() {
            return result;
        }
    }

    read_codex_credentials_from_file()
}

#[cfg(target_os = "macos")]
fn read_codex_credentials_from_keychain() -> Option<CodexCredentials> {
    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-s", "Codex Auth", "-w"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let json_str = String::from_utf8(output.stdout).ok()?;
    let json_str = json_str.trim();
    if json_str.is_empty() {
        return None;
    }

    Some(parse_codex_credentials_json(json_str))
}

fn read_codex_credentials_from_file() -> CodexCredentials {
    let auth_path = crate::codex_config::get_codex_auth_path();

    if !auth_path.exists() {
        return (None, None, CredentialStatus::NotFound, None);
    }

    let content = match std::fs::read_to_string(&auth_path) {
        Ok(content) => content,
        Err(error) => {
            return (
                None,
                None,
                CredentialStatus::ParseError,
                Some(format!("Failed to read Codex auth file: {error}")),
            );
        }
    };

    parse_codex_credentials_json(&content)
}

fn parse_codex_credentials_json(content: &str) -> CodexCredentials {
    let auth: CodexAuthJson = match serde_json::from_str(content) {
        Ok(auth) => auth,
        Err(error) => {
            return (
                None,
                None,
                CredentialStatus::ParseError,
                Some(format!("Failed to parse Codex auth JSON: {error}")),
            );
        }
    };

    if auth.auth_mode.as_deref() != Some("chatgpt") {
        return (
            None,
            None,
            CredentialStatus::NotFound,
            Some("Codex not using OAuth mode".to_string()),
        );
    }

    let tokens = match auth.tokens {
        Some(tokens) => tokens,
        None => {
            return (
                None,
                None,
                CredentialStatus::ParseError,
                Some("No tokens in Codex auth".to_string()),
            );
        }
    };

    let access_token = match tokens.access_token {
        Some(token) if !token.is_empty() => token,
        _ => {
            return (
                None,
                None,
                CredentialStatus::ParseError,
                Some("access_token is empty or missing".to_string()),
            );
        }
    };

    if let Some(ref last_refresh) = auth.last_refresh {
        if is_codex_token_stale(last_refresh) {
            return (
                Some(access_token),
                tokens.account_id,
                CredentialStatus::Expired,
                Some("Codex token may be stale (>8 days since last refresh)".to_string()),
            );
        }
    }

    (
        Some(access_token),
        tokens.account_id,
        CredentialStatus::Valid,
        None,
    )
}

fn is_codex_token_stale(last_refresh: &str) -> bool {
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    chrono::DateTime::parse_from_rfc3339(last_refresh).is_ok_and(|dt| {
        let age_secs = now_secs.saturating_sub(dt.timestamp() as u64);
        age_secs > 8 * 24 * 3600
    })
}

#[derive(Deserialize)]
struct CodexRateLimitWindow {
    used_percent: Option<f64>,
    limit_window_seconds: Option<i64>,
    reset_at: Option<i64>,
}

#[derive(Deserialize)]
struct CodexRateLimit {
    primary_window: Option<CodexRateLimitWindow>,
    secondary_window: Option<CodexRateLimitWindow>,
}

#[derive(Deserialize)]
struct CodexUsageResetCredits {
    available_count: Option<usize>,
    applicable_available_count: Option<usize>,
}

#[derive(Deserialize)]
struct CodexUsageResponse {
    rate_limit: Option<CodexRateLimit>,
    rate_limit_reset_credits: Option<CodexUsageResetCredits>,
}

#[derive(Deserialize)]
pub(crate) struct RawCodexResetCredit {
    pub id: Option<String>,
    pub reset_type: Option<String>,
    pub is_supported_by_plan: Option<bool>,
    pub status: Option<String>,
    pub granted_at: Option<String>,
    pub expires_at: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Deserialize)]
struct RawCodexResetCreditsResponse {
    credits: Option<Vec<RawCodexResetCredit>>,
    available_count: Option<usize>,
}

pub(crate) fn filter_and_sort_codex_reset_credits(
    raw_credits: Vec<RawCodexResetCredit>,
) -> Vec<CodexResetCredit> {
    let mut valid: Vec<CodexResetCredit> = raw_credits
        .into_iter()
        .filter_map(|c| {
            let id = c.id?.trim().to_string();
            if id.is_empty() {
                return None;
            }
            if c.reset_type.as_deref() != Some("codex_rate_limits") {
                return None;
            }
            if c.status.as_deref() != Some("available") {
                return None;
            }
            if c.is_supported_by_plan == Some(false) {
                return None;
            }
            let granted_at = c.granted_at.filter(|s| !s.trim().is_empty())?;
            Some(CodexResetCredit {
                id,
                title: c.title,
                description: c.description,
                reset_type: c.reset_type,
                status: c.status,
                granted_at: Some(granted_at),
                expires_at: c.expires_at,
            })
        })
        .collect();

    valid.sort_by(|a, b| match (&a.expires_at, &b.expires_at) {
        (Some(ea), Some(eb)) => ea.cmp(eb),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });

    valid
}

pub(crate) async fn fetch_codex_reset_credits(
    client: &reqwest::Client,
    access_token: &str,
    account_id: Option<&str>,
) -> Result<Vec<CodexResetCredit>, String> {
    let mut request = client
        .get("https://chatgpt.com/backend-api/wham/rate-limit-reset-credits")
        .header("Authorization", format!("Bearer {access_token}"))
        .header("User-Agent", "codex-cli")
        .header("Accept", "application/json");

    if let Some(account_id) = account_id {
        request = request.header("ChatGPT-Account-Id", account_id);
    }

    let response = request
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("Network error fetching reset credits: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("Failed to fetch reset credits (HTTP {status})"));
    }

    let raw: RawCodexResetCreditsResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse reset credits response: {e}"))?;

    Ok(filter_and_sort_codex_reset_credits(raw.credits.unwrap_or_default()))
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn window_seconds_to_tier_name(secs: i64) -> String {
    match secs {
        18_000 => "five_hour".to_string(),
        604_800 => "seven_day".to_string(),
        secs => {
            let hours = secs / 3600;
            if hours >= 24 {
                format!("{}_day", hours / 24)
            } else {
                format!("{}_hour", hours)
            }
        }
    }
}

fn unix_ts_to_iso(ts: i64) -> Option<String> {
    chrono::DateTime::from_timestamp(ts, 0).map(|dt| dt.to_rfc3339())
}

pub(crate) async fn query_codex_quota(
    access_token: &str,
    account_id: Option<&str>,
    tool_label: &str,
    expired_message: &str,
) -> SubscriptionQuota {
    let client = crate::proxy::http_client::get();

    let mut request = client
        .get("https://chatgpt.com/backend-api/wham/usage")
        .header("Authorization", format!("Bearer {access_token}"))
        .header("User-Agent", "codex-cli")
        .header("Accept", "application/json");

    if let Some(account_id) = account_id {
        request = request.header("ChatGPT-Account-Id", account_id);
    }

    let response = match request
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return SubscriptionQuota::error(
                tool_label,
                CredentialStatus::Valid,
                format!("Network error: {error}"),
            );
        }
    };

    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return SubscriptionQuota::error(
            tool_label,
            CredentialStatus::Expired,
            format!("{expired_message} (HTTP {status})"),
        );
    }

    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return SubscriptionQuota::error(
            tool_label,
            CredentialStatus::Valid,
            format!("API error (HTTP {status}): {body}"),
        );
    }

    let body: CodexUsageResponse = match response.json().await {
        Ok(body) => body,
        Err(error) => {
            return SubscriptionQuota::error(
                tool_label,
                CredentialStatus::Valid,
                format!("Failed to parse API response: {error}"),
            );
        }
    };

    let mut tiers = Vec::new();
    if let Some(rate_limit) = body.rate_limit {
        for window in [rate_limit.primary_window, rate_limit.secondary_window]
            .into_iter()
            .flatten()
        {
            if let Some(utilization) = window.used_percent {
                tiers.push(QuotaTier {
                    name: window
                        .limit_window_seconds
                        .map(window_seconds_to_tier_name)
                        .unwrap_or_else(|| "unknown".to_string()),
                    utilization,
                    resets_at: window.reset_at.and_then(unix_ts_to_iso),
                });
            }
        }
    }

    let reset_credits = match fetch_codex_reset_credits(&client, access_token, account_id).await {
        Ok(credits) => {
            let available_count = credits.len();
            let applicable_available_count = body
                .rate_limit_reset_credits
                .as_ref()
                .and_then(|rc| rc.applicable_available_count);
            Some(CodexResetCreditsSummary {
                available_count,
                applicable_available_count,
                credits,
            })
        }
        Err(_) => body
            .rate_limit_reset_credits
            .as_ref()
            .map(|rc| CodexResetCreditsSummary {
                available_count: rc.available_count.unwrap_or(0),
                applicable_available_count: rc.applicable_available_count,
                credits: Vec::new(),
            }),
    };

    SubscriptionQuota {
        tool: tool_label.to_string(),
        credential_status: CredentialStatus::Valid,
        credential_message: None,
        success: true,
        tiers,
        extra_usage: None,
        error: None,
        queried_at: Some(now_millis()),
        reset_credits,
    }
}


pub async fn consume_codex_reset_credit(
    access_token: &str,
    account_id: Option<&str>,
    credit_id: Option<&str>,
    tool_label: &str,
) -> Result<CodexResetConsumeResult, String> {
    let client = crate::proxy::http_client::get();

    // 1. Fresh GET to fetch currently available credits (Fail-closed)
    let available_credits = fetch_codex_reset_credits(&client, access_token, account_id).await?;
    if available_credits.is_empty() {
        return Err("No available rate limit reset credits found for this account.".to_string());
    }

    // 2. Select target credit: specified ID or earliest expiring
    let target_credit = match credit_id {
        Some(cid) => available_credits
            .into_iter()
            .find(|c| c.id == cid)
            .ok_or_else(|| {
                format!("Specified credit ID '{cid}' is not available or valid for this account.")
            })?,
        None => available_credits
            .into_iter()
            .next()
            .ok_or_else(|| "No available reset credit to consume.".to_string())?,
    };

    let target_credit_id = target_credit.id;
    let redeem_request_id = Uuid::new_v4().to_string();

    // 3. Send consume request
    let mut request = client
        .post("https://chatgpt.com/backend-api/wham/rate-limit-reset-credits/consume")
        .header("Authorization", format!("Bearer {access_token}"))
        .header("User-Agent", "codex-cli")
        .header("Accept", "application/json")
        .header("Content-Type", "application/json");

    if let Some(account_id) = account_id {
        request = request.header("ChatGPT-Account-Id", account_id);
    }

    let payload = serde_json::json!({
        "credit_id": target_credit_id,
        "redeem_request_id": redeem_request_id,
    });

    let response = request
        .json(&payload)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("Network error consuming reset credit: {e}"))?;

    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(format!("Authentication failed (HTTP {status}). Please re-login."));
    }

    if status == reqwest::StatusCode::UNPROCESSABLE_ENTITY {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("OpenAI rejected reset credit consumption (HTTP 422): {body}"));
    }

    let body_text = response.text().await.unwrap_or_default();
    let resp_json: serde_json::Value = serde_json::from_str(&body_text).map_err(|e| {
        format!("Failed to parse consume response (HTTP {status}): {body_text} ({e})")
    })?;

    let code = resp_json
        .get("code")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let windows_reset = resp_json.get("windows_reset").and_then(|v| v.as_i64());

    // CPA-Helper keeper rules: "reset" and "already_redeemed" are both considered successful
    if code == "reset" || code == "already_redeemed" {
        // 4. Re-inspect usage
        let new_quota = query_codex_quota(
            access_token,
            account_id,
            tool_label,
            "Authentication failed on quota re-inspection.",
        )
        .await;

        Ok(CodexResetConsumeResult {
            success: true,
            code,
            credit_id: target_credit_id,
            redeem_request_id,
            windows_reset,
            message: None,
            new_quota: Some(new_quota),
        })
    } else if code == "nothing_to_reset" {
        Err("Rate limit window is not currently restricted (nothing_to_reset).".to_string())
    } else if code == "no_credit" {
        Err("No valid reset credits available to consume (no_credit).".to_string())
    } else {
        Err(format!(
            "Failed to redeem credit: upstream returned code '{code}' (HTTP {status}): {body_text}"
        ))
    }
}

pub async fn reset_codex_subscription_quota(
    credit_id: Option<&str>,
) -> Result<CodexResetConsumeResult, String> {
    let (token, account_id, status, message) = read_codex_credentials();
    match status {
        CredentialStatus::NotFound => Err("Codex credentials not found".to_string()),
        CredentialStatus::ParseError => {
            Err(message.unwrap_or_else(|| "Failed to parse Codex credentials".to_string()))
        }
        CredentialStatus::Expired => {
            let Some(token) = token else {
                return Err(message.unwrap_or_else(|| "Codex token expired".to_string()));
            };
            consume_codex_reset_credit(&token, account_id.as_deref(), credit_id, "codex").await
        }
        CredentialStatus::Valid => {
            let Some(token) = token else {
                return Err("Codex access_token is empty or missing".to_string());
            };
            consume_codex_reset_credit(&token, account_id.as_deref(), credit_id, "codex").await
        }
    }
}

#[derive(Deserialize)]
struct GeminiOAuthCredsFile {
    #[serde(alias = "accessToken")]
    access_token: Option<String>,
    #[serde(alias = "refreshToken")]
    refresh_token: Option<String>,
    #[serde(alias = "expiryDate", alias = "expiresAt")]
    expiry_date: Option<i64>,
    #[serde(alias = "clientId")]
    client_id: Option<String>,
    #[serde(alias = "clientSecret")]
    client_secret: Option<String>,
}

#[derive(Debug, Clone)]
struct GeminiOAuthClient {
    client_id: String,
    client_secret: String,
}

type GeminiCredentials = (
    Option<String>,
    Option<String>,
    Option<GeminiOAuthClient>,
    CredentialStatus,
    Option<String>,
);

fn gemini_oauth_client_from_options(
    client_id: Option<String>,
    client_secret: Option<String>,
) -> Option<GeminiOAuthClient> {
    let client_id = client_id?.trim().to_string();
    let client_secret = client_secret?.trim().to_string();
    if client_id.is_empty() || client_secret.is_empty() {
        return None;
    }
    Some(GeminiOAuthClient {
        client_id,
        client_secret,
    })
}

#[expect(
    dead_code,
    reason = "kept for Gemini OAuth clients loaded from JSON credentials"
)]
fn gemini_oauth_client_from_json(value: &serde_json::Value) -> Option<GeminiOAuthClient> {
    let client_id = value
        .get("client_id")
        .or_else(|| value.get("clientId"))
        .and_then(|value| value.as_str())
        .map(String::from);
    let client_secret = value
        .get("client_secret")
        .or_else(|| value.get("clientSecret"))
        .and_then(|value| value.as_str())
        .map(String::from);
    gemini_oauth_client_from_options(client_id, client_secret)
}

fn read_gemini_credentials() -> GeminiCredentials {
    #[cfg(target_os = "macos")]
    {
        if let Some(result) = read_gemini_credentials_from_keychain() {
            return result;
        }
    }

    read_gemini_credentials_from_file()
}

#[cfg(target_os = "macos")]
fn read_gemini_credentials_from_keychain() -> Option<GeminiCredentials> {
    let output = std::process::Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            "gemini-cli-oauth",
            "-a",
            "main-account",
            "-w",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let json_str = String::from_utf8(output.stdout).ok()?;
    let json_str = json_str.trim();
    if json_str.is_empty() {
        return None;
    }

    Some(parse_gemini_keychain_json(json_str))
}

#[cfg(target_os = "macos")]
fn parse_gemini_keychain_json(content: &str) -> GeminiCredentials {
    let parsed: serde_json::Value = match serde_json::from_str(content) {
        Ok(value) => value,
        Err(error) => {
            return (
                None,
                None,
                None,
                CredentialStatus::ParseError,
                Some(format!("Failed to parse Gemini keychain JSON: {error}")),
            );
        }
    };

    let Some(token) = parsed.get("token") else {
        return parse_gemini_file_json(content);
    };

    let access_token = token
        .get("accessToken")
        .and_then(|value| value.as_str())
        .map(String::from);
    let refresh_token = token
        .get("refreshToken")
        .and_then(|value| value.as_str())
        .map(String::from);
    let expires_at = token.get("expiresAt").and_then(|value| value.as_i64());
    let oauth_client =
        gemini_oauth_client_from_json(token).or_else(|| gemini_oauth_client_from_json(&parsed));

    match access_token {
        Some(token) if !token.is_empty() => {
            if expires_at.is_some_and(|expires_at| expires_at < now_millis()) {
                return (
                    Some(token),
                    refresh_token,
                    oauth_client,
                    CredentialStatus::Expired,
                    Some("Gemini access token has expired".to_string()),
                );
            }

            (
                Some(token),
                refresh_token,
                oauth_client,
                CredentialStatus::Valid,
                None,
            )
        }
        _ => (
            None,
            refresh_token,
            oauth_client,
            CredentialStatus::ParseError,
            Some("accessToken is empty or missing".to_string()),
        ),
    }
}

fn read_gemini_credentials_from_file() -> GeminiCredentials {
    let cred_path = crate::gemini_config::get_gemini_dir().join("oauth_creds.json");

    if !cred_path.exists() {
        return (None, None, None, CredentialStatus::NotFound, None);
    }

    let content = match std::fs::read_to_string(&cred_path) {
        Ok(content) => content,
        Err(error) => {
            return (
                None,
                None,
                None,
                CredentialStatus::ParseError,
                Some(format!("Failed to read Gemini credentials: {error}")),
            );
        }
    };

    parse_gemini_file_json(&content)
}

fn parse_gemini_file_json(content: &str) -> GeminiCredentials {
    let creds: GeminiOAuthCredsFile = match serde_json::from_str(content) {
        Ok(creds) => creds,
        Err(error) => {
            return (
                None,
                None,
                None,
                CredentialStatus::ParseError,
                Some(format!("Failed to parse Gemini credentials: {error}")),
            );
        }
    };

    let access_token = match creds.access_token {
        Some(token) if !token.is_empty() => token,
        _ => {
            return (
                None,
                creds.refresh_token,
                gemini_oauth_client_from_options(creds.client_id, creds.client_secret),
                CredentialStatus::ParseError,
                Some("access_token is empty or missing".to_string()),
            );
        }
    };

    if creds
        .expiry_date
        .is_some_and(|expires_at| expires_at < now_millis())
    {
        return (
            Some(access_token),
            creds.refresh_token,
            gemini_oauth_client_from_options(creds.client_id, creds.client_secret),
            CredentialStatus::Expired,
            Some("Gemini access token has expired".to_string()),
        );
    }

    (
        Some(access_token),
        creds.refresh_token,
        gemini_oauth_client_from_options(creds.client_id, creds.client_secret),
        CredentialStatus::Valid,
        None,
    )
}

async fn refresh_gemini_token(
    refresh_token: &str,
    oauth_client: &GeminiOAuthClient,
) -> Option<String> {
    let client = crate::proxy::http_client::get();

    let response = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", oauth_client.client_id.as_str()),
            ("client_secret", oauth_client.client_secret.as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .ok()?;

    if !response.status().is_success() {
        return None;
    }

    let body: serde_json::Value = response.json().await.ok()?;
    body.get("access_token")?.as_str().map(String::from)
}

#[derive(Deserialize)]
struct GeminiLoadCodeAssistResponse {
    #[serde(rename = "cloudaicompanionProject")]
    cloudaicompanion_project: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct GeminiBucketInfo {
    #[serde(rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime")]
    reset_time: Option<String>,
    #[serde(rename = "modelId")]
    model_id: Option<String>,
}

#[derive(Deserialize)]
struct GeminiQuotaResponse {
    buckets: Option<Vec<GeminiBucketInfo>>,
}

fn extract_project_id(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Object(object) => object
            .get("id")
            .or_else(|| object.get("projectId"))
            .and_then(|value| value.as_str())
            .map(String::from),
        _ => None,
    }
}

fn classify_gemini_model(model_id: &str) -> &str {
    if model_id.contains("flash-lite") {
        "gemini_flash_lite"
    } else if model_id.contains("flash") {
        "gemini_flash"
    } else if model_id.contains("pro") {
        "gemini_pro"
    } else {
        model_id
    }
}

async fn query_gemini_quota(access_token: &str) -> SubscriptionQuota {
    let client = crate::proxy::http_client::get();

    let load_response = match client
        .post("https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist")
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "metadata": {
                "ideType": "GEMINI_CLI",
                "pluginType": "GEMINI"
            }
        }))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return SubscriptionQuota::error(
                "gemini",
                CredentialStatus::Valid,
                format!("Network error (loadCodeAssist): {error}"),
            );
        }
    };

    let load_status = load_response.status();
    if load_status == reqwest::StatusCode::UNAUTHORIZED
        || load_status == reqwest::StatusCode::FORBIDDEN
    {
        return SubscriptionQuota::error(
            "gemini",
            CredentialStatus::Expired,
            format!("Authentication failed (HTTP {load_status}). Please re-login with Gemini CLI."),
        );
    }

    if !load_status.is_success() {
        let body = load_response.text().await.unwrap_or_default();
        return SubscriptionQuota::error(
            "gemini",
            CredentialStatus::Valid,
            format!("loadCodeAssist failed (HTTP {load_status}): {body}"),
        );
    }

    let load_body: GeminiLoadCodeAssistResponse = match load_response.json().await {
        Ok(body) => body,
        Err(error) => {
            return SubscriptionQuota::error(
                "gemini",
                CredentialStatus::Valid,
                format!("Failed to parse loadCodeAssist response: {error}"),
            );
        }
    };

    let project_id = load_body
        .cloudaicompanion_project
        .as_ref()
        .and_then(extract_project_id);

    let mut quota_body = serde_json::json!({});
    if let Some(project_id) = project_id {
        quota_body["project"] = serde_json::Value::String(project_id);
    }

    let quota_response = match client
        .post("https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota")
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Content-Type", "application/json")
        .json(&quota_body)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return SubscriptionQuota::error(
                "gemini",
                CredentialStatus::Valid,
                format!("Network error (retrieveUserQuota): {error}"),
            );
        }
    };

    let quota_status = quota_response.status();
    if quota_status == reqwest::StatusCode::UNAUTHORIZED
        || quota_status == reqwest::StatusCode::FORBIDDEN
    {
        return SubscriptionQuota::error(
            "gemini",
            CredentialStatus::Expired,
            format!("Authentication failed (HTTP {quota_status})."),
        );
    }

    if !quota_status.is_success() {
        let body = quota_response.text().await.unwrap_or_default();
        return SubscriptionQuota::error(
            "gemini",
            CredentialStatus::Valid,
            format!("retrieveUserQuota failed (HTTP {quota_status}): {body}"),
        );
    }

    let quota_data: GeminiQuotaResponse = match quota_response.json().await {
        Ok(body) => body,
        Err(error) => {
            return SubscriptionQuota::error(
                "gemini",
                CredentialStatus::Valid,
                format!("Failed to parse quota response: {error}"),
            );
        }
    };

    let mut category_map: HashMap<String, (f64, Option<String>)> = HashMap::new();

    if let Some(buckets) = quota_data.buckets {
        for bucket in buckets {
            let model_id = bucket.model_id.as_deref().unwrap_or("unknown");
            let category = classify_gemini_model(model_id).to_string();
            let remaining = bucket.remaining_fraction.unwrap_or(1.0).clamp(0.0, 1.0);
            let entry = category_map
                .entry(category)
                .or_insert((remaining, bucket.reset_time.clone()));

            if remaining < entry.0 {
                entry.0 = remaining;
                if bucket.reset_time.is_some() {
                    entry.1.clone_from(&bucket.reset_time);
                }
            }
        }
    }

    let sort_order = |name: &str| -> usize {
        match name {
            "gemini_pro" => 0,
            "gemini_flash" => 1,
            "gemini_flash_lite" => 2,
            _ => 3,
        }
    };

    let mut tiers: Vec<QuotaTier> = category_map
        .into_iter()
        .map(|(name, (remaining, reset_time))| QuotaTier {
            name,
            utilization: (1.0 - remaining) * 100.0,
            resets_at: reset_time,
        })
        .collect();
    tiers.sort_by_key(|tier| sort_order(&tier.name));

    SubscriptionQuota {
        tool: "gemini".to_string(),
        credential_status: CredentialStatus::Valid,
        credential_message: None,
        success: true,
        tiers,
        extra_usage: None,
        error: None,
        queried_at: Some(now_millis()),
        reset_credits: None,
    }
}

pub async fn get_subscription_quota(tool: &str) -> Result<SubscriptionQuota, String> {
    match tool {
        "claude" => {
            let (token, status, message) = read_claude_credentials();
            match status {
                CredentialStatus::NotFound => Ok(SubscriptionQuota::not_found("claude")),
                CredentialStatus::ParseError => Ok(SubscriptionQuota::error(
                    "claude",
                    CredentialStatus::ParseError,
                    message.unwrap_or_else(|| "Failed to parse credentials".to_string()),
                )),
                CredentialStatus::Expired => {
                    if let Some(token) = token {
                        let result = query_claude_quota(&token).await;
                        if result.success {
                            return Ok(result);
                        }
                    }
                    Ok(SubscriptionQuota::error(
                        "claude",
                        CredentialStatus::Expired,
                        message.unwrap_or_else(|| "OAuth token has expired".to_string()),
                    ))
                }
                CredentialStatus::Valid => {
                    let Some(token) = token else {
                        return Ok(SubscriptionQuota::error(
                            "claude",
                            CredentialStatus::ParseError,
                            "accessToken is empty or missing".to_string(),
                        ));
                    };
                    Ok(query_claude_quota(&token).await)
                }
            }
        }
        "codex" => {
            let (token, account_id, status, message) = read_codex_credentials();
            match status {
                CredentialStatus::NotFound => Ok(SubscriptionQuota::not_found("codex")),
                CredentialStatus::ParseError => Ok(SubscriptionQuota::error(
                    "codex",
                    CredentialStatus::ParseError,
                    message.unwrap_or_else(|| "Failed to parse credentials".to_string()),
                )),
                CredentialStatus::Expired => {
                    if let Some(token) = token {
                        let result = query_codex_quota(
                            &token,
                            account_id.as_deref(),
                            "codex",
                            "Authentication failed. Please re-login with Codex CLI.",
                        )
                        .await;
                        if result.success {
                            return Ok(result);
                        }
                    }
                    Ok(SubscriptionQuota::error(
                        "codex",
                        CredentialStatus::Expired,
                        message.unwrap_or_else(|| "Codex OAuth token may be stale".to_string()),
                    ))
                }
                CredentialStatus::Valid => {
                    let Some(token) = token else {
                        return Ok(SubscriptionQuota::error(
                            "codex",
                            CredentialStatus::ParseError,
                            "access_token is empty or missing".to_string(),
                        ));
                    };
                    Ok(query_codex_quota(
                        &token,
                        account_id.as_deref(),
                        "codex",
                        "Authentication failed. Please re-login with Codex CLI.",
                    )
                    .await)
                }
            }
        }
        "gemini" => {
            let (token, refresh_token, oauth_client, status, message) = read_gemini_credentials();
            match status {
                CredentialStatus::NotFound => Ok(SubscriptionQuota::not_found("gemini")),
                CredentialStatus::ParseError => Ok(SubscriptionQuota::error(
                    "gemini",
                    CredentialStatus::ParseError,
                    message.unwrap_or_else(|| "Failed to parse credentials".to_string()),
                )),
                CredentialStatus::Expired => {
                    if let (Some(refresh_token), Some(oauth_client)) =
                        (refresh_token.as_deref(), oauth_client.as_ref())
                    {
                        if let Some(new_token) =
                            refresh_gemini_token(refresh_token, oauth_client).await
                        {
                            return Ok(query_gemini_quota(&new_token).await);
                        }
                    }
                    if let Some(ref token) = token {
                        let result = query_gemini_quota(token).await;
                        if result.success {
                            return Ok(result);
                        }
                    }
                    Ok(SubscriptionQuota::error(
                        "gemini",
                        CredentialStatus::Expired,
                        message.unwrap_or_else(|| "Gemini OAuth token has expired".to_string()),
                    ))
                }
                CredentialStatus::Valid => {
                    let Some(token) = token else {
                        return Ok(SubscriptionQuota::error(
                            "gemini",
                            CredentialStatus::ParseError,
                            "access_token is empty or missing".to_string(),
                        ));
                    };
                    Ok(query_gemini_quota(&token).await)
                }
            }
        }
        _ => Ok(SubscriptionQuota::not_found(tool)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_claude_credentials_accepts_current_and_legacy_keys() {
        let current =
            r#"{"claudeAiOauth":{"accessToken":"tok-current","expiresAt":4102444800000}}"#;
        let legacy = r#"{"claude.ai_oauth":{"accessToken":"tok-legacy","expiresAt":"2100-01-01T00:00:00Z"}}"#;

        assert_eq!(
            parse_claude_credentials_json(current),
            (
                Some("tok-current".to_string()),
                CredentialStatus::Valid,
                None
            )
        );
        assert_eq!(
            parse_claude_credentials_json(legacy),
            (
                Some("tok-legacy".to_string()),
                CredentialStatus::Valid,
                None
            )
        );
    }

    #[test]
    fn parse_claude_credentials_marks_expired_token() {
        let content = r#"{"claudeAiOauth":{"accessToken":"tok","expiresAt":1}}"#;

        let (token, status, message) = parse_claude_credentials_json(content);

        assert_eq!(token, Some("tok".to_string()));
        assert_eq!(status, CredentialStatus::Expired);
        assert_eq!(message, Some("OAuth token has expired".to_string()));
    }

    #[test]
    fn parse_codex_credentials_requires_chatgpt_auth_mode() {
        let content =
            r#"{"auth_mode":"apikey","tokens":{"access_token":"tok","account_id":"acc"}}"#;

        let (token, account_id, status, message) = parse_codex_credentials_json(content);

        assert_eq!(token, None);
        assert_eq!(account_id, None);
        assert_eq!(status, CredentialStatus::NotFound);
        assert_eq!(message, Some("Codex not using OAuth mode".to_string()));
    }

    #[test]
    fn parse_codex_credentials_returns_account_id_for_chatgpt_mode() {
        let content = r#"{
            "auth_mode":"chatgpt",
            "tokens":{"access_token":"tok","account_id":"acc"},
            "last_refresh":"2100-01-01T00:00:00Z"
        }"#;

        let (token, account_id, status, message) = parse_codex_credentials_json(content);

        assert_eq!(token, Some("tok".to_string()));
        assert_eq!(account_id, Some("acc".to_string()));
        assert_eq!(status, CredentialStatus::Valid);
        assert_eq!(message, None);
    }

    #[test]
    fn parse_codex_credentials_marks_stale_token() {
        let content = r#"{
            "auth_mode":"chatgpt",
            "tokens":{"access_token":"tok","account_id":"acc"},
            "last_refresh":"2000-01-01T00:00:00Z"
        }"#;

        let (token, account_id, status, message) = parse_codex_credentials_json(content);

        assert_eq!(token, Some("tok".to_string()));
        assert_eq!(account_id, Some("acc".to_string()));
        assert_eq!(status, CredentialStatus::Expired);
        assert_eq!(
            message,
            Some("Codex token may be stale (>8 days since last refresh)".to_string())
        );
    }

    #[test]
    fn parse_gemini_file_credentials_returns_refresh_token() {
        let content =
            r#"{"access_token":"tok","refresh_token":"refresh","expiry_date":4102444800000}"#;

        let (token, refresh_token, oauth_client, status, message) = parse_gemini_file_json(content);

        assert_eq!(token, Some("tok".to_string()));
        assert_eq!(refresh_token, Some("refresh".to_string()));
        assert!(oauth_client.is_none());
        assert_eq!(status, CredentialStatus::Valid);
        assert_eq!(message, None);
    }

    #[test]
    fn parse_gemini_file_credentials_reads_refresh_client_fields() {
        let content = r#"{"access_token":"tok","refresh_token":"refresh","client_id":"client-id","client_secret":"client-secret","expiry_date":1}"#;

        let (_, _, oauth_client, status, _) = parse_gemini_file_json(content);

        let oauth_client = oauth_client.expect("oauth client fields should be parsed");
        assert_eq!(oauth_client.client_id, "client-id");
        assert_eq!(oauth_client.client_secret, "client-secret");
        assert_eq!(status, CredentialStatus::Expired);
    }

    #[test]
    fn parse_gemini_file_credentials_marks_expired_token() {
        let content = r#"{"access_token":"tok","refresh_token":"refresh","expiry_date":1}"#;

        let (token, refresh_token, oauth_client, status, message) = parse_gemini_file_json(content);

        assert_eq!(token, Some("tok".to_string()));
        assert_eq!(refresh_token, Some("refresh".to_string()));
        assert!(oauth_client.is_none());
        assert_eq!(status, CredentialStatus::Expired);
        assert_eq!(message, Some("Gemini access token has expired".to_string()));
    }

    #[test]
    fn gemini_helpers_match_upstream_shapes() {
        assert_eq!(
            extract_project_id(&serde_json::json!({"projectId": "project-1"})),
            Some("project-1".to_string())
        );
        assert_eq!(
            extract_project_id(&serde_json::json!({"id": "project-2"})),
            Some("project-2".to_string())
        );
        assert_eq!(
            extract_project_id(&serde_json::json!("project-3")),
            Some("project-3".to_string())
        );

        assert_eq!(classify_gemini_model("gemini-3-pro"), "gemini_pro");
        assert_eq!(classify_gemini_model("gemini-3-flash"), "gemini_flash");
        assert_eq!(
            classify_gemini_model("gemini-3-flash-lite"),
            "gemini_flash_lite"
        );
    }

    #[test]
    fn subscription_quota_not_found_matches_upstream_shape() {
        let quota = SubscriptionQuota::not_found("codex_oauth");
        assert_eq!(quota.tool, "codex_oauth");
        assert_eq!(quota.credential_status, CredentialStatus::NotFound);
        assert!(!quota.success);
        assert!(quota.tiers.is_empty());
        assert!(quota.queried_at.is_none());
    }

    #[test]
    fn window_seconds_to_tier_name_matches_known_windows() {
        assert_eq!(window_seconds_to_tier_name(18_000), "five_hour");
        assert_eq!(window_seconds_to_tier_name(604_800), "seven_day");
        assert_eq!(window_seconds_to_tier_name(7_200), "2_hour");
        assert_eq!(window_seconds_to_tier_name(172_800), "2_day");
    }

    #[test]
    fn filter_and_sort_codex_reset_credits_applies_strict_rules() {
        let raw = vec![
            RawCodexResetCredit {
                id: Some("credit-expired".to_string()),
                reset_type: Some("codex_rate_limits".to_string()),
                status: Some("available".to_string()),
                is_supported_by_plan: Some(true),
                granted_at: Some("2025-01-01T00:00:00Z".to_string()),
                expires_at: Some("2026-10-22T00:00:00Z".to_string()),
                title: Some("Card Later".to_string()),
                description: None,
            },
            RawCodexResetCredit {
                id: Some("credit-earliest".to_string()),
                reset_type: Some("codex_rate_limits".to_string()),
                status: Some("available".to_string()),
                is_supported_by_plan: Some(true),
                granted_at: Some("2025-01-01T00:00:00Z".to_string()),
                expires_at: Some("2026-10-04T00:00:00Z".to_string()),
                title: Some("Card Earliest".to_string()),
                description: None,
            },
            // Wrong reset type
            RawCodexResetCredit {
                id: Some("credit-other-type".to_string()),
                reset_type: Some("chatgpt_messages".to_string()),
                status: Some("available".to_string()),
                is_supported_by_plan: Some(true),
                granted_at: Some("2025-01-01T00:00:00Z".to_string()),
                expires_at: Some("2026-10-01T00:00:00Z".to_string()),
                title: None,
                description: None,
            },
            // Redeemed / unavailable status
            RawCodexResetCredit {
                id: Some("credit-redeemed".to_string()),
                reset_type: Some("codex_rate_limits".to_string()),
                status: Some("redeemed".to_string()),
                is_supported_by_plan: Some(true),
                granted_at: Some("2025-01-01T00:00:00Z".to_string()),
                expires_at: Some("2026-10-01T00:00:00Z".to_string()),
                title: None,
                description: None,
            },
            // Unsupported by plan
            RawCodexResetCredit {
                id: Some("credit-unsupported".to_string()),
                reset_type: Some("codex_rate_limits".to_string()),
                status: Some("available".to_string()),
                is_supported_by_plan: Some(false),
                granted_at: Some("2025-01-01T00:00:00Z".to_string()),
                expires_at: Some("2026-10-01T00:00:00Z".to_string()),
                title: None,
                description: None,
            },
            // Missing granted_at
            RawCodexResetCredit {
                id: Some("credit-no-grant".to_string()),
                reset_type: Some("codex_rate_limits".to_string()),
                status: Some("available".to_string()),
                is_supported_by_plan: Some(true),
                granted_at: None,
                expires_at: Some("2026-10-01T00:00:00Z".to_string()),
                title: None,
                description: None,
            },
        ];

        let filtered = filter_and_sort_codex_reset_credits(raw);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].id, "credit-earliest");
        assert_eq!(filtered[1].id, "credit-expired");
    }

    #[test]
    fn parse_codex_usage_with_reset_credits() {
        let json = serde_json::json!({
            "rate_limit": {
                "primary_window": {
                    "used_percent": 12.5,
                    "limit_window_seconds": 18000,
                    "reset_at": 1700000000
                }
            },
            "rate_limit_reset_credits": {
                "available_count": 3,
                "applicable_available_count": 3
            }
        });

        let resp: CodexUsageResponse = serde_json::from_value(json).expect("deserialize CodexUsageResponse");
        let credits = resp.rate_limit_reset_credits.expect("credits present");
        assert_eq!(credits.available_count, Some(3));
        assert_eq!(credits.applicable_available_count, Some(3));
    }
}
