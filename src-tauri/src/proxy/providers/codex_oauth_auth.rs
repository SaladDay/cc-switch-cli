use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use reqwest::Client;
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
    http_client: Client,
    storage_path: PathBuf,
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
            http_client: Client::new(),
            storage_path,
        };

        if let Err(e) = manager.load_from_disk_sync() {
            log::warn!("[CodexOAuth] 加载存储失败: {e}");
        }

        manager
    }

    pub async fn start_device_flow(
        &self,
    ) -> Result<ManagedAuthDeviceCodeResponse, CodexOAuthError> {
        let response = self
            .http_client
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

        let poll_response = self
            .http_client
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

        Ok(Some(account))
    }

    async fn exchange_code_for_tokens(
        &self,
        code: &str,
        code_verifier: &str,
    ) -> Result<OAuthTokenResponse, CodexOAuthError> {
        let response = self
            .http_client
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

    async fn refresh_with_token(
        &self,
        refresh_token: &str,
    ) -> Result<OAuthTokenResponse, CodexOAuthError> {
        let response = self
            .http_client
            .post(OAUTH_TOKEN_URL)
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

    pub async fn get_valid_token_for_account(
        &self,
        account_id: &str,
    ) -> Result<String, CodexOAuthError> {
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

        let new_tokens = self.refresh_with_token(&refresh_token).await?;

        if let Some(new_refresh) = new_tokens.refresh_token.clone() {
            if new_refresh != refresh_token {
                let mut accounts = self.accounts.write().await;
                if let Some(account) = accounts.get_mut(account_id) {
                    account.refresh_token = new_refresh;
                }
                drop(accounts);
                self.save_to_disk().await?;
            }
        }

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
        match self.resolve_default_account_id().await {
            Some(id) => self.get_valid_token_for_account(&id).await,
            None => Err(CodexOAuthError::AccountNotFound(
                "无可用的 ChatGPT 账号".to_string(),
            )),
        }
    }

    pub async fn default_account_id(&self) -> Option<String> {
        self.resolve_default_account_id().await
    }

    pub async fn list_accounts(&self) -> Vec<ManagedAuthAccount> {
        let accounts = self.accounts.read().await.clone();
        let default_id = self.resolve_default_account_id().await;
        Self::sorted_accounts(&accounts, default_id.as_deref())
    }

    pub async fn remove_account(&self, account_id: &str) -> Result<(), CodexOAuthError> {
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
        {
            let accounts = self.accounts.read().await;
            if !accounts.contains_key(account_id) {
                return Err(CodexOAuthError::AccountNotFound(account_id.to_string()));
            }
        }

        *self.default_account_id.write().await = Some(account_id.to_string());
        self.save_to_disk().await?;
        Ok(())
    }

    pub async fn clear_auth(&self) -> Result<(), CodexOAuthError> {
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

    pub async fn is_authenticated(&self) -> bool {
        !self.accounts.read().await.is_empty()
    }

    pub async fn get_status(&self) -> CodexOAuthStatus {
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
}
