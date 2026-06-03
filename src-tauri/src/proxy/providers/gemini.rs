use reqwest::RequestBuilder;

use crate::{provider::Provider, proxy::error::ProxyError};

use super::{AuthInfo, AuthStrategy, ProviderAdapter, ProviderType};

pub struct GeminiAdapter;

#[derive(Debug, Clone)]
pub struct OAuthCredentials {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
}

impl OAuthCredentials {
    #[allow(dead_code)]
    pub fn needs_refresh(&self) -> bool {
        self.refresh_token.is_some() && self.access_token.is_empty()
    }

    #[allow(dead_code)]
    pub fn can_refresh(&self) -> bool {
        self.refresh_token.is_some() && self.client_id.is_some() && self.client_secret.is_some()
    }
}

impl GeminiAdapter {
    pub fn new() -> Self {
        Self
    }

    pub fn provider_type(&self, provider: &Provider) -> ProviderType {
        if let Some(key) = self.extract_key_raw(provider) {
            if key.starts_with("ya29.") || key.starts_with('{') {
                return ProviderType::GeminiCli;
            }
        }

        ProviderType::Gemini
    }

    pub fn detect_auth_type(&self, provider: &Provider) -> AuthStrategy {
        match self.provider_type(provider) {
            ProviderType::GeminiCli => AuthStrategy::GoogleOAuth,
            _ => AuthStrategy::Google,
        }
    }

    fn extract_key_raw(&self, provider: &Provider) -> Option<String> {
        if let Some(env) = provider.settings_config.get("env") {
            if let Some(key) = env
                .get("GEMINI_API_KEY")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                return Some(key.to_string());
            }
        }

