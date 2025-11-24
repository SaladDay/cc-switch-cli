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

    // Provider Management
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

    // MCP Management
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

    // Prompts Management
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
}
