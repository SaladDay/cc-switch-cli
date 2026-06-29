use super::is_chinese;
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
            format!("确认恢复 Claude Code 初次安装确认？\n将从 {path} 删除 hasCompletedOnboarding")
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

pub fn codex_unified_session_history_label() -> &'static str {
    if is_chinese() {
        "统一 Codex 会话历史"
    } else {
        "Unified Codex session history"
    }
}

pub fn codex_unified_session_history_confirm(enable: bool) -> String {
    if is_chinese() {
        if enable {
            "确认开启统一 Codex 会话历史？\n官方订阅将使用共享 custom 供应商标识运行；已有官方会话不会自动迁移，可用 CLI 命令 settings codex-history migrate-existing 单独迁移。".to_string()
        } else {
            "确认关闭统一 Codex 会话历史？\n不会自动恢复已迁移的会话；如需恢复，请使用 CLI 命令 settings codex-history restore。".to_string()
        }
    } else {
        if enable {
            "Enable unified Codex session history?\nOfficial subscriptions will use the shared custom provider id. Existing official sessions are not migrated automatically; use settings codex-history migrate-existing from the CLI if needed.".to_string()
        } else {
            "Disable unified Codex session history?\nMigrated sessions are not restored automatically; use settings codex-history restore from the CLI if needed.".to_string()
        }
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
        "请提供 --snippet（或兼容别名 --json）或 --file"
    } else {
        "Please provide --snippet (or the compatibility alias --json) or --file"
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
        "现在在适用时刷新 live 配置？"
    } else {
        "Refresh live config now when applicable?"
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

pub fn common_config_snippet_apply_not_needed() -> &'static str {
    if is_chinese() {
        "当前应用使用 additive 模式，无需执行当前 provider 刷新。"
    } else {
        "This app uses additive mode; no current-provider refresh is needed."
    }
}

pub fn common_config_snippet_apply_hint() -> &'static str {
    if is_chinese() {
        "提示：切换一次供应商即可重新写入 live 配置。"
    } else {
        "Tip: switch provider once to re-write the live config."
    }
}

pub fn common_config_snippet_extracted() -> &'static str {
    if is_chinese() {
        "已从当前编辑内容提取通用配置片段"
    } else {
        "Extracted common config snippet from current edits"
    }
}

pub fn common_config_snippet_formatted() -> &'static str {
    if is_chinese() {
        "已格式化通用配置片段"
    } else {
        "Formatted common config snippet"
    }
}

pub fn common_config_snippet_extract_empty() -> &'static str {
    if is_chinese() {
        "当前编辑内容没有可提取的通用配置"
    } else {
        "No common config found in the current edits"
    }
}

pub fn tui_common_config_notice_title() -> &'static str {
    if is_chinese() {
        "关于通用配置"
    } else {
        "About Common Config"
    }
}

pub fn tui_common_config_notice_message(app: &str) -> String {
    if is_chinese() {
        format!(
            "通用配置适合保存多个 {app} 供应商共享的插件、环境变量和工具配置。\
             \n\n有可用片段时，新建供应商会默认勾选“添加通用配置”。\
             \n\n如果在当前表单里新增了插件、hooks 或环境变量，可以在“通用配置”编辑器里按 F4 从当前编辑内容提取，再按 Ctrl+S 保存片段。"
        )
    } else {
        format!(
            "Common Config is for plugin, environment, and tool settings shared by multiple {app} providers.\
             \n\nWhen a usable snippet exists, new providers will default to attaching it.\
             \n\nAfter adding plugins, hooks, or environment variables in this form, open Common Config, press F4 to extract from the current edits, then press Ctrl+S to save the snippet."
        )
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
