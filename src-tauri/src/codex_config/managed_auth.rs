//! Managed Codex login ownership and refresh synchronization, aligned with upstream.
use super::*;
use serde::{Deserialize, Serialize};

const CODEX_MANAGED_OAUTH_LIVE_AUTH_MARKER_FILENAME: &str = "codex_managed_oauth_live_auth.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CodexManagedOAuthLiveAuthMarker {
    version: u32,
    /// cc-switch 本地托管账号 ID，用于区分同一 ChatGPT workspace 下的登录。
    account_id: String,
    /// 原生 auth.json 的 `tokens.account_id`，即 ChatGPT workspace ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chatgpt_account_id: Option<String>,
    /// id_token 中跨刷新稳定的用户身份，防止同 workspace 的原生登录串号。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    user_identity: Option<String>,
}

pub(crate) struct CodexManagedLiveRefresh {
    pub(crate) refresh_token: String,
    pub(crate) id_token: Option<String>,
    pub(crate) last_refresh_ms: Option<i64>,
    pub(crate) chatgpt_account_id: String,
}

pub(crate) fn get_codex_managed_oauth_live_auth_marker_path() -> PathBuf {
    crate::config::get_app_config_dir().join(CODEX_MANAGED_OAUTH_LIVE_AUTH_MARKER_FILENAME)
}

fn extract_codex_managed_oauth_account_id(auth: &Value) -> Option<String> {
    let auth_obj = auth.as_object()?;

    if auth_obj.keys().any(|key| {
        !matches!(
            key.as_str(),
            "auth_mode" | "OPENAI_API_KEY" | "tokens" | "last_refresh"
        )
    }) {
        return None;
    }

    if auth.get("auth_mode").and_then(|value| value.as_str()) != Some("chatgpt") {
        return None;
    }

    let api_key_is_clearable = auth
        .get("OPENAI_API_KEY")
        .is_none_or(|value| value.is_null() || value.as_str() == Some("PROXY_MANAGED"));
    if !api_key_is_clearable {
        return None;
    }

    let tokens = auth.get("tokens").and_then(|value| value.as_object())?;

    if tokens.keys().any(|key| {
        !matches!(
            key.as_str(),
            "access_token" | "account_id" | "id_token" | "refresh_token"
        )
    }) {
        return None;
    }

    let account_id = tokens
        .get("account_id")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|id| !id.is_empty())?;
    tokens
        .get("access_token")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|token| !token.is_empty())?;

    Some(account_id.to_string())
}

pub fn codex_managed_oauth_auth_value(
    account_id: &str,
    access_token: &str,
    id_token: Option<&str>,
    refresh_token: &str,
    last_refresh: &str,
) -> Value {
    let mut tokens = serde_json::Map::new();
    if let Some(id_token) = id_token {
        tokens.insert("id_token".to_string(), Value::String(id_token.to_string()));
    }
    tokens.insert(
        "access_token".to_string(),
        Value::String(access_token.to_string()),
    );
    tokens.insert(
        "refresh_token".to_string(),
        Value::String(refresh_token.to_string()),
    );
    tokens.insert(
        "account_id".to_string(),
        Value::String(account_id.to_string()),
    );
    json!({
        "auth_mode": "chatgpt",
        "OPENAI_API_KEY": null,
        "tokens": Value::Object(tokens),
        "last_refresh": last_refresh,
    })
}

pub(crate) fn codex_managed_oauth_marker_bytes(
    auth: &Value,
    managed_account_id: &str,
) -> Result<Option<Vec<u8>>, AppError> {
    let managed_account_id = managed_account_id.trim();
    let Some(chatgpt_account_id) = extract_codex_managed_oauth_account_id(auth) else {
        return Ok(None);
    };
    if managed_account_id.is_empty() {
        return Ok(None);
    }
    let user_identity = extract_codex_auth_user_identity(auth).ok_or_else(|| {
        AppError::Message(
            "Codex 托管 OAuth auth.json 的 id_token 缺少稳定用户身份，无法安全记录账号所有权"
                .to_string(),
        )
    })?;
    let marker = CodexManagedOAuthLiveAuthMarker {
        version: 3,
        account_id: managed_account_id.to_string(),
        chatgpt_account_id: Some(chatgpt_account_id),
        user_identity: Some(user_identity),
    };
    serde_json::to_vec_pretty(&marker)
        .map(Some)
        .map_err(|e| AppError::Message(format!("序列化 Codex 托管账号标记失败: {e}")))
}

