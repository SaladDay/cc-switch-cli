mod app;
mod data;
mod form;
mod route;
mod terminal;
mod theme;
mod ui;

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crossterm::event::{self, KeyEventKind};
use serde_json::json;
use serde_json::Value;

use crate::app_config::AppType;
use crate::cli::i18n::{set_language, texts};
use crate::error::AppError;
use crate::provider::Provider;
use crate::services::{
    skill::SkillRepo, ConfigService, EndpointLatency, McpService, PromptService, ProviderService,
    SkillService, SyncDecision, WebDavSyncService,
};
use crate::settings::{
    get_webdav_sync_settings, set_webdav_sync_settings, webdav_jianguoyun_preset,
    WebDavSyncSettings,
};

use app::{Action, App, EditorSubmit, LoadingKind, Overlay, TextViewState, ToastKind};
use data::{load_state, UiData};
use form::FormState;
use terminal::{PanicRestoreHookGuard, TuiTerminal};

fn command_lookup_name(raw: &str) -> Option<&str> {
    raw.split_whitespace().next()
}

enum SpeedtestMsg {
    Finished {
        url: String,
        result: Result<Vec<EndpointLatency>, String>,
    },
}

enum LocalEnvReq {
    Refresh,
}

enum LocalEnvMsg {
    Finished {
        result: Vec<crate::services::local_env_check::ToolCheckResult>,
    },
}

enum SkillsReq {
    Discover { query: String },
    Install { spec: String, app: AppType },
}

enum SkillsMsg {
    DiscoverFinished {
        query: String,
        result: Result<Vec<crate::services::skill::Skill>, String>,
    },
    InstallFinished {
        spec: String,
        result: Result<crate::services::skill::InstalledSkill, String>,
    },
}

#[derive(Debug, Clone)]
enum WebDavReqKind {
    CheckConnection,
    Upload,
    Download,
    JianguoyunQuickSetup { username: String, password: String },
}

#[derive(Debug, Clone)]
struct WebDavReq {
    request_id: u64,
    kind: WebDavReqKind,
}

#[derive(Debug, Clone)]
enum WebDavDone {
    ConnectionChecked,
    Uploaded {
        decision: SyncDecision,
        message: String,
    },
    Downloaded {
        decision: SyncDecision,
        message: String,
    },
    JianguoyunConfigured,
}

#[derive(Debug, Clone)]
enum WebDavErr {
    Generic(String),
    QuickSetupSave(String),
    QuickSetupCheck(String),
}

enum WebDavMsg {
    Finished {
        request_id: u64,
        req: WebDavReqKind,
        result: Result<WebDavDone, WebDavErr>,
    },
}

struct SpeedtestSystem {
    req_tx: mpsc::Sender<String>,
    result_rx: mpsc::Receiver<SpeedtestMsg>,
    _handle: std::thread::JoinHandle<()>,
}

struct LocalEnvSystem {
    req_tx: mpsc::Sender<LocalEnvReq>,
    result_rx: mpsc::Receiver<LocalEnvMsg>,
    _handle: std::thread::JoinHandle<()>,
}

struct SkillsSystem {
    req_tx: mpsc::Sender<SkillsReq>,
    result_rx: mpsc::Receiver<SkillsMsg>,
    _handle: std::thread::JoinHandle<()>,
}

struct WebDavSystem {
    req_tx: mpsc::Sender<WebDavReq>,
    result_rx: mpsc::Receiver<WebDavMsg>,
    _handle: std::thread::JoinHandle<()>,
}

enum UpdateReq {
    Check { request_id: u64 },
    Download,
}

enum UpdateMsg {
    CheckFinished {
        request_id: u64,
        result: Result<crate::cli::commands::update::UpdateCheckInfo, String>,
    },
    DownloadProgress {
        downloaded: u64,
        total: Option<u64>,
    },
    DownloadFinished(Result<String, String>),
}

struct UpdateSystem {
    req_tx: mpsc::Sender<UpdateReq>,
    result_rx: mpsc::Receiver<UpdateMsg>,
    _handle: std::thread::JoinHandle<()>,
}

#[derive(Debug, Default, Clone, Copy)]
struct RequestTracker {
    seq: u64,
    active: Option<u64>,
}

impl RequestTracker {
    fn start(&mut self) -> u64 {
        self.seq = self.seq.wrapping_add(1);
        self.active = Some(self.seq);
        self.seq
    }

    fn cancel(&mut self) {
        self.active = None;
    }

    fn is_stale(&self, request_id: u64) -> bool {
        matches!(self.active, Some(active_request_id) if active_request_id != request_id)
    }

    fn finish_if_active(&mut self, request_id: u64) -> bool {
        if self.active == Some(request_id) {
            self.active = None;
            true
        } else {
            false
        }
    }
}

