use crate::proxy::providers::codex_oauth_auth::CodexOAuthError;
use crate::proxy::providers::kimi_oauth_auth::KimiOAuthError;
use crate::services::{CodexOAuthService, KimiOAuthService};

const AUTH_PROVIDER_CODEX_OAUTH: &str = "codex_oauth";
const AUTH_PROVIDER_KIMI_OAUTH: &str = "kimi_oauth";
const AUTH_PROVIDER_KIMI_CODE: &str = "kimi-code";
const AUTH_PROVIDER_KIMI: &str = "kimi";

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
        AUTH_PROVIDER_KIMI_OAUTH | AUTH_PROVIDER_KIMI_CODE | AUTH_PROVIDER_KIMI => {
            Ok(AUTH_PROVIDER_KIMI_OAUTH)
        }
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

fn map_kimi_account(
    provider: &str,
    account: crate::proxy::providers::kimi_oauth_auth::ManagedAuthAccount,
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

fn map_kimi_device_code_response(
    provider: &str,
    response: crate::proxy::providers::kimi_oauth_auth::ManagedAuthDeviceCodeResponse,
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
            AUTH_PROVIDER_KIMI_OAUTH => KimiOAuthService::start_device_flow()
                .await
                .map(|response| map_kimi_device_code_response(auth_provider, response))
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
            AUTH_PROVIDER_KIMI_OAUTH => match KimiOAuthService::poll_for_token(device_code).await
            {
                Ok(account) => {
                    let default_account_id =
                        KimiOAuthService::get_status().await.default_account_id;
                    Ok(account.map(|account| {
                        map_kimi_account(auth_provider, account, default_account_id.as_deref())
                    }))
                }
                Err(KimiOAuthError::AuthorizationPending) => Ok(None),
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
            AUTH_PROVIDER_KIMI_OAUTH => {
                let status = KimiOAuthService::get_status().await;
                let default_account_id = status.default_account_id.clone();
                Ok(status
                    .accounts
                    .into_iter()
                    .map(|account| {
                        map_kimi_account(auth_provider, account, default_account_id.as_deref())
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
            AUTH_PROVIDER_KIMI_OAUTH => {
                let status = KimiOAuthService::get_status().await;
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
                            map_kimi_account(auth_provider, account, default_account_id.as_deref())
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
            AUTH_PROVIDER_KIMI_OAUTH => KimiOAuthService::remove_account(account_id)
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
            AUTH_PROVIDER_KIMI_OAUTH => KimiOAuthService::set_default_account(account_id)
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
            AUTH_PROVIDER_KIMI_OAUTH => KimiOAuthService::clear_auth()
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

    #[tokio::test]
    #[expect(
        clippy::await_holding_lock,
        reason = "test serializes global auth manager state"
    )]
    async fn auth_status_marks_default_account() {
        let _lock = lock_test_home_and_settings();
        let _manager = CodexOAuthService::test_manager_with_account(
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
        let default_account = status
            .accounts
            .iter()
            .find(|account| account.id == "acc-456")
            .expect("find default account");
        assert!(default_account.is_default);
    }

    #[tokio::test]
    #[expect(
        clippy::await_holding_lock,
        reason = "test serializes global auth manager state"
    )]
    async fn kimi_auth_status_marks_default_account() {
        let _lock = lock_test_home_and_settings();
        let temp_kimi = tempfile::tempdir().expect("create tempdir");
        let old_kimi_env = std::env::var_os("KIMI_CODE_HOME");
        std::env::set_var("KIMI_CODE_HOME", temp_kimi.path());

        let _manager = KimiOAuthService::test_manager_with_account(
            "kimi-123",
            "rt-1",
            Some("User1"),
            Some("u1@example.com"),
            Some("at-1"),
            None,
        )
        .await
        .expect("seed first account");
        KimiOAuthService::seed_account_for_tests(
            "kimi-456",
            "rt-2",
            Some("User2"),
            Some("u2@example.com"),
            Some("at-2"),
            None,
        )
        .await
        .expect("seed second account");
        AuthService::set_default_account("kimi_oauth", "kimi-456")
            .await
            .expect("set default account");

        let status = AuthService::get_status("kimi_oauth")
            .await
            .expect("get auth status");

        assert_eq!(status.provider, "kimi_oauth");
        assert!(status.authenticated);
        assert_eq!(status.default_account_id.as_deref(), Some("kimi-456"));
        let default_account = status
            .accounts
            .iter()
            .find(|account| account.id == "kimi-456")
            .expect("find default account");
        assert!(default_account.is_default);

        if let Some(val) = old_kimi_env {
            std::env::set_var("KIMI_CODE_HOME", val);
        } else {
            std::env::remove_var("KIMI_CODE_HOME");
        }
    }
}
