use super::{ManagedAuthAccount, ManagedAuthDeviceCodeResponse};
use crate::proxy::providers::codex_oauth_auth::{
    ManagedAuthAccount as NativeAccount, ManagedAuthDeviceCodeResponse as NativeDeviceCodeResponse,
};

pub const PROVIDER: &str = "codex_oauth";

pub fn map_account(
    provider: &str,
    account: NativeAccount,
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

pub fn map_device_code_response(
    provider: &str,
    response: NativeDeviceCodeResponse,
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