pub fn run(app_override: Option<AppType>) -> Result<(), AppError> {
    let _panic_hook = PanicRestoreHookGuard::install();
    let mut terminal = TuiTerminal::new()?;
    let mut app = App::new(app_override);
    let mut data = UiData::load(&app.app_type)?;

    let tick_rate = Duration::from_millis(200);
    let mut last_tick = Instant::now();
    let mut webdav_loading = RequestTracker::default();
    let mut update_check = RequestTracker::default();

    let speedtest = match start_speedtest_system() {
        Ok(system) => Some(system),
        Err(err) => {
            app.push_toast(
                texts::tui_toast_speedtest_unavailable(&err.to_string()),
                ToastKind::Warning,
            );
            None
        }
    };

    let skills = match start_skills_system() {
        Ok(system) => Some(system),
        Err(err) => {
            app.push_toast(
                texts::tui_toast_skills_worker_unavailable(&err.to_string()),
                ToastKind::Warning,
            );
            None
        }
    };

    let local_env = match start_local_env_system() {
        Ok(system) => {
            if let Err(err) = system.req_tx.send(LocalEnvReq::Refresh) {
                app.local_env_loading = false;
                app.push_toast(
                    texts::tui_toast_local_env_check_request_failed(&err.to_string()),
                    ToastKind::Warning,
                );
            }
            Some(system)
        }
        Err(err) => {
            app.local_env_loading = false;
            app.push_toast(
                texts::tui_toast_local_env_check_unavailable(&err.to_string()),
                ToastKind::Warning,
            );
            None
        }
    };

    let webdav = match start_webdav_system() {
        Ok(system) => Some(system),
        Err(err) => {
            app.push_toast(
                texts::tui_toast_webdav_worker_unavailable(&err.to_string()),
                ToastKind::Warning,
            );
            None
        }
    };

    let update_system = match start_update_system() {
        Ok(system) => Some(system),
        Err(err) => {
            app.push_toast(
                texts::tui_toast_update_check_failed(&err.to_string()),
                ToastKind::Warning,
            );
            None
        }
    };

    loop {
        app.last_size = terminal.size()?;
        terminal.draw(|f| ui::render(f, &app, &data))?;

        // Handle async speedtest results (non-blocking).
        if let Some(speedtest) = speedtest.as_ref() {
            while let Ok(msg) = speedtest.result_rx.try_recv() {
                handle_speedtest_msg(&mut app, msg);
            }
        }

        // Handle async local environment check results (non-blocking).
        if let Some(local_env) = local_env.as_ref() {
            while let Ok(msg) = local_env.result_rx.try_recv() {
                handle_local_env_msg(&mut app, msg);
            }
        }

        // Handle async Skills results (non-blocking).
        if let Some(skills) = skills.as_ref() {
            while let Ok(msg) = skills.result_rx.try_recv() {
                if let Err(err) = handle_skills_msg(&mut app, &mut data, msg) {
                    app.push_toast(err.to_string(), ToastKind::Error);
                }
            }
        }

        // Handle async WebDAV results (non-blocking).
        if let Some(webdav) = webdav.as_ref() {
            while let Ok(msg) = webdav.result_rx.try_recv() {
                if let Err(err) = handle_webdav_msg(&mut app, &mut data, &mut webdav_loading, msg) {
                    app.push_toast(err.to_string(), ToastKind::Error);
                }
            }
        }

        // Handle async update results (non-blocking).
        if let Some(us) = update_system.as_ref() {
            while let Ok(msg) = us.result_rx.try_recv() {
                handle_update_msg(&mut app, &mut update_check, msg);
            }
        }

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout).map_err(|e| AppError::Message(e.to_string()))? {
            match event::read().map_err(|e| AppError::Message(e.to_string()))? {
                event::Event::Key(key) if key.kind == KeyEventKind::Press => {
                    let action = app.on_key(key, &data);
                    if let Err(err) = handle_action(
                        &mut terminal,
                        &mut app,
                        &mut data,
                        speedtest.as_ref().map(|s| &s.req_tx),
                        skills.as_ref().map(|s| &s.req_tx),
                        local_env.as_ref().map(|s| &s.req_tx),
                        webdav.as_ref().map(|s| &s.req_tx),
                        &mut webdav_loading,
                        update_system.as_ref().map(|s| &s.req_tx),
                        &mut update_check,
                        action,
                    ) {
                        if matches!(
                            &err,
                            AppError::Localized { key, .. } if *key == "tui_terminal_error"
                        ) {
                            return Err(err);
                        }
                        app.push_toast(err.to_string(), ToastKind::Error);
                    }
                }
                event::Event::Resize(_, _) => {}
                _ => {}
            }
        }

        if last_tick.elapsed() >= tick_rate {
            app.on_tick();
            last_tick = Instant::now();
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

fn handle_speedtest_msg(app: &mut App, msg: SpeedtestMsg) {
    match msg {
        SpeedtestMsg::Finished { url, result } => match result {
            Ok(rows) => {
                let mut lines = vec![texts::tui_speedtest_line_url(&url), String::new()];
                for row in rows {
                    let latency = row
                        .latency
                        .map(texts::tui_latency_ms)
                        .unwrap_or_else(|| texts::tui_na().to_string());
                    let status = row
                        .status
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| texts::tui_na().to_string());
                    let err = row.error.unwrap_or_default();

                    lines.push(texts::tui_speedtest_line_latency(&latency));
                    lines.push(texts::tui_speedtest_line_status(&status));
                    if !err.trim().is_empty() {
                        lines.push(texts::tui_speedtest_line_error(&err));
                    }
                }

                // Only force-open the result modal if the user hasn't closed it.
                match &app.overlay {
                    Overlay::SpeedtestRunning { url: running_url } if running_url == &url => {
                        app.overlay = Overlay::SpeedtestResult {
                            url,
                            lines,
                            scroll: 0,
                        };
                    }
                    _ => {
                        app.push_toast(texts::tui_toast_speedtest_finished(), ToastKind::Success);
                    }
                }
            }
            Err(err) => {
                app.push_toast(texts::tui_toast_speedtest_failed(&err), ToastKind::Error);
                if matches!(&app.overlay, Overlay::SpeedtestRunning { url: running_url } if running_url == &url)
                {
                    app.overlay = Overlay::None;
                }
            }
        },
    }
}

fn handle_local_env_msg(app: &mut App, msg: LocalEnvMsg) {
    match msg {
        LocalEnvMsg::Finished { result } => {
            app.local_env_results = result;
            app.local_env_loading = false;
        }
    }
}

fn handle_skills_msg(app: &mut App, data: &mut UiData, msg: SkillsMsg) -> Result<(), AppError> {
    match msg {
        SkillsMsg::DiscoverFinished { query, result } => match result {
            Ok(skills) => {
                app.overlay = Overlay::None;
                app.skills_discover_results = skills;
                app.skills_discover_idx = 0;
                app.skills_discover_query = query.clone();
                app.push_toast(
                    texts::tui_toast_skills_discover_finished(app.skills_discover_results.len()),
                    ToastKind::Success,
                );
            }
            Err(err) => {
                app.overlay = Overlay::None;
                app.push_toast(
                    texts::tui_toast_skills_discover_failed(&err),
                    ToastKind::Error,
                );
            }
        },
        SkillsMsg::InstallFinished { spec, result } => match result {
            Ok(installed) => {
                app.overlay = Overlay::None;
                // Refresh local snapshots.
                *data = UiData::load(&app.app_type)?;

                // Mark discover result row as installed (best-effort).
                for row in app.skills_discover_results.iter_mut() {
                    if row.directory.eq_ignore_ascii_case(&installed.directory) {
                        row.installed = true;
                    }
                }

                app.push_toast(
                    texts::tui_toast_skill_installed(&installed.directory),
                    ToastKind::Success,
                );
            }
            Err(err) => {
                app.overlay = Overlay::None;
                app.push_toast(
                    texts::tui_toast_skill_install_failed(&spec, &err),
                    ToastKind::Error,
                );
            }
        },
    }

    Ok(())
}

fn is_webdav_loading_overlay(app: &App) -> bool {
    matches!(
        &app.overlay,
        Overlay::Loading {
            kind: LoadingKind::WebDav,
            ..
        }
    )
}

fn handle_webdav_msg(
    app: &mut App,
    data: &mut UiData,
    webdav_loading: &mut RequestTracker,
    msg: WebDavMsg,
) -> Result<(), AppError> {
    match msg {
        WebDavMsg::Finished {
            request_id,
            req,
            result,
        } => match result {
            Ok(done) => {
                if webdav_loading.is_stale(request_id) {
                    return Ok(());
                }

                if webdav_loading.finish_if_active(request_id) && is_webdav_loading_overlay(app) {
                    app.overlay = Overlay::None;
                }

                match done {
                    WebDavDone::ConnectionChecked => {
                        update_webdav_last_error(None);
                        app.push_toast(texts::tui_toast_webdav_connection_ok(), ToastKind::Success);
                    }
                    WebDavDone::Uploaded { decision, message } => {
                        let msg = match decision {
                            SyncDecision::Upload => texts::tui_toast_webdav_upload_ok().to_string(),
                            _ => message,
                        };
                        app.push_toast(msg, ToastKind::Success);
                    }
                    WebDavDone::Downloaded { decision, message } => {
                        let msg = match decision {
                            SyncDecision::Download => {
                                texts::tui_toast_webdav_download_ok().to_string()
                            }
                            _ => message,
                        };
                        app.push_toast(msg, ToastKind::Success);
                    }
                    WebDavDone::JianguoyunConfigured => {
                        app.push_toast(
                            texts::tui_toast_webdav_jianguoyun_configured(),
                            ToastKind::Success,
                        );
                    }
                }
                *data = UiData::load(&app.app_type)?;
            }
            Err(err) => {
                if webdav_loading.is_stale(request_id) {
                    return Ok(());
                }

                if webdav_loading.finish_if_active(request_id) && is_webdav_loading_overlay(app) {
                    app.overlay = Overlay::None;
                }
                let error_detail = match &err {
                    WebDavErr::Generic(e)
                    | WebDavErr::QuickSetupSave(e)
                    | WebDavErr::QuickSetupCheck(e) => e.clone(),
                };
                update_webdav_last_error(Some(error_detail));
                let msg = match req {
                    WebDavReqKind::CheckConnection => {
                        let detail = match err {
                            WebDavErr::Generic(e)
                            | WebDavErr::QuickSetupSave(e)
                            | WebDavErr::QuickSetupCheck(e) => e,
                        };
                        texts::tui_toast_webdav_action_failed(
                            texts::tui_webdav_loading_title_check_connection(),
                            &detail,
                        )
                    }
                    WebDavReqKind::Upload => {
                        let detail = match err {
                            WebDavErr::Generic(e)
                            | WebDavErr::QuickSetupSave(e)
                            | WebDavErr::QuickSetupCheck(e) => e,
                        };
                        texts::tui_toast_webdav_action_failed(
                            texts::tui_webdav_loading_title_upload(),
                            &detail,
                        )
                    }
                    WebDavReqKind::Download => {
                        let detail = match err {
                            WebDavErr::Generic(e)
                            | WebDavErr::QuickSetupSave(e)
                            | WebDavErr::QuickSetupCheck(e) => e,
                        };
                        texts::tui_toast_webdav_action_failed(
                            texts::tui_webdav_loading_title_download(),
                            &detail,
                        )
                    }
                    WebDavReqKind::JianguoyunQuickSetup { .. } => match err {
                        WebDavErr::QuickSetupCheck(e) => {
                            texts::tui_toast_webdav_quick_setup_failed(&e)
                        }
                        WebDavErr::QuickSetupSave(e) | WebDavErr::Generic(e) => {
                            texts::tui_toast_webdav_action_failed(
                                texts::tui_webdav_loading_title_quick_setup(),
                                &e,
                            )
                        }
                    },
                };
                *data = UiData::load(&app.app_type)?;
                app.push_toast(msg, ToastKind::Error);
            }
        },
    }
    Ok(())
}

fn handle_action(
    _terminal: &mut TuiTerminal,
    app: &mut App,
    data: &mut UiData,
    speedtest_req_tx: Option<&mpsc::Sender<String>>,
    skills_req_tx: Option<&mpsc::Sender<SkillsReq>>,
    local_env_req_tx: Option<&mpsc::Sender<LocalEnvReq>>,
    webdav_req_tx: Option<&mpsc::Sender<WebDavReq>>,
    webdav_loading: &mut RequestTracker,
    update_req_tx: Option<&mpsc::Sender<UpdateReq>>,
    update_check: &mut RequestTracker,
    action: Action,
) -> Result<(), AppError> {
    match action {
        Action::None => Ok(()),
        Action::ReloadData => {
            *data = UiData::load(&app.app_type)?;
            Ok(())
        }
        Action::SetAppType(next) => {
            let next_data = UiData::load(&next)?;
            app.app_type = next;
            *data = next_data;
            Ok(())
        }
        Action::LocalEnvRefresh => {
            let Some(tx) = local_env_req_tx else {
                app.local_env_loading = false;
                app.push_toast(
                    texts::tui_toast_local_env_check_disabled(),
                    ToastKind::Warning,
                );
                return Ok(());
            };

            app.local_env_loading = true;
            if let Err(err) = tx.send(LocalEnvReq::Refresh) {
                app.local_env_loading = false;
                app.push_toast(
                    texts::tui_toast_local_env_check_request_failed(&err.to_string()),
                    ToastKind::Warning,
                );
            }
            Ok(())
        }
        Action::SwitchRoute(route) => {
            app.route = route;
            if matches!(app.route, crate::cli::tui::route::Route::SkillsUnmanaged) {
                app.skills_unmanaged_results = SkillService::scan_unmanaged()?;
                app.skills_unmanaged_selected.clear();
                app.skills_unmanaged_idx = 0;
            }
            Ok(())
        }
        Action::Quit => {
            app.should_quit = true;
            Ok(())
        }
        Action::SkillsToggle { directory, enabled } => {
            SkillService::toggle_app(&directory, &app.app_type, enabled)?;
            *data = UiData::load(&app.app_type)?;
            app.push_toast(
                texts::tui_toast_skill_toggled(&directory, enabled),
                ToastKind::Success,
            );
            Ok(())
        }
        Action::SkillsInstall { spec } => {
            let Some(tx) = skills_req_tx else {
                return Err(AppError::Message(
                    texts::tui_error_skills_worker_unavailable().to_string(),
                ));
            };
            app.overlay = Overlay::Loading {
                kind: LoadingKind::Generic,
                title: texts::tui_skills_install_title().to_string(),
                message: texts::tui_loading().to_string(),
            };
            tx.send(SkillsReq::Install {
                spec: spec.clone(),
                app: app.app_type.clone(),
            })
            .map_err(|e| AppError::Message(e.to_string()))?;
            Ok(())
        }
        Action::SkillsUninstall { directory } => {
            SkillService::uninstall(&directory)?;
            *data = UiData::load(&app.app_type)?;
            app.push_toast(
                texts::tui_toast_skill_uninstalled(&directory),
                ToastKind::Success,
            );
            if matches!(
                &app.route,
                crate::cli::tui::route::Route::SkillDetail { directory: current }
                    if current.eq_ignore_ascii_case(&directory)
            ) {
                app.route = crate::cli::tui::route::Route::Skills;
            }
            Ok(())
        }
        Action::SkillsSync { app: scope } => {
            let scope_ref = scope.as_ref();
            SkillService::sync_all_enabled(scope_ref)?;
            *data = UiData::load(&app.app_type)?;
            app.push_toast(texts::tui_toast_skills_synced(), ToastKind::Success);
            Ok(())
        }
        Action::SkillsSetSyncMethod { method } => {
            SkillService::set_sync_method(method)?;
            *data = UiData::load(&app.app_type)?;
            app.push_toast(
                texts::tui_toast_skills_sync_method_set(texts::tui_skills_sync_method_name(method)),
                ToastKind::Success,
            );
            Ok(())
        }
        Action::SkillsDiscover { query } => {
            let Some(tx) = skills_req_tx else {
                return Err(AppError::Message(
                    texts::tui_error_skills_worker_unavailable().to_string(),
                ));
            };
            app.overlay = Overlay::Loading {
                kind: LoadingKind::Generic,
                title: texts::tui_skills_discover_title().to_string(),
                message: texts::tui_loading().to_string(),
            };
            tx.send(SkillsReq::Discover { query })
                .map_err(|e| AppError::Message(e.to_string()))?;
            Ok(())
        }
        Action::SkillsRepoAdd { spec } => {
            let repo = parse_repo_spec(&spec)?;
            SkillService::upsert_repo(repo)?;
            *data = UiData::load(&app.app_type)?;
            app.push_toast(texts::tui_toast_repo_added(), ToastKind::Success);
            Ok(())
        }
        Action::SkillsRepoRemove { owner, name } => {
            SkillService::remove_repo(&owner, &name)?;
            *data = UiData::load(&app.app_type)?;
            app.push_toast(texts::tui_toast_repo_removed(), ToastKind::Success);
            Ok(())
        }
        Action::SkillsRepoToggleEnabled {
            owner,
            name,
            enabled,
        } => {
            let mut index = SkillService::load_index()?;
            if let Some(repo) = index
                .repos
                .iter_mut()
                .find(|r| r.owner == owner && r.name == name)
            {
                repo.enabled = enabled;
                SkillService::save_index(&index)?;
            }
            *data = UiData::load(&app.app_type)?;
            app.push_toast(texts::tui_toast_repo_toggled(enabled), ToastKind::Success);
            Ok(())
        }
        Action::SkillsScanUnmanaged => {
            app.skills_unmanaged_results = SkillService::scan_unmanaged()?;
            app.skills_unmanaged_selected.clear();
            app.skills_unmanaged_idx = 0;
            app.push_toast(
                texts::tui_toast_unmanaged_scanned(app.skills_unmanaged_results.len()),
                ToastKind::Success,
            );
            Ok(())
        }
        Action::SkillsImportFromApps { directories } => {
            let imported = SkillService::import_from_apps(directories)?;
            *data = UiData::load(&app.app_type)?;
            // Refresh unmanaged list after import.
            app.skills_unmanaged_results = SkillService::scan_unmanaged()?;
            app.skills_unmanaged_selected.clear();
            app.skills_unmanaged_idx = 0;
            app.push_toast(
                texts::tui_toast_unmanaged_imported(imported.len()),
                ToastKind::Success,
            );
            Ok(())
        }
        Action::EditorDiscard => {
            app.editor = None;
            Ok(())
        }
        Action::EditorSubmit { submit, content } => match submit {
            EditorSubmit::PromptEdit { id } => {
                let state = load_state()?;
                let prompts = PromptService::get_prompts(&state, app.app_type.clone())?;
                let Some(mut prompt) = prompts.get(&id).cloned() else {
                    app.push_toast(texts::tui_toast_prompt_not_found(&id), ToastKind::Error);
                    return Ok(());
                };

                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                prompt.content = content;
                prompt.updated_at = Some(timestamp);

                if let Err(err) =
                    PromptService::upsert_prompt(&state, app.app_type.clone(), &id, prompt)
                {
                    app.push_toast(err.to_string(), ToastKind::Error);
                    return Ok(());
                }

                app.editor = None;
                app.push_toast(texts::tui_toast_prompt_edit_finished(), ToastKind::Success);
                *data = UiData::load(&app.app_type)?;
                Ok(())
            }
            EditorSubmit::ProviderFormApplyJson => {
                let settings_value: serde_json::Value = match serde_json::from_str(&content) {
                    Ok(value) => value,
                    Err(e) => {
                        app.push_toast(
                            texts::tui_toast_invalid_json(&e.to_string()),
                            ToastKind::Error,
                        );
                        return Ok(());
                    }
                };

                if !settings_value.is_object() {
                    app.push_toast(texts::tui_toast_json_must_be_object(), ToastKind::Error);
                    return Ok(());
                }

                let provider_value = match app.form.as_ref() {
                    Some(FormState::ProviderAdd(form)) => {
                        let mut provider_value = form.to_provider_json_value();
                        if let Some(obj) = provider_value.as_object_mut() {
                            obj.insert("settingsConfig".to_string(), settings_value);
                        }
                        Some(provider_value)
                    }
                    _ => None,
                };

                if let Some(provider_value) = provider_value {
                    let apply_result = match app.form.as_mut() {
                        Some(FormState::ProviderAdd(form)) => {
                            form.apply_provider_json_value_to_fields(provider_value)
                        }
                        _ => Ok(()),
                    };

                    if let Err(err) = apply_result {
                        app.push_toast(err, ToastKind::Error);
                        return Ok(());
                    }
                }
                app.editor = None;
                Ok(())
            }
            EditorSubmit::ProviderFormApplyCodexAuth => {
                let auth_value: serde_json::Value = match serde_json::from_str(&content) {
                    Ok(value) => value,
                    Err(e) => {
                        app.push_toast(
                            texts::tui_toast_invalid_json(&e.to_string()),
                            ToastKind::Error,
                        );
                        return Ok(());
                    }
                };

                if !auth_value.is_object() {
                    app.push_toast(texts::tui_toast_json_must_be_object(), ToastKind::Error);
                    return Ok(());
                }

                let provider_value = match app.form.as_ref() {
                    Some(FormState::ProviderAdd(form)) => {
                        let mut provider_value = form.to_provider_json_value();
                        if let Some(settings_value) = provider_value
                            .as_object_mut()
                            .and_then(|obj| obj.get_mut("settingsConfig"))
                        {
                            if !settings_value.is_object() {
                                *settings_value = json!({});
                            }
                            if let Some(settings_obj) = settings_value.as_object_mut() {
                                settings_obj.insert("auth".to_string(), auth_value);
                            }
                        }
                        Some(provider_value)
                    }
                    _ => None,
                };

                if let Some(provider_value) = provider_value {
                    let apply_result = match app.form.as_mut() {
                        Some(FormState::ProviderAdd(form)) => {
                            form.apply_provider_json_value_to_fields(provider_value)
                        }
                        _ => Ok(()),
                    };

                    if let Err(err) = apply_result {
                        app.push_toast(err, ToastKind::Error);
                        return Ok(());
                    }
                }

                app.editor = None;
                Ok(())
            }
            EditorSubmit::ProviderFormApplyCodexConfigToml => {
                use toml_edit::DocumentMut;

                let config_text = if content.trim().is_empty() {
                    String::new()
                } else {
                    let doc: DocumentMut = match content.parse() {
                        Ok(doc) => doc,
                        Err(e) => {
                            app.push_toast(
                                texts::common_config_snippet_invalid_toml(&e.to_string()),
                                ToastKind::Error,
                            );
                            return Ok(());
                        }
                    };
                    doc.to_string()
                };

                let provider_value = match app.form.as_ref() {
                    Some(FormState::ProviderAdd(form)) => {
                        let mut provider_value = form.to_provider_json_value();
                        if let Some(settings_value) = provider_value
                            .as_object_mut()
                            .and_then(|obj| obj.get_mut("settingsConfig"))
                        {
                            if !settings_value.is_object() {
                                *settings_value = json!({});
                            }
                            if let Some(settings_obj) = settings_value.as_object_mut() {
                                settings_obj
                                    .insert("config".to_string(), Value::String(config_text));
                            }
                        }
                        Some(provider_value)
                    }
                    _ => None,
                };

                if let Some(provider_value) = provider_value {
                    let apply_result = match app.form.as_mut() {
                        Some(FormState::ProviderAdd(form)) => {
                            form.apply_provider_json_value_to_fields(provider_value)
                        }
                        _ => Ok(()),
                    };

                    if let Err(err) = apply_result {
                        app.push_toast(err, ToastKind::Error);
                        return Ok(());
                    }
                }

                app.editor = None;
                Ok(())
            }
            EditorSubmit::ProviderAdd => {
                let provider: Provider = match serde_json::from_str(&content) {
                    Ok(p) => p,
                    Err(e) => {
                        app.push_toast(
                            texts::tui_toast_invalid_json(&e.to_string()),
                            ToastKind::Error,
                        );
                        return Ok(());
                    }
                };

                if provider.id.trim().is_empty() || provider.name.trim().is_empty() {
                    app.push_toast(
                        texts::tui_toast_provider_add_missing_fields(),
                        ToastKind::Warning,
                    );
                    return Ok(());
                }

                let state = load_state()?;
                match ProviderService::add(&state, app.app_type.clone(), provider) {
                    Ok(true) => {
                        app.editor = None;
                        app.form = None;
                        app.push_toast(
                            texts::tui_toast_provider_add_finished(),
                            ToastKind::Success,
                        );
                        *data = UiData::load(&app.app_type)?;
                    }
                    Ok(false) => {
                        app.push_toast(texts::tui_toast_provider_add_failed(), ToastKind::Error);
                    }
                    Err(err) => {
                        app.push_toast(err.to_string(), ToastKind::Error);
                    }
                }

                Ok(())
            }
            EditorSubmit::ProviderEdit { id } => {
                let mut provider: Provider = match serde_json::from_str(&content) {
                    Ok(p) => p,
                    Err(e) => {
                        app.push_toast(
                            texts::tui_toast_invalid_json(&e.to_string()),
                            ToastKind::Error,
                        );
                        return Ok(());
                    }
                };
                provider.id = id.clone();

                if provider.name.trim().is_empty() {
                    app.push_toast(texts::tui_toast_provider_missing_name(), ToastKind::Warning);
                    return Ok(());
                }

                let state = load_state()?;
                if let Err(err) = ProviderService::update(&state, app.app_type.clone(), provider) {
                    app.push_toast(err.to_string(), ToastKind::Error);
                    return Ok(());
                }

                app.editor = None;
                app.form = None;
                app.push_toast(
                    texts::tui_toast_provider_edit_finished(),
                    ToastKind::Success,
                );
                *data = UiData::load(&app.app_type)?;
                Ok(())
            }
            EditorSubmit::McpAdd => {
                let server: crate::app_config::McpServer = match serde_json::from_str(&content) {
                    Ok(s) => s,
                    Err(e) => {
                        app.push_toast(
                            texts::tui_toast_invalid_json(&e.to_string()),
                            ToastKind::Error,
                        );
                        return Ok(());
                    }
                };

                if server.id.trim().is_empty() || server.name.trim().is_empty() {
                    app.push_toast(texts::tui_toast_mcp_missing_fields(), ToastKind::Warning);
                    return Ok(());
                }

                let state = load_state()?;
                if let Err(err) = McpService::upsert_server(&state, server) {
                    app.push_toast(err.to_string(), ToastKind::Error);
                    return Ok(());
                }

                app.editor = None;
                app.form = None;
                app.push_toast(texts::tui_toast_mcp_upserted(), ToastKind::Success);
                *data = UiData::load(&app.app_type)?;
                Ok(())
            }
            EditorSubmit::McpEdit { id } => {
                let mut server: crate::app_config::McpServer = match serde_json::from_str(&content)
                {
                    Ok(s) => s,
                    Err(e) => {
                        app.push_toast(
                            texts::tui_toast_invalid_json(&e.to_string()),
                            ToastKind::Error,
                        );
                        return Ok(());
                    }
                };
                server.id = id.clone();

                if server.name.trim().is_empty() {
                    app.push_toast(texts::tui_toast_mcp_missing_fields(), ToastKind::Warning);
                    return Ok(());
                }

                let state = load_state()?;
                if let Err(err) = McpService::upsert_server(&state, server) {
                    app.push_toast(err.to_string(), ToastKind::Error);
                    return Ok(());
                }

                app.editor = None;
                app.form = None;
                app.push_toast(texts::tui_toast_mcp_upserted(), ToastKind::Success);
                *data = UiData::load(&app.app_type)?;
                Ok(())
            }
            EditorSubmit::ConfigCommonSnippet { app_type } => {
                let edited = content.trim().to_string();
                let (next_snippet, toast) = if edited.is_empty() {
                    (None, texts::common_config_snippet_cleared())
                } else if matches!(app_type, AppType::Codex) {
                    let doc: toml_edit::DocumentMut = match edited.parse() {
                        Ok(v) => v,
                        Err(e) => {
                            app.push_toast(
                                texts::common_config_snippet_invalid_toml(&e.to_string()),
                                ToastKind::Error,
                            );
                            return Ok(());
                        }
                    };
                    let canonical = doc.to_string().trim().to_string();
                    (Some(canonical), texts::common_config_snippet_saved())
                } else {
                    let value: Value = match serde_json::from_str(&edited) {
                        Ok(v) => v,
                        Err(e) => {
                            app.push_toast(
                                texts::common_config_snippet_invalid_json(&e.to_string()),
                                ToastKind::Error,
                            );
                            return Ok(());
                        }
                    };

                    if !value.is_object() {
                        app.push_toast(texts::common_config_snippet_not_object(), ToastKind::Error);
                        return Ok(());
                    }

                    let pretty = match serde_json::to_string_pretty(&value) {
                        Ok(v) => v,
                        Err(e) => {
                            app.push_toast(
                                texts::failed_to_serialize_json(&e.to_string()),
                                ToastKind::Error,
                            );
                            return Ok(());
                        }
                    };

                    (Some(pretty), texts::common_config_snippet_saved())
                };

                let state = load_state()?;
                {
                    let mut cfg = match state.config.write().map_err(AppError::from) {
                        Ok(cfg) => cfg,
                        Err(err) => {
                            app.push_toast(err.to_string(), ToastKind::Error);
                            return Ok(());
                        }
                    };
                    cfg.common_config_snippets
                        .set(&app_type, next_snippet.clone());
                }
                if let Err(err) = state.save() {
                    app.push_toast(err.to_string(), ToastKind::Error);
                    return Ok(());
                }

                app.editor = None;
                app.push_toast(toast, ToastKind::Success);
                *data = UiData::load(&app.app_type)?;

                // Bring the user back to the snippet preview overlay.
                let snippet = next_snippet.unwrap_or_else(|| {
                    texts::tui_default_common_snippet_for_app(app_type.as_str()).to_string()
                });
                app.overlay = Overlay::CommonSnippetView {
                    app_type: app_type.clone(),
                    view: TextViewState {
                        title: texts::tui_common_snippet_title(app_type.as_str()),
                        lines: snippet.lines().map(|s| s.to_string()).collect(),
                        scroll: 0,
                    },
                };
                Ok(())
            }
            EditorSubmit::ConfigWebDavSettings => {
                let edited = content.trim();
                if edited.is_empty() {
                    set_webdav_sync_settings(None)?;
                    app.editor = None;
                    app.push_toast(
                        texts::tui_toast_webdav_settings_cleared(),
                        ToastKind::Success,
                    );
                    *data = UiData::load(&app.app_type)?;
                    return Ok(());
                }

                let cfg: WebDavSyncSettings = serde_json::from_str(edited).map_err(|e| {
                    AppError::Message(texts::tui_toast_invalid_json(&e.to_string()))
                })?;
                set_webdav_sync_settings(Some(cfg))?;

                app.editor = None;
                app.push_toast(texts::tui_toast_webdav_settings_saved(), ToastKind::Success);
                *data = UiData::load(&app.app_type)?;
                Ok(())
            }
        },

        Action::ProviderSwitch { id } => {
            let state = load_state()?;
            ProviderService::switch(&state, app.app_type.clone(), &id)?;
            if !crate::sync_policy::should_sync_live(&app.app_type) {
                let mut message =
                    texts::tui_toast_live_sync_skipped_uninitialized(app.app_type.as_str());
                message.push(' ');
                message.push_str(texts::restart_note());
                app.push_toast(message, ToastKind::Warning);
            } else {
                app.push_toast(texts::restart_note(), ToastKind::Success);
            }
            *data = UiData::load(&app.app_type)?;
            Ok(())
        }
        Action::ProviderDelete { id } => {
            let state = load_state()?;
            ProviderService::delete(&state, app.app_type.clone(), &id)?;
            app.push_toast(texts::tui_toast_provider_deleted(), ToastKind::Success);
            *data = UiData::load(&app.app_type)?;
            Ok(())
        }
        // Provider editing is handled via the in-app editor (EditorSubmit).
        Action::ProviderSpeedtest { url } => {
            let Some(tx) = speedtest_req_tx else {
                if matches!(&app.overlay, Overlay::SpeedtestRunning { url: running_url } if running_url == &url)
                {
                    app.overlay = Overlay::None;
                }
                app.push_toast(texts::tui_toast_speedtest_disabled(), ToastKind::Warning);
                return Ok(());
            };

            if let Err(err) = tx.send(url.clone()) {
                if matches!(&app.overlay, Overlay::SpeedtestRunning { url: running_url } if running_url == &url)
                {
                    app.overlay = Overlay::None;
                }
                app.push_toast(
                    texts::tui_toast_speedtest_request_failed(&err.to_string()),
                    ToastKind::Error,
                );
            }
            Ok(())
        }

        Action::McpToggle { id, enabled } => {
            let state = load_state()?;
            McpService::toggle_app(&state, &id, app.app_type.clone(), enabled)?;
            if !crate::sync_policy::should_sync_live(&app.app_type) {
                let mut message = texts::tui_toast_mcp_updated().to_string();
                message.push(' ');
                message.push_str(&texts::tui_toast_live_sync_skipped_uninitialized(
                    app.app_type.as_str(),
                ));
                app.push_toast(message, ToastKind::Warning);
            } else {
                app.push_toast(texts::tui_toast_mcp_updated(), ToastKind::Success);
            }
            *data = UiData::load(&app.app_type)?;
            Ok(())
        }
        Action::McpSetApps { id, apps } => {
            let Some(before) = data
                .mcp
                .rows
                .iter()
                .find(|row| row.id == id)
                .map(|row| row.server.apps.clone())
            else {
                app.push_toast(texts::tui_toast_mcp_server_not_found(), ToastKind::Warning);
                return Ok(());
            };

            let state = load_state()?;
            let mut skipped: Vec<&str> = Vec::new();
            let mut changed = false;

            for app_type in [AppType::Claude, AppType::Codex, AppType::Gemini] {
                let next_enabled = apps.is_enabled_for(&app_type);
                if before.is_enabled_for(&app_type) == next_enabled {
                    continue;
                }

                changed = true;
                McpService::toggle_app(&state, &id, app_type.clone(), next_enabled)?;
                if !crate::sync_policy::should_sync_live(&app_type) {
                    skipped.push(app_type.as_str());
                }
            }

            if !changed {
                // Shouldn't happen because the picker avoids emitting an action when unchanged.
                app.push_toast(texts::tui_toast_mcp_updated(), ToastKind::Success);
            } else if skipped.is_empty() {
                app.push_toast(texts::tui_toast_mcp_updated(), ToastKind::Success);
            } else {
                app.push_toast(
                    texts::tui_toast_mcp_updated_live_sync_skipped(&skipped),
                    ToastKind::Warning,
                );
            }

            *data = UiData::load(&app.app_type)?;
            Ok(())
        }
        Action::McpDelete { id } => {
            let state = load_state()?;
            let deleted = McpService::delete_server(&state, &id)?;
            if deleted {
                app.push_toast(texts::tui_toast_mcp_server_deleted(), ToastKind::Success);
            } else {
                app.push_toast(texts::tui_toast_mcp_server_not_found(), ToastKind::Warning);
            }
            *data = UiData::load(&app.app_type)?;
            Ok(())
        }
        Action::McpImport => {
            let state = load_state()?;
            let count = match app.app_type {
                AppType::Claude => McpService::import_from_claude(&state)?,
                AppType::Codex => McpService::import_from_codex(&state)?,
                AppType::Gemini => McpService::import_from_gemini(&state)?,
            };
            app.push_toast(texts::tui_toast_mcp_imported(count), ToastKind::Success);
            *data = UiData::load(&app.app_type)?;
            Ok(())
        }
        Action::McpValidate { command } => {
            let Some(bin) = command_lookup_name(&command) else {
                app.push_toast(texts::tui_toast_command_empty(), ToastKind::Warning);
                return Ok(());
            };

            if which::which(bin).is_ok() {
                app.push_toast(
                    texts::tui_toast_command_available_in_path(bin),
                    ToastKind::Success,
                );
            } else {
                app.push_toast(
                    texts::tui_toast_command_not_found_in_path(bin),
                    ToastKind::Warning,
                );
            }
            Ok(())
        }

        Action::PromptActivate { id } => {
            let state = load_state()?;
            PromptService::enable_prompt(&state, app.app_type.clone(), &id)?;
            app.push_toast(texts::tui_toast_prompt_activated(), ToastKind::Success);
            *data = UiData::load(&app.app_type)?;
            Ok(())
        }
        Action::PromptDeactivate { id } => {
            let state = load_state()?;
            PromptService::disable_prompt(&state, app.app_type.clone(), &id)?;
            app.push_toast(texts::tui_toast_prompt_deactivated(), ToastKind::Success);
            *data = UiData::load(&app.app_type)?;
            Ok(())
        }
        Action::PromptDelete { id } => {
            let state = load_state()?;
            PromptService::delete_prompt(&state, app.app_type.clone(), &id)?;
            app.push_toast(texts::tui_toast_prompt_deleted(), ToastKind::Success);
            *data = UiData::load(&app.app_type)?;
            Ok(())
        }

        Action::ConfigExport { path } => {
            let target = PathBuf::from(path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
            }
            ConfigService::export_config_to_path(&target)?;
            app.push_toast(
                texts::tui_toast_exported_to(&target.display().to_string()),
                ToastKind::Success,
            );
            Ok(())
        }
        Action::ConfigShowFull => {
            let state = load_state()?;
            let config = state.config.read().map_err(AppError::from)?;
            let content = serde_json::to_string_pretty(&*config)
                .map_err(|e| AppError::Message(texts::failed_to_serialize_json(&e.to_string())))?;
            let title = texts::config_show_full()
                .trim_start_matches("👁️")
                .trim()
                .to_string();
            app.overlay = Overlay::TextView(TextViewState {
                title,
                lines: content.lines().map(|s| s.to_string()).collect(),
                scroll: 0,
            });
            Ok(())
        }
        Action::ConfigImport { path } => {
            let source = PathBuf::from(path);
            if !source.exists() {
                return Err(AppError::Message(texts::tui_error_import_file_not_found(
                    &source.display().to_string(),
                )));
            }
            let state = load_state()?;
            let backup_id = ConfigService::import_config_from_path(&source, &state)?;
            if backup_id.is_empty() {
                app.push_toast(texts::tui_toast_imported_config(), ToastKind::Success);
            } else {
                app.push_toast(
                    texts::tui_toast_imported_with_backup(&backup_id),
                    ToastKind::Success,
                );
            }
            *data = UiData::load(&app.app_type)?;
            Ok(())
        }
        Action::ConfigBackup { name } => {
            let db_path = crate::config::get_app_config_dir().join("cc-switch.db");
            let id = ConfigService::create_backup(&db_path, name)?;
            if id.is_empty() {
                app.push_toast(texts::tui_toast_no_config_file_to_backup(), ToastKind::Info);
            } else {
                app.push_toast(texts::tui_toast_backup_created(&id), ToastKind::Success);
            }
            *data = UiData::load(&app.app_type)?;
            Ok(())
        }
        Action::ConfigRestoreBackup { id } => {
            let state = load_state()?;
            let pre_backup = ConfigService::restore_from_backup_id(&id, &state)?;
            if pre_backup.is_empty() {
                app.push_toast(texts::tui_toast_restored_from_backup(), ToastKind::Success);
            } else {
                app.push_toast(
                    texts::tui_toast_restored_with_pre_backup(&pre_backup),
                    ToastKind::Success,
                );
            }
            *data = UiData::load(&app.app_type)?;
            Ok(())
        }
        Action::ConfigValidate => {
            let config_dir = crate::config::get_app_config_dir();
            let db_path = config_dir.join("cc-switch.db");
            if !db_path.exists() {
                app.push_toast(
                    texts::tui_toast_config_file_does_not_exist(),
                    ToastKind::Warning,
                );
                return Ok(());
            }

            let db = crate::Database::init()?;
            let claude_count = db.get_all_providers("claude")?.len();
            let codex_count = db.get_all_providers("codex")?.len();
            let gemini_count = db.get_all_providers("gemini")?.len();
            let mcp_count = db.get_all_mcp_servers()?.len();

            let lines = vec![
                texts::tui_config_validation_ok().to_string(),
                String::new(),
                texts::tui_config_validation_provider_count(AppType::Claude.as_str(), claude_count),
                texts::tui_config_validation_provider_count(AppType::Codex.as_str(), codex_count),
                texts::tui_config_validation_provider_count(AppType::Gemini.as_str(), gemini_count),
                texts::tui_config_validation_mcp_servers(mcp_count),
            ];
            app.overlay = Overlay::TextView(TextViewState {
                title: texts::tui_config_validation_title().to_string(),
                lines,
                scroll: 0,
            });
            app.push_toast(texts::tui_toast_validation_passed(), ToastKind::Success);
            Ok(())
        }
        Action::ConfigCommonSnippetClear { app_type } => {
            let state = load_state()?;
            {
                let mut cfg = state.config.write().map_err(AppError::from)?;
                cfg.common_config_snippets.set(&app_type, None);
            }
            state.save()?;

            app.push_toast(texts::common_config_snippet_cleared(), ToastKind::Success);
            *data = UiData::load(&app.app_type)?;
            refresh_common_snippet_overlay(app, data);
            Ok(())
        }
        Action::ConfigCommonSnippetApply { app_type } => {
            let state = load_state()?;
            let current_id = ProviderService::current(&state, app_type.clone())?;
            if current_id.trim().is_empty() {
                app.push_toast(
                    texts::common_config_snippet_no_current_provider(),
                    ToastKind::Info,
                );
                return Ok(());
            }
            ProviderService::switch(&state, app_type.clone(), &current_id)?;
            app.push_toast(texts::common_config_snippet_applied(), ToastKind::Success);
            *data = UiData::load(&app.app_type)?;
            Ok(())
        }
        Action::ConfigWebDavCheckConnection => {
            let Some(tx) = webdav_req_tx else {
                app.push_toast(
                    texts::tui_toast_webdav_worker_disabled(),
                    ToastKind::Warning,
                );
                return Ok(());
            };
            let request_id = webdav_loading.start();
            app.overlay = Overlay::Loading {
                kind: LoadingKind::WebDav,
                title: texts::tui_webdav_loading_title_check_connection().to_string(),
                message: texts::tui_webdav_loading_message().to_string(),
            };
            if let Err(err) = tx.send(WebDavReq {
                request_id,
                kind: WebDavReqKind::CheckConnection,
            }) {
                webdav_loading.cancel();
                app.overlay = Overlay::None;
                app.push_toast(
                    texts::tui_toast_webdav_request_failed(&err.to_string()),
                    ToastKind::Error,
                );
            }
            Ok(())
        }
        Action::ConfigWebDavUpload => {
            let Some(tx) = webdav_req_tx else {
                app.push_toast(
                    texts::tui_toast_webdav_worker_disabled(),
                    ToastKind::Warning,
                );
                return Ok(());
            };
            let request_id = webdav_loading.start();
            app.overlay = Overlay::Loading {
                kind: LoadingKind::WebDav,
                title: texts::tui_webdav_loading_title_upload().to_string(),
                message: texts::tui_webdav_loading_message().to_string(),
            };
            if let Err(err) = tx.send(WebDavReq {
                request_id,
                kind: WebDavReqKind::Upload,
            }) {
                webdav_loading.cancel();
                app.overlay = Overlay::None;
                app.push_toast(
                    texts::tui_toast_webdav_request_failed(&err.to_string()),
                    ToastKind::Error,
                );
            }
            Ok(())
        }
        Action::ConfigWebDavDownload => {
            let Some(tx) = webdav_req_tx else {
                app.push_toast(
                    texts::tui_toast_webdav_worker_disabled(),
                    ToastKind::Warning,
                );
                return Ok(());
            };
            let request_id = webdav_loading.start();
            app.overlay = Overlay::Loading {
                kind: LoadingKind::WebDav,
                title: texts::tui_webdav_loading_title_download().to_string(),
                message: texts::tui_webdav_loading_message().to_string(),
            };
            if let Err(err) = tx.send(WebDavReq {
                request_id,
                kind: WebDavReqKind::Download,
            }) {
                webdav_loading.cancel();
                app.overlay = Overlay::None;
                app.push_toast(
                    texts::tui_toast_webdav_request_failed(&err.to_string()),
                    ToastKind::Error,
                );
            }
            Ok(())
        }
        Action::ConfigWebDavReset => {
            set_webdav_sync_settings(None)?;
            app.push_toast(
                texts::tui_toast_webdav_settings_cleared(),
                ToastKind::Success,
            );
            *data = UiData::load(&app.app_type)?;
            Ok(())
        }
        Action::ConfigWebDavJianguoyunQuickSetup { username, password } => {
            let Some(tx) = webdav_req_tx else {
                app.push_toast(
                    texts::tui_toast_webdav_worker_disabled(),
                    ToastKind::Warning,
                );
                return Ok(());
            };
            let request_id = webdav_loading.start();
            app.overlay = Overlay::Loading {
                kind: LoadingKind::WebDav,
                title: texts::tui_webdav_loading_title_quick_setup().to_string(),
                message: texts::tui_webdav_loading_message().to_string(),
            };
            if let Err(err) = tx.send(WebDavReq {
                request_id,
                kind: WebDavReqKind::JianguoyunQuickSetup { username, password },
            }) {
                webdav_loading.cancel();
                app.overlay = Overlay::None;
                app.push_toast(
                    texts::tui_toast_webdav_request_failed(&err.to_string()),
                    ToastKind::Error,
                );
            }
            Ok(())
        }
        Action::ConfigReset => {
            let config_dir = crate::config::get_app_config_dir();
            let db_path = config_dir.join("cc-switch.db");
            let backup_id = ConfigService::create_backup(&db_path, None)?;

            if db_path.exists() {
                std::fs::remove_file(&db_path).map_err(|e| AppError::io(&db_path, e))?;
            }
            let _ = crate::Database::init()?;
            if backup_id.is_empty() {
                app.push_toast(
                    texts::tui_toast_config_reset_to_defaults(),
                    ToastKind::Success,
                );
            } else {
                app.push_toast(
                    texts::tui_toast_config_reset_with_backup(&backup_id),
                    ToastKind::Success,
                );
            }
            *data = UiData::load(&app.app_type)?;
            Ok(())
        }

        Action::SetSkipClaudeOnboarding { enabled } => {
            crate::settings::set_skip_claude_onboarding(enabled)?;
            app.push_toast(
                texts::tui_toast_skip_claude_onboarding_toggled(enabled),
                ToastKind::Success,
            );
            Ok(())
        }

        Action::SetLanguage(lang) => {
            set_language(lang)?;
            app.push_toast(texts::language_changed(), ToastKind::Success);
            Ok(())
        }

        Action::CheckUpdate => {
            if matches!(app.overlay, Overlay::UpdateDownloading { .. }) {
                return Ok(());
            }
            let Some(tx) = update_req_tx else {
                app.push_toast(
                    texts::tui_toast_update_check_failed(texts::tui_update_err_worker_unavailable()),
                    ToastKind::Warning,
                );
                return Ok(());
            };
            let request_id = update_check.start();
            app.overlay = Overlay::Loading {
                kind: LoadingKind::UpdateCheck,
                title: texts::tui_update_checking_title().to_string(),
                message: texts::tui_loading().to_string(),
            };
            if let Err(err) = tx.send(UpdateReq::Check { request_id }) {
                update_check.cancel();
                app.overlay = Overlay::None;
                app.push_toast(
                    texts::tui_toast_update_check_failed(&err.to_string()),
                    ToastKind::Error,
                );
            }
            Ok(())
        }
        Action::ConfirmUpdate => {
            let Some(tx) = update_req_tx else {
                return Ok(());
            };
            app.overlay = Overlay::UpdateDownloading {
                downloaded: 0,
                total: None,
            };
            if let Err(err) = tx.send(UpdateReq::Download) {
                app.overlay = Overlay::None;
                app.push_toast(
                    texts::tui_toast_update_bg_failed(&err.to_string()),
                    ToastKind::Error,
                );
            }
            Ok(())
        }
        Action::CancelUpdate => {
            app.overlay = Overlay::None;
            Ok(())
        }
        Action::CancelUpdateCheck => {
            update_check.cancel();
            Ok(())
        }
    }
}

fn apply_webdav_jianguoyun_quick_setup<FSave, FCheck>(
    username: &str,
    password: &str,
    save_settings: FSave,
    check_connection: FCheck,
) -> Result<(), AppError>
where
    FSave: FnOnce(WebDavSyncSettings) -> Result<(), AppError>,
    FCheck: FnOnce() -> Result<(), AppError>,
{
    let cfg = webdav_jianguoyun_preset(username, password);
    save_settings(cfg)?;
    check_connection()?;
    Ok(())
}

fn update_webdav_last_error_with<FGet, FSet>(last_error: Option<String>, get: FGet, set: FSet)
where
    FGet: FnOnce() -> Option<WebDavSyncSettings>,
    FSet: FnOnce(WebDavSyncSettings) -> Result<(), AppError>,
{
    let Some(mut cfg) = get() else {
        return;
    };
    cfg.status.last_error = last_error;
    let _ = set(cfg);
}

fn update_webdav_last_error(last_error: Option<String>) {
    update_webdav_last_error_with(last_error, get_webdav_sync_settings, |cfg| {
        set_webdav_sync_settings(Some(cfg))
    });
}

fn refresh_common_snippet_overlay(app: &mut App, data: &UiData) {
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

fn handle_update_msg(app: &mut App, update_check: &mut RequestTracker, msg: UpdateMsg) {
    match msg {
        UpdateMsg::CheckFinished { request_id, result } => {
            if !update_check.finish_if_active(request_id) {
                return;
            }

            match result {
                Ok(info) => {
                    if info.is_already_latest {
                        app.overlay = Overlay::None;
                        app.push_toast(
                            texts::tui_toast_already_latest(&info.current_version),
                            ToastKind::Success,
                        );
                    } else if info.is_downgrade {
                        app.overlay = Overlay::None;
                        app.push_toast(
                            texts::tui_toast_update_downgrade(
                                &info.current_version,
                                &info.target_tag,
                            ),
                            ToastKind::Info,
                        );
                    } else {
                        app.overlay = Overlay::UpdateAvailable {
                            current: info.current_version,
                            latest: info.target_tag,
                            selected: 0,
                        };
                    }
                }
                Err(e) => {
                    app.overlay = Overlay::None;
                    app.push_toast(texts::tui_toast_update_check_failed(&e), ToastKind::Error);
                }
            }
        }
        UpdateMsg::DownloadProgress { downloaded, total } => {
            if let Overlay::UpdateDownloading {
                downloaded: ref mut dl,
                total: ref mut t,
            } = app.overlay
            {
                *dl = downloaded;
                *t = total;
            }
        }
        UpdateMsg::DownloadFinished(result) => match result {
            Ok(tag) => {
                app.overlay = Overlay::UpdateResult {
                    success: true,
                    message: texts::tui_update_success(&tag),
                };
            }
            Err(e) => {
                app.overlay = Overlay::UpdateResult {
                    success: false,
                    message: e,
                };
            }
        },
    }
}

fn start_update_system() -> Result<UpdateSystem, AppError> {
    let (result_tx, result_rx) = mpsc::channel::<UpdateMsg>();
    let (req_tx, req_rx) = mpsc::channel::<UpdateReq>();

    let handle = std::thread::Builder::new()
        .name("cc-switch-update".to_string())
        .spawn(move || update_worker_loop(req_rx, result_tx))
        .map_err(|e| AppError::IoContext {
            context: "failed to spawn update worker thread".to_string(),
            source: e,
        })?;

    Ok(UpdateSystem {
        req_tx,
        result_rx,
        _handle: handle,
    })
}

fn update_worker_loop(rx: mpsc::Receiver<UpdateReq>, tx: mpsc::Sender<UpdateMsg>) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let err = e.to_string();
            while let Ok(req) = rx.recv() {
                let msg = match req {
                    UpdateReq::Check { request_id } => UpdateMsg::CheckFinished {
                        request_id,
                        result: Err(err.clone()),
                    },
                    UpdateReq::Download => UpdateMsg::DownloadFinished(Err(err.clone())),
                };
                let _ = tx.send(msg);
            }
            return;
        }
    };

    let mut last_tag: Option<String> = None;

    while let Ok(req) = rx.recv() {
        match req {
            UpdateReq::Check { request_id } => {
                let result = rt
                    .block_on(crate::cli::commands::update::check_for_update())
                    .map_err(|e| e.to_string());
                if let Ok(ref info) = result {
                    last_tag = Some(info.target_tag.clone());
                }
                let _ = tx.send(UpdateMsg::CheckFinished { request_id, result });
            }
            UpdateReq::Download => {
                let Some(tag) = last_tag.clone() else {
                    let _ = tx.send(UpdateMsg::DownloadFinished(Err(
                        texts::tui_update_err_check_first().to_string(),
                    )));
                    continue;
                };
                let tx2 = tx.clone();
                let result = rt
                    .block_on(crate::cli::commands::update::download_and_apply(
                        &tag,
                        move |dl, total| {
                            let _ = tx2.send(UpdateMsg::DownloadProgress {
                                downloaded: dl,
                                total,
                            });
                        },
                    ))
                    .map(|()| tag)
                    .map_err(|e| e.to_string());
                let _ = tx.send(UpdateMsg::DownloadFinished(result));
            }
        }
    }
}

