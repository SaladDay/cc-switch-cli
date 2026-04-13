use crate::proxy::providers::codex_oauth_auth::CodexOAuthError;
use crate::services::CodexOAuthService;

const AUTH_PROVIDER_CODEX_OAUTH: &str = "codex_oauth";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ManagedAuthAccount {
    pub id: String,
    pub provider: String,
    pub login: String,
    pub avatar_url: Option<String>,
    pub authenticated_at: i64,
    pub is_default: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ManagedAuthStatus {
    pub provider: String,
    pub authenticated: bool,
    pub default_account_id: Option<String>,
    pub migration_error: Option<String>,
    pub accounts: Vec<ManagedAuthAccount>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ManagedAuthDeviceCodeResponse {
    pub provider: String,
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

fn ensure_auth_provider(auth_provider: &str) -> Result<&'static str, String> {
    match auth_provider {
        AUTH_PROVIDER_CODEX_OAUTH => Ok(AUTH_PROVIDER_CODEX_OAUTH),
        _ => Err(format!("Unsupported auth provider: {auth_provider}")),
    }
}

fn map_account(
    provider: &str,
    account: crate::proxy::providers::codex_oauth_auth::ManagedAuthAccount,
    default_account_id: Option<&str>,
) -> ManagedAuthAccount {
    ManagedAuthAccount {
        is_default: default_account_id == Some(account.id.as_str()),
        id: account.id,
        provider: provider.to_string(),
        login: account.login,
        avatar_url: account.avatar_url,
        authenticated_at: account.authenticated_at,
    }
}

fn map_device_code_response(
    provider: &str,
    response: crate::proxy::providers::codex_oauth_auth::ManagedAuthDeviceCodeResponse,
) -> ManagedAuthDeviceCodeResponse {
    ManagedAuthDeviceCodeResponse {
        provider: provider.to_string(),
        device_code: response.device_code,
        user_code: response.user_code,
        verification_uri: response.verification_uri,
        expires_in: response.expires_in,
        interval: response.interval,
    }
}

pub struct AuthService;

impl AuthService {
    pub async fn start_login(auth_provider: &str) -> Result<ManagedAuthDeviceCodeResponse, String> {
        let auth_provider = ensure_auth_provider(auth_provider)?;
        match auth_provider {
            AUTH_PROVIDER_CODEX_OAUTH => CodexOAuthService::start_device_flow()
                .await
                .map(|response| map_device_code_response(auth_provider, response))
                .map_err(|error| error.to_string()),
            _ => unreachable!(),
        }
    }

    pub async fn poll_for_account(
        auth_provider: &str,
        device_code: &str,
    ) -> Result<Option<ManagedAuthAccount>, String> {
        let auth_provider = ensure_auth_provider(auth_provider)?;
        match auth_provider {
            AUTH_PROVIDER_CODEX_OAUTH => match CodexOAuthService::poll_for_token(device_code).await
            {
                Ok(account) => {
                    let default_account_id =
                        CodexOAuthService::get_status().await.default_account_id;
                    Ok(account.map(|account| {
                        map_account(auth_provider, account, default_account_id.as_deref())
                    }))
                }
                Err(CodexOAuthError::AuthorizationPending) => Ok(None),
                Err(error) => Err(error.to_string()),
            },
            _ => unreachable!(),
        }
    }

    pub async fn list_accounts(auth_provider: &str) -> Result<Vec<ManagedAuthAccount>, String> {
        let auth_provider = ensure_auth_provider(auth_provider)?;
        match auth_provider {
            AUTH_PROVIDER_CODEX_OAUTH => {
                let status = CodexOAuthService::get_status().await;
                let default_account_id = status.default_account_id.clone();
                Ok(status
                    .accounts
                    .into_iter()
                    .map(|account| {
                        map_account(auth_provider, account, default_account_id.as_deref())
                    })
                    .collect())
            }
            _ => unreachable!(),
        }
    }

    pub async fn get_status(auth_provider: &str) -> Result<ManagedAuthStatus, String> {
        let auth_provider = ensure_auth_provider(auth_provider)?;
        match auth_provider {
            AUTH_PROVIDER_CODEX_OAUTH => {
                let status = CodexOAuthService::get_status().await;
                let default_account_id = status.default_account_id.clone();
                Ok(ManagedAuthStatus {
                    provider: auth_provider.to_string(),
                    authenticated: status.authenticated,
                    default_account_id: default_account_id.clone(),
                    migration_error: None,
                    accounts: status
                        .accounts
                        .into_iter()
                        .map(|account| {
                            map_account(auth_provider, account, default_account_id.as_deref())
                        })
                        .collect(),
                })
            }
            _ => unreachable!(),
        }
    }

    pub async fn remove_account(auth_provider: &str, account_id: &str) -> Result<(), String> {
        let auth_provider = ensure_auth_provider(auth_provider)?;
        match auth_provider {
            AUTH_PROVIDER_CODEX_OAUTH => CodexOAuthService::remove_account(account_id)
                .await
                .map_err(|error| error.to_string()),
            _ => unreachable!(),
        }
    }

    pub async fn set_default_account(auth_provider: &str, account_id: &str) -> Result<(), String> {
        let auth_provider = ensure_auth_provider(auth_provider)?;
        match auth_provider {
            AUTH_PROVIDER_CODEX_OAUTH => CodexOAuthService::set_default_account(account_id)
                .await
                .map_err(|error| error.to_string()),
            _ => unreachable!(),
        }
    }

    pub async fn logout(auth_provider: &str) -> Result<(), String> {
        let auth_provider = ensure_auth_provider(auth_provider)?;
        match auth_provider {
            AUTH_PROVIDER_CODEX_OAUTH => CodexOAuthService::clear_auth()
                .await
                .map_err(|error| error.to_string()),
            _ => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::lock_test_home_and_settings;
    use std::{env, ffi::OsString};

    struct ConfigDirEnvGuard {
        original: Option<OsString>,
    }

    impl ConfigDirEnvGuard {
        fn set(value: Option<&str>) -> Self {
            let original = env::var_os("CC_SWITCH_CONFIG_DIR");
            match value {
                Some(value) => unsafe { env::set_var("CC_SWITCH_CONFIG_DIR", value) },
                None => unsafe { env::remove_var("CC_SWITCH_CONFIG_DIR") },
            }
            Self { original }
        }
    }

    impl Drop for ConfigDirEnvGuard {
        fn drop(&mut self) {
            match self.original.as_ref() {
                Some(value) => unsafe { env::set_var("CC_SWITCH_CONFIG_DIR", value) },
                None => unsafe { env::remove_var("CC_SWITCH_CONFIG_DIR") },
            }
        }
    }

    #[tokio::test]
    async fn auth_status_marks_default_account() {
        let _lock = lock_test_home_and_settings();
        let temp = tempfile::tempdir().expect("create temp dir");
        let _guard = ConfigDirEnvGuard::set(Some(temp.path().to_string_lossy().as_ref()));
        CodexOAuthService::reset_for_tests();

        CodexOAuthService::seed_account_for_tests(
            "acc-123",
            "rt-1",
            Some("a@example.com"),
            Some("at-1"),
            None,
        )
        .await
        .expect("seed first account");
        CodexOAuthService::seed_account_for_tests(
            "acc-456",
            "rt-2",
            Some("b@example.com"),
            Some("at-2"),
            None,
        )
        .await
        .expect("seed second account");
        AuthService::set_default_account("codex_oauth", "acc-456")
            .await
            .expect("set default account");

        let status = AuthService::get_status("codex_oauth")
            .await
            .expect("get auth status");

        assert_eq!(status.provider, "codex_oauth");
        assert!(status.authenticated);
        assert_eq!(status.default_account_id.as_deref(), Some("acc-456"));
        assert_eq!(status.accounts.len(), 2);
        assert_eq!(status.accounts[0].id, "acc-456");
        assert!(status.accounts[0].is_default);
        assert!(!status.accounts[1].is_default);
    }
}
