use std::path::PathBuf;
use std::sync::mpsc;

use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::error::AppError;
use crate::services::McpService;

use super::super::app::{App, LoadingKind, Overlay, TextViewState, ToastKind};
use super::super::data::{load_proxy_config, load_state, UiData};
use super::super::runtime_systems::{ProxyReq, RequestTracker};

pub(crate) fn import_mcp_for_current_app_with<FImport, FLoad>(
    app: &mut App,
    data: &mut UiData,
    import: FImport,
    load_data: FLoad,
) -> Result<(), AppError>
where
    FImport: FnOnce(&AppType) -> Result<usize, AppError>,
    FLoad: FnOnce(&AppType) -> Result<UiData, AppError>,
{
    let count = import(&app.app_type)?;
    app.push_toast(texts::tui_toast_mcp_imported(count), ToastKind::Info);
    *data = load_data(&app.app_type)?;
    Ok(())
}

pub(crate) fn import_mcp_for_current_app(app: &mut App, data: &mut UiData) -> Result<(), AppError> {
    import_mcp_for_current_app_with(
        app,
        data,
        |app_type| {
            let state = load_state()?;
            match app_type {
                AppType::Claude => McpService::import_from_claude(&state),
                AppType::Codex => McpService::import_from_codex(&state),
                AppType::Gemini => McpService::import_from_gemini(&state),
                AppType::OpenCode => McpService::import_from_opencode(&state),
                AppType::OpenClaw => Ok(0),
            }
        },
        UiData::load,
    )
}

pub(crate) fn open_proxy_help_overlay_with<F>(
    app: &mut App,
    data: &UiData,
    load: F,
) -> Result<(), AppError>
where
    F: FnOnce() -> Result<Option<crate::proxy::ProxyConfig>, AppError>,
{
    let proxy_config = load()?;
    app.open_proxy_help_view(data, proxy_config.as_ref());
    Ok(())
}

pub(crate) fn app_display_name(app_type: &AppType) -> &'static str {
    match app_type {
        AppType::Claude => "Claude",
        AppType::Codex => "Codex",
        AppType::Gemini => "Gemini",
        AppType::OpenCode => "OpenCode",
        AppType::OpenClaw => "OpenClaw",
    }
}

pub(crate) fn queue_managed_proxy_action(
    app: &mut App,
    proxy_req_tx: Option<&mpsc::Sender<ProxyReq>>,
    proxy_loading: &mut RequestTracker,
    app_type: AppType,
    enabled: bool,
) -> Result<(), AppError> {
    let Some(tx) = proxy_req_tx else {
        app.push_toast(
            texts::tui_toast_proxy_request_failed(texts::tui_error_proxy_worker_unavailable()),
            ToastKind::Warning,
        );
        return Ok(());
    };

    let request_id = proxy_loading.start();
    app.overlay = Overlay::Loading {
        kind: LoadingKind::Proxy,
        title: if enabled {
            texts::tui_proxy_loading_title_start().to_string()
        } else {
            texts::tui_proxy_loading_title_stop().to_string()
        },
        message: texts::tui_loading().to_string(),
    };

    if let Err(err) = tx.send(ProxyReq::SetManagedSessionForCurrentApp {
        request_id,
        app_type,
        enabled,
    }) {
        proxy_loading.cancel();
        app.overlay = Overlay::None;
        app.push_toast(
            texts::tui_toast_proxy_request_failed(&err.to_string()),
            ToastKind::Error,
        );
    }

    Ok(())
}

pub(super) fn refresh_common_snippet_overlay(app: &mut App, data: &UiData) {
    let Overlay::CommonSnippetView { app_type, view } = &mut app.overlay else {
        return;
    };

    let snippet = if app_type == &app.app_type {
        data.config.common_snippet.clone()
    } else {
        data.config
            .common_snippets
            .get(app_type)
            .cloned()
            .unwrap_or_default()
    };
    let snippet = if snippet.trim().is_empty() {
        texts::tui_default_common_snippet_for_app(app_type.as_str()).to_string()
    } else {
        snippet
    };

    view.title = texts::tui_common_snippet_title(app_type.as_str());
    view.lines = snippet.lines().map(|s| s.to_string()).collect();
    view.scroll = 0;
}

pub(crate) fn run_external_editor_for_current_editor(
    app: &mut App,
    open_external_editor: impl FnOnce(&str) -> Result<String, AppError>,
) -> Result<(), AppError> {
    let Some(current_text) = app.editor.as_ref().map(|editor| editor.text()) else {
        return Ok(());
    };

    let edited_text = open_external_editor(&current_text)?;
    if let Some(editor) = app.editor.as_mut() {
        editor.replace_text(edited_text);
    }

    Ok(())
}

pub(super) fn export_target(path: String) -> PathBuf {
    PathBuf::from(path)
}

pub(super) fn text_view(title: String, content: String) -> Overlay {
    Overlay::TextView(TextViewState {
        title,
        lines: content.lines().map(|s| s.to_string()).collect(),
        scroll: 0,
        action: None,
    })
}

pub(super) fn open_proxy_help(app: &mut App, data: &UiData) -> Result<(), AppError> {
    open_proxy_help_overlay_with(app, data, load_proxy_config)
}
