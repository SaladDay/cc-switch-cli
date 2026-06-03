use crate::{app_config::AppType, provider::Provider};

use super::super::providers::get_claude_api_format;

const CLAUDE_MESSAGES_ENDPOINT: &str = "/v1/messages";
const OPENAI_CHAT_COMPLETIONS_ENDPOINT: &str = "/v1/chat/completions";
const OPENAI_RESPONSES_ENDPOINT: &str = "/v1/responses";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClaudeEndpointTarget {
    Messages,
    ChatCompletions,
    Responses,
}

impl ClaudeEndpointTarget {
    fn for_provider(provider: &Provider) -> Self {
        match get_claude_api_format(provider) {
            "anthropic" => Self::Messages,
            "openai_chat" => Self::ChatCompletions,
            "openai_responses" => Self::Responses,
            "gemini_native" => Self::Messages,
            format => unreachable!("unsupported Claude API format for endpoint rewrite: {format}"),
        }
    }

    fn rewrite(self, endpoint: &str) -> &str {
        if endpoint != CLAUDE_MESSAGES_ENDPOINT {
            return endpoint;
        }

        match self {
            Self::Messages => endpoint,
            Self::ChatCompletions => OPENAI_CHAT_COMPLETIONS_ENDPOINT,
            Self::Responses => OPENAI_RESPONSES_ENDPOINT,
        }
    }
}

pub(super) fn rewrite_upstream_endpoint(
    app_type: &AppType,
    provider: &Provider,
    endpoint: &str,
) -> String {
    if !matches!(app_type, AppType::Claude) {
        return endpoint.to_string();
    }

    ClaudeEndpointTarget::for_provider(provider)
        .rewrite(endpoint)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::rewrite_upstream_endpoint;
    use crate::{app_config::AppType, provider::Provider};
    use serde_json::json;

    #[test]
    fn rewrite_upstream_endpoint_maps_openai_chat_messages() {
        let provider = Provider::with_id(
            "chat".to_string(),
            "Chat Provider".to_string(),
            json!({ "api_format": "openai_chat" }),
            None,
        );

        assert_eq!(
            rewrite_upstream_endpoint(&AppType::Claude, &provider, "/v1/messages"),
            "/v1/chat/completions"
        );
    }

    #[test]
    fn rewrite_upstream_endpoint_maps_openai_responses_messages() {
        let provider = Provider::with_id(
            "responses".to_string(),
            "Responses Provider".to_string(),
            json!({ "api_format": "openai_responses" }),
            None,
        );

        assert_eq!(
            rewrite_upstream_endpoint(&AppType::Claude, &provider, "/v1/messages"),
            "/v1/responses"
        );
    }

    #[test]
    fn rewrite_upstream_endpoint_leaves_non_rewritten_requests_unchanged() {
        let provider = Provider::with_id(
            "anthropic".to_string(),
            "Anthropic Provider".to_string(),
            json!({}),
            None,
        );
        let openai_provider = Provider::with_id(
            "chat".to_string(),
            "Chat Provider".to_string(),
            json!({ "api_format": "openai_chat" }),
            None,
        );

        assert_eq!(
            rewrite_upstream_endpoint(&AppType::Claude, &provider, "/v1/messages"),
            "/v1/messages"
        );
        assert_eq!(
            rewrite_upstream_endpoint(&AppType::Codex, &openai_provider, "/v1/messages"),
            "/v1/messages"
        );
        assert_eq!(
            rewrite_upstream_endpoint(&AppType::Claude, &openai_provider, "/v1/models"),
            "/v1/models"
        );
    }
}
