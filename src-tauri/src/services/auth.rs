use crate::proxy::providers::codex_oauth_auth::CodexOAuthError;
use crate::services::CodexOAuthService;

const AUTH_PROVIDER_CODEX_OAUTH: &str = "codex_oauth";
const AUTH_PROVIDER_XAI_OAUTH: &str = "xai_oauth";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ManagedAuthAccount {
    pub id: String,
    pub provider: String,
    pub login: String,
    pub avatar_url: Option<String>,
    pub authenticated_at: i64,
    pub is_default: bool,
    #[serde(default)]
    pub reauth_required: bool,
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
        AUTH_PROVIDER_XAI_OAUTH => Ok(AUTH_PROVIDER_XAI_OAUTH),
        _ => Err(format!("Unsupported auth provider: {auth_provider}")),
    }
}

fn map_xai_account(
    provider: &str,
    account: crate::proxy::providers::xai_oauth_auth::XaiOAuthAccount,
    default_account_id: Option<&str>,
) -> ManagedAuthAccount {
    ManagedAuthAccount {
        is_default: default_account_id == Some(account.id.as_str()),
        id: account.id,
        provider: provider.to_string(),
        login: account.login,
        avatar_url: account.avatar_url,
        authenticated_at: account.authenticated_at,
        reauth_required: account.requires_reauth,
    }
}

fn map_xai_device_code_response(
    provider: &str,
    response: crate::proxy::providers::xai_oauth_auth::XaiDeviceCodeResponse,
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
        reauth_required: account.reauth_required,
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
        Self::start_login_for_account(auth_provider, None).await
    }

    pub async fn start_login_for_account(
        auth_provider: &str,
        account_id: Option<&str>,
    ) -> Result<ManagedAuthDeviceCodeResponse, String> {
        let auth_provider = ensure_auth_provider(auth_provider)?;
        match auth_provider {
            AUTH_PROVIDER_CODEX_OAUTH => CodexOAuthService::start_device_flow(account_id)
                .await
                .map(|response| map_device_code_response(auth_provider, response))
                .map_err(|error| error.to_string()),
            AUTH_PROVIDER_XAI_OAUTH => {
                crate::proxy::providers::xai_oauth_auth::XaiOAuthManager::shared()
                    .start_device_flow()
                    .await
                    .map(|response| map_xai_device_code_response(auth_provider, response))
                    .map_err(|error| error.to_string())
            }
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
                Err(CodexOAuthError::DuplicateAccount) => Err(
                    "This account is already added. Reauthenticate the existing account instead."
                        .to_string(),
                ),
                Err(error) => Err(error.to_string()),
            },
            AUTH_PROVIDER_XAI_OAUTH => {
                match crate::proxy::providers::xai_oauth_auth::XaiOAuthManager::shared()
                    .poll_for_token(device_code)
                    .await
                {
                    Ok(account) => {
                        let default_account_id = crate::proxy::providers::xai_oauth_auth::XaiOAuthManager::shared()
                            .default_account_id()
                            .await;
                        Ok(account.map(|account| {
                            map_xai_account(auth_provider, account, default_account_id.as_deref())
                        }))
                    }
                    Err(
                        crate::proxy::providers::xai_oauth_auth::XaiOAuthError::AuthorizationPending,
                    ) => Ok(None),
                    Err(error) => Err(error.to_string()),
                }
            }
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
            AUTH_PROVIDER_XAI_OAUTH => {
                let manager = crate::proxy::providers::xai_oauth_auth::XaiOAuthManager::shared();
                let default_account_id = manager.default_account_id().await;
                Ok(manager
                    .list_accounts()
                    .await
                    .into_iter()
                    .map(|account| {
                        map_xai_account(auth_provider, account, default_account_id.as_deref())
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
            AUTH_PROVIDER_XAI_OAUTH => {
                let manager = crate::proxy::providers::xai_oauth_auth::XaiOAuthManager::shared();
                let default_account_id = manager.default_account_id().await;
                let accounts = manager.list_accounts().await;
                Ok(ManagedAuthStatus {
                    provider: auth_provider.to_string(),
                    authenticated: default_account_id.is_some(),
                    default_account_id: default_account_id.clone(),
                    // 存储损坏 ≠ 未登录：显式呈现，避免误导用户重新登录。
                    migration_error: manager.store_load_error().await,
                    accounts: accounts
                        .into_iter()
                        .map(|account| {
                            map_xai_account(auth_provider, account, default_account_id.as_deref())
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
            AUTH_PROVIDER_XAI_OAUTH => {
                crate::proxy::providers::xai_oauth_auth::XaiOAuthManager::shared()
                    .remove_account(account_id)
                    .await
                    .map_err(|error| error.to_string())
            }
            _ => unreachable!(),
        }
    }

    pub async fn set_default_account(auth_provider: &str, account_id: &str) -> Result<(), String> {
        let auth_provider = ensure_auth_provider(auth_provider)?;
        match auth_provider {
            AUTH_PROVIDER_CODEX_OAUTH => CodexOAuthService::set_default_account(account_id)
                .await
                .map_err(|error| error.to_string()),
            AUTH_PROVIDER_XAI_OAUTH => {
                crate::proxy::providers::xai_oauth_auth::XaiOAuthManager::shared()
                    .set_default_account(account_id)
                    .await
                    .map_err(|error| error.to_string())
            }
            _ => unreachable!(),
        }
    }

    pub async fn get_quota(
        auth_provider: &str,
        account_id: Option<&str>,
    ) -> Result<crate::services::subscription::SubscriptionQuota, String> {
        let auth_provider = ensure_auth_provider(auth_provider)?;
        match auth_provider {
            AUTH_PROVIDER_CODEX_OAUTH => Ok(CodexOAuthService::get_quota(account_id).await),
            AUTH_PROVIDER_XAI_OAUTH => {
                let manager = crate::proxy::providers::xai_oauth_auth::XaiOAuthManager::shared();
                let token = match account_id {
                    Some(id) => manager.get_valid_token_for_account(id).await,
                    None => manager.get_valid_token().await,
                }
                .map_err(|error| format!("xAI OAuth authentication failed: {error}"))?;
                crate::services::subscription_grok::query_grok_quota(
                    &token,
                    AUTH_PROVIDER_XAI_OAUTH,
                    "Sign in again with `cc-switch auth xai login`.",
                )
                .await
            }
            _ => unreachable!(),
        }
    }

    pub async fn reset_quota(
        auth_provider: &str,
        account_id: Option<&str>,
        credit_id: Option<&str>,
    ) -> Result<crate::services::subscription::CodexResetConsumeResult, String> {
        let auth_provider = ensure_auth_provider(auth_provider)?;
        match auth_provider {
            AUTH_PROVIDER_CODEX_OAUTH => {
                CodexOAuthService::reset_quota(account_id, credit_id).await
            }
            _ => unreachable!(),
        }
    }

    pub async fn logout(auth_provider: &str) -> Result<(), String> {
        let auth_provider = ensure_auth_provider(auth_provider)?;
        match auth_provider {
            AUTH_PROVIDER_CODEX_OAUTH => CodexOAuthService::clear_auth()
                .await
                .map_err(|error| error.to_string()),
            AUTH_PROVIDER_XAI_OAUTH => {
                crate::proxy::providers::xai_oauth_auth::XaiOAuthManager::shared()
                    .clear_auth()
                    .await
                    .map_err(|error| error.to_string())
            }
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
        assert_eq!(status.accounts.len(), 2);
        assert_eq!(status.accounts[0].id, "acc-456");
        assert!(status.accounts[0].is_default);
        assert!(!status.accounts[1].is_default);
    }
}
