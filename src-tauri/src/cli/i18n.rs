use crate::settings::{get_settings, update_settings};
use std::sync::OnceLock;
use std::sync::RwLock;

/// Supported languages
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    English,
    Chinese,
}

impl Language {
    pub fn code(&self) -> &'static str {
        match self {
            Language::English => "en",
            Language::Chinese => "zh",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Language::English => "English",
            Language::Chinese => "中文",
        }
    }

    pub fn from_code(code: &str) -> Self {
        match code.to_lowercase().as_str() {
            "zh" | "zh-cn" | "zh-tw" | "chinese" => Language::Chinese,
            _ => Language::English,
        }
    }
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

/// Global language state
fn language_store() -> &'static RwLock<Language> {
    static STORE: OnceLock<RwLock<Language>> = OnceLock::new();
    STORE.get_or_init(|| {
        let settings = get_settings();
        let lang = settings
            .language
            .as_deref()
            .map(Language::from_code)
            .unwrap_or(Language::English);
        RwLock::new(lang)
    })
}

/// Get current language
pub fn current_language() -> Language {
    *language_store().read().expect("Failed to read language")
}

/// Set current language and persist
pub fn set_language(lang: Language) -> Result<(), crate::error::AppError> {
    // Update runtime state
    {
        let mut guard = language_store().write().expect("Failed to write language");
        *guard = lang;
    }

    // Persist to settings
    let mut settings = get_settings();
    settings.language = Some(lang.code().to_string());
    update_settings(settings)
}

/// Check if current language is Chinese
pub fn is_chinese() -> bool {
    current_language() == Language::Chinese
}

// ============================================================================
// Localized Text Macros and Functions
// ============================================================================

/// Get localized text based on current language
#[macro_export]
macro_rules! t {
    ($en:expr, $zh:expr) => {
        if $crate::cli::i18n::is_chinese() {
            $zh
        } else {
            $en
        }
    };
}

// Re-export for convenience
pub use t;

// ============================================================================
// Common UI Texts
// ============================================================================

pub mod texts {
    use super::is_chinese;

    // ============================================
    // ENTITY TYPE CONSTANTS (实体类型常量)
    // ============================================

