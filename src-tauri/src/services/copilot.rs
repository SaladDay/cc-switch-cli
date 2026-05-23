use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use crate::config::get_app_config_dir;
use crate::proxy::providers::copilot_auth::{
    CopilotAuthError, CopilotAuthManager, CopilotAuthStatus, CopilotModel, CopilotUsageResponse,
    GitHubAccount, GitHubDeviceCodeResponse,
};

fn manager_store() -> &'static RwLock<Option<(PathBuf, Arc<CopilotAuthManager>)>> {
    static STORE: OnceLock<RwLock<Option<(PathBuf, Arc<CopilotAuthManager>)>>> = OnceLock::new();
    STORE.get_or_init(|| RwLock::new(None))
}

pub struct CopilotService;

impl CopilotService {
    pub fn manager() -> Arc<CopilotAuthManager> {
        let path = get_app_config_dir();
        {
            let guard = manager_store().read().expect("read copilot auth manager");
            if let Some((cached_path, manager)) = guard.as_ref() {
                if cached_path == &path {
                    return Arc::clone(manager);
                }
            }
        }

        let manager = Arc::new(CopilotAuthManager::new(path.clone()));
        let mut guard = manager_store().write().expect("write copilot auth manager");
        *guard = Some((path, Arc::clone(&manager)));
        manager
    }

    #[cfg(test)]
    pub(crate) fn reset_for_tests() {
        let mut guard = manager_store().write().expect("write copilot auth manager");
        *guard = None;
    }

    pub async fn start_device_flow(
        github_domain: Option<&str>,
    ) -> Result<GitHubDeviceCodeResponse, CopilotAuthError> {
        Self::manager().start_device_flow(github_domain).await
    }

    pub async fn poll_for_token(
        device_code: &str,
        github_domain: Option<&str>,
    ) -> Result<Option<GitHubAccount>, CopilotAuthError> {
        Self::manager()
            .poll_for_token(device_code, github_domain)
            .await
    }

    pub async fn get_valid_token_for_account(
        account_id: &str,
    ) -> Result<String, CopilotAuthError> {
        Self::manager()
            .get_valid_token_for_account(account_id)
            .await
    }

    pub async fn get_valid_token() -> Result<String, CopilotAuthError> {
        Self::manager().get_valid_token().await
    }

    pub async fn list_accounts() -> Vec<GitHubAccount> {
        Self::manager().list_accounts().await
    }

    pub async fn get_account(account_id: &str) -> Option<GitHubAccount> {
        Self::manager().get_account(account_id).await
    }

    pub async fn remove_account(account_id: &str) -> Result<(), CopilotAuthError> {
        Self::manager().remove_account(account_id).await
    }

    pub async fn set_default_account(account_id: &str) -> Result<(), CopilotAuthError> {
        Self::manager().set_default_account(account_id).await
    }

    pub async fn clear_auth() -> Result<(), CopilotAuthError> {
        Self::manager().clear_auth().await
    }

    pub async fn get_status() -> CopilotAuthStatus {
        Self::manager().get_status().await
    }

    pub async fn is_authenticated() -> bool {
        Self::manager().is_authenticated().await
    }

    pub async fn get_api_endpoint(account_id: &str) -> String {
        Self::manager().get_api_endpoint(account_id).await
    }

    pub async fn get_default_api_endpoint() -> String {
        Self::manager().get_default_api_endpoint().await
    }

    pub async fn fetch_models() -> Result<Vec<CopilotModel>, CopilotAuthError> {
        Self::manager().fetch_models().await
    }

    pub async fn fetch_models_for_account(
        account_id: &str,
    ) -> Result<Vec<CopilotModel>, CopilotAuthError> {
        Self::manager().fetch_models_for_account(account_id).await
    }

    pub async fn fetch_usage() -> Result<CopilotUsageResponse, CopilotAuthError> {
        Self::manager().fetch_usage().await
    }

    pub async fn fetch_usage_for_account(
        account_id: &str,
    ) -> Result<CopilotUsageResponse, CopilotAuthError> {
        Self::manager().fetch_usage_for_account(account_id).await
    }

    pub async fn get_model_vendor(
        model_id: &str,
    ) -> Result<Option<String>, CopilotAuthError> {
        Self::manager().get_model_vendor(model_id).await
    }

    pub async fn get_model_vendor_for_account(
        account_id: &str,
        model_id: &str,
    ) -> Result<Option<String>, CopilotAuthError> {
        Self::manager()
            .get_model_vendor_for_account(account_id, model_id)
            .await
    }
}
