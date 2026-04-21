use super::*;

#[cfg(test)]
mod tests {
    use super::types::{McpEnvEditorField, McpEnvEntryEditorState};
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use serde_json::json;
    use serial_test::serial;
    use std::ffi::OsString;
    use std::path::Path;
    use tempfile::TempDir;

    use crate::cli::i18n::{texts, use_test_language, Language};
    use crate::cli::tui::data::ProviderRow;
    use crate::cli::tui::form::{McpEnvVarRow, TextInput};
    use crate::cli::tui::runtime_actions::handle_action;
    use crate::cli::tui::runtime_systems::RequestTracker;
    use crate::cli::tui::terminal::TuiTerminal;
    use crate::commands::workspace::{DailyMemoryFileInfo, DailyMemorySearchResult, ALLOWED_FILES};
    use crate::error::AppError;
    use crate::provider::Provider;
    use crate::settings::{get_settings, update_settings, AppSettings};
    use crate::test_support::{
        lock_test_home_and_settings, set_test_home_override, TestHomeSettingsLock,
    };

    struct EnvGuard {
        _lock: TestHomeSettingsLock,
        old_home: Option<OsString>,
        old_userprofile: Option<OsString>,
    }

    impl EnvGuard {
        fn set_home(home: &Path) -> Self {
            let lock = lock_test_home_and_settings();
            let old_home = std::env::var_os("HOME");
            let old_userprofile = std::env::var_os("USERPROFILE");
            std::env::set_var("HOME", home);
            std::env::set_var("USERPROFILE", home);
            set_test_home_override(Some(home));
            crate::settings::reload_test_settings();
            Self {
                _lock: lock,
                old_home,
                old_userprofile,
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
            set_test_home_override(self.old_home.as_deref().map(Path::new));
            crate::settings::reload_test_settings();
        }
    }

    struct SettingsGuard {
        previous: AppSettings,
    }

    impl SettingsGuard {
        fn with_openclaw_dir(path: &Path) -> Self {
            let previous = get_settings();
            let mut settings = AppSettings::default();
            settings.openclaw_config_dir = Some(path.display().to_string());
            update_settings(settings).expect("set openclaw override dir");
            Self { previous }
        }
    }

    impl Drop for SettingsGuard {
        fn drop(&mut self) {
            update_settings(self.previous.clone()).expect("restore previous settings");
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn data() -> UiData {
        UiData::default()
    }

    fn nav_index(app: &App, item: NavItem) -> usize {
        app.nav_items()
            .iter()
            .position(|candidate| *candidate == item)
            .expect("nav item should be visible for app")
    }

    fn workspace_row_index(row: OpenClawWorkspaceRow) -> usize {
        openclaw_workspace_rows()
            .iter()
            .position(|candidate| *candidate == row)
            .expect("workspace row should exist")
    }

    fn run_runtime_action(
        app: &mut App,
        data: &mut UiData,
        action: Action,
    ) -> Result<(), AppError> {
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

    fn openclaw_provider_row(id: &str, name: &str, models: &[(&str, &str)]) -> ProviderRow {
        let settings_config = json!({
            "models": models
                .iter()
                .map(|(model_id, model_name)| json!({ "id": model_id, "name": model_name }))
                .collect::<Vec<_>>()
        });

        ProviderRow {
            id: id.to_string(),
            provider: Provider::with_id(id.to_string(), name.to_string(), settings_config, None),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: models.first().map(|(model_id, _)| (*model_id).to_string()),
            default_model_id: None,
        }
    }

    fn openclaw_agents_runtime_form(
        defaults: Option<&crate::openclaw_config::OpenClawAgentsDefaults>,
        row: usize,
    ) -> OpenClawAgentsFormState {
        let mut form = OpenClawAgentsFormState::from_snapshot(defaults);
        form.section = OpenClawAgentsSection::Runtime;
        form.row = row;
        form
    }

    #[test]
    fn nav_menu_includes_skills_entry() {
        assert!(
            NavItem::ALL
                .iter()
                .any(|item| matches!(item, NavItem::Skills)),
            "Ratatui TUI nav should include a Skills entry"
        );
        assert!(matches!(
            NavItem::ALL[NavItem::ALL.len() - 1],
            NavItem::Exit
        ));
    }

    #[test]
    fn skills_nav_item_routes_to_skills_page() {
        assert_eq!(
            NavItem::Skills.to_route(),
            Some(Route::Skills),
            "Skills nav item should route to the Skills page"
        );
    }

    #[test]
    fn skills_i_requests_import_picker() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Skills;
        app.focus = Focus::Content;

        let action = app.on_key(key(KeyCode::Char('i')), &data());
        assert!(
            matches!(action, Action::SkillsOpenImport),
            "i in Skills page should open the import picker flow"
        );
    }

    #[test]
    fn skills_f_opens_discover_page() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Skills;
        app.focus = Focus::Content;

        let action = app.on_key(key(KeyCode::Char('f')), &data());
        assert!(
            matches!(action, Action::SwitchRoute(Route::SkillsDiscover)),
            "f in Skills page should navigate to Discover"
        );
    }

    #[test]
    fn skills_m_opens_apps_picker_overlay() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Skills;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.skills
            .installed
            .push(crate::services::skill::InstalledSkill {
                id: "local:hello-skill".to_string(),
                name: "Hello Skill".to_string(),
                description: None,
                directory: "hello-skill".to_string(),
                repo_owner: None,
                repo_name: None,
                repo_branch: None,
                readme_url: None,
                apps: crate::app_config::SkillApps::default(),
                installed_at: 0,
            });

        let action = app.on_key(key(KeyCode::Char('m')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::SkillsAppsPicker {
                directory,
                name,
                selected: 1,
                ..
            } if directory == "hello-skill" && name == "Hello Skill"
        ));
    }

    #[test]
    fn skills_apps_picker_x_toggles_selected_app_and_enter_emits_action() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Skills;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.skills
            .installed
            .push(crate::services::skill::InstalledSkill {
                id: "local:hello-skill".to_string(),
                name: "Hello Skill".to_string(),
                description: None,
                directory: "hello-skill".to_string(),
                repo_owner: None,
                repo_name: None,
                repo_branch: None,
                readme_url: None,
                apps: crate::app_config::SkillApps::default(),
                installed_at: 0,
            });

        app.on_key(key(KeyCode::Char('m')), &data);

