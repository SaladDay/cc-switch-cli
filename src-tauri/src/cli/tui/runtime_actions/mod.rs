use std::sync::mpsc;

use crate::app_config::AppType;
use crate::cli::i18n::{set_language, texts};
use crate::error::AppError;

use super::app::{Action, App, Focus, Overlay, ToastKind};
use super::data::UiData;
use super::runtime_systems::{
    LocalEnvReq, ModelFetchReq, ProxyReq, RequestTracker, SkillsReq, StreamCheckReq, UpdateReq,
    WebDavReq,
};
use super::terminal::TuiTerminal;

mod claude_temp_launch;
mod codex_temp_launch;
mod config;
mod editor;
mod helpers;
mod mcp;
mod prompts;
mod providers;
mod settings;
mod skills;
mod updates;

pub(crate) use helpers::{app_display_name, queue_managed_proxy_action};
#[cfg(test)]
pub(crate) use helpers::{
    import_mcp_for_current_app_with, open_proxy_help_overlay_with,
    run_external_editor_for_current_editor,
};

fn normalize_route_for_app(app_type: &AppType, route: &super::route::Route) -> super::route::Route {
    match app_type {
        AppType::OpenClaw => match route {
            super::route::Route::Main
            | super::route::Route::Providers
            | super::route::Route::ProviderDetail { .. }
            | super::route::Route::ConfigOpenClawWorkspace
            | super::route::Route::ConfigOpenClawDailyMemory
            | super::route::Route::ConfigOpenClawEnv
            | super::route::Route::ConfigOpenClawTools
            | super::route::Route::ConfigOpenClawAgents
            | super::route::Route::Settings
            | super::route::Route::SettingsProxy => route.clone(),
            _ => super::route::Route::Main,
        },
        _ => match route {
            super::route::Route::ConfigOpenClawWorkspace
            | super::route::Route::ConfigOpenClawDailyMemory
            | super::route::Route::ConfigOpenClawEnv
            | super::route::Route::ConfigOpenClawTools
            | super::route::Route::ConfigOpenClawAgents => super::route::Route::Config,
            _ => route.clone(),
        },
    }
}

fn apply_preloaded_app_switch(app: &mut App, data: &mut UiData, next: AppType, next_data: UiData) {
    app.clear_openclaw_daily_memory_search_state();
    app.app_type = next;
    let original_route = app.route.clone();
    app.route = normalize_route_for_app(&app.app_type, &app.route);
    for route in &mut app.route_stack {
        *route = normalize_route_for_app(&app.app_type, route);
    }
    while app.route_stack.last() == Some(&app.route) {
        app.route_stack.pop();
    }
    if let Some(idx) = app
        .nav_items()
        .iter()
        .position(|item| *item == App::nav_item_for_route(&app.app_type, &app.route))
    {
        app.nav_idx = idx;
    }
    if app.route != original_route {
        app.focus = if matches!(app.route, super::route::Route::Main) {
            Focus::Nav
        } else {
            Focus::Content
        };
    }
    *data = next_data;
    app.reset_proxy_activity(
        data.proxy.estimated_input_tokens_total,
        data.proxy.estimated_output_tokens_total,
    );
}

pub(super) struct RuntimeActionContext<'a> {
    terminal: &'a mut TuiTerminal,
    app: &'a mut App,
    data: &'a mut UiData,
    speedtest_req_tx: Option<&'a mpsc::Sender<String>>,
    stream_check_req_tx: Option<&'a mpsc::Sender<StreamCheckReq>>,
    skills_req_tx: Option<&'a mpsc::Sender<SkillsReq>>,
    proxy_req_tx: Option<&'a mpsc::Sender<ProxyReq>>,
    proxy_loading: &'a mut RequestTracker,
    local_env_req_tx: Option<&'a mpsc::Sender<LocalEnvReq>>,
    webdav_req_tx: Option<&'a mpsc::Sender<WebDavReq>>,
    webdav_loading: &'a mut RequestTracker,
    update_req_tx: Option<&'a mpsc::Sender<UpdateReq>>,
    update_check: &'a mut RequestTracker,
    model_fetch_req_tx: Option<&'a mpsc::Sender<ModelFetchReq>>,
}

