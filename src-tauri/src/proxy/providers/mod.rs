mod adapter;
mod auth;
mod claude;
mod codex;
mod gemini;
pub mod streaming;
pub mod streaming_responses;
pub mod transform;
pub mod transform_responses;

use crate::app_config::AppType;

pub use adapter::ProviderAdapter;
pub use auth::{AuthInfo, AuthStrategy};
pub use claude::{get_claude_api_format, ClaudeAdapter};
pub use codex::CodexAdapter;
pub use gemini::GeminiAdapter;

pub fn get_adapter(app_type: &AppType) -> Box<dyn ProviderAdapter> {
    match app_type {
        AppType::Claude => Box::new(ClaudeAdapter::new()),
        AppType::Codex => Box::new(CodexAdapter::new()),
        AppType::Gemini => Box::new(GeminiAdapter::new()),
        AppType::OpenCode => Box::new(CodexAdapter::new()),
        AppType::OpenClaw => Box::new(CodexAdapter::new()),
    }
}