        provider
            .settings_config
            .get("apiKey")
            .or_else(|| provider.settings_config.get("api_key"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    }

    pub fn parse_oauth_credentials(&self, key: &str) -> Option<OAuthCredentials> {
        let key = key.trim();

        if key.starts_with("ya29.") {
            return Some(OAuthCredentials {
                access_token: key.to_string(),
                refresh_token: None,
                client_id: None,
                client_secret: None,
            });
        }

        if !key.starts_with('{') {
            return None;
        }

        let json = serde_json::from_str::<serde_json::Value>(key).ok()?;
        let access_token = json
            .get("access_token")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let refresh_token = json
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let client_id = json
            .get("client_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let client_secret = json
            .get("client_secret")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        if access_token.is_empty() && refresh_token.is_none() {
            return None;
        }

        Some(OAuthCredentials {
            access_token,
            refresh_token,
            client_id,
            client_secret,
        })
    }
}

impl Default for GeminiAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderAdapter for GeminiAdapter {
    fn name(&self) -> &'static str {
        "Gemini"
    }

    fn extract_base_url(&self, provider: &Provider) -> Result<String, ProxyError> {
        if let Some(env) = provider.settings_config.get("env") {
            if let Some(url) = env.get("GOOGLE_GEMINI_BASE_URL").and_then(|v| v.as_str()) {
                return Ok(url.trim_end_matches('/').to_string());
            }
            if let Some(url) = env.get("GEMINI_BASE_URL").and_then(|v| v.as_str()) {
                return Ok(url.trim_end_matches('/').to_string());
            }
            if let Some(url) = env.get("BASE_URL").and_then(|v| v.as_str()) {
                return Ok(url.trim_end_matches('/').to_string());
            }
        }

        if let Some(url) = provider
            .settings_config
            .get("base_url")
            .and_then(|v| v.as_str())
        {
            return Ok(url.trim_end_matches('/').to_string());
        }

        if let Some(url) = provider
            .settings_config
            .get("baseURL")
            .and_then(|v| v.as_str())
        {
            return Ok(url.trim_end_matches('/').to_string());
        }

        Err(ProxyError::ConfigError(
            "Gemini Provider 缺少 base_url 配置".to_string(),
        ))
    }

    fn extract_auth(&self, provider: &Provider) -> Option<AuthInfo> {
        let key = self.extract_key_raw(provider)?;
        match self.detect_auth_type(provider) {
            AuthStrategy::GoogleOAuth => {
                if let Some(creds) = self.parse_oauth_credentials(&key) {
                    if !creds.access_token.is_empty() {
                        return Some(AuthInfo::with_access_token(key, creds.access_token));
                    }
                    return Some(AuthInfo::new(key, AuthStrategy::GoogleOAuth));
                }
                Some(AuthInfo::new(key, AuthStrategy::GoogleOAuth))
            }
            _ => Some(AuthInfo::new(key, AuthStrategy::Google)),
        }
    }

    fn build_url(&self, base_url: &str, endpoint: &str) -> String {
        let base_trimmed = base_url.trim_end_matches('/');
        let endpoint_trimmed = endpoint.trim_start_matches('/');
        let mut url = format!("{base_trimmed}/{endpoint_trimmed}");

        for pattern in ["/v1beta", "/v1"] {
            let duplicate = format!("{pattern}{pattern}");
            if url.contains(&duplicate) {
                url = url.replace(&duplicate, pattern);
            }
        }

        url
    }

    fn add_auth_headers(&self, request: RequestBuilder, auth: &AuthInfo) -> RequestBuilder {
        match auth.strategy {
            AuthStrategy::GoogleOAuth => {
                let token = auth.access_token.as_ref().unwrap_or(&auth.api_key);
                request
                    .header("Authorization", format!("Bearer {token}"))
                    .header("x-goog-api-client", "GeminiCLI/1.0")
            }
            _ => request.header("x-goog-api-key", &auth.api_key),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn create_provider(config: serde_json::Value) -> Provider {
        Provider::with_id(
            "gemini-test".to_string(),
            "Gemini Test".to_string(),
            config,
            None,
        )
    }

    #[test]
    fn oauth_access_token_is_trimmed_and_classified() {
        let adapter = GeminiAdapter::new();
        let provider = create_provider(json!({
            "env": {
                "GEMINI_API_KEY": "\nya29.raw-token-value\n"
            }
        }));

        assert_eq!(adapter.provider_type(&provider), ProviderType::GeminiCli);

        let auth = adapter.extract_auth(&provider).expect("gemini oauth auth");
        assert_eq!(auth.strategy, AuthStrategy::GoogleOAuth);
        assert_eq!(auth.api_key, "ya29.raw-token-value");
        assert_eq!(auth.access_token.as_deref(), Some("ya29.raw-token-value"));
    }

    #[test]
    fn oauth_refresh_only_json_does_not_expose_empty_bearer() {
        let adapter = GeminiAdapter::new();
        let provider = create_provider(json!({
            "env": {
                "GEMINI_API_KEY": r#"{"refresh_token":"rt-abc","client_id":"cid","client_secret":"cs"}"#
            }
        }));

        assert_eq!(adapter.provider_type(&provider), ProviderType::GeminiCli);

        let auth = adapter
            .extract_auth(&provider)
            .expect("refresh-only oauth auth");
        assert_eq!(auth.strategy, AuthStrategy::GoogleOAuth);
        assert_eq!(auth.access_token, None);
    }

    #[test]
    fn oauth_empty_access_token_json_does_not_expose_empty_bearer() {
        let adapter = GeminiAdapter::new();
        let provider = create_provider(json!({
            "env": {
                "GEMINI_API_KEY": r#"{"access_token":"","refresh_token":"rt-abc","client_id":"cid","client_secret":"cs"}"#
            }
        }));

        assert_eq!(adapter.provider_type(&provider), ProviderType::GeminiCli);

        let auth = adapter.extract_auth(&provider).expect("expired oauth auth");
        assert_eq!(auth.strategy, AuthStrategy::GoogleOAuth);
        assert_eq!(auth.access_token, None);
    }

    #[test]
    fn oauth_json_with_leading_whitespace_keeps_access_token() {
        let adapter = GeminiAdapter::new();
        let provider = create_provider(json!({
            "env": {
                "GEMINI_API_KEY": "\n  {\"access_token\":\"ya29.valid\",\"refresh_token\":\"rt\"}\n"
            }
        }));

        assert_eq!(adapter.provider_type(&provider), ProviderType::GeminiCli);

        let auth = adapter.extract_auth(&provider).expect("json oauth auth");
        assert_eq!(auth.strategy, AuthStrategy::GoogleOAuth);
        assert_eq!(auth.access_token.as_deref(), Some("ya29.valid"));
    }
}
