use super::is_chinese;
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
        "技能发现会从这里已启用的仓库加载列表。私有 GitHub 仓库请使用 CLI 绑定 token 环境变量：cc-switch skills repos add <repo> --token-env ENV"
    } else {
        "Skill discovery loads results from the repositories enabled here. Use the CLI to bind a token env var for private GitHub repos: cc-switch skills repos add <repo> --token-env ENV"
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
        "输入 GitHub 仓库（owner/name，可选 @branch）或完整 URL：\n私有仓库的 token_env 仅支持 CLI：cc-switch skills repos add <repo> --token-env ENV"
    } else {
        "Enter a GitHub repository (owner/name, optional @branch) or a full URL:\nPrivate repos must bind token_env via CLI: cc-switch skills repos add <repo> --token-env ENV"
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
