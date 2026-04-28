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
        "[ ]  切换应用\n←→  切换菜单/内容焦点\n↑↓  移动\n/   过滤\nEsc  返回\n?   显示/关闭帮助\n\n页面快捷键（在页面内容区顶部显示）：\n- 供应商：Enter 详情，s 切换，a 添加，e 编辑，d 删除，t 测速，c 健康检查\n- 供应商详情：s 切换，e 编辑，t 测速，c 健康检查\n- MCP：x 启用/禁用(当前应用)，m 选择应用，a 添加，e 编辑，i 导入已有，d 删除\n- 提示词：Enter 查看，a 激活，x 取消激活(当前)，e 编辑，d 删除\n- 技能：Enter 详情，x 启用/禁用(当前应用)，m 选择应用，d 卸载，i 导入已有\n- 配置：Enter 打开/执行，e 编辑片段\n- 设置：Enter 应用"
    } else {
        "[ ]  switch app\n←→  focus menu/content\n↑↓  move\n/   filter\nEsc  back\n?   toggle help\n\nPage keys (shown at the top of each page):\n- Providers: Enter details, s switch, a add, e edit, d delete, t speedtest, c stream check\n- Provider Detail: s switch, e edit, t speedtest, c stream check\n- MCP: x toggle current, m select apps, a add, e edit, i import existing, d delete\n- Prompts: Enter view, a activate, x deactivate active, e edit, d delete\n- Skills: Enter details, x toggle current, m select apps, d uninstall, i import existing\n- Config: Enter open/run, e edit snippet\n- Settings: Enter apply"
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

pub fn tui_provider_switch_first_use_title() -> &'static str {
    if is_chinese() {
        "已有 Claude 配置"
    } else {
        "Existing Claude Config"
    }
}

pub fn tui_provider_switch_first_use_message(path: &str) -> String {
    if is_chinese() {
        format!(
            "⚠ 检测到已有 Claude 配置文件 ({path})。\n切换供应商将覆盖此文件。\n建议先将当前配置导入为供应商。"
        )
    } else {
        format!(
            "WARNING: An existing Claude config file was found at {path}.\nSwitching providers will overwrite this file.\nImport the current config as a provider first if you want to keep it."
        )
    }
}

pub fn tui_codex_provider_switch_first_use_title() -> &'static str {
    if is_chinese() {
        "已有 Codex 配置"
    } else {
        "Existing Codex Config"
    }
}
