use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use crate::config::get_app_config_dir;
use crate::proxy::providers::codex_oauth_auth::{
    CodexOAuthError, CodexOAuthManager, CodexOAuthStatus, ManagedAuthAccount,
    ManagedAuthDeviceCodeResponse,
};
use crate::services::subscription::{query_codex_quota, CredentialStatus, SubscriptionQuota};

type CodexOAuthManagerStore = RwLock<Option<(PathBuf, Arc<CodexOAuthManager>)>>;

fn manager_store() -> &'static CodexOAuthManagerStore {
    static STORE: OnceLock<CodexOAuthManagerStore> = OnceLock::new();
    STORE.get_or_init(|| RwLock::new(None))
}

#[cfg(test)]
fn test_manager_override() -> &'static RwLock<Option<Arc<CodexOAuthManager>>> {
    static STORE: OnceLock<RwLock<Option<Arc<CodexOAuthManager>>>> = OnceLock::new();
    STORE.get_or_init(|| RwLock::new(None))
}

#[cfg(test)]
pub(crate) struct TestCodexOAuthManagerGuard {
    _temp: tempfile::TempDir,
    _manager: Arc<CodexOAuthManager>,
}

#[cfg(test)]
impl Drop for TestCodexOAuthManagerGuard {
    fn drop(&mut self) {
        CodexOAuthService::reset_for_tests();
    }
}

pub struct CodexOAuthService;

