use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use serde_json::json;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn data() -> UiData {
        UiData::default()
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
    fn app_cycles_left_right() {
        let mut app = App::new(Some(AppType::Claude));
        assert!(matches!(
            app.on_key(key(KeyCode::Char(']')), &data()),
            Action::SetAppType(AppType::Codex)
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('[')), &data()),
            Action::SetAppType(AppType::OpenCode)
        ));
    }

    #[test]
    fn app_cycles_through_opencode() {
        let mut app = App::new(Some(AppType::Gemini));
        assert!(matches!(
            app.on_key(key(KeyCode::Char(']')), &data()),
            Action::SetAppType(AppType::OpenCode)
        ));

        let mut app = App::new(Some(AppType::OpenCode));
        assert!(matches!(
            app.on_key(key(KeyCode::Char(']')), &data()),
            Action::SetAppType(AppType::Claude)
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('[')), &data()),
            Action::SetAppType(AppType::Gemini)
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
        });

        let action = app.on_key(key(KeyCode::Char('c')), &data);
        assert!(matches!(action, Action::ProviderStreamCheck { id } if id == "p1"));
        assert!(
            matches!(app.overlay, Overlay::StreamCheckRunning { ref provider_name, .. } if provider_name == "Provider One")
        );
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
        });

        let action = app.on_key(key(KeyCode::Char('c')), &data);
        assert!(matches!(action, Action::ProviderStreamCheck { id } if id == "p1"));
        assert!(
            matches!(app.overlay, Overlay::StreamCheckRunning { ref provider_name, .. } if provider_name == "Provider One")
        );
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
    fn provider_add_form_codex_official_auth_enter_does_not_open_editor() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Right), &data); // select OpenAI Official
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields
        app.on_key(key(KeyCode::Tab), &data); // fields -> preview

        app.on_key(key(KeyCode::Enter), &data); // try to edit auth
        assert!(app.editor.is_none());
        assert!(
            app.toast.is_some(),
            "should show a toast explaining auth is disabled"
        );
    }

    #[test]
    fn config_webdav_item_opens_second_level_menu() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Config;
        app.focus = Focus::Content;
        app.config_idx = ConfigItem::ALL
            .iter()
            .position(|item| matches!(item, ConfigItem::WebDavSync))
            .expect("WebDavSync missing from ConfigItem::ALL");

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
    fn settings_menu_hides_proxy_item_for_single_path_flow() {
        assert!(
            !SettingsItem::ALL
                .iter()
                .any(|item| matches!(item, SettingsItem::Proxy)),
            "Settings should not expose a second proxy control entry"
        );
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
    fn provider_form_ctrl_s_merges_common_snippet_into_submitted_json() {
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
            content.contains("\"alwaysThinkingEnabled\""),
            "submitted provider JSON should include merged common snippet keys when enabled"
        );
        assert!(
            content.contains("\"statusLine\""),
            "submitted provider JSON should include nested common snippet keys when enabled"
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
}
