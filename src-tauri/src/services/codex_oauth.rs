use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use crate::config::get_app_config_dir;
use crate::proxy::providers::codex_oauth_auth::{
    CodexOAuthError, CodexOAuthManager, CodexOAuthStatus, ManagedAuthAccount,
    ManagedAuthDeviceCodeResponse,
};
use crate::services::subscription::{query_codex_quota, CredentialStatus, SubscriptionQuota};

fn manager_store() -> &'static RwLock<Option<(PathBuf, Arc<CodexOAuthManager>)>> {
    static STORE: OnceLock<RwLock<Option<(PathBuf, Arc<CodexOAuthManager>)>>> = OnceLock::new();
    STORE.get_or_init(|| RwLock::new(None))
}

pub struct CodexOAuthService;

impl CodexOAuthService {
    pub fn manager() -> Arc<CodexOAuthManager> {
        let path = get_app_config_dir();
        {
            let guard = manager_store().read().expect("read codex oauth manager");
            if let Some((cached_path, manager)) = guard.as_ref() {
                if cached_path == &path {
                    return Arc::clone(manager);
                }
            }
        }

        let manager = Arc::new(CodexOAuthManager::new(path.clone()));
        let mut guard = manager_store().write().expect("write codex oauth manager");
        *guard = Some((path, Arc::clone(&manager)));
        manager
    }

    #[cfg(test)]
    pub(crate) fn reset_for_tests() {
        let mut guard = manager_store().write().expect("write codex oauth manager");
        *guard = None;
    }

    pub async fn start_device_flow() -> Result<ManagedAuthDeviceCodeResponse, CodexOAuthError> {
        Self::manager().start_device_flow().await
    }

    pub async fn poll_for_token(
        device_code: &str,
    ) -> Result<Option<ManagedAuthAccount>, CodexOAuthError> {
        Self::manager().poll_for_token(device_code).await
    }

    pub async fn get_valid_token_for_account(account_id: &str) -> Result<String, CodexOAuthError> {
        Self::manager()
            .get_valid_token_for_account(account_id)
            .await
    }

    pub async fn get_valid_token() -> Result<String, CodexOAuthError> {
        Self::manager().get_valid_token().await
    }

    pub async fn default_account_id() -> Option<String> {
        Self::manager().default_account_id().await
    }

    pub async fn list_accounts() -> Vec<ManagedAuthAccount> {
        Self::manager().list_accounts().await
    }

    pub async fn remove_account(account_id: &str) -> Result<(), CodexOAuthError> {
        Self::manager().remove_account(account_id).await
    }

    pub async fn set_default_account(account_id: &str) -> Result<(), CodexOAuthError> {
        Self::manager().set_default_account(account_id).await
    }

    pub async fn clear_auth() -> Result<(), CodexOAuthError> {
        Self::manager().clear_auth().await
    }

    pub async fn get_status() -> CodexOAuthStatus {
        Self::manager().get_status().await
    }

    pub async fn get_quota(account_id: Option<&str>) -> SubscriptionQuota {
        let manager = Self::manager();
        let resolved_account_id = match account_id {
            Some(account_id) => Some(account_id.to_string()),
            None => manager.default_account_id().await,
        };

        let Some(account_id) = resolved_account_id else {
            return SubscriptionQuota::not_found("codex_oauth");
        };

        let token = match manager.get_valid_token_for_account(&account_id).await {
            Ok(token) => token,
            Err(error) => {
                return SubscriptionQuota::error(
                    "codex_oauth",
                    CredentialStatus::Expired,
                    format!("Codex OAuth token unavailable: {error}"),
                );
            }
        };

        query_codex_quota(
            &token,
            Some(&account_id),
            "codex_oauth",
            "Codex OAuth access token expired or rejected. Please re-login via cc-switch.",
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn seed_account_for_tests(
        account_id: &str,
        refresh_token: &str,
        email: Option<&str>,
        access_token: Option<&str>,
        expires_at_ms: Option<i64>,
    ) -> Result<(), CodexOAuthError> {
        Self::manager()
            .seed_account_for_tests(
                account_id,
                refresh_token,
                email,
                access_token,
                expires_at_ms,
            )
            .await
    }
}
