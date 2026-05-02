use crate::settings::{get_settings, update_settings};
use std::sync::OnceLock;
use std::sync::RwLock;

#[cfg(test)]
use std::cell::RefCell;

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
        let lang = if cfg!(test) {
            // Keep unit tests deterministic and avoid reading real user settings.
            Language::English
        } else {
            let settings = get_settings();
            settings
                .language
                .as_deref()
                .map(Language::from_code)
                .unwrap_or(Language::English)
        };
        RwLock::new(lang)
    })
}

#[cfg(test)]
thread_local! {
    static TEST_LANGUAGE_OVERRIDE: RefCell<Option<Language>> = const { RefCell::new(None) };
}

#[cfg(test)]
pub(crate) struct TestLanguageGuard(Option<Language>);

#[cfg(test)]
impl Drop for TestLanguageGuard {
    fn drop(&mut self) {
        TEST_LANGUAGE_OVERRIDE.with(|slot| {
            *slot.borrow_mut() = self.0;
        });
    }
}

#[cfg(test)]
pub(crate) fn use_test_language(lang: Language) -> TestLanguageGuard {
    let previous = TEST_LANGUAGE_OVERRIDE.with(|slot| slot.replace(Some(lang)));
    TestLanguageGuard(previous)
}

/// Get current language
pub fn current_language() -> Language {
    #[cfg(test)]
    if let Some(lang) = TEST_LANGUAGE_OVERRIDE.with(|slot| *slot.borrow()) {
        return lang;
    }

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

    pub fn interactive_requires_tty() -> &'static str {
        if is_chinese() {
            "交互模式需要在 TTY 终端中运行（请不要通过管道/重定向调用）。"
        } else {
            "Interactive mode requires a TTY (do not run with pipes/redirection)."
        }
    }

    pub fn interactive_legacy_tui_removed() -> &'static str {
        if is_chinese() {
            "旧版 legacy TUI 已移除，请直接使用当前默认的交互 TUI。"
        } else {
            "The legacy TUI has been removed. Please use the default interactive TUI instead."
        }
    }

    // Ratatui TUI (new interactive UI)
    pub fn tui_app_title() -> &'static str {
        "cc-switch"
    }

    pub fn tui_tabs_title() -> &'static str {
        if is_chinese() {
            "App"
        } else {
            "App"
        }
    }

    pub fn tui_hint_app_switch() -> &'static str {
        if is_chinese() {
            "切换 App:"
        } else {
            "Switch App:"
        }
    }

    pub fn tui_filter_icon() -> &'static str {
        "🔎 "
    }

    pub fn tui_marker_active() -> &'static str {
        "✓"
    }

    pub fn tui_marker_inactive() -> &'static str {
        " "
    }

    pub fn tui_highlight_symbol() -> &'static str {
        "➤ "
    }

    pub fn tui_toast_prefix_info() -> &'static str {
        " ℹ "
    }

    pub fn tui_toast_prefix_success() -> &'static str {
        " ✓ "
    }

    pub fn tui_toast_prefix_warning() -> &'static str {
        " ! "
    }

    pub fn tui_toast_prefix_error() -> &'static str {
        " ✗ "
    }

    pub fn tui_toast_invalid_json(details: &str) -> String {
        if is_chinese() {
            format!("JSON 无效：{details}")
        } else {
            format!("Invalid JSON: {details}")
        }
    }

    pub fn tui_toast_json_must_be_object() -> &'static str {
        if is_chinese() {
            "JSON 必须是对象（例如：{\"env\":{...}}）"
        } else {
            "JSON must be an object (e.g. {\"env\":{...}})"
        }
    }

    pub fn tui_error_invalid_config_structure(e: &str) -> String {
        if is_chinese() {
            format!("配置结构无效：{e}")
        } else {
            format!("Invalid config structure: {e}")
        }
    }

    pub fn tui_rule(width: usize) -> String {
        if is_chinese() {
            "─".repeat(width)
        } else {
            "─".repeat(width)
        }
    }

    pub fn tui_rule_heavy(width: usize) -> String {
        if is_chinese() {
            "═".repeat(width)
        } else {
            "═".repeat(width)
        }
    }

    pub fn tui_icon_app() -> &'static str {
        "📱"
    }

    pub fn tui_default_config_filename() -> &'static str {
        "config.json"
    }

    pub fn tui_default_config_export_path() -> &'static str {
        "./config-export.sql"
    }

    pub fn tui_default_common_snippet() -> &'static str {
        "{}\n"
    }

    pub fn tui_default_common_snippet_for_app(app: &str) -> &'static str {
        match app {
            "codex" => "",
            _ => "{}\n",
        }
    }

    pub fn tui_latency_ms(ms: u128) -> String {
        if is_chinese() {
            format!("{ms} ms")
        } else {
            format!("{ms} ms")
        }
    }
    pub fn tui_nav_title() -> &'static str {
        if is_chinese() {
            "菜单"
        } else {
            "Menu"
        }
    }

    pub fn tui_filter_title() -> &'static str {
        if is_chinese() {
            "过滤"
        } else {
            "Filter"
        }
    }

    pub fn tui_footer_global() -> &'static str {
        if is_chinese() {
            "[ ] 切换应用  ←→ 切换菜单/内容  ↑↓ 移动  Enter 详情  s 切换  / 过滤  Esc 返回  ? 帮助"
        } else {
            "[ ] switch app  ←→ focus menu/content  ↑↓ move  Enter details  s switch  / filter  Esc back  ? help"
        }
    }

    pub fn tui_footer_group_nav() -> &'static str {
        if is_chinese() {
            "导航"
        } else {
            "NAV"
        }
    }

    pub fn tui_footer_group_actions() -> &'static str {
        if is_chinese() {
            "功能"
        } else {
            "ACT"
        }
    }

    pub fn tui_footer_nav_keys() -> &'static str {
        if is_chinese() {
            "←→ 菜单/内容  ↑↓ 移动"
        } else {
            "←→ menu/content  ↑↓ move"
        }
    }

    pub fn tui_footer_action_keys() -> &'static str {
        if is_chinese() {
            "[ ] 切换应用  Enter 详情  s 切换  / 过滤  Esc 返回  ? 帮助"
        } else {
            "[ ] switch app  Enter details  s switch  / filter  Esc back  ? help"
        }
    }

    pub fn tui_footer_action_keys_main() -> &'static str {
        if is_chinese() {
            "[ ] 切换应用  / 过滤  Esc 返回  ? 帮助"
        } else {
            "[ ] switch app  / filter  Esc back  ? help"
        }
    }

    pub fn tui_footer_action_keys_providers() -> &'static str {
        if is_chinese() {
            "[ ] 切换应用  Enter 详情  s 切换  a 添加  e 编辑  d 删除  t 测速  c 健康检查  / 过滤  Esc 返回  ? 帮助"
        } else {
            "[ ] switch app  Enter details  s switch  a add  e edit  d delete  t speedtest  c stream check  / filter  Esc back  ? help"
        }
    }

    pub fn tui_footer_action_keys_provider_detail() -> &'static str {
        if is_chinese() {
            "[ ] 切换应用  s 切换  e 编辑  t 测速  c 健康检查  / 过滤  Esc 返回  ? 帮助"
        } else {
            "[ ] switch app  s switch  e edit  t speedtest  c stream check  / filter  Esc back  ? help"
        }
    }

    pub fn tui_footer_action_keys_mcp() -> &'static str {
        if is_chinese() {
            "[ ] 切换应用  x 启用/禁用  m 应用  a 添加  e 编辑  i 导入  d 删除  / 过滤  Esc 返回  ? 帮助"
        } else {
            "[ ] switch app  x toggle  m apps  a add  e edit  i import  d delete  / filter  Esc back  ? help"
        }
    }

    pub fn tui_footer_action_keys_prompts() -> &'static str {
        if is_chinese() {
            "[ ] 切换应用  Enter 查看  a 激活  x 取消激活  e 编辑  d 删除  / 过滤  Esc 返回  ? 帮助"
        } else {
            "[ ] switch app  Enter view  a activate  x deactivate  e edit  d delete  / filter  Esc back  ? help"
        }
    }

    pub fn tui_footer_action_keys_config() -> &'static str {
        if is_chinese() {
            "[ ] 切换应用  Enter 打开  e 编辑片段  / 过滤  Esc 返回  ? 帮助"
        } else {
            "[ ] switch app  Enter open  e edit snippet  / filter  Esc back  ? help"
        }
    }

    pub fn tui_footer_action_keys_common_snippet_view() -> &'static str {
        if is_chinese() {
            "a 应用  c 清空  e 编辑  ↑↓ 滚动  Esc 返回"
        } else {
            "a apply  c clear  e edit  ↑↓ scroll  Esc back"
        }
    }

    pub fn tui_footer_action_keys_settings() -> &'static str {
        if is_chinese() {
            "[ ] 切换应用  Enter 应用  / 过滤  Esc 返回  ? 帮助"
        } else {
            "[ ] switch app  Enter apply  / filter  Esc back  ? help"
        }
    }

    pub fn tui_footer_action_keys_global() -> &'static str {
        if is_chinese() {
            "[ ] 切换应用  / 过滤  Esc 返回  ? 帮助"
        } else {
            "[ ] switch app  / filter  Esc back  ? help"
        }
    }

    pub fn tui_footer_filter_mode() -> &'static str {
        if is_chinese() {
            "输入关键字过滤，Enter 应用，Esc 清空并退出"
        } else {
            "Type to filter, Enter apply, Esc clear & exit"
        }
    }

    pub fn tui_help_title() -> &'static str {
        if is_chinese() {
            "帮助"
        } else {
            "Help"
        }
    }

    pub fn tui_help_text() -> &'static str {
        if is_chinese() {
            "[ ]  切换应用\n←→  切换菜单/内容焦点\n↑↓  移动\n/   过滤\nEsc  返回\n?   显示/关闭帮助\n\n页面快捷键（在页面内容区顶部显示）：\n- 供应商：Enter 详情，s 切换/添加移除，a 添加，e 编辑，d 删除，t 测速，c 健康检查\n- 供应商详情：s 切换/添加移除，e 编辑，t 测速，c 健康检查\n- MCP：x 启用/禁用(当前应用)，m 选择应用，a 添加，e 编辑，i 导入已有，d 删除\n- 提示词：Enter 查看，a 激活，x 取消激活(当前)，e 编辑，d 删除\n- 技能：Enter 详情，x 启用/禁用(当前应用)，m 选择应用，d 卸载，i 导入已有\n- 配置：Enter 打开/执行，e 编辑片段\n- 设置：Enter 应用"
        } else {
            "[ ]  switch app\n←→  focus menu/content\n↑↓  move\n/   filter\nEsc  back\n?   toggle help\n\nPage keys (shown at the top of each page):\n- Providers: Enter details, s switch/add-remove, a add, e edit, d delete, t speedtest, c stream check\n- Provider Detail: s switch/add-remove, e edit, t speedtest, c stream check\n- MCP: x toggle current, m select apps, a add, e edit, i import existing, d delete\n- Prompts: Enter view, a activate, x deactivate active, e edit, d delete\n- Skills: Enter details, x toggle current, m select apps, d uninstall, i import existing\n- Config: Enter open/run, e edit snippet\n- Settings: Enter apply"
        }
    }

    pub fn tui_confirm_title() -> &'static str {
        if is_chinese() {
            "确认"
        } else {
            "Confirm"
        }
    }

    pub fn tui_confirm_exit_title() -> &'static str {
        if is_chinese() {
            "退出"
        } else {
            "Exit"
        }
    }

    pub fn tui_confirm_exit_message() -> &'static str {
        if is_chinese() {
            "确定退出 cc-switch？"
        } else {
            "Exit cc-switch?"
        }
    }

    pub fn tui_confirm_yes_hint() -> &'static str {
        if is_chinese() {
            "y/Enter = 是"
        } else {
            "y/Enter = Yes"
        }
    }

    pub fn tui_confirm_no_hint() -> &'static str {
        if is_chinese() {
            "n/Esc   = 否"
        } else {
            "n/Esc   = No"
        }
    }

    pub fn tui_input_title() -> &'static str {
        if is_chinese() {
            "输入"
        } else {
            "Input"
        }
    }

    pub fn tui_editor_text_field_title() -> &'static str {
        if is_chinese() {
            "文本"
        } else {
            "Text"
        }
    }

    pub fn tui_editor_json_field_title() -> &'static str {
        "JSON"
    }

    pub fn tui_editor_hint_view() -> &'static str {
        if is_chinese() {
            "Enter 编辑  ↑↓ 滚动  Ctrl+S 保存  Esc 返回"
        } else {
            "Enter edit  ↑↓ scroll  Ctrl+S save  Esc back"
        }
    }

    pub fn tui_editor_hint_edit() -> &'static str {
        if is_chinese() {
            "编辑中：Esc 退出编辑  Ctrl+S 保存"
        } else {
            "Editing: Esc stop editing  Ctrl+S save"
        }
    }

    pub fn tui_editor_discard_title() -> &'static str {
        if is_chinese() {
            "放弃修改"
        } else {
            "Discard Changes"
        }
    }

    pub fn tui_editor_discard_message() -> &'static str {
        if is_chinese() {
            "有未保存的修改，确定放弃？"
        } else {
            "You have unsaved changes. Discard them?"
        }
    }

    pub fn tui_editor_save_before_close_title() -> &'static str {
        if is_chinese() {
            "当前未保存"
        } else {
            "Unsaved Changes"
        }
    }

    pub fn tui_editor_save_before_close_message() -> &'static str {
        if is_chinese() {
            "当前有未保存的修改。"
        } else {
            "You have unsaved changes."
        }
    }

    pub fn tui_speedtest_title() -> &'static str {
        if is_chinese() {
            "测速"
        } else {
            "Speedtest"
        }
    }

    pub fn tui_stream_check_title() -> &'static str {
        if is_chinese() {
            "健康检查"
        } else {
            "Stream Check"
        }
    }

    pub fn tui_main_hint() -> &'static str {
        if is_chinese() {
            "使用左侧菜单（↑↓ + Enter）。←→ 在菜单与内容间切换焦点。"
        } else {
            "Use the left menu (↑↓ + Enter). ←→ switches focus between menu and content."
        }
    }

    pub fn tui_header_proxy_status(enabled: bool) -> String {
        if is_chinese() {
            format!("代理: {}", if enabled { "开" } else { "关" })
        } else {
            format!("Proxy: {}", if enabled { "On" } else { "Off" })
        }
    }

    pub fn tui_header_config_error() -> &'static str {
        if is_chinese() {
            "配置错误"
        } else {
            "Config Error"
        }
    }

    pub fn tui_home_section_connection() -> &'static str {
        if is_chinese() {
            "连接信息"
        } else {
            "Connection Details"
        }
    }

    pub fn tui_home_section_proxy() -> &'static str {
        if is_chinese() {
            "代理仪表盘"
        } else {
            "Proxy Dashboard"
        }
    }

    pub fn tui_home_section_context() -> &'static str {
        if is_chinese() {
            "Session Context"
        } else {
            "Session Context"
        }
    }

    pub fn tui_home_section_local_env_check() -> &'static str {
        if is_chinese() {
            "本地环境检查"
        } else {
            "Local environment check"
        }
    }

    pub fn tui_home_section_webdav() -> &'static str {
        if is_chinese() {
            "WebDAV 同步"
        } else {
            "WebDAV Sync"
        }
    }

    pub fn tui_label_webdav_status() -> &'static str {
        if is_chinese() {
            "状态"
        } else {
            "Status"
        }
    }

    pub fn tui_label_webdav_last_sync() -> &'static str {
        if is_chinese() {
            "最近同步"
        } else {
            "Last sync"
        }
    }

    pub fn tui_webdav_status_not_configured() -> &'static str {
        if is_chinese() {
            "未配置"
        } else {
            "Not configured"
        }
    }

    pub fn tui_webdav_status_configured() -> &'static str {
        if is_chinese() {
            "已配置"
        } else {
            "Configured"
        }
    }

    pub fn tui_webdav_status_never_synced() -> &'static str {
        if is_chinese() {
            "从未同步"
        } else {
            "Never synced"
        }
    }

    pub fn tui_webdav_status_ok() -> &'static str {
        if is_chinese() {
            "正常"
        } else {
            "OK"
        }
    }

    pub fn tui_webdav_status_error() -> &'static str {
        if is_chinese() {
            "失败"
        } else {
            "Error"
        }
    }

    pub fn tui_webdav_status_error_with_detail(detail: &str) -> String {
        if is_chinese() {
            format!("失败（{detail}）")
        } else {
            format!("Error ({detail})")
        }
    }

    pub fn tui_local_env_not_installed() -> &'static str {
        if is_chinese() {
            "未安装或不可执行"
        } else {
            "not installed or not executable"
        }
    }

    pub fn tui_home_status_online() -> &'static str {
        if is_chinese() {
            "在线"
        } else {
            "Online"
        }
    }

    pub fn tui_home_status_offline() -> &'static str {
        if is_chinese() {
            "离线"
        } else {
            "Offline"
        }
    }

    pub fn tui_proxy_dashboard_status_running() -> &'static str {
        if is_chinese() {
            "已启用"
        } else {
            "ACTIVE"
        }
    }

    pub fn tui_proxy_dashboard_status_stopped() -> &'static str {
        if is_chinese() {
            "本地"
        } else {
            "LOCAL"
        }
    }

    pub fn tui_proxy_dashboard_status_local_only() -> &'static str {
        if is_chinese() {
            "仅本地"
        } else {
            "LOCAL ONLY"
        }
    }

    pub fn tui_proxy_dashboard_status_unsupported() -> &'static str {
        if is_chinese() {
            "不支持"
        } else {
            "UNSUPPORTED"
        }
    }

    pub fn tui_proxy_dashboard_manual_routing_copy(app: &str) -> String {
        if is_chinese() {
            format!("手动路由：{app} 的流量会通过 cc-switch。")
        } else {
            format!("Manual routing only: traffic goes through cc-switch for {app}.")
        }
    }

    pub fn tui_proxy_dashboard_failover_copy() -> &'static str {
        if is_chinese() {
            "仅做手动路由，不会自动切换供应商。"
        } else {
            "automatic failover stays off; provider changes stay manual."
        }
    }

    pub fn tui_proxy_dashboard_cta_start(app: &str) -> String {
        if is_chinese() {
            format!("按 P 启动托管代理，并让 {app} 走 cc-switch。")
        } else {
            format!("Press P to start the managed proxy and route {app} through cc-switch.")
        }
    }

    pub fn tui_proxy_dashboard_cta_stop(app: &str) -> String {
        if is_chinese() {
            format!("按 P 恢复 {app} 的 live 配置，并停止托管代理。")
        } else {
            format!("Press P to restore {app} to its live config and stop the managed proxy.")
        }
    }

    pub fn tui_proxy_loading_title_start() -> &'static str {
        if is_chinese() {
            "启动代理中"
        } else {
            "Starting proxy"
        }
    }

    pub fn tui_proxy_loading_title_stop() -> &'static str {
        if is_chinese() {
            "停止代理中"
        } else {
            "Stopping proxy"
        }
    }

    pub fn tui_proxy_dashboard_running_elsewhere() -> &'static str {
        if is_chinese() {
            "代理已在运行。请先停止当前路由，再从这里启动。"
        } else {
            "Proxy is already running. Stop the current route before starting it here."
        }
    }

    pub fn tui_proxy_dashboard_current_app_on(app: &str) -> String {
        if is_chinese() {
            format!("{app} 已接入代理")
        } else {
            format!("{app} active")
        }
    }

    pub fn tui_proxy_dashboard_current_app_off(app: &str) -> String {
        if is_chinese() {
            format!("{app} 本地直连")
        } else {
            format!("{app} local")
        }
    }

    pub fn tui_proxy_dashboard_unsupported_app(app: &str) -> String {
        if is_chinese() {
            format!("{app} 仅本地")
        } else {
            format!("{app} local only")
        }
    }

    pub fn tui_proxy_dashboard_shared_runtime_ready() -> &'static str {
        if is_chinese() {
            "共享 runtime 就绪"
        } else {
            "Shared runtime ready"
        }
    }

    pub fn tui_proxy_dashboard_no_route_for_app(app: &str) -> String {
        if is_chinese() {
            format!("{app} 暂无路由")
        } else {
            format!("No route for {app} yet")
        }
    }

    pub fn tui_proxy_dashboard_takeover_active() -> &'static str {
        if is_chinese() {
            "已接管"
        } else {
            "active"
        }
    }

    pub fn tui_proxy_dashboard_takeover_inactive() -> &'static str {
        if is_chinese() {
            "未接管"
        } else {
            "inactive"
        }
    }

    pub fn tui_proxy_dashboard_takeover_unsupported() -> &'static str {
        if is_chinese() {
            "不支持"
        } else {
            "not supported"
        }
    }

    pub fn tui_proxy_dashboard_uptime_stopped() -> &'static str {
        if is_chinese() {
            "未运行"
        } else {
            "--"
        }
    }

    pub fn tui_proxy_dashboard_requests_idle() -> &'static str {
        if is_chinese() {
            "暂无流量"
        } else {
            "No traffic yet"
        }
    }

    pub fn tui_proxy_dashboard_tokens_idle() -> &'static str {
        if is_chinese() {
            "暂无 token 流量"
        } else {
            "No token traffic yet"
        }
    }

    pub fn tui_proxy_dashboard_target_waiting() -> &'static str {
        if is_chinese() {
            "等待首个请求"
        } else {
            "Waiting for first request"
        }
    }

    pub fn tui_proxy_dashboard_request_summary(total: u64, success_rate: f32) -> String {
        if is_chinese() {
            format!("{total} 总计 / {success_rate:.1}% 成功")
        } else {
            format!("{total} total / {success_rate:.1}% success")
        }
    }

    pub fn tui_proxy_dashboard_token_summary(output: &str, input: &str) -> String {
        if is_chinese() {
            format!("{output} 下行 / {input} 上行")
        } else {
            format!("{output} out / {input} in")
        }
    }

    pub fn tui_label_current_app_takeover() -> &'static str {
        if is_chinese() {
            "当前应用接管"
        } else {
            "Current app takeover"
        }
    }

    pub fn tui_label_current_app_route() -> &'static str {
        if is_chinese() {
            "当前应用路由"
        } else {
            "Current app route"
        }
    }

    pub fn tui_label_latest_proxy_route() -> &'static str {
        if is_chinese() {
            "最近代理路由"
        } else {
            "Latest proxy route"
        }
    }

    pub fn tui_label_shared_runtime() -> &'static str {
        if is_chinese() {
            "共享 runtime"
        } else {
            "Shared runtime"
        }
    }

    pub fn tui_label_listen() -> &'static str {
        if is_chinese() {
            "监听"
        } else {
            "Listen"
        }
    }

    pub fn tui_label_uptime() -> &'static str {
        if is_chinese() {
            "运行时长"
        } else {
            "Uptime"
        }
    }

    pub fn tui_label_requests() -> &'static str {
        if is_chinese() {
            "请求"
        } else {
            "Requests"
        }
    }

    pub fn tui_label_traffic() -> &'static str {
        if is_chinese() {
            "流量"
        } else {
            "Traffic"
        }
    }

    pub fn tui_label_proxy_requests() -> &'static str {
        if is_chinese() {
            "代理总请求"
        } else {
            "Proxy requests"
        }
    }

    pub fn tui_label_active_target() -> &'static str {
        if is_chinese() {
            "当前路由目标"
        } else {
            "Active target"
        }
    }

    pub fn tui_label_last_error() -> &'static str {
        if is_chinese() {
            "最近错误"
        } else {
            "Last error"
        }
    }

    pub fn tui_label_last_proxy_error() -> &'static str {
        if is_chinese() {
            "最近一次代理错误"
        } else {
            "Last proxy error"
        }
    }

    pub fn tui_label_mcp_servers_active() -> &'static str {
        if is_chinese() {
            "已启用"
        } else {
            "Active"
        }
    }

    pub fn tui_na() -> &'static str {
        "N/A"
    }

    pub fn tui_loading() -> &'static str {
        if is_chinese() {
            "处理中…"
        } else {
            "Working…"
        }
    }

    pub fn tui_header_quota() -> &'static str {
        if is_chinese() {
            "额度"
        } else {
            "Quota"
        }
    }

    pub fn tui_label_quota() -> &'static str {
        if is_chinese() {
            "额度"
        } else {
            "Quota"
        }
    }

    pub fn tui_quota_loading() -> &'static str {
        if is_chinese() {
            "查询中…"
        } else {
            "checking…"
        }
    }

    pub fn tui_quota_not_available() -> &'static str {
        if is_chinese() {
            "不可用"
        } else {
            "not available"
        }
    }

    pub fn tui_quota_parse_error() -> &'static str {
        if is_chinese() {
            "凭据解析失败"
        } else {
            "credential parse failed"
        }
    }

    pub fn tui_quota_expired() -> &'static str {
        if is_chinese() {
            "登录过期"
        } else {
            "login expired"
        }
    }

    pub fn tui_quota_query_failed() -> &'static str {
        if is_chinese() {
            "查询失败"
        } else {
            "query failed"
        }
    }

    pub fn tui_quota_not_queried() -> &'static str {
        if is_chinese() {
            "未查询"
        } else {
            "not queried"
        }
    }

    pub fn tui_quota_refresh_hint() -> &'static str {
        if is_chinese() {
            "按 r 刷新"
        } else {
            "press r to refresh"
        }
    }

    pub fn tui_quota_ok() -> &'static str {
        if is_chinese() {
            "已获取"
        } else {
            "ok"
        }
    }

    pub fn tui_quota_last_checked() -> &'static str {
        if is_chinese() {
            "更新于"
        } else {
            "checked"
        }
    }

    pub fn tui_quota_resets_in(time: &str) -> String {
        if is_chinese() {
            format!("{time} 后重置")
        } else {
            format!("resets in {time}")
        }
    }

    pub fn tui_quota_just_now() -> &'static str {
        if is_chinese() {
            "刚刚"
        } else {
            "just now"
        }
    }

    pub fn tui_quota_minutes_ago(count: i64) -> String {
        if is_chinese() {
            format!("{count} 分钟前")
        } else if count == 1 {
            "1 minute ago".to_string()
        } else {
            format!("{count} minutes ago")
        }
    }

    pub fn tui_quota_hours_ago(count: i64) -> String {
        if is_chinese() {
            format!("{count} 小时前")
        } else if count == 1 {
            "1 hour ago".to_string()
        } else {
            format!("{count} hours ago")
        }
    }

    pub fn tui_quota_days_ago(count: i64) -> String {
        if is_chinese() {
            format!("{count} 天前")
        } else if count == 1 {
            "1 day ago".to_string()
        } else {
            format!("{count} days ago")
        }
    }

    pub fn tui_quota_extra_usage() -> &'static str {
        if is_chinese() {
            "额外用量"
        } else {
            "Extra usage"
        }
    }

    pub fn tui_quota_tier_five_hour() -> &'static str {
        if is_chinese() {
            "5小时"
        } else {
            "5h"
        }
    }

    pub fn tui_quota_tier_seven_day() -> &'static str {
        if is_chinese() {
            "7天"
        } else {
            "7d"
        }
    }

    pub fn tui_quota_tier_seven_day_opus() -> &'static str {
        if is_chinese() {
            "7天 Opus"
        } else {
            "7d Opus"
        }
    }

    pub fn tui_quota_tier_seven_day_sonnet() -> &'static str {
        if is_chinese() {
            "7天 Sonnet"
        } else {
            "7d Sonnet"
        }
    }

    pub fn tui_quota_tier_weekly_limit() -> &'static str {
        if is_chinese() {
            "周额度"
        } else {
            "weekly"
        }
    }

    pub fn tui_quota_tier_premium() -> &'static str {
        "premium"
    }

    pub fn tui_quota_tier_gemini_pro() -> &'static str {
        "Gemini Pro"
    }

    pub fn tui_quota_tier_gemini_flash() -> &'static str {
        "Gemini Flash"
    }

    pub fn tui_quota_tier_gemini_flash_lite() -> &'static str {
        "Gemini Flash Lite"
    }

    pub fn tui_header_id() -> &'static str {
        "ID"
    }

    pub fn tui_header_api_url() -> &'static str {
        "API URL"
    }

    pub fn tui_header_directory() -> &'static str {
        if is_chinese() {
            "目录"
        } else {
            "Directory"
        }
    }

    pub fn tui_header_repo() -> &'static str {
        if is_chinese() {
            "仓库"
        } else {
            "Repo"
        }
    }

    pub fn tui_header_branch() -> &'static str {
        if is_chinese() {
            "分支"
        } else {
            "Branch"
        }
    }

    pub fn tui_header_path() -> &'static str {
        if is_chinese() {
            "路径"
        } else {
            "Path"
        }
    }

    pub fn tui_header_found_in() -> &'static str {
        if is_chinese() {
            "发现于"
        } else {
            "Found In"
        }
    }

    pub fn tui_header_field() -> &'static str {
        if is_chinese() {
            "字段"
        } else {
            "Field"
        }
    }

    pub fn tui_header_value() -> &'static str {
        if is_chinese() {
            "值"
        } else {
            "Value"
        }
    }

    pub fn tui_header_claude_short() -> &'static str {
        "C"
    }

    pub fn tui_header_codex_short() -> &'static str {
        "X"
    }

    pub fn tui_header_gemini_short() -> &'static str {
        "G"
    }

    pub fn tui_header_opencode_short() -> &'static str {
        "O"
    }

    pub fn tui_label_id() -> &'static str {
        "ID"
    }

    pub fn tui_label_api_url() -> &'static str {
        "API URL"
    }

    pub fn tui_label_directory() -> &'static str {
        if is_chinese() {
            "目录"
        } else {
            "Directory"
        }
    }

    pub fn tui_label_enabled_for() -> &'static str {
        if is_chinese() {
            "已启用"
        } else {
            "Enabled"
        }
    }

    pub fn tui_label_repo() -> &'static str {
        if is_chinese() {
            "仓库"
        } else {
            "Repo"
        }
    }

    pub fn tui_label_readme() -> &'static str {
        if is_chinese() {
            "README"
        } else {
            "README"
        }
    }

    pub fn tui_label_base_url() -> &'static str {
        if is_chinese() {
            "API 请求地址"
        } else {
            "Base URL"
        }
    }

    pub fn tui_label_api_key() -> &'static str {
        if is_chinese() {
            "API Key"
        } else {
            "API Key"
        }
    }

    pub fn tui_label_claude_api_format() -> &'static str {
        if is_chinese() {
            "API 格式"
        } else {
            "API Format"
        }
    }

    pub fn tui_claude_api_format_value(api_format: &str) -> &'static str {
        match api_format {
            "openai_chat" => {
                if is_chinese() {
                    "OpenAI Chat Completions (需开启代理)"
                } else {
                    "OpenAI Chat Completions (Requires proxy)"
                }
            }
            "openai_responses" => {
                if is_chinese() {
                    "OpenAI Responses API (需开启代理)"
                } else {
                    "OpenAI Responses API (Requires proxy)"
                }
            }
            _ => {
                if is_chinese() {
                    "Anthropic Messages (原生)"
                } else {
                    "Anthropic Messages (Native)"
                }
            }
        }
    }

    pub fn tui_claude_api_format_requires_proxy_title() -> &'static str {
        if is_chinese() {
            "需开启代理"
        } else {
            "Proxy Required"
        }
    }

    pub fn tui_claude_api_format_requires_proxy_message(api_format: &str) -> String {
        let label = tui_claude_api_format_value(api_format);
        if is_chinese() {
            format!("已切换为 {label}。\n该格式需开启本地代理使用。\n请在主页按 P 打开本地代理。")
        } else {
            format!("Switched to {label}.\nThis format requires the local proxy.\nPress P on the home page to open local proxy.")
        }
    }

    pub fn tui_claude_api_format_popup_title() -> &'static str {
        if is_chinese() {
            "API 格式"
        } else {
            "API Format"
        }
    }

    pub fn tui_label_claude_model_config() -> &'static str {
        if is_chinese() {
            "Claude 模型配置"
        } else {
            "Claude Model Config"
        }
    }

    pub fn tui_label_claude_hide_attribution() -> &'static str {
        if is_chinese() {
            "隐藏 AI 署名"
        } else {
            "Hide AI Attribution"
        }
    }

    pub fn tui_label_provider_package() -> &'static str {
        if is_chinese() {
            "Provider / npm 包"
        } else {
            "Provider / npm"
        }
    }

    pub fn tui_label_openclaw_api() -> &'static str {
        if is_chinese() {
            "API 协议"
        } else {
            "API Mode"
        }
    }

    pub fn tui_label_openclaw_user_agent() -> &'static str {
        if is_chinese() {
            "发送 User-Agent"
        } else {
            "Send User-Agent"
        }
    }

    pub fn tui_label_openclaw_models() -> &'static str {
        if is_chinese() {
            "模型列表"
        } else {
            "Models"
        }
    }

    pub fn tui_label_openclaw_status() -> &'static str {
        if is_chinese() {
            "状态"
        } else {
            "Status"
        }
    }

    pub fn tui_opencode_config_status_label() -> &'static str {
        if is_chinese() {
            "OpenCode 配置"
        } else {
            "OpenCode Config"
        }
    }

    pub fn tui_label_provider_config_status() -> &'static str {
        if is_chinese() {
            "配置状态"
        } else {
            "Config Status"
        }
    }

    pub fn tui_provider_config_count(in_config: usize, total: usize) -> String {
        if is_chinese() {
            format!("{in_config}/{total} 已添加")
        } else {
            format!("{in_config}/{total} in config")
        }
    }

    pub fn tui_provider_status_in_config() -> &'static str {
        if is_chinese() {
            "已添加到配置"
        } else {
            "in config"
        }
    }

    pub fn tui_provider_status_saved_only() -> &'static str {
        if is_chinese() {
            "仅已保存"
        } else {
            "saved only"
        }
    }

    pub fn tui_provider_status_untracked() -> &'static str {
        if is_chinese() {
            "未跟踪"
        } else {
            "untracked"
        }
    }

    pub fn tui_label_openclaw_model() -> &'static str {
        if is_chinese() {
            "模型"
        } else {
            "Model"
        }
    }

    pub fn tui_openclaw_status_default() -> &'static str {
        if is_chinese() {
            "默认"
        } else {
            "default"
        }
    }

    pub fn tui_openclaw_status_in_config_and_saved() -> &'static str {
        if is_chinese() {
            "配置中 + 已保存"
        } else {
            "in config + saved"
        }
    }

    pub fn tui_openclaw_status_live_only() -> &'static str {
        if is_chinese() {
            "仅当前配置"
        } else {
            "live only"
        }
    }

    pub fn tui_openclaw_status_saved_only() -> &'static str {
        if is_chinese() {
            "仅已保存"
        } else {
            "saved only"
        }
    }

    pub fn tui_openclaw_status_untracked() -> &'static str {
        if is_chinese() {
            "未跟踪"
        } else {
            "untracked"
        }
    }

    pub fn tui_openclaw_models_summary(total: usize) -> String {
        if is_chinese() {
            if total == 0 {
                "未配置模型".to_string()
            } else {
                format!(
                    "{total} 个模型（1 主模型 + {} 回退）",
                    total.saturating_sub(1)
                )
            }
        } else if total == 0 {
            "No models configured".to_string()
        } else {
            format!(
                "{total} models (1 primary + {} fallbacks)",
                total.saturating_sub(1)
            )
        }
    }

    pub fn tui_openclaw_models_open_hint() -> &'static str {
        if is_chinese() {
            "按 Enter 编辑 OpenClaw 模型列表"
        } else {
            "Press Enter to edit OpenClaw models"
        }
    }

    pub fn tui_openclaw_models_editor_title() -> &'static str {
        if is_chinese() {
            "OpenClaw 模型列表"
        } else {
            "OpenClaw Models"
        }
    }

    pub fn tui_toast_json_must_be_array() -> &'static str {
        if is_chinese() {
            "JSON 必须是数组"
        } else {
            "JSON must be an array"
        }
    }

    pub fn tui_label_opencode_model_id() -> &'static str {
        if is_chinese() {
            "主模型 ID"
        } else {
            "Main Model ID"
        }
    }

    pub fn tui_label_opencode_model_name() -> &'static str {
        if is_chinese() {
            "主模型名称"
        } else {
            "Main Model Name"
        }
    }

    pub fn tui_label_context_limit() -> &'static str {
        if is_chinese() {
            "上下文限制"
        } else {
            "Context Limit"
        }
    }

    pub fn tui_label_output_limit() -> &'static str {
        if is_chinese() {
            "输出限制"
        } else {
            "Output Limit"
        }
    }

    pub fn tui_label_command() -> &'static str {
        if is_chinese() {
            "命令"
        } else {
            "Command"
        }
    }

    pub fn tui_label_mcp_type() -> &'static str {
        if is_chinese() {
            "连接类型"
        } else {
            "Transport"
        }
    }

    pub fn tui_label_url() -> &'static str {
        "URL"
    }

    pub fn tui_label_args() -> &'static str {
        if is_chinese() {
            "参数"
        } else {
            "Args"
        }
    }

    pub fn tui_label_env() -> &'static str {
        if is_chinese() {
            "环境变量"
        } else {
            "Env"
        }
    }

    pub fn tui_mcp_env_entry_count(count: usize) -> String {
        if is_chinese() {
            format!("{count} 项")
        } else if count == 1 {
            "1 entry".to_string()
        } else {
            format!("{count} entries")
        }
    }

    pub fn tui_mcp_env_editor_hint() -> &'static str {
        if is_chinese() {
            "按 Enter 管理环境变量"
        } else {
            "Press Enter to manage env entries"
        }
    }

    pub fn tui_mcp_type_editor_hint() -> &'static str {
        if is_chinese() {
            "按 Enter 选择连接类型"
        } else {
            "Press Enter to choose transport"
        }
    }

    pub fn tui_mcp_env_key_label() -> &'static str {
        if is_chinese() {
            "键"
        } else {
            "Key"
        }
    }

    pub fn tui_mcp_env_value_label() -> &'static str {
        if is_chinese() {
            "值"
        } else {
            "Value"
        }
    }

    pub fn tui_label_app_claude() -> &'static str {
        if is_chinese() {
            "应用: Claude"
        } else {
            "App: Claude"
        }
    }

    pub fn tui_label_app_codex() -> &'static str {
        if is_chinese() {
            "应用: Codex"
        } else {
            "App: Codex"
        }
    }

    pub fn tui_label_app_gemini() -> &'static str {
        if is_chinese() {
            "应用: Gemini"
        } else {
            "App: Gemini"
        }
    }

    pub fn tui_label_app_opencode() -> &'static str {
        if is_chinese() {
            "应用: OpenCode"
        } else {
            "App: OpenCode"
        }
    }

    pub fn tui_form_templates_title() -> &'static str {
        if is_chinese() {
            "模板"
        } else {
            "Templates"
        }
    }

    pub fn tui_form_common_config_button() -> &'static str {
        if is_chinese() {
            "通用配置"
        } else {
            "Common Config"
        }
    }

    pub fn tui_form_attach_common_config() -> &'static str {
        if is_chinese() {
            "添加通用配置"
        } else {
            "Attach Common Config"
        }
    }

    pub fn tui_form_fields_title() -> &'static str {
        if is_chinese() {
            "字段"
        } else {
            "Fields"
        }
    }

    pub fn tui_form_json_title() -> &'static str {
        "JSON"
    }

    pub fn tui_codex_auth_json_title() -> &'static str {
        if is_chinese() {
            "auth.json (JSON) *"
        } else {
            "auth.json (JSON) *"
        }
    }

    pub fn tui_codex_config_toml_title() -> &'static str {
        if is_chinese() {
            "config.toml (TOML)"
        } else {
            "config.toml (TOML)"
        }
    }

    pub fn tui_form_input_title() -> &'static str {
        if is_chinese() {
            "输入"
        } else {
            "Input"
        }
    }

    pub fn tui_form_editing_title() -> &'static str {
        if is_chinese() {
            "编辑中"
        } else {
            "Editing"
        }
    }

    pub fn tui_claude_model_config_popup_title() -> &'static str {
        if is_chinese() {
            "Claude 模型配置"
        } else {
            "Claude Model Configuration"
        }
    }

    pub fn tui_claude_model_main_label() -> &'static str {
        if is_chinese() {
            "主模型"
        } else {
            "Main Model"
        }
    }

    pub fn tui_claude_reasoning_model_label() -> &'static str {
        if is_chinese() {
            "推理模型 (Thinking)"
        } else {
            "Reasoning Model (Thinking)"
        }
    }

    pub fn tui_claude_default_haiku_model_label() -> &'static str {
        if is_chinese() {
            "默认 Haiku 模型"
        } else {
            "Default Haiku Model"
        }
    }

    pub fn tui_claude_default_sonnet_model_label() -> &'static str {
        if is_chinese() {
            "默认 Sonnet 模型"
        } else {
            "Default Sonnet Model"
        }
    }

    pub fn tui_claude_default_opus_model_label() -> &'static str {
        if is_chinese() {
            "默认 Opus 模型"
        } else {
            "Default Opus Model"
        }
    }

    pub fn tui_claude_model_config_summary(configured_count: usize) -> String {
        if is_chinese() {
            format!("已配置 {configured_count}/5")
        } else {
            format!("Configured {configured_count}/5")
        }
    }

    pub fn tui_claude_model_config_open_hint() -> &'static str {
        if is_chinese() {
            "按 Enter 配置 Claude 模型"
        } else {
            "Press Enter to configure Claude models"
        }
    }

    pub fn tui_hint_press() -> &'static str {
        if is_chinese() {
            "按 "
        } else {
            "Press "
        }
    }

    pub fn tui_hint_auto_fetch_models_from_api() -> &'static str {
        if is_chinese() {
            " 从 API 自动获取模型。"
        } else {
            " to auto-fetch models from API."
        }
    }

    pub fn tui_model_fetch_popup_title(fetching: bool) -> String {
        if is_chinese() {
            if fetching {
                "选择模型 (获取中...)".to_string()
            } else {
                "选择模型".to_string()
            }
        } else {
            if fetching {
                "Select Model (Fetching...)".to_string()
            } else {
                "Select Model".to_string()
            }
        }
    }

    pub fn tui_model_fetch_search_placeholder() -> &'static str {
        if is_chinese() {
            "输入过滤 或 直接回车使用输入值..."
        } else {
            "Type to filter, or press Enter to use input..."
        }
    }

    pub fn tui_model_fetch_search_title() -> &'static str {
        if is_chinese() {
            "模型搜索"
        } else {
            "Model Search"
        }
    }

    pub fn tui_model_fetch_no_models() -> &'static str {
        if is_chinese() {
            "没有获取到模型 (可直接输入并在此回车)"
        } else {
            "No models found (type custom and press Enter)"
        }
    }

    pub fn tui_model_fetch_no_matches() -> &'static str {
        if is_chinese() {
            "没有匹配结果 (可直接输入并在此回车)"
        } else {
            "No matching models (press Enter to use input)"
        }
    }

    pub fn tui_model_fetch_error_hint(err: &str) -> String {
        if is_chinese() {
            format!("获取失败: {}", err)
        } else {
            format!("Fetch failed: {}", err)
        }
    }

    pub fn tui_provider_not_found() -> &'static str {
        if is_chinese() {
            "未找到该供应商。"
        } else {
            "Provider not found."
        }
    }

    pub fn tui_provider_title() -> &'static str {
        if is_chinese() {
            "供应商"
        } else {
            "Provider"
        }
    }

    pub fn tui_provider_detail_title() -> &'static str {
        if is_chinese() {
            "供应商详情"
        } else {
            "Provider Detail"
        }
    }

    pub fn tui_provider_add_title() -> &'static str {
        if is_chinese() {
            "新增供应商"
        } else {
            "Add Provider"
        }
    }

    pub fn tui_codex_official_no_api_key_tip() -> &'static str {
        if is_chinese() {
            "官方无需填写 API Key，直接保存即可。"
        } else {
            "Official provider doesn't require an API key. Just save."
        }
    }

    pub fn tui_toast_codex_official_auth_json_disabled() -> &'static str {
        if is_chinese() {
            "官方模式下不支持编辑 auth.json（切换时会移除）。"
        } else {
            "auth.json editing is disabled for the official provider (it will be removed on switch)."
        }
    }

    pub fn tui_provider_edit_title(name: &str) -> String {
        if is_chinese() {
            format!("编辑供应商: {name}")
        } else {
            format!("Edit Provider: {name}")
        }
    }

    pub fn tui_provider_detail_keys() -> &'static str {
        if is_chinese() {
            "按键：s=切换  e=编辑  t=测速  c=健康检查"
        } else {
            "Keys: s=switch  e=edit  t=speedtest  c=stream check"
        }
    }

    pub fn tui_key_switch() -> &'static str {
        if is_chinese() {
            "切换"
        } else {
            "switch"
        }
    }

    pub fn tui_key_add_remove() -> &'static str {
        if is_chinese() {
            "添加/移除"
        } else {
            "add/remove"
        }
    }

    pub fn tui_key_set_default() -> &'static str {
        if is_chinese() {
            "设为默认"
        } else {
            "set default"
        }
    }

    pub fn tui_key_edit() -> &'static str {
        if is_chinese() {
            "编辑"
        } else {
            "edit"
        }
    }

    pub fn tui_key_speedtest() -> &'static str {
        if is_chinese() {
            "测速"
        } else {
            "speedtest"
        }
    }

    pub fn tui_key_stream_check() -> &'static str {
        if is_chinese() {
            "健康检查"
        } else {
            "stream check"
        }
    }

    pub fn tui_key_launch_temp() -> &'static str {
        if is_chinese() {
            "临时启动"
        } else {
            "launch temp"
        }
    }

    pub fn tui_stream_check_status_operational() -> &'static str {
        if is_chinese() {
            "正常"
        } else {
            "operational"
        }
    }

    pub fn tui_stream_check_status_degraded() -> &'static str {
        if is_chinese() {
            "降级"
        } else {
            "degraded"
        }
    }

    pub fn tui_stream_check_status_failed() -> &'static str {
        if is_chinese() {
            "失败"
        } else {
            "failed"
        }
    }

    pub fn tui_key_details() -> &'static str {
        if is_chinese() {
            "详情"
        } else {
            "details"
        }
    }

    pub fn tui_key_view() -> &'static str {
        if is_chinese() {
            "查看"
        } else {
            "view"
        }
    }

    pub fn tui_key_add() -> &'static str {
        if is_chinese() {
            "新增"
        } else {
            "add"
        }
    }

    pub fn tui_key_delete() -> &'static str {
        if is_chinese() {
            "删除"
        } else {
            "delete"
        }
    }

    pub fn tui_key_import() -> &'static str {
        if is_chinese() {
            "导入"
        } else {
            "import"
        }
    }

    pub fn tui_key_install() -> &'static str {
        if is_chinese() {
            "安装"
        } else {
            "install"
        }
    }

    pub fn tui_key_uninstall() -> &'static str {
        if is_chinese() {
            "卸载"
        } else {
            "uninstall"
        }
    }

    pub fn tui_key_discover() -> &'static str {
        if is_chinese() {
            "发现"
        } else {
            "discover"
        }
    }

    pub fn tui_key_unmanaged() -> &'static str {
        if is_chinese() {
            "已有"
        } else {
            "existing"
        }
    }

    pub fn tui_key_repos() -> &'static str {
        if is_chinese() {
            "仓库"
        } else {
            "repos"
        }
    }

    pub fn tui_key_sync() -> &'static str {
        if is_chinese() {
            "同步"
        } else {
            "sync"
        }
    }

    pub fn tui_key_sync_method() -> &'static str {
        if is_chinese() {
            "同步方式"
        } else {
            "sync method"
        }
    }

    pub fn tui_key_search() -> &'static str {
        if is_chinese() {
            "搜索"
        } else {
            "search"
        }
    }

    pub fn tui_key_refresh() -> &'static str {
        if is_chinese() {
            "刷新"
        } else {
            "refresh"
        }
    }

    pub fn tui_key_start_proxy() -> &'static str {
        if is_chinese() {
            "启动代理"
        } else {
            "start proxy"
        }
    }

    pub fn tui_key_stop_proxy() -> &'static str {
        if is_chinese() {
            "停止代理"
        } else {
            "stop proxy"
        }
    }

    pub fn tui_key_proxy_on() -> &'static str {
        if is_chinese() {
            "代理开"
        } else {
            "proxy on"
        }
    }

    pub fn tui_key_proxy_off() -> &'static str {
        if is_chinese() {
            "代理关"
        } else {
            "proxy off"
        }
    }

    pub fn tui_key_focus() -> &'static str {
        if is_chinese() {
            "切换窗口"
        } else {
            "next pane"
        }
    }

    pub fn tui_key_toggle() -> &'static str {
        if is_chinese() {
            "启用/禁用"
        } else {
            "toggle"
        }
    }

    pub fn tui_key_apps() -> &'static str {
        if is_chinese() {
            "应用"
        } else {
            "apps"
        }
    }

    pub fn tui_key_activate() -> &'static str {
        if is_chinese() {
            "激活"
        } else {
            "activate"
        }
    }

    pub fn tui_key_deactivate() -> &'static str {
        if is_chinese() {
            "取消激活"
        } else {
            "deactivate"
        }
    }

    pub fn tui_key_open() -> &'static str {
        if is_chinese() {
            "打开"
        } else {
            "open"
        }
    }

    pub fn tui_key_open_directory() -> &'static str {
        if is_chinese() {
            "打开目录"
        } else {
            "open dir"
        }
    }

    pub fn tui_key_create() -> &'static str {
        if is_chinese() {
            "新建"
        } else {
            "create"
        }
    }

    pub fn tui_key_apply() -> &'static str {
        if is_chinese() {
            "应用"
        } else {
            "apply"
        }
    }

    pub fn tui_key_edit_snippet() -> &'static str {
        if is_chinese() {
            "编辑片段"
        } else {
            "edit snippet"
        }
    }

    pub fn tui_key_close() -> &'static str {
        if is_chinese() {
            "关闭"
        } else {
            "close"
        }
    }

    pub fn tui_key_exit() -> &'static str {
        if is_chinese() {
            "退出"
        } else {
            "exit"
        }
    }

    pub fn tui_key_cancel() -> &'static str {
        if is_chinese() {
            "取消"
        } else {
            "cancel"
        }
    }

    pub fn tui_key_submit() -> &'static str {
        if is_chinese() {
            "提交"
        } else {
            "submit"
        }
    }

    pub fn tui_key_yes() -> &'static str {
        if is_chinese() {
            "确认"
        } else {
            "confirm"
        }
    }

    pub fn tui_key_no() -> &'static str {
        if is_chinese() {
            "返回"
        } else {
            "back"
        }
    }

    pub fn tui_key_scroll() -> &'static str {
        if is_chinese() {
            "滚动"
        } else {
            "scroll"
        }
    }

    pub fn tui_key_restore() -> &'static str {
        if is_chinese() {
            "恢复"
        } else {
            "restore"
        }
    }

    pub fn tui_key_takeover() -> &'static str {
        if is_chinese() {
            "接管"
        } else {
            "take over"
        }
    }

    pub fn tui_key_save() -> &'static str {
        if is_chinese() {
            "保存"
        } else {
            "save"
        }
    }

    pub fn tui_key_external_editor() -> &'static str {
        if is_chinese() {
            "外部编辑器"
        } else {
            "external editor"
        }
    }

    pub fn tui_key_save_and_exit() -> &'static str {
        if is_chinese() {
            "保存并退出"
        } else {
            "save & exit"
        }
    }

    pub fn tui_key_exit_without_save() -> &'static str {
        if is_chinese() {
            "不保存退出"
        } else {
            "exit w/o save"
        }
    }

    pub fn tui_key_edit_mode() -> &'static str {
        if is_chinese() {
            "编辑"
        } else {
            "edit"
        }
    }

    pub fn tui_key_clear() -> &'static str {
        if is_chinese() {
            "清除"
        } else {
            "clear"
        }
    }

    pub fn tui_key_move() -> &'static str {
        if is_chinese() {
            "移动"
        } else {
            "move"
        }
    }

    pub fn tui_key_exit_edit() -> &'static str {
        if is_chinese() {
            "退出编辑"
        } else {
            "exit edit"
        }
    }

    pub fn tui_key_select() -> &'static str {
        if is_chinese() {
            "选择"
        } else {
            "select"
        }
    }

    pub fn tui_key_fetch_model() -> &'static str {
        if is_chinese() {
            "获取模型"
        } else {
            "fetch model"
        }
    }

    pub fn tui_key_deactivate_active() -> &'static str {
        if is_chinese() {
            "取消激活(当前)"
        } else {
            "deactivate active"
        }
    }

    pub fn tui_provider_list_keys() -> &'static str {
        if is_chinese() {
            "按键：a=新增  e=编辑  Enter=详情  s=切换  /=搜索"
        } else {
            "Keys: a=add  e=edit  Enter=details  s=switch  /=filter"
        }
    }

    pub fn tui_home_ascii_logo() -> &'static str {
        // Same ASCII art across languages.
        r#"                                  _  _         _
   ___  ___        ___ __      __(_)| |_  ___ | |__
  / __|/ __|_____ / __|\ \ /\ / /| || __|/ __|| '_ \
 | (__| (__|_____|\__ \ \ V  V / | || |_| (__ | | | |
  \___|\___|      |___/  \_/\_/  |_| \__|\___||_| |_|
                                                      "#
    }

    pub fn tui_common_snippet_keys() -> &'static str {
        if is_chinese() {
            "按键：e=编辑  c=清除  a=应用  Esc=返回"
        } else {
            "Keys: e=edit  c=clear  a=apply  Esc=back"
        }
    }

    pub fn tui_view_config_app(app: &str) -> String {
        if is_chinese() {
            format!("应用: {}", app)
        } else {
            format!("App: {}", app)
        }
    }

    pub fn tui_view_config_provider(provider: &str) -> String {
        if is_chinese() {
            format!("供应商: {}", provider)
        } else {
            format!("Provider: {}", provider)
        }
    }

    pub fn tui_view_config_api_url(url: &str) -> String {
        if is_chinese() {
            format!("API URL:  {}", url)
        } else {
            format!("API URL:  {}", url)
        }
    }

    pub fn tui_view_config_mcp_servers(enabled: usize, total: usize) -> String {
        if is_chinese() {
            format!("MCP 服务器: {} 启用 / {} 总数", enabled, total)
        } else {
            format!("MCP servers: {} enabled / {} total", enabled, total)
        }
    }

    pub fn tui_view_config_prompts(active: &str) -> String {
        if is_chinese() {
            format!("提示词: {}", active)
        } else {
            format!("Prompts: {}", active)
        }
    }

    pub fn tui_view_config_config_file(path: &str) -> String {
        if is_chinese() {
            format!("配置文件: {}", path)
        } else {
            format!("Config file: {}", path)
        }
    }

    pub fn tui_settings_header_language() -> &'static str {
        if is_chinese() {
            "语言"
        } else {
            "Language"
        }
    }

    pub fn tui_settings_header_setting() -> &'static str {
        if is_chinese() {
            "设置项"
        } else {
            "Setting"
        }
    }

    pub fn tui_settings_header_value() -> &'static str {
        if is_chinese() {
            "值"
        } else {
            "Value"
        }
    }

    pub fn tui_settings_title() -> &'static str {
        if is_chinese() {
            "设置"
        } else {
            "Settings"
        }
    }

    pub fn tui_settings_visible_apps_label() -> &'static str {
        if is_chinese() {
            "可见应用"
        } else {
            "Visible Apps"
        }
    }

    pub fn tui_settings_visible_apps_title() -> &'static str {
        if is_chinese() {
            "选择可见应用"
        } else {
            "Choose Visible Apps"
        }
    }

    pub fn tui_settings_proxy_title() -> &'static str {
        if is_chinese() {
            "本地代理"
        } else {
            "Local Proxy"
        }
    }

    pub fn tui_settings_proxy_listen_address_label() -> &'static str {
        if is_chinese() {
            "监听地址"
        } else {
            "Listen Address"
        }
    }

    pub fn tui_settings_proxy_listen_port_label() -> &'static str {
        if is_chinese() {
            "监听端口"
        } else {
            "Listen Port"
        }
    }

    pub fn tui_settings_proxy_listen_address_prompt() -> &'static str {
        if is_chinese() {
            "输入监听地址（如 127.0.0.1 / localhost / 0.0.0.0）"
        } else {
            "Enter listen address (for example 127.0.0.1 / localhost / 0.0.0.0)"
        }
    }

    pub fn tui_settings_proxy_listen_port_prompt() -> &'static str {
        if is_chinese() {
            "输入监听端口（1024-65535）"
        } else {
            "Enter listen port (1024-65535)"
        }
    }

    pub fn tui_settings_openclaw_config_dir_label() -> &'static str {
        if is_chinese() {
            "OpenClaw 配置目录"
        } else {
            "OpenClaw Config Directory"
        }
    }

    pub fn tui_settings_openclaw_config_dir_prompt() -> &'static str {
        if is_chinese() {
            "输入 OpenClaw 配置目录；留空恢复默认 ~/.openclaw"
        } else {
            "Enter the OpenClaw config directory; leave empty to use ~/.openclaw"
        }
    }

    pub fn tui_settings_openclaw_config_dir_default_value() -> &'static str {
        "Default (~/.openclaw)"
    }

    pub fn tui_settings_proxy_restart_hint() -> &'static str {
        if is_chinese() {
            "修改监听地址或端口后，需先停止并重新开启本地代理才能生效"
        } else {
            "Changes to listen address or port require stopping and restarting the local proxy"
        }
    }

    pub fn tui_settings_proxy_stop_before_edit_hint() -> &'static str {
        if is_chinese() {
            "请先停止本地代理，再修改监听地址或端口"
        } else {
            "Stop the local proxy before editing listen address or port"
        }
    }

    pub fn tui_toast_proxy_listen_address_invalid() -> &'static str {
        if is_chinese() {
            "地址无效，请输入有效的 IPv4 地址、localhost 或 0.0.0.0"
        } else {
            "Invalid address. Enter a valid IPv4 address, localhost, or 0.0.0.0"
        }
    }

    pub fn tui_toast_proxy_listen_port_invalid() -> &'static str {
        if is_chinese() {
            "端口无效，请输入 1024-65535 之间的数字"
        } else {
            "Invalid port. Enter a number between 1024 and 65535"
        }
    }

    pub fn tui_toast_proxy_settings_saved() -> &'static str {
        if is_chinese() {
            "本地代理配置已保存。"
        } else {
            "Local proxy settings saved."
        }
    }

    pub fn tui_toast_proxy_settings_restart_required() -> &'static str {
        if is_chinese() {
            "本地代理正在运行；新监听地址/端口会在重启代理后生效。"
        } else {
            "The local proxy is running; the new listen address/port will apply after restart."
        }
    }

    pub fn tui_toast_proxy_settings_stop_before_edit() -> &'static str {
        if is_chinese() {
            "本地代理正在运行。请先停止代理，再修改监听地址或端口。"
        } else {
            "The local proxy is running. Stop it before editing listen address or port."
        }
    }

    pub fn tui_toast_openclaw_config_dir_saved() -> &'static str {
        if is_chinese() {
            "OpenClaw 配置目录已保存。"
        } else {
            "OpenClaw config directory saved."
        }
    }

    pub fn tui_toast_openclaw_config_dir_sync_skipped() -> &'static str {
        if is_chinese() {
            "目标 OpenClaw 目录尚未初始化；已保存设置，但暂未同步 live 配置。"
        } else {
            "The target OpenClaw directory is not initialized yet; the setting was saved but live sync was skipped."
        }
    }

    pub fn tui_toast_openclaw_config_dir_sync_failed(err: &str) -> String {
        if is_chinese() {
            format!("OpenClaw 配置目录已保存，但同步 live 配置失败: {err}")
        } else {
            format!("OpenClaw config directory saved, but live sync failed: {err}")
        }
    }

    pub fn tui_toast_visible_apps_zero_selection_warning() -> &'static str {
        if is_chinese() {
            "至少保留一个可见应用。"
        } else {
            "Keep at least one app visible."
        }
    }

    pub fn tui_toast_visible_apps_saved() -> &'static str {
        if is_chinese() {
            "可见应用已保存。"
        } else {
            "Visible apps saved."
        }
    }

    pub fn tui_config_title() -> &'static str {
        if is_chinese() {
            "配置"
        } else {
            "Configuration"
        }
    }

    // ---------------------------------------------------------------------
    // Ratatui TUI - Skills
    // ---------------------------------------------------------------------

    pub fn tui_skills_install_title() -> &'static str {
        if is_chinese() {
            "安装 Skill"
        } else {
            "Install Skill"
        }
    }

    pub fn tui_skills_install_prompt() -> &'static str {
        if is_chinese() {
            "输入技能目录，或完整标识（owner/name:directory）："
        } else {
            "Enter a skill directory, or a full key (owner/name:directory):"
        }
    }

    pub fn tui_skills_uninstall_title() -> &'static str {
        if is_chinese() {
            "卸载 Skill"
        } else {
            "Uninstall Skill"
        }
    }

    pub fn tui_confirm_uninstall_skill_message(name: &str, directory: &str) -> String {
        if is_chinese() {
            format!("确认卸载 '{name}'（{directory}）？")
        } else {
            format!("Uninstall '{name}' ({directory})?")
        }
    }

    pub fn tui_skills_discover_title() -> &'static str {
        if is_chinese() {
            "发现 Skills"
        } else {
            "Discover Skills"
        }
    }

    pub fn tui_skills_discover_prompt() -> &'static str {
        if is_chinese() {
            "输入关键词（留空显示全部）："
        } else {
            "Enter a keyword (leave empty to show all):"
        }
    }

    pub fn tui_skills_discover_query_empty() -> &'static str {
        if is_chinese() {
            "全部"
        } else {
            "all"
        }
    }

    pub fn tui_skills_discover_hint() -> &'static str {
        if is_chinese() {
            "按 f 搜索仓库里的技能，按 r 管理技能仓库。"
        } else {
            "Press f to search skills from enabled repositories, or r to manage repositories."
        }
    }

    pub fn tui_skills_repos_title() -> &'static str {
        if is_chinese() {
            "Skill 仓库"
        } else {
            "Skill Repositories"
        }
    }

    pub fn tui_skills_repos_hint() -> &'static str {
        if is_chinese() {
            "技能发现会从这里已启用的仓库加载列表。"
        } else {
            "Skill discovery loads results from the repositories enabled here."
        }
    }

    pub fn tui_skills_repos_empty() -> &'static str {
        if is_chinese() {
            "未配置任何 Skill 仓库。按 a 添加。"
        } else {
            "No skill repositories configured. Press a to add."
        }
    }

    pub fn tui_skills_repos_add_title() -> &'static str {
        if is_chinese() {
            "添加仓库"
        } else {
            "Add Repository"
        }
    }

    pub fn tui_skills_repos_add_prompt() -> &'static str {
        if is_chinese() {
            "输入 GitHub 仓库（owner/name，可选 @branch）或完整 URL："
        } else {
            "Enter a GitHub repository (owner/name, optional @branch) or a full URL:"
        }
    }

    pub fn tui_skills_repos_remove_title() -> &'static str {
        if is_chinese() {
            "移除仓库"
        } else {
            "Remove Repository"
        }
    }

    pub fn tui_confirm_remove_repo_message(owner: &str, name: &str) -> String {
        let repo = format!("{owner}/{name}");
        if is_chinese() {
            format!("确认移除仓库 '{repo}'？")
        } else {
            format!("Remove repository '{repo}'?")
        }
    }

    pub fn tui_skills_unmanaged_title() -> &'static str {
        tui_skills_import_title()
    }

    pub fn tui_skills_import_title() -> &'static str {
        if is_chinese() {
            "导入已有技能"
        } else {
            "Import Existing Skills"
        }
    }

    pub fn tui_skills_unmanaged_hint() -> &'static str {
        tui_skills_import_description()
    }

    pub fn tui_skills_import_description() -> &'static str {
        if is_chinese() {
            "选择要导入到 CC Switch 统一管理的技能。"
        } else {
            "Select skills to import into CC Switch unified management."
        }
    }

    pub fn tui_skills_unmanaged_empty() -> &'static str {
        if is_chinese() {
            "未发现可导入的技能。"
        } else {
            "No skills to import found."
        }
    }

    pub fn tui_skills_detail_title() -> &'static str {
        if is_chinese() {
            "Skill 详情"
        } else {
            "Skill Detail"
        }
    }

    pub fn tui_skill_not_found() -> &'static str {
        if is_chinese() {
            "未找到该 Skill。"
        } else {
            "Skill not found."
        }
    }

    pub fn tui_skills_sync_method_label() -> &'static str {
        if is_chinese() {
            "同步方式"
        } else {
            "Sync"
        }
    }

    pub fn tui_skills_sync_method_title() -> &'static str {
        if is_chinese() {
            "选择同步方式"
        } else {
            "Select Sync Method"
        }
    }

    pub fn tui_skills_sync_method_name(method: crate::services::skill::SyncMethod) -> &'static str {
        match method {
            crate::services::skill::SyncMethod::Auto => {
                if is_chinese() {
                    "自动（优先使用链接，失败时复制）"
                } else {
                    "Automatic (prefer links, fall back to copy)"
                }
            }
            crate::services::skill::SyncMethod::Symlink => {
                if is_chinese() {
                    "仅链接"
                } else {
                    "Links only"
                }
            }
            crate::services::skill::SyncMethod::Copy => {
                if is_chinese() {
                    "仅复制"
                } else {
                    "Copy only"
                }
            }
        }
    }

    pub fn tui_skills_installed_summary(installed: usize, enabled: usize, app: &str) -> String {
        if is_chinese() {
            format!("已安装: {installed}   当前应用({app})已启用: {enabled}")
        } else {
            format!("Installed: {installed}   Enabled for {app}: {enabled}")
        }
    }

    pub fn tui_skills_installed_counts(
        claude: usize,
        codex: usize,
        gemini: usize,
        opencode: usize,
    ) -> String {
        if is_chinese() {
            format!(
                "已安装 · Claude: {claude} · Codex: {codex} · Gemini: {gemini} · OpenCode: {opencode}"
            )
        } else {
            format!(
                "Installed · Claude: {claude} · Codex: {codex} · Gemini: {gemini} · OpenCode: {opencode}"
            )
        }
    }

    pub fn tui_mcp_server_counts(
        claude: usize,
        codex: usize,
        gemini: usize,
        opencode: usize,
    ) -> String {
        if is_chinese() {
            format!(
                "已安装 · Claude: {claude} · Codex: {codex} · Gemini: {gemini} · OpenCode: {opencode}"
            )
        } else {
            format!(
                "Installed · Claude: {claude} · Codex: {codex} · Gemini: {gemini} · OpenCode: {opencode}"
            )
        }
    }

    pub fn tui_mcp_action_import_existing() -> &'static str {
        if is_chinese() {
            "导入已有"
        } else {
            "Import Existing"
        }
    }

    pub fn tui_skills_action_import_existing() -> &'static str {
        if is_chinese() {
            "导入已有"
        } else {
            "Import Existing"
        }
    }

    pub fn tui_skills_empty_title() -> &'static str {
        if is_chinese() {
            "暂无已安装的技能"
        } else {
            "No installed skills"
        }
    }

    pub fn tui_skills_empty_subtitle() -> &'static str {
        if is_chinese() {
            "从仓库发现并安装技能，或导入已有技能。"
        } else {
            "Discover and install skills from repositories, or import existing skills."
        }
    }

    pub fn tui_skills_empty_hint() -> &'static str {
        if is_chinese() {
            "暂无已安装技能。按 f 发现新技能，或按 i 导入已有技能。"
        } else {
            "No installed skills. Press f to discover skills, or i to import existing skills."
        }
    }

    pub fn tui_config_item_export() -> &'static str {
        if is_chinese() {
            "导出配置"
        } else {
            "Export Config"
        }
    }

    pub fn tui_config_item_import() -> &'static str {
        if is_chinese() {
            "导入配置"
        } else {
            "Import Config"
        }
    }

    pub fn tui_config_item_backup() -> &'static str {
        if is_chinese() {
            "备份配置"
        } else {
            "Backup Config"
        }
    }

    pub fn tui_config_item_restore() -> &'static str {
        if is_chinese() {
            "恢复配置"
        } else {
            "Restore Config"
        }
    }

    pub fn tui_config_item_validate() -> &'static str {
        if is_chinese() {
            "验证配置"
        } else {
            "Validate Config"
        }
    }

    pub fn tui_config_item_common_snippet() -> &'static str {
        if is_chinese() {
            "通用配置片段"
        } else {
            "Common Config Snippet"
        }
    }

    pub fn tui_config_item_proxy() -> &'static str {
        if is_chinese() {
            "本地代理"
        } else {
            "Local Proxy"
        }
    }

    pub fn tui_config_item_openclaw_env() -> &'static str {
        if is_chinese() {
            "环境变量"
        } else {
            "Env Variables"
        }
    }

    pub fn tui_config_item_openclaw_workspace() -> &'static str {
        if is_chinese() {
            "Workspace 文件管理"
        } else {
            "Workspace Files"
        }
    }

    pub fn tui_config_item_openclaw_tools() -> &'static str {
        if is_chinese() {
            "工具权限"
        } else {
            "Tool Permissions"
        }
    }

    pub fn tui_config_item_openclaw_agents() -> &'static str {
        if is_chinese() {
            "Agents 配置"
        } else {
            "Agents Config"
        }
    }

    pub fn tui_config_item_webdav_sync() -> &'static str {
        if is_chinese() {
            "WebDAV 同步"
        } else {
            "WebDAV Sync"
        }
    }

    pub fn tui_config_item_webdav_settings() -> &'static str {
        if is_chinese() {
            "WebDAV 同步设置（JSON）"
        } else {
            "WebDAV Sync Settings (JSON)"
        }
    }

    pub fn tui_config_item_webdav_check_connection() -> &'static str {
        if is_chinese() {
            "WebDAV 检查连接"
        } else {
            "WebDAV Check Connection"
        }
    }

    pub fn tui_config_item_webdav_upload() -> &'static str {
        if is_chinese() {
            "WebDAV 上传到远端"
        } else {
            "WebDAV Upload to Remote"
        }
    }

    pub fn tui_config_item_webdav_download() -> &'static str {
        if is_chinese() {
            "WebDAV 下载到本地"
        } else {
            "WebDAV Download to Local"
        }
    }

    pub fn tui_config_item_webdav_reset() -> &'static str {
        if is_chinese() {
            "重置 WebDAV 配置"
        } else {
            "Reset WebDAV Settings"
        }
    }

    pub fn tui_config_item_webdav_jianguoyun_quick_setup() -> &'static str {
        if is_chinese() {
            "坚果云一键配置"
        } else {
            "Jianguoyun Quick Setup"
        }
    }

    pub fn tui_webdav_settings_editor_title() -> &'static str {
        if is_chinese() {
            "编辑 WebDAV 同步设置（JSON）"
        } else {
            "Edit WebDAV Sync Settings (JSON)"
        }
    }

    pub fn tui_config_webdav_title() -> &'static str {
        if is_chinese() {
            "WebDAV 同步"
        } else {
            "WebDAV Sync"
        }
    }

    pub fn tui_openclaw_config_env_title() -> &'static str {
        tui_config_item_openclaw_env()
    }

    pub fn tui_openclaw_workspace_title() -> &'static str {
        tui_config_item_openclaw_workspace()
    }

    pub fn tui_openclaw_workspace_files_block_title() -> &'static str {
        if is_chinese() {
            "Workspace 文件"
        } else {
            "Workspace Files"
        }
    }

    pub fn tui_openclaw_workspace_directory_label() -> &'static str {
        if is_chinese() {
            "工作区目录"
        } else {
            "Workspace directory"
        }
    }

    pub fn tui_openclaw_workspace_daily_memory_label() -> &'static str {
        if is_chinese() {
            "Daily Memory"
        } else {
            "Daily Memory"
        }
    }

    pub fn tui_openclaw_workspace_daily_memory_count(count: usize) -> String {
        if is_chinese() {
            format!("{count} 个文件")
        } else if count == 1 {
            "1 file".to_string()
        } else {
            format!("{count} files")
        }
    }

    pub fn tui_openclaw_workspace_status_exists() -> &'static str {
        if is_chinese() {
            "已存在"
        } else {
            "Exists"
        }
    }

    pub fn tui_openclaw_workspace_status_missing() -> &'static str {
        if is_chinese() {
            "缺失"
        } else {
            "Missing"
        }
    }

    pub fn tui_openclaw_config_tools_title() -> &'static str {
        tui_config_item_openclaw_tools()
    }

    pub fn tui_openclaw_tools_description() -> &'static str {
        if is_chinese() {
            "管理 openclaw.json 中的工具权限配置（允许/拒绝列表）"
        } else {
            "Manage tool permissions in openclaw.json (allow/deny lists)"
        }
    }

    pub fn tui_openclaw_tools_profile_block_title() -> &'static str {
        if is_chinese() {
            "权限档位"
        } else {
            "Permission Profile"
        }
    }

    pub fn tui_openclaw_tools_rules_block_title() -> &'static str {
        if is_chinese() {
            "规则列表"
        } else {
            "Rule Lists"
        }
    }

    pub fn tui_openclaw_tools_profile_label() -> &'static str {
        if is_chinese() {
            "配置档位"
        } else {
            "Profile"
        }
    }

    pub fn tui_openclaw_tools_profile_unset() -> &'static str {
        if is_chinese() {
            "未设置"
        } else {
            "Not set"
        }
    }

    pub fn tui_openclaw_tools_profile_minimal() -> &'static str {
        if is_chinese() {
            "最小权限"
        } else {
            "Minimal"
        }
    }

    pub fn tui_openclaw_tools_profile_coding() -> &'static str {
        if is_chinese() {
            "编码"
        } else {
            "Coding"
        }
    }

    pub fn tui_openclaw_tools_profile_messaging() -> &'static str {
        if is_chinese() {
            "对话"
        } else {
            "Messaging"
        }
    }

    pub fn tui_openclaw_tools_profile_full() -> &'static str {
        if is_chinese() {
            "完全访问"
        } else {
            "Full"
        }
    }

    pub fn tui_openclaw_tools_unsupported_profile_title() -> &'static str {
        if is_chinese() {
            "检测到不受支持的工具配置"
        } else {
            "Unsupported tools profile detected"
        }
    }

    pub fn tui_openclaw_tools_unsupported_profile_description(value: &str) -> String {
        if is_chinese() {
            format!(
                "当前 tools.profile 的值“{value}”不在 OpenClaw 支持列表内。在你手动选择新值之前，它会被保留。"
            )
        } else {
            format!(
                "The current tools.profile value '{value}' is not in the supported OpenClaw list. It will be preserved until you choose a new value."
            )
        }
    }

    pub fn tui_openclaw_tools_unsupported_profile_label() -> &'static str {
        if is_chinese() {
            "不受支持"
        } else {
            "unsupported"
        }
    }

    pub fn tui_openclaw_tools_allow_list_label() -> &'static str {
        if is_chinese() {
            "允许列表"
        } else {
            "Allow List"
        }
    }

    pub fn tui_openclaw_tools_deny_list_label() -> &'static str {
        if is_chinese() {
            "拒绝列表"
        } else {
            "Deny List"
        }
    }

    pub fn tui_openclaw_tools_pattern_placeholder() -> &'static str {
        if is_chinese() {
            "工具名称或模式"
        } else {
            "Tool name or pattern"
        }
    }

    pub fn tui_openclaw_tools_add_allow_rule() -> &'static str {
        if is_chinese() {
            "+ 添加允许规则"
        } else {
            "+ Add allow rule"
        }
    }

    pub fn tui_openclaw_tools_add_deny_rule() -> &'static str {
        if is_chinese() {
            "+ 添加拒绝规则"
        } else {
            "+ Add deny rule"
        }
    }

    pub fn tui_openclaw_tools_extra_fields_label() -> &'static str {
        if is_chinese() {
            "保留的其他字段"
        } else {
            "Preserved extra fields"
        }
    }

    pub fn tui_openclaw_tools_save_label() -> &'static str {
        if is_chinese() {
            "保存"
        } else {
            "Save"
        }
    }

    pub fn tui_openclaw_tools_load_failed_message() -> &'static str {
        if is_chinese() {
            "当前 tools 配置无法加载；请先修复上方解析警告，再编辑工具权限。"
        } else {
            "The current tools section could not be loaded. Fix the parse warning above before editing tool permissions."
        }
    }

    pub fn tui_toast_openclaw_tools_save_result(success: bool) -> &'static str {
        if success {
            if is_chinese() {
                "工具权限已保存"
            } else {
                "Tool permissions saved"
            }
        } else if is_chinese() {
            "保存工具权限失败"
        } else {
            "Failed to save tool permissions"
        }
    }

    pub fn tui_toast_openclaw_tools_save_failed_detail(err: &str) -> String {
        if is_chinese() {
            format!("保存工具权限失败: {err}")
        } else {
            format!("Failed to save tool permissions: {err}")
        }
    }

    pub fn tui_toast_openclaw_tools_save_blocked_parse_error() -> &'static str {
        if is_chinese() {
            "请先修复 OpenClaw 工具配置解析警告，再保存工具权限"
        } else {
            "Fix OpenClaw tools parse warnings before saving tool permissions"
        }
    }

    pub fn tui_toast_openclaw_tools_rule_empty() -> &'static str {
        if is_chinese() {
            "工具规则不能为空"
        } else {
            "Tool rule cannot be empty"
        }
    }

    pub fn tui_openclaw_agents_description() -> &'static str {
        if is_chinese() {
            "管理 openclaw.json 中的 agents.defaults 配置（默认模型、运行参数等）"
        } else {
            "Manage agents.defaults in openclaw.json (default model, runtime parameters, etc.)"
        }
    }

    pub fn tui_openclaw_agents_model_section() -> &'static str {
        if is_chinese() {
            "模型配置"
        } else {
            "Model Configuration"
        }
    }

    pub fn tui_openclaw_agents_primary_model() -> &'static str {
        if is_chinese() {
            "默认模型"
        } else {
            "Default Model"
        }
    }

    pub fn tui_openclaw_agents_not_set() -> &'static str {
        if is_chinese() {
            "未设置"
        } else {
            "Not set"
        }
    }

    pub fn tui_openclaw_agents_fallback_models() -> &'static str {
        if is_chinese() {
            "回退模型"
        } else {
            "Fallback Models"
        }
    }

    pub fn tui_openclaw_agents_add_fallback() -> &'static str {
        if is_chinese() {
            "添加回退模型"
        } else {
            "Add fallback model"
        }
    }

    pub fn tui_openclaw_agents_add_fallback_disabled() -> &'static str {
        if is_chinese() {
            "没有可添加的回退模型了"
        } else {
            "No fallback models available to add"
        }
    }

    pub fn tui_openclaw_agents_not_configured_suffix() -> &'static str {
        if is_chinese() {
            "供应商未配置"
        } else {
            "not configured"
        }
    }

    pub fn tui_openclaw_agents_not_in_list(value: &str) -> String {
        if is_chinese() {
            format!("{value} (供应商未配置)")
        } else {
            format!("{value} (not configured)")
        }
    }

    pub fn tui_openclaw_agents_runtime_section() -> &'static str {
        if is_chinese() {
            "运行参数"
        } else {
            "Runtime Parameters"
        }
    }

    pub fn tui_openclaw_agents_workspace() -> &'static str {
        if is_chinese() {
            "工作区路径"
        } else {
            "Workspace Path"
        }
    }

    pub fn tui_openclaw_agents_timeout() -> &'static str {
        if is_chinese() {
            "超时时间（秒）"
        } else {
            "Timeout (seconds)"
        }
    }

    pub fn tui_openclaw_agents_context_tokens() -> &'static str {
        if is_chinese() {
            "上下文 Token 数"
        } else {
            "Context Tokens"
        }
    }

    pub fn tui_openclaw_agents_max_concurrent() -> &'static str {
        if is_chinese() {
            "最大并发数"
        } else {
            "Max Concurrent"
        }
    }

    pub fn tui_openclaw_agents_preserved_non_standard_value(value: &str) -> String {
        if is_chinese() {
            format!("{value}（已保留的非标准值）")
        } else {
            format!("{value} (preserved non-standard value)")
        }
    }

    pub fn tui_openclaw_agents_preserved_runtime_notice() -> &'static str {
        if is_chinese() {
            "非标准运行参数会在你替换它们之前保持原样保存。"
        } else {
            "Non-standard runtime values are preserved until you replace them."
        }
    }

    pub fn tui_openclaw_agents_preserved_fields_label() -> &'static str {
        if is_chinese() {
            "保留字段"
        } else {
            "Preserved Fields"
        }
    }

    pub fn tui_openclaw_agents_legacy_timeout_title() -> &'static str {
        if is_chinese() {
            "检测到旧版超时字段"
        } else {
            "Legacy timeout detected"
        }
    }

    pub fn tui_openclaw_agents_legacy_timeout_description() -> &'static str {
        if is_chinese() {
            "当前配置仍在使用 agents.defaults.timeout。保存本页面时会迁移为 timeoutSeconds。"
        } else {
            "This config still uses agents.defaults.timeout. Saving here will migrate it to timeoutSeconds."
        }
    }

    pub fn tui_openclaw_agents_legacy_timeout_invalid_description() -> &'static str {
        if is_chinese() {
            "当前配置仍在使用 agents.defaults.timeout，但该值无法自动迁移。请先改为数字，或清空该字段后再保存。"
        } else {
            "This config still uses agents.defaults.timeout, but the current value cannot be migrated automatically. Change it to a number or clear the field before saving."
        }
    }

    pub fn tui_openclaw_agents_load_failed_message() -> &'static str {
        if is_chinese() {
            "当前 agents.defaults 配置无法加载；请先修复上方解析警告，再编辑 Agents 配置。"
        } else {
            "The current agents.defaults section could not be loaded. Fix the parse warning above before editing agents defaults."
        }
    }

    pub fn tui_openclaw_agents_save_label() -> &'static str {
        if is_chinese() {
            "保存"
        } else {
            "Save"
        }
    }

    pub fn tui_toast_openclaw_agents_save_result(success: bool) -> &'static str {
        if success {
            if is_chinese() {
                "Agents 配置已保存"
            } else {
                "Agents config saved"
            }
        } else if is_chinese() {
            "保存 Agents 配置失败"
        } else {
            "Failed to save agents config"
        }
    }

    pub fn tui_toast_openclaw_agents_save_failed_detail(err: &str) -> String {
        if is_chinese() {
            format!("保存 Agents 配置失败: {err}")
        } else {
            format!("Failed to save agents config: {err}")
        }
    }

    pub fn tui_toast_openclaw_agents_save_blocked_parse_error() -> &'static str {
        if is_chinese() {
            "请先修复 OpenClaw agents.defaults 解析警告，再保存 Agents 配置"
        } else {
            "Fix OpenClaw agents parse warnings before saving agents defaults"
        }
    }

    pub fn tui_toast_openclaw_agents_save_blocked_legacy_timeout() -> &'static str {
        if is_chinese() {
            "请先处理 agents.defaults.timeout，再保存 Agents 配置"
        } else {
            "Resolve agents.defaults.timeout before saving agents config"
        }
    }

    pub fn tui_openclaw_config_agents_title() -> &'static str {
        tui_config_item_openclaw_agents()
    }

    pub fn tui_openclaw_config_env_editor_title() -> &'static str {
        if is_chinese() {
            "编辑环境变量 (JSON)"
        } else {
            "Edit Env Variables (JSON)"
        }
    }

    pub fn tui_openclaw_config_env_description() -> &'static str {
        if is_chinese() {
            "管理 openclaw.json 中的环境变量映射；保存时会写回 env.vars。"
        } else {
            "Manage the environment variable map in openclaw.json; saving writes back to env.vars."
        }
    }

    pub fn tui_openclaw_config_env_empty() -> &'static str {
        if is_chinese() {
            "未配置环境变量"
        } else {
            "No environment variables configured"
        }
    }

    pub fn tui_openclaw_config_tools_editor_title() -> &'static str {
        if is_chinese() {
            "编辑工具权限 (JSON)"
        } else {
            "Edit Tool Permissions (JSON)"
        }
    }

    pub fn tui_openclaw_config_agents_editor_title() -> &'static str {
        if is_chinese() {
            "编辑 Agents 配置 (JSON)"
        } else {
            "Edit Agents Config (JSON)"
        }
    }

    pub fn tui_openclaw_config_warning_title() -> &'static str {
        if is_chinese() {
            "OpenClaw 配置告警"
        } else {
            "OpenClaw Health Warnings"
        }
    }

    pub fn tui_openclaw_config_file_label() -> &'static str {
        if is_chinese() {
            "配置文件"
        } else {
            "Config file"
        }
    }

    pub fn tui_openclaw_config_section_label() -> &'static str {
        if is_chinese() {
            "当前配置"
        } else {
            "Section"
        }
    }

    pub fn tui_openclaw_config_warning_state_label() -> &'static str {
        if is_chinese() {
            "告警状态"
        } else {
            "Warnings"
        }
    }

    pub fn tui_openclaw_config_warning_present() -> &'static str {
        if is_chinese() {
            "发现告警"
        } else {
            "Warnings detected"
        }
    }

    pub fn tui_openclaw_config_warning_clean() -> &'static str {
        if is_chinese() {
            "正常"
        } else {
            "Healthy"
        }
    }

    pub fn tui_openclaw_config_path_not_available() -> &'static str {
        if is_chinese() {
            "不可用"
        } else {
            "n/a"
        }
    }

    pub fn tui_toast_openclaw_config_saved(section: &str) -> String {
        if is_chinese() {
            format!("已保存 {section}")
        } else {
            format!("Saved {section}")
        }
    }

    pub fn tui_openclaw_workspace_editor_title(filename: &str) -> String {
        if is_chinese() {
            format!("编辑 Workspace 文件: {filename}")
        } else {
            format!("Edit Workspace File: {filename}")
        }
    }

    pub fn tui_openclaw_workspace_saved(filename: &str) -> String {
        if is_chinese() {
            format!("已保存 Workspace 文件: {filename}")
        } else {
            format!("Saved workspace file: {filename}")
        }
    }

    pub fn tui_openclaw_workspace_open_failed(filename: &str, detail: &str) -> String {
        if is_chinese() {
            format!("打开 Workspace 文件失败 {filename}: {detail}")
        } else {
            format!("Failed to open workspace file {filename}: {detail}")
        }
    }

    pub fn tui_openclaw_workspace_save_failed(filename: &str, detail: &str) -> String {
        if is_chinese() {
            format!("保存 Workspace 文件失败 {filename}: {detail}")
        } else {
            format!("Failed to save workspace file {filename}: {detail}")
        }
    }

    pub fn tui_openclaw_workspace_refresh_failed(detail: &str) -> String {
        if is_chinese() {
            format!("刷新 Workspace 状态失败: {detail}")
        } else {
            format!("Failed to refresh workspace state: {detail}")
        }
    }

    pub fn tui_openclaw_workspace_directory_open_failed(detail: &str) -> String {
        if is_chinese() {
            format!("打开 Workspace 目录失败: {detail}")
        } else {
            format!("Failed to open workspace directory: {detail}")
        }
    }

    pub fn tui_openclaw_daily_memory_title() -> &'static str {
        if is_chinese() {
            "Daily Memory"
        } else {
            "Daily Memory"
        }
    }

    pub fn tui_openclaw_daily_memory_directory_label() -> &'static str {
        if is_chinese() {
            "Memory 目录"
        } else {
            "Memory directory"
        }
    }

    pub fn tui_openclaw_daily_memory_create_title() -> &'static str {
        if is_chinese() {
            "新建 Daily Memory"
        } else {
            "Create Daily Memory"
        }
    }

    pub fn tui_openclaw_daily_memory_create_prompt() -> &'static str {
        if is_chinese() {
            "输入文件名（YYYY-MM-DD.md）："
        } else {
            "Enter a filename (YYYY-MM-DD.md):"
        }
    }

    pub fn tui_openclaw_daily_memory_invalid_filename() -> &'static str {
        if is_chinese() {
            "文件名无效，请使用 YYYY-MM-DD.md。"
        } else {
            "Invalid filename. Use YYYY-MM-DD.md."
        }
    }

    pub fn tui_openclaw_daily_memory_editor_title(filename: &str) -> String {
        if is_chinese() {
            format!("编辑 Daily Memory: {filename}")
        } else {
            format!("Edit Daily Memory: {filename}")
        }
    }

    pub fn tui_openclaw_daily_memory_saved(filename: &str) -> String {
        if is_chinese() {
            format!("已保存 Daily Memory: {filename}")
        } else {
            format!("Saved daily memory: {filename}")
        }
    }

    pub fn tui_openclaw_daily_memory_open_failed(filename: &str, detail: &str) -> String {
        if is_chinese() {
            format!("打开 Daily Memory 失败 {filename}: {detail}")
        } else {
            format!("Failed to open daily memory {filename}: {detail}")
        }
    }

    pub fn tui_openclaw_daily_memory_save_failed(filename: &str, detail: &str) -> String {
        if is_chinese() {
            format!("保存 Daily Memory 失败 {filename}: {detail}")
        } else {
            format!("Failed to save daily memory {filename}: {detail}")
        }
    }

    pub fn tui_openclaw_daily_memory_deleted(filename: &str) -> String {
        if is_chinese() {
            format!("已删除 Daily Memory: {filename}")
        } else {
            format!("Deleted daily memory: {filename}")
        }
    }

    pub fn tui_openclaw_daily_memory_delete_failed(filename: &str, detail: &str) -> String {
        if is_chinese() {
            format!("删除 Daily Memory 失败 {filename}: {detail}")
        } else {
            format!("Failed to delete daily memory {filename}: {detail}")
        }
    }

    pub fn tui_openclaw_daily_memory_search_failed(detail: &str) -> String {
        if is_chinese() {
            format!("搜索 Daily Memory 失败: {detail}")
        } else {
            format!("Failed to search daily memory: {detail}")
        }
    }

    pub fn tui_openclaw_daily_memory_refresh_failed(detail: &str) -> String {
        if is_chinese() {
            format!("刷新 Daily Memory 列表失败: {detail}")
        } else {
            format!("Failed to refresh daily memory list: {detail}")
        }
    }

    pub fn tui_openclaw_daily_memory_delete_title() -> &'static str {
        if is_chinese() {
            "删除 Daily Memory"
        } else {
            "Delete Daily Memory"
        }
    }

    pub fn tui_openclaw_daily_memory_delete_message(filename: &str) -> String {
        if is_chinese() {
            format!("确认删除 {filename}？")
        } else {
            format!("Delete {filename}?")
        }
    }

    pub fn tui_openclaw_memory_directory_open_failed(detail: &str) -> String {
        if is_chinese() {
            format!("打开 Memory 目录失败: {detail}")
        } else {
            format!("Failed to open memory directory: {detail}")
        }
    }

    pub fn tui_openclaw_daily_memory_empty() -> &'static str {
        if is_chinese() {
            "还没有 Daily Memory 文件。按 a 新建。"
        } else {
            "No daily memory files yet. Press a to create one."
        }
    }

    pub fn tui_openclaw_daily_memory_search_empty() -> &'static str {
        if is_chinese() {
            "没有匹配的 Daily Memory 文件。"
        } else {
            "No matching daily memory files."
        }
    }

    pub fn tui_webdav_jianguoyun_setup_title() -> &'static str {
        if is_chinese() {
            "坚果云一键配置"
        } else {
            "Jianguoyun Quick Setup"
        }
    }

    pub fn tui_webdav_jianguoyun_username_prompt() -> &'static str {
        if is_chinese() {
            "请输入坚果云账号（通常是邮箱）："
        } else {
            "Enter your Jianguoyun account (usually email):"
        }
    }

    pub fn tui_webdav_jianguoyun_app_password_prompt() -> &'static str {
        if is_chinese() {
            "请输入坚果云第三方应用密码："
        } else {
            "Enter your Jianguoyun app password:"
        }
    }

    pub fn tui_webdav_loading_title_check_connection() -> &'static str {
        if is_chinese() {
            "WebDAV 检查连接"
        } else {
            "WebDAV Check Connection"
        }
    }

    pub fn tui_webdav_loading_title_upload() -> &'static str {
        if is_chinese() {
            "WebDAV 上传"
        } else {
            "WebDAV Upload"
        }
    }

    pub fn tui_webdav_loading_title_download() -> &'static str {
        if is_chinese() {
            "WebDAV 下载"
        } else {
            "WebDAV Download"
        }
    }

    pub fn tui_webdav_loading_title_quick_setup() -> &'static str {
        if is_chinese() {
            "坚果云一键配置"
        } else {
            "Jianguoyun Quick Setup"
        }
    }

    pub fn tui_webdav_loading_message() -> &'static str {
        if is_chinese() {
            "正在处理 WebDAV 请求，请稍候…"
        } else {
            "Processing WebDAV request, please wait..."
        }
    }

    pub fn tui_config_item_reset() -> &'static str {
        if is_chinese() {
            "重置配置"
        } else {
            "Reset Config"
        }
    }

    pub fn tui_config_item_show_full() -> &'static str {
        if is_chinese() {
            "查看完整配置"
        } else {
            "Show Full Config"
        }
    }

    pub fn tui_config_item_show_path() -> &'static str {
        if is_chinese() {
            "显示配置路径"
        } else {
            "Show Config Path"
        }
    }

    pub fn tui_hint_esc_close() -> &'static str {
        if is_chinese() {
            "Esc = 关闭"
        } else {
            "Esc = Close"
        }
    }

    pub fn tui_hint_enter_submit_esc_cancel() -> &'static str {
        if is_chinese() {
            "Enter = 提交, Esc = 取消"
        } else {
            "Enter = Submit, Esc = Cancel"
        }
    }

    pub fn tui_hint_enter_restore_esc_cancel() -> &'static str {
        if is_chinese() {
            "Enter = 恢复, Esc = 取消"
        } else {
            "Enter = restore, Esc = cancel"
        }
    }

    pub fn tui_backup_picker_title() -> &'static str {
        if is_chinese() {
            "选择备份（Enter 恢复）"
        } else {
            "Select Backup (Enter to restore)"
        }
    }

    pub fn tui_speedtest_running(url: &str) -> String {
        if is_chinese() {
            format!("正在测速: {}", url)
        } else {
            format!("Running: {}", url)
        }
    }

    pub fn tui_speedtest_title_with_url(url: &str) -> String {
        if is_chinese() {
            format!("测速: {}", url)
        } else {
            format!("Speedtest: {}", url)
        }
    }

    pub fn tui_stream_check_running(provider_name: &str) -> String {
        if is_chinese() {
            format!("正在检查: {}", provider_name)
        } else {
            format!("Checking: {}", provider_name)
        }
    }

    pub fn tui_stream_check_title_with_provider(provider_name: &str) -> String {
        if is_chinese() {
            format!("健康检查: {}", provider_name)
        } else {
            format!("Stream Check: {}", provider_name)
        }
    }

    pub fn tui_toast_provider_already_in_use() -> &'static str {
        if is_chinese() {
            "已在使用该供应商。"
        } else {
            "Already using this provider."
        }
    }

    pub fn tui_toast_provider_cannot_delete_current() -> &'static str {
        if is_chinese() {
            "不能删除当前供应商。"
        } else {
            "Cannot delete current provider."
        }
    }

    pub fn tui_toast_provider_cannot_remove_default_model() -> &'static str {
        if is_chinese() {
            "被当前默认模型引用的供应商不能直接从配置中移除。"
        } else {
            "A provider referenced by the current default model cannot be removed from config directly."
        }
    }

    pub fn tui_toast_provider_default_requires_live_config() -> &'static str {
        if is_chinese() {
            "请先将该供应商添加到配置中，再设为默认。"
        } else {
            "Add this provider to config before setting it as default."
        }
    }

    pub fn tui_toast_provider_default_model_missing() -> &'static str {
        if is_chinese() {
            "该供应商缺少可设为默认的主模型。"
        } else {
            "This provider has no primary model to set as default."
        }
    }

    pub fn tui_toast_provider_removed_from_config() -> &'static str {
        if is_chinese() {
            "已从当前 OpenClaw 配置中移除该供应商。"
        } else {
            "Provider removed from the current OpenClaw config."
        }
    }

    pub fn tui_toast_provider_added_to_app_config(app: &str) -> String {
        if is_chinese() {
            format!("已将该供应商添加到当前 {app} 配置。")
        } else {
            format!("Provider added to the current {app} config.")
        }
    }

    pub fn tui_toast_provider_removed_from_app_config(app: &str) -> String {
        if is_chinese() {
            format!("已从当前 {app} 配置中移除该供应商。")
        } else {
            format!("Provider removed from the current {app} config.")
        }
    }

    pub fn tui_toast_provider_set_as_default(model: &str) -> String {
        if is_chinese() {
            format!("已设为默认模型: {}", model)
        } else {
            format!("Set default model: {}", model)
        }
    }

    pub fn tui_temp_launch_failed(message: &str) -> String {
        if is_chinese() {
            format!("临时启动失败: {}", message)
        } else {
            format!("Temporary launch failed: {}", message)
        }
    }

    pub fn tui_confirm_delete_provider_title() -> &'static str {
        if is_chinese() {
            "删除供应商"
        } else {
            "Delete Provider"
        }
    }

    pub fn tui_confirm_delete_provider_message(name: &str, id: &str) -> String {
        if is_chinese() {
            format!("确定删除供应商 '{}' ({})？", name, id)
        } else {
            format!("Delete provider '{}' ({})?", name, id)
        }
    }

    pub fn tui_mcp_add_title() -> &'static str {
        if is_chinese() {
            "新增 MCP 服务器"
        } else {
            "Add MCP Server"
        }
    }

    pub fn tui_mcp_edit_title(name: &str) -> String {
        if is_chinese() {
            format!("编辑 MCP 服务器: {}", name)
        } else {
            format!("Edit MCP Server: {}", name)
        }
    }

    pub fn tui_mcp_apps_title(name: &str) -> String {
        if is_chinese() {
            format!("选择 MCP 应用: {}", name)
        } else {
            format!("Select MCP Apps: {}", name)
        }
    }

    pub fn tui_mcp_type_title() -> &'static str {
        if is_chinese() {
            "选择 MCP 连接类型"
        } else {
            "Select MCP Transport"
        }
    }

    pub fn tui_mcp_env_title() -> &'static str {
        if is_chinese() {
            "MCP 环境变量"
        } else {
            "MCP Env"
        }
    }

    pub fn tui_mcp_env_add_entry_title() -> &'static str {
        if is_chinese() {
            "新增环境变量"
        } else {
            "Add Env Entry"
        }
    }

    pub fn tui_mcp_env_edit_entry_title() -> &'static str {
        if is_chinese() {
            "编辑环境变量"
        } else {
            "Edit Env Entry"
        }
    }

    pub fn tui_mcp_env_empty_state() -> &'static str {
        if is_chinese() {
            "暂无环境变量，按 a 新增。"
        } else {
            "No env entries yet. Press a to add one."
        }
    }

    pub fn tui_skill_apps_title(name: &str) -> String {
        if is_chinese() {
            format!("选择 Skill 应用: {}", name)
        } else {
            format!("Select Skill Apps: {}", name)
        }
    }

    pub fn tui_toast_provider_no_api_url() -> &'static str {
        if is_chinese() {
            "该供应商未配置 API URL。"
        } else {
            "No API URL configured for this provider."
        }
    }

    pub fn tui_confirm_delete_mcp_title() -> &'static str {
        if is_chinese() {
            "删除 MCP 服务器"
        } else {
            "Delete MCP Server"
        }
    }

    pub fn tui_confirm_delete_mcp_message(name: &str, id: &str) -> String {
        if is_chinese() {
            format!("确定删除 MCP 服务器 '{}' ({})？", name, id)
        } else {
            format!("Delete MCP server '{}' ({})?", name, id)
        }
    }

    pub fn tui_prompt_title(name: &str) -> String {
        if is_chinese() {
            format!("提示词: {}", name)
        } else {
            format!("Prompt: {}", name)
        }
    }

    pub fn tui_toast_prompt_no_active_to_deactivate() -> &'static str {
        if is_chinese() {
            "没有可停用的活动提示词。"
        } else {
            "No active prompt to deactivate."
        }
    }

    pub fn tui_toast_prompt_cannot_delete_active() -> &'static str {
        if is_chinese() {
            "不能删除正在启用的提示词。"
        } else {
            "Cannot delete the active prompt."
        }
    }

    pub fn tui_confirm_delete_prompt_title() -> &'static str {
        if is_chinese() {
            "删除提示词"
        } else {
            "Delete Prompt"
        }
    }

    pub fn tui_confirm_delete_prompt_message(name: &str, id: &str) -> String {
        if is_chinese() {
            format!("确定删除提示词 '{}' ({})？", name, id)
        } else {
            format!("Delete prompt '{}' ({})?", name, id)
        }
    }

    pub fn tui_toast_prompt_edit_not_implemented() -> &'static str {
        if is_chinese() {
            "提示词编辑尚未实现。"
        } else {
            "Prompt editing not implemented yet."
        }
    }

    pub fn tui_toast_prompt_edit_finished() -> &'static str {
        if is_chinese() {
            "提示词编辑完成"
        } else {
            "Prompt edit finished"
        }
    }

    pub fn tui_toast_prompt_not_found(id: &str) -> String {
        if is_chinese() {
            format!("未找到提示词：{}", id)
        } else {
            format!("Prompt not found: {}", id)
        }
    }

    pub fn tui_config_paths_title() -> &'static str {
        if is_chinese() {
            "配置路径"
        } else {
            "Configuration Paths"
        }
    }

    pub fn tui_config_paths_config_file(path: &str) -> String {
        if is_chinese() {
            format!("配置文件: {}", path)
        } else {
            format!("Config file: {}", path)
        }
    }

    pub fn tui_config_paths_config_dir(path: &str) -> String {
        if is_chinese() {
            format!("配置目录:  {}", path)
        } else {
            format!("Config dir:  {}", path)
        }
    }

    pub fn tui_error_failed_to_read_config(e: &str) -> String {
        if is_chinese() {
            format!("读取配置失败: {e}")
        } else {
            format!("Failed to read config: {e}")
        }
    }

    pub fn tui_config_export_title() -> &'static str {
        if is_chinese() {
            "导出配置"
        } else {
            "Export Configuration"
        }
    }

    pub fn tui_config_export_prompt() -> &'static str {
        if is_chinese() {
            "导出路径："
        } else {
            "Export path:"
        }
    }

    pub fn tui_config_import_title() -> &'static str {
        if is_chinese() {
            "导入配置"
        } else {
            "Import Configuration"
        }
    }

    pub fn tui_config_import_prompt() -> &'static str {
        if is_chinese() {
            "从路径导入："
        } else {
            "Import from path:"
        }
    }

    pub fn tui_config_backup_title() -> &'static str {
        if is_chinese() {
            "备份配置"
        } else {
            "Backup Configuration"
        }
    }

    pub fn tui_config_backup_prompt() -> &'static str {
        if is_chinese() {
            "可选名称（留空使用默认值）："
        } else {
            "Optional name (empty for default):"
        }
    }

    pub fn tui_toast_no_backups_found() -> &'static str {
        if is_chinese() {
            "未找到备份。"
        } else {
            "No backups found."
        }
    }

    pub fn tui_error_failed_to_read(e: &str) -> String {
        if is_chinese() {
            format!("读取失败: {e}")
        } else {
            format!("Failed to read: {e}")
        }
    }

    pub fn tui_common_snippet_title(app: &str) -> String {
        if is_chinese() {
            format!("通用片段 ({})", app)
        } else {
            format!("Common Snippet ({})", app)
        }
    }

    pub fn tui_config_reset_title() -> &'static str {
        if is_chinese() {
            "重置配置"
        } else {
            "Reset Configuration"
        }
    }

    pub fn tui_config_reset_message() -> &'static str {
        if is_chinese() {
            "重置为默认配置？（这将覆盖当前配置）"
        } else {
            "Reset to default configuration? (This will overwrite your current config)"
        }
    }

    pub fn tui_toast_export_path_empty() -> &'static str {
        if is_chinese() {
            "导出路径为空。"
        } else {
            "Export path is empty."
        }
    }

    pub fn tui_toast_import_path_empty() -> &'static str {
        if is_chinese() {
            "导入路径为空。"
        } else {
            "Import path is empty."
        }
    }

    pub fn tui_confirm_import_message(path: &str) -> String {
        if is_chinese() {
            format!("确认从 '{}' 导入？", path)
        } else {
            format!("Import from '{}'?", path)
        }
    }

    pub fn tui_toast_command_empty() -> &'static str {
        if is_chinese() {
            "命令为空。"
        } else {
            "Command is empty."
        }
    }

    pub fn tui_toast_url_empty() -> &'static str {
        if is_chinese() {
            "URL 为空。"
        } else {
            "URL is empty."
        }
    }

    pub fn tui_toast_mcp_env_key_empty() -> &'static str {
        if is_chinese() {
            "环境变量 Key 不能为空。"
        } else {
            "Env key cannot be empty."
        }
    }

    pub fn tui_toast_mcp_env_duplicate_key(key: &str) -> String {
        if is_chinese() {
            format!("环境变量 Key '{}' 已存在。", key)
        } else {
            format!("Env key '{key}' already exists.")
        }
    }

    pub fn tui_confirm_restore_backup_title() -> &'static str {
        if is_chinese() {
            "恢复备份"
        } else {
            "Restore Backup"
        }
    }

    pub fn tui_confirm_restore_backup_message(name: &str) -> String {
        if is_chinese() {
            format!("确认从备份 '{}' 恢复？", name)
        } else {
            format!("Restore from backup '{}'?", name)
        }
    }

    pub fn tui_speedtest_line_url(url: &str) -> String {
        format!("URL: {}", url)
    }

    pub fn tui_stream_check_line_provider(provider_name: &str) -> String {
        if is_chinese() {
            format!("供应商: {provider_name}")
        } else {
            format!("Provider: {provider_name}")
        }
    }

    pub fn tui_stream_check_line_status(status: &str) -> String {
        if is_chinese() {
            format!("状态:   {status}")
        } else {
            format!("Status:  {status}")
        }
    }

    pub fn tui_stream_check_line_response_time(response_time: &str) -> String {
        if is_chinese() {
            format!("耗时:   {response_time}")
        } else {
            format!("Time:    {response_time}")
        }
    }

    pub fn tui_stream_check_line_http_status(status: &str) -> String {
        if is_chinese() {
            format!("HTTP:   {status}")
        } else {
            format!("HTTP:    {status}")
        }
    }

    pub fn tui_stream_check_line_model(model: &str) -> String {
        if is_chinese() {
            format!("模型:   {model}")
        } else {
            format!("Model:   {model}")
        }
    }

    pub fn tui_stream_check_line_retries(retries: &str) -> String {
        if is_chinese() {
            format!("重试:   {retries}")
        } else {
            format!("Retries: {retries}")
        }
    }

    pub fn tui_stream_check_line_message(message: &str) -> String {
        if is_chinese() {
            format!("信息:   {message}")
        } else {
            format!("Message: {message}")
        }
    }

    pub fn tui_speedtest_line_latency(latency: &str) -> String {
        if is_chinese() {
            format!("延迟:   {latency}")
        } else {
            format!("Latency: {latency}")
        }
    }

    pub fn tui_speedtest_line_status(status: &str) -> String {
        if is_chinese() {
            format!("状态:   {status}")
        } else {
            format!("Status:  {status}")
        }
    }

    pub fn tui_speedtest_line_error(err: &str) -> String {
        if is_chinese() {
            format!("错误:   {err}")
        } else {
            format!("Error:   {err}")
        }
    }

    pub fn tui_toast_speedtest_finished() -> &'static str {
        if is_chinese() {
            "测速完成。"
        } else {
            "Speedtest finished."
        }
    }

    pub fn tui_toast_speedtest_failed(err: &str) -> String {
        if is_chinese() {
            format!("测速失败: {err}")
        } else {
            format!("Speedtest failed: {err}")
        }
    }

    pub fn tui_toast_speedtest_unavailable(err: &str) -> String {
        if is_chinese() {
            format!("测速不可用: {err}")
        } else {
            format!("Speedtest unavailable: {err}")
        }
    }

    pub fn tui_toast_speedtest_disabled() -> &'static str {
        if is_chinese() {
            "本次会话测速不可用。"
        } else {
            "Speedtest is disabled for this session."
        }
    }

    pub fn tui_toast_local_env_check_unavailable(err: &str) -> String {
        if is_chinese() {
            format!("本地环境检查不可用: {err}")
        } else {
            format!("Local environment check unavailable: {err}")
        }
    }

    pub fn tui_toast_local_env_check_disabled() -> &'static str {
        if is_chinese() {
            "本次会话本地环境检查不可用。"
        } else {
            "Local environment check is disabled for this session."
        }
    }

    pub fn tui_toast_local_env_check_request_failed(err: &str) -> String {
        if is_chinese() {
            format!("本地环境检查刷新请求失败: {err}")
        } else {
            format!("Failed to enqueue local environment check: {err}")
        }
    }

    pub fn tui_toast_speedtest_request_failed(err: &str) -> String {
        if is_chinese() {
            format!("测速请求失败: {err}")
        } else {
            format!("Failed to enqueue speedtest: {err}")
        }
    }

    pub fn tui_toast_stream_check_finished() -> &'static str {
        if is_chinese() {
            "健康检查完成。"
        } else {
            "Stream check finished."
        }
    }

    pub fn tui_toast_stream_check_failed(err: &str) -> String {
        if is_chinese() {
            format!("健康检查失败: {err}")
        } else {
            format!("Stream check failed: {err}")
        }
    }

    pub fn tui_toast_stream_check_unavailable(err: &str) -> String {
        if is_chinese() {
            format!("健康检查不可用: {err}")
        } else {
            format!("Stream check unavailable: {err}")
        }
    }

    pub fn tui_toast_stream_check_disabled() -> &'static str {
        if is_chinese() {
            "本次会话健康检查不可用。"
        } else {
            "Stream check is disabled for this session."
        }
    }

    pub fn tui_toast_stream_check_request_failed(err: &str) -> String {
        if is_chinese() {
            format!("健康检查请求失败: {err}")
        } else {
            format!("Failed to enqueue stream check: {err}")
        }
    }

    pub fn tui_toast_quota_not_available() -> &'static str {
        if is_chinese() {
            "当前供应商没有官方额度查询。"
        } else {
            "This provider has no official quota query."
        }
    }

    pub fn tui_toast_quota_worker_unavailable(err: &str) -> String {
        if is_chinese() {
            format!("额度查询后台任务不可用: {err}")
        } else {
            format!("Quota worker unavailable: {err}")
        }
    }

    pub fn tui_toast_quota_refresh_started(provider: &str) -> String {
        if is_chinese() {
            format!("正在刷新额度: {provider}")
        } else {
            format!("Refreshing quota: {provider}")
        }
    }

    pub fn tui_toast_quota_refresh_finished(provider: &str) -> String {
        if is_chinese() {
            format!("额度已刷新: {provider}")
        } else {
            format!("Quota refreshed: {provider}")
        }
    }

    pub fn tui_toast_quota_refresh_failed(err: &str) -> String {
        if is_chinese() {
            format!("额度刷新失败: {err}")
        } else {
            format!("Quota refresh failed: {err}")
        }
    }

    pub fn tui_toast_skills_worker_unavailable(err: &str) -> String {
        if is_chinese() {
            format!("Skills 后台任务不可用: {err}")
        } else {
            format!("Skills worker unavailable: {err}")
        }
    }

    pub fn tui_toast_webdav_worker_unavailable(err: &str) -> String {
        if is_chinese() {
            format!("WebDAV 后台任务不可用: {err}")
        } else {
            format!("WebDAV worker unavailable: {err}")
        }
    }

    pub fn tui_toast_model_fetch_worker_unavailable(err: &str) -> String {
        if is_chinese() {
            format!("模型获取后台任务不可用: {err}")
        } else {
            format!("Model fetch worker unavailable: {err}")
        }
    }

    pub fn tui_toast_model_fetch_worker_disabled() -> &'static str {
        if is_chinese() {
            "本次会话模型获取后台任务不可用。"
        } else {
            "Model fetch worker is disabled for this session."
        }
    }

    pub fn tui_toast_webdav_worker_disabled() -> &'static str {
        if is_chinese() {
            "本次会话 WebDAV 后台任务不可用。"
        } else {
            "WebDAV worker is disabled for this session."
        }
    }

    pub fn tui_error_skills_worker_unavailable() -> &'static str {
        if is_chinese() {
            "Skills 后台任务不可用。"
        } else {
            "Skills worker unavailable."
        }
    }

    pub fn tui_toast_skills_discover_finished(count: usize) -> String {
        if is_chinese() {
            format!("发现完成：{count} 个结果。")
        } else {
            format!("Discover finished: {count} result(s).")
        }
    }

    pub fn tui_toast_skills_discover_failed(err: &str) -> String {
        if is_chinese() {
            format!("发现失败: {err}")
        } else {
            format!("Discover failed: {err}")
        }
    }

    pub fn tui_toast_skill_installed(directory: &str) -> String {
        if is_chinese() {
            format!("已安装: {directory}")
        } else {
            format!("Installed: {directory}")
        }
    }

    pub fn tui_toast_skill_install_failed(spec: &str, err: &str) -> String {
        if is_chinese() {
            format!("安装失败（{spec}）: {err}")
        } else {
            format!("Install failed ({spec}): {err}")
        }
    }

    pub fn tui_toast_skill_already_installed() -> &'static str {
        if is_chinese() {
            "该 Skill 已安装。"
        } else {
            "Skill already installed."
        }
    }

    pub fn tui_toast_skill_spec_empty() -> &'static str {
        if is_chinese() {
            "Skill 不能为空。"
        } else {
            "Skill spec is empty."
        }
    }

    pub fn tui_toast_skill_toggled(directory: &str, enabled: bool) -> String {
        if is_chinese() {
            format!("{} {directory}", if enabled { "已启用" } else { "已禁用" })
        } else {
            format!(
                "{} {directory}",
                if enabled { "Enabled" } else { "Disabled" }
            )
        }
    }

    pub fn tui_toast_skill_uninstalled(directory: &str) -> String {
        if is_chinese() {
            format!("已卸载: {directory}")
        } else {
            format!("Uninstalled: {directory}")
        }
    }

    pub fn tui_toast_skill_apps_updated() -> &'static str {
        if is_chinese() {
            "Skill 应用已更新。"
        } else {
            "Skill apps updated."
        }
    }

    pub fn tui_toast_skills_synced() -> &'static str {
        if is_chinese() {
            "Skills 同步完成。"
        } else {
            "Skills synced."
        }
    }

    pub fn tui_toast_skills_sync_method_set(method: &str) -> String {
        if is_chinese() {
            format!("同步方式已设置为: {method}")
        } else {
            format!("Sync method set to: {method}")
        }
    }

    pub fn tui_toast_repo_spec_empty() -> &'static str {
        if is_chinese() {
            "仓库不能为空。"
        } else {
            "Repository is empty."
        }
    }

    pub fn tui_error_repo_spec_empty() -> &'static str {
        if is_chinese() {
            "仓库不能为空。"
        } else {
            "Repository cannot be empty."
        }
    }

    pub fn tui_error_repo_spec_invalid() -> &'static str {
        if is_chinese() {
            "仓库格式无效。请使用 owner/name 或 https://github.com/owner/name"
        } else {
            "Invalid repo format. Use owner/name or https://github.com/owner/name"
        }
    }

    pub fn tui_toast_repo_added() -> &'static str {
        if is_chinese() {
            "仓库已添加。"
        } else {
            "Repository added."
        }
    }

    pub fn tui_toast_repo_removed() -> &'static str {
        if is_chinese() {
            "仓库已移除。"
        } else {
            "Repository removed."
        }
    }

    pub fn tui_toast_repo_toggled(enabled: bool) -> String {
        if is_chinese() {
            if enabled {
                "仓库已启用。".to_string()
            } else {
                "仓库已禁用。".to_string()
            }
        } else {
            if enabled {
                "Repository enabled.".to_string()
            } else {
                "Repository disabled.".to_string()
            }
        }
    }

    pub fn tui_toast_skip_claude_onboarding_toggled(enabled: bool) -> String {
        if is_chinese() {
            if enabled {
                "已跳过 Claude Code 初次安装确认。".to_string()
            } else {
                "已恢复 Claude Code 初次安装确认。".to_string()
            }
        } else {
            if enabled {
                "Claude Code onboarding confirmation will be skipped.".to_string()
            } else {
                "Claude Code onboarding confirmation restored.".to_string()
            }
        }
    }

    pub fn tui_toast_claude_plugin_integration_toggled(enabled: bool) -> String {
        if is_chinese() {
            if enabled {
                "已启用 Claude Code for VSCode 插件联动。".to_string()
            } else {
                "已关闭 Claude Code for VSCode 插件联动。".to_string()
            }
        } else {
            if enabled {
                "Claude Code for VSCode integration enabled.".to_string()
            } else {
                "Claude Code for VSCode integration disabled.".to_string()
            }
        }
    }

    pub fn tui_toast_claude_plugin_sync_failed(err: &str) -> String {
        if is_chinese() {
            format!("同步 Claude Code for VSCode 插件失败: {err}")
        } else {
            format!("Failed to sync Claude Code for VSCode integration: {err}")
        }
    }

    pub fn tui_toast_unmanaged_scanned(count: usize) -> String {
        if is_chinese() {
            format!("扫描完成：发现 {count} 个可导入技能。")
        } else {
            format!("Scan finished: found {count} skill(s) available to import.")
        }
    }

    pub fn tui_toast_no_unmanaged_selected() -> &'static str {
        if is_chinese() {
            "请至少选择一个要导入的技能。"
        } else {
            "Select at least one skill to import."
        }
    }

    pub fn tui_toast_unmanaged_imported(count: usize) -> String {
        if is_chinese() {
            format!("已导入 {count} 个技能。")
        } else {
            format!("Imported {count} skill(s).")
        }
    }

    pub fn tui_toast_provider_deleted() -> &'static str {
        if is_chinese() {
            "供应商已删除。"
        } else {
            "Provider deleted."
        }
    }

    pub fn tui_toast_provider_add_finished() -> &'static str {
        if is_chinese() {
            "供应商新增流程已完成。"
        } else {
            "Provider add flow finished."
        }
    }

    pub fn tui_toast_provider_add_missing_fields() -> &'static str {
        if is_chinese() {
            "请填写 name，id 会自动生成。"
        } else {
            "Please fill in name. id will be generated automatically."
        }
    }

    pub fn tui_toast_provider_missing_name() -> &'static str {
        if is_chinese() {
            "请填写 name，id 会自动生成。"
        } else {
            "Please fill in name. id will be generated automatically."
        }
    }

    pub fn tui_toast_provider_add_failed() -> &'static str {
        if is_chinese() {
            "新增供应商失败。"
        } else {
            "Failed to add provider."
        }
    }

    pub fn tui_toast_provider_edit_finished() -> &'static str {
        if is_chinese() {
            "供应商编辑流程已完成。"
        } else {
            "Provider edit flow finished."
        }
    }

    pub fn tui_toast_mcp_updated() -> &'static str {
        if is_chinese() {
            "MCP 已更新。"
        } else {
            "MCP updated."
        }
    }

    pub fn tui_toast_mcp_upserted() -> &'static str {
        if is_chinese() {
            "MCP 服务器已保存。"
        } else {
            "MCP server saved."
        }
    }

    pub fn tui_toast_mcp_missing_fields() -> &'static str {
        if is_chinese() {
            "请在 JSON 中填写 id 和 name。"
        } else {
            "Please fill in id and name in JSON."
        }
    }

    pub fn tui_toast_mcp_server_deleted() -> &'static str {
        if is_chinese() {
            "MCP 服务器已删除。"
        } else {
            "MCP server deleted."
        }
    }

    pub fn tui_toast_mcp_server_not_found() -> &'static str {
        if is_chinese() {
            "未找到 MCP 服务器。"
        } else {
            "MCP server not found."
        }
    }

    pub fn tui_toast_mcp_imported(count: usize) -> String {
        if is_chinese() {
            format!("已导入 {count} 个 MCP 服务器。")
        } else {
            format!("Imported {count} MCP server(s).")
        }
    }

    pub fn tui_toast_live_sync_skipped_uninitialized(app: &str) -> String {
        if is_chinese() {
            format!(
                "未检测到 {app} 客户端本地配置，已跳过写入 live 文件；先运行一次 {app} 初始化后再试。"
            )
        } else {
            format!("Live sync skipped: {app} client not initialized; run it once to initialize, then retry.")
        }
    }

    pub fn tui_toast_mcp_updated_live_sync_skipped(apps: &[&str]) -> String {
        let list = if is_chinese() {
            apps.join("、")
        } else {
            apps.join(", ")
        };

        if is_chinese() {
            format!(
                "MCP 已更新，但以下客户端未初始化，已跳过写入 live 文件：{list}；先运行一次对应客户端初始化后再试。"
            )
        } else {
            format!(
                "MCP updated, but live sync skipped for uninitialized client(s): {list}; run them once to initialize, then retry."
            )
        }
    }

    pub fn tui_toast_prompt_activated() -> &'static str {
        if is_chinese() {
            "提示词已启用。"
        } else {
            "Prompt activated."
        }
    }

    pub fn tui_toast_prompt_deactivated() -> &'static str {
        if is_chinese() {
            "提示词已停用。"
        } else {
            "Prompt deactivated."
        }
    }

    pub fn tui_toast_prompt_deleted() -> &'static str {
        if is_chinese() {
            "提示词已删除。"
        } else {
            "Prompt deleted."
        }
    }

    pub fn tui_toast_exported_to(path: &str) -> String {
        if is_chinese() {
            format!("已导出到 {}", path)
        } else {
            format!("Exported to {}", path)
        }
    }

    pub fn tui_error_import_file_not_found(path: &str) -> String {
        if is_chinese() {
            format!("导入文件不存在: {}", path)
        } else {
            format!("Import file not found: {}", path)
        }
    }

    pub fn tui_toast_imported_config() -> &'static str {
        if is_chinese() {
            "配置已导入。"
        } else {
            "Imported config."
        }
    }

    pub fn tui_toast_imported_with_backup(backup_id: &str) -> String {
        if is_chinese() {
            format!("已导入（备份: {backup_id}）")
        } else {
            format!("Imported (backup: {backup_id})")
        }
    }

    pub fn tui_toast_no_config_file_to_backup() -> &'static str {
        if is_chinese() {
            "没有可备份的配置文件。"
        } else {
            "No config file to backup."
        }
    }

    pub fn tui_toast_backup_created(id: &str) -> String {
        if is_chinese() {
            format!("备份已创建: {id}")
        } else {
            format!("Backup created: {id}")
        }
    }

    pub fn tui_toast_restored_from_backup() -> &'static str {
        if is_chinese() {
            "已从备份恢复。"
        } else {
            "Restored from backup."
        }
    }

    pub fn tui_toast_restored_with_pre_backup(pre_backup: &str) -> String {
        if is_chinese() {
            format!("已恢复（恢复前备份: {pre_backup}）")
        } else {
            format!("Restored (pre-backup: {pre_backup})")
        }
    }

    pub fn tui_toast_webdav_settings_saved() -> &'static str {
        if is_chinese() {
            "WebDAV 同步设置已保存。"
        } else {
            "WebDAV sync settings saved."
        }
    }

    pub fn tui_toast_proxy_takeover_requires_running() -> &'static str {
        if is_chinese() {
            "前台代理未运行，请先启动 `cc-switch proxy serve`。"
        } else {
            "Foreground proxy is not running. Start `cc-switch proxy serve` first."
        }
    }

    pub fn tui_toast_proxy_takeover_updated(app: &str, enabled: bool) -> String {
        if is_chinese() {
            if enabled {
                format!("已将 {app} 接管到前台代理。")
            } else {
                format!("已将 {app} 恢复到 live 配置。")
            }
        } else if enabled {
            format!("{app} now uses the foreground proxy.")
        } else {
            format!("{app} restored to its live config.")
        }
    }

    pub fn tui_toast_proxy_managed_current_app_updated(app: &str, enabled: bool) -> String {
        if is_chinese() {
            if enabled {
                format!("{app} 已走 cc-switch 代理。")
            } else {
                format!("{app} 已恢复 live 配置。")
            }
        } else if enabled {
            format!("{app} now routes through cc-switch.")
        } else {
            format!("{app} restored to its live config.")
        }
    }

    pub fn tui_toast_proxy_worker_unavailable(err: &str) -> String {
        if is_chinese() {
            format!("代理任务不可用：{err}")
        } else {
            format!("Proxy worker unavailable: {err}")
        }
    }

    pub fn tui_toast_proxy_request_failed(err: &str) -> String {
        if is_chinese() {
            format!("代理请求发送失败：{err}")
        } else {
            format!("Proxy request failed: {err}")
        }
    }

    pub fn tui_error_proxy_worker_unavailable() -> &'static str {
        if is_chinese() {
            "代理任务不可用。"
        } else {
            "Proxy worker unavailable."
        }
    }

    pub fn tui_toast_webdav_settings_cleared() -> &'static str {
        if is_chinese() {
            "WebDAV 同步设置已清空。"
        } else {
            "WebDAV sync settings cleared."
        }
    }

    pub fn tui_toast_webdav_connection_ok() -> &'static str {
        if is_chinese() {
            "WebDAV 连接检查通过。"
        } else {
            "WebDAV connection check passed."
        }
    }

    pub fn tui_toast_webdav_upload_ok() -> &'static str {
        if is_chinese() {
            "WebDAV 上传完成。"
        } else {
            "WebDAV upload completed."
        }
    }

    pub fn tui_toast_webdav_download_ok() -> &'static str {
        if is_chinese() {
            "WebDAV 下载完成。"
        } else {
            "WebDAV download completed."
        }
    }

    pub fn tui_webdav_v1_migration_title() -> &'static str {
        if is_chinese() {
            "发现旧版同步数据"
        } else {
            "Legacy sync data detected"
        }
    }

    pub fn tui_webdav_v1_migration_message() -> &'static str {
        if is_chinese() {
            "远端存在 V1 格式的同步数据，是否迁移到 V2？\n迁移将下载旧数据、应用到本地、重新上传为新格式，并清理旧数据。"
        } else {
            "V1 sync data found on remote. Migrate to V2?\nThis will download old data, apply locally, re-upload as V2, and clean up V1 data."
        }
    }

    pub fn tui_webdav_loading_title_v1_migration() -> &'static str {
        if is_chinese() {
            "V1 → V2 迁移"
        } else {
            "V1 → V2 Migration"
        }
    }

    pub fn tui_toast_webdav_v1_migration_ok() -> &'static str {
        if is_chinese() {
            "V1 → V2 迁移完成，旧数据已清理。"
        } else {
            "V1 → V2 migration completed, old data cleaned up."
        }
    }

    pub fn tui_toast_webdav_jianguoyun_configured() -> &'static str {
        if is_chinese() {
            "坚果云一键配置完成，连接检查通过。"
        } else {
            "Jianguoyun quick setup completed and connection verified."
        }
    }

    pub fn tui_toast_webdav_username_empty() -> &'static str {
        if is_chinese() {
            "请输入 WebDAV 用户名。"
        } else {
            "Please enter a WebDAV username."
        }
    }

    pub fn tui_toast_webdav_password_empty() -> &'static str {
        if is_chinese() {
            "请输入 WebDAV 第三方应用密码。"
        } else {
            "Please enter a WebDAV app password."
        }
    }

    pub fn tui_toast_webdav_request_failed(err: &str) -> String {
        if is_chinese() {
            format!("WebDAV 请求提交失败: {err}")
        } else {
            format!("Failed to enqueue WebDAV request: {err}")
        }
    }

    pub fn tui_toast_webdav_action_failed(action: &str, err: &str) -> String {
        if is_chinese() {
            format!("{action} 失败: {err}")
        } else {
            format!("{action} failed: {err}")
        }
    }

    pub fn tui_toast_webdav_quick_setup_failed(err: &str) -> String {
        if is_chinese() {
            format!("坚果云一键配置已保存，但连接检查失败: {err}")
        } else {
            format!("Jianguoyun quick setup was saved, but connection check failed: {err}")
        }
    }

    pub fn tui_toast_config_file_does_not_exist() -> &'static str {
        if is_chinese() {
            "配置文件不存在。"
        } else {
            "Config file does not exist."
        }
    }

    pub fn tui_config_validation_title() -> &'static str {
        if is_chinese() {
            "配置校验"
        } else {
            "Config Validation"
        }
    }

    pub fn tui_config_validation_failed_title() -> &'static str {
        if is_chinese() {
            "配置校验失败"
        } else {
            "Config Validation Failed"
        }
    }

    pub fn tui_config_validation_ok() -> &'static str {
        if is_chinese() {
            "✓ 配置是有效的 JSON"
        } else {
            "✓ Configuration is valid JSON"
        }
    }

    pub fn tui_config_validation_provider_count(app: &str, count: usize) -> String {
        if is_chinese() {
            format!("{app} 供应商:  {count}")
        } else {
            format!("{app} providers:  {count}")
        }
    }

    pub fn tui_config_validation_mcp_servers(count: usize) -> String {
        if is_chinese() {
            format!("MCP 服务器:       {count}")
        } else {
            format!("MCP servers:       {count}")
        }
    }

    pub fn tui_toast_validation_passed() -> &'static str {
        if is_chinese() {
            "校验通过。"
        } else {
            "Validation passed."
        }
    }

    pub fn tui_toast_config_reset_to_defaults() -> &'static str {
        if is_chinese() {
            "配置已重置为默认值。"
        } else {
            "Config reset to defaults."
        }
    }

    pub fn tui_toast_config_reset_with_backup(backup_id: &str) -> String {
        if is_chinese() {
            format!("配置已重置（备份: {backup_id}）")
        } else {
            format!("Config reset (backup: {backup_id})")
        }
    }

    pub fn menu_home() -> &'static str {
        let (en, zh) = menu_home_variants();
        if is_chinese() {
            zh
        } else {
            en
        }
    }

    pub fn menu_home_variants() -> (&'static str, &'static str) {
        ("🏠 Home", "🏠 首页")
    }

    pub fn menu_manage_providers() -> &'static str {
        let (en, zh) = menu_manage_providers_variants();
        if is_chinese() {
            zh
        } else {
            en
        }
    }

    pub fn menu_manage_providers_variants() -> (&'static str, &'static str) {
        ("🔑 Providers", "🔑 供应商")
    }

    pub fn menu_manage_mcp() -> &'static str {
        let (en, zh) = menu_manage_mcp_variants();
        if is_chinese() {
            zh
        } else {
            en
        }
    }

    pub fn menu_manage_mcp_variants() -> (&'static str, &'static str) {
        ("🔌 MCP Servers", "🔌 MCP 服务器")
    }

    pub fn menu_manage_prompts() -> &'static str {
        let (en, zh) = menu_manage_prompts_variants();
        if is_chinese() {
            zh
        } else {
            en
        }
    }

    pub fn menu_manage_prompts_variants() -> (&'static str, &'static str) {
        ("💬 Prompts", "💬 提示词")
    }

    pub fn menu_manage_config() -> &'static str {
        let (en, zh) = menu_manage_config_variants();
        if is_chinese() {
            zh
        } else {
            en
        }
    }

    pub fn menu_manage_config_variants() -> (&'static str, &'static str) {
        ("📋 Configuration", "📋 配置")
    }

    pub fn menu_manage_skills() -> &'static str {
        let (en, zh) = menu_manage_skills_variants();
        if is_chinese() {
            zh
        } else {
            en
        }
    }

    pub fn menu_manage_skills_variants() -> (&'static str, &'static str) {
        ("🧩 Skills", "🧩 技能")
    }

    pub fn menu_openclaw_workspace() -> &'static str {
        let (en, zh) = menu_openclaw_workspace_variants();
        if is_chinese() {
            zh
        } else {
            en
        }
    }

    pub fn menu_openclaw_workspace_variants() -> (&'static str, &'static str) {
        ("📁 Workspace Files", "📁 Workspace 文件管理")
    }

    pub fn menu_openclaw_env() -> &'static str {
        let (en, zh) = menu_openclaw_env_variants();
        if is_chinese() {
            zh
        } else {
            en
        }
    }

    pub fn menu_openclaw_env_variants() -> (&'static str, &'static str) {
        ("🌱 Env Variables", "🌱 环境变量")
    }

    pub fn menu_openclaw_tools() -> &'static str {
        let (en, zh) = menu_openclaw_tools_variants();
        if is_chinese() {
            zh
        } else {
            en
        }
    }

    pub fn menu_openclaw_tools_variants() -> (&'static str, &'static str) {
        ("🔐 Tool Permissions", "🔐 工具权限")
    }

    pub fn menu_openclaw_agents() -> &'static str {
        let (en, zh) = menu_openclaw_agents_variants();
        if is_chinese() {
            zh
        } else {
            en
        }
    }

    pub fn menu_openclaw_agents_variants() -> (&'static str, &'static str) {
        ("🤖 Agents Config", "🤖 Agents 配置")
    }

    pub fn menu_settings() -> &'static str {
        let (en, zh) = menu_settings_variants();
        if is_chinese() {
            zh
        } else {
            en
        }
    }

    pub fn menu_settings_variants() -> (&'static str, &'static str) {
        ("🔧 Settings", "🔧 设置")
    }

    pub fn menu_exit() -> &'static str {
        let (en, zh) = menu_exit_variants();
        if is_chinese() {
            zh
        } else {
            en
        }
    }

    pub fn menu_exit_variants() -> (&'static str, &'static str) {
        ("🚪 Exit", "🚪 退出")
    }

    // ============================================
    // SKILLS (Skills)
    // ============================================

    pub fn skills_management() -> &'static str {
        if is_chinese() {
            "技能管理"
        } else {
            "Skills Management"
        }
    }

    pub fn no_skills_installed() -> &'static str {
        if is_chinese() {
            "未安装任何 Skills。"
        } else {
            "No skills installed."
        }
    }

    pub fn skills_discover() -> &'static str {
        if is_chinese() {
            "🔎 发现/搜索 Skills"
        } else {
            "🔎 Discover/Search Skills"
        }
    }

    pub fn skills_install() -> &'static str {
        if is_chinese() {
            "⬇️  安装 Skill"
        } else {
            "⬇️  Install Skill"
        }
    }

    pub fn skills_uninstall() -> &'static str {
        if is_chinese() {
            "🗑️  卸载 Skill"
        } else {
            "🗑️  Uninstall Skill"
        }
    }

    pub fn skills_toggle_for_app() -> &'static str {
        if is_chinese() {
            "✅ 启用/禁用（当前应用）"
        } else {
            "✅ Enable/Disable (Current App)"
        }
    }

    pub fn skills_show_info() -> &'static str {
        if is_chinese() {
            "ℹ️  查看 Skill 信息"
        } else {
            "ℹ️  Skill Info"
        }
    }

    pub fn skills_sync_now() -> &'static str {
        if is_chinese() {
            "🔄 同步 Skills 到本地"
        } else {
            "🔄 Sync Skills to Live"
        }
    }

    pub fn skills_sync_method() -> &'static str {
        if is_chinese() {
            "🔗 同步方式（auto/symlink/copy）"
        } else {
            "🔗 Sync Method (auto/symlink/copy)"
        }
    }

    pub fn skills_select_sync_method() -> &'static str {
        if is_chinese() {
            "选择同步方式："
        } else {
            "Select sync method:"
        }
    }

    pub fn skills_current_sync_method(method: &str) -> String {
        if is_chinese() {
            format!("当前同步方式：{method}")
        } else {
            format!("Current sync method: {method}")
        }
    }

    pub fn skills_current_app_note(app: &str) -> String {
        if is_chinese() {
            format!("提示：启用/禁用将作用于当前应用（{app}）。")
        } else {
            format!("Note: Enable/Disable applies to the current app ({app}).")
        }
    }

    pub fn skills_scan_unmanaged() -> &'static str {
        if is_chinese() {
            "🕵️  查找已有技能"
        } else {
            "🕵️  Find Existing Skills"
        }
    }

    pub fn skills_import_from_apps() -> &'static str {
        if is_chinese() {
            "📥 导入已有技能"
        } else {
            "📥 Import Existing Skills"
        }
    }

    pub fn skills_manage_repos() -> &'static str {
        if is_chinese() {
            "📦 管理技能仓库"
        } else {
            "📦 Manage Skill Repos"
        }
    }

    pub fn skills_enter_query() -> &'static str {
        if is_chinese() {
            "输入搜索关键词（可选）："
        } else {
            "Enter search query (optional):"
        }
    }

    pub fn skills_enter_install_spec() -> &'static str {
        if is_chinese() {
            "输入技能目录，或完整标识（owner/name:directory）："
        } else {
            "Enter a skill directory, or a full key (owner/name:directory):"
        }
    }

    pub fn skills_select_skill() -> &'static str {
        if is_chinese() {
            "选择一个 Skill："
        } else {
            "Select a skill:"
        }
    }

    pub fn skills_confirm_install(name: &str, app: &str) -> String {
        if is_chinese() {
            format!("确认安装 '{name}' 并启用到 {app}？")
        } else {
            format!("Install '{name}' and enable for {app}?")
        }
    }

    pub fn skills_confirm_uninstall(name: &str) -> String {
        if is_chinese() {
            format!("确认卸载 '{name}'？")
        } else {
            format!("Uninstall '{name}'?")
        }
    }

    pub fn skills_confirm_toggle(name: &str, app: &str, enabled: bool) -> String {
        if is_chinese() {
            if enabled {
                format!("确认启用 '{name}' 到 {app}？")
            } else {
                format!("确认在 {app} 禁用 '{name}'？")
            }
        } else if enabled {
            format!("Enable '{name}' for {app}?")
        } else {
            format!("Disable '{name}' for {app}?")
        }
    }

    pub fn skills_no_unmanaged_found() -> &'static str {
        if is_chinese() {
            "未发现可导入的技能。所有技能已在 CC Switch 中统一管理。"
        } else {
            "No skills to import found. All skills are already managed by CC Switch."
        }
    }

    pub fn skills_select_unmanaged_to_import() -> &'static str {
        if is_chinese() {
            "选择要导入的技能："
        } else {
            "Select skills to import:"
        }
    }

    pub fn skills_repos_management() -> &'static str {
        if is_chinese() {
            "技能仓库管理"
        } else {
            "Skill Repos"
        }
    }

    pub fn skills_repo_list() -> &'static str {
        if is_chinese() {
            "📋 查看仓库列表"
        } else {
            "📋 List Repos"
        }
    }

    pub fn skills_repo_add() -> &'static str {
        if is_chinese() {
            "➕ 添加仓库"
        } else {
            "➕ Add Repo"
        }
    }

    pub fn skills_repo_remove() -> &'static str {
        if is_chinese() {
            "➖ 移除仓库"
        } else {
            "➖ Remove Repo"
        }
    }

    pub fn skills_repo_enter_spec() -> &'static str {
        if is_chinese() {
            "输入 GitHub 仓库（owner/name，可选 @branch）或完整 URL："
        } else {
            "Enter a GitHub repository (owner/name, optional @branch) or a full URL:"
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

    pub fn provider_added_to_app_config(id: &str, app: &str) -> String {
        if is_chinese() {
            format!("✓ 已将供应商 '{}' 添加到 {} 配置", id, app)
        } else {
            format!("✓ Added provider '{}' to {} config", id, app)
        }
    }

    pub fn restart_note() -> &'static str {
        if is_chinese() {
            "注意：请重启 CLI 客户端以应用更改。"
        } else {
            "Note: Restart your CLI client to apply the changes."
        }
    }

    pub fn live_sync_skipped_uninitialized_warning(app: &str) -> String {
        if is_chinese() {
            format!("⚠ 未检测到 {app} 客户端本地配置，已跳过写入 live 文件；先运行一次 {app} 初始化后再试。")
        } else {
            format!("⚠ Live sync skipped: {app} client not initialized; run it once to initialize, then retry.")
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

    pub fn selection_cancelled() -> &'static str {
        if is_chinese() {
            "已取消选择"
        } else {
            "Selection cancelled"
        }
    }

    pub fn invalid_selection() -> &'static str {
        if is_chinese() {
            "选择无效"
        } else {
            "Invalid selection"
        }
    }

    pub fn available_backups() -> &'static str {
        if is_chinese() {
            "可用备份"
        } else {
            "Available Backups"
        }
    }

    pub fn no_backups_found() -> &'static str {
        if is_chinese() {
            "未找到备份。"
        } else {
            "No backups found."
        }
    }

    pub fn create_backup_first_hint() -> &'static str {
        if is_chinese() {
            "请先创建备份：cc-switch config backup"
        } else {
            "Create a backup first: cc-switch config backup"
        }
    }

    pub fn found_backups(count: usize) -> String {
        if is_chinese() {
            format!("找到 {} 个备份：", count)
        } else {
            format!("Found {} backup(s):", count)
        }
    }

    pub fn select_backup_to_restore() -> &'static str {
        if is_chinese() {
            "选择要恢复的备份："
        } else {
            "Select backup to restore:"
        }
    }

    pub fn warning_title() -> &'static str {
        if is_chinese() {
            "警告："
        } else {
            "Warning:"
        }
    }

    pub fn config_restore_warning_replace() -> &'static str {
        if is_chinese() {
            "这将用所选备份替换你当前的配置。"
        } else {
            "This will replace your current configuration with the selected backup."
        }
    }

    pub fn config_restore_warning_pre_backup() -> &'static str {
        if is_chinese() {
            "系统会先创建一次当前状态的备份。"
        } else {
            "A backup of the current state will be created first."
        }
    }

    pub fn config_restore_confirm_prompt() -> &'static str {
        if is_chinese() {
            "确认继续恢复？"
        } else {
            "Continue with restore?"
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
            "Website URL (opt.):"
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

    pub fn base_url_empty_error() -> &'static str {
        if is_chinese() {
            "API 请求地址不能为空"
        } else {
            "API URL cannot be empty"
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

    pub fn select_config_file_to_edit() -> &'static str {
        if is_chinese() {
            "选择要编辑的配置文件："
        } else {
            "Select config file to edit:"
        }
    }

    pub fn provider_missing_auth_field() -> &'static str {
        if is_chinese() {
            "settings_config 中缺少 'auth' 字段"
        } else {
            "Missing 'auth' field in settings_config"
        }
    }

    pub fn provider_missing_or_invalid_config_field() -> &'static str {
        if is_chinese() {
            "settings_config 中缺少或无效的 'config' 字段"
        } else {
            "Missing or invalid 'config' field in settings_config"
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

    pub fn tui_label_mcp_short() -> &'static str {
        "MCP:"
    }

    pub fn tui_label_skills() -> &'static str {
        if is_chinese() {
            "技能:"
        } else {
            "Skills:"
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

    pub fn disabled() -> &'static str {
        if is_chinese() {
            "禁用"
        } else {
            "Disabled"
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

    pub fn skip_claude_onboarding() -> &'static str {
        if is_chinese() {
            "🚫 跳过 Claude Code 初次安装确认"
        } else {
            "🚫 Skip Claude Code onboarding confirmation"
        }
    }

    pub fn skip_claude_onboarding_label() -> &'static str {
        if is_chinese() {
            "跳过 Claude Code 初次安装确认"
        } else {
            "Skip Claude Code onboarding confirmation"
        }
    }

    pub fn skip_claude_onboarding_confirm(enable: bool, path: &str) -> String {
        if is_chinese() {
            if enable {
                format!(
                    "确认启用跳过 Claude Code 初次安装确认？\n将写入 {path}: hasCompletedOnboarding=true"
                )
            } else {
                format!(
                    "确认恢复 Claude Code 初次安装确认？\n将从 {path} 删除 hasCompletedOnboarding"
                )
            }
        } else {
            if enable {
                format!(
                    "Enable skipping Claude Code onboarding confirmation?\nWrites hasCompletedOnboarding=true to {path}"
                )
            } else {
                format!(
                    "Disable skipping Claude Code onboarding confirmation?\nRemoves hasCompletedOnboarding from {path}"
                )
            }
        }
    }

    pub fn skip_claude_onboarding_changed(enable: bool) -> String {
        if is_chinese() {
            if enable {
                "✓ 已启用：跳过 Claude Code 初次安装确认".to_string()
            } else {
                "✓ 已恢复 Claude Code 初次安装确认".to_string()
            }
        } else {
            if enable {
                "✓ Skip Claude Code onboarding confirmation enabled".to_string()
            } else {
                "✓ Claude Code onboarding confirmation restored".to_string()
            }
        }
    }

    pub fn enable_claude_plugin_integration() -> &'static str {
        if is_chinese() {
            "🔌 接管 Claude Code for VSCode 插件"
        } else {
            "🔌 Apply to Claude Code for VSCode"
        }
    }

    pub fn enable_claude_plugin_integration_label() -> &'static str {
        if is_chinese() {
            "接管 Claude Code for VSCode 插件"
        } else {
            "Apply to Claude Code for VSCode"
        }
    }

    pub fn enable_claude_plugin_integration_confirm(enable: bool, path: &str) -> String {
        if is_chinese() {
            if enable {
                format!(
                    "确认启用 Claude Code for VSCode 插件联动？\n将写入 {path}: primaryApiKey=\"any\""
                )
            } else {
                "确认关闭 Claude Code for VSCode 插件联动？".to_string()
            }
        } else {
            if enable {
                format!(
                    "Enable Claude Code for VSCode integration?\nWrites primaryApiKey=\"any\" to {path}"
                )
            } else {
                format!(
                    "Disable Claude Code for VSCode integration?\nRemoves primaryApiKey from {path}"
                )
            }
        }
    }

    pub fn enable_claude_plugin_integration_changed(enable: bool) -> String {
        if is_chinese() {
            if enable {
                "✓ 已启用 Claude Code for VSCode 插件联动".to_string()
            } else {
                "✓ 已关闭 Claude Code for VSCode 插件联动".to_string()
            }
        } else {
            if enable {
                "✓ Claude Code for VSCode integration enabled".to_string()
            } else {
                "✓ Claude Code for VSCode integration disabled".to_string()
            }
        }
    }

    pub fn claude_plugin_sync_failed_warning(err: &str) -> String {
        if is_chinese() {
            format!("⚠ Claude Code for VSCode 插件联动失败: {err}")
        } else {
            format!("⚠ Claude Code for VSCode integration failed: {err}")
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

    pub fn config_common_snippet_title() -> &'static str {
        if is_chinese() {
            "通用配置片段"
        } else {
            "Common Config Snippet"
        }
    }

    pub fn config_common_snippet_none_set() -> &'static str {
        if is_chinese() {
            "未设置通用配置片段。"
        } else {
            "No common config snippet is set."
        }
    }

    pub fn config_common_snippet_set_for_app(app: &str) -> String {
        if is_chinese() {
            format!("✓ 已为应用 '{}' 设置通用配置片段", app)
        } else {
            format!("✓ Common config snippet set for app '{}'", app)
        }
    }

    pub fn config_common_snippet_require_json_or_file() -> &'static str {
        if is_chinese() {
            "请提供 --json 或 --file"
        } else {
            "Please provide --json or --file"
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
        let is_codex = app == "codex";
        if is_chinese() {
            if is_codex {
                format!("编辑 {app} 的通用配置片段（TOML，留空则清除）：")
            } else {
                format!("编辑 {app} 的通用配置片段（JSON 对象，留空则清除）：")
            }
        } else {
            if is_codex {
                format!("Edit common config snippet for {app} (TOML; empty to clear):")
            } else {
                format!("Edit common config snippet for {app} (JSON object; empty to clear):")
            }
        }
    }

    pub fn common_config_snippet_invalid_json(err: &str) -> String {
        if is_chinese() {
            format!("JSON 无效：{err}")
        } else {
            format!("Invalid JSON: {err}")
        }
    }

    pub fn common_config_snippet_invalid_toml(err: &str) -> String {
        if is_chinese() {
            format!("TOML 无效：{err}")
        } else {
            format!("Invalid TOML: {err}")
        }
    }

    pub fn failed_to_serialize_json(err: &str) -> String {
        if is_chinese() {
            format!("序列化 JSON 失败：{err}")
        } else {
            format!("Failed to serialize JSON: {err}")
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

    pub fn common_config_snippet_no_current_provider_after_clear() -> &'static str {
        if is_chinese() {
            "当前未选择供应商，已清除通用配置片段。"
        } else {
            "No current provider selected; common config snippet cleared."
        }
    }

    pub fn common_config_snippet_applied() -> &'static str {
        if is_chinese() {
            "✓ 已在适用时刷新 live 配置（请重启对应客户端）"
        } else {
            "✓ Refreshed live config when applicable (restart the client)"
        }
    }

    pub fn common_config_snippet_apply_hint() -> &'static str {
        if is_chinese() {
            "提示：切换一次供应商即可重新写入 live 配置。"
        } else {
            "Tip: switch provider once to re-write the live config."
        }
    }

    pub fn common_config_snippet_apply_not_needed() -> &'static str {
        if is_chinese() {
            "当前配置已是最新，无需重新应用。"
        } else {
            "The current live config is already up to date; nothing to apply."
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
            "📥 导入已有 MCP 服务器"
        } else {
            "📥 Import Existing MCP Servers"
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
            format!("✓ 已导入 {count} 个 MCP 服务器")
        } else {
            format!("✓ Imported {count} MCP server(s)")
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

    // ============================================
    // TUI UPDATE (TUI 自更新)
    // ============================================

    pub fn tui_settings_check_for_updates() -> &'static str {
        if is_chinese() {
            "检查更新"
        } else {
            "Check for Updates"
        }
    }

    pub fn tui_update_checking_title() -> &'static str {
        if is_chinese() {
            "检查更新中"
        } else {
            "Checking for Updates"
        }
    }

    pub fn tui_update_available_title() -> &'static str {
        if is_chinese() {
            "发现新版本"
        } else {
            "Update Available"
        }
    }

    pub fn tui_update_downloading_title() -> &'static str {
        if is_chinese() {
            "正在更新"
        } else {
            "Updating"
        }
    }

    pub fn tui_update_result_title() -> &'static str {
        if is_chinese() {
            "更新结果"
        } else {
            "Update Result"
        }
    }

    pub fn tui_update_version_info(current: &str, new: &str) -> String {
        if is_chinese() {
            format!("当前: v{current}  →  最新: {new}")
        } else {
            format!("Current: v{current}  →  Latest: {new}")
        }
    }

    pub fn tui_update_btn_update() -> &'static str {
        if is_chinese() {
            "更新"
        } else {
            "Update"
        }
    }

    pub fn tui_update_btn_cancel() -> &'static str {
        if is_chinese() {
            "取消"
        } else {
            "Cancel"
        }
    }

    pub fn tui_update_downloading_kb(kb: u64) -> String {
        if is_chinese() {
            format!("已下载 {kb} KB")
        } else {
            format!("Downloaded {kb} KB")
        }
    }

    pub fn tui_update_downloading_progress(pct: u64, downloaded_kb: u64, total_kb: u64) -> String {
        if is_chinese() {
            format!("{pct}%  ({downloaded_kb} / {total_kb} KB)")
        } else {
            format!("{pct}%  ({downloaded_kb} / {total_kb} KB)")
        }
    }

    pub fn tui_update_success(tag: &str) -> String {
        if is_chinese() {
            format!("已更新到 {tag}，按 Enter 退出")
        } else {
            format!("Updated to {tag}. Press Enter to exit.")
        }
    }

    pub fn tui_update_err_worker_unavailable() -> &'static str {
        if is_chinese() {
            "更新服务不可用"
        } else {
            "Update worker unavailable"
        }
    }

    pub fn tui_update_err_check_first() -> &'static str {
        if is_chinese() {
            "请先检查更新"
        } else {
            "Please check for updates first"
        }
    }

    pub fn tui_toast_already_latest(v: &str) -> String {
        if is_chinese() {
            format!("已是最新版本 v{v}")
        } else {
            format!("Already on latest v{v}")
        }
    }

    pub fn tui_toast_update_downgrade(current: &str, target: &str) -> String {
        if is_chinese() {
            format!("当前 v{current} 比 {target} 更新")
        } else {
            format!("Current v{current} is newer than {target}")
        }
    }

    pub fn tui_toast_update_check_failed(err: &str) -> String {
        if is_chinese() {
            format!("检查更新失败: {err}")
        } else {
            format!("Update check failed: {err}")
        }
    }

    pub fn tui_key_hide() -> &'static str {
        if is_chinese() {
            "隐藏"
        } else {
            "hide"
        }
    }

    pub fn tui_toast_update_bg_success(tag: &str) -> String {
        if is_chinese() {
            format!("后台更新到 {tag} 完成")
        } else {
            format!("Background update to {tag} complete")
        }
    }

    pub fn tui_toast_update_bg_failed(err: &str) -> String {
        if is_chinese() {
            format!("后台更新失败: {err}")
        } else {
            format!("Background update failed: {err}")
        }
    }

    pub fn tui_provider_imported_live_config_name() -> &'static str {
        if is_chinese() {
            "导入的 Live 配置"
        } else {
            "Imported Live Config"
        }
    }

    pub fn tui_codex_imported_live_config_name() -> &'static str {
        if is_chinese() {
            "导入的 Codex Live 配置"
        } else {
            "Imported Codex Live Config"
        }
    }

    pub fn tui_toast_provider_live_config_imported() -> &'static str {
        if is_chinese() {
            "已将当前 live 配置导入为供应商"
        } else {
            "Imported the current live config as a provider"
        }
    }

    pub fn tui_toast_codex_live_config_imported() -> &'static str {
        if is_chinese() {
            "已将当前 Codex live 配置导入为供应商"
        } else {
            "Imported the current Codex live config as a provider"
        }
    }

    pub fn tui_provider_switch_first_use_title() -> &'static str {
        if is_chinese() {
            "检测到现有 live 配置"
        } else {
            "Existing live config detected"
        }
    }

    pub fn tui_provider_switch_first_use_message(path: &str) -> String {
        if is_chinese() {
            format!("首次切换前，检测到 {path} 已存在配置。你可以先导入当前 live 配置，或继续切换并覆盖它。")
        } else {
            format!("Before the first switch, an existing config was found at {path}. You can import the current live config first, or continue and overwrite it.")
        }
    }

    pub fn tui_codex_provider_switch_first_use_title() -> &'static str {
        if is_chinese() {
            "检测到现有 Codex live 配置"
        } else {
            "Existing Codex live config detected"
        }
    }

    pub fn tui_codex_provider_switch_first_use_message(paths: &str) -> String {
        if is_chinese() {
            format!("首次切换前，检测到 {paths} 已存在配置。你可以先导入当前 live 配置，或继续切换并覆盖它。")
        } else {
            format!("Before the first switch, existing config was found in {paths}. You can import the current live config first, or continue and overwrite it.")
        }
    }

    pub fn tui_provider_switch_first_use_import_button() -> &'static str {
        if is_chinese() {
            "导入当前配置"
        } else {
            "Import Current Config"
        }
    }

    pub fn tui_provider_switch_first_use_continue_button() -> &'static str {
        if is_chinese() {
            "继续切换"
        } else {
            "Continue Switching"
        }
    }

    pub fn tui_provider_switch_first_use_cancel_button() -> &'static str {
        if is_chinese() {
            "取消"
        } else {
            "Cancel"
        }
    }

    pub fn tui_provider_switch_shared_config_tip_title() -> &'static str {
        if is_chinese() {
            "已切换供应商"
        } else {
            "Provider Switched"
        }
    }

    pub fn tui_provider_switch_shared_config_tip_message() -> String {
        if is_chinese() {
            "当前 live 配置还会叠加通用配置片段；如果结果和预期不同，请检查 Common Snippet。"
                .to_string()
        } else {
            "The live config still applies the shared common snippet; if the result looks different than expected, check Common Snippet."
                .to_string()
        }
    }

    pub fn tui_codex_provider_switch_shared_config_tip_message() -> String {
        if is_chinese() {
            "当前 Codex live 配置还会叠加通用配置片段；如果结果和预期不同，请检查 Common Snippet。"
                .to_string()
        } else {
            "The Codex live config still applies the shared common snippet; if the result looks different than expected, check Common Snippet."
                .to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{texts, use_test_language, Language};
    use std::sync::mpsc;
    use std::thread;

    #[test]
    fn website_url_label_keeps_optional_with_abbrev() {
        let label = texts::website_url_label();
        assert_eq!(label, "Website URL (opt.):");
        assert!(label.contains("(opt.)"));
        assert!(!label.contains("(optional)"));
    }

    #[test]
    fn chinese_tui_copy_avoids_key_mixed_english_labels() {
        let _lang = use_test_language(Language::Chinese);

        assert_eq!(texts::tui_home_section_connection(), "连接信息");
        assert_eq!(texts::tui_home_status_online(), "在线");
        assert_eq!(texts::tui_home_status_offline(), "离线");
        assert_eq!(texts::tui_label_mcp_servers_active(), "已启用");
        assert_eq!(texts::skills_management(), "技能管理");
        assert_eq!(texts::menu_manage_mcp(), "🔌 MCP 服务器");

        let help = texts::tui_help_text();
        assert!(help.contains("供应商：Enter 详情"));
        assert!(help.contains("供应商详情：s 切换/添加移除"));
        assert!(help.contains("提示词：Enter 查看"));
        assert!(help.contains("技能：Enter 详情"));
        assert!(help.contains("配置：Enter 打开/执行"));
        assert!(help.contains("设置：Enter 应用"));
        assert!(!help.contains("Providers:"));
        assert!(!help.contains("Provider Detail:"));
        assert!(!help.contains("Skills:"));
        assert!(!help.contains("Config:"));
        assert!(!help.contains("Settings:"));
    }

    #[test]
    fn proxy_dashboard_copy_is_fully_localized_in_chinese() {
        let _lang = use_test_language(Language::Chinese);

        assert_eq!(texts::tui_home_section_connection(), "连接信息");
        assert_eq!(
            texts::tui_proxy_dashboard_failover_copy(),
            "仅做手动路由，不会自动切换供应商。"
        );
        assert_eq!(
            texts::tui_proxy_dashboard_manual_routing_copy("Claude"),
            "手动路由：Claude 的流量会通过 cc-switch。"
        );
    }

    #[test]
    fn openclaw_provider_status_copy_is_fully_localized_in_chinese() {
        let _lang = use_test_language(Language::Chinese);

        assert_eq!(texts::tui_label_openclaw_status(), "状态");
        assert_eq!(texts::tui_label_openclaw_model(), "模型");
        assert_eq!(texts::tui_openclaw_status_default(), "默认");
        assert_eq!(
            texts::tui_openclaw_status_in_config_and_saved(),
            "配置中 + 已保存"
        );
        assert_eq!(texts::tui_openclaw_status_live_only(), "仅当前配置");
        assert_eq!(texts::tui_openclaw_status_saved_only(), "仅已保存");
        assert_eq!(texts::tui_openclaw_status_untracked(), "未跟踪");
    }

    #[test]
    fn test_language_override_does_not_leak_across_threads() {
        let _lang = use_test_language(Language::English);
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();

        let handle = thread::spawn(move || {
            let _lang = use_test_language(Language::Chinese);
            ready_tx.send(()).expect("signal ready");
            release_rx.recv().expect("wait for release");
        });

        ready_rx.recv().expect("wait for child language override");

        assert_eq!(
            texts::tui_home_section_connection(),
            "Connection Details",
            "child thread language override should not affect this test thread"
        );

        release_tx.send(()).expect("release child thread");
        handle.join().expect("join child thread");
    }
}