    pub fn entity_provider() -> &'static str {
        if is_chinese() {
            "供应商"
        } else {
            "provider"
        }
    }

    pub fn entity_server() -> &'static str {
        if is_chinese() {
            "服务器"
        } else {
            "server"
        }
    }

    pub fn entity_prompt() -> &'static str {
        if is_chinese() {
            "提示词"
        } else {
            "prompt"
        }
    }

    // ============================================
    // GENERIC ENTITY OPERATIONS (通用实体操作)
    // ============================================

    pub fn entity_added_success(entity_type: &str, name: &str) -> String {
        if is_chinese() {
            format!("✓ 成功添加{} '{}'", entity_type, name)
        } else {
            format!("✓ Successfully added {} '{}'", entity_type, name)
        }
    }

    pub fn entity_updated_success(entity_type: &str, name: &str) -> String {
        if is_chinese() {
            format!("✓ 成功更新{} '{}'", entity_type, name)
        } else {
            format!("✓ Successfully updated {} '{}'", entity_type, name)
        }
    }

    pub fn entity_deleted_success(entity_type: &str, name: &str) -> String {
        if is_chinese() {
            format!("✓ 成功删除{} '{}'", entity_type, name)
        } else {
            format!("✓ Successfully deleted {} '{}'", entity_type, name)
        }
    }

    pub fn entity_not_found(entity_type: &str, id: &str) -> String {
        if is_chinese() {
            format!("{}不存在: {}", entity_type, id)
        } else {
            format!("{} not found: {}", entity_type, id)
        }
    }

    pub fn confirm_create_entity(entity_type: &str) -> String {
        if is_chinese() {
            format!("\n确认创建此{}？", entity_type)
        } else {
            format!("\nConfirm create this {}?", entity_type)
        }
    }

    pub fn confirm_update_entity(entity_type: &str) -> String {
        if is_chinese() {
            format!("\n确认更新此{}？", entity_type)
        } else {
            format!("\nConfirm update this {}?", entity_type)
        }
    }

    pub fn confirm_delete_entity(entity_type: &str, name: &str) -> String {
        if is_chinese() {
            format!("\n确认删除{} '{}'？", entity_type, name)
        } else {
            format!("\nConfirm delete {} '{}'?", entity_type, name)
        }
    }

    pub fn select_to_delete_entity(entity_type: &str) -> String {
        if is_chinese() {
            format!("选择要删除的{}：", entity_type)
        } else {
            format!("Select {} to delete:", entity_type)
        }
    }

    pub fn no_entities_to_delete(entity_type: &str) -> String {
        if is_chinese() {
            format!("没有可删除的{}", entity_type)
        } else {
            format!("No {} available for deletion", entity_type)
        }
    }

    // ============================================
    // COMMON UI ELEMENTS (通用界面元素)
    // ============================================

    // Welcome & Headers
    pub fn welcome_title() -> &'static str {
        if is_chinese() {
            "    🎯 CC-Switch 交互模式"
        } else {
            "    🎯 CC-Switch Interactive Mode"
        }
    }

    pub fn application() -> &'static str {
        if is_chinese() {
            "应用程序"
        } else {
            "Application"
        }
    }

    pub fn goodbye() -> &'static str {
        if is_chinese() {
            "👋 再见！"
        } else {
            "👋 Goodbye!"
        }
    }

    // Main Menu
    pub fn main_menu_prompt(app: &str) -> String {
        if is_chinese() {
            format!("请选择操作 (当前: {})", app)
        } else {
            format!("What would you like to do? (Current: {})", app)
        }
    }

    pub fn main_menu_help() -> &'static str {
        if is_chinese() {
            "↑↓ 选择，←→ 切换应用，/ 搜索，Enter 确认，Esc 清除/退出"
        } else {
            "↑↓ to move, ←→ to switch app, / to search, Enter to select, Esc to clear/exit"
        }
    }

    pub fn main_menu_search_prompt() -> &'static str {
        if is_chinese() {
            "输入搜索关键字（空或 Esc 清除过滤）："
        } else {
            "Enter search keyword (empty/Esc to clear):"
        }
    }

    pub fn main_menu_filtering(query: &str) -> String {
        if is_chinese() {
            format!("🔎 搜索: {}", query)
        } else {
            format!("🔎 Search: {}", query)
        }
    }

    pub fn main_menu_no_matches() -> &'static str {
        if is_chinese() {
            "没有匹配的菜单项"
        } else {
            "No matching menu items"
        }
    }

    pub fn menu_manage_providers() -> &'static str {
        if is_chinese() {
            "🔌 管理供应商"
        } else {
            "🔌 Manage Providers"
        }
    }

    pub fn menu_manage_mcp() -> &'static str {
        if is_chinese() {
            "🛠️  管理 MCP 服务器"
        } else {
            "🛠️  Manage MCP Servers"
        }
    }

    pub fn menu_manage_prompts() -> &'static str {
        if is_chinese() {
            "💬 管理提示词"
        } else {
            "💬 Manage Prompts"
        }
    }

    pub fn menu_manage_config() -> &'static str {
        if is_chinese() {
            "⚙️  配置文件管理"
        } else {
            "⚙️  Manage Configuration"
        }
    }

    pub fn menu_view_config() -> &'static str {
        if is_chinese() {
            "👁️  查看当前配置"
        } else {
            "👁️  View Current Configuration"
        }
    }

    pub fn menu_switch_app() -> &'static str {
        if is_chinese() {
            "🔄 切换应用"
        } else {
            "🔄 Switch Application"
        }
    }

    pub fn menu_settings() -> &'static str {
        if is_chinese() {
            "⚙️  设置"
        } else {
            "⚙️  Settings"
        }
    }

    pub fn menu_exit() -> &'static str {
        if is_chinese() {
            "🚪 退出"
        } else {
            "🚪 Exit"
        }
    }

    // ============================================
    // PROVIDER MANAGEMENT (供应商管理)
    // ============================================

    pub fn provider_management() -> &'static str {
        if is_chinese() {
            "🔌 供应商管理"
        } else {
            "🔌 Provider Management"
        }
    }

    pub fn no_providers() -> &'static str {
        if is_chinese() {
            "未找到供应商。"
        } else {
            "No providers found."
        }
    }

    pub fn view_current_provider() -> &'static str {
        if is_chinese() {
            "📋 查看当前供应商详情"
        } else {
            "📋 View Current Provider Details"
        }
    }

    pub fn switch_provider() -> &'static str {
        if is_chinese() {
            "🔄 切换供应商"
        } else {
            "🔄 Switch Provider"
        }
    }

    pub fn add_provider() -> &'static str {
        if is_chinese() {
            "➕ 新增供应商"
        } else {
            "➕ Add Provider"
        }
    }

    pub fn add_official_provider() -> &'static str {
        if is_chinese() {
            "添加官方供应商"
        } else {
            "Add Official Provider"
        }
    }

    pub fn add_third_party_provider() -> &'static str {
        if is_chinese() {
            "添加第三方供应商"
        } else {
            "Add Third-Party Provider"
        }
    }

    pub fn select_provider_add_mode() -> &'static str {
        if is_chinese() {
            "请选择供应商类型："
        } else {
            "Select provider type:"
        }
    }

    pub fn delete_provider() -> &'static str {
        if is_chinese() {
            "🗑️  删除供应商"
        } else {
            "🗑️  Delete Provider"
        }
    }

    pub fn back_to_main() -> &'static str {
        if is_chinese() {
            "⬅️  返回主菜单"
        } else {
            "⬅️  Back to Main Menu"
        }
    }

    pub fn choose_action() -> &'static str {
        if is_chinese() {
            "选择操作："
        } else {
            "Choose an action:"
        }
    }

    pub fn esc_to_go_back_help() -> &'static str {
        if is_chinese() {
            "Esc 返回上一步"
        } else {
            "Esc to go back"
        }
    }

    pub fn select_filter_help() -> &'static str {
        if is_chinese() {
            "Esc 返回；输入可过滤"
        } else {
            "Esc to go back; type to filter"
        }
    }

    pub fn current_provider_details() -> &'static str {
        if is_chinese() {
            "当前供应商详情"
        } else {
            "Current Provider Details"
        }
    }

    pub fn only_one_provider() -> &'static str {
        if is_chinese() {
            "只有一个供应商，无法切换。"
        } else {
            "Only one provider available. Cannot switch."
        }
    }

    pub fn no_other_providers() -> &'static str {
        if is_chinese() {
            "没有其他供应商可切换。"
        } else {
            "No other providers to switch to."
        }
    }

    pub fn select_provider_to_switch() -> &'static str {
        if is_chinese() {
            "选择要切换到的供应商："
        } else {
            "Select provider to switch to:"
        }
    }

    pub fn switched_to_provider(id: &str) -> String {
        if is_chinese() {
            format!("✓ 已切换到供应商 '{}'", id)
        } else {
            format!("✓ Switched to provider '{}'", id)
        }
    }

    pub fn restart_note() -> &'static str {
        if is_chinese() {
            "注意：请重启 CLI 客户端以应用更改。"
        } else {
            "Note: Restart your CLI client to apply the changes."
        }
    }

    pub fn no_deletable_providers() -> &'static str {
        if is_chinese() {
            "没有可删除的供应商（无法删除当前供应商）。"
        } else {
            "No providers available for deletion (cannot delete current provider)."
        }
    }

    pub fn select_provider_to_delete() -> &'static str {
        if is_chinese() {
            "选择要删除的供应商："
        } else {
            "Select provider to delete:"
        }
    }

    pub fn confirm_delete(id: &str) -> String {
        if is_chinese() {
            format!("确定要删除供应商 '{}' 吗？", id)
        } else {
            format!("Are you sure you want to delete provider '{}'?", id)
        }
    }

    pub fn cancelled() -> &'static str {
        if is_chinese() {
            "已取消。"
        } else {
            "Cancelled."
        }
    }

    pub fn deleted_provider(id: &str) -> String {
        if is_chinese() {
            format!("✓ 已删除供应商 '{}'", id)
        } else {
            format!("✓ Deleted provider '{}'", id)
        }
    }

    // Provider Input - Basic Fields
    pub fn provider_name_label() -> &'static str {
        if is_chinese() {
            "供应商名称："
        } else {
            "Provider Name:"
        }
    }

    pub fn provider_name_help() -> &'static str {
        if is_chinese() {
            "必填，用于显示的友好名称"
        } else {
            "Required, friendly display name"
        }
    }

    pub fn provider_name_help_edit() -> &'static str {
        if is_chinese() {
            "必填，直接回车保持原值"
        } else {
            "Required, press Enter to keep"
        }
    }

    pub fn provider_name_placeholder() -> &'static str {
        "OpenAI"
    }

    pub fn provider_name_empty_error() -> &'static str {
        if is_chinese() {
            "供应商名称不能为空"
        } else {
            "Provider name cannot be empty"
        }
    }

    pub fn website_url_label() -> &'static str {
        if is_chinese() {
            "官网 URL（可选）："
        } else {
            "Website URL (optional):"
        }
    }

    pub fn website_url_help() -> &'static str {
        if is_chinese() {
            "供应商的网站地址，直接回车跳过"
        } else {
            "Provider's website, press Enter to skip"
        }
    }

    pub fn website_url_help_edit() -> &'static str {
        if is_chinese() {
            "留空则不修改，直接回车跳过"
        } else {
            "Leave blank to keep, Enter to skip"
        }
    }

    pub fn website_url_placeholder() -> &'static str {
        "https://openai.com"
    }

    // Provider Commands
    pub fn no_providers_hint() -> &'static str {
        "Use 'cc-switch provider add' to create a new provider."
    }

    pub fn app_config_not_found(app: &str) -> String {
        if is_chinese() {
            format!("应用 {} 配置不存在", app)
        } else {
            format!("Application {} configuration not found", app)
        }
    }

    pub fn provider_not_found(id: &str) -> String {
        if is_chinese() {
            format!("供应商不存在: {}", id)
        } else {
            format!("Provider not found: {}", id)
        }
    }

    pub fn generated_id(id: &str) -> String {
        if is_chinese() {
            format!("生成的 ID: {}", id)
        } else {
            format!("Generated ID: {}", id)
        }
    }

    pub fn configure_optional_fields_prompt() -> &'static str {
        if is_chinese() {
            "配置可选字段（备注、排序索引）？"
        } else {
            "Configure optional fields (notes, sort index)?"
        }
    }

    pub fn current_config_header() -> &'static str {
        if is_chinese() {
            "当前配置："
        } else {
            "Current Configuration:"
        }
    }

    pub fn modify_provider_config_prompt() -> &'static str {
        if is_chinese() {
            "修改供应商配置（API Key, Base URL 等）？"
        } else {
            "Modify provider configuration (API Key, Base URL, etc.)?"
        }
    }

    pub fn modify_optional_fields_prompt() -> &'static str {
        if is_chinese() {
            "修改可选字段（备注、排序索引）？"
        } else {
            "Modify optional fields (notes, sort index)?"
        }
    }

    pub fn current_provider_synced_warning() -> &'static str {
        if is_chinese() {
            "⚠ 此供应商当前已激活，修改已同步到 live 配置"
        } else {
            "⚠ This provider is currently active, changes synced to live config"
        }
    }

    pub fn input_failed_error(err: &str) -> String {
        if is_chinese() {
            format!("输入失败: {}", err)
        } else {
            format!("Input failed: {}", err)
        }
    }

    pub fn cannot_delete_current_provider() -> &'static str {
        "Cannot delete the current active provider. Please switch to another provider first."
    }

    // Provider Input - Basic Fields
    pub fn provider_name_prompt() -> &'static str {
        if is_chinese() {
            "供应商名称："
        } else {
            "Provider Name:"
        }
    }

    // Provider Input - Claude Configuration
    pub fn config_claude_header() -> &'static str {
        if is_chinese() {
            "配置 Claude 供应商："
        } else {
            "Configure Claude Provider:"
        }
    }

    pub fn api_key_label() -> &'static str {
        if is_chinese() {
            "API Key："
        } else {
            "API Key:"
        }
    }

    pub fn api_key_help() -> &'static str {
        if is_chinese() {
            "留空使用默认值"
        } else {
            "Leave empty to use default"
        }
    }

    pub fn base_url_label() -> &'static str {
        if is_chinese() {
            "Base URL："
        } else {
            "Base URL:"
        }
    }

    pub fn base_url_placeholder() -> &'static str {
        if is_chinese() {
            "如 https://api.anthropic.com"
        } else {
            "e.g., https://api.anthropic.com"
        }
    }

    pub fn configure_model_names_prompt() -> &'static str {
        if is_chinese() {
            "配置模型名称？"
        } else {
            "Configure model names?"
        }
    }

    pub fn model_default_label() -> &'static str {
        if is_chinese() {
            "默认模型："
        } else {
            "Default Model:"
        }
    }

    pub fn model_default_help() -> &'static str {
        if is_chinese() {
            "留空使用 Claude Code 默认模型"
        } else {
            "Leave empty to use Claude Code default"
        }
    }

    pub fn model_haiku_label() -> &'static str {
        if is_chinese() {
            "Haiku 模型："
        } else {
            "Haiku Model:"
        }
    }

    pub fn model_haiku_placeholder() -> &'static str {
        if is_chinese() {
            "如 claude-3-5-haiku-20241022"
        } else {
            "e.g., claude-3-5-haiku-20241022"
        }
    }

    pub fn model_sonnet_label() -> &'static str {
        if is_chinese() {
            "Sonnet 模型："
        } else {
            "Sonnet Model:"
        }
    }

    pub fn model_sonnet_placeholder() -> &'static str {
        if is_chinese() {
            "如 claude-3-5-sonnet-20241022"
        } else {
            "e.g., claude-3-5-sonnet-20241022"
        }
    }

    pub fn model_opus_label() -> &'static str {
        if is_chinese() {
            "Opus 模型："
        } else {
            "Opus Model:"
        }
    }

    pub fn model_opus_placeholder() -> &'static str {
        if is_chinese() {
            "如 claude-3-opus-20240229"
        } else {
            "e.g., claude-3-opus-20240229"
        }
    }

    // Provider Input - Codex Configuration
    pub fn config_codex_header() -> &'static str {
        if is_chinese() {
            "配置 Codex 供应商："
        } else {
            "Configure Codex Provider:"
        }
    }

    pub fn openai_api_key_label() -> &'static str {
        if is_chinese() {
            "OpenAI API Key："
        } else {
            "OpenAI API Key:"
        }
    }

    pub fn anthropic_api_key_label() -> &'static str {
        if is_chinese() {
            "Anthropic API Key："
        } else {
            "Anthropic API Key:"
        }
    }

    pub fn config_toml_label() -> &'static str {
        if is_chinese() {
            "配置内容 (TOML)："
        } else {
            "Config Content (TOML):"
        }
    }

    pub fn config_toml_help() -> &'static str {
        if is_chinese() {
            "按 Esc 后 Enter 提交"
        } else {
            "Press Esc then Enter to submit"
        }
    }

    pub fn config_toml_placeholder() -> &'static str {
        if is_chinese() {
            "留空使用默认配置"
        } else {
            "Leave empty to use default config"
        }
    }

    // Codex 0.64+ Configuration
    pub fn codex_auth_mode_info() -> &'static str {
        if is_chinese() {
            "⚠ 请选择 Codex 的鉴权方式（决定 API Key 从哪里读取）"
        } else {
            "⚠ Choose how Codex authenticates (where the API key is read from)"
        }
    }

    pub fn codex_auth_mode_label() -> &'static str {
        if is_chinese() {
            "认证方式："
        } else {
            "Auth Mode:"
        }
    }

    pub fn codex_auth_mode_help() -> &'static str {
        if is_chinese() {
            "OpenAI 认证：使用 auth.json/凭据存储；环境变量：使用 env_key 指定的变量（未设置会报错）"
        } else {
            "OpenAI auth uses auth.json/credential store; env var mode uses env_key (missing env var will error)"
        }
    }

    pub fn codex_auth_mode_openai() -> &'static str {
        if is_chinese() {
            "OpenAI 认证（推荐，无需环境变量）"
        } else {
            "OpenAI auth (recommended, no env var)"
        }
    }

    pub fn codex_auth_mode_env_var() -> &'static str {
        if is_chinese() {
            "环境变量（env_key，需要手动 export）"
        } else {
            "Environment variable (env_key, requires export)"
        }
    }

    pub fn codex_official_provider_tip() -> &'static str {
        if is_chinese() {
            "提示：官方供应商将使用 Codex 官方登录保存的凭证（codex login 可能会打开浏览器），无需填写 API Key"
        } else {
            "Tip: Official provider uses Codex login credentials (`codex login` may open a browser); no API key required"
        }
    }

    pub fn codex_env_key_info() -> &'static str {
        if is_chinese() {
            "⚠ 环境变量模式：Codex 将从指定的环境变量读取 API Key"
        } else {
            "⚠ Env var mode: Codex will read the API key from the specified environment variable"
        }
    }

    pub fn codex_env_key_label() -> &'static str {
        if is_chinese() {
            "环境变量名称："
        } else {
            "Environment Variable Name:"
        }
    }

    pub fn codex_env_key_help() -> &'static str {
        if is_chinese() {
            "Codex 将从此环境变量读取 API 密钥（默认: OPENAI_API_KEY）"
        } else {
            "Codex will read API key from this env var (default: OPENAI_API_KEY)"
        }
    }

    pub fn codex_wire_api_label() -> &'static str {
        if is_chinese() {
            "API 格式："
        } else {
            "API Format:"
        }
    }

    pub fn codex_wire_api_help() -> &'static str {
        if is_chinese() {
            "chat = Chat Completions API (大多数第三方), responses = OpenAI Responses API"
        } else {
            "chat = Chat Completions API (most providers), responses = OpenAI Responses API"
        }
    }

    pub fn codex_env_reminder(env_key: &str) -> String {
        if is_chinese() {
            format!(
                "⚠ 请确保已设置环境变量 {} 并包含您的 API 密钥\n  例如: export {}=\"your-api-key\"",
                env_key, env_key
            )
        } else {
            format!(
                "⚠ Make sure to set the {} environment variable with your API key\n  Example: export {}=\"your-api-key\"",
                env_key, env_key
            )
        }
    }

    pub fn codex_openai_auth_info() -> &'static str {
        if is_chinese() {
            "✓ OpenAI 认证模式：Codex 将使用 auth.json/系统凭据存储，无需设置 OPENAI_API_KEY 环境变量"
        } else {
            "✓ OpenAI auth mode: Codex will use auth.json/credential store; no OPENAI_API_KEY env var required"
        }
    }

    pub fn codex_dual_write_info(env_key: &str, _api_key: &str) -> String {
        if is_chinese() {
            format!(
                "✓ 双写模式已启用（兼容所有 Codex 版本）\n\
                  • 旧版本 Codex: 将使用 auth.json 中的 API Key\n\
                  • Codex 0.64+: 可使用环境变量 {} (更安全)\n\
                    例如: export {}=\"your-api-key\"",
                env_key, env_key
            )
        } else {
            format!(
                "✓ Dual-write mode enabled (compatible with all Codex versions)\n\
                  • Legacy Codex: Will use API Key from auth.json\n\
                  • Codex 0.64+: Can use env variable {} (more secure)\n\
                    Example: export {}=\"your-api-key\"",
                env_key, env_key
            )
        }
    }

    pub fn use_current_config_prompt() -> &'static str {
        if is_chinese() {
            "使用当前配置？"
        } else {
            "Use current configuration?"
        }
    }

    pub fn use_current_config_help() -> &'static str {
        if is_chinese() {
            "选择 No 将进入自定义输入模式"
        } else {
            "Select No to enter custom input mode"
        }
    }

    pub fn input_toml_config() -> &'static str {
        if is_chinese() {
            "输入 TOML 配置（多行，输入空行结束）："
        } else {
            "Enter TOML config (multiple lines, empty line to finish):"
        }
    }

    pub fn direct_enter_to_finish() -> &'static str {
        if is_chinese() {
            "直接回车结束输入"
        } else {
            "Press Enter to finish"
        }
    }

    pub fn current_config_label() -> &'static str {
        if is_chinese() {
            "当前配置："
        } else {
            "Current Config:"
        }
    }

    pub fn config_toml_header() -> &'static str {
        if is_chinese() {
            "Config.toml 配置："
        } else {
            "Config.toml Configuration:"
        }
    }

    // Provider Input - Gemini Configuration
    pub fn config_gemini_header() -> &'static str {
        if is_chinese() {
            "配置 Gemini 供应商："
        } else {
            "Configure Gemini Provider:"
        }
    }

    pub fn auth_type_label() -> &'static str {
        if is_chinese() {
            "认证类型："
        } else {
            "Auth Type:"
        }
    }

    pub fn auth_type_api_key() -> &'static str {
        if is_chinese() {
            "API Key"
        } else {
            "API Key"
        }
    }

    pub fn auth_type_service_account() -> &'static str {
        if is_chinese() {
            "Service Account (ADC)"
        } else {
            "Service Account (ADC)"
        }
    }

    pub fn gemini_api_key_label() -> &'static str {
        if is_chinese() {
            "Gemini API Key："
        } else {
            "Gemini API Key:"
        }
    }

    pub fn gemini_base_url_label() -> &'static str {
        if is_chinese() {
            "Base URL："
        } else {
            "Base URL:"
        }
    }

    pub fn gemini_base_url_help() -> &'static str {
        if is_chinese() {
            "留空使用官方 API"
        } else {
            "Leave empty to use official API"
        }
    }

    pub fn gemini_base_url_placeholder() -> &'static str {
        if is_chinese() {
            "如 https://generativelanguage.googleapis.com"
        } else {
            "e.g., https://generativelanguage.googleapis.com"
        }
    }

    pub fn adc_project_id_label() -> &'static str {
        if is_chinese() {
            "GCP Project ID："
        } else {
            "GCP Project ID:"
        }
    }

    pub fn adc_location_label() -> &'static str {
        if is_chinese() {
            "GCP Location："
        } else {
            "GCP Location:"
        }
    }

    pub fn adc_location_placeholder() -> &'static str {
        if is_chinese() {
            "如 us-central1"
        } else {
            "e.g., us-central1"
        }
    }

    pub fn google_oauth_official() -> &'static str {
        if is_chinese() {
            "Google OAuth（官方）"
        } else {
            "Google OAuth (Official)"
        }
    }

    pub fn packycode_api_key() -> &'static str {
        if is_chinese() {
            "PackyCode API Key"
        } else {
            "PackyCode API Key"
        }
    }

    pub fn generic_api_key() -> &'static str {
        if is_chinese() {
            "通用 API Key"
        } else {
            "Generic API Key"
        }
    }

    pub fn select_auth_method_help() -> &'static str {
        if is_chinese() {
            "选择 Gemini 的认证方式"
        } else {
            "Select authentication method for Gemini"
        }
    }

    pub fn use_google_oauth_warning() -> &'static str {
        if is_chinese() {
            "使用 Google OAuth，将清空 API Key 配置"
        } else {
            "Using Google OAuth, API Key config will be cleared"
        }
    }

    pub fn packycode_api_key_help() -> &'static str {
        if is_chinese() {
            "从 PackyCode 获取的 API Key"
        } else {
            "API Key obtained from PackyCode"
        }
    }

    pub fn packycode_endpoint_help() -> &'static str {
        if is_chinese() {
            "PackyCode API 端点"
        } else {
            "PackyCode API endpoint"
        }
    }

    pub fn generic_api_key_help() -> &'static str {
        if is_chinese() {
            "通用的 Gemini API Key"
        } else {
            "Generic Gemini API Key"
        }
    }

    // Provider Input - Optional Fields
    pub fn notes_label() -> &'static str {
        if is_chinese() {
            "备注："
        } else {
            "Notes:"
        }
    }

    pub fn notes_placeholder() -> &'static str {
        if is_chinese() {
            "可选的备注信息"
        } else {
            "Optional notes"
        }
    }

    pub fn sort_index_label() -> &'static str {
        if is_chinese() {
            "排序索引："
        } else {
            "Sort Index:"
        }
    }

    pub fn sort_index_help() -> &'static str {
        if is_chinese() {
            "数字越小越靠前，留空使用创建时间排序"
        } else {
            "Lower numbers appear first, leave empty to sort by creation time"
        }
    }

    pub fn sort_index_placeholder() -> &'static str {
        if is_chinese() {
            "如 1, 2, 3..."
        } else {
            "e.g., 1, 2, 3..."
        }
    }

    pub fn invalid_sort_index() -> &'static str {
        if is_chinese() {
            "排序索引必须是有效的数字"
        } else {
            "Sort index must be a valid number"
        }
    }

    pub fn optional_fields_config() -> &'static str {
        if is_chinese() {
            "可选字段配置："
        } else {
            "Optional Fields Configuration:"
        }
    }

    pub fn notes_example_placeholder() -> &'static str {
        if is_chinese() {
            "自定义供应商，用于测试"
        } else {
            "Custom provider for testing"
        }
    }

    pub fn notes_help_edit() -> &'static str {
        if is_chinese() {
            "关于此供应商的额外说明，直接回车保持原值"
        } else {
            "Additional notes about this provider, press Enter to keep current value"
        }
    }

    pub fn notes_help_new() -> &'static str {
        if is_chinese() {
            "关于此供应商的额外说明，直接回车跳过"
        } else {
            "Additional notes about this provider, press Enter to skip"
        }
    }

    pub fn sort_index_help_edit() -> &'static str {
        if is_chinese() {
            "数字，用于控制显示顺序，直接回车保持原值"
        } else {
            "Number for display order, press Enter to keep current value"
        }
    }

    pub fn sort_index_help_new() -> &'static str {
        if is_chinese() {
            "数字，用于控制显示顺序，直接回车跳过"
        } else {
            "Number for display order, press Enter to skip"
        }
    }

    pub fn invalid_sort_index_number() -> &'static str {
        if is_chinese() {
            "排序索引必须是数字"
        } else {
            "Sort index must be a number"
        }
    }

    pub fn provider_config_summary() -> &'static str {
        if is_chinese() {
            "=== 供应商配置摘要 ==="
        } else {
            "=== Provider Configuration Summary ==="
        }
    }

    pub fn id_label() -> &'static str {
        if is_chinese() {
            "ID"
        } else {
            "ID"
        }
    }

    pub fn website_label() -> &'static str {
        if is_chinese() {
            "官网"
        } else {
            "Website"
        }
    }

    pub fn core_config_label() -> &'static str {
        if is_chinese() {
            "核心配置："
        } else {
            "Core Configuration:"
        }
    }

    pub fn model_label() -> &'static str {
        if is_chinese() {
            "模型"
        } else {
            "Model"
        }
    }

    pub fn config_toml_lines(count: usize) -> String {
        if is_chinese() {
            format!("Config (TOML): {} 行", count)
        } else {
            format!("Config (TOML): {} lines", count)
        }
    }

    pub fn optional_fields_label() -> &'static str {
        if is_chinese() {
            "可选字段："
        } else {
            "Optional Fields:"
        }
    }

    pub fn notes_label_colon() -> &'static str {
        if is_chinese() {
            "备注"
        } else {
            "Notes"
        }
    }

    pub fn sort_index_label_colon() -> &'static str {
        if is_chinese() {
            "排序索引"
        } else {
            "Sort Index"
        }
    }

    pub fn id_label_colon() -> &'static str {
        if is_chinese() {
            "ID"
        } else {
            "ID"
        }
    }

    pub fn url_label_colon() -> &'static str {
        if is_chinese() {
            "网址"
        } else {
            "URL"
        }
    }

    pub fn api_url_label_colon() -> &'static str {
        if is_chinese() {
            "API 地址"
        } else {
            "API URL"
        }
    }

    pub fn summary_divider() -> &'static str {
        "======================"
    }

    // Provider Input - Summary Display
    pub fn basic_info_header() -> &'static str {
        if is_chinese() {
            "基本信息"
        } else {
            "Basic Info"
        }
    }

    pub fn name_display_label() -> &'static str {
        if is_chinese() {
            "名称"
        } else {
            "Name"
        }
    }

    pub fn app_display_label() -> &'static str {
        if is_chinese() {
            "应用"
        } else {
            "App"
        }
    }

    pub fn notes_display_label() -> &'static str {
        if is_chinese() {
            "备注"
        } else {
            "Notes"
        }
    }

    pub fn sort_index_display_label() -> &'static str {
        if is_chinese() {
            "排序"
        } else {
            "Sort Index"
        }
    }

    pub fn config_info_header() -> &'static str {
        if is_chinese() {
            "配置信息"
        } else {
            "Configuration"
        }
    }

    pub fn api_key_display_label() -> &'static str {
        if is_chinese() {
            "API Key"
        } else {
            "API Key"
        }
    }

    pub fn base_url_display_label() -> &'static str {
        if is_chinese() {
            "Base URL"
        } else {
            "Base URL"
        }
    }

    pub fn model_config_header() -> &'static str {
        if is_chinese() {
            "模型配置"
        } else {
            "Model Configuration"
        }
    }

    pub fn default_model_display() -> &'static str {
        if is_chinese() {
            "默认"
        } else {
            "Default"
        }
    }

    pub fn haiku_model_display() -> &'static str {
        if is_chinese() {
            "Haiku"
        } else {
            "Haiku"
        }
    }

    pub fn sonnet_model_display() -> &'static str {
        if is_chinese() {
            "Sonnet"
        } else {
            "Sonnet"
        }
    }

    pub fn opus_model_display() -> &'static str {
        if is_chinese() {
            "Opus"
        } else {
            "Opus"
        }
    }

    pub fn auth_type_display_label() -> &'static str {
        if is_chinese() {
            "认证"
        } else {
            "Auth Type"
        }
    }

    pub fn project_id_display_label() -> &'static str {
        if is_chinese() {
            "项目 ID"
        } else {
            "Project ID"
        }
    }

    pub fn location_display_label() -> &'static str {
        if is_chinese() {
            "位置"
        } else {
            "Location"
        }
    }

    // Interactive Provider - Menu Options
    pub fn edit_provider_menu() -> &'static str {
        if is_chinese() {
            "➕ 编辑供应商"
        } else {
            "➕ Edit Provider"
        }
    }

    pub fn no_editable_providers() -> &'static str {
        if is_chinese() {
            "没有可编辑的供应商"
        } else {
            "No providers available for editing"
        }
    }

    pub fn select_provider_to_edit() -> &'static str {
        if is_chinese() {
            "选择要编辑的供应商："
        } else {
            "Select provider to edit:"
        }
    }

    pub fn choose_edit_mode() -> &'static str {
        if is_chinese() {
            "选择编辑模式："
        } else {
            "Choose edit mode:"
        }
    }

    pub fn edit_mode_interactive() -> &'static str {
        if is_chinese() {
            "📝 交互式编辑 (分步提示)"
        } else {
            "📝 Interactive editing (step-by-step prompts)"
        }
    }

    pub fn edit_mode_json_editor() -> &'static str {
        if is_chinese() {
            "✏️  JSON 编辑 (使用外部编辑器)"
        } else {
            "✏️  JSON editing (use external editor)"
        }
    }

    pub fn cancel() -> &'static str {
        if is_chinese() {
            "❌ 取消"
        } else {
            "❌ Cancel"
        }
    }

    pub fn opening_external_editor() -> &'static str {
        if is_chinese() {
            "正在打开外部编辑器..."
        } else {
            "Opening external editor..."
        }
    }

    pub fn invalid_json_syntax() -> &'static str {
        if is_chinese() {
            "无效的 JSON 语法"
        } else {
            "Invalid JSON syntax"
        }
    }

    pub fn invalid_provider_structure() -> &'static str {
        if is_chinese() {
            "无效的供应商结构"
        } else {
            "Invalid provider structure"
        }
    }

    pub fn provider_id_cannot_be_changed() -> &'static str {
        if is_chinese() {
            "供应商 ID 不能被修改"
        } else {
            "Provider ID cannot be changed"
        }
    }

    pub fn retry_editing() -> &'static str {
        if is_chinese() {
            "是否重新编辑？"
        } else {
            "Retry editing?"
        }
    }

    pub fn no_changes_detected() -> &'static str {
        if is_chinese() {
            "未检测到任何更改"
        } else {
            "No changes detected"
        }
    }

    pub fn provider_summary() -> &'static str {
        if is_chinese() {
            "供应商信息摘要"
        } else {
            "Provider Summary"
        }
    }

    pub fn confirm_save_changes() -> &'static str {
        if is_chinese() {
            "确认保存更改？"
        } else {
            "Save changes?"
        }
    }

    pub fn editor_failed() -> &'static str {
        if is_chinese() {
            "编辑器失败"
        } else {
            "Editor failed"
        }
    }

    pub fn invalid_selection_format() -> &'static str {
        if is_chinese() {
            "无效的选择格式"
        } else {
            "Invalid selection format"
        }
    }

    // Provider Display Labels (for show_current and view_provider_detail)
    pub fn basic_info_section_header() -> &'static str {
        if is_chinese() {
            "基本信息 / Basic Info"
        } else {
            "Basic Info"
        }
    }

    pub fn name_label_with_colon() -> &'static str {
        if is_chinese() {
            "名称"
        } else {
            "Name"
        }
    }

    pub fn app_label_with_colon() -> &'static str {
        if is_chinese() {
            "应用"
        } else {
            "App"
        }
    }

    pub fn api_config_section_header() -> &'static str {
        if is_chinese() {
            "API 配置 / API Configuration"
        } else {
            "API Configuration"
        }
    }

    pub fn model_config_section_header() -> &'static str {
        if is_chinese() {
            "模型配置 / Model Configuration"
        } else {
            "Model Configuration"
        }
    }

    pub fn main_model_label_with_colon() -> &'static str {
        if is_chinese() {
            "主模型"
        } else {
            "Main Model"
        }
    }

    pub fn updated_config_header() -> &'static str {
        if is_chinese() {
            "修改后配置："
        } else {
            "Updated Configuration:"
        }
    }

    // Provider Add/Edit Messages
    pub fn generated_id_message(id: &str) -> String {
        if is_chinese() {
            format!("生成的 ID: {}", id)
        } else {
            format!("Generated ID: {}", id)
        }
    }

    pub fn edit_fields_instruction() -> &'static str {
        if is_chinese() {
            "逐个编辑字段（直接回车保留当前值）：\n"
        } else {
            "Edit fields one by one (press Enter to keep current value):\n"
        }
    }

    // ============================================
    // MCP SERVER MANAGEMENT (MCP 服务器管理)
    // ============================================

    pub fn mcp_management() -> &'static str {
        if is_chinese() {
            "🛠️  MCP 服务器管理"
        } else {
            "🛠️  MCP Server Management"
        }
    }

    pub fn no_mcp_servers() -> &'static str {
        if is_chinese() {
            "未找到 MCP 服务器。"
        } else {
            "No MCP servers found."
        }
    }

    pub fn sync_all_servers() -> &'static str {
        if is_chinese() {
            "🔄 同步所有服务器"
        } else {
            "🔄 Sync All Servers"
        }
    }

    pub fn synced_successfully() -> &'static str {
        if is_chinese() {
            "✓ 所有 MCP 服务器同步成功"
        } else {
            "✓ All MCP servers synced successfully"
        }
    }

    // ============================================
    // PROMPT MANAGEMENT (提示词管理)
    // ============================================

    pub fn prompts_management() -> &'static str {
        if is_chinese() {
            "💬 提示词管理"
        } else {
            "💬 Prompt Management"
        }
    }

    pub fn no_prompts() -> &'static str {
        if is_chinese() {
            "未找到提示词预设。"
        } else {
            "No prompt presets found."
        }
    }

    pub fn switch_active_prompt() -> &'static str {
        if is_chinese() {
            "🔄 切换活动提示词"
        } else {
            "🔄 Switch Active Prompt"
        }
    }

    pub fn no_prompts_available() -> &'static str {
        if is_chinese() {
            "没有可用的提示词。"
        } else {
            "No prompts available."
        }
    }

    pub fn select_prompt_to_activate() -> &'static str {
        if is_chinese() {
            "选择要激活的提示词："
        } else {
            "Select prompt to activate:"
        }
    }

    pub fn activated_prompt(id: &str) -> String {
        if is_chinese() {
            format!("✓ 已激活提示词 '{}'", id)
        } else {
            format!("✓ Activated prompt '{}'", id)
        }
    }

    pub fn deactivated_prompt(id: &str) -> String {
        if is_chinese() {
            format!("✓ 已取消激活提示词 '{}'", id)
        } else {
            format!("✓ Deactivated prompt '{}'", id)
        }
    }

    pub fn prompt_cleared_note() -> &'static str {
        if is_chinese() {
            "实时文件已清空"
        } else {
            "Live prompt file has been cleared"
        }
    }

    pub fn prompt_synced_note() -> &'static str {
        if is_chinese() {
            "注意：提示词已同步到实时配置文件。"
        } else {
            "Note: The prompt has been synced to the live configuration file."
        }
    }

    // Configuration View
    pub fn current_configuration() -> &'static str {
        if is_chinese() {
            "👁️  当前配置"
        } else {
            "👁️  Current Configuration"
        }
    }

    pub fn provider_label() -> &'static str {
        if is_chinese() {
            "供应商："
        } else {
            "Provider:"
        }
    }

    pub fn mcp_servers_label() -> &'static str {
        if is_chinese() {
            "MCP 服务器："
        } else {
            "MCP Servers:"
        }
    }

    pub fn prompts_label() -> &'static str {
        if is_chinese() {
            "提示词："
        } else {
            "Prompts:"
        }
    }

    pub fn total() -> &'static str {
        if is_chinese() {
            "总计"
        } else {
            "Total"
        }
    }

    pub fn enabled() -> &'static str {
        if is_chinese() {
            "启用"
        } else {
            "Enabled"
        }
    }

    pub fn active() -> &'static str {
        if is_chinese() {
            "活动"
        } else {
            "Active"
        }
    }

    pub fn none() -> &'static str {
        if is_chinese() {
            "无"
        } else {
            "None"
        }
    }

    // Settings
    pub fn settings_title() -> &'static str {
        if is_chinese() {
            "⚙️  设置"
        } else {
            "⚙️  Settings"
        }
    }

    pub fn change_language() -> &'static str {
        if is_chinese() {
            "🌐 切换语言"
        } else {
            "🌐 Change Language"
        }
    }

    pub fn current_language_label() -> &'static str {
        if is_chinese() {
            "当前语言"
        } else {
            "Current Language"
        }
    }

    pub fn select_language() -> &'static str {
        if is_chinese() {
            "选择语言："
        } else {
            "Select language:"
        }
    }

    pub fn language_changed() -> &'static str {
        if is_chinese() {
            "✓ 语言已更改"
        } else {
            "✓ Language changed"
        }
    }

    // App Selection
    pub fn select_application() -> &'static str {
        if is_chinese() {
            "选择应用程序："
        } else {
            "Select application:"
        }
    }

    pub fn switched_to_app(app: &str) -> String {
        if is_chinese() {
            format!("✓ 已切换到 {}", app)
        } else {
            format!("✓ Switched to {}", app)
        }
    }

    // Common
    pub fn press_enter() -> &'static str {
        if is_chinese() {
            "按 Enter 继续..."
        } else {
            "Press Enter to continue..."
        }
    }

    pub fn error_prefix() -> &'static str {
        if is_chinese() {
            "错误"
        } else {
            "Error"
        }
    }

    // Table Headers
    pub fn header_name() -> &'static str {
        if is_chinese() {
            "名称"
        } else {
            "Name"
        }
    }

    pub fn header_category() -> &'static str {
        if is_chinese() {
            "类别"
        } else {
            "Category"
        }
    }

    pub fn header_description() -> &'static str {
        if is_chinese() {
            "描述"
        } else {
            "Description"
        }
    }

    // Config Management
    pub fn config_management() -> &'static str {
        if is_chinese() {
            "⚙️  配置文件管理"
        } else {
            "⚙️  Configuration Management"
        }
    }

    pub fn config_export() -> &'static str {
        if is_chinese() {
            "📤 导出配置"
        } else {
            "📤 Export Config"
        }
    }

    pub fn config_import() -> &'static str {
        if is_chinese() {
            "📥 导入配置"
        } else {
            "📥 Import Config"
        }
    }

    pub fn config_backup() -> &'static str {
        if is_chinese() {
            "💾 备份配置"
        } else {
            "💾 Backup Config"
        }
    }

    pub fn config_restore() -> &'static str {
        if is_chinese() {
            "♻️  恢复配置"
        } else {
            "♻️  Restore Config"
        }
    }

    pub fn config_validate() -> &'static str {
        if is_chinese() {
            "✓ 验证配置"
        } else {
            "✓ Validate Config"
        }
    }

    pub fn config_common_snippet() -> &'static str {
        if is_chinese() {
            "🧩 通用配置片段"
        } else {
            "🧩 Common Config Snippet"
        }
    }

    pub fn config_reset() -> &'static str {
        if is_chinese() {
            "🔄 重置配置"
        } else {
            "🔄 Reset Config"
        }
    }

    pub fn config_show_full() -> &'static str {
        if is_chinese() {
            "👁️  查看完整配置"
        } else {
            "👁️  Show Full Config"
        }
    }

    pub fn config_show_path() -> &'static str {
        if is_chinese() {
            "📍 显示配置路径"
        } else {
            "📍 Show Config Path"
        }
    }

    pub fn enter_export_path() -> &'static str {
        if is_chinese() {
            "输入导出文件路径："
        } else {
            "Enter export file path:"
        }
    }

    pub fn enter_import_path() -> &'static str {
        if is_chinese() {
            "输入导入文件路径："
        } else {
            "Enter import file path:"
        }
    }

    pub fn enter_restore_path() -> &'static str {
        if is_chinese() {
            "输入备份文件路径："
        } else {
            "Enter backup file path:"
        }
    }

    pub fn confirm_import() -> &'static str {
        if is_chinese() {
            "确定要导入配置吗？这将覆盖当前配置。"
        } else {
            "Are you sure you want to import? This will overwrite current configuration."
        }
    }

    pub fn confirm_reset() -> &'static str {
        if is_chinese() {
            "确定要重置配置吗？这将删除所有自定义设置。"
        } else {
            "Are you sure you want to reset? This will delete all custom settings."
        }
    }

    pub fn common_config_snippet_editor_prompt(app: &str) -> String {
        if is_chinese() {
            format!("编辑 {app} 的通用配置片段（JSON 对象，留空则清除）：")
        } else {
            format!("Edit common config snippet for {app} (JSON object; empty to clear):")
        }
    }

    pub fn common_config_snippet_invalid_json(err: &str) -> String {
        if is_chinese() {
            format!("JSON 无效：{err}")
        } else {
            format!("Invalid JSON: {err}")
        }
    }

    pub fn common_config_snippet_not_object() -> &'static str {
        if is_chinese() {
            "通用配置必须是 JSON 对象（例如：{\"env\":{...}}）"
        } else {
            "Common config must be a JSON object (e.g. {\"env\":{...}})"
        }
    }

    pub fn common_config_snippet_saved() -> &'static str {
        if is_chinese() {
            "✓ 已保存通用配置片段"
        } else {
            "✓ Common config snippet saved"
        }
    }

    pub fn common_config_snippet_cleared() -> &'static str {
        if is_chinese() {
            "✓ 已清除通用配置片段"
        } else {
            "✓ Common config snippet cleared"
        }
    }

    pub fn common_config_snippet_apply_now() -> &'static str {
        if is_chinese() {
            "现在应用到当前供应商（写入 live 配置）？"
        } else {
            "Apply to current provider now (write live config)?"
        }
    }

    pub fn common_config_snippet_no_current_provider() -> &'static str {
        if is_chinese() {
            "当前未选择供应商，已保存通用配置片段。"
        } else {
            "No current provider selected; common config snippet saved."
        }
    }

    pub fn common_config_snippet_applied() -> &'static str {
        if is_chinese() {
            "✓ 已应用到 live 配置（请重启对应客户端）"
        } else {
            "✓ Applied to live config (restart the client)"
        }
    }

    pub fn common_config_snippet_apply_hint() -> &'static str {
        if is_chinese() {
            "提示：切换一次供应商即可重新写入 live 配置。"
        } else {
            "Tip: switch provider once to re-write the live config."
        }
    }

    pub fn confirm_restore() -> &'static str {
        if is_chinese() {
            "确定要从备份恢复配置吗？"
        } else {
            "Are you sure you want to restore from backup?"
        }
    }

    pub fn exported_to(path: &str) -> String {
        if is_chinese() {
            format!("✓ 已导出到 '{}'", path)
        } else {
            format!("✓ Exported to '{}'", path)
        }
    }

    pub fn imported_from(path: &str) -> String {
        if is_chinese() {
            format!("✓ 已从 '{}' 导入", path)
        } else {
            format!("✓ Imported from '{}'", path)
        }
    }

    pub fn backup_created(id: &str) -> String {
        if is_chinese() {
            format!("✓ 已创建备份，ID: {}", id)
        } else {
            format!("✓ Backup created, ID: {}", id)
        }
    }

    pub fn restored_from(path: &str) -> String {
        if is_chinese() {
            format!("✓ 已从 '{}' 恢复", path)
        } else {
            format!("✓ Restored from '{}'", path)
        }
    }

    pub fn config_valid() -> &'static str {
        if is_chinese() {
            "✓ 配置文件有效"
        } else {
            "✓ Configuration is valid"
        }
    }

    pub fn config_reset_done() -> &'static str {
        if is_chinese() {
            "✓ 配置已重置为默认值"
        } else {
            "✓ Configuration reset to defaults"
        }
    }

    pub fn file_overwrite_confirm(path: &str) -> String {
        if is_chinese() {
            format!("文件 '{}' 已存在，是否覆盖？", path)
        } else {
            format!("File '{}' exists. Overwrite?", path)
        }
    }

    // MCP Management Additional
    pub fn mcp_delete_server() -> &'static str {
        if is_chinese() {
            "🗑️  删除服务器"
        } else {
            "🗑️  Delete Server"
        }
    }

    pub fn mcp_enable_server() -> &'static str {
        if is_chinese() {
            "✅ 启用服务器"
        } else {
            "✅ Enable Server"
        }
    }

    pub fn mcp_disable_server() -> &'static str {
        if is_chinese() {
            "❌ 禁用服务器"
        } else {
            "❌ Disable Server"
        }
    }

    pub fn mcp_import_servers() -> &'static str {
        if is_chinese() {
            "📥 从实时配置导入"
        } else {
            "📥 Import from Live Config"
        }
    }

    pub fn mcp_validate_command() -> &'static str {
        if is_chinese() {
            "✓ 验证命令"
        } else {
            "✓ Validate Command"
        }
    }

    pub fn select_server_to_delete() -> &'static str {
        if is_chinese() {
            "选择要删除的服务器："
        } else {
            "Select server to delete:"
        }
    }

    pub fn select_server_to_enable() -> &'static str {
        if is_chinese() {
            "选择要启用的服务器："
        } else {
            "Select server to enable:"
        }
    }

    pub fn select_server_to_disable() -> &'static str {
        if is_chinese() {
            "选择要禁用的服务器："
        } else {
            "Select server to disable:"
        }
    }

    pub fn select_apps_to_enable() -> &'static str {
        if is_chinese() {
            "选择要启用的应用："
        } else {
            "Select apps to enable for:"
        }
    }

    pub fn mcp_enable_apps_help() -> &'static str {
        if is_chinese() {
            "空格 选择/取消选择，→ 全选，← 取消全选，Esc 返回；输入可过滤"
        } else {
            "Space to toggle, → select all, ← clear all, Esc to go back; type to filter"
        }
    }

    pub fn select_apps_to_disable() -> &'static str {
        if is_chinese() {
            "选择要禁用的应用："
        } else {
            "Select apps to disable for:"
        }
    }

    pub fn enter_command_to_validate() -> &'static str {
        if is_chinese() {
            "输入要验证的命令："
        } else {
            "Enter command to validate:"
        }
    }

    pub fn server_deleted(id: &str) -> String {
        if is_chinese() {
            format!("✓ 已删除服务器 '{}'", id)
        } else {
            format!("✓ Deleted server '{}'", id)
        }
    }

    pub fn server_enabled(id: &str) -> String {
        if is_chinese() {
            format!("✓ 已启用服务器 '{}'", id)
        } else {
            format!("✓ Enabled server '{}'", id)
        }
    }

    pub fn server_disabled(id: &str) -> String {
        if is_chinese() {
            format!("✓ 已禁用服务器 '{}'", id)
        } else {
            format!("✓ Disabled server '{}'", id)
        }
    }

    pub fn servers_imported(count: usize) -> String {
        if is_chinese() {
            format!("✓ 已导入 {} 个服务器", count)
        } else {
            format!("✓ Imported {} servers", count)
        }
    }

    pub fn command_valid(cmd: &str) -> String {
        if is_chinese() {
            format!("✓ 命令 '{}' 有效", cmd)
        } else {
            format!("✓ Command '{}' is valid", cmd)
        }
    }

    pub fn command_invalid(cmd: &str) -> String {
        if is_chinese() {
            format!("✗ 命令 '{}' 未找到", cmd)
        } else {
            format!("✗ Command '{}' not found", cmd)
        }
    }

    // Prompts Management Additional
    pub fn prompts_show_content() -> &'static str {
        if is_chinese() {
            "👁️  查看完整内容"
        } else {
            "👁️  View Full Content"
        }
    }

    pub fn prompts_delete() -> &'static str {
        if is_chinese() {
            "🗑️  删除提示词"
        } else {
            "🗑️  Delete Prompt"
        }
    }

    pub fn prompts_view_current() -> &'static str {
        if is_chinese() {
            "📋 查看当前提示词"
        } else {
            "📋 View Current Prompt"
        }
    }

    pub fn select_prompt_to_view() -> &'static str {
        if is_chinese() {
            "选择要查看的提示词："
        } else {
            "Select prompt to view:"
        }
    }

    pub fn select_prompt_to_delete() -> &'static str {
        if is_chinese() {
            "选择要删除的提示词："
        } else {
            "Select prompt to delete:"
        }
    }

    pub fn prompt_deleted(id: &str) -> String {
        if is_chinese() {
            format!("✓ 已删除提示词 '{}'", id)
        } else {
            format!("✓ Deleted prompt '{}'", id)
        }
    }

    pub fn no_active_prompt() -> &'static str {
        if is_chinese() {
            "当前没有激活的提示词。"
        } else {
            "No active prompt."
        }
    }

    pub fn cannot_delete_active() -> &'static str {
        if is_chinese() {
            "无法删除当前激活的提示词。"
        } else {
            "Cannot delete the active prompt."
        }
    }

    pub fn no_servers_to_delete() -> &'static str {
        if is_chinese() {
            "没有可删除的服务器。"
        } else {
            "No servers to delete."
        }
    }

    pub fn no_prompts_to_delete() -> &'static str {
        if is_chinese() {
            "没有可删除的提示词。"
        } else {
            "No prompts to delete."
        }
    }

    // Provider Speedtest
    pub fn speedtest_endpoint() -> &'static str {
        if is_chinese() {
            "🚀 测试端点速度"
        } else {
            "🚀 Speedtest endpoint"
        }
    }

    pub fn back() -> &'static str {
        if is_chinese() {
            "← 返回"
        } else {
            "← Back"
        }
    }
}