pub fn record_codex_managed_oauth_live_auth(
    auth: &Value,
    managed_account_id: &str,
) -> Result<(), AppError> {
    let managed_account_id = managed_account_id.trim();
    let Some(chatgpt_account_id) = extract_codex_managed_oauth_account_id(auth) else {
        return Ok(());
    };
    if managed_account_id.is_empty() {
        return Ok(());
    }
    let user_identity = extract_codex_auth_user_identity(auth).ok_or_else(|| {
        AppError::Message(
            "Codex 托管 OAuth auth.json 的 id_token 缺少稳定用户身份，无法安全记录账号所有权"
                .to_string(),
        )
    })?;

    let marker = CodexManagedOAuthLiveAuthMarker {
        version: 3,
        account_id: managed_account_id.to_string(),
        chatgpt_account_id: Some(chatgpt_account_id),
        user_identity: Some(user_identity),
    };
    crate::config::write_json_file(&get_codex_managed_oauth_live_auth_marker_path(), &marker)
}

fn migrate_legacy_codex_managed_oauth_live_auth_marker(
    auth: &Value,
    managed_account_id: &str,
    managed_id_token: Option<&str>,
) -> Result<(), AppError> {
    let marker_path = get_codex_managed_oauth_live_auth_marker_path();
    if !marker_path.exists() {
        return Ok(());
    }
    let marker: CodexManagedOAuthLiveAuthMarker = read_json_file(&marker_path)?;
    if !matches!(marker.version, 1 | 2) || marker.account_id != managed_account_id {
        return Ok(());
    }

    let auth_account_id = extract_codex_managed_oauth_account_id(auth);
    let auth_user_identity = extract_codex_auth_user_identity(auth);
    let managed_user_identity = managed_id_token.and_then(extract_codex_id_token_user_identity);
    if auth_account_id.as_deref() != Some(managed_account_id)
        || auth_user_identity.as_deref() != managed_user_identity.as_deref()
        || managed_user_identity.is_none()
    {
        return Err(AppError::Message(format!(
            "旧版 Codex OAuth 账号 {managed_account_id} 无法通过稳定用户身份确认磁盘凭据所有权；为避免覆盖或串用 auth.json，本次操作已取消，请在认证中心重新登录该账号"
        )));
    }

    record_codex_managed_oauth_live_auth(auth, managed_account_id)
}

pub fn codex_auth_matches_recorded_managed_oauth(
    auth: &Value,
    account_id: &str,
) -> Result<bool, AppError> {
    let account_id = account_id.trim();
    if account_id.is_empty() {
        return Ok(false);
    }

    let Some(auth_account_id) = extract_codex_managed_oauth_account_id(auth) else {
        return Ok(false);
    };
    let auth_user_identity = extract_codex_auth_user_identity(auth);
    let marker_path = get_codex_managed_oauth_live_auth_marker_path();
    // No marker is the normal state for a native ChatGPT login (no managed
    // account ever wrote auth.json, or it was cleaned up when switching away);
    // only a marker that exists but cannot be read is worth a warning.
    if !marker_path.exists() {
        return Ok(false);
    }
    let marker: CodexManagedOAuthLiveAuthMarker = match read_json_file(&marker_path) {
        Ok(marker) => marker,
        Err(err) => {
            log::warn!(
                "Failed to read Codex managed OAuth auth marker at {}: {err}",
                marker_path.display()
            );
            return Ok(false);
        }
    };

    // v1/v2 markers do not carry a stable user identity. Since multiple users
    // can share one workspace, those markers cannot safely authorize adopting
    // or deleting credentials. The next explicit activation replaces them
    // with a v3 marker.
    Ok(marker.account_id == account_id
        && match marker.version {
            3 => {
                marker.chatgpt_account_id.as_deref() == Some(auth_account_id.as_str())
                    && marker
                        .user_identity
                        .as_deref()
                        .is_some_and(|identity| auth_user_identity.as_deref() == Some(identity))
            }
            _ => false,
        })
}

pub fn ensure_codex_live_auth_unchanged_for_managed_account(
    account_id: &str,
    expected_refresh_token: &str,
) -> Result<(), AppError> {
    let auth_path = get_codex_auth_path();
    if !auth_path.exists() {
        return Err(AppError::Message(format!(
            "Codex CLI 账号 {account_id} 的 live auth 已在切换期间被移除，请重试"
        )));
    }
    let auth: Value = read_json_file(&auth_path)?;
    let current_refresh_token = auth
        .pointer("/tokens/refresh_token")
        .and_then(Value::as_str)
        .map(str::trim);
    if !codex_live_auth_is_managed_chatgpt_login(&auth, account_id)
        || current_refresh_token != Some(expected_refresh_token.trim())
    {
        return Err(AppError::Message(format!(
            "Codex CLI 账号 {account_id} 的 live 凭据在切换期间已刷新；为避免覆盖新 refresh token，本次操作已取消，请重试"
        )));
    }
    Ok(())
}

