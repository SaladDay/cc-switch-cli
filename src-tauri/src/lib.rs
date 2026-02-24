// Core modules
mod app_config;
mod claude_mcp;
mod claude_plugin;
mod codex_config;
mod config;
mod database;
mod deeplink;
mod error;
mod gemini_config;
mod gemini_mcp;
mod import_export;
mod init_status;
mod mcp;
mod prompt;
mod prompt_files;
mod provider;
mod provider_defaults;
mod proxy;
mod services;
mod settings;
mod store;
mod sync_policy;
mod usage_script;

// CLI module
pub mod cli;

// Public exports
pub use app_config::{AppType, McpApps, McpServer, MultiAppConfig};
pub use codex_config::{get_codex_auth_path, get_codex_config_path, write_codex_live_atomic};
pub use config::{get_claude_mcp_path, get_claude_settings_path, read_json_file};
pub use database::{Database, FailoverQueueItem};
pub use deeplink::{import_provider_from_deeplink, parse_deeplink_url, DeepLinkImportRequest};
pub use error::AppError;
pub use import_export::export_config_to_file;
pub use mcp::{
    import_from_claude, import_from_codex, import_from_gemini, remove_server_from_claude,
    remove_server_from_codex, remove_server_from_gemini, sync_enabled_to_claude,
    sync_enabled_to_codex, sync_enabled_to_gemini, sync_single_server_to_claude,
    sync_single_server_to_codex, sync_single_server_to_gemini,
};
pub use provider::{Provider, ProviderMeta};
pub use services::{
    ConfigService, EndpointLatency, McpService, MemoryService, PromptService, ProviderService,
    SkillService, SpeedtestService, SyncDecision, WebDavSyncService, WebDavSyncSummary,
};
pub use settings::{
    get_skip_claude_onboarding, get_webdav_sync_settings, set_skip_claude_onboarding,
    set_webdav_sync_settings, update_settings, webdav_jianguoyun_preset, AppSettings,
    WebDavSyncSettings, WebDavSyncStatus,
};
pub use store::AppState;