fn start_webdav_system() -> Result<WebDavSystem, AppError> {
    let (result_tx, result_rx) = mpsc::channel::<WebDavMsg>();
    let (req_tx, req_rx) = mpsc::channel::<WebDavReq>();

    let handle = std::thread::Builder::new()
        .name("cc-switch-webdav".to_string())
        .spawn(move || webdav_worker_loop(req_rx, result_tx))
        .map_err(|e| AppError::IoContext {
            context: "failed to spawn webdav worker thread".to_string(),
            source: e,
        })?;

    Ok(WebDavSystem {
        req_tx,
        result_rx,
        _handle: handle,
    })
}

fn drain_latest_webdav_req(mut req: WebDavReq, rx: &mpsc::Receiver<WebDavReq>) -> WebDavReq {
    for next in rx.try_iter() {
        req = next;
    }
    req
}

fn webdav_worker_loop(rx: mpsc::Receiver<WebDavReq>, tx: mpsc::Sender<WebDavMsg>) {
    while let Ok(req) = rx.recv() {
        let req = drain_latest_webdav_req(req, &rx);
        let request_id = req.request_id;
        let req_for_msg = req.kind.clone();
        let result = match req.kind {
            WebDavReqKind::CheckConnection => WebDavSyncService::check_connection()
                .map(|_| WebDavDone::ConnectionChecked)
                .map_err(|e| WebDavErr::Generic(e.to_string())),
            WebDavReqKind::Upload => WebDavSyncService::upload()
                .map(|summary| WebDavDone::Uploaded {
                    decision: summary.decision,
                    message: summary.message,
                })
                .map_err(|e| WebDavErr::Generic(e.to_string())),
            WebDavReqKind::Download => WebDavSyncService::download()
                .map(|summary| WebDavDone::Downloaded {
                    decision: summary.decision,
                    message: summary.message,
                })
                .map_err(|e| WebDavErr::Generic(e.to_string())),
            WebDavReqKind::JianguoyunQuickSetup { username, password } => {
                let cfg = webdav_jianguoyun_preset(&username, &password);
                if let Err(err) = set_webdav_sync_settings(Some(cfg)) {
                    Err(WebDavErr::QuickSetupSave(err.to_string()))
                } else if let Err(err) = WebDavSyncService::check_connection() {
                    Err(WebDavErr::QuickSetupCheck(err.to_string()))
                } else {
                    Ok(WebDavDone::JianguoyunConfigured)
                }
            }
        };

        let _ = tx.send(WebDavMsg::Finished {
            request_id,
            req: req_for_msg,
            result,
        });
    }
}

