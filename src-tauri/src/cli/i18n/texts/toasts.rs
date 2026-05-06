use super::is_chinese;
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

pub fn tui_toast_provider_live_config_imported() -> &'static str {
    if is_chinese() {
        "已将当前 Claude 配置导入为供应商。"
    } else {
        "Imported the current Claude config as a provider."
    }
}

pub fn tui_toast_codex_live_config_imported() -> &'static str {
    if is_chinese() {
        "已将当前 Codex 配置导入为供应商。"
    } else {
        "Imported the current Codex config as a provider."
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
        "请在 JSON 中填写 name。"
    } else {
        "Please fill in name in JSON."
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

pub fn tui_toast_prompt_created() -> &'static str {
    if is_chinese() {
        "提示词已创建。"
    } else {
        "Prompt created."
    }
}

pub fn tui_toast_prompt_renamed() -> &'static str {
    if is_chinese() {
        "提示词已重命名。"
    } else {
        "Prompt renamed."
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
