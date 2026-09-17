use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use tokio::sync::{Mutex, RwLock};

const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const DEVICE_AUTH_USERCODE_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
const DEVICE_AUTH_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
const OAUTH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const DEVICE_VERIFICATION_URL: &str = "https://auth.openai.com/codex/device";
const DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
const TOKEN_REFRESH_BUFFER_MS: i64 = 60_000;
const DEVICE_CODE_DEFAULT_EXPIRES_IN: u64 = 900;
const POLLING_SAFETY_MARGIN_SECS: u64 = 3;
const CODEX_USER_AGENT: &str = "cc-switch-codex-oauth";

#[derive(Debug, thiserror::Error)]
pub enum CodexOAuthError {
    #[error("等待用户授权中")]
    AuthorizationPending,
    #[error("用户拒绝授权")]
    #[allow(dead_code)]
    AccessDenied,
    #[error("Device Code 已过期")]
    ExpiredToken,
    #[error("OAuth Token 获取失败: {0}")]
    TokenFetchFailed(String),
    #[error("Refresh Token 失效或已过期")]
    RefreshTokenInvalid,
    #[error("网络错误: {0}")]
    NetworkError(String),
    #[error("解析错误: {0}")]
    ParseError(String),
    #[error("IO 错误: {0}")]
    IoError(String),
    #[error("账号不存在: {0}")]
    AccountNotFound(String),
}

impl From<reqwest::Error> for CodexOAuthError {
    fn from(err: reqwest::Error) -> Self {
        CodexOAuthError::NetworkError(err.to_string())
    }
}