fn start_speedtest_system() -> Result<SpeedtestSystem, AppError> {
    let (result_tx, result_rx) = mpsc::channel::<SpeedtestMsg>();
    let (req_tx, req_rx) = mpsc::channel::<String>();

    let handle = std::thread::Builder::new()
        .name("cc-switch-speedtest".to_string())
        .spawn(move || speedtest_worker_loop(req_rx, result_tx))
        .map_err(|e| AppError::IoContext {
            context: "failed to spawn speedtest worker thread".to_string(),
            source: e,
        })?;

    Ok(SpeedtestSystem {
        req_tx,
        result_rx,
        _handle: handle,
    })
}

fn speedtest_worker_loop(rx: mpsc::Receiver<String>, tx: mpsc::Sender<SpeedtestMsg>) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let err = e.to_string();
            while let Ok(url) = rx.recv() {
                let _ = tx.send(SpeedtestMsg::Finished {
                    url,
                    result: Err(err.clone()),
                });
            }
            return;
        }
    };

    while let Ok(mut url) = rx.recv() {
        for next in rx.try_iter() {
            url = next;
        }

        let result = rt
            .block_on(async {
                crate::services::SpeedtestService::test_endpoints(vec![url.clone()], None).await
            })
            .map_err(|e| e.to_string());

        let _ = tx.send(SpeedtestMsg::Finished { url, result });
    }
}