impl CodexOAuthService {
    pub fn manager() -> Arc<CodexOAuthManager> {
        #[cfg(test)]
        {
            let guard = test_manager_override()
                .read()
                .expect("read codex oauth test manager");
            if let Some(manager) = guard.as_ref() {
                return Arc::clone(manager);
            }
        }

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
    pub(crate) fn set_manager_for_tests(manager: Arc<CodexOAuthManager>) {
        let mut guard = test_manager_override()
            .write()
            .expect("write codex oauth test manager");
        *guard = Some(manager);
    }

    #[cfg(test)]
    pub(crate) async fn test_manager_with_account(
        account_id: &str,
        refresh_token: &str,
        email: Option<&str>,
        access_token: Option<&str>,
        expires_at_ms: Option<i64>,
    ) -> Result<TestCodexOAuthManagerGuard, CodexOAuthError> {
        let temp = tempfile::tempdir()?;
        let manager = Arc::new(CodexOAuthManager::new(temp.path().to_path_buf()));
        manager
            .seed_account_for_tests(
                account_id,
                refresh_token,
                email,
                access_token,
                expires_at_ms,
            )
            .await?;
        Self::set_manager_for_tests(Arc::clone(&manager));
        Ok(TestCodexOAuthManagerGuard {
            _temp: temp,
            _manager: manager,
        })
    }

    #[cfg(test)]
    pub(crate) async fn test_empty_manager() -> Result<TestCodexOAuthManagerGuard, CodexOAuthError>
    {
        let temp = tempfile::tempdir()?;
        let manager = Arc::new(CodexOAuthManager::new(temp.path().to_path_buf()));
        Self::set_manager_for_tests(Arc::clone(&manager));
        Ok(TestCodexOAuthManagerGuard {
            _temp: temp,
            _manager: manager,
        })
    }

    #[cfg(test)]
    pub(crate) fn reset_for_tests() {
        let mut test_guard = test_manager_override()
            .write()
            .expect("write codex oauth test manager");
        *test_guard = None;
        drop(test_guard);

        let mut guard = manager_store().write().expect("write codex oauth manager");
        *guard = None;
    }

    pub async fn start_device_flow(
        target_account_id: Option<&str>,
    ) -> Result<ManagedAuthDeviceCodeResponse, CodexOAuthError> {
        Self::manager().start_device_flow(target_account_id).await
    }

    pub async fn poll_for_token(
        device_code: &str,
    ) -> Result<Option<ManagedAuthAccount>, CodexOAuthError> {
        Self::manager()
            .poll_for_token(device_code, || async {})
            .await
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

    #[allow(dead_code)]
    pub async fn list_accounts() -> Vec<ManagedAuthAccount> {
        Self::manager().list_accounts().await
    }

    pub async fn remove_account(account_id: &str) -> Result<(), CodexOAuthError> {
        let _guard = super::state_coordination::acquire_restore_mutation_guard()
            .await
            .map_err(CodexOAuthError::TokenFetchFailed)?;
        Self::manager().remove_account(account_id).await?;
        Self::unbind_deleted_accounts(&[account_id.to_string()])
    }

    pub async fn set_default_account(account_id: &str) -> Result<(), CodexOAuthError> {
        Self::manager().set_default_account(account_id).await
    }

    pub async fn clear_auth() -> Result<(), CodexOAuthError> {
        let _guard = super::state_coordination::acquire_restore_mutation_guard()
            .await
            .map_err(CodexOAuthError::TokenFetchFailed)?;
        let manager = Self::manager();
        let accounts = manager
            .list_accounts()
            .await
            .into_iter()
            .map(|account| account.id)
            .collect::<Vec<_>>();
        manager.clear_auth().await?;
        Self::unbind_deleted_accounts(&accounts)
    }

    fn unbind_deleted_accounts(accounts: &[String]) -> Result<(), CodexOAuthError> {
        if accounts.is_empty() {
            return Ok(());
        }
        crate::Database::init()
            .and_then(|db| db.unbind_codex_managed_accounts(accounts))
            .map(|_| ())
            .map_err(|error| {
                CodexOAuthError::TokenFetchFailed(format!(
                    "账号已删除，但 Codex 供应商解绑失败；请编辑供应商重新选择账号: {error}"
                ))
            })
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

        let workspace = match manager.chatgpt_account_id_for_account(&account_id).await {
            Ok(workspace) => workspace,
            Err(error) => {
                return SubscriptionQuota::error(
                    "codex_oauth",
                    CredentialStatus::Expired,
                    error.to_string(),
                )
            }
        };
        query_codex_quota(
            &token,
            Some(&workspace),
            "codex_oauth",
            "Codex OAuth access token expired or rejected. Please re-login via cc-switch.",
        )
        .await
    }

    pub async fn reset_quota(
        account_id: Option<&str>,
        credit_id: Option<&str>,
    ) -> Result<crate::services::subscription::CodexResetConsumeResult, String> {
        let manager = Self::manager();
        let resolved_account_id = match account_id
            .map(str::trim)
            .filter(|account_id| !account_id.is_empty())
        {
            Some(account_id) => Some(account_id.to_string()),
            None => manager.default_account_id().await,
        };

        let Some(account_id) = resolved_account_id else {
            return Err("No ChatGPT account available".to_string());
        };

        let token = manager
            .get_valid_token_for_account(&account_id)
            .await
            .map_err(|error| format!("Codex OAuth token unavailable: {error}"))?;

        let workspace = manager
            .chatgpt_account_id_for_account(&account_id)
            .await
            .map_err(|e| e.to_string())?;
        crate::services::subscription::consume_codex_reset_credit(
            &token,
            Some(&workspace),
            credit_id,
            "codex_oauth",
        )
        .await
    }

    pub async fn get_models(
        account_id: Option<&str>,
    ) -> Result<Vec<crate::services::FetchedModel>, String> {
        let manager = Self::manager();
        let resolved_account_id = match account_id
            .map(str::trim)
            .filter(|account_id| !account_id.is_empty())
        {
            Some(account_id) => Some(account_id.to_string()),
            None => manager.default_account_id().await,
        };

        let Some(account_id) = resolved_account_id else {
            return Err("No ChatGPT account available".to_string());
        };

        let token = manager
            .get_valid_token_for_account(&account_id)
            .await
            .map_err(|error| format!("Codex OAuth token unavailable: {error}"))?;

        let workspace = manager
            .chatgpt_account_id_for_account(&account_id)
            .await
            .map_err(|e| e.to_string())?;
        crate::services::codex_oauth_models::fetch_models_with_token(&token, &workspace).await
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

#[cfg(test)]
mod managed_removal_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn codex_removal_clears_owned_login_and_detaches_only_deleted_accounts() {
        let temp = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let db = crate::Database::init().unwrap();
        let manager = CodexOAuthService::manager();
        for account in ["alice", "bob"] {
            runtime
                .block_on(manager.add_test_account_with_workspace_and_access_token(
                    account,
                    "workspace",
                    "access",
                    Some(&crate::codex_config::test_codex_id_token(account)),
                ))
                .unwrap();
            let provider = serde_json::from_value(json!({
                "id": account, "name": account, "category": "official",
                "settingsConfig": {"auth": {}, "config": ""},
                "meta": {"codexOfficial": true, "providerType": "codex_oauth", "authBinding": {
                    "source": "managed_account", "authProvider": "codex_oauth", "accountId": account
                }}
            }))
            .unwrap();
            db.save_provider("codex", &provider).unwrap();
        }
        let alice = crate::codex_config::codex_managed_oauth_auth_value(
            "workspace",
            "access",
            Some(&crate::codex_config::test_codex_id_token("alice")),
            "refresh",
            "2026-11-05T00:00:00Z",
        );
        crate::config::write_json_file(&crate::codex_config::get_codex_auth_path(), &alice)
            .unwrap();
        crate::codex_config::record_codex_managed_oauth_live_auth(&alice, "alice").unwrap();
        runtime
            .block_on(CodexOAuthService::remove_account("alice"))
            .unwrap();
        assert!(!crate::codex_config::get_codex_auth_path().exists());
        assert!(!crate::codex_config::get_codex_managed_oauth_live_auth_marker_path().exists());
        let rows = db.get_all_providers("codex").unwrap();
        assert!(rows["alice"].meta.as_ref().unwrap().auth_binding.is_none());
        assert!(!rows["alice"].is_codex_oauth());
        assert_eq!(
            rows["bob"]
                .meta
                .as_ref()
                .unwrap()
                .managed_account_id_for("codex_oauth")
                .as_deref(),
            Some("bob")
        );
        // A native login of another member in the same workspace survives logout.
        let bob = crate::codex_config::codex_managed_oauth_auth_value(
            "workspace",
            "access",
            Some(&crate::codex_config::test_codex_id_token("bob")),
            "refresh",
            "2026-11-05T00:00:00Z",
        );
        crate::codex_config::record_codex_managed_oauth_live_auth(&bob, "bob").unwrap();
        crate::config::write_json_file(&crate::codex_config::get_codex_auth_path(), &alice)
            .unwrap();
        runtime.block_on(CodexOAuthService::clear_auth()).unwrap();
        assert_eq!(
            crate::config::read_json_file::<serde_json::Value>(
                &crate::codex_config::get_codex_auth_path()
            )
            .unwrap(),
            alice
        );
        assert!(!crate::codex_config::get_codex_managed_oauth_live_auth_marker_path().exists());
        assert!(db.get_all_providers("codex").unwrap()["bob"]
            .meta
            .as_ref()
            .unwrap()
            .auth_binding
            .is_none());
    }
}