impl From<std::io::Error> for CodexOAuthError {
    fn from(err: std::io::Error) -> Self {
        CodexOAuthError::IoError(err.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedAuthAccount {
    pub id: String,
    pub login: String,
    pub avatar_url: Option<String>,
    pub authenticated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedAuthDeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexOAuthStatus {
    pub accounts: Vec<ManagedAuthAccount>,
    pub default_account_id: Option<String>,
    pub authenticated: bool,
    pub username: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeviceCodeResponse {
    device_auth_id: String,
    user_code: String,
    #[serde(default)]
    interval: Option<serde_json::Value>,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
struct DevicePollSuccess {
    authorization_code: String,
    code_verifier: String,
}

#[derive(Debug, Clone, Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct IdTokenClaims {
    #[serde(default)]
    chatgpt_account_id: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    organizations: Vec<OrgClaim>,
    #[serde(default, rename = "https://api.openai.com/auth")]
    openai_auth: Option<OpenAiAuthClaim>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct OrgClaim {
    #[serde(default)]
    id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct OpenAiAuthClaim {
    #[serde(default)]
    chatgpt_account_id: Option<String>,
}

#[derive(Debug, Clone)]
struct CachedAccessToken {
    token: String,
    expires_at_ms: i64,
}

impl CachedAccessToken {
    fn is_expiring_soon(&self) -> bool {
        let now = chrono::Utc::now().timestamp_millis();
        self.expires_at_ms - now < TOKEN_REFRESH_BUFFER_MS
    }
}

#[derive(Debug, Clone)]
struct PendingDeviceCode {
    user_code: String,
    expires_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CodexAccountData {
    pub account_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub refresh_token: String,
    pub authenticated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_auth: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_native_sync: Option<crate::services::codex_account::NativeAuthCopies>,
}

impl From<&CodexAccountData> for ManagedAuthAccount {
    fn from(data: &CodexAccountData) -> Self {
        Self {
            id: data.account_id.clone(),
            login: data
                .email
                .clone()
                .unwrap_or_else(|| format!("ChatGPT ({})", &data.account_id)),
            avatar_url: None,
            authenticated_at: data.authenticated_at,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CodexOAuthStore {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    accounts: HashMap<String, CodexAccountData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_account_id: Option<String>,
}

pub struct CodexOAuthManager {
    accounts: std::sync::Arc<RwLock<HashMap<String, CodexAccountData>>>,
    default_account_id: std::sync::Arc<RwLock<Option<String>>>,
    access_tokens: std::sync::Arc<RwLock<HashMap<String, CachedAccessToken>>>,
    refresh_locks: std::sync::Arc<RwLock<HashMap<String, std::sync::Arc<Mutex<()>>>>>,
    pending_device_codes: std::sync::Arc<RwLock<HashMap<String, PendingDeviceCode>>>,
    storage_path: PathBuf,
    #[cfg(test)]
    token_endpoint: Option<String>,
}

impl CodexOAuthManager {
    pub fn new(data_dir: PathBuf) -> Self {
        let storage_path = data_dir.join("codex_oauth_auth.json");
        let manager = Self {
            accounts: std::sync::Arc::new(RwLock::new(HashMap::new())),
            default_account_id: std::sync::Arc::new(RwLock::new(None)),
            access_tokens: std::sync::Arc::new(RwLock::new(HashMap::new())),
            refresh_locks: std::sync::Arc::new(RwLock::new(HashMap::new())),
            pending_device_codes: std::sync::Arc::new(RwLock::new(HashMap::new())),
            storage_path,
            #[cfg(test)]
            token_endpoint: None,
        };

        if let Err(e) = manager.load_from_disk_sync() {
            log::warn!("[CodexOAuth] 加载存储失败: {e}");
        }

        manager
    }

    pub(crate) async fn lock_store(&self) -> Result<fs::File, CodexOAuthError> {
        let path = self.storage_path.with_extension("lock");
        tokio::task::spawn_blocking(move || {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(path)?;
            file.lock()?;
            Ok::<_, std::io::Error>(file)
        })
        .await
        .map_err(|_| CodexOAuthError::IoError("Account lock worker failed".into()))?
        .map_err(Into::into)
    }

    pub async fn start_device_flow(
        &self,
    ) -> Result<ManagedAuthDeviceCodeResponse, CodexOAuthError> {
        let response = crate::proxy::http_client::get()
            .post(DEVICE_AUTH_USERCODE_URL)
            .header("Content-Type", "application/json")
            .header("User-Agent", CODEX_USER_AGENT)
            .json(&serde_json::json!({ "client_id": CODEX_CLIENT_ID }))
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(CodexOAuthError::NetworkError(format!(
                "Device Code 请求失败: {status} - {text}"
            )));
        }

        let device: DeviceCodeResponse = response
            .json()
            .await
            .map_err(|e| CodexOAuthError::ParseError(e.to_string()))?;

        let interval = parse_interval(device.interval.as_ref());
        let expires_in = device.expires_in.unwrap_or(DEVICE_CODE_DEFAULT_EXPIRES_IN);
        let expires_at_ms = chrono::Utc::now().timestamp_millis() + (expires_in as i64) * 1000;

        {
            let mut pending = self.pending_device_codes.write().await;
            let now_ms = chrono::Utc::now().timestamp_millis();
            pending.retain(|_, entry| entry.expires_at_ms > now_ms);
            pending.insert(
                device.device_auth_id.clone(),
                PendingDeviceCode {
                    user_code: device.user_code.clone(),
                    expires_at_ms,
                },
            );
        }

        Ok(ManagedAuthDeviceCodeResponse {
            device_code: device.device_auth_id,
            user_code: device.user_code,
            verification_uri: DEVICE_VERIFICATION_URL.to_string(),
            expires_in,
            interval,
        })
    }

    pub async fn poll_for_token(
        &self,
        device_code: &str,
    ) -> Result<Option<ManagedAuthAccount>, CodexOAuthError> {
        let _lock = self.lock_store().await?;
        self.reload_from_disk().await?;
        self.poll_for_token_locked(device_code).await
    }

    pub(crate) async fn poll_for_token_locked(
        &self,
        device_code: &str,
    ) -> Result<Option<ManagedAuthAccount>, CodexOAuthError> {
        let entry = {
            let pending = self.pending_device_codes.read().await;
            pending.get(device_code).cloned()
        }
        .ok_or_else(|| {
            CodexOAuthError::TokenFetchFailed(
                "未找到对应的 user_code，请重新启动登录流程".to_string(),
            )
        })?;

        if entry.expires_at_ms <= chrono::Utc::now().timestamp_millis() {
            let mut pending = self.pending_device_codes.write().await;
            pending.remove(device_code);
            return Err(CodexOAuthError::ExpiredToken);
        }

        let poll_response = crate::proxy::http_client::get()
            .post(DEVICE_AUTH_TOKEN_URL)
            .header("Content-Type", "application/json")
            .header("User-Agent", CODEX_USER_AGENT)
            .json(&serde_json::json!({
                "device_auth_id": device_code,
                "user_code": entry.user_code,
            }))
            .send()
            .await?;

        let status = poll_response.status();
        if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::NOT_FOUND {
            return Err(CodexOAuthError::AuthorizationPending);
        }
        if status == reqwest::StatusCode::GONE {
            return Err(CodexOAuthError::ExpiredToken);
        }
        if !status.is_success() {
            let text = poll_response.text().await.unwrap_or_default();
            return Err(CodexOAuthError::TokenFetchFailed(format!(
                "{status} - {text}"
            )));
        }

        let success: DevicePollSuccess = poll_response
            .json()
            .await
            .map_err(|e| CodexOAuthError::ParseError(e.to_string()))?;

        let tokens = self
            .exchange_code_for_tokens(&success.authorization_code, &success.code_verifier)
            .await?;

        {
            let mut pending = self.pending_device_codes.write().await;
            pending.remove(device_code);
        }

        let refresh_token = tokens.refresh_token.clone().ok_or_else(|| {
            CodexOAuthError::TokenFetchFailed("响应缺少 refresh_token".to_string())
        })?;
        let (account_id, email) = extract_identity_from_tokens(&tokens);
        let account_id = account_id.ok_or_else(|| {
            CodexOAuthError::ParseError("无法从 token 中提取 account_id".to_string())
        })?;

        {
            let mut tokens_cache = self.access_tokens.write().await;
            tokens_cache.insert(
                account_id.clone(),
                CachedAccessToken {
                    token: tokens.access_token.clone(),
                    expires_at_ms: compute_expires_at_ms(tokens.expires_in),
                },
            );
        }

        let account = self
            .add_account_internal(account_id, refresh_token, email)
            .await?;

        self.remember_token_response(&account.id, &tokens, None)
            .await?;
        Ok(Some(account))
    }

    async fn exchange_code_for_tokens(
        &self,
        code: &str,
        code_verifier: &str,
    ) -> Result<OAuthTokenResponse, CodexOAuthError> {
        let response = crate::proxy::http_client::get()
            .post(OAUTH_TOKEN_URL)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("User-Agent", CODEX_USER_AGENT)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", DEVICE_REDIRECT_URI),
                ("client_id", CODEX_CLIENT_ID),
                ("code_verifier", code_verifier),
            ])
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(CodexOAuthError::TokenFetchFailed(format!(
                "Token 交换失败: {status} - {text}"
            )));
        }

        response
            .json()
            .await
            .map_err(|e| CodexOAuthError::ParseError(e.to_string()))
    }

    fn refresh_endpoint(&self) -> &str {
        #[cfg(test)]
        if let Some(endpoint) = self.token_endpoint.as_deref() {
            return endpoint;
        }
        OAUTH_TOKEN_URL
    }

    async fn refresh_with_token(
        &self,
        refresh_token: &str,
    ) -> Result<OAuthTokenResponse, CodexOAuthError> {
        let response = crate::proxy::http_client::get()
            .post(self.refresh_endpoint())
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("User-Agent", CODEX_USER_AGENT)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", CODEX_CLIENT_ID),
                ("scope", "openid profile email"),
            ])
            .send()
            .await?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(CodexOAuthError::RefreshTokenInvalid);
        }

        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(CodexOAuthError::TokenFetchFailed(format!(
                "Refresh 失败: {status} - {text}"
            )));
        }

        response
            .json()
            .await
            .map_err(|e| CodexOAuthError::ParseError(e.to_string()))
    }

    /// Retain the full credential bundle needed by a standalone Codex process.
    async fn remember_token_response(
        &self,
        account_id: &str,
        tokens: &OAuthTokenResponse,
        copies: Option<crate::services::codex_account::NativeAuthCopies>,
    ) -> Result<(), CodexOAuthError> {
        let mut accounts = self.accounts.write().await;
        let account = accounts
            .get_mut(account_id)
            .ok_or_else(|| CodexOAuthError::AccountNotFound(account_id.to_string()))?;
        if let Some(refresh) = &tokens.refresh_token {
            account.refresh_token = refresh.clone();
        }
        let id_token = tokens.id_token.clone().or_else(|| {
            account
                .codex_auth
                .as_ref()
                .and_then(|a| a.pointer("/tokens/id_token"))
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        });
        account.pending_native_sync = copies;
        account.codex_auth = Some(serde_json::json!({
            "auth_mode": "chatgpt", "OPENAI_API_KEY": null,
            "tokens": {"account_id": account_id, "access_token": tokens.access_token,
                "refresh_token": account.refresh_token, "id_token": id_token},
            "last_refresh": chrono::Utc::now().to_rfc3339(),
        }));
        drop(accounts);
        self.save_to_disk().await
    }

    /// Import credentials refreshed by Codex without adding unrelated live accounts.
    pub(crate) async fn capture_codex_auth(
        &self,
        auth: &serde_json::Value,
    ) -> Result<(), CodexOAuthError> {
        if auth
            .get("auth_mode")
            .and_then(|v| v.as_str())
            .is_some_and(|mode| mode != "chatgpt")
        {
            return Ok(());
        }
        let Some(id) = auth.pointer("/tokens/account_id").and_then(|v| v.as_str()) else {
            return Ok(());
        };
        let Some(refresh) = auth
            .pointer("/tokens/refresh_token")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
        else {
            return Ok(());
        };
        self.reconcile_native_copies(id).await?;
        let mut accounts = self.accounts.write().await;
        let Some(account) = accounts.get_mut(id) else {
            return Ok(());
        };
        if let Some(saved) = account.codex_auth.as_ref() {
            if saved == auth {
                return Ok(());
            }
            let timestamp = |v: &serde_json::Value| {
                v.get("last_refresh")
                    .and_then(|x| x.as_str())
                    .and_then(|x| chrono::DateTime::parse_from_rfc3339(x).ok())
            };
            if timestamp(saved).is_some() && timestamp(auth) < timestamp(saved) {
                return Ok(());
            }
        }
        let copies =
            crate::services::codex_account::NativeAuthCopies::read(id, &account.refresh_token)
                .map_err(CodexOAuthError::IoError)?;
        account.pending_native_sync = Some(copies);
        account.refresh_token = refresh.to_string();
        account.codex_auth = Some(auth.clone());
        drop(accounts);
        self.access_tokens.write().await.remove(id);
        self.save_to_disk().await?;
        self.reconcile_native_copies(id).await
    }

    /// The new token and its pending destinations are committed together. Retrying
    /// is safe after partial publication: destination hashes protect explicit edits.
    async fn reconcile_native_copies(&self, account_id: &str) -> Result<(), CodexOAuthError> {
        let pending = self
            .accounts
            .read()
            .await
            .get(account_id)
            .and_then(|account| {
                account
                    .pending_native_sync
                    .clone()
                    .zip(account.codex_auth.clone())
            });
        let Some((copies, auth)) = pending else {
            return Ok(());
        };
        copies.publish(&auth).map_err(CodexOAuthError::IoError)?;
        if let Some(account) = self.accounts.write().await.get_mut(account_id) {
            account.pending_native_sync = None;
        }
        self.save_to_disk().await
    }

    pub(crate) async fn contains_account(&self, account_id: &str) -> bool {
        self.accounts.read().await.contains_key(account_id)
    }

    pub(crate) async fn export_codex_auth(
        &self,
        account_id: &str,
    ) -> Result<serde_json::Value, CodexOAuthError> {
        self.reconcile_native_copies(account_id).await?;
        let saved = self
            .accounts
            .read()
            .await
            .get(account_id)
            .ok_or_else(|| CodexOAuthError::AccountNotFound(account_id.to_string()))?
            .codex_auth
            .clone();
        let usable = saved
            .as_ref()
            .and_then(stored_access_expiration)
            .is_some_and(|exp| {
                exp > chrono::Utc::now().timestamp_millis() + TOKEN_REFRESH_BUFFER_MS
            });
        if !usable {
            self.access_tokens.write().await.remove(account_id);
            self.get_valid_token_for_account_locked(account_id).await?;
        }
        let accounts = self.accounts.read().await;
        let auth = accounts
            .get(account_id)
            .and_then(|a| a.codex_auth.clone())
            .ok_or_else(|| {
                CodexOAuthError::ParseError("Missing Codex credentials; sign in again.".into())
            })?;
        if auth.pointer("/tokens/account_id").and_then(|v| v.as_str()) != Some(account_id) {
            return Err(CodexOAuthError::ParseError(
                "Stored Codex account identity mismatch".into(),
            ));
        }
        for key in ["id_token", "access_token", "refresh_token"] {
            if auth["tokens"][key].as_str().is_none_or(str::is_empty) {
                return Err(CodexOAuthError::ParseError(
                    "Incomplete Codex credentials; sign in again.".into(),
                ));
            }
        }
        Ok(auth)
    }

    pub async fn get_valid_token_for_account(
        &self,
        account_id: &str,
    ) -> Result<String, CodexOAuthError> {
        let _state_guard = crate::services::state_coordination::acquire_restore_mutation_guard()
            .await
            .map_err(CodexOAuthError::IoError)?;
        let _lock = self.lock_store().await?;
        self.reload_from_disk().await?;
        self.get_valid_token_for_account_locked(account_id).await
    }

    pub(crate) async fn get_valid_token_for_account_locked(
        &self,
        account_id: &str,
    ) -> Result<String, CodexOAuthError> {
        self.reconcile_native_copies(account_id).await?;
        let live_path = crate::codex_config::get_codex_auth_path();
        let live_before = fs::read(&live_path).ok();
        if let Some(live) = live_before
            .as_ref()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(bytes).ok())
        {
            if live.pointer("/tokens/account_id").and_then(|v| v.as_str()) == Some(account_id) {
                self.capture_codex_auth(&live).await?;
            }
        }
        if let Some(auth) = self
            .accounts
            .read()
            .await
            .get(account_id)
            .and_then(|a| a.codex_auth.as_ref())
        {
            if let Some(expires_at_ms) = stored_access_expiration(auth) {
                if expires_at_ms > chrono::Utc::now().timestamp_millis() + TOKEN_REFRESH_BUFFER_MS {
                    self.access_tokens.write().await.insert(
                        account_id.to_string(),
                        CachedAccessToken {
                            token: auth["tokens"]["access_token"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                            expires_at_ms,
                        },
                    );
                }
            }
        }
        {
            let tokens = self.access_tokens.read().await;
            if let Some(cached) = tokens.get(account_id) {
                if !cached.is_expiring_soon() {
                    return Ok(cached.token.clone());
                }
            }
        }

        let refresh_lock = self.get_refresh_lock(account_id).await;
        let _guard = refresh_lock.lock().await;

        {
            let tokens = self.access_tokens.read().await;
            if let Some(cached) = tokens.get(account_id) {
                if !cached.is_expiring_soon() {
                    return Ok(cached.token.clone());
                }
            }
        }

        let refresh_token = {
            let accounts = self.accounts.read().await;
            accounts
                .get(account_id)
                .map(|a| a.refresh_token.clone())
                .ok_or_else(|| CodexOAuthError::AccountNotFound(account_id.to_string()))?
        };

        let copies =
            crate::services::codex_account::NativeAuthCopies::read(account_id, &refresh_token)
                .map_err(CodexOAuthError::IoError)?;
        let new_tokens = self.refresh_with_token(&refresh_token).await?;

        self.remember_token_response(account_id, &new_tokens, Some(copies))
            .await?;
        self.reconcile_native_copies(account_id).await?;

        let access_token = new_tokens.access_token.clone();
        let expires_at_ms = compute_expires_at_ms(new_tokens.expires_in);

        {
            let mut tokens = self.access_tokens.write().await;
            tokens.insert(
                account_id.to_string(),
                CachedAccessToken {
                    token: access_token.clone(),
                    expires_at_ms,
                },
            );
        }

        Ok(access_token)
    }

    pub async fn get_valid_token(&self) -> Result<String, CodexOAuthError> {
        let _state_guard = crate::services::state_coordination::acquire_restore_mutation_guard()
            .await
            .map_err(CodexOAuthError::IoError)?;
        let _lock = self.lock_store().await?;
        self.reload_from_disk().await?;
        match self.resolve_default_account_id().await {
            Some(id) => self.get_valid_token_for_account_locked(&id).await,
            None => Err(CodexOAuthError::AccountNotFound(
                "无可用的 ChatGPT 账号".to_string(),
            )),
        }
    }

    pub async fn default_account_id(&self) -> Option<String> {
        self.get_status().await.default_account_id
    }

    #[allow(dead_code)]
    pub async fn list_accounts(&self) -> Vec<ManagedAuthAccount> {
        self.get_status().await.accounts
    }

    pub async fn remove_account(&self, account_id: &str) -> Result<(), CodexOAuthError> {
        let _lock = self.lock_store().await?;
        self.reload_from_disk().await?;
        self.remove_account_locked(account_id).await
    }

    pub(crate) async fn remove_account_locked(
        &self,
        account_id: &str,
    ) -> Result<(), CodexOAuthError> {
        {
            let mut accounts = self.accounts.write().await;
            if accounts.remove(account_id).is_none() {
                return Err(CodexOAuthError::AccountNotFound(account_id.to_string()));
            }
        }

        self.access_tokens.write().await.remove(account_id);
        self.refresh_locks.write().await.remove(account_id);

        {
            let accounts = self.accounts.read().await;
            let mut default = self.default_account_id.write().await;
            if default.as_deref() == Some(account_id) {
                *default = Self::fallback_default_account_id(&accounts);
            }
        }

        self.save_to_disk().await?;
        Ok(())
    }

    pub async fn set_default_account(&self, account_id: &str) -> Result<(), CodexOAuthError> {
        let _lock = self.lock_store().await?;
        self.reload_from_disk().await?;
        self.set_default_account_locked(account_id).await
    }

    pub(crate) async fn set_default_account_locked(
        &self,
        account_id: &str,
    ) -> Result<(), CodexOAuthError> {
        {
            let accounts = self.accounts.read().await;
            if !accounts.contains_key(account_id) {
                return Err(CodexOAuthError::AccountNotFound(account_id.to_string()));
            }
        }

        let previous = self.default_account_id.read().await.clone();
        *self.default_account_id.write().await = Some(account_id.to_string());
        if let Err(error) = self.save_to_disk().await {
            *self.default_account_id.write().await = previous;
            return Err(error);
        }
        Ok(())
    }

    pub async fn clear_auth(&self) -> Result<(), CodexOAuthError> {
        let _lock = self.lock_store().await?;
        self.reload_from_disk().await?;
        self.clear_auth_locked().await
    }

    pub(crate) async fn clear_auth_locked(&self) -> Result<(), CodexOAuthError> {
        self.accounts.write().await.clear();
        *self.default_account_id.write().await = None;
        self.access_tokens.write().await.clear();
        self.refresh_locks.write().await.clear();
        self.pending_device_codes.write().await.clear();

        if self.storage_path.exists() {
            std::fs::remove_file(&self.storage_path)?;
        }

        Ok(())
    }

    #[allow(dead_code)]
    pub async fn is_authenticated(&self) -> bool {
        !self.accounts.read().await.is_empty()
    }

    pub async fn get_status(&self) -> CodexOAuthStatus {
        let _lock = self.lock_store().await.ok();
        if _lock.is_some() {
            let _ = self.reload_from_disk().await;
        }
        let accounts_map = self.accounts.read().await.clone();
        let default_id = self.resolve_default_account_id().await;
        let account_list = Self::sorted_accounts(&accounts_map, default_id.as_deref());
        let authenticated = !account_list.is_empty();
        let username = default_id
            .as_ref()
            .and_then(|id| accounts_map.get(id))
            .and_then(|a| a.email.clone())
            .or_else(|| account_list.first().map(|a| a.login.clone()));

        CodexOAuthStatus {
            accounts: account_list,
            default_account_id: default_id,
            authenticated,
            username,
        }
    }

    async fn add_account_internal(
        &self,
        account_id: String,
        refresh_token: String,
        email: Option<String>,
    ) -> Result<ManagedAuthAccount, CodexOAuthError> {
        let now = chrono::Utc::now().timestamp();
        let data = CodexAccountData {
            account_id: account_id.clone(),
            email,
            refresh_token,
            authenticated_at: now,
            codex_auth: None,
            pending_native_sync: None,
        };
        let account = ManagedAuthAccount::from(&data);

        self.accounts.write().await.insert(account_id.clone(), data);
        {
            let mut default = self.default_account_id.write().await;
            if default.is_none() {
                *default = Some(account_id);
            }
        }

        self.save_to_disk().await?;
        Ok(account)
    }

    fn fallback_default_account_id(accounts: &HashMap<String, CodexAccountData>) -> Option<String> {
        accounts
            .iter()
            .max_by(|(id_a, a), (id_b, b)| {
                a.authenticated_at
                    .cmp(&b.authenticated_at)
                    .then_with(|| id_b.cmp(id_a))
            })
            .map(|(id, _)| id.clone())
    }

    fn sorted_accounts(
        accounts: &HashMap<String, CodexAccountData>,
        default_account_id: Option<&str>,
    ) -> Vec<ManagedAuthAccount> {
        let mut list: Vec<ManagedAuthAccount> =
            accounts.values().map(ManagedAuthAccount::from).collect();
        list.sort_by(|a, b| {
            let a_default = default_account_id == Some(a.id.as_str());
            let b_default = default_account_id == Some(b.id.as_str());
            b_default
                .cmp(&a_default)
                .then_with(|| b.authenticated_at.cmp(&a.authenticated_at))
                .then_with(|| a.login.cmp(&b.login))
        });
        list
    }

    async fn resolve_default_account_id(&self) -> Option<String> {
        let stored = self.default_account_id.read().await.clone();
        let accounts = self.accounts.read().await;
        if let Some(id) = stored {
            if accounts.contains_key(&id) {
                return Some(id);
            }
        }
        Self::fallback_default_account_id(&accounts)
    }

    async fn get_refresh_lock(&self, account_id: &str) -> std::sync::Arc<Mutex<()>> {
        {
            let locks = self.refresh_locks.read().await;
            if let Some(lock) = locks.get(account_id) {
                return std::sync::Arc::clone(lock);
            }
        }

        let mut locks = self.refresh_locks.write().await;
        std::sync::Arc::clone(
            locks
                .entry(account_id.to_string())
                .or_insert_with(|| std::sync::Arc::new(Mutex::new(()))),
        )
    }

    fn write_store_atomic(&self, content: &str) -> Result<(), CodexOAuthError> {
        if let Some(parent) = self.storage_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let parent = self
            .storage_path
            .parent()
            .ok_or_else(|| CodexOAuthError::IoError("无效的存储路径".to_string()))?;
        let file_name = self
            .storage_path
            .file_name()
            .ok_or_else(|| CodexOAuthError::IoError("无效的存储文件名".to_string()))?
            .to_string_lossy()
            .to_string();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp_path = parent.join(format!("{file_name}.tmp.{ts}"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&tmp_path)?;
            file.write_all(content.as_bytes())?;
            file.flush()?;
            fs::rename(&tmp_path, &self.storage_path)?;
            fs::set_permissions(&self.storage_path, fs::Permissions::from_mode(0o600))?;
        }

        #[cfg(windows)]
        {
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&tmp_path)?;
            file.write_all(content.as_bytes())?;
            file.flush()?;
            if self.storage_path.exists() {
                let _ = fs::remove_file(&self.storage_path);
            }
            fs::rename(&tmp_path, &self.storage_path)?;
        }

        Ok(())
    }

    fn load_from_disk_sync(&self) -> Result<(), CodexOAuthError> {
        if !self.storage_path.exists() {
            return Ok(());
        }

        let content = std::fs::read_to_string(&self.storage_path)?;
        let store: CodexOAuthStore = serde_json::from_str(&content)
            .map_err(|e| CodexOAuthError::ParseError(e.to_string()))?;

        if let Ok(mut accounts) = self.accounts.try_write() {
            *accounts = store.accounts;
        }
        if let Ok(mut default) = self.default_account_id.try_write() {
            *default = store.default_account_id;
            if default.is_none() {
                if let Ok(accounts) = self.accounts.try_read() {
                    *default = Self::fallback_default_account_id(&accounts);
                }
            }
        }

        Ok(())
    }

    pub(crate) async fn reload_from_disk(&self) -> Result<(), CodexOAuthError> {
        let content = match fs::read_to_string(&self.storage_path) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.accounts.write().await.clear();
                *self.default_account_id.write().await = None;
                self.access_tokens.write().await.clear();
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };
        let store: CodexOAuthStore = serde_json::from_str(&content)
            .map_err(|_| CodexOAuthError::ParseError("Invalid managed account store".into()))?;
        let mut accounts = self.accounts.write().await;
        self.access_tokens.write().await.retain(|id, _| {
            accounts
                .get(id)
                .zip(store.accounts.get(id))
                .is_some_and(|(old, new)| old.refresh_token == new.refresh_token)
        });
        *accounts = store.accounts;
        *self.default_account_id.write().await = store.default_account_id;
        Ok(())
    }

    async fn save_to_disk(&self) -> Result<(), CodexOAuthError> {
        let accounts = self.accounts.read().await.clone();
        let default = self.resolve_default_account_id().await;
        let store = CodexOAuthStore {
            version: 1,
            accounts,
            default_account_id: default,
        };

        let content = serde_json::to_string_pretty(&store)
            .map_err(|e| CodexOAuthError::ParseError(e.to_string()))?;
        self.write_store_atomic(&content)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn seed_account_for_tests(
        &self,
        account_id: &str,
        refresh_token: &str,
        email: Option<&str>,
        access_token: Option<&str>,
        expires_at_ms: Option<i64>,
    ) -> Result<(), CodexOAuthError> {
        self.add_account_internal(
            account_id.to_string(),
            refresh_token.to_string(),
            email.map(str::to_string),
        )
        .await?;

        if let Some(access_token) = access_token {
            self.access_tokens.write().await.insert(
                account_id.to_string(),
                CachedAccessToken {
                    token: access_token.to_string(),
                    expires_at_ms: expires_at_ms
                        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis() + 3_600_000),
                },
            );
        }

        Ok(())
    }
}

fn stored_access_expiration(auth: &serde_json::Value) -> Option<i64> {
    let token = auth.pointer("/tokens/access_token")?.as_str()?;
    let part = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(part).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    claims.get("exp")?.as_i64()?.checked_mul(1000)
}

fn parse_interval(value: Option<&serde_json::Value>) -> u64 {
    let raw = match value {
        Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(5),
        Some(serde_json::Value::String(s)) => s.parse::<u64>().unwrap_or(5),
        _ => 5,
    };
    raw.max(1) + POLLING_SAFETY_MARGIN_SECS
}

fn compute_expires_at_ms(expires_in: Option<i64>) -> i64 {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let secs = expires_in.unwrap_or(3600);
    now_ms + secs * 1000
}

fn parse_jwt_claims(token: &str) -> Option<IdTokenClaims> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let decoded = URL_SAFE_NO_PAD.decode(parts[1]).ok()?;
    serde_json::from_slice(&decoded).ok()
}