fn start_local_env_system() -> Result<LocalEnvSystem, AppError> {
    let (result_tx, result_rx) = mpsc::channel::<LocalEnvMsg>();
    let (req_tx, req_rx) = mpsc::channel::<LocalEnvReq>();

    let handle = std::thread::Builder::new()
        .name("cc-switch-local-env".to_string())
        .spawn(move || local_env_worker_loop(req_rx, result_tx))
        .map_err(|e| AppError::IoContext {
            context: "failed to spawn local env worker thread".to_string(),
            source: e,
        })?;

    Ok(LocalEnvSystem {
        req_tx,
        result_rx,
        _handle: handle,
    })
}

fn local_env_worker_loop(rx: mpsc::Receiver<LocalEnvReq>, tx: mpsc::Sender<LocalEnvMsg>) {
    while let Ok(mut req) = rx.recv() {
        for next in rx.try_iter() {
            req = next;
        }

        match req {
            LocalEnvReq::Refresh => {
                let result = crate::services::local_env_check::check_local_environment();
                let _ = tx.send(LocalEnvMsg::Finished { result });
            }
        }
    }
}

fn start_skills_system() -> Result<SkillsSystem, AppError> {
    let (result_tx, result_rx) = mpsc::channel::<SkillsMsg>();
    let (req_tx, req_rx) = mpsc::channel::<SkillsReq>();

    let handle = std::thread::Builder::new()
        .name("cc-switch-skills".to_string())
        .spawn(move || skills_worker_loop(req_rx, result_tx))
        .map_err(|e| AppError::IoContext {
            context: "failed to spawn skills worker thread".to_string(),
            source: e,
        })?;

    Ok(SkillsSystem {
        req_tx,
        result_rx,
        _handle: handle,
    })
}

