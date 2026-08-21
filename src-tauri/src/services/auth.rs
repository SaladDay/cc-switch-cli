use crate::proxy::providers::codex_oauth_auth::CodexOAuthError;
use crate::services::CodexOAuthService;

pub mod claude;
pub mod codex;

use claude::PROVIDER as AUTH_PROVIDER_CLAUDE_OAUTH;
use codex::PROVIDER as AUTH_PROVIDER_CODEX_OAUTH;

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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct BrowserAuthStart {
    pub provider: String,
    pub authorization_url: String,
    pub state: String,
    pub redirect_uri: String,
    pub expires_in: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "flow", rename_all = "snake_case")]
pub enum AuthStartResponse {
    DeviceCode(ManagedAuthDeviceCodeResponse),
    Browser(BrowserAuthStart),
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct AuthCompletionResponse {
    pub provider: String,
    pub account_id: String,
    pub login: Option<String>,
    pub organization_id: Option<String>,
    pub is_default: bool,
}

fn unsupported(provider: &str, operation: &str) -> String {
    format!("Auth provider '{provider}' does not support {operation}")
}

fn ensure_auth_provider(auth_provider: &str) -> Result<&'static str, String> {
    match auth_provider {
        AUTH_PROVIDER_CODEX_OAUTH => Ok(AUTH_PROVIDER_CODEX_OAUTH),
        AUTH_PROVIDER_CLAUDE_OAUTH => Ok(AUTH_PROVIDER_CLAUDE_OAUTH),
        _ => Err(format!("Unsupported auth provider: {auth_provider}")),
    }
}

pub struct AuthService;

impl AuthService {
    /// Start the provider's native auth strategy. Browser providers return an
    /// authorization URL; device providers return a device-code response.
    pub async fn start(
        auth_provider: &str,
        redirect_uri: Option<&str>,
    ) -> Result<AuthStartResponse, String> {
        let auth_provider = ensure_auth_provider(auth_provider)?;
        match auth_provider {
            AUTH_PROVIDER_CODEX_OAUTH => CodexOAuthService::start_device_flow()
                .await
                .map(|response| {
                    AuthStartResponse::DeviceCode(codex::map_device_code_response(
                        auth_provider,
                        response,
                    ))
                })
                .map_err(|error| error.to_string()),
            AUTH_PROVIDER_CLAUDE_OAUTH => claude::manager(crate::config::get_claude_config_dir())
                .start_login(redirect_uri)
                .await
                .map(AuthStartResponse::Browser),
            _ => unreachable!("validated provider must have a start strategy"),
        }
    }

    /// Complete a browser callback for a provider that uses authorization code
    /// OAuth. Device-code providers are completed by `poll_for_account`.
    pub async fn complete(
        auth_provider: &str,
        callback_url: &str,
    ) -> Result<AuthCompletionResponse, String> {
        let auth_provider = ensure_auth_provider(auth_provider)?;
        match auth_provider {
            AUTH_PROVIDER_CLAUDE_OAUTH => {
                claude::manager(crate::config::get_claude_config_dir())
                    .complete_login(callback_url)
                    .await
            }
            AUTH_PROVIDER_CODEX_OAUTH => Err("Auth provider uses device-code polling".into()),
            _ => unreachable!("validated provider must have a completion strategy"),
        }
    }

    pub async fn start_login(auth_provider: &str) -> Result<ManagedAuthDeviceCodeResponse, String> {
        match Self::start(auth_provider, None).await? {
            AuthStartResponse::DeviceCode(response) => Ok(response),
            AuthStartResponse::Browser(_) => Err("Auth provider uses browser callback".into()),
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
                        codex::map_account(auth_provider, account, default_account_id.as_deref())
                    }))
                }
                Err(CodexOAuthError::AuthorizationPending) => Ok(None),
                Err(error) => Err(error.to_string()),
            },
            _ => Err(unsupported(auth_provider, "device-code polling")),
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
                        codex::map_account(auth_provider, account, default_account_id.as_deref())
                    })
                    .collect())
            }
            _ => Err(unsupported(auth_provider, "account listing")),
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
                            codex::map_account(
                                auth_provider,
                                account,
                                default_account_id.as_deref(),
                            )
                        })
                        .collect(),
                })
            }
            _ => Err(unsupported(auth_provider, "status")),
        }
    }

    pub async fn remove_account(auth_provider: &str, account_id: &str) -> Result<(), String> {
        let auth_provider = ensure_auth_provider(auth_provider)?;
        match auth_provider {
            AUTH_PROVIDER_CODEX_OAUTH => CodexOAuthService::remove_account(account_id)
                .await
                .map_err(|error| error.to_string()),
            _ => Err(unsupported(auth_provider, "account removal")),
        }
    }

    pub async fn set_default_account(auth_provider: &str, account_id: &str) -> Result<(), String> {
        let auth_provider = ensure_auth_provider(auth_provider)?;
        match auth_provider {
            AUTH_PROVIDER_CODEX_OAUTH => CodexOAuthService::set_default_account(account_id)
                .await
                .map_err(|error| error.to_string()),
            _ => Err(unsupported(auth_provider, "default account selection")),
        }
    }

    pub async fn logout(auth_provider: &str) -> Result<(), String> {
        let auth_provider = ensure_auth_provider(auth_provider)?;
        match auth_provider {
            AUTH_PROVIDER_CODEX_OAUTH => CodexOAuthService::clear_auth()
                .await
                .map_err(|error| error.to_string()),
            _ => Err(unsupported(auth_provider, "logout")),
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

    #[tokio::test]
    async fn generic_start_dispatches_by_flow_type() {
        let response = AuthService::start(AUTH_PROVIDER_CLAUDE_OAUTH, None)
            .await
            .expect("start browser auth");
        let AuthStartResponse::Browser(response) = response else {
            panic!("Claude must use the browser callback strategy");
        };
        assert_eq!(response.provider, AUTH_PROVIDER_CLAUDE_OAUTH);
        assert!(response.authorization_url.starts_with("https://claude.ai/"));
    }

    #[tokio::test]
    async fn unsupported_claude_account_operations_are_explicit_errors() {
        let error = AuthService::get_status(AUTH_PROVIDER_CLAUDE_OAUTH)
            .await
            .unwrap_err();
        assert!(error.contains("does not support status"));
    }
}
