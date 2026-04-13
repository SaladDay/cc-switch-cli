mod adapter;
mod auth;
mod claude;
mod codex;
pub mod codex_oauth_auth;
mod gemini;
pub mod streaming;
pub mod streaming_responses;
pub mod transform;
pub mod transform_responses;

use crate::app_config::AppType;
use crate::provider::Provider;
use serde::{Deserialize, Serialize};

pub use adapter::ProviderAdapter;
pub use auth::{AuthInfo, AuthStrategy};
#[allow(unused_imports)]
pub use claude::{
    claude_api_format_needs_transform, get_claude_api_format,
    transform_claude_request_for_api_format, ClaudeAdapter,
};
pub use codex::CodexAdapter;
pub use gemini::GeminiAdapter;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    Claude,
    ClaudeAuth,
    Codex,
    Gemini,
    GeminiCli,
    OpenRouter,
    GitHubCopilot,
    CodexOAuth,
}

impl ProviderType {
    #[allow(dead_code)]
    pub fn needs_transform(&self) -> bool {
        match self {
            ProviderType::GitHubCopilot | ProviderType::CodexOAuth => true,
            ProviderType::OpenRouter => false,
            _ => false,
        }
    }

    #[allow(dead_code)]
    pub fn default_endpoint(&self) -> &'static str {
        match self {
            ProviderType::Claude | ProviderType::ClaudeAuth => "https://api.anthropic.com",
            ProviderType::Codex => "https://api.openai.com",
            ProviderType::Gemini | ProviderType::GeminiCli => {
                "https://generativelanguage.googleapis.com"
            }
            ProviderType::OpenRouter => "https://openrouter.ai/api",
            ProviderType::GitHubCopilot => "https://api.githubcopilot.com",
            ProviderType::CodexOAuth => "https://chatgpt.com/backend-api/codex",
        }
    }

    #[allow(dead_code)]
    pub fn from_app_type_and_config(app_type: &AppType, provider: &Provider) -> Self {
        match app_type {
            AppType::Claude => {
                if let Some(meta) = provider.meta.as_ref() {
                    if meta.provider_type.as_deref() == Some("github_copilot") {
                        return ProviderType::GitHubCopilot;
                    }
                    if meta.provider_type.as_deref() == Some("codex_oauth") {
                        return ProviderType::CodexOAuth;
                    }
                }

                let adapter = ClaudeAdapter::new();
                if let Ok(base_url) = adapter.extract_base_url(provider) {
                    if base_url.contains("githubcopilot.com") {
                        return ProviderType::GitHubCopilot;
                    }
                    if base_url.contains("openrouter.ai") {
                        return ProviderType::OpenRouter;
                    }
                }

                if let Some(auth_mode) = provider
                    .settings_config
                    .get("auth_mode")
                    .and_then(|value| value.as_str())
                {
                    if auth_mode == "bearer_only" {
                        return ProviderType::ClaudeAuth;
                    }
                }

                if let Some(env) = provider.settings_config.get("env") {
                    if let Some(auth_mode) = env.get("AUTH_MODE").and_then(|value| value.as_str()) {
                        if auth_mode == "bearer_only" {
                            return ProviderType::ClaudeAuth;
                        }
                    }
                }

                ProviderType::Claude
            }
            AppType::Codex => ProviderType::Codex,
            AppType::Gemini => {
                let adapter = GeminiAdapter::new();
                if let Some(auth) = adapter.extract_auth(provider) {
                    let key = &auth.api_key;
                    if key.starts_with("ya29.") || key.starts_with('{') {
                        return ProviderType::GeminiCli;
                    }
                }
                ProviderType::Gemini
            }
            AppType::OpenCode | AppType::OpenClaw => ProviderType::Codex,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderType::Claude => "claude",
            ProviderType::ClaudeAuth => "claude_auth",
            ProviderType::Codex => "codex",
            ProviderType::Gemini => "gemini",
            ProviderType::GeminiCli => "gemini_cli",
            ProviderType::OpenRouter => "openrouter",
            ProviderType::GitHubCopilot => "github_copilot",
            ProviderType::CodexOAuth => "codex_oauth",
        }
    }
}

impl std::fmt::Display for ProviderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for ProviderType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "claude" => Ok(ProviderType::Claude),
            "claude_auth" | "claude-auth" => Ok(ProviderType::ClaudeAuth),
            "codex" => Ok(ProviderType::Codex),
            "gemini" => Ok(ProviderType::Gemini),
            "gemini_cli" | "gemini-cli" => Ok(ProviderType::GeminiCli),
            "openrouter" => Ok(ProviderType::OpenRouter),
            "github_copilot" | "github-copilot" | "githubcopilot" => {
                Ok(ProviderType::GitHubCopilot)
            }
            "codex_oauth" | "codex-oauth" | "codexoauth" => Ok(ProviderType::CodexOAuth),
            _ => Err(format!("Invalid provider type: {s}")),
        }
    }
}

pub fn get_adapter(app_type: &AppType) -> Box<dyn ProviderAdapter> {
    match app_type {
        AppType::Claude => Box::new(ClaudeAdapter::new()),
        AppType::Codex => Box::new(CodexAdapter::new()),
        AppType::Gemini => Box::new(GeminiAdapter::new()),
        AppType::OpenCode => Box::new(CodexAdapter::new()),
        AppType::OpenClaw => Box::new(CodexAdapter::new()),
    }
}

#[allow(dead_code)]
pub fn get_adapter_for_provider_type(provider_type: &ProviderType) -> Box<dyn ProviderAdapter> {
    match provider_type {
        ProviderType::Claude
        | ProviderType::ClaudeAuth
        | ProviderType::OpenRouter
        | ProviderType::GitHubCopilot
        | ProviderType::CodexOAuth => Box::new(ClaudeAdapter::new()),
        ProviderType::Codex => Box::new(CodexAdapter::new()),
        ProviderType::Gemini | ProviderType::GeminiCli => Box::new(GeminiAdapter::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn create_provider(config: serde_json::Value) -> Provider {
        Provider {
            id: "test".to_string(),
            name: "Test Provider".to_string(),
            settings_config: config,
            website_url: None,
            category: None,
            created_at: None,
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        }
    }

    #[test]
    fn provider_type_from_app_type_detects_codex_oauth_metadata() {
        let provider: Provider = serde_json::from_value(json!({
            "id": "codex-oauth",
            "name": "Codex OAuth",
            "settingsConfig": {
                "env": {
                    "ANTHROPIC_BASE_URL": "https://relay.example.com",
                    "ANTHROPIC_AUTH_TOKEN": "token"
                }
            },
            "meta": {
                "providerType": "codex_oauth"
            }
        }))
        .expect("provider should deserialize");

        assert_eq!(
            ProviderType::from_app_type_and_config(&AppType::Claude, &provider),
            ProviderType::CodexOAuth
        );
    }

    #[test]
    fn provider_type_from_app_type_detects_gemini_cli_oauth() {
        let provider = create_provider(json!({
            "env": {
                "GEMINI_API_KEY": "ya29.test-access-token"
            }
        }));

        assert_eq!(
            ProviderType::from_app_type_and_config(&AppType::Gemini, &provider),
            ProviderType::GeminiCli
        );
    }
}
