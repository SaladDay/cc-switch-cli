use super::{AuthInfo, AuthStrategy, CodexAdapter, ProviderAdapter, XAI_API_BASE_URL};
use crate::{provider::Provider, proxy::error::ProxyError};
use reqwest::RequestBuilder;

/// Custom profiles use the existing Responses adapter. The native Official
/// entry has no stored API key/profile: routing it uses the managed xAI account.
pub struct GrokBuildAdapter;

pub(super) fn is_official(provider: &Provider) -> bool {
    provider.category.as_deref() == Some("official")
}

impl ProviderAdapter for GrokBuildAdapter {
    fn name(&self) -> &'static str {
        "Grok Build"
    }
    fn extract_base_url(&self, provider: &Provider) -> Result<String, ProxyError> {
        if is_official(provider) {
            return Ok(XAI_API_BASE_URL.into());
        }
        CodexAdapter::new().extract_base_url(provider)
    }
    fn extract_auth(&self, provider: &Provider) -> Option<AuthInfo> {
        if is_official(provider) {
            return Some(AuthInfo::new(
                "xai_oauth_placeholder".into(),
                AuthStrategy::XaiOAuth,
            ));
        }
        CodexAdapter::new().extract_auth(provider)
    }
    fn build_url(&self, base: &str, endpoint: &str) -> String {
        CodexAdapter::new().build_url(base, endpoint)
    }
    fn add_auth_headers(&self, request: RequestBuilder, auth: &AuthInfo) -> RequestBuilder {
        CodexAdapter::new().add_auth_headers(request, auth)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn official_grok_routes_to_pinned_xai_with_managed_auth_without_native_credentials() {
        let mut provider = Provider::with_id(
            "grokbuild-official".into(),
            "Grok Official".into(),
            serde_json::json!({"config":""}),
            None,
        );
        provider.category = Some("official".into());
        assert_eq!(
            GrokBuildAdapter.extract_base_url(&provider).unwrap(),
            XAI_API_BASE_URL
        );
        assert_eq!(
            GrokBuildAdapter.extract_auth(&provider).unwrap().strategy,
            AuthStrategy::XaiOAuth
        );
    }
}
