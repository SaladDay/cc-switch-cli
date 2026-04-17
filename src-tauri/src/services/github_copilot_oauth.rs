use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use reqwest::Client;
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock as AsyncRwLock};

use crate::config::get_app_config_dir;

const COPILOT_SESSION_TOKEN_URL: &str = "https://api.github.com/copilot_internal/v2/token";
const COPILOT_EDITOR_VERSION: &str = "vscode/1.85.0";
const COPILOT_EDITOR_PLUGIN_VERSION: &str = "copilot/1.150.0";
const COPILOT_USER_AGENT: &str = "cc-switch-github-copilot";
const TOKEN_REFRESH_BUFFER_MS: i64 = 60_000;

#[derive(Debug, thiserror::Error)]
pub enum GitHubCopilotOAuthError {
    #[error("未找到 GitHub Copilot 账号: {0}")]
    AccountNotFound(String),
    #[error("未找到可用的 GitHub Copilot 默认账号")]
    DefaultAccountNotFound,
    #[error("GitHub Copilot OAuth 凭证不存在")]
    CredentialsMissing,
    #[error("GitHub Copilot token 交换失败: {0}")]
    TokenExchangeFailed(String),
    #[error("网络错误: {0}")]
    NetworkError(String),
    #[error("解析错误: {0}")]
    ParseError(String),
    #[error("IO 错误: {0}")]
    IoError(String),
}

impl From<reqwest::Error> for GitHubCopilotOAuthError {
    fn from(err: reqwest::Error) -> Self {
        Self::NetworkError(err.to_string())
    }
}

