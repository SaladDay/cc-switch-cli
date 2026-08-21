//! Claude Code OAuth authorization-code flow.
//!
//! This module deliberately keeps token exchange native-side. Callers receive
//! an authorization URL and may pass the final callback URL back to
//! `complete_login`; access and refresh tokens are never part of the public
//! response types.

use super::{AuthCompletionResponse, BrowserAuthStart};
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
    credential_path: PathBuf,
    token_url: String,
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
        Self::with_token_url(config_dir, TOKEN_URL)
    }

    fn with_token_url(config_dir: PathBuf, token_url: impl Into<String>) -> Self {
        Self {
            credential_path: config_dir.join("credentials").join("claude.json"),
            token_url: token_url.into(),
            pending: Mutex::new(None),
        }
    }

    pub async fn start_login(
        &self,
        redirect_uri: Option<&str>,
    ) -> Result<BrowserAuthStart, String> {
        let redirect_uri = redirect_uri.unwrap_or(DEFAULT_REDIRECT_URI).trim();
        if redirect_uri != DEFAULT_REDIRECT_URI {
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
        Ok(BrowserAuthStart {
            provider: "claude_oauth".to_string(),
            authorization_url: format!("{AUTHORIZE_URL}?{}", query.finish()),
            state,
            redirect_uri: redirect_uri.to_string(),
            expires_in: 300,
        })
    }

    /// Validates a callback URL and exchanges its code. Tokens are persisted
    /// below the configured native store and are never returned to callers.
    pub async fn complete_login(
        &self,
        callback_url: &str,
    ) -> Result<AuthCompletionResponse, String> {
        let callback =
            url::Url::parse(callback_url).map_err(|e| format!("invalid callback URL: {e}"))?;
        let mut pending_guard = self.pending.lock().await;
        let pending = pending_guard
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
        // Consume the exact record while holding the same guard used for all
        // callback checks. A concurrent start cannot replace A with B between
        // validation and take, and malformed callbacks remain retryable.
        let pending = pending_guard
            .take()
            .ok_or("no pending Claude OAuth login")?;
        drop(pending_guard);
        let body = serde_json::json!({"grant_type":"authorization_code","code":code,"redirect_uri":pending.redirect_uri,"client_id":CLIENT_ID,"code_verifier":pending.verifier,"state":pending.state});
        let response = reqwest::Client::new()
            .post(&self.token_url)
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
        let email = token
            .get("email")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);
        let organization_id = token
            .get("organization_id")
            .or_else(|| token.get("organizationId"))
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);
        let stable_id = token
            .get("account_id")
            .or_else(|| token.get("user_id"))
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned)
            .or_else(|| email.clone())
            .or_else(|| organization_id.clone());
        let account = StoredAccount {
            id: stable_id.unwrap_or_else(|| Uuid::new_v4().to_string()),
            email,
            organization_id,
            access_token: access_token.to_string(),
            refresh_token: refresh_token.to_string(),
            expires_at: chrono::Utc::now().timestamp()
                + token
                    .get("expires_in")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(3600),
        };
        let path = &self.credential_path;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create Claude config dir: {e}"))?;
        }
        let lock_path = path.with_extension("credentials.lock");
        let _lock = CredentialLock::acquire(&lock_path)?;
        let mut credentials = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes)
                .map_err(|e| format!("invalid existing Claude credentials: {e}"))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
            Err(error) => return Err(format!("read Claude credentials: {error}")),
        };
        let oauth = credentials
            .as_object_mut()
            .ok_or("Claude credentials root must be an object")?
            .entry("claudeAiOauth")
            .or_insert_with(|| serde_json::json!({}));
        let oauth = oauth
            .as_object_mut()
            .ok_or("claudeAiOauth must be an object")?;
        oauth.insert(
            "accessToken".into(),
            serde_json::json!(account.access_token),
        );
        oauth.insert(
            "refreshToken".into(),
            serde_json::json!(account.refresh_token),
        );
        oauth.insert(
            "expiresAt".into(),
            serde_json::json!(account.expires_at * 1000),
        );
        let write_result = write_secure_json_unlocked(path, &credentials);
        write_result?;
        Ok(AuthCompletionResponse {
            account_id: account.id,
            provider: "claude_oauth".into(),
            login: account.email,
            organization_id: account.organization_id,
            is_default: true,
        })
    }
}

struct CredentialLock(PathBuf);

impl CredentialLock {
    fn acquire(path: &PathBuf) -> Result<Self, String> {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| format!("Claude credentials busy or unavailable: {e}"))?;
        Ok(Self(path.clone()))
    }
}

impl Drop for CredentialLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn write_secure_json_unlocked(path: &PathBuf, value: &serde_json::Value) -> Result<(), String> {
    let result = (|| {
        let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("credentials.json.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        use std::io::Write;
        let mut file = options.open(&tmp).map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    })();
    let _ = std::fs::remove_file(path.with_extension("credentials.json.tmp"));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::State, routing::post, Json, Router};
    use tempfile::tempdir;

    #[derive(Clone)]
    struct TokenServerState {
        request: Arc<Mutex<Option<serde_json::Value>>>,
    }

    async fn token_handler(
        State(state): State<TokenServerState>,
        Json(body): Json<serde_json::Value>,
    ) -> Json<serde_json::Value> {
        *state.request.lock().await = Some(body);
        Json(serde_json::json!({
            "access_token": "access-secret",
            "refresh_token": "refresh-secret",
            "expires_in": 3600
        }))
    }

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

    #[tokio::test]
    async fn successful_exchange_writes_claude_native_credentials_without_public_secrets() {
        let request = Arc::new(Mutex::new(None));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/token", post(token_handler))
            .with_state(TokenServerState {
                request: Arc::clone(&request),
            });
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let config = tempdir().unwrap();
        let service = ClaudeOAuthService::with_token_url(
            config.path().to_path_buf(),
            format!("http://{address}/token"),
        );
        let start = service.start_login(None).await.unwrap();
        let callback = format!(
            "{}?code=authorization-code&state={}",
            start.redirect_uri, start.state
        );
        let public = service.complete_login(&callback).await.unwrap();

        let body = request.lock().await.clone().unwrap();
        assert_eq!(body["grant_type"], "authorization_code");
        assert_eq!(body["code"], "authorization-code");
        assert_eq!(body["redirect_uri"], DEFAULT_REDIRECT_URI);
        assert_eq!(body["client_id"], CLIENT_ID);
        assert!(body["code_verifier"].as_str().unwrap().len() >= 43);

        let credentials: serde_json::Value = serde_json::from_slice(
            &std::fs::read(config.path().join("credentials/claude.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(credentials["claudeAiOauth"]["accessToken"], "access-secret");
        assert_eq!(
            credentials["claudeAiOauth"]["refreshToken"],
            "refresh-secret"
        );

        let public_json = serde_json::to_value(public).unwrap().to_string();
        assert!(!public_json.contains("access-secret"));
        assert!(!public_json.contains("refresh-secret"));
        assert!(service.pending.lock().await.is_none());
    }
}