        let action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::SkillsAppsPicker { apps, .. } if apps.codex
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            action,
            Action::SkillsSetApps { directory, apps }
                if directory == "hello-skill" && apps.codex && !apps.claude && !apps.gemini
        ));
    }

    #[test]
    fn skills_apps_picker_from_openclaw_targets_opencode_last_visible_row() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::Skills;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.skills
            .installed
            .push(crate::services::skill::InstalledSkill {
                id: "local:hello-skill".to_string(),
                name: "Hello Skill".to_string(),
                description: None,
                directory: "hello-skill".to_string(),
                repo_owner: None,
                repo_name: None,
                repo_branch: None,
                readme_url: None,
                apps: crate::app_config::SkillApps::default(),
                installed_at: 0,
            });

        let action = app.on_key(key(KeyCode::Char('m')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::SkillsAppsPicker { selected, .. } if *selected == 3
        ));

        let action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::SkillsAppsPicker { selected, apps, .. }
                if *selected == 3
                    && !apps.claude
                    && !apps.codex
                    && !apps.gemini
                    && apps.opencode
        ));
    }

    #[test]
    fn skills_d_opens_uninstall_confirm_from_list() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Skills;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.skills
            .installed
            .push(crate::services::skill::InstalledSkill {
                id: "local:hello-skill".to_string(),
                name: "Hello Skill".to_string(),
                description: None,
                directory: "hello-skill".to_string(),
                repo_owner: None,
                repo_name: None,
                repo_branch: None,
                readme_url: None,
                apps: crate::app_config::SkillApps::default(),
                installed_at: 0,
            });

        let action = app.on_key(key(KeyCode::Char('d')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::SkillsUninstall { directory },
                ..
            }) if directory == "hello-skill"
        ));
    }

    #[test]
    fn config_e_key_opens_common_snippet_picker_when_selected() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Config;
        app.focus = Focus::Content;
        app.config_idx = ConfigItem::ALL
            .iter()
            .position(|item| matches!(item, ConfigItem::CommonSnippet))
            .expect("CommonSnippet missing from ConfigItem::ALL");

        let action = app.on_key(key(KeyCode::Char('e')), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::CommonSnippetPicker { .. }));
    }

    #[test]
    #[serial(home_settings)]
    fn app_cycles_left_right() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        crate::settings::set_visible_apps(crate::settings::VisibleApps {
            claude: true,
            codex: true,
            gemini: true,
            opencode: true,
            openclaw: true,
        })
        .expect("save visible apps");
        let mut app = App::new(Some(AppType::Claude));
        assert!(matches!(
            app.on_key(key(KeyCode::Char(']')), &data()),
            Action::SetAppType(AppType::Codex)
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('[')), &data()),
            Action::SetAppType(AppType::OpenClaw)
        ));
    }

    #[test]
    #[serial(home_settings)]
    fn app_cycles_through_opencode() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        crate::settings::set_visible_apps(crate::settings::VisibleApps {
            claude: true,
            codex: true,
            gemini: true,
            opencode: true,
            openclaw: true,
        })
        .expect("save visible apps");
        let mut app = App::new(Some(AppType::Gemini));
        assert!(matches!(
            app.on_key(key(KeyCode::Char(']')), &data()),
            Action::SetAppType(AppType::OpenCode)
        ));

        let mut app = App::new(Some(AppType::OpenCode));
        assert!(matches!(
            app.on_key(key(KeyCode::Char(']')), &data()),
            Action::SetAppType(AppType::OpenClaw)
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('[')), &data()),
            Action::SetAppType(AppType::Gemini)
        ));

        let mut app = App::new(Some(AppType::OpenClaw));
        assert!(matches!(
            app.on_key(key(KeyCode::Char(']')), &data()),
            Action::SetAppType(AppType::Claude)
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('[')), &data()),
            Action::SetAppType(AppType::OpenCode)
        ));
    }

    #[test]
    #[serial(home_settings)]
    fn app_cycle_skips_hidden_apps_from_settings() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        crate::settings::set_visible_apps(crate::settings::VisibleApps {
            claude: true,
            codex: false,
            gemini: false,
            opencode: true,
            openclaw: true,
        })
        .expect("save visible apps");

        let mut app = App::new(Some(AppType::Claude));

        assert!(matches!(
            app.on_key(key(KeyCode::Char(']')), &data()),
            Action::SetAppType(AppType::OpenCode)
        ));
    }

    #[test]
    #[serial(home_settings)]
    fn app_cycle_noops_when_only_one_app_is_visible() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        crate::settings::set_visible_apps(crate::settings::VisibleApps {
            claude: false,
            codex: true,
            gemini: false,
            opencode: false,
            openclaw: false,
        })
        .expect("save visible apps");

        let mut app = App::new(Some(AppType::Codex));

        assert!(matches!(
            app.on_key(key(KeyCode::Char(']')), &data()),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('[')), &data()),
            Action::None
        ));
    }

    #[test]
    #[serial(home_settings)]
    fn app_cycle_backwards_skips_hidden_apps_and_wraps() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        crate::settings::set_visible_apps(crate::settings::VisibleApps {
            claude: true,
            codex: true,
            gemini: false,
            opencode: false,
            openclaw: true,
        })
        .expect("save visible apps");

        let mut app = App::new(Some(AppType::Claude));

        assert!(matches!(
            app.on_key(key(KeyCode::Char('[')), &data()),
            Action::SetAppType(AppType::OpenClaw)
        ));
    }

    #[test]
    #[serial(home_settings)]
    fn hidden_current_app_wraps_to_first_visible_replacement() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        crate::settings::set_visible_apps(crate::settings::VisibleApps {
            claude: true,
            codex: true,
            gemini: false,
            opencode: false,
            openclaw: false,
        })
        .expect("save visible apps");

        let mut app = App::new(Some(AppType::OpenClaw));

        assert!(matches!(
            app.on_key(key(KeyCode::Char(']')), &data()),
            Action::SetAppType(AppType::Claude)
        ));
    }

    #[test]
    fn proxy_activity_records_estimated_token_deltas() {
        let mut app = App::new(Some(AppType::Claude));

        app.reset_proxy_activity(40, 80);
        app.observe_proxy_token_activity(40, 80);
        app.observe_proxy_token_activity(52, 108);
        app.observe_proxy_token_activity(60, 124);

        assert_eq!(app.proxy_input_activity_samples, vec![0, 12, 8]);
        assert_eq!(app.proxy_output_activity_samples, vec![0, 28, 16]);
    }

    #[test]
    fn proxy_activity_resets_when_token_counter_moves_backwards() {
        let mut app = App::new(Some(AppType::Claude));

        app.reset_proxy_activity(10, 20);
        app.observe_proxy_token_activity(16, 36);
        app.observe_proxy_token_activity(3, 8);

        assert_eq!(app.proxy_input_activity_samples, vec![0]);
        assert_eq!(app.proxy_output_activity_samples, vec![0]);
        assert_eq!(app.proxy_activity_last_input_tokens, Some(3));
        assert_eq!(app.proxy_activity_last_output_tokens, Some(8));
    }

    #[test]
    fn proxy_transition_starts_when_proxy_route_state_changes() {
        let mut app = App::new(Some(AppType::Claude));

        let off = UiData::default();
        app.observe_proxy_visual_state(&off);
        assert_eq!(app.proxy_visual_state, Some(false));
        assert!(app.proxy_visual_transition.is_none());

        let mut on = UiData::default();
        on.proxy.running = true;
        on.proxy.claude_takeover = true;

        app.observe_proxy_visual_state(&on);

        assert_eq!(app.proxy_visual_state, Some(true));
        assert!(app.proxy_visual_transition.is_some());
    }

    #[test]
    fn proxy_transition_expires_after_duration() {
        let mut app = App::new(Some(AppType::Claude));

        let off = UiData::default();
        app.observe_proxy_visual_state(&off);

        let mut on = UiData::default();
        on.proxy.running = true;
        on.proxy.claude_takeover = true;
        app.observe_proxy_visual_state(&on);
        assert!(app.proxy_visual_transition.is_some());

        for _ in 0..PROXY_HERO_TRANSITION_TICKS {
            app.on_tick();
        }

        assert!(app.proxy_visual_transition.is_none());
    }

    #[test]
    fn proxy_transition_stays_active_long_enough_for_flash_return_phase() {
        let mut app = App::new(Some(AppType::Claude));

        let off = UiData::default();
        app.observe_proxy_visual_state(&off);

        let mut on = UiData::default();
        on.proxy.running = true;
        on.proxy.claude_takeover = true;
        app.observe_proxy_visual_state(&on);

        for _ in 0..7 {
            app.on_tick();
        }

        assert!(app.proxy_visual_transition.is_some());
    }

    #[test]
    fn proxy_transition_does_not_start_when_switching_to_an_already_running_proxy_app() {
        let mut app = App::new(Some(AppType::Codex));

        let mut shared_runtime = UiData::default();
        shared_runtime.proxy.running = true;
        shared_runtime.proxy.managed_runtime = true;
        shared_runtime.proxy.claude_takeover = true;

        app.observe_proxy_visual_state(&shared_runtime);
        assert_eq!(app.proxy_visual_state, Some(true));
        assert!(app.proxy_visual_transition.is_none());

        app.app_type = AppType::Claude;
        app.observe_proxy_visual_state(&shared_runtime);

        assert_eq!(app.proxy_visual_state, Some(true));
        assert!(
            app.proxy_visual_transition.is_none(),
            "switching apps should not look like opening the proxy runtime"
        );
    }

    #[test]
    fn proxy_activity_poll_interval_stays_at_one_second_with_200ms_tick() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Main;

        app.tick = 4;
        assert!(!app.should_poll_proxy_activity());

        app.tick = 5;
        assert!(app.should_poll_proxy_activity());
    }

    #[test]
    fn q_from_main_opens_exit_confirm_overlay() {
        let mut app = App::new(Some(AppType::Claude));
        assert_eq!(app.route, Route::Main);
        app.on_key(key(KeyCode::Char('q')), &data());
        assert!(matches!(app.overlay, Overlay::Confirm(_)));
    }

    #[test]
    fn provider_add_form_notes_is_length_limited() {
        let mut app = App::new(Some(AppType::Claude));
        app.open_provider_add_form();

        let notes_idx = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form
                .fields()
                .iter()
                .position(|f| *f == ProviderAddField::Notes)
                .expect("Notes field should exist"),
            _ => panic!("provider form should be open"),
        };

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = FormFocus::Fields;
            form.field_idx = notes_idx;
            form.editing = false;
        }

        // Enter edit mode for Notes.
        app.on_key(key(KeyCode::Enter), &data());
        for _ in 0..(PROVIDER_NOTES_MAX_CHARS + 10) {
            app.on_key(key(KeyCode::Char('a')), &data());
        }

        let notes_len = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form.notes.value.chars().count(),
            _ => 0,
        };
        assert_eq!(notes_len, PROVIDER_NOTES_MAX_CHARS);
    }

    #[test]
    fn filter_mode_updates_buffer_and_exits() {
        let mut app = App::new(Some(AppType::Claude));
        assert_eq!(app.filter.active, false);
        app.on_key(key(KeyCode::Char('/')), &data());
        assert_eq!(app.filter.active, true);
        app.on_key(key(KeyCode::Char('a')), &data());
        app.on_key(key(KeyCode::Char('b')), &data());
        assert_eq!(app.filter.buffer, "ab");
        app.on_key(key(KeyCode::Backspace), &data());
        assert_eq!(app.filter.buffer, "a");
        app.on_key(key(KeyCode::Enter), &data());
        assert_eq!(app.filter.active, false);
    }

    #[test]
    fn tab_key_is_noop() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Nav;

        let data = UiData::default();
        let action = app.on_key(key(KeyCode::Tab), &data);
        assert!(matches!(action, Action::None));
        assert_eq!(app.focus, Focus::Nav);
    }

    #[test]
    fn provider_json_editor_hides_internal_fields() {
        let original = json!({
            "id": "p1",
            "name": "demo",
            "meta": {
                "applyCommonConfig": true,
                "custom_endpoints": {
                    "https://example.com": {
                        "url": "https://example.com"
                    }
                }
            },
            "icon": "openai",
            "iconColor": "#00A67E",
            "settingsConfig": {
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "secret-token",
                    "FOO": "bar"
                }
            },
            "createdAt": 123,
            "sortIndex": 9,
            "category": "demo",
            "inFailoverQueue": true
        });

        let display = super::super::form::strip_provider_internal_fields(&original);
        assert!(display.get("createdAt").is_none());
        assert!(display.get("meta").is_none());
        assert!(display.get("icon").is_none());
        assert!(display.get("iconColor").is_none());
        assert!(display.get("sortIndex").is_none());
        assert!(display.get("category").is_none());
        assert!(display.get("inFailoverQueue").is_none());
        assert_eq!(
            display["settingsConfig"]["env"]["ANTHROPIC_AUTH_TOKEN"],
            "secret-token"
        );
    }

    #[test]
    fn providers_enter_key_opens_detail() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: None,
            default_model_id: None,
        });

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            action,
            Action::SwitchRoute(Route::ProviderDetail { id }) if id == "p1"
        ));
    }

    #[test]
    fn providers_s_key_triggers_switch_action() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: None,
            default_model_id: None,
        });

        let action = app.on_key(key(KeyCode::Char('s')), &data);
        assert!(matches!(action, Action::ProviderSwitch { id } if id == "p1"));
    }

    #[test]
    fn providers_c_key_requests_stream_check() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com","ANTHROPIC_AUTH_TOKEN":"sk-demo"}}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: None,
            default_model_id: None,
        });

        let action = app.on_key(key(KeyCode::Char('c')), &data);
        assert!(matches!(action, Action::ProviderStreamCheck { id } if id == "p1"));
        assert!(
            matches!(app.overlay, Overlay::StreamCheckRunning { ref provider_name, .. } if provider_name == "Provider One")
        );
    }

    #[test]
    fn providers_c_key_is_noop_for_openclaw() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"apiKey":"sk-demo","baseUrl":"https://example.com"}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: false,
            is_saved: true,
            is_default_model: false,
            primary_model_id: Some("claude-sonnet-4".to_string()),
            default_model_id: None,
        });

        let action = app.on_key(key(KeyCode::Char('c')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn openclaw_providers_s_key_adds_or_removes_live_config_membership() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"apiKey":"sk-demo","baseUrl":"https://example.com"}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: false,
            is_saved: true,
            is_default_model: false,
            primary_model_id: Some("claude-sonnet-4".to_string()),
            default_model_id: None,
        });

        let add_action = app.on_key(key(KeyCode::Char('s')), &data);
        assert!(matches!(add_action, Action::ProviderSwitch { id } if id == "p1"));

        data.providers.rows[0].is_in_config = true;
        let remove_action = app.on_key(key(KeyCode::Char('s')), &data);
        assert!(matches!(remove_action, Action::ProviderRemoveFromConfig { id } if id == "p1"));
    }

    #[test]
    fn openclaw_providers_e_key_allows_editing_saved_only_provider() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "saved-only".to_string(),
            provider: crate::provider::Provider::with_id(
                "saved-only".to_string(),
                "Saved Only".to_string(),
                json!({"apiKey":"sk-demo","baseUrl":"https://example.com"}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: false,
            is_saved: true,
            is_default_model: false,
            primary_model_id: Some("saved-model".to_string()),
            default_model_id: None,
        });

        let action = app.on_key(key(KeyCode::Char('e')), &data);
        assert!(matches!(action, Action::None));
        assert!(
            app.form.is_some(),
            "saved-only provider should open edit form"
        );
        assert!(app.toast.is_none(), "saved-only edit should not be blocked");
    }

    #[test]
    fn openclaw_providers_x_key_sets_default_model_from_selected_provider() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"apiKey":"sk-demo","baseUrl":"https://example.com"}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: Some("claude-sonnet-4".to_string()),
            default_model_id: None,
        });

        let action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(
            action,
            Action::ProviderSetDefaultModel { provider_id, model_id }
                if provider_id == "p1" && model_id == "claude-sonnet-4"
        ));
    }

    #[test]
    fn openclaw_providers_s_key_allows_removing_fallback_only_default_provider() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p2".to_string(),
            provider: crate::provider::Provider::with_id(
                "p2".to_string(),
                "Provider Two".to_string(),
                json!({"apiKey":"sk-demo","baseUrl":"https://example.com"}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: Some("shared-model".to_string()),
            default_model_id: Some("shared-model".to_string()),
        });

        let action = app.on_key(key(KeyCode::Char('s')), &data);
        assert!(matches!(action, Action::ProviderRemoveFromConfig { id } if id == "p2"));
        assert!(app.toast.is_none());
    }

    #[test]
    fn openclaw_providers_s_key_blocks_removing_primary_default_provider() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"apiKey":"sk-demo","baseUrl":"https://example.com"}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: true,
            primary_model_id: Some("primary-model".to_string()),
            default_model_id: Some("primary-model".to_string()),
        });

        let action = app.on_key(key(KeyCode::Char('s')), &data);
        assert!(matches!(action, Action::None));
        assert!(app.toast.is_some());
    }

    #[test]
    fn openclaw_providers_x_key_promotes_fallback_only_provider_even_when_model_matches_primary() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p2".to_string(),
            provider: crate::provider::Provider::with_id(
                "p2".to_string(),
                "Provider Two".to_string(),
                json!({"apiKey":"sk-demo","baseUrl":"https://example.com"}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: Some("shared-model".to_string()),
            default_model_id: Some("shared-model".to_string()),
        });

        let action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(
            action,
            Action::ProviderSetDefaultModel { provider_id, model_id }
                if provider_id == "p2" && model_id == "shared-model"
        ));
    }

    #[test]
    fn openclaw_providers_d_key_allows_deleting_provider_referenced_by_default_model() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"apiKey":"sk-demo","baseUrl":"https://example.com"}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: Some("primary-model".to_string()),
            default_model_id: Some("fallback-model".to_string()),
        });

        let action = app.on_key(key(KeyCode::Char('d')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::ProviderDelete { id },
                ..
            }) if id == "p1"
        ));
        assert!(
            app.toast.is_none(),
            "should not show a blocking warning toast"
        );
    }

    #[test]
    fn openclaw_providers_x_key_can_reset_default_back_to_primary_model() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"apiKey":"sk-demo","baseUrl":"https://example.com"}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: true,
            primary_model_id: Some("primary-model".to_string()),
            default_model_id: Some("fallback-model".to_string()),
        });

        let action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(
            action,
            Action::ProviderSetDefaultModel { provider_id, model_id }
                if provider_id == "p1" && model_id == "primary-model"
        ));
    }

    #[test]
    fn openclaw_providers_x_key_reapplies_primary_default_to_rebuild_fallbacks() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"apiKey":"sk-demo","baseUrl":"https://example.com"}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: true,
            primary_model_id: Some("primary-model".to_string()),
            default_model_id: Some("primary-model".to_string()),
        });

        let action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(
            action,
            Action::ProviderSetDefaultModel { provider_id, model_id }
                if provider_id == "p1" && model_id == "primary-model"
        ));
    }

    #[test]
    fn provider_detail_c_key_requests_stream_check() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::ProviderDetail {
            id: "p1".to_string(),
        };
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com","ANTHROPIC_AUTH_TOKEN":"sk-demo"}}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: None,
            default_model_id: None,
        });

        let action = app.on_key(key(KeyCode::Char('c')), &data);
        assert!(matches!(action, Action::ProviderStreamCheck { id } if id == "p1"));
        assert!(
            matches!(app.overlay, Overlay::StreamCheckRunning { ref provider_name, .. } if provider_name == "Provider One")
        );
    }

    #[test]
    fn provider_detail_c_key_is_noop_for_openclaw() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ProviderDetail {
            id: "p1".to_string(),
        };
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"apiKey":"sk-demo","baseUrl":"https://example.com"}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: false,
            is_saved: true,
            is_default_model: false,
            primary_model_id: Some("claude-sonnet-4".to_string()),
            default_model_id: None,
        });

        let action = app.on_key(key(KeyCode::Char('c')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn openclaw_provider_detail_x_key_sets_default_model() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ProviderDetail {
            id: "p1".to_string(),
        };
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"apiKey":"sk-demo","baseUrl":"https://example.com"}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: Some("claude-sonnet-4".to_string()),
            default_model_id: None,
        });

        let action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(
            action,
            Action::ProviderSetDefaultModel { provider_id, model_id }
                if provider_id == "p1" && model_id == "claude-sonnet-4"
        ));
    }

    #[test]
    fn openclaw_provider_detail_e_key_allows_editing_saved_only_provider() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ProviderDetail {
            id: "saved-only".to_string(),
        };
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "saved-only".to_string(),
            provider: crate::provider::Provider::with_id(
                "saved-only".to_string(),
                "Saved Only".to_string(),
                json!({"apiKey":"sk-demo","baseUrl":"https://example.com"}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: false,
            is_saved: true,
            is_default_model: false,
            primary_model_id: Some("saved-model".to_string()),
            default_model_id: None,
        });

        let action = app.on_key(key(KeyCode::Char('e')), &data);
        assert!(matches!(action, Action::None));
        assert!(
            app.form.is_some(),
            "saved-only provider should open edit form"
        );
        assert!(app.toast.is_none(), "saved-only edit should not be blocked");
    }

    #[test]
    fn openclaw_provider_detail_x_key_can_reset_default_back_to_primary_model() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ProviderDetail {
            id: "p1".to_string(),
        };
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"apiKey":"sk-demo","baseUrl":"https://example.com"}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: true,
            primary_model_id: Some("primary-model".to_string()),
            default_model_id: Some("fallback-model".to_string()),
        });

        let action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(
            action,
            Action::ProviderSetDefaultModel { provider_id, model_id }
                if provider_id == "p1" && model_id == "primary-model"
        ));
    }

    #[test]
    fn openclaw_provider_detail_x_key_reapplies_primary_default_to_rebuild_fallbacks() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ProviderDetail {
            id: "p1".to_string(),
        };
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"apiKey":"sk-demo","baseUrl":"https://example.com"}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: true,
            primary_model_id: Some("primary-model".to_string()),
            default_model_id: Some("primary-model".to_string()),
        });

        let action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(
            action,
            Action::ProviderSetDefaultModel { provider_id, model_id }
                if provider_id == "p1" && model_id == "primary-model"
        ));
    }

    #[test]
    fn provider_switch_first_use_overlay_enter_requests_import() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.overlay = Overlay::ProviderSwitchFirstUseConfirm {
            provider_id: "p1".to_string(),
            title: texts::tui_provider_switch_first_use_title().to_string(),
            message: texts::tui_provider_switch_first_use_message("~/.claude/settings.json"),
            selected: 0,
        };

        let action = app.on_key(key(KeyCode::Enter), &data());

        assert!(matches!(action, Action::ProviderImportLiveConfig));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn provider_switch_first_use_overlay_right_then_enter_confirms_switch() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.overlay = Overlay::ProviderSwitchFirstUseConfirm {
            provider_id: "p1".to_string(),
            title: texts::tui_provider_switch_first_use_title().to_string(),
            message: texts::tui_provider_switch_first_use_message("~/.claude/settings.json"),
            selected: 0,
        };

        let move_action = app.on_key(key(KeyCode::Right), &data());
        assert!(matches!(move_action, Action::None));

        let action = app.on_key(key(KeyCode::Enter), &data());

        assert!(matches!(action, Action::ProviderSwitchForce { id } if id == "p1"));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn openclaw_provider_detail_s_key_allows_removing_fallback_only_default_provider() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ProviderDetail {
            id: "p2".to_string(),
        };
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p2".to_string(),
            provider: crate::provider::Provider::with_id(
                "p2".to_string(),
                "Provider Two".to_string(),
                json!({"apiKey":"sk-demo","baseUrl":"https://example.com"}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: Some("shared-model".to_string()),
            default_model_id: Some("shared-model".to_string()),
        });

        let action = app.on_key(key(KeyCode::Char('s')), &data);
        assert!(matches!(action, Action::ProviderRemoveFromConfig { id } if id == "p2"));
        assert!(app.toast.is_none());
    }

    #[test]
    fn openclaw_provider_detail_s_key_blocks_removing_primary_default_provider() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ProviderDetail {
            id: "p1".to_string(),
        };
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"apiKey":"sk-demo","baseUrl":"https://example.com"}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: true,
            primary_model_id: Some("primary-model".to_string()),
            default_model_id: Some("primary-model".to_string()),
        });

        let action = app.on_key(key(KeyCode::Char('s')), &data);
        assert!(matches!(action, Action::None));
        assert!(app.toast.is_some());
    }

    #[test]
    fn openclaw_provider_detail_x_key_promotes_fallback_only_provider_even_when_model_matches_primary(
    ) {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ProviderDetail {
            id: "p2".to_string(),
        };
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p2".to_string(),
            provider: crate::provider::Provider::with_id(
                "p2".to_string(),
                "Provider Two".to_string(),
                json!({"apiKey":"sk-demo","baseUrl":"https://example.com"}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: Some("shared-model".to_string()),
            default_model_id: Some("shared-model".to_string()),
        });

        let action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(
            action,
            Action::ProviderSetDefaultModel { provider_id, model_id }
                if provider_id == "p2" && model_id == "shared-model"
        ));
    }

    #[test]
    fn provider_detail_s_key_triggers_switch_action_and_enter_is_noop() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::ProviderDetail {
            id: "p1".to_string(),
        };
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: None,
            default_model_id: None,
        });

        let enter_action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(enter_action, Action::None));

        let action = app.on_key(key(KeyCode::Char('s')), &data);
        assert!(matches!(action, Action::ProviderSwitch { id } if id == "p1"));
    }

    #[test]
    fn mcp_x_key_toggles_current_app() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Mcp;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.mcp.rows.push(super::super::data::McpRow {
            id: "m1".to_string(),
            server: crate::app_config::McpServer {
                id: "m1".to_string(),
                name: "Server".to_string(),
                server: json!({}),
                apps: crate::app_config::McpApps::default(),
                description: None,
                homepage: None,
                docs: None,
                tags: vec![],
            },
        });

        let action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(
            action,
            Action::McpToggle {
                id,
                enabled: true
            } if id == "m1"
        ));
    }

    #[test]
    fn mcp_a_opens_add_form() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Mcp;
        app.focus = Focus::Content;

        let data = UiData::default();
        let action = app.on_key(key(KeyCode::Char('a')), &data);
        assert!(matches!(action, Action::None));
        assert!(
            app.editor.is_none(),
            "MCP 'a' should open the new add form (not the JSON editor)"
        );
    }

    #[test]
    fn mcp_v_does_nothing() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Mcp;
        app.focus = Focus::Content;

        let action = app.on_key(key(KeyCode::Char('v')), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn mcp_m_opens_apps_picker_overlay() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Mcp;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.mcp.rows.push(super::super::data::McpRow {
            id: "m1".to_string(),
            server: crate::app_config::McpServer {
                id: "m1".to_string(),
                name: "Server".to_string(),
                server: json!({}),
                apps: crate::app_config::McpApps::default(),
                description: None,
                homepage: None,
                docs: None,
                tags: vec![],
            },
        });

        let action = app.on_key(key(KeyCode::Char('m')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::McpAppsPicker {
                id,
                name,
                selected: 1,
                ..
            } if id == "m1" && name == "Server"
        ));
    }

    #[test]
    fn mcp_apps_picker_x_toggles_selected_app_and_enter_emits_action() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Mcp;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.mcp.rows.push(super::super::data::McpRow {
            id: "m1".to_string(),
            server: crate::app_config::McpServer {
                id: "m1".to_string(),
                name: "Server".to_string(),
                server: json!({}),
                apps: crate::app_config::McpApps::default(),
                description: None,
                homepage: None,
                docs: None,
                tags: vec![],
            },
        });

        app.on_key(key(KeyCode::Char('m')), &data);

        let action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::McpAppsPicker { apps, .. } if apps.codex
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            action,
            Action::McpSetApps { id, apps } if id == "m1" && apps.codex && !apps.claude && !apps.gemini
        ));
    }

    #[test]
    fn mcp_apps_picker_can_select_opencode() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Mcp;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.mcp.rows.push(super::super::data::McpRow {
            id: "m1".to_string(),
            server: crate::app_config::McpServer {
                id: "m1".to_string(),
                name: "Server".to_string(),
                server: json!({}),
                apps: crate::app_config::McpApps::default(),
                description: None,
                homepage: None,
                docs: None,
                tags: vec![],
            },
        });

        app.on_key(key(KeyCode::Char('m')), &data);
        app.on_key(key(KeyCode::Down), &data);
        app.on_key(key(KeyCode::Down), &data);
        app.on_key(key(KeyCode::Down), &data);

        let action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::McpAppsPicker { selected, apps, .. } if *selected == 3 && apps.opencode
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            action,
            Action::McpSetApps { id, apps }
                if id == "m1" && !apps.claude && !apps.codex && !apps.gemini && apps.opencode
        ));
    }

    #[test]
    fn mcp_apps_picker_from_openclaw_targets_opencode_last_visible_row() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::Mcp;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.mcp.rows.push(super::super::data::McpRow {
            id: "m1".to_string(),
            server: crate::app_config::McpServer {
                id: "m1".to_string(),
                name: "Server".to_string(),
                server: json!({}),
                apps: crate::app_config::McpApps::default(),
                description: None,
                homepage: None,
                docs: None,
                tags: vec![],
            },
        });

        let action = app.on_key(key(KeyCode::Char('m')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::McpAppsPicker { selected, .. } if *selected == 3
        ));

        let action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::McpAppsPicker { selected, apps, .. }
                if *selected == 3
                    && !apps.claude
                    && !apps.codex
                    && !apps.gemini
                    && apps.opencode
        ));
    }

    #[test]
    fn mcp_e_opens_edit_form() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Mcp;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.mcp.rows.push(super::super::data::McpRow {
            id: "m1".to_string(),
            server: crate::app_config::McpServer {
                id: "m1".to_string(),
                name: "Server".to_string(),
                server: json!({"command":"foo","args":[]}),
                apps: crate::app_config::McpApps::default(),
                description: None,
                homepage: None,
                docs: None,
                tags: vec![],
            },
        });

        let action = app.on_key(key(KeyCode::Char('e')), &data);
        assert!(matches!(action, Action::None));
        assert!(app.editor.is_none());
        assert!(app.form.is_some());
    }

    #[test]
    fn mcp_env_picker_enter_from_form() {
        let mut app = App::new(Some(AppType::Claude));
        let mut form = McpAddFormState::new();
        form.focus = FormFocus::Fields;
        form.field_idx = form
            .fields()
            .iter()
            .position(|field| *field == McpAddField::Env)
            .expect("Env field should exist");
        app.form = Some(FormState::McpAdd(form));

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());

        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::McpEnvPicker { selected: 0 }));
    }

    #[test]
    fn mcp_env_picker_add_edit_delete_flow() {
        let mut app = App::new(Some(AppType::Claude));
        let mut form = McpAddFormState::new();
        form.focus = FormFocus::Fields;
        form.env_rows.push(McpEnvVarRow {
            key: "API_KEY".to_string(),
            value: "old".to_string(),
        });
        app.form = Some(FormState::McpAdd(form));
        app.overlay = Overlay::McpEnvPicker { selected: 0 };

        app.on_key(key(KeyCode::Enter), &UiData::default());
        app.on_key(key(KeyCode::Tab), &UiData::default());
        app.on_key(key(KeyCode::Backspace), &UiData::default());
        app.on_key(key(KeyCode::Char('n')), &UiData::default());
        app.on_key(key(KeyCode::Char('e')), &UiData::default());
        app.on_key(key(KeyCode::Char('w')), &UiData::default());
        app.on_key(key(KeyCode::Enter), &UiData::default());
        app.on_key(key(KeyCode::Delete), &UiData::default());

        let FormState::McpAdd(form) = app.form.expect("mcp form should remain open") else {
            panic!("expected MCP form");
        };
        assert!(
            form.env_rows.is_empty(),
            "Delete should remove the only env row"
        );
    }

    #[test]
    fn mcp_env_picker_backspace_deletes_selected_row() {
        let mut app = App::new(Some(AppType::Claude));
        let mut form = McpAddFormState::new();
        form.focus = FormFocus::Fields;
        form.env_rows.push(McpEnvVarRow {
            key: "FIRST".to_string(),
            value: "one".to_string(),
        });
        form.env_rows.push(McpEnvVarRow {
            key: "SECOND".to_string(),
            value: "two".to_string(),
        });
        app.form = Some(FormState::McpAdd(form));
        app.overlay = Overlay::McpEnvPicker { selected: 1 };

        app.on_key(key(KeyCode::Backspace), &UiData::default());

        assert!(matches!(app.overlay, Overlay::McpEnvPicker { selected: 0 }));
        let FormState::McpAdd(form) = app.form.expect("mcp form should remain open") else {
            panic!("expected MCP form");
        };
        assert_eq!(
            form.env_rows.len(),
            1,
            "Backspace should delete selected env row"
        );
        assert_eq!(form.env_rows[0].key, "FIRST");
    }

    #[test]
    fn mcp_env_picker_add_save_keeps_selection_on_new_key() {
        let mut app = App::new(Some(AppType::Claude));
        let mut form = McpAddFormState::new();
        form.focus = FormFocus::Fields;
        form.env_rows.push(McpEnvVarRow {
            key: "A_KEY".to_string(),
            value: "a".to_string(),
        });
        app.form = Some(FormState::McpAdd(form));
        app.overlay = Overlay::McpEnvPicker { selected: 0 };

        app.on_key(key(KeyCode::Char('a')), &UiData::default());
        for c in ['B', '_', 'K', 'E', 'Y'] {
            app.on_key(key(KeyCode::Char(c)), &UiData::default());
        }
        app.on_key(key(KeyCode::Tab), &UiData::default());
        for c in ['v', 'a', 'l'] {
            app.on_key(key(KeyCode::Char(c)), &UiData::default());
        }
        app.on_key(key(KeyCode::Enter), &UiData::default());

        let selected = match &app.overlay {
            Overlay::McpEnvPicker { selected } => *selected,
            other => panic!("expected MCP env picker, got {other:?}"),
        };
        let form = match app.form.as_ref() {
            Some(FormState::McpAdd(form)) => form,
            other => panic!("expected MCP form, got {other:?}"),
        };
        let new_idx = form
            .env_rows
            .iter()
            .position(|row| row.key == "B_KEY" && row.value == "val")
            .expect("new env row should be saved");
        assert_eq!(selected, new_idx);
    }

    #[test]
    fn mcp_env_editor_rejects_blank_and_duplicate_keys() {
        let mut app = App::new(Some(AppType::Claude));
        let mut form = McpAddFormState::new();
        form.env_rows.push(McpEnvVarRow {
            key: "API_KEY".to_string(),
            value: "secret".to_string(),
        });
        app.form = Some(FormState::McpAdd(form));
        app.overlay = Overlay::McpEnvEntryEditor(McpEnvEntryEditorState {
            row: None,
            return_selected: 0,
            field: McpEnvEditorField::Key,
            key: TextInput::new(""),
            value: TextInput::new(""),
        });

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::McpEnvEntryEditor(_)));

        if let Overlay::McpEnvEntryEditor(editor) = &mut app.overlay {
            editor.key.set("API_KEY");
        }

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::McpEnvEntryEditor(_)));
    }

    #[test]
    fn mcp_env_editor_rejects_duplicate_key_when_existing_has_whitespace() {
        let mut app = App::new(Some(AppType::Claude));
        let mut form = McpAddFormState::new();
        form.env_rows.push(McpEnvVarRow {
            key: " KEY".to_string(),
            value: "secret".to_string(),
        });
        app.form = Some(FormState::McpAdd(form));
        app.overlay = Overlay::McpEnvEntryEditor(McpEnvEntryEditorState {
            row: None,
            return_selected: 0,
            field: McpEnvEditorField::Key,
            key: TextInput::new("KEY"),
            value: TextInput::new("new"),
        });

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::McpEnvEntryEditor(_)));

        let form = match app.form.as_ref() {
            Some(FormState::McpAdd(form)) => form,
            other => panic!("expected MCP form, got {other:?}"),
        };
        assert_eq!(form.env_rows.len(), 1, "duplicate should not be inserted");
    }

    #[test]
    fn mcp_env_picker_edit_reorder_keeps_selection_on_edited_row() {
        let mut app = App::new(Some(AppType::Claude));
        let mut form = McpAddFormState::new();
        form.env_rows.push(McpEnvVarRow {
            key: "A_KEY".to_string(),
            value: "a".to_string(),
        });
        form.env_rows.push(McpEnvVarRow {
            key: "Z_KEY".to_string(),
            value: "z".to_string(),
        });
        app.form = Some(FormState::McpAdd(form));
        app.overlay = Overlay::McpEnvPicker { selected: 1 };

        app.on_key(key(KeyCode::Enter), &UiData::default());
        if let Overlay::McpEnvEntryEditor(editor) = &mut app.overlay {
            editor.key.set("0_KEY");
        }
        app.on_key(key(KeyCode::Enter), &UiData::default());

        assert!(matches!(app.overlay, Overlay::McpEnvPicker { selected: 0 }));

        app.on_key(key(KeyCode::Delete), &UiData::default());
        let FormState::McpAdd(form) = app.form.expect("mcp form should remain open") else {
            panic!("expected MCP form");
        };
        assert_eq!(form.env_rows.len(), 1);
        assert_eq!(form.env_rows[0].key, "A_KEY");
    }

    #[test]
    fn mcp_env_editor_esc_restores_previous_picker_selection() {
        let mut app = App::new(Some(AppType::Claude));
        let mut form = McpAddFormState::new();
        form.env_rows.push(McpEnvVarRow {
            key: "A_KEY".to_string(),
            value: "a".to_string(),
        });
        form.env_rows.push(McpEnvVarRow {
            key: "B_KEY".to_string(),
            value: "b".to_string(),
        });
        app.form = Some(FormState::McpAdd(form));
        app.overlay = Overlay::McpEnvPicker { selected: 1 };

        app.on_key(key(KeyCode::Char('a')), &UiData::default());
        assert!(matches!(app.overlay, Overlay::McpEnvEntryEditor(_)));

        app.on_key(key(KeyCode::Esc), &UiData::default());
        assert!(matches!(app.overlay, Overlay::McpEnvPicker { selected: 1 }));
    }

    #[test]
    fn mcp_env_editor_esc_without_form_closes_overlay() {
        let mut app = App::new(Some(AppType::Claude));
        app.form = None;
        app.overlay = Overlay::McpEnvEntryEditor(McpEnvEntryEditorState {
            row: None,
            return_selected: 0,
            field: McpEnvEditorField::Key,
            key: TextInput::new("K"),
            value: TextInput::new("V"),
        });

        let action = app.on_key(key(KeyCode::Esc), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn prompts_a_key_triggers_activate_action() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.prompts.rows.push(super::super::data::PromptRow {
            id: "pr1".to_string(),
            prompt: crate::prompt::Prompt {
                id: "pr1".to_string(),
                name: "My Prompt".to_string(),
                content: "Hello".to_string(),
                description: None,
                enabled: false,
                created_at: None,
                updated_at: None,
            },
        });

        let action = app.on_key(key(KeyCode::Char('a')), &data);
        assert!(matches!(action, Action::PromptActivate { id } if id == "pr1"));
    }

    #[test]
    fn back_from_provider_detail_returns_to_providers() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: None,
            default_model_id: None,
        });

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::SwitchRoute(Route::ProviderDetail { .. })
        ));
        assert!(matches!(app.route, Route::ProviderDetail { .. }));

        assert!(matches!(
            app.on_key(key(KeyCode::Esc), &data),
            Action::SwitchRoute(Route::Providers)
        ));
        assert_eq!(app.route, Route::Providers);
    }

    #[test]
    fn config_common_snippet_picker_and_view_support_edit_clear_apply_actions() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Config;
        app.focus = Focus::Content;
        app.config_idx = ConfigItem::ALL
            .iter()
            .position(|item| matches!(item, ConfigItem::CommonSnippet))
            .expect("CommonSnippet missing from ConfigItem::ALL");

        let data = UiData::default();
        app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(app.overlay, Overlay::CommonSnippetPicker { .. }));

        // Picker default should be the current app type (Claude). Enter opens the preview overlay.
        app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            app.overlay,
            Overlay::CommonSnippetView {
                app_type: AppType::Claude,
                ..
            }
        ));

        assert!(matches!(
            app.on_key(key(KeyCode::Char('a')), &data),
            Action::ConfigCommonSnippetApply {
                app_type: AppType::Claude
            }
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('c')), &data),
            Action::ConfigCommonSnippetClear {
                app_type: AppType::Claude
            }
        ));

        let action = app.on_key(key(KeyCode::Char('e')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.editor.as_ref().map(|e| e.kind),
            Some(EditorKind::Json)
        ));
    }

    #[test]
    fn config_common_snippet_picker_shows_snippet_for_non_current_app() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Config;
        app.focus = Focus::Content;
        app.config_idx = ConfigItem::ALL
            .iter()
            .position(|item| matches!(item, ConfigItem::CommonSnippet))
            .expect("CommonSnippet missing from ConfigItem::ALL");

        let mut data = UiData::default();
        data.config.common_snippets.codex = Some("disable_response_storage = true".to_string());

        app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(app.overlay, Overlay::CommonSnippetPicker { .. }));

        app.on_key(key(KeyCode::Down), &data); // Claude -> Codex
        app.on_key(key(KeyCode::Enter), &data);

        let snippet = match &app.overlay {
            Overlay::CommonSnippetView {
                app_type: AppType::Codex,
                view,
            } => view.lines.join("\n"),
            other => panic!("expected Codex snippet view, got {other:?}"),
        };
        assert!(
            snippet.contains("disable_response_storage"),
            "expected Codex snippet content to be loaded from snapshot"
        );
    }

    #[test]
    fn provider_add_form_codex_tab_cycles_fields_auth_config_templates() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields

        app.on_key(key(KeyCode::Tab), &data); // fields -> auth preview
        let (focus, section) = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => (form.focus, form.codex_preview_section),
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(focus, super::super::form::FormFocus::JsonPreview);
        assert_eq!(section, super::super::form::CodexPreviewSection::Auth);

        app.on_key(key(KeyCode::Tab), &data); // auth preview -> config preview
        let (focus, section) = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => (form.focus, form.codex_preview_section),
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(focus, super::super::form::FormFocus::JsonPreview);
        assert_eq!(section, super::super::form::CodexPreviewSection::Config);

        app.on_key(key(KeyCode::Tab), &data); // config preview -> templates
        let focus = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form.focus,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(focus, super::super::form::FormFocus::Templates);
    }

    #[test]
    fn provider_add_form_codex_preview_left_right_do_not_switch_panes() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields
        app.on_key(key(KeyCode::Tab), &data); // fields -> auth preview

        app.on_key(key(KeyCode::Right), &data);
        let section = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form.codex_preview_section,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(section, super::super::form::CodexPreviewSection::Auth);

        app.on_key(key(KeyCode::Left), &data);
        let section = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form.codex_preview_section,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(section, super::super::form::CodexPreviewSection::Auth);
    }

    #[test]
    fn provider_add_form_common_snippet_row_opens_editor_claude() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.editing = false;
            let fields = form.fields();
            form.field_idx = fields
                .iter()
                .position(|f| *f == ProviderAddField::CommonSnippet)
                .expect("CommonSnippet field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            app.editor.as_ref().map(|e| (&e.kind, &e.submit)),
            Some((
                EditorKind::Json,
                EditorSubmit::ConfigCommonSnippet {
                    app_type: AppType::Claude
                }
            ))
        ));
    }

    #[test]
    fn provider_add_form_common_snippet_row_opens_editor_codex() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.editing = false;
            let fields = form.fields();
            form.field_idx = fields
                .iter()
                .position(|f| *f == ProviderAddField::CommonSnippet)
                .expect("CommonSnippet field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            app.editor.as_ref().map(|e| (&e.kind, &e.submit)),
            Some((
                EditorKind::Plain,
                EditorSubmit::ConfigCommonSnippet {
                    app_type: AppType::Codex
                }
            ))
        ));
    }

    #[test]
    fn provider_add_form_common_snippet_row_opens_editor_gemini() {
        let mut app = App::new(Some(AppType::Gemini));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.editing = false;
            let fields = form.fields();
            form.field_idx = fields
                .iter()
                .position(|f| *f == ProviderAddField::CommonSnippet)
                .expect("CommonSnippet field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            app.editor.as_ref().map(|e| (&e.kind, &e.submit)),
            Some((
                EditorKind::Json,
                EditorSubmit::ConfigCommonSnippet {
                    app_type: AppType::Gemini
                }
            ))
        ));
    }

    #[test]
    fn provider_add_form_codex_preview_enter_opens_auth_editor() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields
        app.on_key(key(KeyCode::Tab), &data); // fields -> preview

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.editor.as_ref().map(|e| (&e.kind, &e.submit)),
            Some((EditorKind::Json, EditorSubmit::ProviderFormApplyCodexAuth))
        ));
    }

    #[test]
    fn provider_add_form_openclaw_models_enter_opens_models_editor() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.editing = false;
            let fields = form.fields();
            form.field_idx = fields
                .iter()
                .position(|f| *f == ProviderAddField::OpenClawModels)
                .expect("OpenClawModels field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.editor.as_ref().map(|e| (&e.kind, &e.submit)),
            Some((
                EditorKind::Json,
                EditorSubmit::ProviderFormApplyOpenClawModels
            ))
        ));
    }

    #[test]
    fn provider_add_form_openclaw_models_editor_ctrl_s_applies_models_array_back_to_form() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.editing = false;
            let fields = form.fields();
            form.field_idx = fields
                .iter()
                .position(|f| *f == ProviderAddField::OpenClawModels)
                .expect("OpenClawModels field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        app.on_key(key(KeyCode::Enter), &data);
        let injected = r#"[
  {
    "id": "primary-model",
    "name": "Primary Model",
    "contextWindow": 128000,
    "providerHint": "reasoning"
  },
  {
    "id": "fallback-model",
    "name": "Fallback Model",
    "contextWindow": 64000
  }
]"#;
        if let Some(editor) = app.editor.as_mut() {
            editor.lines = injected.lines().map(|s| s.to_string()).collect();
            editor.cursor_row = 0;
            editor.cursor_col = 0;
            editor.scroll = 0;
        } else {
            panic!("expected editor to be open");
        }

        let submit = app.on_key(ctrl(KeyCode::Char('s')), &data);
        let Action::EditorSubmit { submit, content } = submit else {
            panic!("expected EditorSubmit action");
        };
        assert!(matches!(
            submit,
            EditorSubmit::ProviderFormApplyOpenClawModels
        ));

        let models_value: serde_json::Value =
            serde_json::from_str(&content).expect("valid json array");
        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            let mut provider_value = form.to_provider_json_value();
            let settings_value = provider_value
                .as_object_mut()
                .and_then(|obj| obj.get_mut("settingsConfig"))
                .expect("settingsConfig should exist");
            let settings_obj = settings_value
                .as_object_mut()
                .expect("settingsConfig should be object");
            settings_obj.insert("models".to_string(), models_value);
            form.apply_provider_json_value_to_fields(provider_value)
                .expect("apply should succeed");
        } else {
            panic!("expected ProviderAdd form");
        }
        app.editor = None;

        if let Some(FormState::ProviderAdd(form)) = app.form.as_ref() {
            let provider_value = form.to_provider_json_value();
            let models = provider_value["settingsConfig"]["models"]
                .as_array()
                .expect("models should remain an array");
            assert_eq!(models.len(), 2);
            assert_eq!(models[0]["id"], "primary-model");
            assert_eq!(models[1]["id"], "fallback-model");
            assert_eq!(models[0]["providerHint"], "reasoning");
        } else {
            panic!("expected ProviderAdd form");
        }
    }

    #[test]
    fn provider_add_form_codex_preview_tab_then_enter_opens_config_editor() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields
        app.on_key(key(KeyCode::Tab), &data); // fields -> preview
        app.on_key(key(KeyCode::Tab), &data); // auth -> config

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.editor.as_ref().map(|e| (&e.kind, &e.submit)),
            Some((
                EditorKind::Plain,
                EditorSubmit::ProviderFormApplyCodexConfigToml
            ))
        ));
    }

    #[test]
    fn provider_add_form_codex_preview_c_does_not_open_common_snippet_view() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.common_snippet = "disable_response_storage = true".to_string();

        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields
        app.on_key(key(KeyCode::Tab), &data); // fields -> preview

        let action = app.on_key(key(KeyCode::Char('c')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        assert!(app.editor.is_none());
    }

    #[test]
    fn provider_add_form_codex_official_auth_enter_opens_editor() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Right), &data); // select OpenAI Official
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields
        app.on_key(key(KeyCode::Tab), &data); // fields -> preview

        app.on_key(key(KeyCode::Enter), &data); // try to edit auth
        assert!(app.editor.is_some());
        assert!(app.toast.is_none());
    }

    #[test]
    fn config_webdav_item_opens_second_level_menu() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Config;
        app.focus = Focus::Content;
        app.config_idx = visible_config_items(&app.filter, &app.app_type)
            .iter()
            .position(|item| matches!(item, ConfigItem::WebDavSync))
            .expect("WebDavSync should be visible in the filtered config menu");

        let data = UiData::default();
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::SwitchRoute(Route::ConfigWebDav)));
        assert!(matches!(app.route, Route::ConfigWebDav));
    }

    #[test]
    fn config_menu_hides_proxy_item_for_single_path_flow() {
        assert!(
            !ConfigItem::ALL
                .iter()
                .any(|item| matches!(item, ConfigItem::Proxy)),
            "Config menu should not expose a second proxy control entry"
        );
    }

    #[test]
    fn openclaw_config_menu_hides_workspace_env_tools_and_agents_items() {
        let app = App::new(Some(AppType::OpenClaw));
        let items = visible_config_items(&app.filter, &app.app_type);

        assert!(!items
            .iter()
            .any(|item| matches!(item, ConfigItem::OpenClawWorkspace)));
        assert!(!items
            .iter()
            .any(|item| matches!(item, ConfigItem::OpenClawEnv)));
        assert!(!items
            .iter()
            .any(|item| matches!(item, ConfigItem::OpenClawTools)));
        assert!(!items
            .iter()
            .any(|item| matches!(item, ConfigItem::OpenClawAgents)));
    }

    #[test]
    fn openclaw_config_item_metadata_keeps_visibility_label_route_and_title_aligned() {
        let cases = [
            (
                ConfigItem::OpenClawWorkspace,
                texts::tui_config_item_openclaw_workspace(),
                texts::tui_openclaw_workspace_title(),
                Route::ConfigOpenClawWorkspace,
            ),
            (
                ConfigItem::OpenClawEnv,
                texts::tui_config_item_openclaw_env(),
                texts::tui_openclaw_config_env_title(),
                Route::ConfigOpenClawEnv,
            ),
            (
                ConfigItem::OpenClawTools,
                texts::tui_config_item_openclaw_tools(),
                texts::tui_openclaw_config_tools_title(),
                Route::ConfigOpenClawTools,
            ),
            (
                ConfigItem::OpenClawAgents,
                texts::tui_config_item_openclaw_agents(),
                texts::tui_openclaw_config_agents_title(),
                Route::ConfigOpenClawAgents,
            ),
        ];

        for (item, label, detail_title, route) in cases {
            assert!(item.visible_for_app(&AppType::OpenClaw));
            assert!(!item.visible_for_app(&AppType::Claude));
            assert_eq!(item.label(), label);
            assert_eq!(item.detail_title(), Some(detail_title));
            assert!(matches!(item.detail_route(), Some(actual) if actual == route));
            assert!(
                matches!(ConfigItem::from_openclaw_route(&route), Some(actual) if actual == item)
            );
        }
    }

    #[test]
    fn non_openclaw_config_menu_hides_env_tools_and_agents_items() {
        let app = App::new(Some(AppType::Claude));
        let items = visible_config_items(&app.filter, &app.app_type);

        assert!(!items
            .iter()
            .any(|item| matches!(item, ConfigItem::OpenClawWorkspace)));
        assert!(!items
            .iter()
            .any(|item| matches!(item, ConfigItem::OpenClawEnv)));
        assert!(!items
            .iter()
            .any(|item| matches!(item, ConfigItem::OpenClawTools)));
        assert!(!items
            .iter()
            .any(|item| matches!(item, ConfigItem::OpenClawAgents)));
    }

    #[test]
    fn openclaw_nav_env_enter_opens_dedicated_subroute() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.focus = Focus::Nav;
        app.nav_idx = nav_index(&app, NavItem::OpenClawEnv);

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());

        assert!(matches!(
            action,
            Action::SwitchRoute(Route::ConfigOpenClawEnv)
        ));
        assert!(matches!(app.route, Route::ConfigOpenClawEnv));
        assert_eq!(app.route_stack, vec![Route::Main]);
    }

    #[test]
    fn openclaw_nav_workspace_enter_opens_dedicated_subroute() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.focus = Focus::Nav;
        app.nav_idx = nav_index(&app, NavItem::OpenClawWorkspace);

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());

        assert!(matches!(
            action,
            Action::SwitchRoute(Route::ConfigOpenClawWorkspace)
        ));
        assert!(matches!(app.route, Route::ConfigOpenClawWorkspace));
        assert_eq!(app.route_stack, vec![Route::Main]);
    }

    #[test]
    fn openclaw_nav_split_keeps_non_openclaw_generic_routes() {
        let cases = [
            (NavItem::Mcp, Route::Mcp),
            (NavItem::Skills, Route::Skills),
            (NavItem::Prompts, Route::Prompts),
            (NavItem::Config, Route::Config),
        ];

        for (nav_item, expected_route) in cases {
            let mut app = App::new(Some(AppType::Claude));
            app.focus = Focus::Nav;
            app.nav_idx = nav_index(&app, nav_item);

            let action = app.on_key(key(KeyCode::Enter), &UiData::default());

            assert!(matches!(
                action,
                Action::SwitchRoute(actual) if actual == expected_route
            ));
            assert_eq!(app.route, expected_route);
            assert_eq!(app.route_stack, vec![Route::Main]);
        }
    }

    #[test]
    fn openclaw_workspace_route_enter_opens_workspace_file() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawWorkspace;
        app.focus = Focus::Content;
        app.workspace_idx = workspace_row_index(OpenClawWorkspaceRow::File(ALLOWED_FILES[0]));

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());

        assert!(matches!(
            action,
            Action::OpenClawWorkspaceOpenFile { filename }
                if filename == ALLOWED_FILES[0]
        ));
    }

    #[test]
    fn openclaw_workspace_route_enter_on_daily_memory_row_from_nav_opens_daily_memory_route() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawWorkspace;
        app.route_stack = vec![Route::Main];
        app.focus = Focus::Content;
        app.workspace_idx = workspace_row_index(OpenClawWorkspaceRow::DailyMemory);

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());

        assert!(matches!(
            action,
            Action::SwitchRoute(Route::ConfigOpenClawDailyMemory)
        ));
        assert!(matches!(app.route, Route::ConfigOpenClawDailyMemory));
        assert_eq!(
            app.route_stack,
            vec![Route::Main, Route::ConfigOpenClawWorkspace]
        );
    }

    #[test]
    fn openclaw_daily_memory_entry_clears_stale_global_filter_state() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawWorkspace;
        app.route_stack = vec![Route::Config];
        app.focus = Focus::Content;
        app.workspace_idx = workspace_row_index(OpenClawWorkspaceRow::DailyMemory);
        app.filter.buffer = "workspace".to_string();
        app.openclaw_daily_memory_search_results = vec![DailyMemorySearchResult {
            filename: "2026-03-20.md".to_string(),
            date: "2026-03-20".to_string(),
            size_bytes: 12,
            modified_at: 1,
            snippet: "stale".to_string(),
            match_count: 1,
        }];

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());

        assert!(matches!(
            action,
            Action::SwitchRoute(Route::ConfigOpenClawDailyMemory)
        ));
        assert_eq!(app.route, Route::ConfigOpenClawDailyMemory);
        assert!(!app.filter.active);
        assert!(app.filter.buffer.is_empty());
        assert!(app.openclaw_daily_memory_search_results.is_empty());
    }

    #[test]
    fn openclaw_daily_memory_back_returns_to_workspace_route() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawDailyMemory;
        app.route_stack = vec![Route::Main, Route::ConfigOpenClawWorkspace];
        app.focus = Focus::Content;

        let action = app.on_key(key(KeyCode::Esc), &UiData::default());

        assert!(matches!(
            action,
            Action::SwitchRoute(Route::ConfigOpenClawWorkspace)
        ));
        assert_eq!(app.route, Route::ConfigOpenClawWorkspace);
        assert_eq!(app.focus, Focus::Content);
    }

    #[test]
    fn openclaw_daily_memory_exit_clears_route_local_search_state() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawDailyMemory;
        app.route_stack = vec![Route::Main, Route::ConfigOpenClawWorkspace];
        app.focus = Focus::Content;
        app.filter.buffer = "focus".to_string();
        app.openclaw_daily_memory_search_results = vec![DailyMemorySearchResult {
            filename: "2026-03-20.md".to_string(),
            date: "2026-03-20".to_string(),
            size_bytes: 12,
            modified_at: 1,
            snippet: "focus".to_string(),
            match_count: 1,
        }];

        let action = app.on_key(key(KeyCode::Esc), &UiData::default());

        assert!(matches!(
            action,
            Action::SwitchRoute(Route::ConfigOpenClawWorkspace)
        ));
        assert_eq!(app.route, Route::ConfigOpenClawWorkspace);
        assert!(!app.filter.active);
        assert!(app.filter.buffer.is_empty());
        assert!(app.openclaw_daily_memory_search_results.is_empty());
    }

    #[test]
    fn openclaw_daily_memory_create_prefills_today_filename() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawDailyMemory;
        app.focus = Focus::Content;
        let before = chrono::Local::now().format("%Y-%m-%d.md").to_string();

        let action = app.on_key(key(KeyCode::Char('a')), &UiData::default());

        let after = chrono::Local::now().format("%Y-%m-%d.md").to_string();

        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::TextInput(TextInputState { submit, buffer, .. })
                if *submit == TextSubmit::OpenClawDailyMemoryFilename
                    && (buffer == &before || buffer == &after)
        ));
    }

    #[test]
    fn openclaw_daily_memory_invalid_filename_is_rejected_before_editor_open() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawDailyMemory;
        app.focus = Focus::Content;
        app.overlay = Overlay::TextInput(TextInputState {
            title: texts::tui_openclaw_daily_memory_create_title().to_string(),
            prompt: texts::tui_openclaw_daily_memory_create_prompt().to_string(),
            buffer: "bad-name.md".to_string(),
            submit: TextSubmit::OpenClawDailyMemoryFilename,
            secret: false,
        });

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());

        assert!(matches!(action, Action::None));
        assert!(
            app.editor.is_none(),
            "invalid filename should not open the editor"
        );
        assert!(matches!(
            &app.overlay,
            Overlay::TextInput(TextInputState { buffer, .. }) if buffer == "bad-name.md"
        ));
        assert!(matches!(
            app.toast.as_ref(),
            Some(Toast {
                kind: ToastKind::Warning,
                ..
            })
        ));
    }

    #[test]
    fn openclaw_daily_memory_enter_dispatches_open_for_selected_file() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawDailyMemory;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_workspace.daily_memory_files = vec![DailyMemoryFileInfo {
            filename: "2026-03-20.md".to_string(),
            date: "2026-03-20".to_string(),
            size_bytes: 12,
            modified_at: 1,
            preview: "hello".to_string(),
        }];

        let action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(
            action,
            Action::OpenClawDailyMemoryOpenFile { filename }
                if filename == "2026-03-20.md"
        ));
    }

    #[test]
    fn openclaw_daily_memory_delete_requires_confirmation() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawDailyMemory;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_workspace.daily_memory_files = vec![DailyMemoryFileInfo {
            filename: "2026-03-20.md".to_string(),
            date: "2026-03-20".to_string(),
            size_bytes: 12,
            modified_at: 1,
            preview: "hello".to_string(),
        }];

        let action = app.on_key(key(KeyCode::Char('d')), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::OpenClawDailyMemoryDelete { filename },
                ..
            }) if filename == "2026-03-20.md"
        ));
    }

    #[test]
    fn openclaw_workspace_o_dispatches_open_workspace_directory() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawWorkspace;
        app.focus = Focus::Content;

        let action = app.on_key(key(KeyCode::Char('o')), &UiData::default());

        assert!(matches!(
            action,
            Action::OpenClawOpenDirectory { subdir } if subdir.is_empty()
        ));
    }

    #[test]
    fn openclaw_daily_memory_o_dispatches_open_memory_directory() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawDailyMemory;
        app.focus = Focus::Content;

        let action = app.on_key(key(KeyCode::Char('o')), &UiData::default());

        assert!(matches!(
            action,
            Action::OpenClawOpenDirectory { subdir } if subdir == "memory"
        ));
    }

    #[test]
    fn openclaw_daily_memory_search_edits_filter_buffer_while_filtering() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawDailyMemory;
        app.filter.active = true;

        let action = app.on_key(key(KeyCode::Char('m')), &UiData::default());

        assert!(matches!(action, Action::None));
        assert_eq!(app.filter.buffer, "m");
    }

    #[test]
    fn openclaw_daily_memory_search_dispatches_on_enter_after_filter_edits() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawDailyMemory;
        app.filter.active = true;
        app.filter.buffer = "focus".to_string();

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());

        assert!(matches!(
            action,
            Action::OpenClawDailyMemorySearch { query } if query == "focus"
        ));
        assert!(!app.filter.active);
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_workspace_open_missing_file_loads_empty_editor_state() {
        let temp_home = TempDir::new().expect("create temp home");
        let openclaw_dir = temp_home.path().join(".openclaw");
        std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawWorkspace;
        let mut data = UiData::load(&AppType::OpenClaw).expect("load openclaw ui data");

        run_runtime_action(
            &mut app,
            &mut data,
            Action::OpenClawWorkspaceOpenFile {
                filename: "AGENTS.md".to_string(),
            },
        )
        .expect("open workspace file action should succeed");

        let editor = app
            .editor
            .as_ref()
            .expect("missing workspace file should open an editor");
        assert!(matches!(editor.kind, EditorKind::Plain));
        assert_eq!(
            editor.submit,
            EditorSubmit::OpenClawWorkspaceFile {
                filename: "AGENTS.md".to_string()
            }
        );
        assert_eq!(editor.text(), "");
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_workspace_save_refreshes_existence_state_after_editor_submit() {
        let temp_home = TempDir::new().expect("create temp home");
        let openclaw_dir = temp_home.path().join(".openclaw");
        std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawWorkspace;
        let mut data = UiData::load(&AppType::OpenClaw).expect("load openclaw ui data");

        run_runtime_action(
            &mut app,
            &mut data,
            Action::EditorSubmit {
                submit: EditorSubmit::OpenClawWorkspaceFile {
                    filename: "AGENTS.md".to_string(),
                },
                content: "workspace body".to_string(),
            },
        )
        .expect("save workspace file");

        assert!(app.editor.is_none());
        assert_eq!(app.route, Route::ConfigOpenClawWorkspace);
        assert_eq!(
            data.config
                .openclaw_workspace
                .file_exists
                .get("AGENTS.md")
                .copied(),
            Some(true)
        );
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_daily_memory_open_existing_file_loads_editor_state() {
        let temp_home = TempDir::new().expect("create temp home");
        let openclaw_dir = temp_home.path().join(".openclaw");
        std::fs::create_dir_all(openclaw_dir.join("workspace/memory")).expect("create memory dir");
        std::fs::write(
            openclaw_dir.join("workspace/memory/2026-03-20.md"),
            "hello memory",
        )
        .expect("seed memory file");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawDailyMemory;
        let mut data = UiData::load(&AppType::OpenClaw).expect("load openclaw ui data");

        run_runtime_action(
            &mut app,
            &mut data,
            Action::OpenClawDailyMemoryOpenFile {
                filename: "2026-03-20.md".to_string(),
            },
        )
        .expect("open daily memory file");

        let editor = app
            .editor
            .as_ref()
            .expect("daily memory editor should open");
        assert_eq!(editor.text(), "hello memory");
        assert_eq!(
            editor.submit,
            EditorSubmit::OpenClawDailyMemoryFile {
                filename: "2026-03-20.md".to_string()
            }
        );
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_workspace_open_failure_is_localized() {
        let temp_home = TempDir::new().expect("create temp home");
        let openclaw_dir = temp_home.path().join(".openclaw");
        let workspace_dir = openclaw_dir.join("workspace");
        std::fs::create_dir_all(&workspace_dir).expect("create workspace dir");
        let target = temp_home.path().join("outside.md");
        std::fs::write(&target, "outside").expect("seed target file");
        std::os::unix::fs::symlink(&target, workspace_dir.join("AGENTS.md"))
            .expect("create workspace symlink");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawWorkspace;
        let mut data = UiData::default();

        let err = run_runtime_action(
            &mut app,
            &mut data,
            Action::OpenClawWorkspaceOpenFile {
                filename: "AGENTS.md".to_string(),
            },
        )
        .expect_err("workspace open should fail for symlinked file");

        assert_eq!(
            err.to_string(),
            texts::tui_openclaw_workspace_open_failed(
                "AGENTS.md",
                &format!(
                    "Refusing to read workspace file symlink: {}",
                    workspace_dir.join("AGENTS.md").display()
                )
            )
        );
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_daily_memory_create_existing_filename_reopens_existing_content() {
        let temp_home = TempDir::new().expect("create temp home");
        let openclaw_dir = temp_home.path().join(".openclaw");
        std::fs::create_dir_all(openclaw_dir.join("workspace/memory")).expect("create memory dir");
        std::fs::write(
            openclaw_dir.join("workspace/memory/2026-03-20.md"),
            "existing content",
        )
        .expect("seed memory file");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawDailyMemory;
        app.focus = Focus::Content;
        app.overlay = Overlay::TextInput(TextInputState {
            title: texts::tui_openclaw_daily_memory_create_title().to_string(),
            prompt: texts::tui_openclaw_daily_memory_create_prompt().to_string(),
            buffer: "2026-03-20.md".to_string(),
            submit: TextSubmit::OpenClawDailyMemoryFilename,
            secret: false,
        });
        let mut data = UiData::load(&AppType::OpenClaw).expect("load openclaw ui data");

        let action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(
            action,
            Action::OpenClawDailyMemoryOpenFile { ref filename } if filename == "2026-03-20.md"
        ));

        run_runtime_action(&mut app, &mut data, action).expect("open existing daily memory file");

        let editor = app
            .editor
            .as_ref()
            .expect("existing file should open in editor");
        assert_eq!(editor.text(), "existing content");
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_daily_memory_create_stale_snapshot_still_reopens_existing_content() {
        let temp_home = TempDir::new().expect("create temp home");
        let openclaw_dir = temp_home.path().join(".openclaw");
        std::fs::create_dir_all(openclaw_dir.join("workspace/memory")).expect("create memory dir");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawDailyMemory;
        app.focus = Focus::Content;
        app.overlay = Overlay::TextInput(TextInputState {
            title: texts::tui_openclaw_daily_memory_create_title().to_string(),
            prompt: texts::tui_openclaw_daily_memory_create_prompt().to_string(),
            buffer: "2026-03-20.md".to_string(),
            submit: TextSubmit::OpenClawDailyMemoryFilename,
            secret: false,
        });
        let mut data = UiData::load(&AppType::OpenClaw).expect("load openclaw ui data");

        std::fs::write(
            openclaw_dir.join("workspace/memory/2026-03-20.md"),
            "late content",
        )
        .expect("seed memory file after snapshot load");

        let action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(
            action,
            Action::OpenClawDailyMemoryOpenFile { ref filename } if filename == "2026-03-20.md"
        ));

        run_runtime_action(&mut app, &mut data, action)
            .expect("open existing daily memory file from stale snapshot");

        let editor = app
            .editor
            .as_ref()
            .expect("existing file should open in editor");
        assert_eq!(editor.text(), "late content");
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_daily_memory_save_failure_is_localized() {
        let temp_home = TempDir::new().expect("create temp home");
        let openclaw_dir = temp_home.path().join(".openclaw");
        let workspace_dir = openclaw_dir.join("workspace");
        std::fs::create_dir_all(&workspace_dir).expect("create workspace dir");
        let target = temp_home.path().join("memory-target");
        std::fs::create_dir_all(&target).expect("create symlink target dir");
        std::os::unix::fs::symlink(&target, workspace_dir.join("memory"))
            .expect("create memory dir symlink");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawDailyMemory;
        let mut data = UiData::default();

        let err = run_runtime_action(
            &mut app,
            &mut data,
            Action::EditorSubmit {
                submit: EditorSubmit::OpenClawDailyMemoryFile {
                    filename: "2026-03-20.md".to_string(),
                },
                content: "hello".to_string(),
            },
        )
        .expect_err("daily memory save should fail for symlinked memory dir");

        assert_eq!(
            err.to_string(),
            texts::tui_openclaw_daily_memory_save_failed(
                "2026-03-20.md",
                &format!(
                    "Refusing to use symlinked daily memory directory: {}",
                    workspace_dir.join("memory").display()
                )
            )
        );
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_daily_memory_save_refreshes_list_and_active_search_results() {
        let temp_home = TempDir::new().expect("create temp home");
        let openclaw_dir = temp_home.path().join(".openclaw");
        std::fs::create_dir_all(openclaw_dir.join("workspace/memory")).expect("create memory dir");
        std::fs::write(
            openclaw_dir.join("workspace/memory/2026-03-19.md"),
            "old note",
        )
        .expect("seed old memory file");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawDailyMemory;
        app.filter.buffer = "focus".to_string();
        app.openclaw_daily_memory_search_query = "focus".to_string();
        app.openclaw_daily_memory_search_results = vec![DailyMemorySearchResult {
            filename: "2026-03-19.md".to_string(),
            date: "2026-03-19".to_string(),
            size_bytes: 8,
            modified_at: 1,
            snippet: "old".to_string(),
            match_count: 1,
        }];
        let mut data = UiData::load(&AppType::OpenClaw).expect("load openclaw ui data");

        run_runtime_action(
            &mut app,
            &mut data,
            Action::EditorSubmit {
                submit: EditorSubmit::OpenClawDailyMemoryFile {
                    filename: "2026-03-20.md".to_string(),
                },
                content: "focus refreshed".to_string(),
            },
        )
        .expect("save daily memory file");

        assert!(app.editor.is_none());
        assert_eq!(app.route, Route::ConfigOpenClawDailyMemory);
        assert!(data
            .config
            .openclaw_workspace
            .daily_memory_files
            .iter()
            .any(|row| row.filename == "2026-03-20.md"));
        assert!(app
            .openclaw_daily_memory_search_results
            .iter()
            .any(|row| row.filename == "2026-03-20.md"));
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_daily_memory_delete_refreshes_search_results_and_keeps_nearest_row_selected() {
        let temp_home = TempDir::new().expect("create temp home");
        let openclaw_dir = temp_home.path().join(".openclaw");
        std::fs::create_dir_all(openclaw_dir.join("workspace/memory")).expect("create memory dir");
        std::fs::write(
            openclaw_dir.join("workspace/memory/2026-03-20.md"),
            "focus newest",
        )
        .expect("seed newest memory file");
        std::fs::write(
            openclaw_dir.join("workspace/memory/2026-03-19.md"),
            "focus previous",
        )
        .expect("seed previous memory file");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawDailyMemory;
        app.filter.buffer = "focus".to_string();
        app.openclaw_daily_memory_search_query = "focus".to_string();
        app.daily_memory_idx = 1;
        let mut data = UiData::load(&AppType::OpenClaw).expect("load openclaw ui data");
        app.openclaw_daily_memory_search_results = vec![
            DailyMemorySearchResult {
                filename: "2026-03-20.md".to_string(),
                date: "2026-03-20".to_string(),
                size_bytes: 12,
                modified_at: 2,
                snippet: "focus newest".to_string(),
                match_count: 1,
            },
            DailyMemorySearchResult {
                filename: "2026-03-19.md".to_string(),
                date: "2026-03-19".to_string(),
                size_bytes: 14,
                modified_at: 1,
                snippet: "focus previous".to_string(),
                match_count: 1,
            },
        ];

        run_runtime_action(
            &mut app,
            &mut data,
            Action::OpenClawDailyMemoryDelete {
                filename: "2026-03-19.md".to_string(),
            },
        )
        .expect("delete daily memory file");

        assert!(data
            .config
            .openclaw_workspace
            .daily_memory_files
            .iter()
            .all(|row| row.filename != "2026-03-19.md"));
        assert!(app
            .openclaw_daily_memory_search_results
            .iter()
            .all(|row| row.filename != "2026-03-19.md"));
        assert_eq!(app.daily_memory_idx, 0);
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_open_directory_failure_keeps_route_and_shows_feedback() {
        let temp_home = TempDir::new().expect("create temp home");
        let blocked_path = temp_home.path().join("blocked-openclaw");
        std::fs::write(&blocked_path, "not a directory").expect("seed blocking file");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&blocked_path);

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawDailyMemory;
        let mut data = UiData::default();

        run_runtime_action(
            &mut app,
            &mut data,
            Action::OpenClawOpenDirectory {
                subdir: "memory".to_string(),
            },
        )
        .expect("failed open-directory action should surface as toast, not hard error");

        assert_eq!(app.route, Route::ConfigOpenClawDailyMemory);
        assert!(matches!(
            app.toast.as_ref(),
            Some(Toast {
                kind: ToastKind::Error,
                ..
            })
        ));
    }

    #[test]
    fn openclaw_nav_tools_enter_opens_dedicated_subroute() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.focus = Focus::Nav;
        app.nav_idx = nav_index(&app, NavItem::OpenClawTools);

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());

        assert!(matches!(
            action,
            Action::SwitchRoute(Route::ConfigOpenClawTools)
        ));
        assert!(matches!(app.route, Route::ConfigOpenClawTools));
        assert_eq!(app.route_stack, vec![Route::Main]);
    }

    #[test]
    fn openclaw_nav_agents_enter_opens_dedicated_subroute() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.focus = Focus::Nav;
        app.nav_idx = nav_index(&app, NavItem::OpenClawAgents);

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());

        assert!(matches!(
            action,
            Action::SwitchRoute(Route::ConfigOpenClawAgents)
        ));
        assert!(matches!(app.route, Route::ConfigOpenClawAgents));
        assert_eq!(app.route_stack, vec![Route::Main]);
    }

    #[test]
    fn openclaw_provider_edit_form_uses_saved_name_not_live_display_name() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "shared-id".to_string(),
            provider: crate::provider::Provider::with_id(
                "shared-id".to_string(),
                "Saved Snapshot Name".to_string(),
                json!({
                    "api": "openai-completions",
                    "models": [
                        {"id": "live-model", "name": "Live Model Name"}
                    ]
                }),
                None,
            ),
            api_url: Some("https://live.example.com/v1".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: Some("live-model".to_string()),
            default_model_id: None,
        });

        let action = app.on_key(key(KeyCode::Char('e')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.form.as_ref(),
            Some(super::super::form::FormState::ProviderAdd(form))
                if form.name.value == "Saved Snapshot Name"
        ));
    }

    #[test]
    fn openclaw_config_route_env_enter_opens_env_editor() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawEnv;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_env = Some(crate::openclaw_config::OpenClawEnvConfig {
            vars: std::collections::HashMap::from([(
                "OPENCLAW_ENV_TOKEN".to_string(),
                json!("demo-token"),
            )]),
        });

        let action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.editor
                .as_ref()
                .map(|editor| (&editor.kind, &editor.submit)),
            Some((EditorKind::Json, EditorSubmit::ConfigOpenClawEnv))
        ));
        assert_eq!(
            app.editor.as_ref().map(|editor| editor.title.as_str()),
            Some(texts::tui_openclaw_config_env_editor_title())
        );
        assert!(app
            .editor
            .as_ref()
            .expect("env editor should open")
            .text()
            .contains("OPENCLAW_ENV_TOKEN"));
    }

    #[test]
    fn openclaw_tools_enter_on_unsupported_profile_opens_picker_without_changing_form() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
            profile: Some("unsupported-profile".to_string()),
            allow: vec!["Read".to_string()],
            deny: Vec::new(),
            extra: std::collections::HashMap::new(),
        });

        let action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(action, Action::None));
        assert!(app.overlay.is_active());
        let form = app
            .openclaw_tools_form
            .as_ref()
            .expect("tools form should be initialized");
        assert_eq!(form.profile.as_deref(), Some("unsupported-profile"));
        assert_eq!(form.allow, vec!["Read".to_string()]);
        assert!(form.deny.is_empty());
    }

    #[test]
    fn openclaw_tools_profile_enter_opens_picker_with_current_value_preselected() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
            profile: Some("minimal".to_string()),
            allow: vec!["Read".to_string()],
            deny: Vec::new(),
            extra: std::collections::HashMap::new(),
        });

        let open_action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(open_action, Action::None));
        assert!(app.overlay.is_active());

        let confirm_action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(confirm_action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        let form = app
            .openclaw_tools_form
            .as_ref()
            .expect("tools form should stay initialized");
        assert_eq!(form.profile.as_deref(), Some("minimal"));
        assert_eq!(form.allow, vec!["Read".to_string()]);
    }

    #[test]
    fn openclaw_tools_profile_e_shortcut_opens_picker_with_current_value_preselected() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
            profile: Some("minimal".to_string()),
            allow: vec!["Read".to_string()],
            deny: Vec::new(),
            extra: std::collections::HashMap::new(),
        });

        let action = app.on_key(key(KeyCode::Char('e')), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::OpenClawToolsProfilePicker { selected: Some(1) }
        ));
    }

    #[test]
    fn openclaw_tools_profile_picker_navigation_does_not_auto_submit() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
            profile: Some("minimal".to_string()),
            allow: vec!["Read".to_string()],
            deny: Vec::new(),
            extra: std::collections::HashMap::new(),
        });

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(app.overlay.is_active());

        let navigate_action = app.on_key(key(KeyCode::Down), &data);

        assert!(matches!(navigate_action, Action::None));
        assert!(app.overlay.is_active());
        let form = app
            .openclaw_tools_form
            .as_ref()
            .expect("tools form should stay initialized");
        assert_eq!(form.profile.as_deref(), Some("minimal"));
        assert_eq!(form.allow, vec!["Read".to_string()]);

        let confirm_action = app.on_key(key(KeyCode::Enter), &data);

        let Action::EditorSubmit { submit, content } = confirm_action else {
            panic!("expected picker confirmation to auto-submit, got {confirm_action:?}");
        };
        assert_eq!(submit, EditorSubmit::ConfigOpenClawTools);
        assert!(matches!(app.overlay, Overlay::None));

        let saved: crate::openclaw_config::OpenClawToolsConfig =
            serde_json::from_str(&content).expect("serialize tools form");
        assert_eq!(saved.profile.as_deref(), Some("coding"));
        assert_eq!(saved.allow, vec!["Read".to_string()]);
    }

    #[test]
    fn openclaw_tools_profile_picker_escape_leaves_form_unchanged() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
            profile: Some("full".to_string()),
            allow: vec!["Read".to_string()],
            deny: vec!["Exec".to_string()],
            extra: std::collections::HashMap::new(),
        });

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(app.overlay.is_active());

        let action = app.on_key(key(KeyCode::Esc), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        let form = app
            .openclaw_tools_form
            .as_ref()
            .expect("tools form should stay initialized");
        assert_eq!(form.profile.as_deref(), Some("full"));
        assert_eq!(form.allow, vec!["Read".to_string()]);
        assert_eq!(form.deny, vec!["Exec".to_string()]);
    }

    #[test]
    fn openclaw_tools_profile_picker_ctrl_s_is_ignored_while_overlay_is_open() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
            profile: Some("minimal".to_string()),
            allow: vec!["Read".to_string()],
            deny: vec!["Exec".to_string()],
            extra: std::collections::HashMap::new(),
        });

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));

        assert!(matches!(
            app.overlay,
            Overlay::OpenClawToolsProfilePicker { selected: Some(1) }
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.overlay,
            Overlay::OpenClawToolsProfilePicker { selected: Some(2) }
        ));

        let action = app.on_key(ctrl(KeyCode::Char('s')), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::OpenClawToolsProfilePicker { selected: Some(2) }
        ));
        let form = app
            .openclaw_tools_form
            .as_ref()
            .expect("tools form should stay initialized");
        assert_eq!(form.profile.as_deref(), Some("minimal"));
        assert_eq!(form.allow, vec!["Read".to_string()]);
        assert_eq!(form.deny, vec!["Exec".to_string()]);
    }

    #[test]
    fn openclaw_tools_profile_picker_unsupported_value_requires_explicit_selection_before_submit() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
            profile: Some("unsupported-profile".to_string()),
            allow: vec!["Read".to_string()],
            deny: Vec::new(),
            extra: std::collections::HashMap::new(),
        });

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(app.overlay.is_active());

        let no_op_action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(no_op_action, Action::None));
        assert!(app.overlay.is_active());
        let form = app
            .openclaw_tools_form
            .as_ref()
            .expect("tools form should stay initialized");
        assert_eq!(form.profile.as_deref(), Some("unsupported-profile"));
        assert_eq!(form.allow, vec!["Read".to_string()]);
        assert!(form.deny.is_empty());

        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));

        let confirm_action = app.on_key(key(KeyCode::Enter), &data);

        let Action::EditorSubmit { submit, content } = confirm_action else {
            panic!("expected picker confirmation to auto-submit, got {confirm_action:?}");
        };
        assert_eq!(submit, EditorSubmit::ConfigOpenClawTools);

        let saved: crate::openclaw_config::OpenClawToolsConfig =
            serde_json::from_str(&content).expect("serialize tools form");
        assert_eq!(saved.profile, None);
        assert_eq!(saved.allow, vec!["Read".to_string()]);
        assert!(saved.deny.is_empty());
    }

    #[test]
    fn openclaw_tools_profile_picker_full_to_unset_with_empty_rules_saves_silently() {
        let temp_home = TempDir::new().expect("create temp home");
        let openclaw_dir = temp_home.path().join(".openclaw");
        std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
            profile: Some("full".to_string()),
            allow: Vec::new(),
            deny: Vec::new(),
            extra: std::collections::HashMap::new(),
        });

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(app.overlay.is_active());
        for _ in 0..4 {
            assert!(matches!(app.on_key(key(KeyCode::Up), &data), Action::None));
        }

        let confirm_action = app.on_key(key(KeyCode::Enter), &data);

        let Action::EditorSubmit { submit, content } = confirm_action else {
            panic!("expected picker confirmation to auto-submit, got {confirm_action:?}");
        };
        assert_eq!(submit, EditorSubmit::ConfigOpenClawTools);

        let pending: crate::openclaw_config::OpenClawToolsConfig =
            serde_json::from_str(&content).expect("serialize tools form");
        assert_eq!(pending.profile, None);
        assert!(pending.allow.is_empty());
        assert!(pending.deny.is_empty());

        run_runtime_action(
            &mut app,
            &mut data,
            Action::EditorSubmit { submit, content },
        )
        .expect("picker-driven tools save should succeed");

        assert_eq!(
            data.config
                .openclaw_tools
                .as_ref()
                .and_then(|tools| tools.profile.as_deref()),
            None
        );
        assert!(
            app.toast.is_none(),
            "successful tools auto-save should stay silent"
        );
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_tools_profile_picker_confirm_save_failure_shows_error_toast() {
        let temp_home = TempDir::new().expect("create temp home");
        let blocked_path = temp_home.path().join("blocked-openclaw");
        std::fs::write(&blocked_path, "not a directory").expect("seed blocking file");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&blocked_path);

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
            profile: Some("minimal".to_string()),
            allow: Vec::new(),
            deny: Vec::new(),
            extra: std::collections::HashMap::new(),
        });

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);
        let Action::EditorSubmit { submit, content } = action else {
            panic!("expected picker confirmation to auto-submit, got {action:?}");
        };
        assert_eq!(submit, EditorSubmit::ConfigOpenClawTools);
        assert!(matches!(app.overlay, Overlay::None));

        let saved: crate::openclaw_config::OpenClawToolsConfig =
            serde_json::from_str(&content).expect("serialize tools form");
        assert_eq!(saved.profile.as_deref(), Some("coding"));

        run_runtime_action(
            &mut app,
            &mut data,
            Action::EditorSubmit { submit, content },
        )
        .expect("picker-confirm save failure should stay in-route and show a toast");

        assert_eq!(app.route, Route::ConfigOpenClawTools);
        assert!(matches!(app.overlay, Overlay::None));
        let form = app
            .openclaw_tools_form
            .as_ref()
            .expect("tools form should stay initialized after failed save");
        assert_eq!(form.profile.as_deref(), Some("coding"));
        let toast = app
            .toast
            .as_ref()
            .expect("save failure should show a toast");
        assert_eq!(toast.kind, ToastKind::Error);
        assert!(
            toast
                .message
                .starts_with(texts::tui_toast_openclaw_tools_save_result(false)),
            "{}",
            toast.message
        );
        assert!(
            toast.message.contains("blocked-openclaw"),
            "{}",
            toast.message
        );
    }

    #[test]
    fn openclaw_tools_rule_popup_accepts_global_hotkey_characters() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let data = UiData::default();
        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState { ref buffer, .. }) if buffer.is_empty()
        ));

        for ch in ['/', '?', '[', ']', 'q'] {
            assert!(matches!(
                app.on_key(key(KeyCode::Char(ch)), &data),
                Action::None
            ));
        }

        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState { ref buffer, .. }) if buffer == "/?[]q"
        ));
        assert_eq!(app.route, Route::ConfigOpenClawTools);
        assert_eq!(app.app_type, AppType::OpenClaw);
        assert!(!app.filter.active, "typing should not open filter mode");
    }

    #[test]
    fn openclaw_tools_profile_still_honors_global_help_hotkey() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let action = app.on_key(key(KeyCode::Char('?')), &UiData::default());

        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::Help));
        assert!(app.openclaw_tools_form.is_none());
    }

    #[test]
    fn openclaw_tools_profile_row_still_uses_vim_navigation() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let data = UiData::default();

        assert!(matches!(
            app.on_key(key(KeyCode::Char('j')), &data),
            Action::None
        ));
        let form = app
            .openclaw_tools_form
            .as_ref()
            .expect("tools form should be initialized");
        assert_eq!(form.section, OpenClawToolsSection::Allow);
        assert_eq!(form.row, 0);
    }

    #[test]
    fn openclaw_tools_rule_popup_accepts_hjkl_characters() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let data = UiData::default();
        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));

        for ch in ['h', 'j', 'k', 'l'] {
            assert!(matches!(
                app.on_key(key(KeyCode::Char(ch)), &data),
                Action::None
            ));
        }

        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState { ref buffer, .. }) if buffer == "hjkl"
        ));
    }

    #[test]
    fn openclaw_tools_enter_on_existing_and_add_rule_rows_opens_popup_editor() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
            profile: Some("coding".to_string()),
            allow: vec!["Read".to_string()],
            deny: Vec::new(),
            extra: std::collections::HashMap::new(),
        });

        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState { ref buffer, .. }) if buffer == "Read"
        ));

        assert!(matches!(app.on_key(key(KeyCode::Esc), &data), Action::None));
        assert!(matches!(app.overlay, Overlay::None));

        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState { ref buffer, .. }) if buffer.is_empty()
        ));
    }

    #[test]
    fn openclaw_tools_existing_rule_row_e_shortcut_opens_popup_editor() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
            profile: Some("coding".to_string()),
            allow: vec!["Read".to_string()],
            deny: Vec::new(),
            extra: std::collections::HashMap::new(),
        });

        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Char('e')), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState { ref buffer, .. }) if buffer == "Read"
        ));
    }

    #[test]
    fn openclaw_tools_add_rule_row_e_shortcut_opens_popup_editor() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
            profile: Some("coding".to_string()),
            allow: vec!["Read".to_string()],
            deny: Vec::new(),
            extra: std::collections::HashMap::new(),
        });

        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Char('e')), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState { ref buffer, .. }) if buffer.is_empty()
        ));
    }

    #[test]
    fn openclaw_tools_e_shortcut_matches_enter_on_non_text_rows() {
        let make_data = || {
            let mut data = UiData::default();
            data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
                profile: Some("coding".to_string()),
                allow: vec!["Read".to_string()],
                deny: vec!["Exec".to_string()],
                extra: std::collections::HashMap::new(),
            });
            data
        };
        let overlay_signature = |overlay: &Overlay| match overlay {
            Overlay::OpenClawToolsProfilePicker { selected } => {
                format!("profile-picker:{selected:?}")
            }
            Overlay::TextInput(TextInputState { buffer, .. }) => format!("text-input:{buffer}"),
            other => panic!("expected tools picker or popup editor, got {other:?}"),
        };

        for (down_presses, row_name) in [
            (0usize, "profile"),
            (1, "allow existing"),
            (2, "allow add"),
            (3, "deny existing"),
            (4, "deny add"),
        ] {
            let mut enter_app = App::new(Some(AppType::OpenClaw));
            enter_app.route = Route::ConfigOpenClawTools;
            enter_app.focus = Focus::Content;
            let enter_data = make_data();

            for _ in 0..down_presses {
                assert!(matches!(
                    enter_app.on_key(key(KeyCode::Down), &enter_data),
                    Action::None
                ));
            }

            let enter_action = enter_app.on_key(key(KeyCode::Enter), &enter_data);
            assert!(matches!(enter_action, Action::None));
            let enter_overlay = overlay_signature(&enter_app.overlay);

            let mut shortcut_app = App::new(Some(AppType::OpenClaw));
            shortcut_app.route = Route::ConfigOpenClawTools;
            shortcut_app.focus = Focus::Content;
            let shortcut_data = make_data();

            for _ in 0..down_presses {
                assert!(matches!(
                    shortcut_app.on_key(key(KeyCode::Down), &shortcut_data),
                    Action::None
                ));
            }

            let shortcut_action = shortcut_app.on_key(key(KeyCode::Char('e')), &shortcut_data);
            assert!(matches!(shortcut_action, Action::None));
            let shortcut_overlay = overlay_signature(&shortcut_app.overlay);

            assert_eq!(
                shortcut_overlay, enter_overlay,
                "e should mirror Enter on the {row_name} row"
            );
        }
    }

    #[test]
    fn openclaw_tools_existing_rule_popup_cancel_keeps_lists_unchanged() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
            profile: Some("coding".to_string()),
            allow: vec!["Read".to_string()],
            deny: vec!["Exec".to_string()],
            extra: std::collections::HashMap::new(),
        });

        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('W')), &data),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Esc), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        let form = app
            .openclaw_tools_form
            .as_ref()
            .expect("tools form should stay initialized");
        assert_eq!(form.allow, vec!["Read".to_string()]);
        assert_eq!(form.deny, vec!["Exec".to_string()]);
        assert_eq!(form.section, OpenClawToolsSection::Allow);
        assert_eq!(form.row, 0);
    }

    #[test]
    fn openclaw_tools_add_rule_popup_cancel_keeps_lists_unchanged() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
            profile: Some("coding".to_string()),
            allow: vec!["Read".to_string()],
            deny: vec!["Exec".to_string()],
            extra: std::collections::HashMap::new(),
        });

        for _ in 0..4 {
            assert!(matches!(
                app.on_key(key(KeyCode::Down), &data),
                Action::None
            ));
        }
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState { ref buffer, .. }) if buffer.is_empty()
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('G')), &data),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Esc), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        let form = app
            .openclaw_tools_form
            .as_ref()
            .expect("tools form should stay initialized");
        assert_eq!(form.allow, vec!["Read".to_string()]);
        assert_eq!(form.deny, vec!["Exec".to_string()]);
        assert_eq!(form.section, OpenClawToolsSection::Deny);
        assert_eq!(form.row, 1);
    }

    #[test]
    fn openclaw_tools_rule_popup_empty_confirm_is_rejected_without_saving() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let data = UiData::default();
        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState { ref buffer, .. }) if buffer.is_empty()
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::TextInput(_)));
        let form = app
            .openclaw_tools_form
            .as_ref()
            .expect("tools form should be initialized");
        assert!(form.allow.is_empty());
        assert_eq!(form.section, OpenClawToolsSection::Allow);
        assert_eq!(form.row, 0);
    }

    #[test]
    fn openclaw_tools_rule_popup_ctrl_s_does_not_type_or_submit() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let data = UiData::default();
        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));

        let action = app.on_key(ctrl(KeyCode::Char('s')), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState { ref buffer, .. }) if buffer.is_empty()
        ));
    }

    #[test]
    fn openclaw_tools_ctrl_s_on_allow_row_does_not_edit_or_submit() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
            profile: Some("coding".to_string()),
            allow: vec!["Read".to_string()],
            deny: Vec::new(),
            extra: std::collections::HashMap::new(),
        });

        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));

        let action = app.on_key(ctrl(KeyCode::Char('s')), &data);

        assert!(matches!(action, Action::None));
        let form = app
            .openclaw_tools_form
            .as_ref()
            .expect("tools form should be initialized");
        assert_eq!(form.section, OpenClawToolsSection::Allow);
        assert_eq!(form.allow, vec!["Read".to_string()]);
        assert!(app.toast.is_none());
    }

    #[test]
    fn openclaw_tools_navigation_stops_before_hidden_save_section() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
            profile: Some("coding".to_string()),
            allow: vec!["Read".to_string()],
            deny: vec!["Exec".to_string()],
            extra: std::collections::HashMap::new(),
        });

        for _ in 0..3 {
            assert!(matches!(
                app.on_key(key(KeyCode::Down), &data),
                Action::None
            ));
        }

        let form = app
            .openclaw_tools_form
            .as_ref()
            .expect("tools form should be initialized");
        assert_eq!(form.section, OpenClawToolsSection::Deny);
        assert_eq!(form.row, 0);
    }

    #[test]
    fn openclaw_agents_enter_opens_picker_without_opening_json_editor() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows = vec![openclaw_provider_row(
            "demo",
            "Demo Provider",
            &[("gpt-4.1", "GPT-4.1"), ("gpt-4o-mini", "GPT-4o Mini")],
        )];
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "demo/gpt-4.1".to_string(),
                    fallbacks: vec!["demo/gpt-4o-mini".to_string()],
                    extra: std::collections::HashMap::new(),
                }),
                models: None,
                extra: std::collections::HashMap::new(),
            });

        let action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(action, Action::None));
        assert!(app.editor.is_none());
        assert!(app.openclaw_agents_form.is_some());
        assert!(matches!(
            app.overlay,
            Overlay::OpenClawAgentsFallbackPicker { .. }
        ));
    }

    #[test]
    fn openclaw_agents_primary_model_enter_opens_picker_with_supported_value_preselected() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows = vec![openclaw_provider_row(
            "demo",
            "Demo Provider",
            &[
                ("model-a", "Model A"),
                ("model-b", "Model B"),
                ("model-c", "Model C"),
            ],
        )];
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "demo/model-b".to_string(),
                    fallbacks: Vec::new(),
                    extra: std::collections::HashMap::new(),
                }),
                models: None,
                extra: std::collections::HashMap::new(),
            });

        let action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::OpenClawAgentsFallbackPicker { .. }
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);

        let Action::EditorSubmit { submit, content } = action else {
            panic!("expected primary model picker confirmation to auto-submit, got {action:?}");
        };
        assert_eq!(submit, EditorSubmit::ConfigOpenClawAgents);
        assert!(matches!(app.overlay, Overlay::None));

        let saved: crate::openclaw_config::OpenClawAgentsDefaults =
            serde_json::from_str(&content).expect("serialize agents form");
        let model = saved
            .model
            .expect("model should be present after auto-save");
        assert_eq!(model.primary, "demo/model-c");
        assert!(model.fallbacks.is_empty());
    }

    #[test]
    fn openclaw_agents_primary_model_picker_requires_explicit_selection_for_unsupported_value() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows = vec![openclaw_provider_row(
            "demo",
            "Demo Provider",
            &[("model-a", "Model A"), ("model-b", "Model B")],
        )];
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "missing/off-catalog".to_string(),
                    fallbacks: Vec::new(),
                    extra: std::collections::HashMap::new(),
                }),
                models: None,
                extra: std::collections::HashMap::new(),
            });

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.overlay,
            Overlay::OpenClawAgentsFallbackPicker { .. }
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::OpenClawAgentsFallbackPicker { .. }
        ));
        let form = app
            .openclaw_agents_form
            .as_ref()
            .expect("agents form should stay initialized");
        assert_eq!(form.primary_model, "missing/off-catalog");
    }

    #[test]
    fn openclaw_agents_primary_model_picker_escape_leaves_value_unchanged() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows = vec![openclaw_provider_row(
            "demo",
            "Demo Provider",
            &[("model-a", "Model A"), ("model-b", "Model B")],
        )];
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "demo/model-b".to_string(),
                    fallbacks: Vec::new(),
                    extra: std::collections::HashMap::new(),
                }),
                models: None,
                extra: std::collections::HashMap::new(),
            });

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Esc), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        let form = app
            .openclaw_agents_form
            .as_ref()
            .expect("agents form should stay initialized");
        assert_eq!(form.primary_model, "demo/model-b");
    }

    #[test]
    fn openclaw_agents_delete_primary_model_auto_submits_cleared_value() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows = vec![openclaw_provider_row(
            "demo",
            "Demo Provider",
            &[("model-a", "Model A"), ("model-b", "Model B")],
        )];
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "demo/model-a".to_string(),
                    fallbacks: Vec::new(),
                    extra: std::collections::HashMap::new(),
                }),
                models: None,
                extra: std::collections::HashMap::new(),
            });

        let action = app.on_key(key(KeyCode::Delete), &data);

        let Action::EditorSubmit { submit, content } = action else {
            panic!("expected primary-model delete to auto-submit, got {action:?}");
        };
        assert_eq!(submit, EditorSubmit::ConfigOpenClawAgents);

        let form = app
            .openclaw_agents_form
            .as_ref()
            .expect("agents form should stay initialized");
        assert!(form.primary_model.is_empty());

        let saved: crate::openclaw_config::OpenClawAgentsDefaults =
            serde_json::from_str(&content).expect("serialize agents form");
        assert!(saved.model.is_none(), "{content}");
    }

    #[test]
    fn openclaw_agents_backspace_clears_unsupported_primary_model_auto_submits() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows = vec![openclaw_provider_row(
            "demo",
            "Demo Provider",
            &[("model-a", "Model A")],
        )];
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "missing/off-catalog".to_string(),
                    fallbacks: Vec::new(),
                    extra: std::collections::HashMap::new(),
                }),
                models: None,
                extra: std::collections::HashMap::new(),
            });

        let action = app.on_key(key(KeyCode::Backspace), &data);

        let Action::EditorSubmit { submit, content } = action else {
            panic!("expected primary-model backspace clear to auto-submit, got {action:?}");
        };
        assert_eq!(submit, EditorSubmit::ConfigOpenClawAgents);

        let form = app
            .openclaw_agents_form
            .as_ref()
            .expect("agents form should stay initialized");
        assert!(form.primary_model.is_empty());

        let saved: crate::openclaw_config::OpenClawAgentsDefaults =
            serde_json::from_str(&content).expect("serialize agents form");
        assert!(saved.model.is_none(), "{content}");
    }

    #[test]
    fn openclaw_agents_add_fallback_keeps_form_unchanged_until_picker_confirmation() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows = vec![openclaw_provider_row(
            "demo",
            "Demo Provider",
            &[
                ("primary", "Primary"),
                ("fallback-a", "Fallback A"),
                ("fallback-b", "Fallback B"),
            ],
        )];
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "demo/primary".to_string(),
                    fallbacks: vec!["demo/fallback-a".to_string()],
                    extra: std::collections::HashMap::new(),
                }),
                models: None,
                extra: std::collections::HashMap::new(),
            });

        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(action, Action::None));
        let form = app
            .openclaw_agents_form
            .as_ref()
            .expect("agents form should be initialized");
        assert_eq!(form.fallbacks, vec!["demo/fallback-a".to_string()]);
        assert!(
            app.overlay.is_active(),
            "fallback add should open a picker overlay"
        );
    }

    #[test]
    fn openclaw_agents_existing_fallback_enter_opens_picker_excluding_primary_and_other_rows() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows = vec![openclaw_provider_row(
            "demo",
            "Demo Provider",
            &[
                ("primary", "Primary"),
                ("fallback-a", "Fallback A"),
                ("fallback-b", "Fallback B"),
                ("fallback-c", "Fallback C"),
                ("fallback-d", "Fallback D"),
            ],
        )];
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "demo/primary".to_string(),
                    fallbacks: vec!["demo/fallback-a".to_string(), "demo/fallback-c".to_string()],
                    extra: std::collections::HashMap::new(),
                }),
                models: None,
                extra: std::collections::HashMap::new(),
            });

        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(action, Action::None));
        let Overlay::OpenClawAgentsFallbackPicker { options, .. } = &app.overlay else {
            panic!(
                "expected existing fallback edit to open picker, got {:?}",
                app.overlay
            );
        };
        assert_eq!(
            options
                .iter()
                .map(|option| option.value.as_str())
                .collect::<Vec<_>>(),
            vec!["demo/fallback-b", "demo/fallback-c", "demo/fallback-d"]
        );
    }

    #[test]
    fn openclaw_agents_existing_fallback_picker_escape_leaves_row_unchanged() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows = vec![openclaw_provider_row(
            "demo",
            "Demo Provider",
            &[
                ("primary", "Primary"),
                ("fallback-a", "Fallback A"),
                ("fallback-b", "Fallback B"),
                ("fallback-c", "Fallback C"),
            ],
        )];
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "demo/primary".to_string(),
                    fallbacks: vec!["demo/fallback-a".to_string(), "demo/fallback-c".to_string()],
                    extra: std::collections::HashMap::new(),
                }),
                models: None,
                extra: std::collections::HashMap::new(),
            });

        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Esc), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        let form = app
            .openclaw_agents_form
            .as_ref()
            .expect("agents form should stay initialized");
        assert_eq!(
            form.fallbacks,
            vec!["demo/fallback-a".to_string(), "demo/fallback-c".to_string()]
        );
        assert_eq!(form.section, OpenClawAgentsSection::FallbackModels);
        assert_eq!(form.row, 1);
    }

    #[test]
    fn openclaw_agents_delete_existing_fallback_auto_submits_removed_value() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows = vec![openclaw_provider_row(
            "demo",
            "Demo Provider",
            &[
                ("primary", "Primary"),
                ("fallback-a", "Fallback A"),
                ("fallback-b", "Fallback B"),
            ],
        )];
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "demo/primary".to_string(),
                    fallbacks: vec!["demo/fallback-a".to_string(), "demo/fallback-b".to_string()],
                    extra: std::collections::HashMap::new(),
                }),
                models: None,
                extra: std::collections::HashMap::new(),
            });

        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Delete), &data);

        let Action::EditorSubmit { submit, content } = action else {
            panic!("expected fallback delete to auto-submit, got {action:?}");
        };
        assert_eq!(submit, EditorSubmit::ConfigOpenClawAgents);

        let saved: crate::openclaw_config::OpenClawAgentsDefaults =
            serde_json::from_str(&content).expect("serialize agents form");
        let model = saved.model.expect("model should remain present");
        assert_eq!(model.primary, "demo/primary");
        assert_eq!(model.fallbacks, vec!["demo/fallback-a".to_string()]);
    }

    #[test]
    fn openclaw_agents_backspace_deletes_existing_fallback_auto_submits_removed_value() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows = vec![openclaw_provider_row(
            "demo",
            "Demo Provider",
            &[
                ("primary", "Primary"),
                ("fallback-a", "Fallback A"),
                ("fallback-b", "Fallback B"),
            ],
        )];
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "demo/primary".to_string(),
                    fallbacks: vec!["demo/fallback-a".to_string(), "demo/fallback-b".to_string()],
                    extra: std::collections::HashMap::new(),
                }),
                models: None,
                extra: std::collections::HashMap::new(),
            });

        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Backspace), &data);

        let Action::EditorSubmit { submit, content } = action else {
            panic!("expected fallback backspace delete to auto-submit, got {action:?}");
        };
        assert_eq!(submit, EditorSubmit::ConfigOpenClawAgents);

        let saved: crate::openclaw_config::OpenClawAgentsDefaults =
            serde_json::from_str(&content).expect("serialize agents form");
        let model = saved.model.expect("model should remain present");
        assert_eq!(model.primary, "demo/primary");
        assert_eq!(model.fallbacks, vec!["demo/fallback-a".to_string()]);
    }

    #[test]
    fn openclaw_agents_navigation_skips_disabled_add_fallback_row_when_no_eligible_models_remain() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows = vec![openclaw_provider_row(
            "demo",
            "Demo Provider",
            &[("primary", "Primary"), ("fallback-a", "Fallback A")],
        )];
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "demo/primary".to_string(),
                    fallbacks: vec!["demo/fallback-a".to_string()],
                    extra: std::collections::HashMap::new(),
                }),
                models: None,
                extra: std::collections::HashMap::new(),
            });

        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));

        let form = app
            .openclaw_agents_form
            .as_ref()
            .expect("agents form should be initialized");
        assert_eq!(form.fallbacks, vec!["demo/fallback-a".to_string()]);
        assert_eq!(form.section, OpenClawAgentsSection::Runtime);
        assert_eq!(form.row, 0);
        assert!(matches!(app.overlay, Overlay::None));
        assert!(app.toast.is_none());
    }

    #[test]
    fn openclaw_agents_disabled_add_fallback_row_still_noops_for_stale_selection_state() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows = vec![openclaw_provider_row(
            "demo",
            "Demo Provider",
            &[("primary", "Primary"), ("fallback-a", "Fallback A")],
        )];
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "demo/primary".to_string(),
                    fallbacks: vec!["demo/fallback-a".to_string()],
                    extra: std::collections::HashMap::new(),
                }),
                models: None,
                extra: std::collections::HashMap::new(),
            });

        let mut form = crate::cli::tui::app::OpenClawAgentsFormState::from_snapshot(
            data.config.openclaw_agents_defaults.as_ref(),
        );
        form.section = OpenClawAgentsSection::FallbackModels;
        form.row = form.fallbacks.len();
        app.openclaw_agents_form = Some(form);

        let action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(action, Action::None));
        let form = app
            .openclaw_agents_form
            .as_ref()
            .expect("agents form should stay initialized");
        assert_eq!(form.section, OpenClawAgentsSection::FallbackModels);
        assert_eq!(form.row, form.fallbacks.len());
        assert!(matches!(app.overlay, Overlay::None));
        assert!(app.toast.is_none());
    }

    #[test]
    fn openclaw_agents_ctrl_s_on_runtime_row_does_not_edit_or_submit() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "demo/primary".to_string(),
                    fallbacks: Vec::new(),
                    extra: std::collections::HashMap::new(),
                }),
                models: None,
                extra: std::collections::HashMap::from([(
                    "workspace".to_string(),
                    json!("existing-workspace"),
                )]),
            });

        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));

        let action = app.on_key(ctrl(KeyCode::Char('s')), &data);

        assert!(matches!(action, Action::None));
        let form = app
            .openclaw_agents_form
            .as_ref()
            .expect("agents form should be initialized");
        assert_eq!(form.section, OpenClawAgentsSection::Runtime);
        assert_eq!(form.workspace, "existing-workspace");
        assert!(app.toast.is_none());
    }

    #[test]
    fn openclaw_agents_ctrl_s_is_ignored_while_model_picker_overlay_is_open() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows = vec![openclaw_provider_row(
            "demo",
            "Demo Provider",
            &[("model-a", "Model A"), ("model-b", "Model B")],
        )];
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "demo/model-a".to_string(),
                    fallbacks: Vec::new(),
                    extra: std::collections::HashMap::new(),
                }),
                models: None,
                extra: std::collections::HashMap::new(),
            });

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.overlay,
            Overlay::OpenClawAgentsFallbackPicker { .. }
        ));

        let action = app.on_key(ctrl(KeyCode::Char('s')), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::OpenClawAgentsFallbackPicker { .. }
        ));
        let form = app
            .openclaw_agents_form
            .as_ref()
            .expect("agents form should stay initialized");
        assert_eq!(form.primary_model, "demo/model-a");
        assert!(form.fallbacks.is_empty());
        assert!(app.toast.is_none());
    }

    #[test]
    fn openclaw_agents_navigation_stops_before_hidden_save_section() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows = vec![openclaw_provider_row(
            "demo",
            "Demo Provider",
            &[("primary", "Primary")],
        )];
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "demo/primary".to_string(),
                    fallbacks: Vec::new(),
                    extra: std::collections::HashMap::new(),
                }),
                models: None,
                extra: std::collections::HashMap::new(),
            });

        for _ in 0..6 {
            assert!(matches!(
                app.on_key(key(KeyCode::Down), &data),
                Action::None
            ));
        }

        let form = app
            .openclaw_agents_form
            .as_ref()
            .expect("agents form should be initialized");
        assert_eq!(form.section, OpenClawAgentsSection::Runtime);
        assert_eq!(form.row, 3);
    }

    #[test]
    fn openclaw_tools_save_preserves_unsupported_profile_without_explicit_change() {
        let temp_home = TempDir::new().expect("create temp home");
        let openclaw_dir = temp_home.path().join(".openclaw");
        std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
            profile: Some("unsupported-profile".to_string()),
            allow: vec!["Read".to_string()],
            deny: vec!["Bash".to_string()],
            extra: std::collections::HashMap::new(),
        });

        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('W')), &data),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);

        let Action::EditorSubmit { submit, content } = action else {
            panic!("expected tools form save action, got {action:?}");
        };
        assert_eq!(submit, EditorSubmit::ConfigOpenClawTools);

        let pending: crate::openclaw_config::OpenClawToolsConfig =
            serde_json::from_str(&content).expect("serialize tools form");
        assert_eq!(pending.profile.as_deref(), Some("unsupported-profile"));
        assert_eq!(pending.allow, vec!["ReadW".to_string()]);
        assert_eq!(pending.deny, vec!["Bash".to_string()]);

        run_runtime_action(
            &mut app,
            &mut data,
            Action::EditorSubmit { submit, content },
        )
        .expect("structured tools save should succeed");

        assert_eq!(
            data.config
                .openclaw_tools
                .as_ref()
                .and_then(|tools| tools.profile.as_deref()),
            Some("unsupported-profile")
        );
        assert!(
            app.toast.is_none(),
            "successful tools auto-save should stay silent"
        );
    }

    #[test]
    fn openclaw_tools_list_editing_updates_allow_and_deny_entries_before_save() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
            profile: Some("coding".to_string()),
            allow: vec!["Read".to_string()],
            deny: Vec::new(),
            extra: std::collections::HashMap::new(),
        });

        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('W')), &data),
            Action::None
        ));
        let action = app.on_key(key(KeyCode::Enter), &data);
        let Action::EditorSubmit { submit, content } = action else {
            panic!("expected tools form save action, got {action:?}");
        };
        assert_eq!(submit, EditorSubmit::ConfigOpenClawTools);

        let saved: crate::openclaw_config::OpenClawToolsConfig =
            serde_json::from_str(&content).expect("serialize tools form");
        assert_eq!(saved.profile.as_deref(), Some("coding"));
        assert_eq!(saved.allow, vec!["ReadW".to_string()]);

        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        for ch in ['E', 'x', 'e', 'c'] {
            assert!(matches!(
                app.on_key(key(KeyCode::Char(ch)), &data),
                Action::None
            ));
        }
        let action = app.on_key(key(KeyCode::Enter), &data);
        let Action::EditorSubmit { submit, content } = action else {
            panic!("expected tools form save action, got {action:?}");
        };
        assert_eq!(submit, EditorSubmit::ConfigOpenClawTools);

        let saved: crate::openclaw_config::OpenClawToolsConfig =
            serde_json::from_str(&content).expect("serialize tools form");
        assert_eq!(saved.profile.as_deref(), Some("coding"));
        assert_eq!(saved.allow, vec!["ReadW".to_string()]);
        assert_eq!(saved.deny, vec!["Exec".to_string()]);

        let delete_action = app.on_key(key(KeyCode::Delete), &data);
        let Action::EditorSubmit { submit, content } = delete_action else {
            panic!("expected delete to auto-save, got {delete_action:?}");
        };
        assert_eq!(submit, EditorSubmit::ConfigOpenClawTools);

        let saved: crate::openclaw_config::OpenClawToolsConfig =
            serde_json::from_str(&content).expect("serialize tools form");
        assert_eq!(saved.allow, vec!["ReadW".to_string()]);
        assert!(saved.deny.is_empty());
    }

    #[test]
    fn openclaw_tools_backspace_on_existing_rule_row_deletes_and_auto_submits() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
            profile: Some("coding".to_string()),
            allow: vec!["Read".to_string()],
            deny: vec!["Exec".to_string()],
            extra: std::collections::HashMap::new(),
        });

        for _ in 0..3 {
            assert!(matches!(
                app.on_key(key(KeyCode::Down), &data),
                Action::None
            ));
        }

        let action = app.on_key(key(KeyCode::Backspace), &data);

        let Action::EditorSubmit { submit, content } = action else {
            panic!("expected backspace delete to auto-save, got {action:?}");
        };
        assert_eq!(submit, EditorSubmit::ConfigOpenClawTools);

        let saved: crate::openclaw_config::OpenClawToolsConfig =
            serde_json::from_str(&content).expect("serialize tools form");
        assert_eq!(saved.allow, vec!["Read".to_string()]);
        assert!(saved.deny.is_empty());
    }

    #[test]
    fn openclaw_agents_add_and_remove_fallbacks_without_opening_json_editor() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows = vec![openclaw_provider_row(
            "demo",
            "Demo Provider",
            &[
                ("primary", "Primary"),
                ("fallback-a", "Fallback A"),
                ("fallback-b", "Fallback B"),
            ],
        )];
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "demo/primary".to_string(),
                    fallbacks: vec!["demo/fallback-a".to_string()],
                    extra: std::collections::HashMap::new(),
                }),
                models: None,
                extra: std::collections::HashMap::new(),
            });

        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        let _ = app.on_key(key(KeyCode::Enter), &data);
        let _ = app.on_key(key(KeyCode::Enter), &data);
        let action = app.on_key(key(KeyCode::Delete), &data);
        assert!(
            app.editor.is_none(),
            "agents route should stay in structured form mode"
        );
        let Action::EditorSubmit { submit, content } = action else {
            panic!("expected agents delete to auto-submit, got {action:?}");
        };
        assert_eq!(submit, EditorSubmit::ConfigOpenClawAgents);

        let saved: crate::openclaw_config::OpenClawAgentsDefaults =
            serde_json::from_str(&content).expect("serialize agents form");
        let model = saved.model.expect("model should remain present");
        assert_eq!(model.primary, "demo/primary");
        assert_eq!(model.fallbacks, vec!["demo/fallback-a".to_string()]);
    }

    #[test]
    fn openclaw_agents_picker_enter_inserts_selected_fallback_and_auto_submits() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows = vec![openclaw_provider_row(
            "demo",
            "Demo Provider",
            &[
                ("primary", "Primary"),
                ("fallback-a", "Fallback A"),
                ("fallback-b", "Fallback B"),
                ("fallback-c", "Fallback C"),
            ],
        )];
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "demo/primary".to_string(),
                    fallbacks: vec!["demo/fallback-a".to_string()],
                    extra: std::collections::HashMap::new(),
                }),
                models: None,
                extra: std::collections::HashMap::new(),
            });

        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);

        let Action::EditorSubmit { submit, content } = action else {
            panic!("expected picker confirmation to auto-submit, got {action:?}");
        };
        assert_eq!(submit, EditorSubmit::ConfigOpenClawAgents);
        assert!(matches!(app.overlay, Overlay::None));

        let saved: crate::openclaw_config::OpenClawAgentsDefaults =
            serde_json::from_str(&content).expect("serialize agents form");
        let model = saved.model.expect("model should remain present");
        assert_eq!(
            model.fallbacks,
            vec!["demo/fallback-a".to_string(), "demo/fallback-b".to_string(),]
        );
    }

    #[test]
    fn openclaw_agents_picker_navigation_selects_non_default_fallback() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows = vec![openclaw_provider_row(
            "demo",
            "Demo Provider",
            &[
                ("primary", "Primary"),
                ("fallback-a", "Fallback A"),
                ("fallback-b", "Fallback B"),
                ("fallback-c", "Fallback C"),
            ],
        )];
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "demo/primary".to_string(),
                    fallbacks: vec!["demo/fallback-a".to_string()],
                    extra: std::collections::HashMap::new(),
                }),
                models: None,
                extra: std::collections::HashMap::new(),
            });

        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);

        let Action::EditorSubmit { submit, content } = action else {
            panic!("expected picker confirmation to auto-submit, got {action:?}");
        };
        assert_eq!(submit, EditorSubmit::ConfigOpenClawAgents);

        let saved: crate::openclaw_config::OpenClawAgentsDefaults =
            serde_json::from_str(&content).expect("serialize agents form");
        let model = saved.model.expect("model should remain present");
        assert_eq!(
            model.fallbacks,
            vec!["demo/fallback-a".to_string(), "demo/fallback-c".to_string(),]
        );
    }

    #[test]
    fn openclaw_agents_picker_escape_leaves_fallbacks_unchanged() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows = vec![openclaw_provider_row(
            "demo",
            "Demo Provider",
            &[
                ("primary", "Primary"),
                ("fallback-a", "Fallback A"),
                ("fallback-b", "Fallback B"),
            ],
        )];
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "demo/primary".to_string(),
                    fallbacks: vec!["demo/fallback-a".to_string()],
                    extra: std::collections::HashMap::new(),
                }),
                models: None,
                extra: std::collections::HashMap::new(),
            });

        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Esc), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        let form = app
            .openclaw_agents_form
            .as_ref()
            .expect("agents form should be initialized");
        assert_eq!(form.fallbacks, vec!["demo/fallback-a".to_string()]);
    }

    #[test]
    fn openclaw_agents_runtime_enter_opens_popup_editor_for_selected_row() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "demo/primary".to_string(),
                    fallbacks: Vec::new(),
                    extra: std::collections::HashMap::new(),
                }),
                models: None,
                extra: std::collections::HashMap::new(),
            });

        app.openclaw_agents_form = Some(openclaw_agents_runtime_form(
            data.config.openclaw_agents_defaults.as_ref(),
            1,
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                ref title,
                ref prompt,
                ref buffer,
                submit: TextSubmit::OpenClawAgentsRuntimeField {
                    field: OpenClawAgentsRuntimeField::Timeout,
                },
                ..
            }) if title == texts::tui_openclaw_agents_timeout()
                && prompt == texts::tui_openclaw_agents_timeout()
                && buffer.is_empty()
        ));
    }

    #[test]
    fn openclaw_agents_runtime_popup_accepts_hjkl_characters() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.openclaw_agents_form = Some(openclaw_agents_runtime_form(None, 0));

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));

        for ch in ['h', 'j', 'k', 'l'] {
            assert!(matches!(
                app.on_key(key(KeyCode::Char(ch)), &data),
                Action::None
            ));
        }

        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState { ref buffer, .. }) if buffer == "hjkl"
        ));

        let form = app
            .openclaw_agents_form
            .as_ref()
            .expect("agents form should be initialized");
        assert_eq!(form.section, OpenClawAgentsSection::Runtime);
        assert_eq!(form.row, 0);
        assert!(form.workspace.is_empty());
    }

    #[test]
    fn openclaw_agents_runtime_popup_accepts_global_hotkey_characters() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.openclaw_agents_form = Some(openclaw_agents_runtime_form(None, 0));

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));

        for ch in ['/', '?', '[', ']', 'q'] {
            assert!(matches!(
                app.on_key(key(KeyCode::Char(ch)), &data),
                Action::None
            ));
        }

        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState { ref buffer, .. }) if buffer == "/?[]q"
        ));

        let form = app
            .openclaw_agents_form
            .as_ref()
            .expect("agents form should be initialized");
        assert_eq!(form.section, OpenClawAgentsSection::Runtime);
        assert!(form.workspace.is_empty());
        assert_eq!(app.route, Route::ConfigOpenClawAgents);
        assert_eq!(app.app_type, AppType::OpenClaw);
        assert!(!app.filter.active, "typing should not open filter mode");
        assert!(matches!(app.overlay, Overlay::TextInput(_)));
    }

    #[test]
    fn openclaw_agents_runtime_popup_ignores_ctrl_s() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.openclaw_agents_form = Some(openclaw_agents_runtime_form(None, 0));

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('x')), &data),
            Action::None
        ));

        let action = app.on_key(ctrl(KeyCode::Char('s')), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState { ref buffer, .. }) if buffer == "x"
        ));

        let form = app
            .openclaw_agents_form
            .as_ref()
            .expect("agents form should stay initialized");
        assert_eq!(form.workspace, "");
        assert_eq!(form.section, OpenClawAgentsSection::Runtime);
        assert_eq!(form.row, 0);
    }

    #[test]
    fn openclaw_agents_runtime_popup_confirm_applies_value_and_auto_submits() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: None,
                models: None,
                extra: std::collections::HashMap::from([(
                    "workspace".to_string(),
                    json!("existing-workspace"),
                )]),
            });
        app.openclaw_agents_form = Some(openclaw_agents_runtime_form(
            data.config.openclaw_agents_defaults.as_ref(),
            0,
        ));

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('x')), &data),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);

        let Action::EditorSubmit { submit, content } = action else {
            panic!("expected agents popup submit to auto-save, got {action:?}");
        };
        assert_eq!(submit, EditorSubmit::ConfigOpenClawAgents);
        assert!(matches!(app.overlay, Overlay::None));

        let form = app
            .openclaw_agents_form
            .as_ref()
            .expect("agents form should stay initialized");
        assert_eq!(form.workspace, "existing-workspacex");

        let pending: crate::openclaw_config::OpenClawAgentsDefaults =
            serde_json::from_str(&content).expect("serialize agents form");
        assert_eq!(
            pending.extra.get("workspace"),
            Some(&json!("existing-workspacex"))
        );
    }

    #[test]
    fn openclaw_agents_runtime_popup_confirm_allows_unrelated_legacy_timeout_values() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: None,
                models: None,
                extra: std::collections::HashMap::from([
                    ("workspace".to_string(), json!("existing-workspace")),
                    ("timeout".to_string(), json!("manual-value")),
                ]),
            });
        app.openclaw_agents_form = Some(openclaw_agents_runtime_form(
            data.config.openclaw_agents_defaults.as_ref(),
            0,
        ));

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('x')), &data),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);

        let Action::EditorSubmit { submit, content } = action else {
            panic!(
                "expected workspace popup submit to bypass legacy timeout blocker, got {action:?}"
            );
        };
        assert_eq!(submit, EditorSubmit::ConfigOpenClawAgents);
        assert!(app.toast.is_none());

        let pending: crate::openclaw_config::OpenClawAgentsDefaults =
            serde_json::from_str(&content).expect("serialize agents form");
        assert_eq!(
            pending.extra.get("workspace"),
            Some(&json!("existing-workspacex"))
        );
        assert_eq!(
            pending.extra.get("timeoutSeconds"),
            Some(&json!("manual-value"))
        );
        assert!(!pending.extra.contains_key("timeout"));
    }

    #[test]
    fn openclaw_agents_runtime_popup_whitespace_confirm_is_non_destructive() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: None,
                models: None,
                extra: std::collections::HashMap::from([(
                    "workspace".to_string(),
                    json!("existing-workspace"),
                )]),
            });
        app.openclaw_agents_form = Some(openclaw_agents_runtime_form(
            data.config.openclaw_agents_defaults.as_ref(),
            0,
        ));

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        if let Overlay::TextInput(ref mut input) = app.overlay {
            input.buffer = "   ".to_string();
        } else {
            panic!("expected runtime text input overlay");
        }

        let action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        assert!(app.toast.is_none());

        let form = app
            .openclaw_agents_form
            .as_ref()
            .expect("agents form should stay initialized");
        assert_eq!(form.workspace, "existing-workspace");
    }

    #[test]
    fn openclaw_agents_runtime_popup_empty_confirm_does_not_clear_legacy_timeout_state() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: None,
                models: None,
                extra: std::collections::HashMap::from([(
                    "timeout".to_string(),
                    json!("manual-value"),
                )]),
            });
        app.openclaw_agents_form = Some(openclaw_agents_runtime_form(
            data.config.openclaw_agents_defaults.as_ref(),
            1,
        ));

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        if let Overlay::TextInput(ref mut input) = app.overlay {
            input.buffer.clear();
        } else {
            panic!("expected timeout text input overlay");
        }

        let action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        assert!(app.toast.is_none());

        let form = app
            .openclaw_agents_form
            .as_ref()
            .expect("agents form should stay initialized");
        assert_eq!(form.timeout, "manual-value");
        assert!(form.has_legacy_timeout);
        assert!(form.timeout_seconds_seed.is_none());
    }

    #[test]
    fn openclaw_agents_runtime_submit_helper_treats_whitespace_as_non_destructive() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: None,
                models: None,
                extra: std::collections::HashMap::from([(
                    "workspace".to_string(),
                    json!("existing-workspace"),
                )]),
            });
        app.openclaw_agents_form = Some(openclaw_agents_runtime_form(
            data.config.openclaw_agents_defaults.as_ref(),
            0,
        ));

        let action = app.submit_openclaw_agents_runtime_popup_field(
            &data,
            OpenClawAgentsRuntimeField::Workspace,
            "   ".to_string(),
        );

        assert!(matches!(action, Action::None));
        let form = app
            .openclaw_agents_form
            .as_ref()
            .expect("agents form should stay initialized");
        assert_eq!(form.workspace, "existing-workspace");
    }

    #[test]
    fn openclaw_agents_runtime_delete_clears_timeout_seed_and_legacy_state() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: None,
                models: None,
                extra: std::collections::HashMap::from([
                    ("timeout".to_string(), json!(42)),
                    ("timeoutSeconds".to_string(), json!(false)),
                ]),
            });
        app.openclaw_agents_form = Some(openclaw_agents_runtime_form(
            data.config.openclaw_agents_defaults.as_ref(),
            1,
        ));

        let action = app.on_key(key(KeyCode::Delete), &data);

        let Action::EditorSubmit { submit, content } = action else {
            panic!("expected timeout delete to auto-submit, got {action:?}");
        };
        assert_eq!(submit, EditorSubmit::ConfigOpenClawAgents);

        let form = app
            .openclaw_agents_form
            .as_ref()
            .expect("agents form should stay initialized");
        assert_eq!(form.section, OpenClawAgentsSection::Runtime);
        assert_eq!(form.row, 1);
        assert!(form.timeout.is_empty());
        assert!(form.timeout_seconds_seed.is_none());
        assert!(!form.has_legacy_timeout);

        let pending: crate::openclaw_config::OpenClawAgentsDefaults =
            serde_json::from_str(&content).expect("serialize agents form");
        assert!(!pending.extra.contains_key("timeout"), "{content}");
        assert!(!pending.extra.contains_key("timeoutSeconds"), "{content}");
    }

    #[test]
    fn openclaw_agents_runtime_backspace_clears_preserved_non_string_seed() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: None,
                models: None,
                extra: std::collections::HashMap::from([(
                    "contextTokens".to_string(),
                    json!(false),
                )]),
            });
        app.openclaw_agents_form = Some(openclaw_agents_runtime_form(
            data.config.openclaw_agents_defaults.as_ref(),
            2,
        ));

        let action = app.on_key(key(KeyCode::Backspace), &data);

        let Action::EditorSubmit { submit, content } = action else {
            panic!("expected runtime backspace clear to auto-submit, got {action:?}");
        };
        assert_eq!(submit, EditorSubmit::ConfigOpenClawAgents);

        let form = app
            .openclaw_agents_form
            .as_ref()
            .expect("agents form should stay initialized");
        assert_eq!(form.section, OpenClawAgentsSection::Runtime);
        assert_eq!(form.row, 2);
        assert!(form.context_tokens.is_empty());
        assert!(form.context_tokens_seed.is_none());

        let pending: crate::openclaw_config::OpenClawAgentsDefaults =
            serde_json::from_str(&content).expect("serialize agents form");
        assert!(!pending.extra.contains_key("contextTokens"), "{content}");
    }

    #[test]
    fn openclaw_agents_runtime_popup_cancel_leaves_field_unchanged() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: None,
                models: None,
                extra: std::collections::HashMap::from([(
                    "workspace".to_string(),
                    json!("existing-workspace"),
                )]),
            });
        app.openclaw_agents_form = Some(openclaw_agents_runtime_form(
            data.config.openclaw_agents_defaults.as_ref(),
            0,
        ));

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('x')), &data),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Esc), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        let form = app
            .openclaw_agents_form
            .as_ref()
            .expect("agents form should stay initialized");
        assert_eq!(form.workspace, "existing-workspace");
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_agents_save_migrates_legacy_timeout_and_preserves_unknown_fields() {
        let _lang = use_test_language(Language::Chinese);
        let temp_home = TempDir::new().expect("create temp home");
        let openclaw_dir = temp_home.path().join(".openclaw");
        std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows = vec![openclaw_provider_row(
            "catalog",
            "Catalog Provider",
            &[("fallback-a", "Fallback A")],
        )];
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "missing/current-primary".to_string(),
                    fallbacks: vec![
                        "catalog/fallback-a".to_string(),
                        "missing/off-catalog".to_string(),
                    ],
                    extra: std::collections::HashMap::from([(
                        "temperature".to_string(),
                        json!(0.2),
                    )]),
                }),
                models: None,
                extra: std::collections::HashMap::from([
                    ("workspace".to_string(), json!("./workspace")),
                    ("timeout".to_string(), json!(42)),
                    ("contextTokens".to_string(), json!(4096)),
                    ("maxConcurrent".to_string(), json!(3)),
                    ("customFlag".to_string(), json!(true)),
                ]),
            });

        app.openclaw_agents_form = Some(openclaw_agents_runtime_form(
            data.config.openclaw_agents_defaults.as_ref(),
            0,
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('x')), &data),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);
        let Action::EditorSubmit { submit, content } = action else {
            panic!("expected agents runtime popup submit to auto-save, got {action:?}");
        };
        assert_eq!(submit, EditorSubmit::ConfigOpenClawAgents);

        let pending: crate::openclaw_config::OpenClawAgentsDefaults =
            serde_json::from_str(&content).expect("serialize agents form");
        let model = pending.model.as_ref().expect("model should be serialized");
        assert_eq!(model.primary, "missing/current-primary");
        assert_eq!(
            model.fallbacks,
            vec![
                "catalog/fallback-a".to_string(),
                "missing/off-catalog".to_string(),
            ]
        );
        assert_eq!(model.extra.get("temperature"), Some(&json!(0.2)));
        assert_eq!(pending.extra.get("workspace"), Some(&json!("./workspacex")));
        assert_eq!(pending.extra.get("timeoutSeconds"), Some(&json!(42)));
        assert!(!pending.extra.contains_key("timeout"));
        assert_eq!(pending.extra.get("contextTokens"), Some(&json!(4096)));
        assert_eq!(pending.extra.get("maxConcurrent"), Some(&json!(3)));
        assert_eq!(pending.extra.get("customFlag"), Some(&json!(true)));

        run_runtime_action(
            &mut app,
            &mut data,
            Action::EditorSubmit { submit, content },
        )
        .expect("structured agents save should succeed");

        assert!(
            app.toast.is_none(),
            "successful agents auto-save should stay silent"
        );

        let source = std::fs::read_to_string(openclaw_dir.join("openclaw.json"))
            .expect("read saved openclaw config");
        assert!(source.contains("timeoutSeconds"), "{source}");
        assert!(!source.contains("timeout: 42"), "{source}");
        assert!(source.contains("customFlag"), "{source}");
        assert!(source.contains("missing/current-primary"), "{source}");
        assert!(source.contains("missing/off-catalog"), "{source}");
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_agents_save_preserves_existing_invalid_runtime_values() {
        let temp_home = TempDir::new().expect("create temp home");
        let openclaw_dir = temp_home.path().join(".openclaw");
        std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "demo/primary".to_string(),
                    fallbacks: Vec::new(),
                    extra: std::collections::HashMap::new(),
                }),
                models: None,
                extra: std::collections::HashMap::from([
                    ("timeoutSeconds".to_string(), json!("manual-timeout")),
                    ("contextTokens".to_string(), json!("manual-context")),
                    ("maxConcurrent".to_string(), json!("manual-max")),
                ]),
            });

        app.openclaw_agents_form = Some(openclaw_agents_runtime_form(
            data.config.openclaw_agents_defaults.as_ref(),
            0,
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('w')), &data),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);
        let Action::EditorSubmit { submit, content } = action else {
            panic!("expected agents runtime popup submit to auto-save, got {action:?}");
        };
        assert_eq!(submit, EditorSubmit::ConfigOpenClawAgents);

        let pending: crate::openclaw_config::OpenClawAgentsDefaults =
            serde_json::from_str(&content).expect("serialize agents form");
        assert_eq!(pending.extra.get("workspace"), Some(&json!("w")));
        assert_eq!(
            pending.extra.get("timeoutSeconds"),
            Some(&json!("manual-timeout"))
        );
        assert_eq!(
            pending.extra.get("contextTokens"),
            Some(&json!("manual-context"))
        );
        assert_eq!(
            pending.extra.get("maxConcurrent"),
            Some(&json!("manual-max"))
        );

        run_runtime_action(
            &mut app,
            &mut data,
            Action::EditorSubmit { submit, content },
        )
        .expect("structured agents save should preserve invalid runtime strings");

        let source = std::fs::read_to_string(openclaw_dir.join("openclaw.json"))
            .expect("read saved openclaw config");
        assert!(
            source.contains("\"timeoutSeconds\": \"manual-timeout\""),
            "{source}"
        );
        assert!(
            source.contains("\"contextTokens\": \"manual-context\""),
            "{source}"
        );
        assert!(
            source.contains("\"maxConcurrent\": \"manual-max\""),
            "{source}"
        );
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_agents_save_preserves_existing_non_string_runtime_values() {
        let temp_home = TempDir::new().expect("create temp home");
        let openclaw_dir = temp_home.path().join(".openclaw");
        std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "demo/primary".to_string(),
                    fallbacks: Vec::new(),
                    extra: std::collections::HashMap::new(),
                }),
                models: None,
                extra: std::collections::HashMap::from([
                    ("timeoutSeconds".to_string(), json!(false)),
                    ("contextTokens".to_string(), json!(null)),
                    ("maxConcurrent".to_string(), json!({ "raw": 3 })),
                ]),
            });

        app.openclaw_agents_form = Some(openclaw_agents_runtime_form(
            data.config.openclaw_agents_defaults.as_ref(),
            0,
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('w')), &data),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);
        let Action::EditorSubmit { submit, content } = action else {
            panic!("expected agents runtime popup submit to auto-save, got {action:?}");
        };
        assert_eq!(submit, EditorSubmit::ConfigOpenClawAgents);

        let pending: crate::openclaw_config::OpenClawAgentsDefaults =
            serde_json::from_str(&content).expect("serialize agents form");
        assert_eq!(pending.extra.get("workspace"), Some(&json!("w")));
        assert_eq!(pending.extra.get("timeoutSeconds"), Some(&json!(false)));
        assert_eq!(pending.extra.get("contextTokens"), Some(&json!(null)));
        assert_eq!(
            pending.extra.get("maxConcurrent"),
            Some(&json!({ "raw": 3 }))
        );

        run_runtime_action(
            &mut app,
            &mut data,
            Action::EditorSubmit { submit, content },
        )
        .expect("structured agents save should preserve invalid runtime values");

        let source = std::fs::read_to_string(openclaw_dir.join("openclaw.json"))
            .expect("read saved openclaw config");
        assert!(source.contains("\"timeoutSeconds\": false"), "{source}");
        assert!(source.contains("\"contextTokens\": null"), "{source}");
        assert!(
            source.contains("\"maxConcurrent\": {") && source.contains("\"raw\": 3"),
            "{source}"
        );
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_agents_save_failure_surfaces_upstream_copy() {
        let _lang = use_test_language(Language::Chinese);
        let temp_home = TempDir::new().expect("create temp home");
        let blocked_path = temp_home.path().join("blocked-openclaw");
        std::fs::write(&blocked_path, "not a directory").expect("seed blocking file");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&blocked_path);

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows = vec![openclaw_provider_row(
            "demo",
            "Demo Provider",
            &[("primary", "Primary")],
        )];
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: "demo/primary".to_string(),
                    fallbacks: Vec::new(),
                    extra: std::collections::HashMap::new(),
                }),
                models: None,
                extra: std::collections::HashMap::new(),
            });

        app.openclaw_agents_form = Some(openclaw_agents_runtime_form(
            data.config.openclaw_agents_defaults.as_ref(),
            0,
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('x')), &data),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);
        let Action::EditorSubmit { submit, content } = action else {
            panic!("expected agents runtime popup submit to auto-save, got {action:?}");
        };

        run_runtime_action(
            &mut app,
            &mut data,
            Action::EditorSubmit { submit, content },
        )
        .expect("agents save failure should stay in-route and show a toast");

        assert_eq!(app.route, Route::ConfigOpenClawAgents);
        let toast = app
            .toast
            .as_ref()
            .expect("save failure should show a toast");
        assert_eq!(toast.kind, ToastKind::Error);
        assert!(
            toast
                .message
                .starts_with(texts::tui_toast_openclaw_agents_save_result(false)),
            "{}",
            toast.message
        );
        assert!(
            toast.message.contains("blocked-openclaw"),
            "{}",
            toast.message
        );
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_tools_save_failure_surfaces_upstream_copy() {
        let temp_home = TempDir::new().expect("create temp home");
        let blocked_path = temp_home.path().join("blocked-openclaw");
        std::fs::write(&blocked_path, "not a directory").expect("seed blocking file");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&blocked_path);

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
            profile: Some("coding".to_string()),
            allow: Vec::new(),
            deny: Vec::new(),
            extra: std::collections::HashMap::new(),
        });

        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        for ch in ['E', 'x', 'e', 'c'] {
            assert!(matches!(
                app.on_key(key(KeyCode::Char(ch)), &data),
                Action::None
            ));
        }
        let action = app.on_key(key(KeyCode::Enter), &data);
        let Action::EditorSubmit { submit, content } = action else {
            panic!("expected tools form save action, got {action:?}");
        };

        run_runtime_action(
            &mut app,
            &mut data,
            Action::EditorSubmit { submit, content },
        )
        .expect("tools save failure should stay in-route and show a toast");

        assert_eq!(app.route, Route::ConfigOpenClawTools);
        let toast = app
            .toast
            .as_ref()
            .expect("save failure should show a toast");
        assert_eq!(toast.kind, ToastKind::Error);
        assert!(
            toast
                .message
                .starts_with(texts::tui_toast_openclaw_tools_save_result(false)),
            "{}",
            toast.message
        );
        assert!(
            toast.message.contains("blocked-openclaw"),
            "{}",
            toast.message
        );
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_tools_typing_continues_across_failed_and_successful_auto_save() {
        let temp_home = TempDir::new().expect("create temp home");
        let valid_openclaw_dir = temp_home.path().join(".openclaw");
        let blocked_path = temp_home.path().join("blocked-openclaw");
        std::fs::create_dir_all(&valid_openclaw_dir).expect("create openclaw dir");
        std::fs::write(&blocked_path, "not a directory").expect("seed blocking file");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&valid_openclaw_dir);

        let set_openclaw_dir = |path: &Path| {
            let mut settings = get_settings();
            settings.openclaw_config_dir = Some(path.display().to_string());
            update_settings(settings).expect("update openclaw override dir");
        };

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let mut data = UiData::load(&AppType::OpenClaw).expect("load ui data");
        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
            profile: Some("coding".to_string()),
            allow: vec!["Read".to_string()],
            deny: Vec::new(),
            extra: std::collections::HashMap::new(),
        });

        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));

        assert!(matches!(
            app.on_key(key(KeyCode::Char('W')), &data),
            Action::None
        ));
        let first_action = app.on_key(key(KeyCode::Enter), &data);
        let Action::EditorSubmit { submit, content } = first_action else {
            panic!("expected first tools auto-save submit, got {first_action:?}");
        };

        set_openclaw_dir(&blocked_path);
        run_runtime_action(
            &mut app,
            &mut data,
            Action::EditorSubmit { submit, content },
        )
        .expect("failed tools auto-save should stay in-route");

        assert!(matches!(
            app.toast.as_ref(),
            Some(Toast {
                kind: ToastKind::Error,
                ..
            })
        ));
        assert_eq!(
            app.openclaw_tools_form.as_ref().map(|form| (
                form.section,
                form.row,
                form.allow.clone()
            )),
            Some((OpenClawToolsSection::Allow, 0, vec!["ReadW".to_string()]))
        );

        set_openclaw_dir(&valid_openclaw_dir);

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('r')), &data),
            Action::None
        ));
        let second_action = app.on_key(key(KeyCode::Enter), &data);
        let Action::EditorSubmit { submit, content } = second_action else {
            panic!("expected second tools auto-save submit, got {second_action:?}");
        };
        run_runtime_action(
            &mut app,
            &mut data,
            Action::EditorSubmit { submit, content },
        )
        .expect("successful tools auto-save should reload data");

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('i')), &data),
            Action::None
        ));
        let third_action = app.on_key(key(KeyCode::Enter), &data);
        let Action::EditorSubmit { submit, content } = third_action else {
            panic!("expected continued typing submit after auto-save, got {third_action:?}");
        };
        run_runtime_action(
            &mut app,
            &mut data,
            Action::EditorSubmit { submit, content },
        )
        .expect("continued typing after tools auto-save should keep working");

        assert_eq!(app.route, Route::ConfigOpenClawTools);
        assert!(
            app.toast.is_none(),
            "successful tools auto-save should clear stale error toast without showing success"
        );
        assert_eq!(
            app.openclaw_tools_form.as_ref().map(|form| (
                form.section,
                form.row,
                form.allow.clone()
            )),
            Some((OpenClawToolsSection::Allow, 0, vec!["ReadWri".to_string()]))
        );
        assert_eq!(
            data.config
                .openclaw_tools
                .as_ref()
                .map(|tools| tools.allow.clone()),
            Some(vec!["ReadWri".to_string()])
        );
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_agents_runtime_popup_edits_continue_across_failed_and_successful_auto_save() {
        let temp_home = TempDir::new().expect("create temp home");
        let valid_openclaw_dir = temp_home.path().join(".openclaw");
        let blocked_path = temp_home.path().join("blocked-openclaw");
        std::fs::create_dir_all(&valid_openclaw_dir).expect("create openclaw dir");
        std::fs::write(&blocked_path, "not a directory").expect("seed blocking file");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&valid_openclaw_dir);

        let set_openclaw_dir = |path: &Path| {
            let mut settings = get_settings();
            settings.openclaw_config_dir = Some(path.display().to_string());
            update_settings(settings).expect("update openclaw override dir");
        };

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::load(&AppType::OpenClaw).expect("load ui data");
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: None,
                models: None,
                extra: std::collections::HashMap::from([("workspace".to_string(), json!("work"))]),
            });
        app.openclaw_agents_form = Some(openclaw_agents_runtime_form(
            data.config.openclaw_agents_defaults.as_ref(),
            0,
        ));

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('x')), &data),
            Action::None
        ));

        let first_action = app.on_key(key(KeyCode::Enter), &data);
        let Action::EditorSubmit { submit, content } = first_action else {
            panic!("expected first agents popup submit, got {first_action:?}");
        };

        set_openclaw_dir(&blocked_path);
        run_runtime_action(
            &mut app,
            &mut data,
            Action::EditorSubmit { submit, content },
        )
        .expect("failed agents auto-save should stay in-route");

        assert!(matches!(
            app.toast.as_ref(),
            Some(Toast {
                kind: ToastKind::Error,
                ..
            })
        ));
        assert_eq!(
            app.openclaw_agents_form.as_ref().map(|form| (
                form.section,
                form.row,
                form.workspace.clone()
            )),
            Some((OpenClawAgentsSection::Runtime, 0, "workx".to_string()))
        );

        set_openclaw_dir(&valid_openclaw_dir);

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('y')), &data),
            Action::None
        ));

        let second_action = app.on_key(key(KeyCode::Enter), &data);
        let Action::EditorSubmit { submit, content } = second_action else {
            panic!("expected second agents popup submit, got {second_action:?}");
        };
        run_runtime_action(
            &mut app,
            &mut data,
            Action::EditorSubmit { submit, content },
        )
        .expect("successful agents auto-save should reload data");

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('z')), &data),
            Action::None
        ));

        let third_action = app.on_key(key(KeyCode::Enter), &data);
        let Action::EditorSubmit { submit, content } = third_action else {
            panic!("expected continued popup submit after auto-save, got {third_action:?}");
        };
        run_runtime_action(
            &mut app,
            &mut data,
            Action::EditorSubmit { submit, content },
        )
        .expect("continued typing after agents auto-save should keep working");

        assert_eq!(app.route, Route::ConfigOpenClawAgents);
        assert!(
            app.toast.is_none(),
            "successful agents auto-save should clear stale error toast without showing success"
        );
        assert_eq!(
            app.openclaw_agents_form.as_ref().map(|form| (
                form.section,
                form.row,
                form.workspace.clone()
            )),
            Some((OpenClawAgentsSection::Runtime, 0, "workxyz".to_string()))
        );
        assert_eq!(
            data.config
                .openclaw_agents_defaults
                .as_ref()
                .and_then(|defaults| defaults.extra.get("workspace")),
            Some(&json!("workxyz"))
        );
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_agents_save_is_blocked_for_real_malformed_agents_section_without_seeding_form() {
        let temp_home = TempDir::new().expect("create temp home");
        let openclaw_dir = temp_home.path().join(".openclaw");
        std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
        std::fs::write(
            openclaw_dir.join("openclaw.json"),
            r#"{
  agents: {
    defaults: 'broken-defaults',
  },
}"#,
        )
        .expect("write malformed openclaw config");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;
        let data = UiData::load(&AppType::OpenClaw).expect("load openclaw ui data");

        let action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(action, Action::None));
        assert!(app.openclaw_agents_form.is_none());
        assert!(app.editor.is_none());
        assert_eq!(
            app.toast
                .as_ref()
                .map(|toast| (toast.kind, toast.message.as_str())),
            Some((
                ToastKind::Error,
                texts::tui_toast_openclaw_agents_save_blocked_parse_error()
            ))
        );
    }

    #[test]
    fn openclaw_agents_save_is_blocked_when_legacy_timeout_value_cannot_migrate() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows = vec![openclaw_provider_row(
            "demo",
            "Demo Provider",
            &[("primary", "Primary")],
        )];
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: None,
                models: None,
                extra: std::collections::HashMap::from([(
                    "timeout".to_string(),
                    json!("manual-value"),
                )]),
            });

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(action, Action::None));
        assert!(app.editor.is_none());
        assert_eq!(
            app.toast
                .as_ref()
                .map(|toast| (toast.kind, toast.message.as_str())),
            Some((
                ToastKind::Error,
                texts::tui_toast_openclaw_agents_save_blocked_legacy_timeout()
            ))
        );
        assert_eq!(
            app.openclaw_agents_form
                .as_ref()
                .map(|form| form.primary_model.as_str()),
            Some("demo/primary")
        );

        assert!(matches!(
            app.on_back_key(),
            Action::SwitchRoute(Route::Main)
        ));
        assert!(app.openclaw_agents_form.is_none());

        assert!(matches!(
            app.push_route_and_switch(Route::ConfigOpenClawAgents),
            Action::SwitchRoute(Route::ConfigOpenClawAgents)
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));
        assert_eq!(
            app.openclaw_agents_form
                .as_ref()
                .map(|form| form.primary_model.as_str()),
            Some("")
        );
    }

    #[test]
    fn openclaw_tools_save_is_blocked_when_parse_warning_is_present() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
            profile: Some("coding".to_string()),
            allow: vec!["Read".to_string()],
            deny: Vec::new(),
            extra: std::collections::HashMap::new(),
        });
        data.config.openclaw_warnings = Some(vec![crate::openclaw_config::OpenClawHealthWarning {
            code: "config_parse_failed".to_string(),
            message:
                "Failed to parse tools config: invalid type: string \"Read\", expected a sequence"
                    .to_string(),
            path: Some("tools".to_string()),
        }]);

        let open_action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(open_action, Action::None));
        assert!(app.overlay.is_active());
        assert!(
            app.editor.is_none(),
            "tools route should not fall back to editor mode"
        );
        assert!(app.toast.is_none(), "opening picker should stay silent");

        assert!(matches!(
            app.on_key(key(KeyCode::Down), &data),
            Action::None
        ));

        let confirm_action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(confirm_action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        assert_eq!(
            app.toast
                .as_ref()
                .map(|toast| (toast.kind, toast.message.as_str())),
            Some((
                ToastKind::Error,
                texts::tui_toast_openclaw_tools_save_blocked_parse_error()
            ))
        );
        assert_eq!(
            app.openclaw_tools_form
                .as_ref()
                .and_then(|form| form.profile.as_deref()),
            Some("messaging")
        );
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_tools_save_is_blocked_for_real_malformed_tools_section_without_seeding_form() {
        let temp_home = TempDir::new().expect("create temp home");
        let openclaw_dir = temp_home.path().join(".openclaw");
        std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
        std::fs::write(
            openclaw_dir.join("openclaw.json"),
            r#"{
  tools: {
    profile: 'coding',
    allow: 'Read',
  },
}"#,
        )
        .expect("write malformed openclaw config");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawTools;
        app.focus = Focus::Content;
        let data = UiData::load(&AppType::OpenClaw).expect("load openclaw ui data");

        let action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(action, Action::None));
        assert!(app.openclaw_tools_form.is_none());
        assert!(app.editor.is_none());
        assert_eq!(
            app.toast
                .as_ref()
                .map(|toast| (toast.kind, toast.message.as_str())),
            Some((
                ToastKind::Error,
                texts::tui_toast_openclaw_tools_save_blocked_parse_error()
            ))
        );
    }

    #[test]
    fn main_proxy_action_starts_managed_session_for_current_app() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Main;

        let mut data = UiData::default();
        data.proxy.listen_address = "127.0.0.1".to_string();
        data.proxy.listen_port = 15721;

        let action = app.on_key(key(KeyCode::Char('p')), &data);
        assert!(matches!(
            action,
            Action::SetManagedProxyForCurrentApp {
                app_type: AppType::Claude,
                enabled: true,
            }
        ));
    }

    #[test]
    fn main_proxy_action_stops_and_restores_current_app_when_active() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Main;

        let mut data = UiData::default();
        data.proxy.running = true;
        data.proxy.claude_takeover = true;
        data.proxy.listen_address = "127.0.0.1".to_string();
        data.proxy.listen_port = 15721;

        let action = app.on_key(key(KeyCode::Char('p')), &data);
        assert!(matches!(
            action,
            Action::SetManagedProxyForCurrentApp {
                app_type: AppType::Claude,
                enabled: false,
            }
        ));
    }

    #[test]
    fn main_proxy_action_starts_current_app_when_proxy_is_running_for_another_app() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Main;

        let mut data = UiData::default();
        data.proxy.running = true;
        data.proxy.managed_runtime = true;
        data.proxy.codex_takeover = true;
        data.proxy.listen_address = "127.0.0.1".to_string();
        data.proxy.listen_port = 15721;

        let action = app.on_key(key(KeyCode::Char('p')), &data);
        assert!(matches!(
            action,
            Action::SetManagedProxyForCurrentApp {
                app_type: AppType::Claude,
                enabled: true,
            }
        ));
    }

    #[test]
    fn main_proxy_action_stays_disabled_when_only_foreground_runtime_is_running_elsewhere() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Main;

        let mut data = UiData::default();
        data.proxy.running = true;
        data.proxy.managed_runtime = false;
        data.proxy.codex_takeover = true;
        data.proxy.listen_address = "127.0.0.1".to_string();
        data.proxy.listen_port = 15721;

        let action = app.on_key(key(KeyCode::Char('p')), &data);
        assert!(matches!(action, Action::None));
    }

    #[test]
    fn proxy_help_overlay_t_starts_managed_proxy_when_stopped_for_current_app() {
        let mut app = App::new(Some(AppType::Claude));
        let data = UiData::default();

        app.open_proxy_help_view(&data, None);
        let action = app.on_key(key(KeyCode::Char('t')), &data);

        assert!(matches!(
            action,
            Action::SetManagedProxyForCurrentApp {
                app_type: AppType::Claude,
                enabled: true,
            }
        ));
    }

    #[test]
    fn proxy_help_overlay_hides_primary_action_when_foreground_runtime_is_running_elsewhere() {
        let mut app = App::new(Some(AppType::Claude));

        let mut data = UiData::default();
        data.proxy.running = true;
        data.proxy.managed_runtime = false;
        data.proxy.codex_takeover = true;

        app.open_proxy_help_view(&data, None);

        let Overlay::TextView(view) = &app.overlay else {
            panic!("expected proxy help overlay");
        };
        assert!(view.action.is_none());
        assert!(matches!(
            app.on_key(key(KeyCode::Char('t')), &data),
            Action::None
        ));
    }

    #[test]
    fn settings_menu_exposes_proxy_item() {
        assert!(
            SettingsItem::ALL
                .iter()
                .any(|item| matches!(item, SettingsItem::Proxy)),
            "Settings should expose a local proxy entry"
        );
    }

    #[test]
    fn settings_menu_exposes_visible_apps_item() {
        assert!(
            SettingsItem::ALL
                .iter()
                .any(|item| matches!(item, SettingsItem::VisibleApps)),
            "Settings should expose a visible apps entry"
        );
    }

    #[test]
    fn settings_menu_exposes_openclaw_config_dir_item() {
        assert!(
            SettingsItem::ALL
                .iter()
                .any(|item| matches!(item, SettingsItem::OpenClawConfigDir)),
            "Settings should expose an OpenClaw config directory entry"
        );
    }

    #[test]
    #[serial(home_settings)]
    fn settings_openclaw_config_dir_item_opens_text_input() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        let mut settings = crate::settings::get_settings();
        settings.openclaw_config_dir = Some(r"\\wsl$\Ubuntu\home\demo\.openclaw".to_string());
        crate::settings::update_settings(settings).expect("save openclaw override");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Settings;
        app.focus = Focus::Content;
        app.settings_idx = SettingsItem::ALL
            .iter()
            .position(|item| matches!(item, SettingsItem::OpenClawConfigDir))
            .expect("OpenClawConfigDir missing from SettingsItem::ALL");

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::SettingsOpenClawConfigDir,
                buffer,
                ..
            }) if buffer == r"\\wsl$\Ubuntu\home\demo\.openclaw"
        ));
    }

    #[test]
    fn settings_openclaw_config_dir_text_submit_emits_action() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Settings;
        app.focus = Focus::Content;

        app.overlay = Overlay::TextInput(TextInputState {
            title: "OpenClaw Config Directory".to_string(),
            prompt: "path".to_string(),
            buffer: r"\\wsl$\Ubuntu\home\demo\.openclaw".to_string(),
            submit: TextSubmit::SettingsOpenClawConfigDir,
            secret: false,
        });

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());
        assert!(matches!(
            action,
            Action::SetOpenClawConfigDir { path: Some(path) }
                if path == r"\\wsl$\Ubuntu\home\demo\.openclaw"
        ));

        app.overlay = Overlay::TextInput(TextInputState {
            title: "OpenClaw Config Directory".to_string(),
            prompt: "path".to_string(),
            buffer: "   ".to_string(),
            submit: TextSubmit::SettingsOpenClawConfigDir,
            secret: false,
        });

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());
        assert!(matches!(
            action,
            Action::SetOpenClawConfigDir { path: None }
        ));
    }

    #[test]
    #[serial(home_settings)]
    fn settings_visible_apps_item_opens_picker_overlay() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        let expected = crate::settings::get_visible_apps();

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Settings;
        app.focus = Focus::Content;
        app.settings_idx = SettingsItem::ALL
            .iter()
            .position(|item| matches!(item, SettingsItem::VisibleApps))
            .expect("VisibleApps missing from SettingsItem::ALL");

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::VisibleAppsPicker { selected, apps }
                if *selected == app_type_picker_index(&app.app_type) && apps == &expected
        ));
    }

    #[test]
    #[serial(home_settings)]
    fn visible_apps_picker_rejects_zero_selection_without_closing() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        crate::settings::set_visible_apps(crate::settings::VisibleApps {
            claude: true,
            codex: false,
            gemini: false,
            opencode: false,
            openclaw: false,
        })
        .expect("save visible apps");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Settings;
        app.focus = Focus::Content;
        app.overlay = Overlay::VisibleAppsPicker {
            selected: 0,
            apps: crate::settings::get_visible_apps(),
        };

        let data = UiData::default();
        let toggle_action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(toggle_action, Action::None));

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::VisibleAppsPicker { apps, .. }
                if !apps.claude
                    && !apps.codex
                    && !apps.gemini
                    && !apps.opencode
                    && !apps.openclaw
        ));
        assert!(matches!(
            app.toast.as_ref(),
            Some(Toast {
                message,
                kind: ToastKind::Warning,
                ..
            }) if message == texts::tui_toast_visible_apps_zero_selection_warning()
        ));
    }

    #[test]
    fn settings_proxy_item_opens_second_level_menu() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Settings;
        app.focus = Focus::Content;
        app.settings_idx = SettingsItem::ALL
            .iter()
            .position(|item| matches!(item, SettingsItem::Proxy))
            .expect("Proxy missing from SettingsItem::ALL");

        let data = UiData::default();
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::SwitchRoute(Route::SettingsProxy)));
        assert!(matches!(app.route, Route::SettingsProxy));
    }

    #[test]
    fn settings_proxy_submenu_address_opens_text_input() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::SettingsProxy;
        app.focus = Focus::Content;
        app.settings_proxy_idx = LocalProxySettingsItem::ALL
            .iter()
            .position(|item| matches!(item, LocalProxySettingsItem::ListenAddress))
            .expect("ListenAddress missing");

        let mut data = UiData::default();
        data.proxy.listen_address = "127.0.0.1".to_string();

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::SettingsProxyListenAddress,
                ..
            })
        ));
    }

    #[test]
    fn settings_proxy_submenu_port_opens_text_input() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::SettingsProxy;
        app.focus = Focus::Content;
        app.settings_proxy_idx = LocalProxySettingsItem::ALL
            .iter()
            .position(|item| matches!(item, LocalProxySettingsItem::ListenPort))
            .expect("ListenPort missing");

        let mut data = UiData::default();
        data.proxy.listen_port = 15721;

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::SettingsProxyListenPort,
                ..
            })
        ));
    }

    #[test]
    fn settings_proxy_submenu_does_not_open_editor_while_proxy_is_running() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::SettingsProxy;
        app.focus = Focus::Content;
        app.settings_proxy_idx = LocalProxySettingsItem::ALL
            .iter()
            .position(|item| matches!(item, LocalProxySettingsItem::ListenAddress))
            .expect("ListenAddress missing");

        let mut data = UiData::default();
        data.proxy.running = true;
        data.proxy.configured_listen_address = "127.0.0.1".to_string();
        data.proxy.configured_listen_port = 15721;

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        assert!(matches!(
            app.toast.as_ref(),
            Some(Toast {
                message,
                kind: ToastKind::Info,
                ..
            }) if message == "The local proxy is running. Stop it before editing listen address or port."
        ));
    }

    #[test]
    fn settings_proxy_text_submit_validates_and_emits_actions() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::SettingsProxy;
        app.focus = Focus::Content;

        app.overlay = Overlay::TextInput(TextInputState {
            title: "Listen Address".to_string(),
            prompt: "address".to_string(),
            buffer: "127.0.0.1".to_string(),
            submit: TextSubmit::SettingsProxyListenAddress,
            secret: false,
        });
        let data = UiData::default();
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            action,
            Action::SetProxyListenAddress { address } if address == "127.0.0.1"
        ));

        app.overlay = Overlay::TextInput(TextInputState {
            title: "Listen Port".to_string(),
            prompt: "port".to_string(),
            buffer: "15721".to_string(),
            submit: TextSubmit::SettingsProxyListenPort,
            secret: false,
        });
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            action,
            Action::SetProxyListenPort { port } if port == 15721
        ));
    }

    #[test]
    fn settings_proxy_text_submit_invalid_input_keeps_prompt_open() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::SettingsProxy;
        app.focus = Focus::Content;

        app.overlay = Overlay::TextInput(TextInputState {
            title: "Listen Address".to_string(),
            prompt: "address".to_string(),
            buffer: "bad host".to_string(),
            submit: TextSubmit::SettingsProxyListenAddress,
            secret: false,
        });
        let data = UiData::default();
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::SettingsProxyListenAddress,
                ..
            })
        ));

        app.overlay = Overlay::TextInput(TextInputState {
            title: "Listen Port".to_string(),
            prompt: "port".to_string(),
            buffer: "80".to_string(),
            submit: TextSubmit::SettingsProxyListenPort,
            secret: false,
        });
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::SettingsProxyListenPort,
                ..
            })
        ));
    }

    #[test]
    fn settings_proxy_text_submit_is_blocked_if_proxy_starts_running_before_confirm() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::SettingsProxy;
        app.focus = Focus::Content;
        app.overlay = Overlay::TextInput(TextInputState {
            title: "Listen Address".to_string(),
            prompt: "address".to_string(),
            buffer: "127.0.0.1".to_string(),
            submit: TextSubmit::SettingsProxyListenAddress,
            secret: false,
        });

        let mut data = UiData::default();
        data.proxy.running = true;

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        assert!(matches!(
            app.toast.as_ref(),
            Some(Toast {
                message,
                kind: ToastKind::Info,
                ..
            }) if message == "The local proxy is running. Stop it before editing listen address or port."
        ));
    }

    #[test]
    fn config_webdav_settings_opens_json_editor_in_second_level_menu() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::ConfigWebDav;
        app.focus = Focus::Content;
        app.config_webdav_idx = WebDavConfigItem::ALL
            .iter()
            .position(|item| matches!(item, WebDavConfigItem::Settings))
            .expect("Settings missing from WebDavConfigItem::ALL");

        let mut data = UiData::default();
        data.config.webdav_sync = Some(crate::settings::WebDavSyncSettings {
            enabled: true,
            base_url: "https://dav.example.com".to_string(),
            ..crate::settings::WebDavSyncSettings::default()
        });

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.editor.as_ref().map(|e| &e.submit),
            Some(EditorSubmit::ConfigWebDavSettings)
        ));
    }

    #[test]
    fn config_webdav_submenu_items_emit_expected_actions() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::ConfigWebDav;
        app.focus = Focus::Content;
        let data = UiData::default();

        let check_idx = WebDavConfigItem::ALL
            .iter()
            .position(|item| matches!(item, WebDavConfigItem::CheckConnection))
            .expect("WebDavCheckConnection missing");
        app.config_webdav_idx = check_idx;
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::ConfigWebDavCheckConnection
        ));

        let upload_idx = WebDavConfigItem::ALL
            .iter()
            .position(|item| matches!(item, WebDavConfigItem::Upload))
            .expect("WebDavUpload missing");
        app.config_webdav_idx = upload_idx;
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::ConfigWebDavUpload
        ));

        let download_idx = WebDavConfigItem::ALL
            .iter()
            .position(|item| matches!(item, WebDavConfigItem::Download))
            .expect("WebDavDownload missing");
        app.config_webdav_idx = download_idx;
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::ConfigWebDavDownload
        ));

        let reset_idx = WebDavConfigItem::ALL
            .iter()
            .position(|item| matches!(item, WebDavConfigItem::Reset))
            .expect("WebDavReset missing");
        app.config_webdav_idx = reset_idx;
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::ConfigWebDavReset
        ));

        assert_eq!(
            WebDavConfigItem::ALL.len(),
            6,
            "WebDav submenu should include Jianguoyun quick setup"
        );
    }

    #[test]
    fn config_webdav_quick_setup_requires_username_then_password() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::ConfigWebDav;
        app.focus = Focus::Content;

        let quick_setup_idx = WebDavConfigItem::ALL
            .iter()
            .position(|item| matches!(item, WebDavConfigItem::JianguoyunQuickSetup))
            .expect("JianguoyunQuickSetup missing");
        app.config_webdav_idx = quick_setup_idx;

        let data = UiData::default();
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::WebDavJianguoyunUsername,
                ..
            })
        ));

        if let Overlay::TextInput(ref mut input) = app.overlay {
            input.buffer = "demo@nutstore.com".to_string();
        }
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::WebDavJianguoyunPassword,
                secret: true,
                ..
            })
        ));

        if let Overlay::TextInput(ref mut input) = app.overlay {
            input.buffer = "app-password".to_string();
        }
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            action,
            Action::ConfigWebDavJianguoyunQuickSetup {
                username,
                password
            } if username == "demo@nutstore.com" && password == "app-password"
        ));
    }

    #[test]
    fn config_webdav_quick_setup_empty_inputs_keep_prompt_open() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::ConfigWebDav;
        app.focus = Focus::Content;

        let quick_setup_idx = WebDavConfigItem::ALL
            .iter()
            .position(|item| matches!(item, WebDavConfigItem::JianguoyunQuickSetup))
            .expect("JianguoyunQuickSetup missing");
        app.config_webdav_idx = quick_setup_idx;

        let data = UiData::default();
        let _ = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::WebDavJianguoyunUsername,
                ..
            })
        ));

        if let Overlay::TextInput(ref mut input) = app.overlay {
            input.buffer = "   ".to_string();
        }
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::WebDavJianguoyunUsername,
                ..
            })
        ));

        if let Overlay::TextInput(ref mut input) = app.overlay {
            input.buffer = "demo@nutstore.com".to_string();
        }
        let _ = app.on_key(key(KeyCode::Enter), &data);
        if let Overlay::TextInput(ref mut input) = app.overlay {
            input.buffer = "   ".to_string();
        }
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::WebDavJianguoyunPassword,
                secret: true,
                ..
            })
        ));
    }

    #[test]
    fn prompts_e_opens_editor_and_ctrl_s_submits() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.prompts.rows.push(super::super::data::PromptRow {
            id: "pr1".to_string(),
            prompt: crate::prompt::Prompt {
                id: "pr1".to_string(),
                name: "Demo".to_string(),
                content: "hello".to_string(),
                description: None,
                enabled: false,
                created_at: None,
                updated_at: None,
            },
        });

        let action = app.on_key(key(KeyCode::Char('e')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.editor.as_ref().map(|e| &e.submit),
            Some(EditorSubmit::PromptEdit { id }) if id == "pr1"
        ));

        let submit = app.on_key(ctrl(KeyCode::Char('s')), &data);
        assert!(matches!(
            submit,
            Action::EditorSubmit {
                submit: EditorSubmit::PromptEdit { .. },
                content
            } if content.contains("hello")
        ));
    }

    #[test]
    fn prompts_editor_ctrl_shift_s_submits() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.prompts.rows.push(super::super::data::PromptRow {
            id: "pr1".to_string(),
            prompt: crate::prompt::Prompt {
                id: "pr1".to_string(),
                name: "Demo".to_string(),
                content: "hello".to_string(),
                description: None,
                enabled: false,
                created_at: None,
                updated_at: None,
            },
        });

        let action = app.on_key(key(KeyCode::Char('e')), &data);
        assert!(matches!(action, Action::None));
        let submit = app.on_key(
            KeyEvent::new(KeyCode::Char('S'), KeyModifiers::CONTROL),
            &data,
        );
        assert!(
            matches!(
                submit,
                Action::EditorSubmit {
                    submit: EditorSubmit::PromptEdit { .. },
                    ..
                }
            ),
            "Ctrl+Shift+S should be accepted as save shortcut in editor"
        );
    }

    #[test]
    fn prompts_editor_ctrl_s_control_char_submits() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.prompts.rows.push(super::super::data::PromptRow {
            id: "pr1".to_string(),
            prompt: crate::prompt::Prompt {
                id: "pr1".to_string(),
                name: "Demo".to_string(),
                content: "hello".to_string(),
                description: None,
                enabled: false,
                created_at: None,
                updated_at: None,
            },
        });

        let action = app.on_key(key(KeyCode::Char('e')), &data);
        assert!(matches!(action, Action::None));
        let submit = app.on_key(key(KeyCode::Char('\u{13}')), &data);
        assert!(
            matches!(
                submit,
                Action::EditorSubmit {
                    submit: EditorSubmit::PromptEdit { .. },
                    ..
                }
            ),
            "ASCII XOFF control char should be accepted as save shortcut in editor"
        );
    }

    #[test]
    fn prompts_editor_ctrl_o_requests_external_editor() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.prompts.rows.push(super::super::data::PromptRow {
            id: "pr1".to_string(),
            prompt: crate::prompt::Prompt {
                id: "pr1".to_string(),
                name: "Demo".to_string(),
                content: "hello".to_string(),
                description: None,
                enabled: false,
                created_at: None,
                updated_at: None,
            },
        });

        let action = app.on_key(key(KeyCode::Char('e')), &data);
        assert!(matches!(action, Action::None));
        assert!(app.editor.is_some(), "prompt editor should be opened first");

        let action = app.on_key(ctrl(KeyCode::Char('o')), &data);
        assert_eq!(format!("{action:?}"), "EditorOpenExternal");
        assert!(
            app.editor.is_some(),
            "Ctrl+O should keep the editor session open"
        );
    }

    #[test]
    fn prompts_editor_esc_dirty_opens_save_before_close_confirm() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.prompts.rows.push(super::super::data::PromptRow {
            id: "pr1".to_string(),
            prompt: crate::prompt::Prompt {
                id: "pr1".to_string(),
                name: "Demo".to_string(),
                content: "hello".to_string(),
                description: None,
                enabled: false,
                created_at: None,
                updated_at: None,
            },
        });

        app.on_key(key(KeyCode::Char('e')), &data);
        app.on_key(key(KeyCode::Char('x')), &data);
        let action = app.on_key(key(KeyCode::Esc), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::EditorSaveBeforeClose,
                ..
            })
        ));
    }

    #[test]
    fn prompts_editor_save_confirm_yes_submits_changes() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.prompts.rows.push(super::super::data::PromptRow {
            id: "pr1".to_string(),
            prompt: crate::prompt::Prompt {
                id: "pr1".to_string(),
                name: "Demo".to_string(),
                content: "hello".to_string(),
                description: None,
                enabled: false,
                created_at: None,
                updated_at: None,
            },
        });

        app.on_key(key(KeyCode::Char('e')), &data);
        app.on_key(key(KeyCode::Char('x')), &data);
        app.on_key(key(KeyCode::Esc), &data);

        let action = app.on_key(key(KeyCode::Char('y')), &data);
        assert!(
            matches!(
                action,
                Action::EditorSubmit {
                    submit: EditorSubmit::PromptEdit { .. },
                    content
                } if content.starts_with("xhello")
            ),
            "confirm yes should save current editor content"
        );
    }

    #[test]
    fn prompts_editor_save_confirm_no_discards_and_closes() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.prompts.rows.push(super::super::data::PromptRow {
            id: "pr1".to_string(),
            prompt: crate::prompt::Prompt {
                id: "pr1".to_string(),
                name: "Demo".to_string(),
                content: "hello".to_string(),
                description: None,
                enabled: false,
                created_at: None,
                updated_at: None,
            },
        });

        app.on_key(key(KeyCode::Char('e')), &data);
        app.on_key(key(KeyCode::Char('x')), &data);
        app.on_key(key(KeyCode::Esc), &data);

        let action = app.on_key(key(KeyCode::Char('n')), &data);
        assert!(matches!(action, Action::None));
        assert!(
            app.editor.is_none(),
            "confirm no should discard and close editor"
        );
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn providers_e_opens_edit_form_and_ctrl_s_submits() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: None,
            default_model_id: None,
        });

        let action = app.on_key(key(KeyCode::Char('e')), &data);
        assert!(matches!(action, Action::None));
        assert!(app.editor.is_none());
        assert!(app.form.is_some());

        let submit = app.on_key(ctrl(KeyCode::Char('s')), &data);
        assert!(matches!(
            submit,
            Action::EditorSubmit {
                submit: EditorSubmit::ProviderEdit { .. },
                content
            } if content.contains("\"id\"") && content.contains("Provider One")
        ));
    }

    #[test]
    fn provider_edit_form_tab_cycles_between_fields_and_json() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: None,
            default_model_id: None,
        });

        app.on_key(key(KeyCode::Char('e')), &data);

        let focus = match app.form.as_ref() {
            Some(super::super::form::FormState::ProviderAdd(form)) => form.focus,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(focus, super::super::form::FormFocus::Fields);

        app.on_key(key(KeyCode::Tab), &data);
        let focus = match app.form.as_ref() {
            Some(super::super::form::FormState::ProviderAdd(form)) => form.focus,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(focus, super::super::form::FormFocus::JsonPreview);

        app.on_key(key(KeyCode::Tab), &data);
        let focus = match app.form.as_ref() {
            Some(super::super::form::FormState::ProviderAdd(form)) => form.focus,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(focus, super::super::form::FormFocus::Fields);
    }

    #[test]
    fn providers_a_opens_add_form() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        let action = app.on_key(key(KeyCode::Char('a')), &data);
        assert!(matches!(action, Action::None));
        assert!(
            app.editor.is_none(),
            "Providers 'a' should open the new add form (not the JSON editor)"
        );

        let submit = app.on_key(ctrl(KeyCode::Char('s')), &data);
        assert!(
            !matches!(submit, Action::EditorSubmit { .. }),
            "Provider add form should validate fields before submitting"
        );
    }

    #[test]
    fn provider_add_form_ctrl_s_generates_hidden_id_from_name_before_submit() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.name.set("Provider One");
            form.id.set("");
        } else {
            panic!("expected ProviderAdd form");
        }

        let submit = app.on_key(ctrl(KeyCode::Char('s')), &data);
        let Action::EditorSubmit { submit, content } = submit else {
            panic!("Ctrl+S should submit when name is present");
        };

        assert!(matches!(submit, EditorSubmit::ProviderAdd));
        assert!(
            content.contains("\"id\": \"provider-one\""),
            "save should auto-generate an id from name before submit"
        );
        assert!(content.contains("\"name\": \"Provider One\""));
    }

    #[test]
    fn provider_add_form_missing_fields_toast_mentions_name_only() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);

        let submit = app.on_key(ctrl(KeyCode::Char('s')), &data);
        assert!(matches!(submit, Action::None));
        let Some(Toast {
            message,
            kind: ToastKind::Warning,
            ..
        }) = app.toast.as_ref()
        else {
            panic!("expected warning toast for missing add-form fields");
        };
        assert!(message.contains("name"));
        assert!(message.contains("generated automatically"));
        assert!(!message.contains("id and name"));
        assert!(!message.contains("in JSON"));
    }

    #[test]
    fn provider_add_form_codex_requires_base_url_before_submit() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.name.set("Codex Provider");
            form.codex_base_url.set("");
        } else {
            panic!("expected ProviderAdd form");
        }

        let submit = app.on_key(ctrl(KeyCode::Char('s')), &data);
        assert!(matches!(submit, Action::None));
        assert!(matches!(
            app.toast.as_ref(),
            Some(Toast {
                kind: ToastKind::Warning,
                message,
                ..
            }) if message == texts::base_url_empty_error()
        ));
    }

    #[test]
    fn provider_add_form_ctrl_s_rejects_name_that_cannot_generate_id() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.name.set("!!!");
            form.id.set("");
        } else {
            panic!("expected ProviderAdd form");
        }

        let submit = app.on_key(ctrl(KeyCode::Char('s')), &data);
        assert!(matches!(submit, Action::None));
        assert!(matches!(
            app.toast.as_ref(),
            Some(Toast {
                kind: ToastKind::Warning,
                ..
            })
        ));
    }

    #[test]
    fn provider_add_form_tab_cycles_focus() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);

        let focus = match app.form.as_ref() {
            Some(super::super::form::FormState::ProviderAdd(form)) => form.focus,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(focus, super::super::form::FormFocus::Templates);

        app.on_key(key(KeyCode::Tab), &data);
        let focus = match app.form.as_ref() {
            Some(super::super::form::FormState::ProviderAdd(form)) => form.focus,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(focus, super::super::form::FormFocus::Fields);
    }

    #[test]
    fn provider_add_form_right_moves_template_selection() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);

        let idx = match app.form.as_ref() {
            Some(super::super::form::FormState::ProviderAdd(form)) => form.template_idx,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(idx, 0);

        app.on_key(key(KeyCode::Right), &data);
        let idx = match app.form.as_ref() {
            Some(super::super::form::FormState::ProviderAdd(form)) => form.template_idx,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(idx, 1);
    }

    #[test]
    fn provider_add_form_enter_applies_template_and_focuses_fields() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(app.editor.is_none());
        let focus = match app.form.as_ref() {
            Some(super::super::form::FormState::ProviderAdd(form)) => form.focus,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(focus, super::super::form::FormFocus::Fields);
    }

    #[test]
    fn provider_form_esc_dirty_opens_save_before_close_confirm() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.name.set("Provider One");
        } else {
            panic!("expected ProviderAdd form");
        }

        let action = app.on_key(key(KeyCode::Esc), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                title,
                message,
                action: ConfirmAction::FormSaveBeforeClose,
                ..
            }) if title == texts::tui_editor_save_before_close_title()
                && message == texts::tui_editor_save_before_close_message()
        ));
        assert!(matches!(app.form, Some(FormState::ProviderAdd(_))));
    }

    #[test]
    fn provider_form_q_dirty_opens_save_before_close_confirm() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.name.set("Provider One");
        } else {
            panic!("expected ProviderAdd form");
        }

        let action = app.on_key(key(KeyCode::Char('q')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                title,
                message,
                action: ConfirmAction::FormSaveBeforeClose,
                ..
            }) if title == texts::tui_editor_save_before_close_title()
                && message == texts::tui_editor_save_before_close_message()
        ));
        assert!(matches!(app.form, Some(FormState::ProviderAdd(_))));
    }

    #[test]
    fn provider_edit_form_esc_dirty_opens_save_before_close_confirm() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: None,
            default_model_id: None,
        });

        app.on_key(key(KeyCode::Char('e')), &data);
        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.name.set("Provider Renamed");
        } else {
            panic!("expected ProviderAdd form in edit mode");
        }

        let action = app.on_key(key(KeyCode::Esc), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                title,
                message,
                action: ConfirmAction::FormSaveBeforeClose,
                ..
            }) if title == texts::tui_editor_save_before_close_title()
                && message == texts::tui_editor_save_before_close_message()
        ));
        assert!(matches!(app.form, Some(FormState::ProviderAdd(_))));
    }

    #[test]
    fn provider_edit_form_save_confirm_enter_submits_provider_edit_action() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: None,
            default_model_id: None,
        });

        app.on_key(key(KeyCode::Char('e')), &data);
        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.name.set("Provider Renamed");
        } else {
            panic!("expected ProviderAdd form in edit mode");
        }

        app.on_key(key(KeyCode::Esc), &data);
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            action,
            Action::EditorSubmit {
                submit: EditorSubmit::ProviderEdit { id },
                content
            } if id == "p1" && content.contains("\"name\": \"Provider Renamed\"")
        ));
        assert!(matches!(app.form, Some(FormState::ProviderAdd(_))));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn provider_form_esc_clean_closes_without_confirm() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);

        let action = app.on_key(key(KeyCode::Esc), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        assert!(app.form.is_none(), "Esc should close clean provider form");
    }

    #[test]
    fn mcp_form_esc_dirty_opens_save_before_close_confirm() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Mcp;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);

        if let Some(super::super::form::FormState::McpAdd(form)) = app.form.as_mut() {
            form.id.set("m1");
        } else {
            panic!("expected McpAdd form");
        }

        let action = app.on_key(key(KeyCode::Esc), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                title,
                message,
                action: ConfirmAction::FormSaveBeforeClose,
                ..
            }) if title == texts::tui_editor_save_before_close_title()
                && message == texts::tui_editor_save_before_close_message()
        ));
        assert!(matches!(app.form, Some(FormState::McpAdd(_))));
    }

    #[test]
    fn mcp_edit_form_esc_dirty_opens_save_before_close_confirm() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Mcp;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.mcp.rows.push(super::super::data::McpRow {
            id: "m1".to_string(),
            server: crate::app_config::McpServer {
                id: "m1".to_string(),
                name: "Server".to_string(),
                server: json!({"command":"foo","args":[]}),
                apps: crate::app_config::McpApps::default(),
                description: None,
                homepage: None,
                docs: None,
                tags: vec![],
            },
        });

        app.on_key(key(KeyCode::Char('e')), &data);
        if let Some(super::super::form::FormState::McpAdd(form)) = app.form.as_mut() {
            form.name.set("Server Renamed");
        } else {
            panic!("expected McpAdd form in edit mode");
        }

        let action = app.on_key(key(KeyCode::Esc), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                title,
                message,
                action: ConfirmAction::FormSaveBeforeClose,
                ..
            }) if title == texts::tui_editor_save_before_close_title()
                && message == texts::tui_editor_save_before_close_message()
        ));
        assert!(matches!(app.form, Some(FormState::McpAdd(_))));
    }

    #[test]
    fn mcp_edit_form_save_confirm_enter_submits_mcp_edit_action() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Mcp;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.mcp.rows.push(super::super::data::McpRow {
            id: "m1".to_string(),
            server: crate::app_config::McpServer {
                id: "m1".to_string(),
                name: "Server".to_string(),
                server: json!({"command":"foo","args":[]}),
                apps: crate::app_config::McpApps::default(),
                description: None,
                homepage: None,
                docs: None,
                tags: vec![],
            },
        });

        app.on_key(key(KeyCode::Char('e')), &data);
        if let Some(super::super::form::FormState::McpAdd(form)) = app.form.as_mut() {
            form.name.set("Server Renamed");
        } else {
            panic!("expected McpAdd form in edit mode");
        }

        app.on_key(key(KeyCode::Esc), &data);
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            action,
            Action::EditorSubmit {
                submit: EditorSubmit::McpEdit { id },
                content
            } if id == "m1" && content.contains("\"name\": \"Server Renamed\"")
        ));
        assert!(matches!(app.form, Some(FormState::McpAdd(_))));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn mcp_form_esc_clean_closes_without_confirm() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Mcp;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);

        let action = app.on_key(key(KeyCode::Esc), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        assert!(app.form.is_none(), "Esc should close clean MCP form");
    }

    #[test]
    fn provider_form_save_confirm_enter_submits_form_save_action() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.name.set("Provider One");
        } else {
            panic!("expected ProviderAdd form");
        }

        app.on_key(key(KeyCode::Esc), &data);
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            action,
            Action::EditorSubmit {
                submit: EditorSubmit::ProviderAdd,
                content
            } if content.contains("\"name\": \"Provider One\"")
        ));
        assert!(matches!(app.form, Some(FormState::ProviderAdd(_))));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn provider_form_save_confirm_n_discards_and_closes() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.name.set("Provider One");
        } else {
            panic!("expected ProviderAdd form");
        }

        app.on_key(key(KeyCode::Esc), &data);
        let action = app.on_key(key(KeyCode::Char('n')), &data);
        assert!(matches!(action, Action::None));
        assert!(app.form.is_none(), "confirm N should close without saving");
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn provider_form_save_confirm_esc_cancels_and_preserves_edits() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.name.set("Provider One");
        } else {
            panic!("expected ProviderAdd form");
        }

        app.on_key(key(KeyCode::Esc), &data);
        let action = app.on_key(key(KeyCode::Esc), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.form.as_ref(),
            Some(FormState::ProviderAdd(form)) if form.name.value == "Provider One"
        ));
        assert!(matches!(app.overlay, Overlay::None));

        let second_exit = app.on_key(key(KeyCode::Esc), &data);
        assert!(matches!(second_exit, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::FormSaveBeforeClose,
                ..
            })
        ));
    }

    #[test]
    fn mcp_form_save_confirm_enter_submits_form_save_action() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Mcp;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);

        if let Some(super::super::form::FormState::McpAdd(form)) = app.form.as_mut() {
            form.id.set("m1");
            form.name.set("MCP One");
            form.command.set("node");
        } else {
            panic!("expected McpAdd form");
        }

        app.on_key(key(KeyCode::Esc), &data);
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            action,
            Action::EditorSubmit {
                submit: EditorSubmit::McpAdd,
                content
            } if content.contains("\"id\": \"m1\"")
                && content.contains("\"name\": \"MCP One\"")
        ));
        assert!(matches!(app.form, Some(FormState::McpAdd(_))));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn mcp_form_save_confirm_n_discards_and_closes() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Mcp;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);

        if let Some(super::super::form::FormState::McpAdd(form)) = app.form.as_mut() {
            form.id.set("m1");
        } else {
            panic!("expected McpAdd form");
        }

        app.on_key(key(KeyCode::Esc), &data);
        let action = app.on_key(key(KeyCode::Char('n')), &data);
        assert!(matches!(action, Action::None));
        assert!(app.form.is_none(), "confirm N should close MCP form");
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn provider_add_form_json_focus_enter_opens_json_editor() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields
        app.on_key(key(KeyCode::Tab), &data); // fields -> json

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(
            app.editor.is_some(),
            "Enter on provider JSON preview should open in-app JSON editor"
        );
        assert!(matches!(
            app.editor.as_ref().map(|editor| &editor.submit),
            Some(EditorSubmit::ProviderFormApplyJson)
        ));
        assert!(
            matches!(
                app.editor.as_ref().map(|editor| editor.mode),
                Some(EditorMode::Edit)
            ),
            "Enter on provider JSON preview should directly enter edit mode"
        );
        let content = app
            .editor
            .as_ref()
            .map(|editor| editor.text())
            .unwrap_or_default();
        assert!(
            !content.contains("\"id\""),
            "provider id should not be exposed in settingsConfig JSON editor"
        );
        assert!(
            !content.contains("\"name\""),
            "provider name should not be exposed in settingsConfig JSON editor"
        );
    }

    #[test]
    fn provider_json_editor_single_enter_then_ctrl_s_submits_edited_content() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields
        app.on_key(key(KeyCode::Tab), &data); // fields -> json
        app.on_key(key(KeyCode::Enter), &data); // json -> editor(edit mode)

        let original = app
            .editor
            .as_ref()
            .map(|editor| editor.text())
            .expect("editor should be opened");
        assert!(!original.starts_with(' '));

        // Edit immediately (without pressing Enter again) then submit.
        app.on_key(key(KeyCode::Char(' ')), &data);
        let submit = app.on_key(ctrl(KeyCode::Char('s')), &data);

        let Action::EditorSubmit { submit, content } = submit else {
            panic!("Ctrl+S in JSON editor should submit edited content");
        };
        assert!(
            matches!(submit, EditorSubmit::ProviderFormApplyJson),
            "JSON editor submit should apply back to provider form"
        );
        assert!(
            content.starts_with(' '),
            "submitted content should include the in-editor change made right after opening"
        );
    }

    #[test]
    fn provider_json_editor_ctrl_s_applies_unknown_fields_back_to_form() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields
        app.on_key(key(KeyCode::Tab), &data); // fields -> json
        app.on_key(key(KeyCode::Enter), &data); // json -> editor

        // Replace the whole JSON with a value that contains an unknown key inside settingsConfig.
        let injected = r#"{
  "env": {
    "ANTHROPIC_BASE_URL": "https://after.example"
  },
  "unknownField": "kept"
}"#;
        if let Some(editor) = app.editor.as_mut() {
            editor.lines = injected.lines().map(|s| s.to_string()).collect();
            editor.cursor_row = 0;
            editor.cursor_col = 0;
            editor.scroll = 0;
        } else {
            panic!("expected editor to be open");
        }

        let submit = app.on_key(ctrl(KeyCode::Char('s')), &data);
        let Action::EditorSubmit { submit, content } = submit else {
            panic!("expected EditorSubmit action");
        };
        assert!(matches!(submit, EditorSubmit::ProviderFormApplyJson));

        // Simulate main-loop handling of the submit to apply it back to the form.
        let settings_value: serde_json::Value = serde_json::from_str(&content).expect("valid json");
        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            let mut provider_value = form.to_provider_json_value();
            if let Some(obj) = provider_value.as_object_mut() {
                obj.insert("settingsConfig".to_string(), settings_value);
            }
            form.apply_provider_json_value_to_fields(provider_value)
                .expect("apply should succeed");
        } else {
            panic!("expected ProviderAdd form");
        }
        app.editor = None;

        // Re-open the JSON editor and ensure the unknown field is still present.
        app.on_key(key(KeyCode::Enter), &data);
        let reopened = app
            .editor
            .as_ref()
            .map(|editor| editor.text())
            .unwrap_or_default();
        assert!(
            reopened.contains("\"unknownField\""),
            "unknownField should be preserved after applying JSON back to form"
        );
        assert!(
            reopened.contains("\"kept\""),
            "unknownField value should be preserved after applying JSON back to form"
        );
    }

    #[test]
    fn provider_form_ctrl_s_does_not_merge_common_snippet_for_claude() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.common_snippet = r#"{"alwaysThinkingEnabled":false,"statusLine":{"type":"command","command":"~/.claude/statusline.sh","padding":0}}"#.to_string();

        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.id.set("p1");
            form.name.set("Provider One");
            form.include_common_config = true;
            form.claude_base_url.set("https://api.example.com");
        } else {
            panic!("expected ProviderAdd form");
        }

        let submit = app.on_key(ctrl(KeyCode::Char('s')), &data);
        assert!(matches!(submit, Action::EditorSubmit { .. }));
        let Action::EditorSubmit { content, .. } = submit else {
            unreachable!("expected submit action");
        };
        assert!(
            !content.contains("\"alwaysThinkingEnabled\""),
            "submitted provider JSON should keep common snippet keys out of the raw payload"
        );
        assert!(
            !content.contains("\"statusLine\""),
            "submitted provider JSON should keep nested common snippet keys out of the raw payload"
        );
        assert!(
            content.contains("\"ANTHROPIC_BASE_URL\""),
            "submitted provider JSON should still include provider-specific settings"
        );
    }

    #[test]
    fn provider_form_ctrl_s_does_not_merge_common_snippet_for_codex() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.common_snippet = "network_access = true".to_string();

        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.id.set("p1");
            form.name.set("Provider One");
            form.include_common_config = true;
            form.codex_base_url.set("https://api.example.com/v1");
        } else {
            panic!("expected ProviderAdd form");
        }

        let submit = app.on_key(ctrl(KeyCode::Char('s')), &data);
        assert!(matches!(submit, Action::EditorSubmit { .. }));
        let Action::EditorSubmit { content, .. } = submit else {
            unreachable!("expected submit action");
        };
        assert!(
            !content.contains("network_access"),
            "submitted Codex provider JSON should not include merged common snippet TOML"
        );
    }

    #[test]
    fn provider_claude_model_config_field_enter_opens_overlay() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data);

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.editing = false;
            form.field_idx = form
                .fields()
                .iter()
                .position(|field| *field == ProviderAddField::ClaudeModelConfig)
                .expect("ClaudeModelConfig field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::ClaudeModelPicker {
                selected: 0,
                editing: false
            }
        ));
    }

    #[test]
    fn claude_model_overlay_editing_updates_form_value() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data);

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.editing = false;
            form.field_idx = form
                .fields()
                .iter()
                .position(|field| *field == ProviderAddField::ClaudeModelConfig)
                .expect("ClaudeModelConfig field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        app.on_key(key(KeyCode::Enter), &data);
        app.on_key(key(KeyCode::Char(' ')), &data); // enter editing mode in overlay
        app.on_key(key(KeyCode::Char('m')), &data);
        app.on_key(key(KeyCode::Char('1')), &data);

        let model = match app.form.as_ref() {
            Some(super::super::form::FormState::ProviderAdd(form)) => {
                form.claude_model.value.clone()
            }
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(model, "m1");
    }

    #[test]
    fn claude_model_overlay_esc_closes_without_exiting_parent_form() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data);

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.editing = false;
            form.field_idx = form
                .fields()
                .iter()
                .position(|field| *field == ProviderAddField::ClaudeModelConfig)
                .expect("ClaudeModelConfig field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(app.overlay, Overlay::ClaudeModelPicker { .. }));

        let action = app.on_key(key(KeyCode::Esc), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        assert!(matches!(app.form, Some(FormState::ProviderAdd(_))));
    }

    #[test]
    fn update_available_overlay_left_right_switches_selection() {
        let mut app = App::new(None);
        app.overlay = Overlay::UpdateAvailable {
            current: "4.7.0".to_string(),
            latest: "v9.9.9".to_string(),
            selected: 0,
        };

        let action = app.on_key(key(KeyCode::Right), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::UpdateAvailable { selected: 1, .. }
        ));

        let action = app.on_key(key(KeyCode::Left), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::UpdateAvailable { selected: 0, .. }
        ));
    }

    #[test]
    fn update_available_overlay_up_down_does_not_switch_selection() {
        let mut app = App::new(None);
        app.overlay = Overlay::UpdateAvailable {
            current: "4.7.0".to_string(),
            latest: "v9.9.9".to_string(),
            selected: 0,
        };

        let action = app.on_key(key(KeyCode::Down), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::UpdateAvailable { selected: 0, .. }
        ));

        let action = app.on_key(key(KeyCode::Up), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::UpdateAvailable { selected: 0, .. }
        ));
    }

    #[test]
    fn update_check_loading_overlay_esc_emits_cancel_action() {
        let mut app = App::new(None);
        app.overlay = Overlay::Loading {
            kind: LoadingKind::UpdateCheck,
            title: texts::tui_update_checking_title().to_string(),
            message: "Working...".to_string(),
        };

        let action = app.on_key(key(KeyCode::Esc), &data());
        assert!(matches!(action, Action::CancelUpdateCheck));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn update_result_overlay_success_esc_hides_without_exiting() {
        let mut app = App::new(None);
        app.overlay = Overlay::UpdateResult {
            success: true,
            message: "ok".to_string(),
        };

        let action = app.on_key(key(KeyCode::Esc), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        assert!(
            !app.should_quit,
            "Esc should hide the success result overlay without exiting"
        );
    }

    #[test]
    fn update_result_overlay_success_enter_exits() {
        let mut app = App::new(None);
        app.overlay = Overlay::UpdateResult {
            success: true,
            message: "ok".to_string(),
        };

        let action = app.on_key(key(KeyCode::Enter), &data());
        assert!(matches!(action, Action::None));
        assert!(
            app.should_quit,
            "Enter should exit after a successful update"
        );
    }

    #[test]
    fn provider_claude_api_format_field_enter_opens_overlay() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data);

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.editing = false;
            form.field_idx = form
                .fields()
                .iter()
                .position(|field| *field == ProviderAddField::ClaudeApiFormat)
                .expect("ClaudeApiFormat field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::ClaudeApiFormatPicker { selected: 0 }
        ));

        let format = match app.form.as_ref() {
            Some(super::super::form::FormState::ProviderAdd(form)) => form.claude_api_format,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(format, super::super::form::ClaudeApiFormat::Anthropic);
    }

    #[test]
    fn provider_claude_api_format_warns_when_proxy_not_enabled() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data);

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.editing = false;
            form.field_idx = form
                .fields()
                .iter()
                .position(|field| *field == ProviderAddField::ClaudeApiFormat)
                .expect("ClaudeApiFormat field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        app.on_key(key(KeyCode::Enter), &data);
        let action = app.on_key(key(KeyCode::Down), &data);
        assert!(matches!(action, Action::None));
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::ProviderApiFormatProxyNotice,
                ..
            })
        ));

        let format = match app.form.as_ref() {
            Some(super::super::form::FormState::ProviderAdd(form)) => form.claude_api_format,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(format, super::super::form::ClaudeApiFormat::OpenAiChat);
    }

    #[test]
    fn provider_claude_api_format_proxy_notice_enter_dismisses_popup() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data);

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.editing = false;
            form.field_idx = form
                .fields()
                .iter()
                .position(|field| *field == ProviderAddField::ClaudeApiFormat)
                .expect("ClaudeApiFormat field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        app.on_key(key(KeyCode::Enter), &data);
        app.on_key(key(KeyCode::Down), &data);
        app.on_key(key(KeyCode::Enter), &data);
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn provider_claude_api_format_proxy_notice_reveals_pending_shared_config_tip() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.pending_overlay = Some(Overlay::Confirm(ConfirmOverlay {
            title: texts::tui_provider_switch_shared_config_tip_title().to_string(),
            message: texts::tui_provider_switch_shared_config_tip_message(),
            action: ConfirmAction::ProviderSwitchSharedConfigNotice,
        }));
        app.overlay = Overlay::Confirm(ConfirmOverlay {
            title: texts::tui_claude_api_format_requires_proxy_title().to_string(),
            message: texts::tui_claude_api_format_requires_proxy_message("openai_chat"),
            action: ConfirmAction::ProviderApiFormatProxyNotice,
        });

        let action = app.on_key(key(KeyCode::Enter), &data());

        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::ProviderSwitchSharedConfigNotice,
                ..
            })
        ));
        assert!(app.pending_overlay.is_none());
    }

    #[test]
    fn provider_claude_api_format_does_not_warn_when_proxy_routes_current_app() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.proxy.running = true;
        data.proxy.claude_takeover = true;

        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data);

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.editing = false;
            form.field_idx = form
                .fields()
                .iter()
                .position(|field| *field == ProviderAddField::ClaudeApiFormat)
                .expect("ClaudeApiFormat field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        app.on_key(key(KeyCode::Enter), &data);
        app.on_key(key(KeyCode::Down), &data);
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));

        let format = match app.form.as_ref() {
            Some(super::super::form::FormState::ProviderAdd(form)) => form.claude_api_format,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(format, super::super::form::ClaudeApiFormat::OpenAiChat);
    }

    #[test]
    fn openclaw_provider_edit_submit_uses_plain_edit_submit() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"apiKey":"sk-demo","baseUrl":"https://example.com"}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: Some("provider-model".to_string()),
            default_model_id: None,
        });

        let action = app.on_key(key(KeyCode::Char('e')), &data);
        assert!(matches!(action, Action::None));
        assert!(app.form.is_some());

        let submit = app.on_key(ctrl(KeyCode::Char('s')), &data);
        assert!(matches!(
            submit,
            Action::EditorSubmit {
                submit: EditorSubmit::ProviderEdit { id },
                content,
            } if id == "p1" && content.contains("Provider One")
        ));
    }
}
