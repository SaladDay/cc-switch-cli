use crate::app_config::AppType;

/// Whether we should write/delete "live" config files for a given app.
///
/// Policy: **auto** (safe default)
/// - If the target app looks uninitialized (its config dir / key live file is missing),
///   skip live writes/deletes and do **not** create any directories/files.
pub(crate) fn should_sync_live(app_type: &AppType) -> bool {
    match app_type {
        // Claude is considered initialized if either:
        // - ~/.claude (settings dir) exists, or
        // - ~/.claude.json (MCP file) exists
        AppType::Claude => {
            crate::config::get_claude_config_dir().exists()
                || crate::config::get_claude_mcp_path().exists()
        }
        // Codex is considered initialized if ~/.codex (or override dir) exists.
        AppType::Codex => crate::codex_config::get_codex_config_dir().exists(),
        // Gemini is considered initialized if ~/.gemini (or override dir) exists.
        AppType::Gemini => crate::gemini_config::get_gemini_dir().exists(),
        // OpenCode is considered initialized if ~/.config/opencode (or override dir) exists.
        AppType::OpenCode => crate::opencode_config::get_opencode_dir().exists(),
        // OpenClaw is considered initialized if ~/.openclaw (or override dir) exists.
        AppType::OpenClaw => crate::openclaw_config::get_openclaw_dir().exists(),
    }
}
