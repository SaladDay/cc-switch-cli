//! Claude Code OAuth authorization-code flow.
//!
//! This module deliberately keeps token exchange native-side. Callers receive
//! an authorization URL and may pass the final callback URL back to
//! `complete_login`; access and refresh tokens are never part of the public
//! response types.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};
use tokio::sync::Mutex;
use uuid::Uuid;

const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const DEFAULT_REDIRECT_URI: &str = "http://localhost:54545/callback";
const SCOPE: &str =
    "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";

pub const PROVIDER: &str = "claude_oauth";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaudeOAuthStart {
    pub provider: String,
    pub authorization_url: String,
    pub state: String,
    pub redirect_uri: String,
    pub expires_in: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaudeOAuthAccount {
    pub id: String,
    pub provider: String,
    pub email: Option<String>,
    pub organization_id: Option<String>,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingLogin {
    state: String,
    verifier: String,
    redirect_uri: String,
    expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredAccount {
    id: String,
    email: Option<String>,
    organization_id: Option<String>,
    access_token: String,
    refresh_token: String,
    expires_at: i64,
}

pub struct ClaudeOAuthService {
    config_dir: PathBuf,
    pending: Mutex<Option<PendingLogin>>,
}

static SERVICE: OnceLock<RwLock<Option<(PathBuf, Arc<ClaudeOAuthService>)>>> = OnceLock::new();

pub(crate) fn manager(config_dir: PathBuf) -> Arc<ClaudeOAuthService> {
    let store = SERVICE.get_or_init(|| RwLock::new(None));
    if let Some((path, service)) = store.read().expect("read Claude OAuth service").as_ref() {
        if path == &config_dir {
            return Arc::clone(service);
        }
    }
    let service = Arc::new(ClaudeOAuthService::new(config_dir.clone()));
    *store.write().expect("write Claude OAuth service") = Some((config_dir, Arc::clone(&service)));
    service
}

impl ClaudeOAuthService {
    pub fn new(config_dir: PathBuf) -> Self {
        Self {
            config_dir,
            pending: Mutex::new(None),
        }
    }

    pub async fn start_login(
        &self,
        redirect_uri: Option<&str>,
    ) -> Result<ClaudeOAuthStart, String> {
        let redirect_uri = redirect_uri.unwrap_or(DEFAULT_REDIRECT_URI).trim();
        let parsed_redirect = url::Url::parse(redirect_uri).ok();
        if redirect_uri.is_empty()
            || parsed_redirect.as_ref().map(|url| url.scheme()) != Some("http")
            || parsed_redirect.as_ref().and_then(|url| url.host_str()) != Some("localhost")
            || parsed_redirect
                .as_ref()
                .and_then(|url| url.port())
                .is_none()
            || parsed_redirect.as_ref().map(|url| url.path()) != Some("/callback")
        {
            return Err("redirect_uri must be a localhost HTTP callback".to_string());
        }
        let verifier = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let state = Uuid::new_v4().to_string();
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("code", "true");
        query.append_pair("client_id", CLIENT_ID);
        query.append_pair("response_type", "code");
        query.append_pair("redirect_uri", redirect_uri);
        query.append_pair("scope", SCOPE);
        query.append_pair("code_challenge", &challenge);
        query.append_pair("code_challenge_method", "S256");
        query.append_pair("state", &state);
        *self.pending.lock().await = Some(PendingLogin {
            state: state.clone(),
            verifier,
            redirect_uri: redirect_uri.to_string(),
            expires_at: chrono::Utc::now().timestamp() + 300,
        });
        Ok(ClaudeOAuthStart {
            provider: "claude_oauth".to_string(),
            authorization_url: format!("{AUTHORIZE_URL}?{}", query.finish()),
            state,
            redirect_uri: redirect_uri.to_string(),
            expires_in: 300,
        })
    }

    /// Validates a callback URL and exchanges its code. Tokens are persisted
    /// below the configured native store and are never returned to callers.
    pub async fn complete_login(&self, callback_url: &str) -> Result<ClaudeOAuthAccount, String> {
        let callback =
            url::Url::parse(callback_url).map_err(|e| format!("invalid callback URL: {e}"))?;
        let pending = self
            .pending
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or("no pending Claude OAuth login")?;
        let expected = url::Url::parse(&pending.redirect_uri)
            .map_err(|e| format!("invalid pending redirect URI: {e}"))?;
        if callback.scheme() != expected.scheme()
            || callback.host_str() != expected.host_str()
            || callback.port_or_known_default() != expected.port_or_known_default()
            || callback.path() != expected.path()
        {
            return Err(
                "Claude OAuth callback does not match the pending localhost redirect".into(),
            );
        }
        let error = callback
            .query_pairs()
            .find(|(k, _)| k == "error")
            .map(|(_, v)| v.into_owned());
        if let Some(error) = error {
            return Err(format!("Claude OAuth authorization failed: {error}"));
        }
        let code = callback
            .query_pairs()
            .find(|(k, _)| k == "code")
            .map(|(_, v)| v.into_owned());
        let state = callback
            .query_pairs()
            .find(|(k, _)| k == "state")
            .map(|(_, v)| v.into_owned());
        if pending.expires_at < chrono::Utc::now().timestamp() {
            return Err("Claude OAuth callback expired".into());
        }
        if state.as_deref() != Some(pending.state.as_str()) {
            return Err("Claude OAuth state mismatch".into());
        }
        let code = code.ok_or("Claude OAuth callback did not contain a code")?;
        // Consume the state only after all callback-local checks pass. This
        // allows a malformed browser paste to be corrected without issuing a
        // second authorization request, while still making a valid callback
        // single-use before token exchange.
        let pending = self
            .pending
            .lock()
            .await
            .take()
            .ok_or("no pending Claude OAuth login")?;
        let body = serde_json::json!({"grant_type":"authorization_code","code":code,"redirect_uri":pending.redirect_uri,"client_id":CLIENT_ID,"code_verifier":pending.verifier,"state":pending.state});
        let response = reqwest::Client::new()
            .post(TOKEN_URL)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Claude OAuth token exchange failed: {e}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "Claude OAuth token exchange failed with status {}",
                response.status()
            ));
        }
        let token: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("invalid Claude OAuth token response: {e}"))?;
        let access_token = token
            .get("access_token")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .ok_or("Claude OAuth response missing access_token")?;
        let refresh_token = token
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if refresh_token.is_empty() {
            return Err("Claude OAuth response missing refresh_token".into());
        }
        let account = StoredAccount {
            id: Uuid::new_v4().to_string(),
            email: None,
            organization_id: None,
            access_token: access_token.to_string(),
            refresh_token: refresh_token.to_string(),
            expires_at: chrono::Utc::now().timestamp()
                + token
                    .get("expires_in")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(3600),
        };
        std::fs::create_dir_all(&self.config_dir).map_err(|e| format!("create auth store: {e}"))?;
        let bytes = serde_json::to_vec_pretty(&account).map_err(|e| e.to_string())?;
        crate::config::atomic_write(&self.config_dir.join("claude-oauth.json"), &bytes)
            .map_err(|e| e.to_string())?;
        Ok(ClaudeOAuthAccount {
            id: account.id,
            provider: "claude_oauth".into(),
            email: account.email,
            organization_id: account.organization_id,
            is_default: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn start_login_builds_pkce_authorization_request() {
        let service = ClaudeOAuthService::new(tempdir().unwrap().path().to_path_buf());
        let start = service.start_login(None).await.unwrap();
        let url = url::Url::parse(&start.authorization_url).unwrap();
        assert_eq!(url.host_str(), Some("claude.ai"));
        let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query.get("client_id"), Some(&CLIENT_ID.to_string()));
        assert_eq!(
            query.get("redirect_uri"),
            Some(&DEFAULT_REDIRECT_URI.to_string())
        );
        assert_eq!(
            query.get("code_challenge_method"),
            Some(&"S256".to_string())
        );
        assert_eq!(query.get("state"), Some(&start.state));
        assert!(!query
            .get("code_challenge")
            .unwrap_or(&String::new())
            .is_empty());
    }

    #[tokio::test]
    async fn callback_rejects_wrong_origin_and_preserves_pending_state() {
        let service = ClaudeOAuthService::new(tempdir().unwrap().path().to_path_buf());
        let start = service.start_login(None).await.unwrap();
        let bad = format!(
            "http://localhost:54546/callback?code=x&state={}",
            start.state
        );
        let error = service.complete_login(&bad).await.unwrap_err();
        assert!(error.contains("does not match"));
        assert!(service.pending.lock().await.is_some());
    }

    #[tokio::test]
    async fn callback_rejects_provider_error_without_exchange() {
        let service = ClaudeOAuthService::new(tempdir().unwrap().path().to_path_buf());
        let start = service.start_login(None).await.unwrap();
        let callback = format!(
            "{}?error=access_denied&state={}",
            start.redirect_uri, start.state
        );
        let error = service.complete_login(&callback).await.unwrap_err();
        assert!(error.contains("authorization failed"));
        assert!(service.pending.lock().await.is_some());
    }
}
