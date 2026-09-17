use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use tokio::sync::{Mutex, RwLock};

pub const KIMI_CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
pub const KIMI_DEVICE_AUTH_URL: &str = "https://auth.kimi.com/api/oauth/device_authorization";
pub const KIMI_TOKEN_URL: &str = "https://auth.kimi.com/api/oauth/token";
pub const KIMI_USER_INFO_URL: &str = "https://api.kimi.com/coding/v1/me";
pub const KIMI_DEFAULT_VERIFICATION_URL: &str = "https://auth.kimi.com/device";
pub const KIMI_USER_AGENT: &str = "cc-switch-kimi-oauth";
const TOKEN_REFRESH_BUFFER_MS: i64 = 60_000;
const DEVICE_CODE_DEFAULT_EXPIRES_IN: u64 = 300;
const POLLING_SAFETY_MARGIN_SECS: u64 = 3;

#[derive(Debug, thiserror::Error)]
pub enum KimiOAuthError {
    #[error("等待用户授权中 (authorization pending)")]
    AuthorizationPending,
    #[error("用户拒绝授权 (access denied)")]
    AccessDenied,
    #[error("Device Code 已过期 (expired token)")]
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

impl From<reqwest::Error> for KimiOAuthError {
    fn from(err: reqwest::Error) -> Self {
        KimiOAuthError::NetworkError(err.to_string())
    }
}

impl From<std::io::Error> for KimiOAuthError {
    fn from(err: std::io::Error) -> Self {
        KimiOAuthError::IoError(err.to_string())
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
pub struct KimiOAuthStatus {
    pub accounts: Vec<ManagedAuthAccount>,
    pub default_account_id: Option<String>,
    pub authenticated: bool,
    pub username: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    #[serde(default)]
    verification_uri: Option<String>,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    #[serde(default)]
    interval: Option<serde_json::Value>,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct KimiTokenResponse {
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<i64>,
    #[serde(default)]
    pub token_type: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_description: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct KimiUserInfoResponse {
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    nickname: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    avatar: Option<String>,
}

#[derive(Debug, Clone)]
struct CachedAccessToken {
    token: String,
    expires_at_ms: i64,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct PendingDeviceCode {
    user_code: String,
    expires_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KimiAccountData {
    pub account_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    pub refresh_token: String,
    pub authenticated_at: i64,
}

impl From<&KimiAccountData> for ManagedAuthAccount {
    fn from(data: &KimiAccountData) -> Self {
        let login = if let Some(nickname) = data.nickname.as_deref().filter(|s| !s.trim().is_empty()) {
            if let Some(email) = data.email.as_deref().filter(|s| !s.trim().is_empty()) {
                format!("{nickname} ({email})")
            } else {
                nickname.to_string()
            }
        } else if let Some(email) = data.email.as_deref().filter(|s| !s.trim().is_empty()) {
            email.to_string()
        } else {
            format!("Kimi ({})", &data.account_id)
        };

        Self {
            id: data.account_id.clone(),
            login,
            avatar_url: data.avatar_url.clone(),
            authenticated_at: data.authenticated_at,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct KimiOAuthStore {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    accounts: HashMap<String, KimiAccountData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_account_id: Option<String>,
}

pub struct KimiOAuthManager {
    accounts: std::sync::Arc<RwLock<HashMap<String, KimiAccountData>>>,
    default_account_id: std::sync::Arc<RwLock<Option<String>>>,
    access_tokens: std::sync::Arc<RwLock<HashMap<String, CachedAccessToken>>>,
    refresh_locks: std::sync::Arc<RwLock<HashMap<String, std::sync::Arc<Mutex<()>>>>>,
    pending_device_codes: std::sync::Arc<RwLock<HashMap<String, PendingDeviceCode>>>,
    storage_path: PathBuf,
}

impl KimiOAuthManager {
    pub fn new(data_dir: PathBuf) -> Self {
        let storage_path = data_dir.join("kimi_oauth_auth.json");
        let manager = Self {
            accounts: std::sync::Arc::new(RwLock::new(HashMap::new())),
            default_account_id: std::sync::Arc::new(RwLock::new(None)),
            access_tokens: std::sync::Arc::new(RwLock::new(HashMap::new())),
            refresh_locks: std::sync::Arc::new(RwLock::new(HashMap::new())),
            pending_device_codes: std::sync::Arc::new(RwLock::new(HashMap::new())),
            storage_path,
        };

        if let Err(e) = manager.load_from_disk_sync() {
            log::warn!("[KimiOAuth] 加载存储失败: {e}");
        }

        manager
    }

    pub async fn start_device_flow(
        &self,
    ) -> Result<ManagedAuthDeviceCodeResponse, KimiOAuthError> {
        let params = [("client_id", KIMI_CLIENT_ID)];
        let response = crate::proxy::http_client::get()
            .post(KIMI_DEVICE_AUTH_URL)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("User-Agent", KIMI_USER_AGENT)
            .form(&params)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(KimiOAuthError::NetworkError(format!(
                "Kimi Device Code 请求失败: {status} - {text}"
            )));
        }

        let device: DeviceCodeResponse = response
            .json()
            .await
            .map_err(|e| KimiOAuthError::ParseError(e.to_string()))?;

        let interval = parse_interval(device.interval.as_ref());
        let expires_in = device.expires_in.unwrap_or(DEVICE_CODE_DEFAULT_EXPIRES_IN);
        let expires_at_ms = chrono::Utc::now().timestamp_millis() + (expires_in as i64) * 1000;

        {
            let mut pending = self.pending_device_codes.write().await;
            let now_ms = chrono::Utc::now().timestamp_millis();
            pending.retain(|_, entry| entry.expires_at_ms > now_ms);
            pending.insert(
                device.device_code.clone(),
                PendingDeviceCode {
                    user_code: device.user_code.clone(),
                    expires_at_ms,
                },
            );
        }

        let verification_uri = device
            .verification_uri_complete
            .or(device.verification_uri)
            .unwrap_or_else(|| KIMI_DEFAULT_VERIFICATION_URL.to_string());

        Ok(ManagedAuthDeviceCodeResponse {
            device_code: device.device_code,
            user_code: device.user_code,
            verification_uri,
            expires_in,
            interval,
        })
    }

    pub async fn poll_for_token(
        &self,
        device_code: &str,
    ) -> Result<Option<ManagedAuthAccount>, KimiOAuthError> {
        let entry = {
            let pending = self.pending_device_codes.read().await;
            pending.get(device_code).cloned()
        }
        .ok_or_else(|| {
            KimiOAuthError::TokenFetchFailed(
                "未找到对应的 device_code，请重新启动登录流程".to_string(),
            )
        })?;

        if entry.expires_at_ms <= chrono::Utc::now().timestamp_millis() {
            let mut pending = self.pending_device_codes.write().await;
            pending.remove(device_code);
            return Err(KimiOAuthError::ExpiredToken);
        }

        let params = [
            ("client_id", KIMI_CLIENT_ID),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ];

        let poll_response = crate::proxy::http_client::get()
            .post(KIMI_TOKEN_URL)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("User-Agent", KIMI_USER_AGENT)
            .form(&params)
            .send()
            .await?;

        let status = poll_response.status();
        let body_text = poll_response.text().await.unwrap_or_default();

        if !status.is_success() && status.as_u16() >= 500 {
            return Err(KimiOAuthError::NetworkError(format!(
                "Kimi 服务器错误: {status} - {body_text}"
            )));
        }

        let token_resp: KimiTokenResponse = match serde_json::from_str(&body_text) {
            Ok(parsed) => parsed,
            Err(_) => {
                return Err(KimiOAuthError::TokenFetchFailed(format!(
                    "无法解析响应: {status} - {body_text}"
                )));
            }
        };

        if let Some(err_code) = token_resp.error.as_deref() {
            match err_code {
                "authorization_pending" | "slow_down" => {
                    return Err(KimiOAuthError::AuthorizationPending);
                }
                "expired_token" => {
                    let mut pending = self.pending_device_codes.write().await;
                    pending.remove(device_code);
                    return Err(KimiOAuthError::ExpiredToken);
                }
                "access_denied" => {
                    let mut pending = self.pending_device_codes.write().await;
                    pending.remove(device_code);
                    return Err(KimiOAuthError::AccessDenied);
                }
                other => {
                    let desc = token_resp
                        .error_description
                        .unwrap_or_else(|| other.to_string());
                    return Err(KimiOAuthError::TokenFetchFailed(format!(
                        "Token 请求被拒绝: {desc}"
                    )));
                }
            }
        }

        let access_token = token_resp
            .access_token
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| {
                KimiOAuthError::TokenFetchFailed("响应缺少 access_token".to_string())
            })?;

        let refresh_token = token_resp.refresh_token.ok_or_else(|| {
            KimiOAuthError::TokenFetchFailed("响应缺少 refresh_token".to_string())
        })?;

        // 成功获得 token，移除 pending device code
        {
            let mut pending = self.pending_device_codes.write().await;
            pending.remove(device_code);
        }

        // 获取用户资料
        let user_info = Self::fetch_user_info(&access_token).await.ok();
        let account_id = user_info
            .as_ref()
            .and_then(|u| u.user_id.clone())
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| {
                use sha2::{Digest, Sha256};
                let mut hasher = Sha256::new();
                hasher.update(refresh_token.as_bytes());
                let result = hasher.finalize();
                format!("kimi_{:x}", &result)[..16].to_string()
            });

        let account = self
            .add_account_internal(
                account_id,
                refresh_token.clone(),
                user_info.as_ref().and_then(|u| u.nickname.clone()),
                user_info.as_ref().and_then(|u| u.email.clone()),
                user_info.as_ref().and_then(|u| u.avatar.clone()),
            )
            .await?;

        // 缓存 access token
        let expires_in_sec = token_resp.expires_in.unwrap_or(3600);
        let expires_at_ms = chrono::Utc::now().timestamp_millis() + expires_in_sec * 1000;
        {
            let mut tokens = self.access_tokens.write().await;
            tokens.insert(
                account.id.clone(),
                CachedAccessToken {
                    token: access_token.clone(),
                    expires_at_ms,
                },
            );
        }

        // 如果是当前默认账号，同步写入 native ~/.kimi-code
        #[cfg(not(test))]
        if self.default_account_id().await.as_deref() == Some(&account.id) {
            let _ = crate::kimi_config::sync_kimi_account_to_native(
                &access_token,
                &refresh_token,
                expires_in_sec,
                expires_at_ms / 1000,
            );
        }

        Ok(Some(account))
    }

    async fn fetch_user_info(access_token: &str) -> Result<KimiUserInfoResponse, KimiOAuthError> {
        let response = crate::proxy::http_client::get()
            .get(KIMI_USER_INFO_URL)
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Accept", "application/json")
            .header("User-Agent", KIMI_USER_AGENT)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(KimiOAuthError::NetworkError(format!(
                "获取 Kimi 用户信息失败: HTTP {}",
                response.status()
            )));
        }

        let user_info: KimiUserInfoResponse = response
            .json()
            .await
            .map_err(|e| KimiOAuthError::ParseError(e.to_string()))?;

        Ok(user_info)
    }

    async fn refresh_access_token(&self, refresh_token: &str) -> Result<KimiTokenResponse, KimiOAuthError> {
        Self::refresh_token_raw(refresh_token).await
    }

    pub async fn refresh_token_raw(refresh_token: &str) -> Result<KimiTokenResponse, KimiOAuthError> {
        let params = [
            ("client_id", KIMI_CLIENT_ID),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ];

        let response = crate::proxy::http_client::get()
            .post(KIMI_TOKEN_URL)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("User-Agent", KIMI_USER_AGENT)
            .form(&params)
            .send()
            .await?;

        let status = response.status();
        let body_text = response.text().await.unwrap_or_default();

        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(KimiOAuthError::RefreshTokenInvalid);
        }

        if !status.is_success() {
            return Err(KimiOAuthError::TokenFetchFailed(format!(
                "Token 刷新失败: {status} - {body_text}"
            )));
        }

        let token_resp: KimiTokenResponse = serde_json::from_str(&body_text)
            .map_err(|e| KimiOAuthError::ParseError(format!("{e}: {body_text}")))?;

        if token_resp
            .access_token
            .as_ref()
            .map(|s| s.trim().is_empty())
            .unwrap_or(true)
        {
            return Err(KimiOAuthError::TokenFetchFailed(
                "刷新响应中缺少 access_token".to_string(),
            ));
        }

        Ok(token_resp)
    }

    pub async fn get_valid_token_for_account(
        &self,
        account_id: &str,
    ) -> Result<String, KimiOAuthError> {
        let now_ms = chrono::Utc::now().timestamp_millis();

        {
            let tokens = self.access_tokens.read().await;
            if let Some(cached) = tokens.get(account_id) {
                if cached.expires_at_ms > now_ms + TOKEN_REFRESH_BUFFER_MS {
                    return Ok(cached.token.clone());
                }
            }
        }

        let refresh_lock = {
            let mut locks = self.refresh_locks.write().await;
            locks
                .entry(account_id.to_string())
                .or_insert_with(|| std::sync::Arc::new(Mutex::new(())))
                .clone()
        };

        let _guard = refresh_lock.lock().await;

        {
            let tokens = self.access_tokens.read().await;
            if let Some(cached) = tokens.get(account_id) {
                if cached.expires_at_ms > now_ms + TOKEN_REFRESH_BUFFER_MS {
                    return Ok(cached.token.clone());
                }
            }
        }

        let refresh_token = {
            let accounts = self.accounts.read().await;
            accounts
                .get(account_id)
                .ok_or_else(|| KimiOAuthError::AccountNotFound(account_id.to_string()))?
                .refresh_token
                .clone()
        };

        let token_resp = self.refresh_access_token(&refresh_token).await?;
        let access_token = token_resp
            .access_token
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| {
                KimiOAuthError::TokenFetchFailed("刷新响应中缺少 access_token".to_string())
            })?;
        let expires_in_sec = token_resp.expires_in.unwrap_or(3600);
        let expires_at_ms = chrono::Utc::now().timestamp_millis() + expires_in_sec * 1000;

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

        if let Some(new_rt) = token_resp.refresh_token.filter(|rt| rt != &refresh_token) {
            let mut accounts = self.accounts.write().await;
            if let Some(acc) = accounts.get_mut(account_id) {
                acc.refresh_token = new_rt;
            }
            drop(accounts);
            self.save_to_disk().await?;
        }

        // 如果是当前默认账号，同步写入 native ~/.kimi-code
        #[cfg(not(test))]
        if self.default_account_id().await.as_deref() == Some(account_id) {
            let rt = {
                let accounts = self.accounts.read().await;
                accounts.get(account_id).map(|a| a.refresh_token.clone())
            };
            if let Some(rt) = rt {
                let _ = crate::kimi_config::sync_kimi_account_to_native(
                    &access_token,
                    &rt,
                    expires_in_sec,
                    expires_at_ms / 1000,
                );
            }
        }

        Ok(access_token)
    }

    #[allow(dead_code)]
    pub async fn get_valid_token(&self) -> Result<String, KimiOAuthError> {
        let default_id = self.default_account_id().await;
        match default_id {
            Some(id) => self.get_valid_token_for_account(&id).await,
            None => Err(KimiOAuthError::AccountNotFound(
                "未设置默认 Kimi 账号".to_string(),
            )),
        }
    }

    pub async fn remove_account(&self, account_id: &str) -> Result<(), KimiOAuthError> {
        {
            let mut accounts = self.accounts.write().await;
            if accounts.remove(account_id).is_none() {
                return Err(KimiOAuthError::AccountNotFound(account_id.to_string()));
            }

            let mut default_id = self.default_account_id.write().await;
            if default_id.as_deref() == Some(account_id) {
                *default_id = accounts.keys().next().cloned();
            }
        }

        self.access_tokens.write().await.remove(account_id);
        self.refresh_locks.write().await.remove(account_id);
        self.save_to_disk().await?;
        Ok(())
    }

    pub async fn set_default_account(&self, account_id: &str) -> Result<(), KimiOAuthError> {
        {
            let accounts = self.accounts.read().await;
            if !accounts.contains_key(account_id) {
                return Err(KimiOAuthError::AccountNotFound(account_id.to_string()));
            }
        }

        *self.default_account_id.write().await = Some(account_id.to_string());
        self.save_to_disk().await?;

        // 切换默认账号时，自动同步激活至 native ~/.kimi-code
        #[cfg(not(test))]
        if let Ok(token) = self.get_valid_token_for_account(account_id).await {
            let accounts = self.accounts.read().await;
            if let Some(acc) = accounts.get(account_id) {
                let tokens = self.access_tokens.read().await;
                let expires_at_sec = tokens.get(account_id).map(|t| t.expires_at_ms / 1000).unwrap_or(0);
                let _ = crate::kimi_config::sync_kimi_account_to_native(
                    &token,
                    &acc.refresh_token,
                    3600,
                    expires_at_sec,
                );
            }
        }

        Ok(())
    }

    pub async fn clear_auth(&self) -> Result<(), KimiOAuthError> {
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

    pub async fn default_account_id(&self) -> Option<String> {
        self.default_account_id.read().await.clone()
    }

    pub async fn list_accounts(&self) -> Vec<ManagedAuthAccount> {
        let accounts = self.accounts.read().await;
        let mut list: Vec<ManagedAuthAccount> = accounts.values().map(ManagedAuthAccount::from).collect();
        list.sort_by(|a, b| b.authenticated_at.cmp(&a.authenticated_at));
        list
    }

    pub fn find_account_sync(
        &self,
        refresh_token: &str,
        account_id: Option<&str>,
    ) -> Option<ManagedAuthAccount> {
        if let Ok(accounts) = self.accounts.try_read() {
            if !refresh_token.is_empty() {
                for acc in accounts.values() {
                    if acc.refresh_token == refresh_token {
                        return Some(ManagedAuthAccount::from(acc));
                    }
                }
            }
            if let Some(id) = account_id {
                if let Some(acc) = accounts.get(id) {
                    return Some(ManagedAuthAccount::from(acc));
                }
            }
        }
        None
    }

    pub async fn get_status(&self) -> KimiOAuthStatus {
        let accounts = self.list_accounts().await;
        let default_id = self.default_account_id().await;
        let authenticated = !accounts.is_empty();
        let username = default_id
            .as_ref()
            .and_then(|id| accounts.iter().find(|a| &a.id == id))
            .map(|a| a.login.clone());

        KimiOAuthStatus {
            accounts,
            default_account_id: default_id,
            authenticated,
            username,
        }
    }

    async fn add_account_internal(
        &self,
        account_id: String,
        refresh_token: String,
        nickname: Option<String>,
        email: Option<String>,
        avatar_url: Option<String>,
    ) -> Result<ManagedAuthAccount, KimiOAuthError> {
        let now = chrono::Utc::now().timestamp();
        let account_data = KimiAccountData {
            account_id: account_id.clone(),
            nickname,
            email,
            avatar_url,
            refresh_token,
            authenticated_at: now,
        };

        let result = ManagedAuthAccount::from(&account_data);

        {
            let mut accounts = self.accounts.write().await;
            accounts.insert(account_id.clone(), account_data);
        }

        {
            let mut default_id = self.default_account_id.write().await;
            if default_id.is_none() {
                *default_id = Some(account_id);
            }
        }

        self.save_to_disk().await?;
        Ok(result)
    }

    #[cfg(test)]
    pub(crate) async fn seed_account_for_tests(
        &self,
        account_id: &str,
        refresh_token: &str,
        nickname: Option<&str>,
        email: Option<&str>,
        access_token: Option<&str>,
        expires_at_ms: Option<i64>,
    ) -> Result<(), KimiOAuthError> {
        self.add_account_internal(
            account_id.to_string(),
            refresh_token.to_string(),
            nickname.map(str::to_string),
            email.map(str::to_string),
            None,
        )
        .await?;

        if let Some(token) = access_token {
            let mut tokens = self.access_tokens.write().await;
            tokens.insert(
                account_id.to_string(),
                CachedAccessToken {
                    token: token.to_string(),
                    expires_at_ms: expires_at_ms.unwrap_or_else(|| {
                        chrono::Utc::now().timestamp_millis() + 3600 * 1000
                    }),
                },
            );
        }

        Ok(())
    }

    fn write_store_atomic(&self, content: &str) -> Result<(), KimiOAuthError> {
        let parent = self
            .storage_path
            .parent()
            .ok_or_else(|| KimiOAuthError::IoError("无效的存储路径".to_string()))?;

        std::fs::create_dir_all(parent)?;

        let filename = self
            .storage_path
            .file_name()
            .ok_or_else(|| KimiOAuthError::IoError("无效的存储文件名".to_string()))?
            .to_string_lossy();

        let temp_path = parent.join(format!(
            ".{filename}.tmp.{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));

        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temp_path)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }

        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        drop(file);

        if let Err(rename_err) = fs::rename(&temp_path, &self.storage_path) {
            let _ = fs::remove_file(&temp_path);
            return Err(KimiOAuthError::IoError(format!(
                "原子重命名持久化存储文件失败: {rename_err}"
            )));
        }

        Ok(())
    }

    fn load_from_disk_sync(&self) -> Result<(), KimiOAuthError> {
        if !self.storage_path.exists() {
            return Ok(());
        }

        let content = fs::read_to_string(&self.storage_path)?;
        let store: KimiOAuthStore = serde_json::from_str(&content)
            .map_err(|e| KimiOAuthError::ParseError(e.to_string()))?;

        {
            let mut accounts = self
                .accounts
                .try_write()
                .map_err(|_| KimiOAuthError::IoError("获取 accounts 写锁失败".to_string()))?;
            *accounts = store.accounts;
        }

        {
            let mut default_id = self
                .default_account_id
                .try_write()
                .map_err(|_| KimiOAuthError::IoError("获取 default_id 写锁失败".to_string()))?;
            *default_id = store.default_account_id;
        }

        Ok(())
    }

    async fn save_to_disk(&self) -> Result<(), KimiOAuthError> {
        let accounts = self.accounts.read().await.clone();
        let default_id = self.default_account_id.read().await.clone();

        let store = KimiOAuthStore {
            version: 1,
            accounts,
            default_account_id: default_id,
        };

        let json = serde_json::to_string_pretty(&store)
            .map_err(|e| KimiOAuthError::ParseError(e.to_string()))?;

        self.write_store_atomic(&json)
    }
}

fn parse_interval(interval: Option<&serde_json::Value>) -> u64 {
    let base = match interval {
        Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(5),
        Some(serde_json::Value::String(s)) => s.parse::<u64>().unwrap_or(5),
        _ => 5,
    };
    base + POLLING_SAFETY_MARGIN_SECS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_manager_initial_state() {
        let temp = tempfile::tempdir().unwrap();
        let manager = KimiOAuthManager::new(temp.path().to_path_buf());
        assert!(!manager.is_authenticated().await);
        assert!(manager.list_accounts().await.is_empty());
    }

    #[tokio::test]
    async fn test_manager_save_and_load() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        {
            let manager = KimiOAuthManager::new(path.clone());
            manager
                .add_account_internal(
                    "u_123".to_string(),
                    "rt-kimi-secret".to_string(),
                    Some("KimiUser".to_string()),
                    Some("user@kimi.ai".to_string()),
                    None,
                )
                .await
                .unwrap();
        }
        let manager2 = KimiOAuthManager::new(path);
        let accounts = manager2.list_accounts().await;
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, "u_123");
        assert_eq!(accounts[0].login, "KimiUser (user@kimi.ai)");
    }

    #[tokio::test]
    async fn test_remove_account_rehomes_default_account() {
        let temp = tempfile::tempdir().unwrap();
        let manager = KimiOAuthManager::new(temp.path().to_path_buf());

        manager
            .seed_account_for_tests("u_1", "rt-1", Some("User1"), None, Some("at-1"), None)
            .await
            .unwrap();
        manager
            .seed_account_for_tests("u_2", "rt-2", Some("User2"), None, Some("at-2"), None)
            .await
            .unwrap();
        manager.set_default_account("u_1").await.unwrap();

        manager.remove_account("u_1").await.unwrap();

        let accounts = manager.list_accounts().await;
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, "u_2");
        assert_eq!(manager.default_account_id().await.as_deref(), Some("u_2"));
    }

    #[tokio::test]
    async fn test_set_default_account() {
        let temp = tempfile::tempdir().unwrap();
        let manager = KimiOAuthManager::new(temp.path().to_path_buf());

        manager
            .seed_account_for_tests("u_1", "rt-1", Some("User1"), None, Some("at-1"), None)
            .await
            .unwrap();
        manager
            .seed_account_for_tests("u_2", "rt-2", Some("User2"), None, Some("at-2"), None)
            .await
            .unwrap();

        manager.set_default_account("u_2").await.unwrap();
        assert_eq!(manager.default_account_id().await.as_deref(), Some("u_2"));

        let status = manager.get_status().await;
        assert_eq!(status.default_account_id.as_deref(), Some("u_2"));
    }
}
