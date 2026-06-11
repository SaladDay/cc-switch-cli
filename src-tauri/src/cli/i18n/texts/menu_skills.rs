use super::is_chinese;
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

pub fn menu_usage() -> &'static str {
    let (en, zh) = menu_usage_variants();
    if is_chinese() { zh } else { en }
}

pub fn menu_usage_variants() -> (&'static str, &'static str) {
    ("📊 Usage", "📊 使用统计")
}

pub fn menu_pricing() -> &'static str {
    let (en, zh) = menu_pricing_variants();
    if is_chinese() { zh } else { en }
}

pub fn menu_pricing_variants() -> (&'static str, &'static str) {
    ("💵 Pricing", "💵 模型定价")
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