impl From<std::io::Error> for GitHubCopilotOAuthError {
    fn from(err: std::io::Error) -> Self {
        Self::IoError(err.to_string())
    }
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubCopilotAuthStore {
    #[serde(default)]
    accounts: HashMap<String, GitHubCopilotAccountData>,
    #[serde(default)]
    default_account_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubCopilotAccountData {
    github_token: String,
    #[serde(default)]
    authenticated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct CopilotSessionTokenResponse {
    token: String,
    expires_at: i64,
}

#[derive(Debug, Clone)]
struct CachedSessionToken {
    token: String,
    expires_at_ms: i64,
}

impl CachedSessionToken {
    fn is_expiring_soon(&self) -> bool {
        let now = chrono::Utc::now().timestamp_millis();
        self.expires_at_ms - now < TOKEN_REFRESH_BUFFER_MS
    }
}

pub struct GitHubCopilotOAuthManager {
    accounts: Arc<AsyncRwLock<HashMap<String, GitHubCopilotAccountData>>>,
    default_account_id: Arc<AsyncRwLock<Option<String>>>,
    session_tokens: Arc<AsyncRwLock<HashMap<String, CachedSessionToken>>>,
    refresh_locks: Arc<AsyncRwLock<HashMap<String, Arc<Mutex<()>>>>>,
    http_client: Client,
    storage_path: PathBuf,
}

impl GitHubCopilotOAuthManager {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            accounts: Arc::new(AsyncRwLock::new(HashMap::new())),
            default_account_id: Arc::new(AsyncRwLock::new(None)),
            session_tokens: Arc::new(AsyncRwLock::new(HashMap::new())),
            refresh_locks: Arc::new(AsyncRwLock::new(HashMap::new())),
            http_client: Client::new(),
            storage_path: data_dir.join("copilot_auth.json"),
        }
    }

    pub async fn get_valid_token_for_account(
        &self,
        account_id: &str,
    ) -> Result<String, GitHubCopilotOAuthError> {
        self.reload_from_disk().await?;

        if let Some(token) = self.cached_token(account_id).await {
            return Ok(token);
        }

        let refresh_lock = self.refresh_lock(account_id).await;
        let _guard = refresh_lock.lock().await;

        self.reload_from_disk().await?;

        if let Some(token) = self.cached_token(account_id).await {
            return Ok(token);
        }

        let github_token = self.github_token_for_account(account_id).await?;
        let session = self.exchange_session_token(&github_token).await?;
        let token = session.token.clone();
        self.session_tokens
            .write()
            .await
            .insert(account_id.to_string(), session);
        Ok(token)
    }

    pub async fn get_valid_token(&self) -> Result<String, GitHubCopilotOAuthError> {
        let account_id = self
            .default_account_id()
            .await
            .ok_or(GitHubCopilotOAuthError::DefaultAccountNotFound)?;
        self.get_valid_token_for_account(&account_id).await
    }

    pub async fn default_account_id(&self) -> Option<String> {
        if self.reload_from_disk().await.is_err() {
            return None;
        }

        let stored = self.default_account_id.read().await.clone();
        if stored.is_some() {
            return stored;
        }

        let accounts = self.accounts.read().await;
        Self::fallback_default_account_id(&accounts)
    }

    async fn reload_from_disk(&self) -> Result<(), GitHubCopilotOAuthError> {
        let store = if !self.storage_path.exists() {
            GitHubCopilotAuthStore {
                accounts: HashMap::new(),
                default_account_id: None,
            }
        } else {
            let content = fs::read_to_string(&self.storage_path)?;
            serde_json::from_str::<GitHubCopilotAuthStore>(&content)
                .map_err(|err| GitHubCopilotOAuthError::ParseError(err.to_string()))?
        };

        let mut accounts = self.accounts.write().await;
        *accounts = store.accounts;
        let valid_ids = accounts.keys().cloned().collect::<Vec<_>>();

        let mut default_account_id = self.default_account_id.write().await;
        *default_account_id = store.default_account_id;

        self.session_tokens
            .write()
            .await
            .retain(|account_id, _| valid_ids.iter().any(|id| id == account_id));

        Ok(())
    }

    async fn cached_token(&self, account_id: &str) -> Option<String> {
        self.session_tokens
            .read()
            .await
            .get(account_id)
            .filter(|token| !token.is_expiring_soon())
            .map(|token| token.token.clone())
    }

    async fn github_token_for_account(
        &self,
        account_id: &str,
    ) -> Result<String, GitHubCopilotOAuthError> {
        self.accounts
            .read()
            .await
            .get(account_id)
            .map(|account| account.github_token.clone())
            .filter(|token| !token.trim().is_empty())
            .ok_or_else(|| GitHubCopilotOAuthError::AccountNotFound(account_id.to_string()))
    }

    async fn refresh_lock(&self, account_id: &str) -> Arc<Mutex<()>> {
        {
            let locks = self.refresh_locks.read().await;
            if let Some(lock) = locks.get(account_id) {
                return Arc::clone(lock);
            }
        }

        let mut locks = self.refresh_locks.write().await;
        Arc::clone(
            locks
                .entry(account_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    }

    async fn exchange_session_token(
        &self,
        github_token: &str,
    ) -> Result<CachedSessionToken, GitHubCopilotOAuthError> {
        if github_token.trim().is_empty() {
            return Err(GitHubCopilotOAuthError::CredentialsMissing);
        }

        let response = self
            .http_client
            .get(session_token_url())
            .header("Authorization", format!("token {github_token}"))
            .header("Editor-Version", COPILOT_EDITOR_VERSION)
            .header("Editor-Plugin-Version", COPILOT_EDITOR_PLUGIN_VERSION)
            .header("Accept", "application/json")
            .header("User-Agent", COPILOT_USER_AGENT)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(GitHubCopilotOAuthError::TokenExchangeFailed(format!(
                "{status} - {text}"
            )));
        }

        let payload: CopilotSessionTokenResponse = response
            .json()
            .await
            .map_err(|err| GitHubCopilotOAuthError::ParseError(err.to_string()))?;

        if payload.token.trim().is_empty() {
            return Err(GitHubCopilotOAuthError::ParseError(
                "token response missing token".to_string(),
            ));
        }

        Ok(CachedSessionToken {
            token: payload.token,
            expires_at_ms: payload.expires_at * 1000,
        })
    }

    fn fallback_default_account_id(
        accounts: &HashMap<String, GitHubCopilotAccountData>,
    ) -> Option<String> {
        accounts
            .iter()
            .max_by_key(|(_, account)| account.authenticated_at)
            .map(|(account_id, _)| account_id.clone())
    }
}

fn session_token_url() -> String {
    std::env::var("CC_SWITCH_GITHUB_COPILOT_TOKEN_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| COPILOT_SESSION_TOKEN_URL.to_string())
}

fn manager_store() -> &'static RwLock<Option<(PathBuf, Arc<GitHubCopilotOAuthManager>)>> {
    static STORE: OnceLock<RwLock<Option<(PathBuf, Arc<GitHubCopilotOAuthManager>)>>> =
        OnceLock::new();
    STORE.get_or_init(|| RwLock::new(None))
}

pub struct GitHubCopilotOAuthService;

impl GitHubCopilotOAuthService {
    fn manager() -> Arc<GitHubCopilotOAuthManager> {
        let path = get_app_config_dir();
        {
            let guard = manager_store()
                .read()
                .expect("read github copilot oauth manager");
            if let Some((cached_path, manager)) = guard.as_ref() {
                if cached_path == &path {
                    return Arc::clone(manager);
                }
            }
        }

        let manager = Arc::new(GitHubCopilotOAuthManager::new(path.clone()));
        let mut guard = manager_store()
            .write()
            .expect("write github copilot oauth manager");
        *guard = Some((path, Arc::clone(&manager)));
        manager
    }

    pub async fn get_valid_token_for_account(
        account_id: &str,
    ) -> Result<String, GitHubCopilotOAuthError> {
        Self::manager().get_valid_token_for_account(account_id).await
    }

    pub async fn get_valid_token() -> Result<String, GitHubCopilotOAuthError> {
        Self::manager().get_valid_token().await
    }

    #[cfg(test)]
    pub(crate) fn reset_for_tests() {
        let mut guard = manager_store()
            .write()
            .expect("write github copilot oauth manager");
        *guard = None;
    }
}
