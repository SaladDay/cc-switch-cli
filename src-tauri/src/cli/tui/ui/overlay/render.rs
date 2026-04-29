use super::super::theme;
use super::super::*;

pub(crate) fn render_overlay(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    theme: &theme::Theme,
) {
    let content_area = content_pane_rect(frame.area(), theme);

    match &app.overlay {
        Overlay::None => {}
        Overlay::Help => super::basic::render_help_overlay(frame, content_area, theme),
        Overlay::Confirm(confirm) => {
            super::basic::render_confirm_overlay(frame, content_area, theme, confirm)
        }
        Overlay::ProviderSwitchFirstUseConfirm {
            title,
            message,
            selected,
            ..
        } => super::basic::render_provider_switch_first_use_overlay(
            frame,
            content_area,
            theme,
            title,
            message,
            *selected,
        ),
        Overlay::TextInput(input) => {
            super::basic::render_text_input_overlay(frame, content_area, theme, input)
        }
        Overlay::BackupPicker { selected } => {
            super::basic::render_backup_picker_overlay(frame, data, content_area, theme, *selected)
        }
        Overlay::TextView(view) => super::basic::render_text_view_overlay(
            frame,
            content_area,
            theme,
            &view.title,
            &view.lines,
            view.scroll,
            view.action.is_some(),
        ),
        Overlay::CommonSnippetPicker { selected } => {
            super::basic::render_common_snippet_picker_overlay(
                frame,
                content_area,
                theme,
                *selected,
            )
        }
        Overlay::CommonSnippetView { view, .. } => {
            super::basic::render_common_snippet_view_overlay(
                frame,
                content_area,
                theme,
                &view.title,
                &view.lines,
                view.scroll,
            )
        }
        Overlay::ClaudeModelPicker { selected, editing } => {
            super::pickers::render_claude_model_picker_overlay(
                frame,
                app,
                content_area,
                theme,
                *selected,
                *editing,
            )
        }
        Overlay::ClaudeApiFormatPicker { selected } => {
            super::pickers::render_claude_api_format_picker_overlay(
                frame,
                app,
                content_area,
                theme,
                *selected,
            )
        }
        Overlay::ModelFetchPicker {
            input,
            query,
            fetching,
            models,
            error,
            selected_idx,
            ..
        } => super::pickers::render_model_fetch_picker_overlay(
            frame,
            content_area,
            theme,
            input,
            query,
            *fetching,
            models,
            error.as_deref(),
            *selected_idx,
        ),
        Overlay::OpenClawToolsProfilePicker { selected } => {
            super::pickers::render_openclaw_tools_profile_picker_overlay(
                frame,
                app,
                content_area,
                theme,
                *selected,
            )
        }
        Overlay::OpenClawAgentsFallbackPicker {
            selected, options, ..
        } => super::pickers::render_openclaw_agents_fallback_picker_overlay(
            frame,
            app,
            content_area,
            theme,
            *selected,
            options,
        ),
        Overlay::McpAppsPicker {
            name,
            selected,
            apps,
            ..
        } => super::pickers::render_mcp_apps_picker_overlay(
            frame,
            content_area,
            theme,
            name,
            *selected,
            apps,
        ),
        Overlay::McpTypePicker { selected } => {
            super::pickers::render_mcp_type_picker_overlay(frame, content_area, theme, *selected)
        }
        Overlay::VisibleAppsPicker { selected, apps } => {
            super::pickers::render_visible_apps_picker_overlay(
                frame,
                content_area,
                theme,
                *selected,
                apps,
            )
        }
        Overlay::SkillsAppsPicker {
            name,
            selected,
            apps,
            ..
        } => super::pickers::render_skills_apps_picker_overlay(
            frame,
            content_area,
            theme,
            name,
            *selected,
            apps,
        ),
        Overlay::SkillsImportPicker {
            skills,
            selected_idx,
            selected,
        } => super::pickers::render_skills_import_picker_overlay(
            frame,
            content_area,
            theme,
            skills,
            *selected_idx,
            selected,
        ),
        Overlay::SkillsSyncMethodPicker { selected } => {
            super::pickers::render_skills_sync_method_picker_overlay(
                frame,
                data,
                content_area,
                theme,
                *selected,
            )
        }
        Overlay::McpEnvPicker { selected } => super::mcp_env::render_mcp_env_picker_overlay(
            frame,
            app,
            content_area,
            theme,
            *selected,
        ),
        Overlay::McpEnvEntryEditor(_) => super::mcp_env::render_mcp_env_entry_editor_overlay(
            frame,
            content_area,
            theme,
            &app.overlay,
        ),
        Overlay::Loading {
            kind,
            title,
            message,
        } => super::status::render_loading_overlay(
            frame,
            app,
            content_area,
            theme,
            *kind,
            title,
            message,
        ),
        Overlay::SpeedtestRunning { url } => {
            super::status::render_speedtest_running_overlay(frame, content_area, theme, url)
        }
        Overlay::SpeedtestResult { url, lines, scroll } => {
            super::status::render_speedtest_result_overlay(
                frame,
                content_area,
                theme,
                url,
                lines,
                *scroll,
            )
        }
        Overlay::StreamCheckRunning { provider_name, .. } => {
            super::status::render_stream_check_running_overlay(
                frame,
                content_area,
                theme,
                provider_name,
            )
        }
        Overlay::StreamCheckResult {
            provider_name,
            lines,
            scroll,
        } => super::status::render_stream_check_result_overlay(
            frame,
            content_area,
            theme,
            provider_name,
            lines,
            *scroll,
        ),
        Overlay::UpdateAvailable {
            current,
            latest,
            selected,
        } => super::status::render_update_available_overlay(
            frame,
            content_area,
            theme,
            current,
            latest,
            *selected,
        ),
        Overlay::UpdateDownloading { downloaded, total } => {
            super::status::render_update_downloading_overlay(
                frame,
                content_area,
                theme,
                *downloaded,
                *total,
            )
        }
        Overlay::UpdateResult { success, message } => super::status::render_update_result_overlay(
            frame,
            content_area,
            theme,
            *success,
            message,
        ),
    }
}
