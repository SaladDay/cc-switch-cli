use axum::http::HeaderMap;
use serde_json::Value;

use crate::services::session_identity::ProxySessionIdEncoding;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionIdSource {
    Header,
    MetadataUserId,
    MetadataSessionId,
    Generated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionIdResult {
    pub session_id: String,
    pub source: SessionIdSource,
    pub client_provided: bool,
}

pub fn extract_session_id(
    headers: &HeaderMap,
    body: &Value,
    client_format: &str,
) -> SessionIdResult {
    extract_session_id_with_encoding(headers, body, client_format).0
}

pub(crate) fn extract_session_id_with_encoding(
    headers: &HeaderMap,
    body: &Value,
    client_format: &str,
) -> (SessionIdResult, ProxySessionIdEncoding) {
    if client_format == "claude" {
        if let Some(result) = extract_claude_session(headers, body) {
            return result;
        }
    }

    if matches!(client_format, "codex" | "openai") {
        if let Some(result) = extract_codex_session(headers, body) {
            return result;
        }
    }

    if let Some(result) = extract_from_metadata(body) {
        return result;
    }

    (
        SessionIdResult {
            session_id: uuid::Uuid::new_v4().to_string(),
            source: SessionIdSource::Generated,
            client_provided: false,
        },
        ProxySessionIdEncoding::Native,
    )
}

fn extract_claude_session(
    headers: &HeaderMap,
    body: &Value,
) -> Option<(SessionIdResult, ProxySessionIdEncoding)> {
    for header_name in ["x-claude-code-session-id", "claude-code-session-id"] {
        if let Some(session_id) = headers.get(header_name).and_then(|v| v.to_str().ok()) {
            if !session_id.is_empty() {
                return Some((
                    SessionIdResult {
                        session_id: session_id.to_string(),
                        source: SessionIdSource::Header,
                        client_provided: true,
                    },
                    ProxySessionIdEncoding::Native,
                ));
            }
        }
    }

    extract_from_metadata(body)
}

fn extract_codex_session(
    headers: &HeaderMap,
    body: &Value,
) -> Option<(SessionIdResult, ProxySessionIdEncoding)> {
    for header_name in ["session_id", "x-session-id"] {
        if let Some(session_id) = headers.get(header_name).and_then(|v| v.to_str().ok()) {
            if session_id.len() > 20 {
                return Some((
                    SessionIdResult {
                        session_id: format!("codex_{session_id}"),
                        source: SessionIdSource::Header,
                        client_provided: true,
                    },
                    ProxySessionIdEncoding::CodexTransport,
                ));
            }
        }
    }

    if let Some(session_id) = body
        .get("metadata")
        .and_then(|metadata| metadata.get("session_id"))
        .and_then(|v| v.as_str())
    {
        if session_id.len() > 10 {
            return Some((
                SessionIdResult {
                    session_id: format!("codex_{session_id}"),
                    source: SessionIdSource::MetadataSessionId,
                    client_provided: true,
                },
                ProxySessionIdEncoding::CodexTransport,
            ));
        }
    }

    None
}

fn extract_from_metadata(body: &Value) -> Option<(SessionIdResult, ProxySessionIdEncoding)> {
    if let Some(user_id) = body
        .get("metadata")
        .and_then(|metadata| metadata.get("user_id"))
        .and_then(|v| v.as_str())
    {
        if let Some(session_id) = parse_session_from_user_id(user_id) {
            return Some((
                SessionIdResult {
                    session_id,
                    source: SessionIdSource::MetadataUserId,
                    client_provided: true,
                },
                ProxySessionIdEncoding::Native,
            ));
        }
    }

    if let Some(session_id) = body
        .get("metadata")
        .and_then(|metadata| metadata.get("session_id"))
        .and_then(|v| v.as_str())
    {
        if !session_id.is_empty() {
            return Some((
                SessionIdResult {
                    session_id: session_id.to_string(),
                    source: SessionIdSource::MetadataSessionId,
                    client_provided: true,
                },
                ProxySessionIdEncoding::Native,
            ));
        }
    }

    None
}

pub(crate) fn parse_session_from_user_id(user_id: &str) -> Option<String> {
    user_id
        .split_once("_session_")
        .and_then(|(_, session_id)| (!session_id.is_empty()).then(|| session_id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_claude_metadata_session_id() {
        let headers = HeaderMap::new();
        let body = json!({
            "metadata": {
                "session_id": "claude-session-123"
            }
        });

        let result = extract_session_id(&headers, &body, "claude");
        assert_eq!(result.session_id, "claude-session-123");
        assert_eq!(result.source, SessionIdSource::MetadataSessionId);
        assert!(result.client_provided);
    }

    #[test]
    fn extracts_claude_header_session_id() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-claude-code-session-id",
            "claude-header-session".parse().unwrap(),
        );
        let body = json!({});

        let (result, encoding) = extract_session_id_with_encoding(&headers, &body, "claude");
        assert_eq!(result.session_id, "claude-header-session");
        assert_eq!(result.source, SessionIdSource::Header);
        assert!(result.client_provided);
        assert_eq!(encoding, ProxySessionIdEncoding::Native);
    }

    #[test]
    fn preserves_codex_native_prefix_from_metadata_user_id() {
        let headers = HeaderMap::new();
        let body = json!({
            "metadata": {
                "user_id": "user_session_codex_thread-a"
            }
        });

        let (result, encoding) = extract_session_id_with_encoding(&headers, &body, "codex");
        assert_eq!(result.session_id, "codex_thread-a");
        assert_eq!(result.source, SessionIdSource::MetadataUserId);
        assert!(result.client_provided);
        assert_eq!(encoding, ProxySessionIdEncoding::Native);
    }

    #[test]
    fn tracks_codex_metadata_session_id_encoding_across_length_threshold() {
        let headers = HeaderMap::new();
        let short_body = json!({
            "metadata": {
                "session_id": "codex_x"
            }
        });
        let long_body = json!({
            "metadata": {
                "session_id": "codex_thread-a"
            }
        });

        let (short, short_encoding) =
            extract_session_id_with_encoding(&headers, &short_body, "codex");
        let (long, long_encoding) = extract_session_id_with_encoding(&headers, &long_body, "codex");

        assert_eq!(short.source, SessionIdSource::MetadataSessionId);
        assert_eq!(short.session_id, "codex_x");
        assert_eq!(short_encoding, ProxySessionIdEncoding::Native);
        assert_eq!(long.source, SessionIdSource::MetadataSessionId);
        assert_eq!(long.session_id, "codex_codex_thread-a");
        assert_eq!(long_encoding, ProxySessionIdEncoding::CodexTransport);
    }

    #[test]
    fn extracts_codex_header_session_id() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "session_id",
            "00000000-0000-4000-8000-000000000000".parse().unwrap(),
        );
        let body = json!({});

        let (result, encoding) = extract_session_id_with_encoding(&headers, &body, "codex");
        assert_eq!(
            result.session_id,
            "codex_00000000-0000-4000-8000-000000000000"
        );
        assert_eq!(result.source, SessionIdSource::Header);
        assert!(result.client_provided);
        assert_eq!(encoding, ProxySessionIdEncoding::CodexTransport);
    }

    #[test]
    fn codex_previous_response_id_is_not_session_identity() {
        let headers = HeaderMap::new();
        let body = json!({
            "previous_response_id": "resp_abc123"
        });

        let (result, encoding) = extract_session_id_with_encoding(&headers, &body, "codex");
        assert!(!result.session_id.starts_with("codex_resp_abc123"));
        assert_eq!(result.source, SessionIdSource::Generated);
        assert!(!result.client_provided);
        assert_eq!(encoding, ProxySessionIdEncoding::Native);
    }
}
