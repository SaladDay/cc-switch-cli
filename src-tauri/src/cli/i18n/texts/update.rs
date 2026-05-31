use super::is_chinese;
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

pub fn tui_toast_update_homebrew_required(current: &str, target: &str) -> String {
    if is_chinese() {
        format!("发现新版本 {target}（当前 v{current}）\n请使用 brew upgrade cc-switch 更新")
    } else {
        format!(
            "Update {target} is available (current v{current}).\nPlease update with: brew upgrade cc-switch"
        )
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
