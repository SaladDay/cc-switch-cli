//! Deep link import functionality for CC Switch (CLI edition).
//!
//! Implements the `ccswitch://v1/import?...` protocol for importing resources.
//! Supports importing providers, MCP servers, prompts, and skill repositories.

mod mcp;
mod parser;
mod prompt;
mod provider;
mod skill;
mod utils;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use mcp::{import_mcp_from_deeplink, McpImportResult};
pub use parser::parse_deeplink_url;
pub use prompt::import_prompt_from_deeplink;
pub use provider::import_provider_from_deeplink;
pub use skill::import_skill_from_deeplink;

/// Deep link import request model.
///
/// This mirrors the upstream request model to keep URL parsing compatible.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeepLinkImportRequest {
    pub version: String,
    pub resource: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub haiku_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sonnet_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opus_model: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub apps: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_url: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_script: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_access_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_auto_interval: Option<u64>,

    #[serde(skip)]
    pub(crate) openclaw_config: Option<Value>,
}