fn skills_worker_loop(rx: mpsc::Receiver<SkillsReq>, tx: mpsc::Sender<SkillsMsg>) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let err = e.to_string();
            while let Ok(req) = rx.recv() {
                match req {
                    SkillsReq::Discover { query } => {
                        let _ = tx.send(SkillsMsg::DiscoverFinished {
                            query,
                            result: Err(err.clone()),
                        });
                    }
                    SkillsReq::Install { spec, .. } => {
                        let _ = tx.send(SkillsMsg::InstallFinished {
                            spec,
                            result: Err(err.clone()),
                        });
                    }
                }
            }
            return;
        }
    };

    let service = match SkillService::new() {
        Ok(service) => service,
        Err(e) => {
            let err = e.to_string();
            while let Ok(req) = rx.recv() {
                match req {
                    SkillsReq::Discover { query } => {
                        let _ = tx.send(SkillsMsg::DiscoverFinished {
                            query,
                            result: Err(err.clone()),
                        });
                    }
                    SkillsReq::Install { spec, .. } => {
                        let _ = tx.send(SkillsMsg::InstallFinished {
                            spec,
                            result: Err(err.clone()),
                        });
                    }
                }
            }
            return;
        }
    };

    while let Ok(req) = rx.recv() {
        match req {
            SkillsReq::Discover { query } => {
                let query_trimmed = query.trim().to_lowercase();
                let result = rt
                    .block_on(async { service.list_skills().await })
                    .map_err(|e| e.to_string())
                    .map(|mut skills| {
                        if !query_trimmed.is_empty() {
                            skills.retain(|s| {
                                s.name.to_lowercase().contains(&query_trimmed)
                                    || s.directory.to_lowercase().contains(&query_trimmed)
                                    || s.description.to_lowercase().contains(&query_trimmed)
                                    || s.key.to_lowercase().contains(&query_trimmed)
                            });
                        }
                        skills
                    });

                let _ = tx.send(SkillsMsg::DiscoverFinished { query, result });
            }
            SkillsReq::Install { spec, app } => {
                let spec_clone = spec.clone();
                let app_clone = app.clone();
                let result = rt
                    .block_on(async { service.install(&spec_clone, &app_clone).await })
                    .map_err(|e| e.to_string());
                let _ = tx.send(SkillsMsg::InstallFinished { spec, result });
            }
        }
    }
}