pub(crate) fn handle_action(
    terminal: &mut TuiTerminal,
    app: &mut App,
    data: &mut UiData,
    speedtest_req_tx: Option<&mpsc::Sender<String>>,
    stream_check_req_tx: Option<&mpsc::Sender<StreamCheckReq>>,
    skills_req_tx: Option<&mpsc::Sender<SkillsReq>>,
    proxy_req_tx: Option<&mpsc::Sender<ProxyReq>>,
    proxy_loading: &mut RequestTracker,
    local_env_req_tx: Option<&mpsc::Sender<LocalEnvReq>>,
    webdav_req_tx: Option<&mpsc::Sender<WebDavReq>>,
    webdav_loading: &mut RequestTracker,
    update_req_tx: Option<&mpsc::Sender<UpdateReq>>,
    update_check: &mut RequestTracker,
    model_fetch_req_tx: Option<&mpsc::Sender<ModelFetchReq>>,
    action: Action,
) -> Result<(), AppError> {
    let mut ctx = RuntimeActionContext {
        terminal,
        app,
        data,
        speedtest_req_tx,
        stream_check_req_tx,
        skills_req_tx,
        proxy_req_tx,
        proxy_loading,
        local_env_req_tx,
        webdav_req_tx,
        webdav_loading,
        update_req_tx,
        update_check,
        model_fetch_req_tx,
    };

    match action {
        Action::None => Ok(()),
        Action::ReloadData => {
            *ctx.data = UiData::load(&ctx.app.app_type)?;
            Ok(())
        }
        Action::SetAppType(next) => {
            let next_data = UiData::load(&next)?;
            apply_preloaded_app_switch(ctx.app, ctx.data, next, next_data);
            Ok(())
        }
        Action::LocalEnvRefresh => {
            let Some(tx) = ctx.local_env_req_tx else {
                ctx.app.local_env_loading = false;
                ctx.app.push_toast(
                    texts::tui_toast_local_env_check_disabled(),
                    ToastKind::Warning,
                );
                return Ok(());
            };

            ctx.app.local_env_loading = true;
            if let Err(err) = tx.send(LocalEnvReq::Refresh) {
                ctx.app.local_env_loading = false;
                ctx.app.push_toast(
                    texts::tui_toast_local_env_check_request_failed(&err.to_string()),
                    ToastKind::Warning,
                );
            }
            Ok(())
        }
        Action::SwitchRoute(route) => {
            ctx.app.route = route;
            Ok(())
        }
        Action::Quit => {
            ctx.app.should_quit = true;
            Ok(())
        }
        Action::SkillsToggle { directory, enabled } => skills::toggle(&mut ctx, directory, enabled),
        Action::SkillsSetApps { directory, apps } => skills::set_apps(&mut ctx, directory, apps),
        Action::SkillsInstall { spec } => skills::install(&mut ctx, spec),
        Action::SkillsUninstall { directory } => skills::uninstall(&mut ctx, directory),
        Action::SkillsSync { app: scope } => skills::sync(&mut ctx, scope),
        Action::SkillsSetSyncMethod { method } => skills::set_sync_method(&mut ctx, method),
        Action::SkillsDiscover { query } => skills::discover(&mut ctx, query),
        Action::SkillsRepoAdd { spec } => skills::repo_add(&mut ctx, spec),
        Action::SkillsRepoRemove { owner, name } => skills::repo_remove(&mut ctx, owner, name),
        Action::SkillsRepoToggleEnabled {
            owner,
            name,
            enabled,
        } => skills::repo_toggle_enabled(&mut ctx, owner, name, enabled),
        Action::SkillsOpenImport => skills::open_import(&mut ctx),
        Action::SkillsScanUnmanaged => skills::scan_unmanaged(&mut ctx),
        Action::SkillsImportFromApps { directories } => {
            skills::import_from_apps(&mut ctx, directories)
        }
        Action::EditorDiscard => {
            ctx.app.editor = None;
            Ok(())
        }
        Action::EditorOpenExternal => editor::open_external(&mut ctx),
        Action::EditorSubmit { submit, content } => editor::submit(&mut ctx, submit, content),
        Action::ProviderSwitch { id } => providers::switch(&mut ctx, id),
        Action::ProviderRemoveFromConfig { id } => providers::remove_from_config(&mut ctx, id),
        Action::ProviderSetDefaultModel {
            provider_id,
            model_id,
        } => providers::set_default_model(&mut ctx, provider_id, model_id),
        Action::ProviderImportLiveConfig => providers::import_live_config(&mut ctx),
        Action::ProviderDelete { id } => providers::delete(&mut ctx, id),
        Action::ProviderSpeedtest { url } => providers::speedtest(&mut ctx, url),
        Action::ProviderLaunchTemporary { id } => match ctx.app.app_type {
            AppType::Claude => claude_temp_launch::launch(&mut ctx, id),
            AppType::Codex => codex_temp_launch::launch(&mut ctx, id),
            _ => Ok(()),
        },
        Action::ProviderStreamCheck { id } => providers::stream_check(&mut ctx, id),
        Action::ProviderSetFailoverQueue { id, enabled } => {
            providers::set_failover_queue(&mut ctx, id, enabled)
        }
        Action::ProviderMoveFailoverQueue { id, direction } => {
            providers::move_failover_queue(&mut ctx, id, direction)
        }
        Action::ProviderQuotaRefresh { .. } => Ok(()),
        Action::ProviderModelFetch {
            base_url,
            api_key,
            field,
            claude_idx,
        } => providers::model_fetch(&mut ctx, base_url, api_key, field, claude_idx),
        Action::McpToggle { id, enabled } => mcp::toggle(&mut ctx, id, enabled),
        Action::McpSetApps { id, apps } => mcp::set_apps(&mut ctx, id, apps),
        Action::McpDelete { id } => mcp::delete(&mut ctx, id),
        Action::McpImport => mcp::import_current_app(&mut ctx),
        Action::PromptActivate { id } => prompts::activate(&mut ctx, id),
        Action::PromptDeactivate { id } => prompts::deactivate(&mut ctx, id),
        Action::PromptDelete { id } => prompts::delete(&mut ctx, id),
        Action::ConfigExport { path } => config::export(&mut ctx, path),
        Action::ConfigShowFull => config::show_full(&mut ctx),
        Action::ConfigImport { path } => config::import(&mut ctx, path),
        Action::ConfigBackup { name } => config::backup(&mut ctx, name),
        Action::ConfigRestoreBackup { id } => config::restore_backup(&mut ctx, id),
        Action::ConfigValidate => config::validate(&mut ctx),
        Action::ConfigOpenProxyHelp => config::open_proxy_help(&mut ctx),
        Action::ConfigCommonSnippetClear { app_type } => {
            config::clear_common_snippet(&mut ctx, app_type)
        }
        Action::ConfigCommonSnippetApply { app_type } => {
            config::apply_common_snippet(&mut ctx, app_type)
        }
        Action::ConfigWebDavCheckConnection => config::webdav_check_connection(&mut ctx),
        Action::ConfigWebDavUpload => config::webdav_upload(&mut ctx),
        Action::ConfigWebDavDownload => config::webdav_download(&mut ctx),
        Action::ConfigWebDavMigrateV1ToV2 => config::webdav_migrate_v1_to_v2(&mut ctx),
        Action::ConfigWebDavReset => config::webdav_reset(&mut ctx),
        Action::ConfigWebDavJianguoyunQuickSetup { username, password } => {
            config::webdav_jianguoyun_quick_setup(&mut ctx, username, password)
        }
        Action::OpenClawWorkspaceOpenFile { filename } => {
            config::open_openclaw_workspace_file(&mut ctx, filename)
        }
        Action::OpenClawDailyMemoryOpenFile { filename } => {
            config::open_openclaw_daily_memory_file(&mut ctx, filename)
        }
        Action::OpenClawDailyMemorySearch { query } => {
            config::search_openclaw_daily_memory(&mut ctx, query)
        }
        Action::OpenClawDailyMemoryDelete { filename } => {
            config::delete_openclaw_daily_memory(&mut ctx, filename)
        }
        Action::OpenClawOpenDirectory { subdir } => {
            config::open_openclaw_directory(&mut ctx, subdir)
        }
        Action::ConfigReset => config::reset(&mut ctx),
        Action::SetSkipClaudeOnboarding { enabled } => {
            crate::settings::set_skip_claude_onboarding(enabled)?;
            ctx.app.push_toast(
                texts::tui_toast_skip_claude_onboarding_toggled(enabled),
                ToastKind::Success,
            );
            Ok(())
        }
        Action::SetClaudePluginIntegration { enabled } => {
            crate::settings::set_enable_claude_plugin_integration(enabled)?;
            if let Err(err) = crate::claude_plugin::sync_claude_plugin_on_settings_toggle(enabled) {
                ctx.app.push_toast(
                    texts::tui_toast_claude_plugin_sync_failed(&err.to_string()),
                    ToastKind::Warning,
                );
            }
            ctx.app.push_toast(
                texts::tui_toast_claude_plugin_integration_toggled(enabled),
                ToastKind::Success,
            );
            Ok(())
        }
        Action::SetProxyEnabled { enabled } => settings::set_proxy_enabled(&mut ctx, enabled),
        Action::SetProxyListenAddress { address } => {
            settings::set_proxy_listen_address(&mut ctx, address)
        }
        Action::SetProxyListenPort { port } => settings::set_proxy_listen_port(&mut ctx, port),
        Action::SetProxyAutoFailover { app_type, enabled } => {
            settings::set_proxy_auto_failover(&mut ctx, app_type, enabled)
        }
        Action::SetOpenClawConfigDir { path } => settings::set_openclaw_config_dir(&mut ctx, path),
        Action::SetProxyTakeover { app_type, enabled } => {
            settings::set_proxy_takeover(&mut ctx, app_type, enabled)
        }
        Action::SetManagedProxyForCurrentApp { app_type, enabled } => queue_managed_proxy_action(
            ctx.app,
            ctx.proxy_req_tx,
            ctx.proxy_loading,
            app_type,
            enabled,
        ),
        Action::SetLanguage(lang) => {
            set_language(lang)?;
            ctx.app
                .push_toast(texts::language_changed(), ToastKind::Success);
            Ok(())
        }
        Action::SetVisibleApps { apps } => settings::set_visible_apps(&mut ctx, apps),
        Action::CheckUpdate => updates::check(&mut ctx),
        Action::ConfirmUpdate => updates::confirm(&mut ctx),
        Action::CancelUpdate => {
            ctx.app.overlay = Overlay::None;
            Ok(())
        }
        Action::CancelUpdateCheck => {
            ctx.update_check.cancel();
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::AppType;
    use crate::cli::tui::app::App;
    use crate::cli::tui::route::Route;
    use crate::test_support::{
        lock_test_home_and_settings, set_test_home_override, TestHomeSettingsLock,
    };
    use serial_test::serial;
    use std::ffi::OsString;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    struct EnvGuard {
        _lock: TestHomeSettingsLock,
        old_home: Option<OsString>,
        old_userprofile: Option<OsString>,
        old_config_dir: Option<OsString>,
    }

    impl EnvGuard {
        fn set_home(home: &Path) -> Self {
            let lock = lock_test_home_and_settings();
            let old_home = std::env::var_os("HOME");
            let old_userprofile = std::env::var_os("USERPROFILE");
            let old_config_dir = std::env::var_os("CC_SWITCH_CONFIG_DIR");
            std::env::set_var("HOME", home);
            std::env::set_var("USERPROFILE", home);
            std::env::set_var("CC_SWITCH_CONFIG_DIR", home.join(".cc-switch"));
            set_test_home_override(Some(home));
            crate::settings::reload_test_settings();
            Self {
                _lock: lock,
                old_home,
                old_userprofile,
                old_config_dir,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.old_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match &self.old_userprofile {
                Some(value) => std::env::set_var("USERPROFILE", value),
                None => std::env::remove_var("USERPROFILE"),
            }
            match &self.old_config_dir {
                Some(value) => std::env::set_var("CC_SWITCH_CONFIG_DIR", value),
                None => std::env::remove_var("CC_SWITCH_CONFIG_DIR"),
            }
            set_test_home_override(self.old_home.as_deref().map(Path::new));
            crate::settings::reload_test_settings();
        }
    }

    fn run_action(app: &mut App, data: &mut UiData, action: Action) -> Result<(), AppError> {
        let mut terminal = TuiTerminal::new_for_test().expect("create terminal");
        let mut proxy_loading = RequestTracker::default();
        let mut webdav_loading = RequestTracker::default();
        let mut update_check = RequestTracker::default();

        handle_action(
            &mut terminal,
            app,
            data,
            None,
            None,
            None,
            None,
            &mut proxy_loading,
            None,
            None,
            &mut webdav_loading,
            None,
            &mut update_check,
            None,
            action,
        )
    }

    fn write_invalid_legacy_config(home: &Path) {
        let config_dir = home.join(".cc-switch");
        fs::create_dir_all(&config_dir).expect("create config dir");
        fs::write(config_dir.join("config.json"), "{ not valid json }")
            .expect("write invalid legacy config");
    }

    #[test]
    #[serial(home_settings)]
    fn set_app_type_normalizes_openclaw_config_subroutes_back_to_config() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        let mut terminal = TuiTerminal::new_for_test().expect("create terminal");
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.route_stack.push(Route::Config);
        app.filter.active = true;
        app.filter.buffer = "focus".to_string();
        app.openclaw_daily_memory_search_query = "focus".to_string();
        app.daily_memory_idx = 1;
        app.openclaw_daily_memory_search_results =
            vec![crate::commands::workspace::DailyMemorySearchResult {
                filename: "2026-03-20.md".to_string(),
                date: "2026-03-20".to_string(),
                size_bytes: 12,
                modified_at: 1,
                snippet: "focus".to_string(),
                match_count: 1,
            }];
        let mut data = UiData::default();
        let mut proxy_loading = RequestTracker::default();
        let mut webdav_loading = RequestTracker::default();
        let mut update_check = RequestTracker::default();

        handle_action(
            &mut terminal,
            &mut app,
            &mut data,
            None,
            None,
            None,
            None,
            &mut proxy_loading,
            None,
            None,
            &mut webdav_loading,
            None,
            &mut update_check,
            None,
            Action::SetAppType(AppType::Claude),
        )
        .expect("switch app type");

        assert_eq!(app.app_type, AppType::Claude);
        assert_eq!(app.route, Route::Config);
        assert!(
            app.route_stack.is_empty(),
            "route stack should be normalized too so Back does not land on a duplicate config route"
        );
        assert!(!app.filter.active);
        assert!(app.filter.buffer.is_empty());
        assert!(app.openclaw_daily_memory_search_query.is_empty());
        assert!(app.openclaw_daily_memory_search_results.is_empty());
        assert_eq!(app.daily_memory_idx, 0);
    }

    #[test]
    #[serial(home_settings)]
    fn set_app_type_normalizes_unsupported_routes_when_switching_into_openclaw() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Mcp;
        app.route_stack.push(Route::Prompts);
        app.focus = super::super::app::Focus::Content;
        let mut data = UiData::default();

        run_action(&mut app, &mut data, Action::SetAppType(AppType::OpenClaw))
            .expect("switch app type");

        assert_eq!(app.app_type, AppType::OpenClaw);
        assert_eq!(app.route, Route::Main);
        assert!(app.route_stack.is_empty());
        assert_eq!(app.nav_item(), super::super::route::NavItem::Main);
        assert!(matches!(app.focus, super::super::app::Focus::Nav));
    }

    #[test]
    #[serial(home_settings)]
    fn set_visible_apps_forces_switch_and_normalizes_openclaw_routes() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        crate::settings::set_visible_apps(crate::settings::VisibleApps {
            claude: true,
            codex: true,
            gemini: true,
            opencode: true,
            openclaw: true,
        })
        .expect("save initial visible apps");

        let next_visible_apps = crate::settings::VisibleApps {
            claude: true,
            codex: false,
            gemini: false,
            opencode: false,
            openclaw: false,
        };
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.route_stack.push(Route::Config);
        app.filter.active = true;
        app.filter.buffer = "focus".to_string();
        app.openclaw_daily_memory_search_query = "focus".to_string();
        app.daily_memory_idx = 1;
        app.openclaw_daily_memory_search_results =
            vec![crate::commands::workspace::DailyMemorySearchResult {
                filename: "2026-03-20.md".to_string(),
                date: "2026-03-20".to_string(),
                size_bytes: 12,
                modified_at: 1,
                snippet: "focus".to_string(),
                match_count: 1,
            }];
        let mut data = UiData::default();

        run_action(
            &mut app,
            &mut data,
            Action::SetVisibleApps {
                apps: next_visible_apps.clone(),
            },
        )
        .expect("set visible apps");

        assert_eq!(crate::settings::get_visible_apps(), next_visible_apps);
        assert_eq!(app.app_type, AppType::Claude);
        assert_eq!(app.route, Route::Config);
        assert!(matches!(
            app.toast.as_ref(),
            Some(toast)
                if toast.kind == super::super::app::ToastKind::Success
                    && toast.message == texts::tui_toast_visible_apps_saved()
        ));
        assert!(
            app.route_stack.is_empty(),
            "route stack should normalize the same way as SetAppType"
        );
        assert!(!app.filter.active);
        assert!(app.filter.buffer.is_empty());
        assert!(app.openclaw_daily_memory_search_query.is_empty());
        assert!(app.openclaw_daily_memory_search_results.is_empty());
        assert_eq!(app.daily_memory_idx, 0);
    }

    #[test]
    #[serial(home_settings)]
    fn set_visible_apps_keeps_state_unchanged_when_replacement_preload_fails() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        let initial_visible_apps = crate::settings::VisibleApps {
            claude: true,
            codex: true,
            gemini: false,
            opencode: true,
            openclaw: true,
        };
        crate::settings::set_visible_apps(initial_visible_apps.clone())
            .expect("save initial visible apps");
        write_invalid_legacy_config(temp_home.path());

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.route_stack.push(Route::Config);
        let mut data = UiData::default();
        data.providers.current_id = "before".to_string();

        let err = run_action(
            &mut app,
            &mut data,
            Action::SetVisibleApps {
                apps: crate::settings::VisibleApps {
                    claude: true,
                    codex: false,
                    gemini: false,
                    opencode: false,
                    openclaw: false,
                },
            },
        )
        .expect_err("replacement preload should fail");

        assert!(
            !err.to_string().is_empty(),
            "error should explain the preload failure"
        );
        assert_eq!(crate::settings::get_visible_apps(), initial_visible_apps);
        assert_eq!(app.app_type, AppType::OpenClaw);
        assert_eq!(app.route, Route::ConfigOpenClawAgents);
        assert_eq!(app.route_stack, vec![Route::Config]);
        assert_eq!(data.providers.current_id, "before");
    }

    #[test]
    #[serial(home_settings)]
    fn set_visible_apps_does_not_reload_when_current_app_stays_visible() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        crate::settings::set_visible_apps(crate::settings::VisibleApps {
            claude: true,
            codex: true,
            gemini: false,
            opencode: true,
            openclaw: true,
        })
        .expect("save initial visible apps");
        write_invalid_legacy_config(temp_home.path());

        let next_visible_apps = crate::settings::VisibleApps {
            claude: true,
            codex: false,
            gemini: false,
            opencode: true,
            openclaw: false,
        };
        let mut app = App::new(Some(AppType::Claude));
        let mut data = UiData::default();
        data.providers.current_id = "sentinel-current-provider".to_string();

        run_action(
            &mut app,
            &mut data,
            Action::SetVisibleApps {
                apps: next_visible_apps.clone(),
            },
        )
        .expect("persist visible apps without reloading");

        assert_eq!(crate::settings::get_visible_apps(), next_visible_apps);
        assert_eq!(app.app_type, AppType::Claude);
        assert_eq!(data.providers.current_id, "sentinel-current-provider");
        assert!(matches!(
            app.toast.as_ref(),
            Some(toast)
                if toast.kind == super::super::app::ToastKind::Success
                    && toast.message == texts::tui_toast_visible_apps_saved()
        ));
    }

    #[test]
    #[serial(home_settings)]
    fn set_visible_apps_zero_selection_shows_warning_and_keeps_state_unchanged() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        let initial_visible_apps = crate::settings::VisibleApps {
            claude: true,
            codex: true,
            gemini: false,
            opencode: true,
            openclaw: true,
        };
        crate::settings::set_visible_apps(initial_visible_apps.clone())
            .expect("save initial visible apps");

        let mut app = App::new(Some(AppType::Codex));
        let mut data = UiData::default();
        data.providers.current_id = "before".to_string();

        run_action(
            &mut app,
            &mut data,
            Action::SetVisibleApps {
                apps: crate::settings::VisibleApps {
                    claude: false,
                    codex: false,
                    gemini: false,
                    opencode: false,
                    openclaw: false,
                },
            },
        )
        .expect("runtime should warn instead of erroring");

        assert_eq!(crate::settings::get_visible_apps(), initial_visible_apps);
        assert_eq!(app.app_type, AppType::Codex);
        assert_eq!(data.providers.current_id, "before");
        assert!(matches!(
            app.toast.as_ref(),
            Some(toast)
                if toast.kind == super::super::app::ToastKind::Warning
                    && toast.message == texts::tui_toast_visible_apps_zero_selection_warning()
        ));
    }

    #[test]
    #[serial(home_settings)]
    fn claude_provider_launch_temporary_dispatches_to_claude_runtime_handler() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        let mut app = App::new(Some(AppType::Claude));
        let mut data = UiData::default();

        run_action(
            &mut app,
            &mut data,
            Action::ProviderLaunchTemporary {
                id: "missing".to_string(),
            },
        )
        .expect("dispatch should stay inside the TUI");

        assert!(matches!(
            app.toast.as_ref(),
            Some(toast)
                if toast.kind == super::super::app::ToastKind::Error
                    && (toast.message.contains("missing")
                        || toast.message.contains("未找到选中的供应商"))
        ));
    }
}