pub fn codex_live_auth_is_managed_chatgpt_login(auth: &Value, account_id: &str) -> bool {
    codex_auth_matches_recorded_managed_oauth(auth, account_id).unwrap_or(false)
}

pub fn read_codex_live_auth_refresh_for_account(
    account_id: &str,
) -> Option<(String, Option<String>, Option<i64>)> {
    let account_id = account_id.trim();
    if account_id.is_empty() {
        return None;
    }
    let auth_path = get_codex_auth_path();
    if !auth_path.exists() {
        return None;
    }
    let auth: Value = read_json_file(&auth_path).ok()?;
    // 仅在磁盘上确是「该 account_id 的 ChatGPT 登录」时才采纳其 refresh_token，
    // 避免从非 chatgpt/异常 auth 里误取 token。
    if !codex_live_auth_is_managed_chatgpt_login(&auth, account_id) {
        return None;
    }
    let tokens = auth.get("tokens")?.as_object()?;
    let refresh_token = tokens.get("refresh_token")?.as_str()?.trim().to_string();
    if refresh_token.is_empty() {
        return None;
    }
    let id_token = tokens
        .get("id_token")
        .and_then(|value| value.as_str())
        .map(|token| token.to_string());
    let last_refresh_ms = auth
        .get("last_refresh")
        .and_then(Value::as_str)
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp_millis());
    Some((refresh_token, id_token, last_refresh_ms))
}

pub(crate) fn read_codex_live_auth_refresh_for_managed_account(
    account_id: &str,
    managed_id_token: Option<&str>,
) -> Result<Option<CodexManagedLiveRefresh>, AppError> {
    let account_id = account_id.trim();
    if account_id.is_empty() || !get_codex_managed_oauth_live_auth_marker_path().exists() {
        return Ok(None);
    }
    let auth_path = get_codex_auth_path();
    if !auth_path.exists() {
        return Ok(None);
    }
    let auth: Value = read_json_file(&auth_path)?;
    migrate_legacy_codex_managed_oauth_live_auth_marker(&auth, account_id, managed_id_token)?;
    if !codex_auth_matches_recorded_managed_oauth(&auth, account_id)? {
        return Ok(None);
    }
    let Some((refresh_token, id_token, last_refresh_ms)) =
        read_codex_live_auth_refresh_for_account(account_id)
    else {
        return Ok(None);
    };
    let chatgpt_account_id = extract_codex_managed_oauth_account_id(&auth)
        .ok_or_else(|| AppError::Message("Codex live auth 缺少 workspace ID".to_string()))?;
    Ok(Some(CodexManagedLiveRefresh {
        refresh_token,
        id_token,
        last_refresh_ms,
        chatgpt_account_id,
    }))
}

pub fn sync_codex_managed_oauth_live_auth_after_refresh(
    account_id: &str,
    expected_refresh_token: &str,
    refreshed_auth: &Value,
) -> Result<bool, AppError> {
    let account_id = account_id.trim();
    let expected_refresh_token = expected_refresh_token.trim();
    if account_id.is_empty() || expected_refresh_token.is_empty() {
        return Ok(false);
    }

    let auth_path = get_codex_auth_path();
    if !auth_path.exists() {
        return Ok(false);
    }
    let current_auth: Value = read_json_file(&auth_path)?;
    if !codex_live_auth_is_managed_chatgpt_login(&current_auth, account_id) {
        return Ok(false);
    }
    let current_refresh_token = current_auth
        .pointer("/tokens/refresh_token")
        .and_then(Value::as_str)
        .map(str::trim);
    if current_refresh_token != Some(expected_refresh_token) {
        return Ok(false);
    }

    let marker_path = get_codex_managed_oauth_live_auth_marker_path();
    let was_recorded_managed = marker_path.exists()
        && codex_auth_matches_recorded_managed_oauth(&current_auth, account_id)?;

    let bytes = serde_json::to_vec_pretty(refreshed_auth)
        .map_err(|source| AppError::JsonSerialize { source })?;
    crate::config::atomic_write_private(&auth_path, &bytes)?;
    if was_recorded_managed {
        record_codex_managed_oauth_live_auth(refreshed_auth, account_id)?;
    }
    Ok(true)
}

