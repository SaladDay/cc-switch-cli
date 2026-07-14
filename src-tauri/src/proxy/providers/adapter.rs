use reqwest::RequestBuilder;
use serde_json::Value;

use crate::provider::Provider;
use crate::proxy::error::ProxyError;

use super::auth::AuthInfo;

pub trait ProviderAdapter: Send + Sync {
    #[allow(dead_code)]
    fn name(&self) -> &'static str;
    fn extract_base_url(&self, provider: &Provider) -> Result<String, ProxyError>;
    fn extract_auth(&self, provider: &Provider) -> Option<AuthInfo>;
    fn build_url(&self, base_url: &str, endpoint: &str) -> String;
    fn add_auth_headers(&self, request: RequestBuilder, auth: &AuthInfo) -> RequestBuilder;

    fn needs_transform(&self, _provider: &Provider) -> bool {
        false
    }

    fn transform_request(&self, body: Value, _provider: &Provider) -> Result<Value, ProxyError> {
        Ok(body)
    }

    fn transform_response(&self, body: Value) -> Result<Value, ProxyError> {
        Ok(body)
    }
}