fn extract_identity_from_tokens(tokens: &OAuthTokenResponse) -> (Option<String>, Option<String>) {
    let mut account_id: Option<String> = None;
    let mut email: Option<String> = None;

    if let Some(id_token) = tokens.id_token.as_deref() {
        if let Some(claims) = parse_jwt_claims(id_token) {
            account_id = claims
                .chatgpt_account_id
                .clone()
                .or_else(|| {
                    claims
                        .openai_auth
                        .as_ref()
                        .and_then(|a| a.chatgpt_account_id.clone())
                })
                .or_else(|| claims.organizations.first().and_then(|o| o.id.clone()));
            email = claims.email.clone();
        }
    }

    if account_id.is_none() {
        if let Some(claims) = parse_jwt_claims(&tokens.access_token) {
            account_id = claims
                .chatgpt_account_id
                .clone()
                .or_else(|| {
                    claims
                        .openai_auth
                        .as_ref()
                        .and_then(|a| a.chatgpt_account_id.clone())
                })
                .or_else(|| claims.organizations.first().and_then(|o| o.id.clone()));
            if email.is_none() {
                email = claims.email.clone();
            }
        }
    }

    (account_id, email)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_interval_number() {
        let v = serde_json::Value::Number(serde_json::Number::from(5));
        assert_eq!(parse_interval(Some(&v)), 5 + POLLING_SAFETY_MARGIN_SECS);
    }

    #[test]
    fn test_parse_interval_string() {
        let v = serde_json::Value::String("10".to_string());
        assert_eq!(parse_interval(Some(&v)), 10 + POLLING_SAFETY_MARGIN_SECS);
    }

    #[test]
    fn test_parse_jwt_claims_invalid() {
        assert!(parse_jwt_claims("not-a-jwt").is_none());
    }

    #[tokio::test]
    async fn test_manager_initial_state() {
        let temp = tempfile::tempdir().unwrap();
        let manager = CodexOAuthManager::new(temp.path().to_path_buf());
        assert!(!manager.is_authenticated().await);
        assert!(manager.list_accounts().await.is_empty());
    }

    #[tokio::test]
    async fn test_manager_save_and_load() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        {
            let manager = CodexOAuthManager::new(path.clone());
            manager
                .add_account_internal(
                    "acc-123".to_string(),
                    "rt-secret".to_string(),
                    Some("user@example.com".to_string()),
                )
                .await
                .unwrap();
        }
        let manager2 = CodexOAuthManager::new(path);
        let accounts = manager2.list_accounts().await;
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, "acc-123");
    }

    #[tokio::test]
    async fn test_remove_account_rehomes_default_account() {
        let temp = tempfile::tempdir().unwrap();
        let manager = CodexOAuthManager::new(temp.path().to_path_buf());

        manager
            .seed_account_for_tests("acc-123", "rt-1", Some("a@example.com"), Some("at-1"), None)
            .await
            .unwrap();
        manager
            .seed_account_for_tests("acc-456", "rt-2", Some("b@example.com"), Some("at-2"), None)
            .await
            .unwrap();
        manager.set_default_account("acc-123").await.unwrap();

        manager.remove_account("acc-123").await.unwrap();

        let accounts = manager.list_accounts().await;
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, "acc-456");
        assert_eq!(
            manager.default_account_id().await.as_deref(),
            Some("acc-456")
        );
    }

    #[tokio::test]
    async fn test_set_default_account_reorders_status() {
        let temp = tempfile::tempdir().unwrap();
        let manager = CodexOAuthManager::new(temp.path().to_path_buf());

        manager
            .seed_account_for_tests("acc-123", "rt-1", Some("a@example.com"), Some("at-1"), None)
            .await
            .unwrap();
        manager
            .seed_account_for_tests("acc-456", "rt-2", Some("b@example.com"), Some("at-2"), None)
            .await
            .unwrap();

        manager.set_default_account("acc-456").await.unwrap();

        let status = manager.get_status().await;
        assert_eq!(status.default_account_id.as_deref(), Some("acc-456"));
        assert_eq!(
            status.accounts.first().map(|account| account.id.as_str()),
            Some("acc-456")
        );
    }
    #[tokio::test]
    async fn native_codex_credentials_keep_id_token_across_refresh_and_reload() {
        let temp = tempfile::tempdir().unwrap();
        let manager = CodexOAuthManager::new(temp.path().to_path_buf());
        manager
            .seed_account_for_tests("a", "refresh-a", None, None, None)
            .await
            .unwrap();
        manager
            .remember_token_response(
                "a",
                &OAuthTokenResponse {
                    access_token: "access-a".into(),
                    refresh_token: Some("rotated-1".into()),
                    id_token: Some("id-a".into()),
                    expires_in: Some(3600),
                },
                None,
            )
            .await
            .unwrap();
        manager
            .remember_token_response(
                "a",
                &OAuthTokenResponse {
                    access_token: "access-b".into(),
                    refresh_token: Some("rotated-2".into()),
                    id_token: None,
                    expires_in: Some(3600),
                },
                None,
            )
            .await
            .unwrap();
        let reloaded = CodexOAuthManager::new(temp.path().to_path_buf());
        let accounts = reloaded.accounts.read().await;
        let account = &accounts["a"];
        assert_eq!(account.refresh_token, "rotated-2");
        let auth = account.codex_auth.as_ref().unwrap();
        assert_eq!(auth["tokens"]["refresh_token"], "rotated-2");
        assert_eq!(auth["tokens"]["access_token"], "access-b");
        assert_eq!(auth["tokens"]["id_token"], "id-a");
    }

    #[tokio::test]
    async fn native_codex_capture_does_not_restore_an_older_refresh_token() {
        let temp = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());
        let manager = CodexOAuthManager::new(temp.path().to_path_buf());
        manager
            .seed_account_for_tests("a", "initial", None, None, None)
            .await
            .unwrap();
        let newer = serde_json::json!({"tokens":{"account_id":"a","refresh_token":"new"},"last_refresh":"2026-02-01T00:00:00Z"});
        manager.capture_codex_auth(&newer).await.unwrap();
        let older = serde_json::json!({"tokens":{"account_id":"a","refresh_token":"old"},"last_refresh":"2026-01-01T00:00:00Z"});
        manager.capture_codex_auth(&older).await.unwrap();
        assert_eq!(manager.accounts.read().await["a"].refresh_token, "new");
        assert_eq!(
            manager.accounts.read().await["a"].codex_auth.as_ref(),
            Some(&newer)
        );
    }
    async fn exercise_native_refresh(obstruct_live: bool) {
        use serde_json::json;
        let temp = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());
        let db = crate::Database::init().unwrap();
        let jwt = |exp| {
            format!(
                "e30.{}.sig",
                URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!({"exp":exp})).unwrap())
            )
        };
        let expired = jwt(1);
        let fresh = jwt(chrono::Utc::now().timestamp() + 3600);
        let auth = json!({"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{
            "account_id":"a","access_token":expired,"refresh_token":"old","id_token":"id-a"},"last_refresh":"2026-01-01T00:00:00Z"});
        crate::config::write_json_file(&crate::codex_config::get_codex_auth_path(), &auth).unwrap();
        let mut provider = crate::Provider::with_id(
            "official".into(),
            "Official".into(),
            json!({"auth":auth,"config":""}),
            None,
        );
        provider.category = Some("official".into());
        db.save_provider("codex", &provider).unwrap();
        let response = json!({"access_token":fresh,"refresh_token":"rotated","expires_in":3600});
        let live_path = crate::codex_config::get_codex_auth_path();
        let obstruction_path = live_path.clone();
        let app = axum::Router::new().route(
            "/token",
            axum::routing::post(move || async move {
                if obstruct_live {
                    std::fs::rename(&obstruction_path, obstruction_path.with_extension("old"))
                        .unwrap();
                    std::fs::create_dir(&obstruction_path).unwrap();
                }
                axum::Json(response)
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut manager = CodexOAuthManager::new(crate::config::get_app_config_dir());
        manager.token_endpoint = Some(format!("http://{address}/token"));
        manager
            .seed_account_for_tests("a", "old", None, None, None)
            .await
            .unwrap();
        let result = manager.get_valid_token_for_account("a").await;
        if obstruct_live {
            assert!(result.is_err());
            assert!(manager.accounts.read().await["a"]
                .pending_native_sync
                .is_some());
            std::fs::remove_dir(&live_path).unwrap();
            std::fs::rename(live_path.with_extension("old"), &live_path).unwrap();
        } else {
            assert_eq!(result.unwrap(), fresh);
        }
        server.abort();
        // Retry in a different manager after the server is gone. It must finish the
        // persisted publication, not rotate the token again.
        let mut recovered = CodexOAuthManager::new(crate::config::get_app_config_dir());
        recovered.token_endpoint = Some(format!("http://{address}/closed"));
        assert_eq!(
            recovered.get_valid_token_for_account("a").await.unwrap(),
            fresh
        );
        assert!(recovered.accounts.read().await["a"]
            .pending_native_sync
            .is_none());
        let live: serde_json::Value =
            crate::config::read_json_file(&crate::codex_config::get_codex_auth_path()).unwrap();
        assert_eq!(live["tokens"]["refresh_token"], "rotated");
        assert_eq!(live["tokens"]["id_token"], "id-a");
        assert_eq!(
            db.get_all_providers("codex").unwrap()["official"].settings_config["auth"],
            live
        );
        // A fresh manager must use persisted access credentials, not refresh again.
        let mut restarted = CodexOAuthManager::new(crate::config::get_app_config_dir());
        restarted.token_endpoint = Some(format!("http://{address}/closed"));
        assert_eq!(
            restarted.get_valid_token_for_account("a").await.unwrap(),
            fresh
        );
    }
    #[tokio::test]
    async fn native_codex_http_refresh_updates_live_and_launch_copy_and_survives_restart() {
        exercise_native_refresh(false).await;
    }

    #[tokio::test]
    async fn native_codex_refresh_recovers_failed_publication_in_a_new_process_manager() {
        exercise_native_refresh(true).await;
    }
}
