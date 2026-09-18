use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use crate::config::get_app_config_dir;
use crate::proxy::providers::kimi_oauth_auth::{
    KimiOAuthError, KimiOAuthManager, KimiOAuthStatus, ManagedAuthAccount,
    ManagedAuthDeviceCodeResponse,
};

type KimiOAuthManagerStore = RwLock<Option<(PathBuf, Arc<KimiOAuthManager>)>>;

fn manager_store() -> &'static KimiOAuthManagerStore {
    static STORE: OnceLock<KimiOAuthManagerStore> = OnceLock::new();
    STORE.get_or_init(|| RwLock::new(None))
}

#[cfg(test)]
fn test_manager_override() -> &'static RwLock<Option<Arc<KimiOAuthManager>>> {
    static STORE: OnceLock<RwLock<Option<Arc<KimiOAuthManager>>>> = OnceLock::new();
    STORE.get_or_init(|| RwLock::new(None))
}

#[cfg(test)]
pub(crate) struct TestKimiOAuthManagerGuard {
    _temp: tempfile::TempDir,
    _manager: Arc<KimiOAuthManager>,
}

#[cfg(test)]
impl Drop for TestKimiOAuthManagerGuard {
    fn drop(&mut self) {
        KimiOAuthService::reset_for_tests();
    }
}

pub struct KimiOAuthService;

impl KimiOAuthService {
    pub fn manager() -> Arc<KimiOAuthManager> {
        #[cfg(test)]
        {
            let guard = test_manager_override()
                .read()
                .expect("read kimi oauth test manager");
            if let Some(manager) = guard.as_ref() {
                return Arc::clone(manager);
            }
        }

        let path = get_app_config_dir();
        {
            let guard = manager_store().read().expect("read kimi oauth manager");
            if let Some((cached_path, manager)) = guard.as_ref() {
                if cached_path == &path {
                    return Arc::clone(manager);
                }
            }
        }

        let manager = Arc::new(KimiOAuthManager::new(path.clone()));
        let mut guard = manager_store().write().expect("write kimi oauth manager");
        *guard = Some((path, Arc::clone(&manager)));
        manager
    }

    #[cfg(test)]
    pub(crate) fn set_manager_for_tests(manager: Arc<KimiOAuthManager>) {
        let mut guard = test_manager_override()
            .write()
            .expect("write kimi oauth test manager");
        *guard = Some(manager);
    }

    #[cfg(test)]
    pub(crate) fn reset_for_tests() {
        let mut guard = test_manager_override()
            .write()
            .expect("reset kimi oauth test manager");
        *guard = None;
        let mut store = manager_store()
            .write()
            .expect("reset kimi oauth manager store");
        *store = None;
    }

    #[cfg(test)]
    pub(crate) async fn test_manager_with_account(
        account_id: &str,
        refresh_token: &str,
        nickname: Option<&str>,
        email: Option<&str>,
        access_token: Option<&str>,
        expires_at_ms: Option<i64>,
    ) -> Result<TestKimiOAuthManagerGuard, KimiOAuthError> {
        let temp = tempfile::tempdir().expect("create tempdir");
        let manager = Arc::new(KimiOAuthManager::new(temp.path().to_path_buf()));
        manager
            .seed_account_for_tests(
                account_id,
                refresh_token,
                nickname,
                email,
                access_token,
                expires_at_ms,
            )
            .await?;
        Self::set_manager_for_tests(Arc::clone(&manager));
        Ok(TestKimiOAuthManagerGuard {
            _temp: temp,
            _manager: manager,
        })
    }

    #[cfg(test)]
    pub(crate) async fn seed_account_for_tests(
        account_id: &str,
        refresh_token: &str,
        nickname: Option<&str>,
        email: Option<&str>,
        access_token: Option<&str>,
        expires_at_ms: Option<i64>,
    ) -> Result<(), KimiOAuthError> {
        let manager = Self::manager();
        manager
            .seed_account_for_tests(
                account_id,
                refresh_token,
                nickname,
                email,
                access_token,
                expires_at_ms,
            )
            .await
    }

    pub async fn start_device_flow() -> Result<ManagedAuthDeviceCodeResponse, KimiOAuthError> {
        Self::manager().start_device_flow().await
    }

    pub async fn poll_for_token(
        device_code: &str,
    ) -> Result<Option<ManagedAuthAccount>, KimiOAuthError> {
        Self::manager().poll_for_token(device_code).await
    }

    pub async fn get_status() -> KimiOAuthStatus {
        Self::manager().get_status().await
    }

    pub async fn remove_account(account_id: &str) -> Result<(), KimiOAuthError> {
        Self::manager().remove_account(account_id).await
    }

    pub async fn set_default_account(account_id: &str) -> Result<(), KimiOAuthError> {
        Self::manager().set_default_account(account_id).await
    }

    pub async fn clear_auth() -> Result<(), KimiOAuthError> {
        Self::manager().clear_auth().await
    }

    #[allow(dead_code)]
    pub async fn get_valid_token_for_account(
        account_id: &str,
    ) -> Result<String, KimiOAuthError> {
        Self::manager().get_valid_token_for_account(account_id).await
    }

    #[allow(dead_code)]
    pub async fn get_valid_token() -> Result<String, KimiOAuthError> {
        Self::manager().get_valid_token().await
    }
}