pub(crate) fn prepare_codex_live_auth_for_managed_account_removal(
    managed_account_id: &str,
    managed_id_token: Option<&str>,
) -> Result<(), AppError> {
    if !get_codex_managed_oauth_live_auth_marker_path().exists() {
        return Ok(());
    }
    let auth_path = get_codex_auth_path();
    if !auth_path.exists() {
        return Ok(());
    }
    let auth: Value = read_json_file(&auth_path)?;
    migrate_legacy_codex_managed_oauth_live_auth_marker(&auth, managed_account_id, managed_id_token)
}

pub(crate) fn clear_codex_managed_oauth_live_auth_marker_for_account(
    account_id: &str,
) -> Result<(), AppError> {
    let marker_path = get_codex_managed_oauth_live_auth_marker_path();
    if !marker_path.exists() {
        return Ok(());
    }
    let marker: CodexManagedOAuthLiveAuthMarker = match read_json_file(&marker_path) {
        Ok(marker) => marker,
        Err(error) => {
            log::warn!(
                "Failed to read Codex managed OAuth auth marker at {} while cleaning account {}: {error}",
                marker_path.display(),
                account_id
            );
            // A malformed marker cannot establish ownership for any account
            // and is unusable for rollback/synchronization; remove the stale
            // bookkeeping file while leaving non-matching live auth untouched.
            return delete_file(&marker_path);
        }
    };
    if marker.account_id == account_id.trim() {
        delete_file(&marker_path)?;
    }
    Ok(())
}

pub fn clear_codex_live_auth_for_managed_account(account_id: &str) -> Result<(), AppError> {
    if !get_codex_managed_oauth_live_auth_marker_path().exists() {
        return Ok(());
    }
    clear_codex_live_auth_for_managed_account_if_unchanged(account_id, None)
}

pub fn clear_codex_live_auth_for_managed_account_if_unchanged(
    account_id: &str,
    expected_refresh_token: Option<&str>,
) -> Result<(), AppError> {
    let auth_path = get_codex_auth_path();
    let mut removed_matching_auth = false;
    if auth_path.exists() {
        let auth: Value = read_json_file(&auth_path)?;
        if codex_live_auth_is_managed_chatgpt_login(&auth, account_id) {
            if let Some(expected_refresh_token) = expected_refresh_token {
                let current_refresh_token = auth
                    .pointer("/tokens/refresh_token")
                    .and_then(Value::as_str)
                    .map(str::trim);
                if current_refresh_token != Some(expected_refresh_token.trim()) {
                    return Err(AppError::Message(format!(
                        "Codex CLI 账号 {account_id} 的 live 凭据在切换期间已刷新；为避免删除新 refresh token，本次操作已取消，请重试"
                    )));
                }
            }
            delete_file(&auth_path)?;
            removed_matching_auth = true;
        }
    }

    if removed_matching_auth {
        // Once the matching live file is gone, any marker is stale regardless
        // of version or parseability.
        delete_file(&get_codex_managed_oauth_live_auth_marker_path())?;
    } else {
        clear_codex_managed_oauth_live_auth_marker_for_account(account_id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_ownership_distinguishes_members_of_the_same_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());
        let auth = codex_managed_oauth_auth_value(
            "workspace",
            "access-a",
            Some(&test_codex_id_token("alice")),
            "refresh-a",
            "2026-11-05T00:00:00Z",
        );
        crate::config::write_json_file(&get_codex_auth_path(), &auth).unwrap();
        assert!(!codex_live_auth_is_managed_chatgpt_login(&auth, "local-a"));
        record_codex_managed_oauth_live_auth(&auth, "local-a").unwrap();
        assert!(codex_live_auth_is_managed_chatgpt_login(&auth, "local-a"));
        assert!(!codex_live_auth_is_managed_chatgpt_login(&auth, "local-b"));
        let mut other_user = auth.clone();
        other_user["tokens"]["id_token"] = json!(test_codex_id_token("bob"));
        assert!(!codex_live_auth_is_managed_chatgpt_login(
            &other_user,
            "local-a"
        ));
        let mut refreshed = auth.clone();
        refreshed["tokens"]["refresh_token"] = json!("refresh-b");
        assert!(!sync_codex_managed_oauth_live_auth_after_refresh(
            "local-a",
            "wrong-generation",
            &refreshed
        )
        .unwrap());
        assert_eq!(
            read_json_file::<Value>(&get_codex_auth_path()).unwrap(),
            auth
        );
        assert!(sync_codex_managed_oauth_live_auth_after_refresh(
            "local-a",
            "refresh-a",
            &refreshed
        )
        .unwrap());
        assert_eq!(
            read_json_file::<Value>(&get_codex_auth_path()).unwrap(),
            refreshed
        );
        assert!(
            ensure_codex_live_auth_unchanged_for_managed_account("local-a", "refresh-a").is_err()
        );
    }
}