fn parse_repo_spec(raw: &str) -> Result<SkillRepo, AppError> {
    let raw = raw.trim().trim_end_matches('/');
    if raw.is_empty() {
        return Err(AppError::InvalidInput(
            texts::tui_error_repo_spec_empty().to_string(),
        ));
    }

    // Allow: https://github.com/owner/name or owner/name[@branch]
    let without_prefix = raw
        .strip_prefix("https://github.com/")
        .or_else(|| raw.strip_prefix("http://github.com/"))
        .unwrap_or(raw);

    let without_git = without_prefix.trim_end_matches(".git");

    let (path, branch) = if let Some((left, right)) = without_git.rsplit_once('@') {
        (left, Some(right))
    } else {
        (without_git, None)
    };

    let Some((owner, name)) = path.split_once('/') else {
        return Err(AppError::InvalidInput(
            texts::tui_error_repo_spec_invalid().to_string(),
        ));
    };

    Ok(SkillRepo {
        owner: owner.to_string(),
        name: name.to_string(),
        branch: branch.unwrap_or("main").to_string(),
        enabled: true,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::app::{App, LoadingKind, Overlay};
    use crate::cli::i18n::texts;
    use crate::AppError;

    #[test]
    fn command_lookup_name_extracts_first_token() {
        assert_eq!(super::command_lookup_name("node --version"), Some("node"));
        assert_eq!(super::command_lookup_name("  rg   -n foo "), Some("rg"));
    }

    #[test]
    fn command_lookup_name_rejects_empty_or_whitespace() {
        assert_eq!(super::command_lookup_name(""), None);
        assert_eq!(super::command_lookup_name("   "), None);
    }

    #[test]
    fn quick_setup_helper_saves_preset_and_runs_connection_check() {
        let mut captured = None;
        let mut checked = false;

        super::apply_webdav_jianguoyun_quick_setup(
            " demo@nutstore.com ",
            " app-password ",
            |cfg| {
                captured = Some(cfg);
                Ok(())
            },
            || {
                checked = true;
                Ok(())
            },
        )
        .expect("quick setup helper should succeed");

        let saved = captured.expect("settings should be saved");
        assert!(saved.enabled);
        assert_eq!(saved.base_url, "https://dav.jianguoyun.com/dav");
        assert_eq!(saved.remote_root, "cc-switch-sync");
        assert_eq!(saved.profile, "default");
        assert_eq!(saved.username, "demo@nutstore.com");
        assert_eq!(saved.password, "app-password");
        assert!(checked, "connection check should be called");
    }

    #[test]
    fn quick_setup_helper_stops_when_save_fails() {
        let mut checked = false;
        let err = super::apply_webdav_jianguoyun_quick_setup(
            "u",
            "p",
            |_cfg| Err(AppError::Message("save failed".to_string())),
            || {
                checked = true;
                Ok(())
            },
        )
        .expect_err("save failure should be returned");

        assert!(err.to_string().contains("save failed"));
        assert!(!checked, "connection check should not run when save fails");
    }

    #[test]
    fn drain_latest_webdav_req_prefers_last_enqueued_request() {
        let (tx, rx) = mpsc::channel();
        tx.send(super::WebDavReq {
            request_id: 1,
            kind: super::WebDavReqKind::CheckConnection,
        })
        .expect("send check request");
        tx.send(super::WebDavReq {
            request_id: 2,
            kind: super::WebDavReqKind::Upload,
        })
        .expect("send upload request");
        tx.send(super::WebDavReq {
            request_id: 3,
            kind: super::WebDavReqKind::JianguoyunQuickSetup {
                username: "u@example.com".to_string(),
                password: "p".to_string(),
            },
        })
        .expect("send quick setup request");

        let first = rx.recv().expect("receive first request");
        let latest = super::drain_latest_webdav_req(first, &rx);
        assert!(matches!(
            latest,
            super::WebDavReq {
                request_id: 3,
                kind: super::WebDavReqKind::JianguoyunQuickSetup { username, password }
            }
                if username == "u@example.com" && password == "p"
        ));
    }

    #[test]
    fn update_webdav_last_error_with_updates_status_when_present() {
        let mut captured = None;
        super::update_webdav_last_error_with(
            Some("network timeout".to_string()),
            || Some(crate::settings::WebDavSyncSettings::default()),
            |cfg| {
                captured = Some(cfg);
                Ok(())
            },
        );

        let saved = captured.expect("expected settings to be saved");
        assert_eq!(saved.status.last_error.as_deref(), Some("network timeout"));
    }

    #[test]
    fn update_webdav_last_error_with_skips_when_settings_absent() {
        let mut saved = false;
        super::update_webdav_last_error_with(
            Some("network timeout".to_string()),
            || None,
            |_cfg| {
                saved = true;
                Ok(())
            },
        );
        assert!(
            !saved,
            "set callback should not run when webdav settings are missing"
        );
    }

    #[test]
    fn update_success_does_not_force_exit_when_overlay_hidden() {
        let mut app = App::new(None);
        app.overlay = Overlay::None;
        let mut update_check = super::RequestTracker::default();

        super::handle_update_msg(
            &mut app,
            &mut update_check,
            super::UpdateMsg::DownloadFinished(Ok("v9.9.9".to_string())),
        );

        assert!(
            !app.should_quit,
            "successful update should not force exit without user confirmation"
        );
        assert!(
            matches!(app.overlay, Overlay::UpdateResult { success: true, .. }),
            "successful update should show result overlay even when progress overlay was hidden"
        );
    }

    #[test]
    fn update_check_finished_is_ignored_when_canceled() {
        let mut app = App::new(None);
        app.overlay = Overlay::None;
        let mut update_check = super::RequestTracker::default();

        let info = crate::cli::commands::update::UpdateCheckInfo {
            current_version: "4.7.0".to_string(),
            target_tag: "v9.9.9".to_string(),
            is_already_latest: false,
            is_downgrade: false,
        };

        super::handle_update_msg(
            &mut app,
            &mut update_check,
            super::UpdateMsg::CheckFinished {
                request_id: 1,
                result: Ok(info),
            },
        );

        assert!(
            matches!(app.overlay, Overlay::None),
            "update check result should be ignored after cancel/hide"
        );
    }

    #[test]
    fn update_check_finished_is_processed_when_request_id_matches() {
        let mut app = App::new(None);
        app.overlay = Overlay::Loading {
            kind: LoadingKind::UpdateCheck,
            title: texts::tui_update_checking_title().to_string(),
            message: texts::tui_loading().to_string(),
        };
        let mut update_check = super::RequestTracker::default();
        update_check.active = Some(7);

        let info = crate::cli::commands::update::UpdateCheckInfo {
            current_version: "4.7.0".to_string(),
            target_tag: "v9.9.9".to_string(),
            is_already_latest: false,
            is_downgrade: false,
        };

        super::handle_update_msg(
            &mut app,
            &mut update_check,
            super::UpdateMsg::CheckFinished {
                request_id: 7,
                result: Ok(info),
            },
        );

        assert_eq!(update_check.active, None);
        assert!(matches!(
            app.overlay,
            Overlay::UpdateAvailable {
                latest,
                selected: 0,
                ..
            } if latest == "v9.9.9"
        ));
    }

    #[test]
    fn update_check_finished_is_ignored_when_request_id_mismatch() {
        let mut app = App::new(None);
        app.overlay = Overlay::None;
        let mut update_check = super::RequestTracker::default();
        update_check.active = Some(2);

        let stale = crate::cli::commands::update::UpdateCheckInfo {
            current_version: "4.7.0".to_string(),
            target_tag: "v1.0.0".to_string(),
            is_already_latest: false,
            is_downgrade: false,
        };
        super::handle_update_msg(
            &mut app,
            &mut update_check,
            super::UpdateMsg::CheckFinished {
                request_id: 1,
                result: Ok(stale),
            },
        );

        assert_eq!(update_check.active, Some(2));
        assert!(matches!(app.overlay, Overlay::None));

        let latest = crate::cli::commands::update::UpdateCheckInfo {
            current_version: "4.7.0".to_string(),
            target_tag: "v9.9.9".to_string(),
            is_already_latest: false,
            is_downgrade: false,
        };
        super::handle_update_msg(
            &mut app,
            &mut update_check,
            super::UpdateMsg::CheckFinished {
                request_id: 2,
                result: Ok(latest),
            },
        );

        assert_eq!(update_check.active, None);
        assert!(matches!(app.overlay, Overlay::UpdateAvailable { .. }));
    }
}
