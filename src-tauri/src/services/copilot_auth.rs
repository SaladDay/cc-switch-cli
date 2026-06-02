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

pub struct CopilotAuthService;

impl CopilotAuthService {
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

    #[allow(dead_code)]
    pub async fn start_device_flow(
        domain: Option<&str>,
    ) -> Result<GitHubDeviceCodeResponse, CopilotAuthError> {
        Self::manager().start_device_flow(domain).await
    }

    #[allow(dead_code)]
    pub async fn poll_for_token(
        device_code: &str,
    ) -> Result<Option<GitHubAccount>, CopilotAuthError> {
        Self::manager().poll_for_token(device_code, None).await
    }

    pub async fn get_valid_token_for_account(account_id: &str) -> Result<String, CopilotAuthError> {
        Self::manager()
            .get_valid_token_for_account(account_id)
            .await
    }

    pub async fn get_valid_token() -> Result<String, CopilotAuthError> {
        Self::manager().get_valid_token().await
    }

    pub async fn get_model_vendor_for_account(
        account_id: &str,
        model_id: &str,
    ) -> Result<Option<String>, CopilotAuthError> {
        Self::manager()
            .get_model_vendor_for_account(account_id, model_id)
            .await
    }

    pub async fn get_model_vendor(model_id: &str) -> Result<Option<String>, CopilotAuthError> {
        Self::manager().get_model_vendor(model_id).await
    }

    pub async fn fetch_models_for_account(
        account_id: &str,
    ) -> Result<Vec<CopilotModel>, CopilotAuthError> {
        Self::manager().fetch_models_for_account(account_id).await
    }

    pub async fn fetch_models() -> Result<Vec<CopilotModel>, CopilotAuthError> {
        Self::manager().fetch_models().await
    }

    pub async fn get_api_endpoint(account_id: &str) -> String {
        Self::manager().get_api_endpoint(account_id).await
    }

    pub async fn get_default_api_endpoint() -> String {
        Self::manager().get_default_api_endpoint().await
    }

    #[allow(dead_code)]
    pub async fn fetch_usage_for_account(
        account_id: &str,
    ) -> Result<CopilotUsageResponse, CopilotAuthError> {
        Self::manager().fetch_usage_for_account(account_id).await
    }

    #[allow(dead_code)]
    pub async fn fetch_usage() -> Result<CopilotUsageResponse, CopilotAuthError> {
        Self::manager().fetch_usage().await
    }

    #[allow(dead_code)]
    pub async fn get_status() -> CopilotAuthStatus {
        Self::manager().get_status().await
    }

    #[cfg(test)]
    pub(crate) async fn seed_account_for_tests(
        account_id: &str,
        github_token: &str,
        copilot_token: Option<&str>,
        api_endpoint: Option<&str>,
        models: Vec<CopilotModel>,
    ) -> Result<(), CopilotAuthError> {
        Self::manager()
            .seed_account_for_tests(
                account_id,
                github_token,
                copilot_token,
                api_endpoint,
                models,
            )
            .await
    }
}
