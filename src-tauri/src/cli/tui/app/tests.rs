use super::*;

#[cfg(test)]
#[expect(
    clippy::module_inception,
    reason = "test module mirrors the file layout"
)]
mod tests {
    use super::types::{
        McpEnvEditorField, McpEnvEntryEditorState, UsageLogPager, MODEL_FETCH_FILTER_MAX_MATCHES,
    };
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use serde_json::json;
    use serial_test::serial;
    use std::path::Path;
    use tempfile::TempDir;
    use unicode_width::UnicodeWidthStr;

    use crate::cli::i18n::{texts, use_test_language, Language};
    use crate::cli::tui::data::ProviderRow;
    use crate::cli::tui::form::{
        CodexLocalRoutingField, CodexModelCatalogField, CodexPreviewSection, McpEnvVarRow,
        McpTransport, PromptCacheRoutingMode, ProviderAddFormState, TextInput, UsageQueryField,
        UsageQueryTemplate,
    };
    use crate::cli::tui::runtime_actions::{
        handle_action, run_external_editor_for_prompt_form_content,
    };
    use crate::cli::tui::runtime_systems::{
        handle_managed_auth_msg, ManagedAuthMsg, RequestTracker,
    };
    use crate::cli::tui::terminal::TuiTerminal;
    use crate::commands::workspace::{DailyMemoryFileInfo, DailyMemorySearchResult, ALLOWED_FILES};
    use crate::error::AppError;
    use crate::prompt::Prompt;
    use crate::provider::Provider;
    use crate::services::PromptService;
    use crate::settings::{get_settings, update_settings, AppSettings};
    use crate::test_support::TestEnvGuard;
    struct SettingsGuard {
        previous: AppSettings,
    }

    impl SettingsGuard {
        fn with_openclaw_dir(path: &Path) -> Self {
            let previous = get_settings();
            let settings = AppSettings {
                openclaw_config_dir: Some(path.display().to_string()),
                ..Default::default()
            };
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

    fn alt(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::ALT)
    }

    fn open_prompt_editor(app: &mut App) {
        app.open_editor(
            "Prompt",
            EditorKind::Plain,
            "hello",
            EditorSubmit::PromptEdit {
                id: "pr1".to_string(),
            },
        );
    }

    fn data() -> UiData {
        UiData::default()
    }

    fn managed_auth_status() -> crate::services::ManagedAuthStatus {
        crate::services::ManagedAuthStatus {
            provider: "codex_oauth".to_string(),
            authenticated: true,
            default_account_id: Some("acc-default".to_string()),
            migration_error: None,
            accounts: vec![
                crate::services::ManagedAuthAccount {
                    id: "acc-default".to_string(),
                    provider: "codex_oauth".to_string(),
                    login: "default@example.com".to_string(),
                    avatar_url: None,
                    authenticated_at: 1,
                    is_default: true,
                },
                crate::services::ManagedAuthAccount {
                    id: "acc-alt".to_string(),
                    provider: "codex_oauth".to_string(),
                    login: "alt@example.com".to_string(),
                    avatar_url: None,
                    authenticated_at: 2,
                    is_default: false,
                },
            ],
        }
    }

    fn managed_auth_device() -> crate::services::ManagedAuthDeviceCodeResponse {
        crate::services::ManagedAuthDeviceCodeResponse {
            provider: "codex_oauth".to_string(),
            device_code: "device-1".to_string(),
            user_code: "USER-1".to_string(),
            verification_uri: "https://auth.example.test/device".to_string(),
            expires_in: 900,
            interval: 5,
        }
    }

    fn claude_codex_oauth_form() -> ProviderAddFormState {
        let mut form = ProviderAddFormState::new(AppType::Claude);
        let idx = form
            .template_labels()
            .iter()
            .position(|label| *label == "Codex")
            .expect("Codex template should exist");
        form.apply_template(idx, &[]);
        form
    }

    #[test]
    fn overlay_is_editing_reflects_text_input_state_only() {
        assert!(!Overlay::None.is_editing());
        assert!(!Overlay::Confirm(ConfirmOverlay {
            title: "Confirm".to_string(),
            message: "Continue?".to_string(),
            action: ConfirmAction::Quit,
        })
        .is_editing());
        assert!(!Overlay::ProviderTestMenu {
            provider_id: "provider".to_string(),
            selected: 0,
        }
        .is_editing());
        assert!(!Overlay::ClaudeModelPicker {
            selected: 0,
            column: ClaudeModelPickerColumn::Model,
            editing: false,
        }
        .is_editing());
        assert!(!Overlay::HermesModelsPicker { editing: false }.is_editing());

        assert!(Overlay::TextInput(TextInputState {
            title: "Title".to_string(),
            prompt: "Prompt".to_string(),
            input: TextInput::new(""),
            submit: TextSubmit::ConfigExport,
        })
        .is_editing());
        assert!(Overlay::ClaudeModelPicker {
            selected: 0,
            column: ClaudeModelPickerColumn::Model,
            editing: true,
        }
        .is_editing());
        assert!(Overlay::HermesModelsPicker { editing: true }.is_editing());
        assert!(Overlay::ModelFetchPicker {
            request_id: 1,
            field: ProviderAddField::Name,
            claude_idx: None,
            input: TextInput::new(""),
            query: String::new(),
            fetching: false,
            models: Vec::new(),
            filtered_indices: None,
            filter_incomplete: false,
            error: None,
            selected_idx: 0,
            selection_active: false,
        }
        .is_editing());
        assert!(Overlay::McpEnvEntryEditor(McpEnvEntryEditorState {
            row: None,
            return_selected: 0,
            field: McpEnvEditorField::Key,
            key: TextInput::new(""),
            value: TextInput::new(""),
        })
        .is_editing());
    }

    fn select_provider_common_snippet_row(app: &mut App) {
        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = FormFocus::Fields;
            form.clear_text_edit();
            let fields = form.fields();
            form.field_idx = fields
                .iter()
                .position(|f| *f == ProviderAddField::CommonSnippet)
                .expect("CommonSnippet field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }
    }

    fn open_provider_fields_form(app_type: AppType) -> App {
        let mut app = App::new(Some(app_type));
        app.open_provider_add_form(&UiData::default());
        app.on_key(key(KeyCode::Enter), &data());
        app
    }

    fn open_mcp_fields_form() -> App {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Mcp;
        app.focus = Focus::Content;
        app.on_key(key(KeyCode::Char('a')), &data());
        app.on_key(key(KeyCode::Enter), &data());
        app
    }

    fn assert_provider_remove_confirm(app: &App, expected_id: &str, expected_name: &str) {
        assert!(matches!(
            &app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                title,
                message,
                action: ConfirmAction::ProviderRemoveFromConfig { id },
            }) if id == expected_id
                && title == texts::tui_confirm_remove_provider_title()
                && message == &texts::tui_confirm_remove_provider_message(expected_name)
        ));
    }

    fn assert_provider_copy_confirm(app: &App, expected_id: &str, expected_name: &str) {
        assert!(matches!(
            &app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                title,
                message,
                action: ConfirmAction::ProviderCopy { id },
            }) if id == expected_id
                && title == texts::tui_confirm_copy_provider_title()
                && message == &texts::tui_confirm_copy_provider_message(expected_name, expected_id)
        ));
    }

    fn open_prompt_fields_form() -> App {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;
        app.on_key(key(KeyCode::Char('a')), &data());
        app
    }

    fn provider_field_idx(app: &App) -> usize {
        match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form.field_idx,
            _ => panic!("provider form should be open"),
        }
    }

    fn provider_editing_name(app: &App) -> (bool, String) {
        match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => {
                (form.is_editing_main_text(), form.name.value.clone())
            }
            _ => panic!("provider form should be open"),
        }
    }

    fn provider_json_scroll(app: &App) -> usize {
        match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form.json_scroll,
            _ => panic!("provider form should be open"),
        }
    }

    fn provider_codex_auth_scroll(app: &App) -> usize {
        match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form.codex_auth_scroll,
            _ => panic!("provider form should be open"),
        }
    }

    fn focus_provider_json_preview(app: &mut App) {
        match app.form.as_mut() {
            Some(FormState::ProviderAdd(form)) => form.focus = FormFocus::JsonPreview,
            _ => panic!("provider form should be open"),
        }
    }

    fn mcp_field_idx(app: &App) -> usize {
        match app.form.as_ref() {
            Some(FormState::McpAdd(form)) => form.field_idx,
            _ => panic!("mcp form should be open"),
        }
    }

    fn mcp_editing_id(app: &App) -> (bool, String) {
        match app.form.as_ref() {
            Some(FormState::McpAdd(form)) => (form.text_edit.is_some(), form.id.value.clone()),
            _ => panic!("mcp form should be open"),
        }
    }

    fn mcp_json_scroll(app: &App) -> usize {
        match app.form.as_ref() {
            Some(FormState::McpAdd(form)) => form.json_scroll,
            _ => panic!("mcp form should be open"),
        }
    }

    fn focus_mcp_json_preview(app: &mut App) {
        match app.form.as_mut() {
            Some(FormState::McpAdd(form)) => form.focus = FormFocus::JsonPreview,
            _ => panic!("mcp form should be open"),
        }
    }

    fn prompt_field_idx(app: &App) -> usize {
        match app.form.as_ref() {
            Some(FormState::PromptMeta(form)) => form.field_idx,
            _ => panic!("prompt form should be open"),
        }
    }

    fn prompt_editing_id(app: &App) -> (bool, String) {
        match app.form.as_ref() {
            Some(FormState::PromptMeta(form)) => (form.text_edit.is_some(), form.id.value.clone()),
            _ => panic!("prompt form should be open"),
        }
    }

    fn select_provider_usage_query_row(app: &mut App) {
        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = FormFocus::Fields;
            form.clear_text_edit();
            let fields = form.fields();
            form.field_idx = fields
                .iter()
                .position(|f| *f == ProviderAddField::UsageQuery)
                .expect("UsageQuery field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }
    }

    fn select_provider_field(app: &mut App, field: ProviderAddField) {
        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = FormFocus::Fields;
            form.clear_text_edit();
            let fields = form.fields();
            form.field_idx = fields
                .iter()
                .position(|candidate| *candidate == field)
                .unwrap_or_else(|| panic!("{field:?} field should exist"));
        } else {
            panic!("expected ProviderAdd form");
        }
    }

    fn open_local_proxy_settings_page() -> App {
        let mut app = App::new(Some(AppType::Claude));
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Claude,
        )));
        select_provider_field(&mut app, ProviderAddField::LocalProxySettings);
        app.on_key(key(KeyCode::Enter), &data());
        app
    }

    fn select_local_proxy_settings_field(app: &mut App, field: form::LocalProxySettingsField) {
        let Some(FormState::ProviderAdd(provider)) = app.form.as_mut() else {
            panic!("expected ProviderAdd form");
        };
        provider.local_proxy_settings_field_idx = provider
            .local_proxy_settings_fields()
            .iter()
            .position(|candidate| *candidate == field)
            .unwrap_or_else(|| panic!("{field:?} local proxy settings field should exist"));
    }

    fn select_usage_query_field(app: &mut App, field: UsageQueryField) {
        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = FormFocus::Fields;
            form.clear_text_edit();
            let fields = form.usage_query_table_fields();
            form.usage_query_field_idx = fields
                .iter()
                .position(|candidate| *candidate == field)
                .unwrap_or_else(|| panic!("{field:?} usage query field should exist"));
        } else {
            panic!("expected ProviderAdd form");
        }
    }

    fn help_text(app: &App) -> String {
        let Overlay::Help(help) = &app.overlay else {
            panic!("expected Help overlay, got {:?}", app.overlay);
        };
        let content = &help.content;
        format!(
            "{}\n{}\n{}",
            content.title,
            content.eyebrow,
            content.lines.join("\n")
        )
    }

    fn claude_provider_row(id: &str) -> ProviderRow {
        ProviderRow {
            id: id.to_string(),
            provider: Provider::with_id(
                id.to_string(),
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
        }
    }

    fn installed_skill(directory: &str, name: &str) -> crate::services::skill::InstalledSkill {
        crate::services::skill::InstalledSkill {
            id: format!("local:{directory}"),
            name: name.to_string(),
            description: None,
            directory: directory.to_string(),
            repo_owner: None,
            repo_name: None,
            repo_branch: None,
            readme_url: None,
            apps: crate::app_config::SkillApps::default(),
            installed_at: 0,
        }
    }

    fn unmanaged_skill(directory: &str) -> crate::services::skill::UnmanagedSkill {
        crate::services::skill::UnmanagedSkill {
            directory: directory.to_string(),
            name: "Hello Skill".to_string(),
            description: None,
            found_in: vec!["claude".to_string()],
            path: format!("/tmp/{directory}"),
        }
    }

    fn prompt_import_candidate(
        filename: &str,
        content: &str,
    ) -> super::super::data::PromptImportCandidate {
        super::super::data::PromptImportCandidate {
            filename: filename.to_string(),
            content: content.to_string(),
        }
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
            None,
            &mut webdav_loading,
            None,
            &mut update_check,
            None,
            None,
            action,
        )
    }

    fn run_runtime_action_with_session_tx(
        app: &mut App,
        data: &mut UiData,
        session_tx: &std::sync::mpsc::Sender<crate::cli::tui::runtime_systems::SessionReq>,
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
            Some(session_tx),
            None,
            &mut webdav_loading,
            None,
            &mut update_check,
            None,
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
    fn skills_discover_tab_toggles_marketplace_source() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::SkillsDiscover;
        app.focus = Focus::Content;
        app.skills_discover_query = "python".to_string();

        let action = app.on_key(key(KeyCode::Tab), &data());

        assert_eq!(
            app.skills_discover_source,
            SkillsDiscoverSource::Marketplace
        );
        assert!(matches!(action, Action::None));
    }

    #[test]
    fn skills_discover_r_refreshes_current_source() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::SkillsDiscover;
        app.focus = Focus::Content;
        app.skills_discover_query = "python".to_string();
        app.skills_discover_source = SkillsDiscoverSource::Marketplace;

        let action = app.on_key(key(KeyCode::Char('r')), &data());

        assert!(matches!(
            action,
            Action::SkillsDiscover {
                query,
                source: SkillsDiscoverSource::Marketplace,
                force: true,
            } if query == "python"
        ));
    }

    #[test]
    fn skills_discover_e_opens_repo_manager() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::SkillsDiscover;
        app.focus = Focus::Content;

        let action = app.on_key(key(KeyCode::Char('e')), &data());

        assert!(matches!(action, Action::SwitchRoute(Route::SkillsRepos)));
    }

    #[test]
    fn skills_m_opens_apps_picker_overlay() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Skills;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.skills
            .installed
            .push(installed_skill("hello-skill", "Hello Skill"));

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
    fn skills_space_key_toggles_current_app() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Skills;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.skills
            .installed
            .push(installed_skill("hello-skill", "Hello Skill"));

        let action = app.on_key(key(KeyCode::Char(' ')), &data);
        assert!(matches!(
            action,
            Action::SkillsToggle {
                directory,
                enabled: true
            } if directory == "hello-skill"
        ));
    }

    #[test]
    fn skills_x_key_does_not_toggle_current_app() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Skills;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.skills
            .installed
            .push(installed_skill("hello-skill", "Hello Skill"));

        let action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(action, Action::None));
    }

    #[test]
    fn skill_detail_space_key_toggles_current_app() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::SkillDetail {
            directory: "hello-skill".to_string(),
        };
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.skills
            .installed
            .push(installed_skill("hello-skill", "Hello Skill"));

        let action = app.on_key(key(KeyCode::Char(' ')), &data);
        assert!(matches!(
            action,
            Action::SkillsToggle {
                directory,
                enabled: true
            } if directory == "hello-skill"
        ));
    }

    #[test]
    fn skill_detail_x_key_does_not_toggle_current_app() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::SkillDetail {
            directory: "hello-skill".to_string(),
        };
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.skills
            .installed
            .push(installed_skill("hello-skill", "Hello Skill"));

        let action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(action, Action::None));
    }

    #[test]
    fn skills_apps_picker_space_toggles_selected_app_and_enter_emits_action() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Skills;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.skills
            .installed
            .push(installed_skill("hello-skill", "Hello Skill"));

        app.on_key(key(KeyCode::Char('m')), &data);

        let action = app.on_key(key(KeyCode::Char(' ')), &data);
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
    fn skills_apps_picker_x_does_not_toggle_selected_app() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Skills;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.skills
            .installed
            .push(installed_skill("hello-skill", "Hello Skill"));

        app.on_key(key(KeyCode::Char('m')), &data);

        let action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::SkillsAppsPicker { apps, .. } if !apps.codex
        ));
    }

    #[test]
    fn skills_apps_picker_can_select_hermes() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Skills;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.skills
            .installed
            .push(installed_skill("hello-skill", "Hello Skill"));

        app.on_key(key(KeyCode::Char('m')), &data);
        app.on_key(key(KeyCode::Down), &data);
        app.on_key(key(KeyCode::Down), &data);
        app.on_key(key(KeyCode::Down), &data);
        app.on_key(key(KeyCode::Down), &data);

        let action = app.on_key(key(KeyCode::Char(' ')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::SkillsAppsPicker { selected, apps, .. } if *selected == 4 && apps.hermes
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            action,
            Action::SkillsSetApps { directory, apps }
                if directory == "hello-skill"
                    && !apps.claude
                    && !apps.codex
                    && !apps.gemini
                    && !apps.opencode
                    && apps.hermes
        ));
    }

    #[test]
    fn skills_apps_picker_from_openclaw_targets_hermes_last_visible_row() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::Skills;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.skills
            .installed
            .push(installed_skill("hello-skill", "Hello Skill"));

        let action = app.on_key(key(KeyCode::Char('m')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::SkillsAppsPicker { selected, .. } if *selected == 4
        ));

        let action = app.on_key(key(KeyCode::Char(' ')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::SkillsAppsPicker { selected, apps, .. }
                if *selected == 4
                    && !apps.claude
                    && !apps.codex
                    && !apps.gemini
                    && !apps.opencode
                    && apps.hermes
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
            .push(installed_skill("hello-skill", "Hello Skill"));

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
    fn skills_repos_space_key_toggles_enabled() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::SkillsRepos;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.skills.repos.push(crate::services::skill::SkillRepo {
            owner: "anthropics".to_string(),
            name: "skills".to_string(),
            branch: "main".to_string(),
            enabled: false,
        });

        let action = app.on_key(key(KeyCode::Char(' ')), &data);
        assert!(matches!(
            action,
            Action::SkillsRepoToggleEnabled {
                owner,
                name,
                enabled: true
            } if owner == "anthropics" && name == "skills"
        ));
    }

    #[test]
    fn skills_repos_x_key_does_not_toggle_enabled() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::SkillsRepos;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.skills.repos.push(crate::services::skill::SkillRepo {
            owner: "anthropics".to_string(),
            name: "skills".to_string(),
            branch: "main".to_string(),
            enabled: false,
        });

        let action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(action, Action::None));
    }

    #[test]
    fn skills_import_picker_space_toggles_selection() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Skills;
        app.focus = Focus::Content;
        app.overlay = Overlay::SkillsImportPicker {
            skills: vec![unmanaged_skill("hello-skill")],
            selected_idx: 0,
            selected: std::collections::HashSet::new(),
        };

        let action = app.on_key(key(KeyCode::Char(' ')), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::SkillsImportPicker { selected, .. }
                if selected.contains("hello-skill")
        ));
    }

    #[test]
    fn skills_import_picker_x_does_not_toggle_selection() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Skills;
        app.focus = Focus::Content;
        app.overlay = Overlay::SkillsImportPicker {
            skills: vec![unmanaged_skill("hello-skill")],
            selected_idx: 0,
            selected: std::collections::HashSet::new(),
        };

        let action = app.on_key(key(KeyCode::Char('x')), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::SkillsImportPicker { selected, .. } if selected.is_empty()
        ));
    }

    #[test]
    fn skills_import_picker_enter_builds_explicit_apps_from_found_in() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Skills;
        app.focus = Focus::Content;
        app.overlay = Overlay::SkillsImportPicker {
            skills: vec![crate::services::skill::UnmanagedSkill {
                directory: "hello-skill".to_string(),
                name: "Hello Skill".to_string(),
                description: None,
                found_in: vec![
                    AppType::Claude.as_str().to_string(),
                    AppType::OpenCode.as_str().to_string(),
                    AppType::OpenClaw.as_str().to_string(),
                ],
                path: "/tmp/hello-skill".to_string(),
            }],
            selected_idx: 0,
            selected: std::iter::once("hello-skill".to_string()).collect(),
        };

        let action = app.on_key(key(KeyCode::Enter), &data());
        let Action::SkillsImportFromApps { imports } = action else {
            panic!("expected skills import action");
        };

        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].directory, "hello-skill");
        assert!(imports[0].apps.claude);
        assert!(imports[0].apps.opencode);
        assert!(
            !imports[0].apps.is_empty(),
            "supported app targets should be preserved"
        );
        assert!(
            !imports[0].apps.is_enabled_for(&AppType::OpenClaw),
            "OpenClaw source should not become a target app"
        );
    }

    #[test]
    fn config_e_key_opens_common_snippet_editor_when_selected() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Config;
        app.focus = Focus::Content;
        app.config_idx = ConfigItem::ALL
            .iter()
            .position(|item| matches!(item, ConfigItem::CommonSnippet))
            .expect("CommonSnippet missing from ConfigItem::ALL");

        let action = app.on_key(key(KeyCode::Char('e')), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.editor.as_ref().map(|e| (&e.kind, &e.submit)),
            Some((
                EditorKind::Json,
                EditorSubmit::ConfigCommonSnippet {
                    app_type: AppType::Claude,
                    source: CommonSnippetViewSource::Global
                }
            ))
        ));
    }

    #[test]
    #[serial(home_settings)]
    fn app_cycles_left_right() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());
        crate::settings::set_visible_apps(crate::settings::VisibleApps {
            claude: true,
            codex: true,
            gemini: true,
            opencode: true,
            hermes: false,
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
    fn app_cycles_with_chinese_brackets() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());
        crate::settings::set_visible_apps(crate::settings::VisibleApps {
            claude: true,
            codex: true,
            gemini: true,
            opencode: true,
            hermes: false,
            openclaw: true,
        })
        .expect("save visible apps");
        let mut app = App::new(Some(AppType::Claude));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('】')), &data()),
            Action::SetAppType(AppType::Codex)
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('【')), &data()),
            Action::SetAppType(AppType::OpenClaw)
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('］')), &data()),
            Action::SetAppType(AppType::Codex)
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('［')), &data()),
            Action::SetAppType(AppType::OpenClaw)
        ));
    }

    #[test]
    #[serial(home_settings)]
    fn app_cycles_through_opencode() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());
        crate::settings::set_visible_apps(crate::settings::VisibleApps {
            claude: true,
            codex: true,
            gemini: true,
            opencode: true,
            hermes: false,
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
        let _env = TestEnvGuard::isolated(temp_home.path());
        crate::settings::set_visible_apps(crate::settings::VisibleApps {
            claude: true,
            codex: false,
            gemini: false,
            opencode: true,
            hermes: false,
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
        let _env = TestEnvGuard::isolated(temp_home.path());
        crate::settings::set_visible_apps(crate::settings::VisibleApps {
            claude: false,
            codex: true,
            gemini: false,
            opencode: false,
            hermes: false,
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
        let _env = TestEnvGuard::isolated(temp_home.path());
        crate::settings::set_visible_apps(crate::settings::VisibleApps {
            claude: true,
            codex: true,
            gemini: false,
            opencode: false,
            hermes: false,
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
        let _env = TestEnvGuard::isolated(temp_home.path());
        crate::settings::set_visible_apps(crate::settings::VisibleApps {
            claude: true,
            codex: true,
            gemini: false,
            opencode: false,
            hermes: false,
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

        app.route = Route::Providers;
        app.overlay = Overlay::FailoverQueueManager {
            selected_provider_id: None,
        };
        assert!(app.should_poll_proxy_activity());

        app.overlay = Overlay::None;
        assert!(!app.should_poll_proxy_activity());
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
        let ui_data = UiData::default();
        app.open_provider_add_form(&ui_data);

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
            form.clear_text_edit();
        }

        // Enter edit mode for Notes.
        app.on_key(key(KeyCode::Enter), &ui_data);
        for _ in 0..(PROVIDER_NOTES_MAX_CHARS + 10) {
            app.on_key(key(KeyCode::Char('a')), &ui_data);
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
        assert!(!app.filter.active);
        app.on_key(key(KeyCode::Char('/')), &data());
        assert!(app.filter.active);
        app.on_key(key(KeyCode::Char('a')), &data());
        app.on_key(key(KeyCode::Char('b')), &data());
        app.on_key(key(KeyCode::Char('j')), &data());
        app.on_key(key(KeyCode::Char('k')), &data());
        assert_eq!(app.filter.input.value, "abjk");
        app.on_key(key(KeyCode::Backspace), &data());
        assert_eq!(app.filter.input.value, "abj");
        app.on_key(key(KeyCode::Enter), &data());
        assert!(!app.filter.active);
    }

    #[test]
    fn filter_mode_supports_readline_shortcuts() {
        let mut app = App::new(Some(AppType::Claude));
        app.on_key(key(KeyCode::Char('/')), &data());
        for ch in "alpha beta".chars() {
            app.on_key(key(KeyCode::Char(ch)), &data());
        }

        app.on_key(ctrl(KeyCode::Char('a')), &data());
        app.on_key(key(KeyCode::Char('>')), &data());
        app.on_key(ctrl(KeyCode::Char('e')), &data());
        app.on_key(ctrl(KeyCode::Char('w')), &data());

        assert_eq!(app.filter.input.value, ">alpha ");
        assert_eq!(app.filter.input.cursor, ">alpha ".chars().count());
    }

    #[test]
    fn text_input_overlay_supports_readline_shortcuts() {
        let mut app = App::new(Some(AppType::Claude));
        app.overlay = Overlay::TextInput(TextInputState {
            title: "Demo".to_string(),
            prompt: "Value".to_string(),
            input: TextInput::new("alpha beta"),
            submit: TextSubmit::ConfigBackupName,
        });

        app.on_key(ctrl(KeyCode::Char('a')), &data());
        app.on_key(key(KeyCode::Char('>')), &data());
        app.on_key(ctrl(KeyCode::Char('e')), &data());
        app.on_key(ctrl(KeyCode::Char('w')), &data());

        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState { input, .. })
                if input.value == ">alpha " && input.cursor == ">alpha ".chars().count()
        ));
    }

    #[test]
    fn provider_field_editor_supports_readline_shortcuts() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.on_key(key(KeyCode::Char('a')), &data());
        app.on_key(key(KeyCode::Enter), &data());

        let name_idx = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form
                .fields()
                .iter()
                .position(|field| *field == ProviderAddField::Name)
                .expect("name field should exist"),
            _ => panic!("provider form should be open"),
        };

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.field_idx = name_idx;
            form.name.set("alpha beta");
            assert!(form.begin_main_text_edit(ProviderAddField::Name));
        }

        app.on_key(ctrl(KeyCode::Char('a')), &data());
        app.on_key(key(KeyCode::Char('>')), &data());
        app.on_key(ctrl(KeyCode::Char('e')), &data());
        app.on_key(ctrl(KeyCode::Char('w')), &data());

        let form = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form,
            _ => panic!("provider form should stay open"),
        };
        assert_eq!(form.name.value, ">alpha ");
    }

    #[test]
    fn provider_local_proxy_settings_row_opens_page_and_escape_returns_to_main() {
        let mut app = App::new(Some(AppType::Claude));
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Claude,
        )));
        select_provider_field(&mut app, ProviderAddField::LocalProxySettings);

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data()),
            Action::None
        ));
        let Some(FormState::ProviderAdd(provider)) = app.form.as_ref() else {
            panic!("expected ProviderAdd form");
        };
        assert!(matches!(
            provider.page,
            form::ProviderFormPage::LocalProxySettings
        ));
        assert_eq!(
            provider.selected_local_proxy_settings_field(),
            Some(form::LocalProxySettingsField::UserAgent)
        );

        assert!(matches!(
            app.on_key(key(KeyCode::Esc), &data()),
            Action::None
        ));
        let Some(FormState::ProviderAdd(provider)) = app.form.as_ref() else {
            panic!("expected ProviderAdd form");
        };
        assert!(matches!(provider.page, form::ProviderFormPage::Main));
        assert!(matches!(provider.focus, FormFocus::Fields));
    }

    #[test]
    fn provider_local_proxy_user_agent_enter_opens_picker() {
        let mut app = open_local_proxy_settings_page();

        app.on_key(key(KeyCode::Enter), &data());
        assert!(matches!(app.overlay, Overlay::UserAgentPicker { .. }));
    }

    #[test]
    fn provider_local_proxy_user_agent_picker_space_is_noop() {
        let mut app = open_local_proxy_settings_page();

        app.on_key(key(KeyCode::Enter), &data());
        let selected = match app.overlay {
            Overlay::UserAgentPicker { selected } => selected,
            ref overlay => panic!("expected UserAgentPicker, got {overlay:?}"),
        };

        let action = app.on_key(key(KeyCode::Char(' ')), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::UserAgentPicker {
                selected: current
            } if current == selected
        ));
    }

    #[test]
    fn provider_local_proxy_user_agent_u_shortcut_is_not_bound() {
        let mut app = open_local_proxy_settings_page();
        select_local_proxy_settings_field(&mut app, form::LocalProxySettingsField::BodyOverrides);

        app.on_key(key(KeyCode::Char('u')), &data());
        assert!(matches!(app.overlay, Overlay::None));
        assert!(matches!(
            app.form,
            Some(FormState::ProviderAdd(ref provider))
                if provider.selected_local_proxy_settings_field()
                    == Some(form::LocalProxySettingsField::BodyOverrides)
        ));
    }

    #[test]
    fn provider_local_proxy_user_agent_picker_help_restores_picker() {
        let _lang = use_test_language(Language::English);
        let mut app = open_local_proxy_settings_page();

        app.on_key(key(KeyCode::Enter), &data());
        let selected = match &app.overlay {
            Overlay::UserAgentPicker { selected } => *selected,
            overlay => panic!("expected UserAgentPicker, got {overlay:?}"),
        };

        app.on_key(key(KeyCode::Char('?')), &data());
        let text = help_text(&app);
        assert!(text.contains("local proxy"), "{text}");
        assert!(matches!(
            app.pending_overlay,
            Some(Overlay::UserAgentPicker {
                selected: pending_selected
            }) if pending_selected == selected
        ));

        app.on_key(key(KeyCode::Char('?')), &data());
        assert!(matches!(
            app.overlay,
            Overlay::UserAgentPicker {
                selected: restored_selected
            } if restored_selected == selected
        ));
        assert!(app.pending_overlay.is_none());
    }

    #[test]
    fn provider_local_proxy_user_agent_custom_choice_opens_prefilled_input_and_applies() {
        let mut app = open_local_proxy_settings_page();
        let Some(FormState::ProviderAdd(provider)) = app.form.as_mut() else {
            panic!("expected ProviderAdd form");
        };
        provider.custom_user_agent.set("existing-agent/1.0");

        app.on_key(key(KeyCode::Enter), &data());
        app.on_key(key(KeyCode::Enter), &data());
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                ref input,
                submit: TextSubmit::ProviderCustomUserAgent,
                ..
            }) if input.value == "existing-agent/1.0"
        ));

        let Overlay::TextInput(input) = &mut app.overlay else {
            panic!("expected custom User-Agent input");
        };
        input.input.set("cc-switch-cli/1.0");
        app.on_key(key(KeyCode::Enter), &data());

        let Some(FormState::ProviderAdd(provider)) = app.form.as_ref() else {
            panic!("expected ProviderAdd form");
        };
        assert_eq!(provider.custom_user_agent.value, "cc-switch-cli/1.0");
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn provider_local_proxy_user_agent_no_override_choice_clears_value() {
        let mut app = open_local_proxy_settings_page();
        let Some(FormState::ProviderAdd(provider)) = app.form.as_mut() else {
            panic!("expected ProviderAdd form");
        };
        provider.custom_user_agent.set("existing-agent/1.0");

        app.on_key(key(KeyCode::Enter), &data());
        for _ in 0..6 {
            app.on_key(key(KeyCode::Down), &data());
        }
        app.on_key(key(KeyCode::Enter), &data());

        let Some(FormState::ProviderAdd(provider)) = app.form.as_ref() else {
            panic!("expected ProviderAdd form");
        };
        assert!(provider.custom_user_agent.value.is_empty());
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn provider_local_proxy_user_agent_preset_choice_applies_value() {
        let mut app = open_local_proxy_settings_page();
        let Some(FormState::ProviderAdd(provider)) = app.form.as_mut() else {
            panic!("expected ProviderAdd form");
        };
        provider.custom_user_agent.set("custom-agent/1.0");

        app.on_key(key(KeyCode::Enter), &data());
        for _ in 0..2 {
            app.on_key(key(KeyCode::Down), &data());
        }
        app.on_key(key(KeyCode::Enter), &data());

        let Some(FormState::ProviderAdd(provider)) = app.form.as_ref() else {
            panic!("expected ProviderAdd form");
        };
        assert_eq!(
            provider.custom_user_agent.value,
            form::USER_AGENT_PRESETS[1]
        );
        assert!(matches!(app.overlay, Overlay::None));
        assert!(matches!(
            provider.page,
            form::ProviderFormPage::LocalProxySettings
        ));
    }

    #[test]
    fn provider_local_proxy_header_overrides_open_json_editor_with_current_value() {
        let mut app = open_local_proxy_settings_page();
        let Some(FormState::ProviderAdd(provider)) = app.form.as_mut() else {
            panic!("expected ProviderAdd form");
        };
        provider
            .local_proxy_header_overrides
            .insert("x-client-name".to_string(), "cc-switch".to_string());
        select_local_proxy_settings_field(&mut app, form::LocalProxySettingsField::HeaderOverrides);

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data()),
            Action::None
        ));
        let editor = app.editor.as_ref().expect("JSON editor should open");
        assert!(matches!(editor.kind, EditorKind::Json));
        assert!(matches!(
            editor.submit,
            EditorSubmit::ProviderFormApplyLocalProxyHeaders
        ));
        assert_eq!(editor.text(), "{\n  \"x-client-name\": \"cc-switch\"\n}");
    }

    #[test]
    fn provider_local_proxy_body_overrides_open_json_editor_with_current_value() {
        let mut app = open_local_proxy_settings_page();
        let Some(FormState::ProviderAdd(provider)) = app.form.as_mut() else {
            panic!("expected ProviderAdd form");
        };
        provider.local_proxy_body_override = Some(json!({ "temperature": 0.4 }));
        select_local_proxy_settings_field(&mut app, form::LocalProxySettingsField::BodyOverrides);

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data()),
            Action::None
        ));
        let editor = app.editor.as_ref().expect("JSON editor should open");
        assert!(matches!(editor.kind, EditorKind::Json));
        assert!(matches!(
            editor.submit,
            EditorSubmit::ProviderFormApplyLocalProxyBody
        ));
        assert_eq!(editor.text(), "{\n  \"temperature\": 0.4\n}");
    }

    #[test]
    fn provider_local_proxy_explanations_live_in_context_help() {
        let _lang = use_test_language(Language::English);
        let mut app = open_local_proxy_settings_page();

        app.on_key(key(KeyCode::Char('?')), &data());
        let user_agent_help = help_text(&app);
        assert!(user_agent_help.contains("local proxy"), "{user_agent_help}");
        assert!(
            user_agent_help.contains("Control characters"),
            "{user_agent_help}"
        );

        app.on_key(key(KeyCode::Char('?')), &data());
        select_local_proxy_settings_field(&mut app, form::LocalProxySettingsField::HeaderOverrides);
        app.on_key(key(KeyCode::Char('?')), &data());
        let header_help = help_text(&app);
        assert!(header_help.contains("JSON object"), "{header_help}");
        assert!(
            header_help.contains("cannot be overridden"),
            "{header_help}"
        );

        app.on_key(key(KeyCode::Char('?')), &data());
        select_local_proxy_settings_field(&mut app, form::LocalProxySettingsField::BodyOverrides);
        app.on_key(key(KeyCode::Char('?')), &data());
        let body_help = help_text(&app);
        assert!(body_help.contains("store: false"), "{body_help}");
        assert!(body_help.contains("top-level stream"), "{body_help}");
    }

    #[test]
    fn claude_fallback_model_edit_persists_anthropic_model() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.on_key(key(KeyCode::Char('a')), &data());
        app.on_key(key(KeyCode::Enter), &data());

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.name.set("Provider One");
            form.focus = FormFocus::Fields;
            form.field_idx = form
                .fields()
                .iter()
                .position(|field| *field == ProviderAddField::ClaudeFallbackModel)
                .expect("fallback model field should exist");
            assert!(form.begin_main_text_edit(ProviderAddField::ClaudeFallbackModel));
        }

        for ch in "gpt-4o".chars() {
            app.on_key(key(KeyCode::Char(ch)), &data());
        }
        app.on_key(key(KeyCode::Enter), &data());

        // The fallback row lives outside the model-mapping sub-page, but editing it
        // must still mark the model config touched so ANTHROPIC_MODEL is persisted.
        let provider = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => {
                assert_eq!(form.claude_model.value, "gpt-4o");
                form.to_provider_json_value()
            }
            _ => panic!("provider form should stay open"),
        };
        assert_eq!(
            provider["settingsConfig"]["env"]["ANTHROPIC_MODEL"].as_str(),
            Some("gpt-4o"),
        );
    }

    #[test]
    fn mcp_field_editor_supports_readline_shortcuts() {
        let mut app = App::new(Some(AppType::Claude));
        let mut form = McpAddFormState::new();
        form.focus = FormFocus::Fields;
        form.field_idx = form
            .fields()
            .iter()
            .position(|field| *field == McpAddField::Name)
            .expect("name field should exist");
        form.name.set("alpha beta");
        assert!(form.begin_text_edit(McpAddField::Name));
        app.form = Some(FormState::McpAdd(form));

        app.on_key(ctrl(KeyCode::Char('a')), &data());
        app.on_key(key(KeyCode::Char('>')), &data());
        app.on_key(ctrl(KeyCode::Char('e')), &data());
        app.on_key(ctrl(KeyCode::Char('w')), &data());

        let form = match app.form.as_ref() {
            Some(FormState::McpAdd(form)) => form,
            _ => panic!("mcp form should stay open"),
        };
        assert_eq!(form.name.value, ">alpha ");
    }

    #[test]
    fn mcp_env_entry_editor_supports_readline_shortcuts() {
        let mut app = App::new(Some(AppType::Claude));
        app.form = Some(FormState::McpAdd(McpAddFormState::new()));
        app.overlay = Overlay::McpEnvEntryEditor(McpEnvEntryEditorState {
            row: None,
            return_selected: 0,
            field: McpEnvEditorField::Key,
            key: TextInput::new("alpha beta"),
            value: TextInput::new(""),
        });

        app.on_key(ctrl(KeyCode::Char('a')), &data());
        app.on_key(key(KeyCode::Char('>')), &data());
        app.on_key(ctrl(KeyCode::Char('e')), &data());
        app.on_key(ctrl(KeyCode::Char('w')), &data());

        assert!(matches!(
            app.overlay,
            Overlay::McpEnvEntryEditor(McpEnvEntryEditorState { key, .. })
                if key.value == ">alpha " && key.cursor == ">alpha ".chars().count()
        ));
    }

    #[test]
    fn model_fetch_picker_supports_readline_shortcuts() {
        let mut app = App::new(Some(AppType::Claude));
        app.overlay = Overlay::ModelFetchPicker {
            request_id: 1,
            field: ProviderAddField::Name,
            claude_idx: None,
            input: TextInput::new("alpha beta"),
            query: "alpha beta".to_string(),
            fetching: false,
            models: vec!["alpha beta".to_string()],
            filtered_indices: None,
            filter_incomplete: false,
            error: None,
            selected_idx: 0,
            selection_active: false,
        };

        app.on_key(ctrl(KeyCode::Char('a')), &data());
        app.on_key(key(KeyCode::Char('>')), &data());
        app.on_key(ctrl(KeyCode::Char('e')), &data());
        app.on_key(ctrl(KeyCode::Char('w')), &data());

        assert!(matches!(
            app.overlay,
            Overlay::ModelFetchPicker { input, query, .. }
                if input.value == ">alpha "
                    && input.cursor == ">alpha ".chars().count()
                    && query == ">alpha "
        ));
    }

    #[test]
    fn model_fetch_filter_cache_uses_none_for_all_and_indices_for_matches() {
        let models = vec![
            "Alpha".to_string(),
            "beta".to_string(),
            "ALPHABET".to_string(),
        ];

        assert_eq!(model_fetch_filter(&models, "   ").indices, None);
        assert_eq!(
            model_fetch_filter(&models, " alp ").indices,
            Some(vec![0, 2])
        );
        assert_eq!(
            model_fetch_filter(&models, "missing").indices,
            Some(Vec::new())
        );
    }

    #[test]
    fn model_fetch_filter_searches_a_typical_ten_thousand_row_catalog_completely() {
        let models = (0..10_000)
            .map(|index| format!("model-{index:05}"))
            .collect::<Vec<_>>();

        let filter = model_fetch_filter(&models, "09999");

        assert_eq!(filter.indices, Some(vec![9_999]));
        assert!(!filter.incomplete);
    }

    #[test]
    fn model_fetch_filter_bounds_single_values_queries_and_match_collections() {
        let models = vec!["x".repeat(1_000_000), "alpha".to_string()];
        let filter = model_fetch_filter(&models, "alpha");
        assert_eq!(filter.indices, Some(vec![1]));
        assert!(filter.incomplete);

        let oversized_query = "q".repeat(MODEL_FETCH_QUERY_MAX_BYTES + 1);
        let filter = model_fetch_filter(&models, &oversized_query);
        assert_eq!(filter.indices, Some(Vec::new()));
        assert!(filter.incomplete);

        let matching = vec!["match".to_string(); MODEL_FETCH_FILTER_MAX_MATCHES + 1];
        let filter = model_fetch_filter(&matching, "match");
        assert_eq!(
            filter.indices.as_ref().map(Vec::len),
            Some(MODEL_FETCH_FILTER_MAX_MATCHES)
        );
        assert!(filter.incomplete);
    }

    #[test]
    fn model_fetch_navigation_does_not_copy_a_huge_model_into_the_query() {
        let mut app = App::new(Some(AppType::Claude));
        app.overlay = Overlay::ModelFetchPicker {
            request_id: 1,
            field: ProviderAddField::Name,
            claude_idx: None,
            input: TextInput::new("needle"),
            query: "needle".to_string(),
            fetching: false,
            models: vec![format!("needle-{}", "x".repeat(1_000_000))],
            filtered_indices: Some(vec![0]),
            filter_incomplete: true,
            error: None,
            selected_idx: 0,
            selection_active: false,
        };

        app.on_key(key(KeyCode::Down), &data());

        assert!(matches!(
            app.overlay,
            Overlay::ModelFetchPicker {
                ref input,
                selection_active: true,
                ..
            } if input.value == "needle"
        ));
    }

    #[test]
    fn provider_add_form_hermes_models_enter_opens_model_picker() {
        let mut app = App::new(Some(AppType::Hermes));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Hermes,
        )));

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = FormFocus::Fields;
            form.hermes_base_url.set("https://api.example.com/v1");
            form.hermes_api_key.set("sk-hermes");
            form.field_idx = form
                .fields()
                .iter()
                .position(|field| *field == ProviderAddField::HermesModels)
                .expect("HermesModels field should exist");
        }

        let action = app.on_key(key(KeyCode::Enter), &data());
        assert!(matches!(action, Action::None));
        assert!(
            app.editor.is_none(),
            "Hermes Models Enter should not open a JSON editor"
        );
        assert!(matches!(
            app.overlay,
            Overlay::HermesModelsPicker { editing: false }
        ));
        let Some(FormState::ProviderAdd(form)) = app.form.as_ref() else {
            panic!("expected ProviderAdd form");
        };
        assert!(matches!(form.page, form::ProviderFormPage::Main));
    }

    #[test]
    fn provider_add_form_hermes_navigation_skips_advanced_divider() {
        let mut app = App::new(Some(AppType::Hermes));
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Hermes,
        )));

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = FormFocus::Fields;
            form.clear_text_edit();
            let fields = form.fields();
            form.field_idx = fields
                .iter()
                .position(|field| *field == ProviderAddField::HermesModels)
                .expect("HermesModels field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        app.on_key(key(KeyCode::Down), &UiData::default());
        assert!(matches!(
            app.form,
            Some(FormState::ProviderAdd(ref form))
                if form.fields().get(form.field_idx) == Some(&ProviderAddField::HermesRateLimitDelay)
        ));

        app.on_key(key(KeyCode::Up), &UiData::default());
        assert!(matches!(
            app.form,
            Some(FormState::ProviderAdd(ref form))
                if form.fields().get(form.field_idx) == Some(&ProviderAddField::HermesModels)
        ));
    }

    #[test]
    fn provider_add_form_hermes_rate_limit_delay_rejects_invalid_number() {
        let mut app = App::new(Some(AppType::Hermes));
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Hermes,
        )));

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.id.set("hermes-provider");
            form.name.set("Hermes Provider");
            form.hermes_base_url.set("https://api.example.com/v1");
            form.hermes_rate_limit_delay.set("..");
        } else {
            panic!("expected ProviderAdd form");
        }

        let action = app.on_key(ctrl(KeyCode::Char('s')), &UiData::default());

        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.toast.as_ref(),
            Some(toast)
                if toast.kind == ToastKind::Warning
                    && toast.message == texts::tui_hermes_rate_limit_delay_invalid()
        ));
    }

    #[test]
    fn provider_add_form_hermes_rate_limit_delay_sanitizes_to_number_chars() {
        let mut app = App::new(Some(AppType::Hermes));
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Hermes,
        )));

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = FormFocus::Fields;
            let fields = form.fields();
            form.field_idx = fields
                .iter()
                .position(|field| *field == ProviderAddField::HermesRateLimitDelay)
                .expect("HermesRateLimitDelay field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        app.on_key(key(KeyCode::Enter), &data());
        app.on_key(key(KeyCode::Char('a')), &data());
        app.on_key(key(KeyCode::Char('0')), &data());
        app.on_key(key(KeyCode::Char('.')), &data());
        app.on_key(key(KeyCode::Char('5')), &data());

        let Some(FormState::ProviderAdd(form)) = app.form.as_ref() else {
            panic!("expected ProviderAdd form");
        };
        assert_eq!(form.hermes_rate_limit_delay.value, "0.5");
    }

    #[test]
    fn provider_add_form_hermes_models_picker_fetch_is_explicit() {
        let mut app = App::new(Some(AppType::Hermes));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Hermes,
        )));

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = FormFocus::Fields;
            form.hermes_base_url.set("https://api.example.com/v1");
            form.hermes_api_key.set("sk-hermes");
            form.custom_user_agent.set("hermes-client/test");
            form.open_hermes_models_picker();
        }
        app.overlay = Overlay::HermesModelsPicker { editing: false };

        let action = app.on_key(key(KeyCode::Char('f')), &data());
        assert!(matches!(
            action,
            Action::ProviderModelFetch {
                base_url,
                api_key: Some(api_key),
                custom_user_agent: Some(custom_user_agent),
                codex_oauth: false,
                codex_oauth_account_id: None,
                field: ProviderAddField::HermesModels,
                claude_idx: None,
            } if base_url == "https://api.example.com/v1"
                && api_key == "sk-hermes"
                && custom_user_agent == "hermes-client/test"
        ));
    }

    #[test]
    fn provider_add_form_hermes_models_picker_enter_does_not_fetch_or_add_models() {
        let mut app = App::new(Some(AppType::Hermes));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Hermes,
        )));

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = FormFocus::Fields;
            form.hermes_base_url.set("https://api.example.com/v1");
            form.hermes_api_key.set("sk-hermes");
            form.open_hermes_models_picker();
            form.hermes_models_field_idx = 0;
        }
        app.overlay = Overlay::HermesModelsPicker { editing: false };

        let action = app.on_key(key(KeyCode::Enter), &data());
        assert!(matches!(action, Action::None));
        let Some(FormState::ProviderAdd(form)) = app.form.as_ref() else {
            panic!("expected ProviderAdd form");
        };
        assert!(
            form.hermes_models.is_empty(),
            "fetching should only open the fetched-model picker"
        );
    }

    #[test]
    fn provider_add_form_hermes_models_picker_fetch_requires_base_url_and_api_key() {
        let mut app = App::new(Some(AppType::Hermes));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Hermes,
        )));

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = FormFocus::Fields;
            form.open_hermes_models_picker();
        }
        app.overlay = Overlay::HermesModelsPicker { editing: false };

        let action = app.on_key(key(KeyCode::Char('f')), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.toast.as_ref(),
            Some(toast) if toast.message == texts::tui_model_fetch_need_config()
        ));
    }

    #[test]
    fn provider_add_form_hermes_models_picker_add_edit_and_delete_model() {
        let mut app = App::new(Some(AppType::Hermes));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Hermes,
        )));

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = FormFocus::Fields;
            form.open_hermes_models_picker();
        }
        app.overlay = Overlay::HermesModelsPicker { editing: false };

        assert!(matches!(
            app.on_key(key(KeyCode::Char('a')), &data()),
            Action::None
        ));
        let Some(FormState::ProviderAdd(form)) = app.form.as_ref() else {
            panic!("expected ProviderAdd form");
        };
        assert_eq!(form.hermes_models, vec![json!({ "id": "", "name": "" })]);
        assert!(matches!(
            form.selected_hermes_model_field(),
            Some(form::HermesModelField::Id(0))
        ));

        app.on_key(key(KeyCode::Enter), &data());
        assert!(matches!(
            app.overlay,
            Overlay::HermesModelsPicker { editing: true }
        ));
        for ch in "gpt-5.4".chars() {
            app.on_key(key(KeyCode::Char(ch)), &data());
        }
        app.on_key(key(KeyCode::Enter), &data());
        assert!(matches!(
            app.overlay,
            Overlay::HermesModelsPicker { editing: false }
        ));

        let Some(FormState::ProviderAdd(form)) = app.form.as_ref() else {
            panic!("expected ProviderAdd form");
        };
        assert_eq!(form.hermes_models[0]["id"], "gpt-5.4");
        assert_eq!(form.hermes_models[0]["name"], "");

        assert!(matches!(
            app.on_key(key(KeyCode::Char('d')), &data()),
            Action::None
        ));
        let Some(FormState::ProviderAdd(form)) = app.form.as_ref() else {
            panic!("expected ProviderAdd form");
        };
        assert!(form.hermes_models.is_empty());
    }

    #[test]
    fn provider_add_form_hermes_models_picker_space_does_not_start_editing() {
        let mut app = App::new(Some(AppType::Hermes));
        let mut form = ProviderAddFormState::new(AppType::Hermes);
        form.open_hermes_models_picker();
        form.add_empty_hermes_model();
        app.form = Some(FormState::ProviderAdd(form));
        app.overlay = Overlay::HermesModelsPicker { editing: false };

        let action = app.on_key(key(KeyCode::Char(' ')), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::HermesModelsPicker { editing: false }
        ));
    }

    #[test]
    fn provider_add_form_hermes_models_picker_preserves_edit_cursor() {
        let mut app = App::new(Some(AppType::Hermes));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Hermes,
        )));

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = FormFocus::Fields;
            form.open_hermes_models_picker();
        }
        app.overlay = Overlay::HermesModelsPicker { editing: false };

        app.on_key(key(KeyCode::Char('a')), &data());
        app.on_key(key(KeyCode::Enter), &data());
        for ch in "gpt-54".chars() {
            app.on_key(key(KeyCode::Char(ch)), &data());
        }
        app.on_key(key(KeyCode::Left), &data());
        app.on_key(key(KeyCode::Char('.')), &data());

        let Some(FormState::ProviderAdd(form)) = app.form.as_ref() else {
            panic!("expected ProviderAdd form");
        };
        assert_eq!(form.hermes_models[0]["id"], "gpt-5.4");
        assert_eq!(form.hermes_model_input.cursor, "gpt-5.".chars().count());
    }

    #[test]
    fn model_fetch_picker_hermes_models_sets_current_model_id_only() {
        let mut app = App::new(Some(AppType::Hermes));
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Hermes,
        )));
        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.open_hermes_models_picker();
            form.add_empty_hermes_model();
            form.set_hermes_model_field_text(form::HermesModelField::Name(0), "Display Name");
        }
        app.overlay = Overlay::ModelFetchPicker {
            request_id: 1,
            field: ProviderAddField::HermesModels,
            claude_idx: None,
            input: TextInput::new("gpt-5.4"),
            query: "gpt-5.4".to_string(),
            fetching: false,
            models: vec!["gpt-5.4".to_string()],
            filtered_indices: None,
            filter_incomplete: false,
            error: None,
            selected_idx: 0,
            selection_active: false,
        };
        app.pending_overlay = Some(Overlay::HermesModelsPicker { editing: false });

        let action = app.on_key(key(KeyCode::Enter), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::HermesModelsPicker { editing: false }
        ));

        let Some(FormState::ProviderAdd(form)) = app.form.as_ref() else {
            panic!("expected ProviderAdd form");
        };
        assert_eq!(form.hermes_models.len(), 1);
        assert_eq!(form.hermes_models[0]["id"], "gpt-5.4");
        assert_eq!(form.hermes_models[0]["name"], "Display Name");
    }

    #[test]
    fn provider_add_form_hermes_id_input_is_sanitized_like_upstream() {
        let mut app = App::new(Some(AppType::Hermes));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Hermes,
        )));

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = FormFocus::Fields;
            form.field_idx = 0;
            assert!(form.begin_main_text_edit(ProviderAddField::Id));
        }

        for ch in "My Provider_1".chars() {
            app.on_key(key(KeyCode::Char(ch)), &data());
        }

        let Some(FormState::ProviderAdd(form)) = app.form.as_ref() else {
            panic!("expected ProviderAdd form");
        };
        assert_eq!(form.id.value, "myprovider1");
    }

    #[test]
    fn provider_template_clears_stale_inline_errors() {
        let mut form = ProviderAddFormState::new(AppType::Claude);
        form.set_main_field_error(ProviderAddField::Name, "stale name error");
        form.set_usage_query_field_error(UsageQueryField::Script, "stale script error");

        form.apply_template(0, &[]);

        assert!(form.field_errors.is_empty());
        assert!(form.usage_query_field_errors.is_empty());
    }

    #[test]
    fn multiline_editor_supports_readline_shortcuts() {
        let mut app = App::new(Some(AppType::Claude));
        app.open_editor(
            "Prompt",
            EditorKind::Plain,
            "first line\nalpha beta",
            EditorSubmit::PromptCreate {
                id: "demo".to_string(),
                name: "Demo".to_string(),
                description: None,
            },
        );
        if let Some(editor) = app.editor.as_mut() {
            editor.cursor_row = 1;
            editor.cursor_col = "alpha beta".chars().count();
        }

        app.on_key(ctrl(KeyCode::Char('a')), &data());
        assert_eq!(app.editor.as_ref().unwrap().cursor_col, 0);

        app.on_key(ctrl(KeyCode::Char('e')), &data());
        app.on_key(ctrl(KeyCode::Char('w')), &data());
        assert_eq!(app.editor.as_ref().unwrap().lines[1], "alpha ");

        app.on_key(alt(KeyCode::Char('b')), &data());
        assert_eq!(app.editor.as_ref().unwrap().cursor_col, 0);
    }

    #[test]
    fn editor_scrolls_within_one_long_wrapped_physical_line() {
        let content = format!("{}TAIL", "x".repeat(120));
        let mut editor = EditorState::new(
            "Long line",
            EditorKind::Plain,
            EditorSubmit::PromptCreate {
                id: "demo".to_string(),
                name: "Demo".to_string(),
                description: None,
            },
            &content,
        );
        editor.cursor_col = content.len();

        editor.ensure_cursor_visible(Size {
            width: 10,
            height: 3,
        });

        assert_eq!(editor.scroll, 0);
        assert!(editor.scroll_subline > 0);
        assert!(editor.cursor_visual_offset_from_scroll(10).0 < 3);
        assert!(editor
            .visible_wrapped_lines(10, 3)
            .iter()
            .any(|line| line.contains("TAIL")));

        editor.apply_text_command(TextEditCommand::MoveLineStart);
        editor.ensure_cursor_visible(Size {
            width: 10,
            height: 3,
        });
        assert_eq!(editor.scroll_subline, 0);
        assert_eq!(editor.cursor_visual_offset_from_scroll(10).0, 0);
    }

    #[test]
    fn editor_large_ascii_line_materializes_only_the_visible_window() {
        let content = format!("{}TAIL", "x".repeat(1_000_000));
        let mut editor = EditorState::new(
            "Large line",
            EditorKind::Plain,
            EditorSubmit::PromptCreate {
                id: "demo".to_string(),
                name: "Demo".to_string(),
                description: None,
            },
            &content,
        );
        editor.cursor_col = content.len();
        let viewport = Size {
            width: 10,
            height: 3,
        };

        let (scroll, subline) = editor.viewport_origin(viewport);
        let visible = editor.visible_wrapped_lines_from(10, 3, scroll, subline);

        assert_eq!(scroll, 0);
        assert_eq!(visible.len(), 1);
        assert!(visible.iter().map(String::len).sum::<usize>() <= 10);
        assert!(visible.last().is_some_and(|line| line.ends_with("TAIL")));
        assert!(
            editor
                .cursor_visual_offset_from_origin(10, scroll, subline)
                .0
                < 3
        );
    }

    #[test]
    fn editor_large_unicode_line_materializes_only_the_cursor_window() {
        let content = format!("{}TAIL", "你".repeat(1_000_000));
        let mut editor = EditorState::new(
            "Large Unicode line",
            EditorKind::Plain,
            EditorSubmit::PromptCreate {
                id: "demo".to_string(),
                name: "Demo".to_string(),
                description: None,
            },
            &content,
        );
        editor.cursor_col = content.len();
        let viewport = Size {
            width: 10,
            height: 3,
        };

        let (scroll, subline) = editor.viewport_origin(viewport);
        let visible = editor.visible_wrapped_lines_from(10, 3, scroll, subline);

        assert_eq!((scroll, subline), (0, 0));
        assert_eq!(visible.len(), 1);
        assert!(UnicodeWidthStr::width(visible[0].as_str()) <= 10);
        assert!(visible[0].chars().count() <= 10);
        assert!(visible[0].ends_with("TAIL"));
        assert!(
            editor
                .cursor_visual_offset_from_origin(10, scroll, subline)
                .0
                < 3
        );
    }

    #[test]
    fn editor_large_zero_width_line_keeps_rendered_character_count_bounded() {
        let content = "\u{0301}".repeat(1_000_000);
        let mut editor = EditorState::new(
            "Large combining line",
            EditorKind::Plain,
            EditorSubmit::PromptCreate {
                id: "demo".to_string(),
                name: "Demo".to_string(),
                description: None,
            },
            &content,
        );
        editor.cursor_col = content.len();

        let visible = editor.visible_wrapped_lines_from(10, 3, 0, 0);

        assert_eq!(visible.len(), 1);
        assert!(visible[0].chars().count() <= 10);
        assert_eq!(editor.cursor_visual_offset_from_origin(10, 0, 0), (0, 9));
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
    fn providers_enter_key_imports_current_config_when_empty() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());

        assert!(matches!(action, Action::ProviderImportLiveConfig));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn providers_i_key_is_noop() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let action = app.on_key(key(KeyCode::Char('i')), &UiData::default());

        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
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
    fn providers_r_key_refreshes_official_quota() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.current_id = "official".to_string();
        let mut provider = crate::provider::Provider::with_id(
            "official".to_string(),
            "Claude Official".to_string(),
            json!({"env": {}}),
            None,
        );
        provider.category = Some("official".to_string());
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "official".to_string(),
            provider,
            api_url: None,
            is_current: true,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: None,
            default_model_id: None,
        });

        let action = app.on_key(key(KeyCode::Char('r')), &data);
        assert!(matches!(action, Action::ProviderQuotaRefresh { id } if id == "official"));
    }

    #[test]
    fn providers_r_key_ignores_non_official_quota() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "custom".to_string(),
            provider: crate::provider::Provider::with_id(
                "custom".to_string(),
                "Custom".to_string(),
                json!({"env": {"ANTHROPIC_BASE_URL": "https://example.com"}}),
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

        let action = app.on_key(key(KeyCode::Char('r')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.toast.as_ref(),
            Some(toast)
                if toast.kind == ToastKind::Info
                    && toast.message == texts::tui_toast_quota_not_available()
        ));
    }

    #[test]
    fn providers_c_key_opens_copy_confirm() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(claude_provider_row("p1"));

        let action = app.on_key(key(KeyCode::Char('c')), &data);
        assert!(matches!(action, Action::None));
        assert_provider_copy_confirm(&app, "p1", "Provider One");
    }

    #[test]
    fn providers_c_key_opens_copy_confirm_for_openclaw() {
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
        assert_provider_copy_confirm(&app, "p1", "Provider One");
    }

    #[test]
    fn providers_t_key_opens_test_menu() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(claude_provider_row("p1"));

        let action = app.on_key(key(KeyCode::Char('t')), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::ProviderTestMenu {
                ref provider_id,
                selected: 0
            } if provider_id == "p1"
        ));
    }

    #[test]
    fn provider_test_menu_enter_runs_speedtest() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.overlay = Overlay::ProviderTestMenu {
            provider_id: "p1".to_string(),
            selected: 0,
        };

        let mut data = UiData::default();
        data.providers.rows.push(claude_provider_row("p1"));

        let action = app.on_key(key(KeyCode::Enter), &data);

        assert!(
            matches!(action, Action::ProviderSpeedtest { ref url } if url == "https://example.com")
        );
        assert!(
            matches!(app.overlay, Overlay::SpeedtestRunning { ref url } if url == "https://example.com")
        );
    }

    #[test]
    fn provider_test_menu_space_is_noop() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.overlay = Overlay::ProviderTestMenu {
            provider_id: "p1".to_string(),
            selected: 0,
        };

        let mut data = UiData::default();
        data.providers.rows.push(claude_provider_row("p1"));

        let action = app.on_key(key(KeyCode::Char(' ')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::ProviderTestMenu {
                ref provider_id,
                selected: 0
            } if provider_id == "p1"
        ));
    }

    #[test]
    fn provider_test_menu_second_item_runs_stream_check() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.overlay = Overlay::ProviderTestMenu {
            provider_id: "p1".to_string(),
            selected: 1,
        };

        let mut data = UiData::default();
        data.providers.rows.push(claude_provider_row("p1"));

        let action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(action, Action::ProviderStreamCheck { ref id } if id == "p1"));
        assert!(
            matches!(app.overlay, Overlay::StreamCheckRunning { ref provider_name, .. } if provider_name == "Provider One")
        );
    }

    #[test]
    fn provider_test_menu_t_key_is_noop() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.overlay = Overlay::ProviderTestMenu {
            provider_id: "p1".to_string(),
            selected: 0,
        };

        let mut data = UiData::default();
        data.providers.rows.push(claude_provider_row("p1"));

        let action = app.on_key(key(KeyCode::Char('t')), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::ProviderTestMenu {
                ref provider_id,
                selected: 0
            } if provider_id == "p1"
        ));
    }

    #[test]
    fn provider_test_menu_c_key_is_noop() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.overlay = Overlay::ProviderTestMenu {
            provider_id: "p1".to_string(),
            selected: 1,
        };

        let mut data = UiData::default();
        data.providers.rows.push(claude_provider_row("p1"));

        let action = app.on_key(key(KeyCode::Char('c')), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::ProviderTestMenu {
                ref provider_id,
                selected: 1
            } if provider_id == "p1"
        ));
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
        assert!(matches!(remove_action, Action::None));
        assert_provider_remove_confirm(&app, "p1", "Provider One");
    }

    #[test]
    fn opencode_providers_s_key_adds_or_removes_live_config_membership() {
        let mut app = App::new(Some(AppType::OpenCode));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"options":{"baseURL":"https://example.com"}}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: false,
            is_saved: true,
            is_default_model: false,
            primary_model_id: Some("main".to_string()),
            default_model_id: None,
        });

        let add_action = app.on_key(key(KeyCode::Char('s')), &data);
        assert!(matches!(add_action, Action::ProviderSwitch { id } if id == "p1"));

        data.providers.rows[0].is_in_config = true;
        let remove_action = app.on_key(key(KeyCode::Char('s')), &data);
        assert!(matches!(remove_action, Action::None));
        assert_provider_remove_confirm(&app, "p1", "Provider One");
    }

    #[test]
    fn hermes_providers_s_key_adds_or_prompts_to_remove_live_config_membership() {
        let mut app = App::new(Some(AppType::Hermes));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({
                    "base_url": "https://example.com",
                    "api_key": "sk-demo",
                    "models": [{"id": "main"}]
                }),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: false,
            is_saved: true,
            is_default_model: false,
            primary_model_id: Some("main".to_string()),
            default_model_id: None,
        });

        let add_action = app.on_key(key(KeyCode::Char('s')), &data);
        assert!(matches!(add_action, Action::ProviderSwitch { id } if id == "p1"));

        data.providers.rows[0].is_in_config = true;
        let remove_action = app.on_key(key(KeyCode::Char('s')), &data);
        assert!(matches!(remove_action, Action::None));
        assert_provider_remove_confirm(&app, "p1", "Provider One");
    }

    #[test]
    fn provider_remove_confirm_enter_removes_from_config() {
        let mut app = App::new(Some(AppType::OpenCode));
        app.overlay = Overlay::Confirm(ConfirmOverlay {
            title: texts::tui_confirm_remove_provider_title().to_string(),
            message: texts::tui_confirm_remove_provider_message("Provider One"),
            action: ConfirmAction::ProviderRemoveFromConfig {
                id: "p1".to_string(),
            },
        });

        let action = app.on_key(key(KeyCode::Enter), &data());

        assert!(matches!(action, Action::ProviderRemoveFromConfig { id } if id == "p1"));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn hermes_providers_x_key_enables_provider_from_live_config() {
        let mut app = App::new(Some(AppType::Hermes));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({
                    "base_url": "https://example.com",
                    "api_key": "sk-demo",
                    "models": [{"id": "main"}]
                }),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: Some("main".to_string()),
            default_model_id: None,
        });

        let action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(
            action,
            Action::ProviderSetDefaultModel { provider_id, model_id }
                if provider_id == "p1" && model_id == "main"
        ));
    }

    #[test]
    fn hermes_providers_s_key_blocks_removing_active_provider() {
        let mut app = App::new(Some(AppType::Hermes));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({
                    "base_url": "https://example.com",
                    "api_key": "sk-demo",
                    "models": [{"id": "main"}]
                }),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: true,
            is_in_config: true,
            is_saved: true,
            is_default_model: true,
            primary_model_id: Some("main".to_string()),
            default_model_id: None,
        });

        let action = app.on_key(key(KeyCode::Char('s')), &data);
        assert!(matches!(action, Action::None));
        assert!(app.toast.is_some());
        assert!(matches!(app.overlay, Overlay::None));
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
    fn hermes_providers_e_key_blocks_read_only_provider() {
        let mut app = App::new(Some(AppType::Hermes));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "remote".to_string(),
            provider: crate::provider::Provider::with_id(
                "remote".to_string(),
                "Remote".to_string(),
                json!({
                    "_cc_source": crate::hermes_config::PROVIDER_SOURCE_DICT,
                    "base_url": "https://example.com",
                    "models": [{"id": "main"}]
                }),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: Some("main".to_string()),
            default_model_id: None,
        });

        let action = app.on_key(key(KeyCode::Char('e')), &data);
        assert!(matches!(action, Action::None));
        assert!(
            app.form.is_none(),
            "read-only provider should not open edit form"
        );
        assert!(matches!(
            app.toast.as_ref(),
            Some(toast)
                if toast.kind == ToastKind::Info
                    && toast.message == texts::tui_toast_provider_managed_by_hermes()
        ));
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
        assert!(matches!(action, Action::None));
        assert_provider_remove_confirm(&app, "p2", "Provider Two");
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
    fn hermes_providers_d_key_allows_deleting_current_writable_provider() {
        let mut app = App::new(Some(AppType::Hermes));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "local".to_string(),
            provider: crate::provider::Provider::with_id(
                "local".to_string(),
                "Local".to_string(),
                json!({
                    "_cc_source": crate::hermes_config::PROVIDER_SOURCE_CUSTOM_LIST,
                    "base_url": "https://example.com",
                    "models": [{"id": "main"}]
                }),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: true,
            is_in_config: true,
            is_saved: true,
            is_default_model: true,
            primary_model_id: Some("main".to_string()),
            default_model_id: None,
        });

        let action = app.on_key(key(KeyCode::Char('d')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::ProviderDelete { id },
                ..
            }) if id == "local"
        ));
        assert!(
            app.toast.is_none(),
            "Hermes additive current provider delete should not be blocked"
        );
    }

    #[test]
    fn hermes_providers_d_key_blocks_read_only_provider() {
        let mut app = App::new(Some(AppType::Hermes));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "remote".to_string(),
            provider: crate::provider::Provider::with_id(
                "remote".to_string(),
                "Remote".to_string(),
                json!({
                    "_cc_source": crate::hermes_config::PROVIDER_SOURCE_DICT,
                    "base_url": "https://example.com",
                    "models": [{"id": "main"}]
                }),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: Some("main".to_string()),
            default_model_id: None,
        });

        let action = app.on_key(key(KeyCode::Char('d')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        assert!(matches!(
            app.toast.as_ref(),
            Some(toast)
                if toast.kind == ToastKind::Info
                    && toast.message == texts::tui_toast_provider_managed_by_hermes()
        ));
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
    fn mcp_space_key_toggles_current_app() {
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
            }
            .into(),
        });

        let action = app.on_key(key(KeyCode::Char(' ')), &data);
        assert!(matches!(
            action,
            Action::McpToggle {
                id,
                enabled: true
            } if id == "m1"
        ));
    }

    #[test]
    fn mcp_x_key_does_not_toggle_current_app() {
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
            }
            .into(),
        });

        let action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(action, Action::None));
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
            }
            .into(),
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
    fn mcp_apps_picker_space_toggles_selected_app_and_enter_emits_action() {
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
            }
            .into(),
        });

        app.on_key(key(KeyCode::Char('m')), &data);

        let action = app.on_key(key(KeyCode::Char(' ')), &data);
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
    fn mcp_apps_picker_x_does_not_toggle_selected_app() {
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
            }
            .into(),
        });

        app.on_key(key(KeyCode::Char('m')), &data);

        let action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::McpAppsPicker { apps, .. } if !apps.codex
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
            }
            .into(),
        });

        app.on_key(key(KeyCode::Char('m')), &data);
        app.on_key(key(KeyCode::Down), &data);
        app.on_key(key(KeyCode::Down), &data);
        app.on_key(key(KeyCode::Down), &data);

        let action = app.on_key(key(KeyCode::Char(' ')), &data);
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
    fn mcp_apps_picker_can_select_hermes() {
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
            }
            .into(),
        });

        app.on_key(key(KeyCode::Char('m')), &data);
        app.on_key(key(KeyCode::Down), &data);
        app.on_key(key(KeyCode::Down), &data);
        app.on_key(key(KeyCode::Down), &data);
        app.on_key(key(KeyCode::Down), &data);

        let action = app.on_key(key(KeyCode::Char(' ')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::McpAppsPicker { selected, apps, .. } if *selected == 4 && apps.hermes
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            action,
            Action::McpSetApps { id, apps }
                if id == "m1"
                    && !apps.claude
                    && !apps.codex
                    && !apps.gemini
                    && !apps.opencode
                    && apps.hermes
        ));
    }

    #[test]
    fn mcp_apps_picker_from_openclaw_targets_hermes_last_visible_row() {
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
            }
            .into(),
        });

        let action = app.on_key(key(KeyCode::Char('m')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::McpAppsPicker { selected, .. } if *selected == 4
        ));

        let action = app.on_key(key(KeyCode::Char(' ')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::McpAppsPicker { selected, apps, .. }
                if *selected == 4
                    && !apps.claude
                    && !apps.codex
                    && !apps.gemini
                    && !apps.opencode
                    && apps.hermes
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
            }
            .into(),
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
    fn prompts_space_key_toggles_activate_and_deactivate_actions() {
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

        let action = app.on_key(key(KeyCode::Char(' ')), &data);
        assert!(matches!(action, Action::PromptActivate { id } if id == "pr1"));

        data.prompts.rows[0].prompt.enabled = true;
        let action = app.on_key(key(KeyCode::Char(' ')), &data);
        assert!(matches!(action, Action::PromptDeactivate { id } if id == "pr1"));
    }

    #[test]
    fn prompts_x_key_no_longer_deactivates_active_prompt() {
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
                enabled: true,
                created_at: None,
                updated_at: None,
            },
        });

        let action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(action, Action::None));
    }

    #[test]
    fn prompts_route_prompts_once_when_empty_and_live_prompt_exists() {
        let mut app = App::new(Some(AppType::Claude));
        let mut data = UiData::default();
        data.prompts.import_candidate =
            Some(prompt_import_candidate("CLAUDE.md", "# Existing prompt"));

        app.set_route_no_history(Route::Prompts);
        app.maybe_prompt_import_candidate(&data);
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::PromptOpenImportCandidate { .. },
                ..
            })
        ));

        app.overlay = Overlay::None;
        app.maybe_prompt_import_candidate(&data);
        assert!(
            matches!(app.overlay, Overlay::None),
            "the import prompt should not repeat in the same TUI session after dismissal"
        );
    }

    #[test]
    fn prompts_import_prompt_no_dismisses_without_repeating() {
        let mut app = App::new(Some(AppType::Claude));
        let mut data = UiData::default();
        data.prompts.import_candidate =
            Some(prompt_import_candidate("CLAUDE.md", "# Existing prompt"));

        app.set_route_no_history(Route::Prompts);
        app.maybe_prompt_import_candidate(&data);

        let action = app.on_key(key(KeyCode::Char('n')), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        assert!(app.form.is_none());

        app.maybe_prompt_import_candidate(&data);
        assert!(
            matches!(app.overlay, Overlay::None),
            "declining should suppress the import prompt for this app in the same TUI session"
        );
    }

    #[test]
    fn prompts_import_prompt_esc_dismisses_without_repeating() {
        let mut app = App::new(Some(AppType::Claude));
        let mut data = UiData::default();
        data.prompts.import_candidate =
            Some(prompt_import_candidate("CLAUDE.md", "# Existing prompt"));

        app.set_route_no_history(Route::Prompts);
        app.maybe_prompt_import_candidate(&data);

        let action = app.on_key(key(KeyCode::Esc), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        assert!(app.form.is_none());

        app.maybe_prompt_import_candidate(&data);
        assert!(
            matches!(app.overlay, Overlay::None),
            "escaping should suppress the import prompt for this app in the same TUI session"
        );
    }

    #[test]
    fn prompts_import_prompt_is_tracked_per_app() {
        let mut app = App::new(Some(AppType::Claude));
        let mut data = UiData::default();
        data.prompts.import_candidate =
            Some(prompt_import_candidate("CLAUDE.md", "# Existing prompt"));

        app.set_route_no_history(Route::Prompts);
        app.maybe_prompt_import_candidate(&data);
        app.overlay = Overlay::None;

        app.app_type = AppType::Codex;
        data.prompts.import_candidate = Some(prompt_import_candidate(
            "AGENTS.md",
            "# Existing codex prompt",
        ));
        app.maybe_prompt_import_candidate(&data);

        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::PromptOpenImportCandidate { .. },
                ..
            })
        ));
    }

    #[test]
    fn prompts_route_does_not_prompt_when_prompt_rows_exist() {
        let mut app = App::new(Some(AppType::Claude));
        let mut data = UiData::default();
        data.prompts.import_candidate =
            Some(prompt_import_candidate("CLAUDE.md", "# Existing prompt"));
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

        app.set_route_no_history(Route::Prompts);
        app.maybe_prompt_import_candidate(&data);

        assert!(matches!(app.overlay, Overlay::None));
        assert!(app.prompt_import_prompted_apps.is_empty());
    }

    #[test]
    #[serial]
    fn prompts_route_detects_live_prompt_file_and_prompts_import() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = TestEnvGuard::isolated(temp.path());
        let prompt_path =
            crate::prompt_files::prompt_file_path(&AppType::Claude).expect("prompt path");
        std::fs::create_dir_all(prompt_path.parent().expect("prompt parent"))
            .expect("create prompt parent");
        std::fs::write(&prompt_path, "# Existing prompt").expect("write prompt file");

        let mut app = App::new(Some(AppType::Claude));
        let mut data = UiData::load(&app.app_type).expect("load ui data");
        let candidate = data
            .prompts
            .import_candidate
            .as_ref()
            .expect("import candidate");
        assert_eq!(candidate.filename, "CLAUDE.md");
        assert_eq!(candidate.content, "# Existing prompt");

        run_runtime_action(&mut app, &mut data, Action::SwitchRoute(Route::Prompts))
            .expect("switch to prompts");

        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::PromptOpenImportCandidate { .. },
                ..
            })
        ));
    }

    #[test]
    #[serial]
    fn prompts_reload_data_detects_live_prompt_file_and_prompts_import() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = TestEnvGuard::isolated(temp.path());
        let prompt_path =
            crate::prompt_files::prompt_file_path(&AppType::Claude).expect("prompt path");
        std::fs::create_dir_all(prompt_path.parent().expect("prompt parent"))
            .expect("create prompt parent");
        std::fs::write(&prompt_path, "# Existing prompt").expect("write prompt file");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;
        let mut data = UiData::default();

        run_runtime_action(&mut app, &mut data, Action::ReloadData).expect("reload data");

        assert_eq!(
            data.prompts
                .import_candidate
                .as_ref()
                .expect("import candidate")
                .content,
            "# Existing prompt"
        );
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::PromptOpenImportCandidate { .. },
                ..
            })
        ));
    }

    #[test]
    #[serial]
    fn prompts_switch_app_detects_live_prompt_file_and_prompts_import() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = TestEnvGuard::isolated(temp.path());
        let prompt_path =
            crate::prompt_files::prompt_file_path(&AppType::Codex).expect("prompt path");
        std::fs::create_dir_all(prompt_path.parent().expect("prompt parent"))
            .expect("create prompt parent");
        std::fs::write(&prompt_path, "# Codex prompt").expect("write prompt file");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;
        let mut data = UiData::load(&app.app_type).expect("load claude data");

        run_runtime_action(&mut app, &mut data, Action::SetAppType(AppType::Codex))
            .expect("switch app");

        assert_eq!(app.app_type, AppType::Codex);
        assert_eq!(
            data.prompts
                .import_candidate
                .as_ref()
                .expect("import candidate")
                .filename,
            "AGENTS.md"
        );
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::PromptOpenImportCandidate { .. },
                ..
            })
        ));
    }

    #[test]
    #[serial]
    fn legacy_config_migration_leaves_live_prompt_for_tui_import_prompt() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = TestEnvGuard::isolated(temp.path());
        let mut legacy_config = crate::app_config::MultiAppConfig::default();
        legacy_config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager")
            .providers
            .insert(
                "provider-one".to_string(),
                claude_provider_row("provider-one").provider,
            );
        legacy_config.save().expect("write legacy config");
        let prompt_path =
            crate::prompt_files::prompt_file_path(&AppType::Claude).expect("prompt path");
        std::fs::create_dir_all(prompt_path.parent().expect("prompt parent"))
            .expect("create prompt parent");
        std::fs::write(&prompt_path, "# Existing prompt").expect("write prompt file");

        let state = crate::AppState::try_new().expect("migrate legacy config");

        assert!(
            PromptService::get_prompts(&state, AppType::Claude)
                .expect("load prompts")
                .is_empty(),
            "legacy config migration must not silently import live prompt files"
        );
        let data = UiData::load(&AppType::Claude).expect("load ui data");
        assert_eq!(
            data.prompts
                .import_candidate
                .as_ref()
                .expect("import candidate")
                .content,
            "# Existing prompt"
        );
    }

    #[test]
    fn config_common_snippet_opens_editor_directly() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Config;
        app.focus = Focus::Content;
        app.config_idx = ConfigItem::ALL
            .iter()
            .position(|item| matches!(item, ConfigItem::CommonSnippet))
            .expect("CommonSnippet missing from ConfigItem::ALL");

        let data = UiData::default();
        app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            app.editor.as_ref().map(|e| (&e.kind, &e.submit)),
            Some((
                EditorKind::Json,
                EditorSubmit::ConfigCommonSnippet {
                    app_type: AppType::Claude,
                    source: CommonSnippetViewSource::Global
                }
            ))
        ));
    }

    #[test]
    fn config_common_snippet_codex_opens_toml_editor_directly() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Config;
        app.focus = Focus::Content;
        app.config_idx = ConfigItem::ALL
            .iter()
            .position(|item| matches!(item, ConfigItem::CommonSnippet))
            .expect("CommonSnippet missing from ConfigItem::ALL");

        let mut data = UiData::default();
        data.config.common_snippet = "disable_response_storage = true".to_string();

        app.on_key(key(KeyCode::Enter), &data);
        let editor = app.editor.as_ref().expect("expected common snippet editor");
        assert_eq!(editor.kind, EditorKind::Toml);
        assert_eq!(
            editor.submit,
            EditorSubmit::ConfigCommonSnippet {
                app_type: AppType::Codex,
                source: CommonSnippetViewSource::Global
            }
        );
        assert!(editor.text().contains("disable_response_storage"));
    }

    #[test]
    fn common_snippet_picker_opens_editor_for_non_current_app() {
        let mut app = App::new(Some(AppType::Claude));
        app.overlay = Overlay::CommonSnippetPicker {
            selected: snippet_picker_index_for_app_type(&AppType::Codex),
        };

        let mut data = UiData::default();
        data.config.common_snippets.codex = Some("disable_response_storage = true".to_string());

        app.on_key(key(KeyCode::Enter), &data);

        let editor = app.editor.as_ref().expect("expected Codex snippet editor");
        assert_eq!(editor.kind, EditorKind::Toml);
        assert_eq!(
            editor.submit,
            EditorSubmit::ConfigCommonSnippet {
                app_type: AppType::Codex,
                source: CommonSnippetViewSource::Global
            }
        );
        assert!(
            editor.text().contains("disable_response_storage"),
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
    fn provider_add_form_common_snippet_row_opens_json_editor_claude() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = data();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields
        select_provider_common_snippet_row(&mut app);

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.editor.as_ref().map(|e| (&e.kind, &e.submit)),
            Some((
                EditorKind::Json,
                EditorSubmit::ConfigCommonSnippet {
                    app_type: AppType::Claude,
                    source: CommonSnippetViewSource::ProviderForm
                }
            ))
        ));
    }

    #[test]
    fn common_snippet_editor_function_keys_trigger_format_and_extract() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = data();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields
        select_provider_common_snippet_row(&mut app);
        app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(
            app.on_key(key(KeyCode::F(2)), &data),
            Action::EditorFormatCommonSnippet {
                app_type: AppType::Claude
            }
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::F(4)), &data),
            Action::EditorExtractCommonSnippet {
                app_type: AppType::Claude
            }
        ));
    }

    #[test]
    fn global_common_snippet_editor_only_formats_not_extracts() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Config;
        app.focus = Focus::Content;
        app.open_common_snippet_editor(
            AppType::Claude,
            &data(),
            Some(r#"{"env":{"COMMON_FLAG":"1"}}"#.to_string()),
            CommonSnippetViewSource::Global,
        );

        assert!(matches!(
            app.on_key(key(KeyCode::F(2)), &data()),
            Action::EditorFormatCommonSnippet {
                app_type: AppType::Claude
            }
        ));

        let before = app.editor.as_ref().map(|editor| editor.text());
        assert!(matches!(
            app.on_key(key(KeyCode::F(4)), &data()),
            Action::None
        ));
        assert_eq!(app.editor.as_ref().map(|editor| editor.text()), before);
    }

    #[test]
    fn provider_add_form_common_snippet_row_opens_toml_editor_codex() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = data();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields
        select_provider_common_snippet_row(&mut app);

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.editor.as_ref().map(|e| (&e.kind, &e.submit)),
            Some((
                EditorKind::Toml,
                EditorSubmit::ConfigCommonSnippet {
                    app_type: AppType::Codex,
                    source: CommonSnippetViewSource::ProviderForm
                }
            ))
        ));
    }

    #[test]
    fn provider_add_form_common_snippet_row_opens_json_editor_gemini() {
        let mut app = App::new(Some(AppType::Gemini));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = data();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields
        select_provider_common_snippet_row(&mut app);

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.editor.as_ref().map(|e| (&e.kind, &e.submit)),
            Some((
                EditorKind::Json,
                EditorSubmit::ConfigCommonSnippet {
                    app_type: AppType::Gemini,
                    source: CommonSnippetViewSource::ProviderForm
                }
            ))
        ));
    }

    #[test]
    fn provider_add_form_first_open_shows_common_config_notice_for_supported_apps() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.common_config_notice_confirmed = false;

        let action = app.on_key(key(KeyCode::Char('a')), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(app.form, Some(FormState::ProviderAdd(_))));
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::CommonConfigNotice,
                ..
            })
        ));
    }

    #[test]
    fn provider_add_form_skips_common_config_notice_after_confirmed() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = data();

        let action = app.on_key(key(KeyCode::Char('a')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(app.form, Some(FormState::ProviderAdd(_))));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn provider_add_form_skips_common_config_notice_for_unsupported_apps() {
        let mut app = App::new(Some(AppType::OpenCode));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.common_config_notice_confirmed = false;

        let action = app.on_key(key(KeyCode::Char('a')), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(app.form, Some(FormState::ProviderAdd(_))));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn provider_copy_confirm_opens_form_without_notice_after_common_config_confirmed() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(claude_provider_row("p1"));

        app.on_key(key(KeyCode::Char('c')), &data);
        let action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(app.form, Some(FormState::ProviderAdd(_))));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn provider_copy_confirm_save_submits_new_copy_with_collision_free_id() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.common_config_notice_confirmed = true;

        let mut data = UiData::default();
        data.providers.rows.push(claude_provider_row("p1"));
        data.providers
            .rows
            .push(claude_provider_row("provider-one-copy"));

        app.on_key(key(KeyCode::Char('c')), &data);
        app.on_key(key(KeyCode::Enter), &data);
        let action = app.on_key(ctrl(KeyCode::Char('s')), &data);

        let Action::EditorSubmit { submit, content } = action else {
            panic!("saving a copied provider should submit new provider content");
        };
        assert!(matches!(submit, EditorSubmit::ProviderAdd));

        let copied: serde_json::Value =
            serde_json::from_str(&content).expect("copy payload should be valid JSON");
        assert_eq!(copied["id"], "p1-copy");
        assert_eq!(copied["name"], "Provider One copy");
        assert_eq!(
            copied["settingsConfig"]["env"]["ANTHROPIC_AUTH_TOKEN"],
            "sk-demo"
        );
    }

    #[test]
    #[serial]
    fn provider_copy_confirm_save_persists_with_source_id_copy_semantics() {
        let temp_home = TempDir::new().expect("create temp home");
        let _home = TestEnvGuard::isolated(temp_home.path());

        let state = crate::cli::tui::data::load_state().expect("load state");
        {
            let mut config = state.config.write().expect("lock config");
            let manager = config
                .get_manager_mut(&AppType::Claude)
                .expect("claude manager");

            let mut original = Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_BASE_URL": "https://example.com",
                        "ANTHROPIC_AUTH_TOKEN": "sk-demo"
                    }
                }),
                None,
            );
            original.sort_index = Some(4);
            manager.providers.insert(original.id.clone(), original);

            let mut existing_copy = Provider::with_id(
                "p1-copy".to_string(),
                "Existing Copy".to_string(),
                json!({"env": {"ANTHROPIC_BASE_URL": "https://copy.example"}}),
                None,
            );
            existing_copy.sort_index = Some(5);
            manager
                .providers
                .insert(existing_copy.id.clone(), existing_copy);
        }
        state.save().expect("persist providers");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.common_config_notice_confirmed = true;
        let mut data = UiData::load(&AppType::Claude).expect("load data");

        app.on_key(key(KeyCode::Char('c')), &data);
        app.on_key(key(KeyCode::Enter), &data);
        let action = app.on_key(ctrl(KeyCode::Char('s')), &data);

        run_runtime_action(&mut app, &mut data, action).expect("save copied provider");

        let refreshed = UiData::load(&AppType::Claude).expect("reload data");
        let copied = refreshed
            .providers
            .rows
            .iter()
            .find(|row| row.id == "p1-copy-2")
            .expect("copy should use source id copy suffix");
        assert_eq!(copied.provider.name, "Provider One copy");
        assert_eq!(copied.provider.sort_index, Some(5));
        assert!(copied.provider.created_at.is_some());
        assert_eq!(
            copied.provider.settings_config["env"]["ANTHROPIC_AUTH_TOKEN"],
            "sk-demo"
        );

        let shifted = refreshed
            .providers
            .rows
            .iter()
            .find(|row| row.id == "p1-copy")
            .expect("existing copy remains");
        assert_eq!(shifted.provider.sort_index, Some(6));
    }

    #[test]
    fn provider_copy_confirm_preserves_first_common_config_notice() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.common_config_notice_confirmed = false;

        let mut data = UiData::default();
        data.providers.rows.push(claude_provider_row("p1"));

        app.on_key(key(KeyCode::Char('c')), &data);
        let action = app.on_key(key(KeyCode::Enter), &data);

        assert!(matches!(action, Action::None));
        assert!(matches!(app.form, Some(FormState::ProviderAdd(_))));
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::CommonConfigNotice,
                ..
            })
        ));
    }

    #[test]
    fn common_config_notice_enter_esc_and_no_all_mark_confirmed() {
        for key_code in [KeyCode::Enter, KeyCode::Esc, KeyCode::Char('n')] {
            let mut app = App::new(Some(AppType::Claude));
            app.overlay = Overlay::Confirm(ConfirmOverlay {
                title: texts::tui_common_config_notice_title().to_string(),
                message: texts::tui_common_config_notice_message(AppType::Claude.as_str()),
                action: ConfirmAction::CommonConfigNotice,
            });

            let action = app.on_key(key(key_code), &data());
            assert!(matches!(action, Action::ConfirmCommonConfigNotice));
            assert!(matches!(app.overlay, Overlay::None));
        }
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
            form.clear_text_edit();
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
            form.clear_text_edit();
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
    fn config_cloud_sync_item_opens_backend_menu() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Config;
        app.focus = Focus::Content;
        app.config_idx = visible_config_items(&app.filter, &app.app_type)
            .iter()
            .position(|item| matches!(item, ConfigItem::CloudSync))
            .expect("CloudSync should be visible in the filtered config menu");

        let data = UiData::default();
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            action,
            Action::SwitchRoute(Route::ConfigCloudSync)
        ));
        assert!(matches!(app.route, Route::ConfigCloudSync));
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
    fn hermes_and_openclaw_nav_config_enter_opens_config_panel() {
        for app_type in [AppType::Hermes, AppType::OpenClaw] {
            let mut app = App::new(Some(app_type));
            app.focus = Focus::Nav;
            app.nav_idx = nav_index(&app, NavItem::Config);

            let action = app.on_key(key(KeyCode::Enter), &UiData::default());

            assert!(matches!(action, Action::SwitchRoute(Route::Config)));
            assert_eq!(app.route, Route::Config);
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
        app.filter.input.set("workspace".to_string());
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
        assert!(app.filter.input.value.is_empty());
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
        app.filter.input.set("focus".to_string());
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
        assert!(app.filter.input.value.is_empty());
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
            Overlay::TextInput(TextInputState { submit, input, .. })
                if *submit == TextSubmit::OpenClawDailyMemoryFilename
                    && (input.value == before || input.value == after)
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
            input: TextInput::new("bad-name.md".to_string()),
            submit: TextSubmit::OpenClawDailyMemoryFilename,
        });

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());

        assert!(matches!(action, Action::None));
        assert!(
            app.editor.is_none(),
            "invalid filename should not open the editor"
        );
        assert!(matches!(
            &app.overlay,
            Overlay::TextInput(TextInputState { input, .. }) if input.value == "bad-name.md"
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
        assert_eq!(app.filter.input.value, "m");
    }

    #[test]
    fn openclaw_daily_memory_search_dispatches_on_enter_after_filter_edits() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawDailyMemory;
        app.filter.active = true;
        app.filter.input.set("focus".to_string());

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
        let _env = TestEnvGuard::isolated(temp_home.path());
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
        let _env = TestEnvGuard::isolated(temp_home.path());
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
        let _env = TestEnvGuard::isolated(temp_home.path());
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
        let _env = TestEnvGuard::isolated(temp_home.path());
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
        let _env = TestEnvGuard::isolated(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawDailyMemory;
        app.focus = Focus::Content;
        app.overlay = Overlay::TextInput(TextInputState {
            title: texts::tui_openclaw_daily_memory_create_title().to_string(),
            prompt: texts::tui_openclaw_daily_memory_create_prompt().to_string(),
            input: TextInput::new("2026-03-20.md".to_string()),
            submit: TextSubmit::OpenClawDailyMemoryFilename,
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
        let _env = TestEnvGuard::isolated(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawDailyMemory;
        app.focus = Focus::Content;
        app.overlay = Overlay::TextInput(TextInputState {
            title: texts::tui_openclaw_daily_memory_create_title().to_string(),
            prompt: texts::tui_openclaw_daily_memory_create_prompt().to_string(),
            input: TextInput::new("2026-03-20.md".to_string()),
            submit: TextSubmit::OpenClawDailyMemoryFilename,
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
        let _env = TestEnvGuard::isolated(temp_home.path());
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
        let _env = TestEnvGuard::isolated(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawDailyMemory;
        app.filter.input.set("focus".to_string());
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
        let _env = TestEnvGuard::isolated(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawDailyMemory;
        app.filter.input.set("focus".to_string());
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
        let _env = TestEnvGuard::isolated(temp_home.path());
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
        let _env = TestEnvGuard::isolated(temp_home.path());
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
        let _env = TestEnvGuard::isolated(temp_home.path());
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
            Overlay::TextInput(TextInputState { ref input, .. }) if input.value.is_empty()
        ));

        for ch in ['/', '?', '[', ']', 'q'] {
            assert!(matches!(
                app.on_key(key(KeyCode::Char(ch)), &data),
                Action::None
            ));
        }

        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState { ref input, .. }) if input.value == "/?[]q"
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
        assert!(matches!(app.overlay, Overlay::Help(_)));
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
            Overlay::TextInput(TextInputState { ref input, .. }) if input.value == "hjkl"
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
            Overlay::TextInput(TextInputState { ref input, .. }) if input.value == "Read"
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
            Overlay::TextInput(TextInputState { ref input, .. }) if input.value.is_empty()
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
            Overlay::TextInput(TextInputState { ref input, .. }) if input.value == "Read"
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
            Overlay::TextInput(TextInputState { ref input, .. }) if input.value.is_empty()
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
            Overlay::TextInput(TextInputState { input, .. }) => {
                format!("text-input:{}", input.value)
            }
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
            Overlay::TextInput(TextInputState { ref input, .. }) if input.value.is_empty()
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
            Overlay::TextInput(TextInputState { ref input, .. }) if input.value.is_empty()
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
            Overlay::TextInput(TextInputState { ref input, .. }) if input.value.is_empty()
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
    fn openclaw_agents_primary_picker_preserves_large_catalog_and_complete_values() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::ConfigOpenClawAgents;
        app.focus = Focus::Content;

        let middle = 5_000usize;
        let huge_model_id = format!("huge-model-{}", "x".repeat(2 * 1024 * 1024));
        let mut models = (0..10_000)
            .map(|index| (format!("model-{index:05}"), format!("Model {index:05}")))
            .collect::<Vec<_>>();
        models[middle].0 = huge_model_id.clone();
        let model_refs = models
            .iter()
            .map(|(id, name)| (id.as_str(), name.as_str()))
            .collect::<Vec<_>>();
        let primary = format!("demo/{huge_model_id}");

        let mut data = UiData::default();
        data.providers.rows = vec![openclaw_provider_row("demo", "Demo Provider", &model_refs)];
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(crate::openclaw_config::OpenClawDefaultModel {
                    primary: primary.clone(),
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
        let Overlay::OpenClawAgentsFallbackPicker {
            selected,
            active,
            options,
            ..
        } = &app.overlay
        else {
            panic!("expected primary model picker");
        };

        assert_eq!(options.len(), 10_000);
        assert_eq!(*active, Some(*selected));
        assert_eq!(options[*selected].value, primary);
        assert_eq!(models[middle].0, huge_model_id);
        assert_eq!(
            data.providers.rows[0].provider.settings_config["models"][middle]["id"].as_str(),
            Some(models[middle].0.as_str())
        );
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
        let _env = TestEnvGuard::isolated(temp_home.path());
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
                ref input,
                submit: TextSubmit::OpenClawAgentsRuntimeField {
                    field: OpenClawAgentsRuntimeField::Timeout,
                },
                ..
            }) if title == texts::tui_openclaw_agents_timeout()
                && prompt == texts::tui_openclaw_agents_timeout()
                && input.value.is_empty()
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
            Overlay::TextInput(TextInputState { ref input, .. }) if input.value == "hjkl"
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
            Overlay::TextInput(TextInputState { ref input, .. }) if input.value == "/?[]q"
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
            Overlay::TextInput(TextInputState { ref input, .. }) if input.value == "x"
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
            input.input.set("   ".to_string());
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
            input.input.set("");
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
        let _env = TestEnvGuard::isolated(temp_home.path());
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
        let _env = TestEnvGuard::isolated(temp_home.path());
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
        let _env = TestEnvGuard::isolated(temp_home.path());
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
        let _env = TestEnvGuard::isolated(temp_home.path());
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
        let _env = TestEnvGuard::isolated(temp_home.path());
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
        let _env = TestEnvGuard::isolated(temp_home.path());
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
        let _env = TestEnvGuard::isolated(temp_home.path());
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
        let _env = TestEnvGuard::isolated(temp_home.path());
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
    fn openclaw_warning_helpers_bound_extreme_paths_and_warning_scans() {
        let mut data = UiData::default();
        data.config.openclaw_config_path =
            Some(std::path::PathBuf::from("p".repeat(3 * 1024 * 1024)));
        data.config.openclaw_warnings = Some(
            (0..OPENCLAW_WARNING_SCAN_ITEMS)
                .map(|index| crate::openclaw_config::OpenClawHealthWarning {
                    code: "config_parse_failed".to_string(),
                    message: format!("warning {index}"),
                    path: Some("unrelated.section".to_string()),
                })
                .collect(),
        );

        assert!(!openclaw_tools_load_failed(&data));
        assert!(!openclaw_agents_load_failed(&data));
        assert!(!openclaw_tools_has_blocking_warning(&data));
        assert!(!openclaw_agents_has_blocking_warning(&data));

        data.config
            .openclaw_warnings
            .as_mut()
            .expect("warnings")
            .push(crate::openclaw_config::OpenClawHealthWarning {
                code: "config_parse_failed".to_string(),
                message: "warning beyond inspection budget".to_string(),
                path: Some("unrelated.section".to_string()),
            });

        // The section is absent and the remaining warning is uninspected, so
        // saving must fail closed even though the visible prefix is unrelated.
        assert!(openclaw_tools_load_failed(&data));
        assert!(openclaw_agents_load_failed(&data));

        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig::default());
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults::default());

        assert!(!openclaw_tools_load_failed(&data));
        assert!(!openclaw_agents_load_failed(&data));
        assert!(!openclaw_tools_has_blocking_warning(&data));
        assert!(!openclaw_agents_has_blocking_warning(&data));

        data.config.openclaw_warnings.as_mut().expect("warnings")
            [OPENCLAW_WARNING_SCAN_ITEMS - 1]
            .path = None;
        assert!(openclaw_tools_has_blocking_warning(&data));
        assert!(openclaw_agents_has_blocking_warning(&data));
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
        let _env = TestEnvGuard::isolated(temp_home.path());
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
    fn settings_menu_exposes_managed_accounts_item() {
        assert!(
            matches!(
                SettingsItem::ALL.first(),
                Some(SettingsItem::ManagedAccounts)
            ),
            "Managed Accounts should be the first Settings entry"
        );
        assert!(
            SettingsItem::ALL
                .iter()
                .any(|item| matches!(item, SettingsItem::ManagedAccounts)),
            "Settings should expose global managed accounts"
        );
    }

    #[test]
    fn settings_menu_exposes_codex_unified_session_history_item() {
        assert!(
            SettingsItem::ALL
                .iter()
                .any(|item| matches!(item, SettingsItem::CodexUnifiedSessionHistory)),
            "Settings should expose unified Codex session history"
        );
    }

    #[test]
    fn settings_menu_places_preferred_editor_after_icons() {
        let editor_index = SettingsItem::ALL
            .iter()
            .position(|item| matches!(item, SettingsItem::PreferredEditor))
            .expect("PreferredEditor missing from SettingsItem::ALL");

        assert!(editor_index > 0);
        assert!(matches!(
            SettingsItem::ALL.get(editor_index - 1),
            Some(SettingsItem::Icons)
        ));
    }

    #[test]
    fn settings_preferred_editor_has_contextual_help() {
        let _lang = use_test_language(Language::English);
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Settings;
        app.focus = Focus::Content;
        app.settings_idx = SettingsItem::ALL
            .iter()
            .position(|item| matches!(item, SettingsItem::PreferredEditor))
            .expect("PreferredEditor missing from SettingsItem::ALL");

        let help = crate::cli::tui::help::context_help_for_app(&app, &UiData::default());
        let body = help.lines.join("\n");

        assert_eq!(help.title, "External Editor");
        assert!(body.contains("VISUAL"), "{body}");
        assert!(body.contains("EDITOR"), "{body}");
        assert!(body.contains("without a shell"), "{body}");
        assert!(body.contains("code --wait"), "{body}");
    }

    #[test]
    #[serial(home_settings)]
    fn settings_preferred_editor_opens_detected_picker_with_custom_selected() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());
        crate::settings::set_preferred_editor(Some(
            "cc-switch-unrecognized-editor --wait".to_string(),
        ))
        .expect("save custom editor");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Settings;
        app.focus = Focus::Content;
        app.settings_idx = SettingsItem::ALL
            .iter()
            .position(|item| matches!(item, SettingsItem::PreferredEditor))
            .expect("PreferredEditor missing from SettingsItem::ALL");

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());

        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::ExternalEditorPicker { selected, editors }
                if *selected == editors.len()
        ));
    }

    #[test]
    #[serial(home_settings)]
    fn opening_external_editor_picker_does_not_choose_an_editor_automatically() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());
        crate::settings::set_preferred_editor(None).expect("clear preferred editor");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Settings;
        app.focus = Focus::Content;
        app.settings_idx = SettingsItem::ALL
            .iter()
            .position(|item| matches!(item, SettingsItem::PreferredEditor))
            .expect("PreferredEditor missing from SettingsItem::ALL");

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &UiData::default()),
            Action::None
        ));
        assert!(matches!(
            app.overlay,
            Overlay::ExternalEditorPicker { selected: 0, .. }
        ));
        assert_eq!(crate::settings::get_preferred_editor(), None);
    }

    fn detected_editor(label: &str, command: &str) -> crate::cli::editor::DetectedEditor {
        crate::cli::editor::DetectedEditor {
            label: label.to_string(),
            command: command.to_string(),
        }
    }

    #[test]
    #[serial(home_settings)]
    fn external_editor_picker_applies_only_an_explicit_detected_choice() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());
        crate::settings::set_preferred_editor(Some("nvim".to_string()))
            .expect("save initial editor");

        let mut app = App::new(Some(AppType::Claude));
        app.overlay = Overlay::ExternalEditorPicker {
            selected: 0,
            editors: vec![detected_editor("Visual Studio Code", "code --wait")],
        };
        let action = app.on_key(key(KeyCode::Enter), &UiData::default());
        assert!(matches!(
            action,
            Action::SetPreferredEditor {
                command: Some(command)
            } if command == "code --wait"
        ));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    #[serial(home_settings)]
    fn external_editor_picker_custom_choice_reuses_text_input() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());
        crate::settings::set_preferred_editor(Some("my-editor --block".to_string()))
            .expect("save initial editor");

        let mut app = App::new(Some(AppType::Claude));
        app.overlay = Overlay::ExternalEditorPicker {
            selected: 1,
            editors: vec![detected_editor("Neovim", "nvim")],
        };

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::SettingsPreferredEditor,
                input,
                ..
            }) if input.value == "my-editor --block"
        ));
    }

    #[test]
    fn settings_preferred_editor_custom_submit_validates_and_clears() {
        let mut app = App::new(Some(AppType::Claude));
        app.overlay = Overlay::TextInput(TextInputState {
            title: "External Editor".to_string(),
            prompt: "command".to_string(),
            input: TextInput::new("code --wait"),
            submit: TextSubmit::SettingsPreferredEditor,
        });
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &UiData::default()),
            Action::SetPreferredEditor {
                command: Some(command)
            } if command == "code --wait"
        ));

        app.overlay = Overlay::TextInput(TextInputState {
            title: "External Editor".to_string(),
            prompt: "command".to_string(),
            input: TextInput::new("   "),
            submit: TextSubmit::SettingsPreferredEditor,
        });
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &UiData::default()),
            Action::SetPreferredEditor { command: None }
        ));

        app.overlay = Overlay::TextInput(TextInputState {
            title: "External Editor".to_string(),
            prompt: "command".to_string(),
            input: TextInput::new("\"unterminated"),
            submit: TextSubmit::SettingsPreferredEditor,
        });
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &UiData::default()),
            Action::None
        ));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::SettingsPreferredEditor,
                ..
            })
        ));
        assert!(matches!(
            app.toast,
            Some(Toast {
                kind: ToastKind::Error,
                ..
            })
        ));
    }

    #[test]
    fn settings_managed_accounts_item_opens_page_and_refreshes_when_status_missing() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Settings;
        app.focus = Focus::Content;
        app.settings_idx = SettingsItem::ALL
            .iter()
            .position(|item| matches!(item, SettingsItem::ManagedAccounts))
            .expect("ManagedAccounts missing from SettingsItem::ALL");

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());

        assert!(matches!(app.route, Route::SettingsManagedAccounts));
        assert!(matches!(
            action,
            Action::ManagedAuthRefresh { auth_provider } if auth_provider == "codex_oauth"
        ));
    }

    #[test]
    fn settings_managed_accounts_page_supports_multi_account_actions() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::SettingsManagedAccounts;
        app.focus = Focus::Content;

        app.settings_managed_accounts_idx = 0;
        let action = app.on_key(key(KeyCode::Enter), &UiData::default());
        assert!(matches!(
            action,
            Action::ManagedAuthRefresh { auth_provider } if auth_provider == "codex_oauth"
        ));

        app.managed_auth_status = Some(crate::services::ManagedAuthStatus {
            accounts: vec![],
            ..managed_auth_status()
        });
        let action = app.on_key(key(KeyCode::Enter), &UiData::default());
        assert!(matches!(
            action,
            Action::ManagedAuthStartLogin { auth_provider } if auth_provider == "codex_oauth"
        ));

        let action = app.on_key(key(KeyCode::Char('a')), &UiData::default());
        assert!(matches!(
            action,
            Action::ManagedAuthStartLogin { auth_provider } if auth_provider == "codex_oauth"
        ));

        app.managed_auth_status = Some(managed_auth_status());
        app.settings_managed_accounts_idx = 0;
        let action = app.on_key(key(KeyCode::Down), &UiData::default());
        assert!(matches!(action, Action::None));
        assert_eq!(app.settings_managed_accounts_idx, 1);

        let action = app.on_key(key(KeyCode::Char(' ')), &UiData::default());
        assert!(matches!(
            action,
            Action::ManagedAuthSetDefault {
                auth_provider,
                account_id,
            } if auth_provider == "codex_oauth" && account_id == "acc-alt"
        ));

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::ManagedAccountActionPicker {
                auth_provider,
                account_id,
                selected: 0,
            } if auth_provider == "codex_oauth" && account_id == "acc-alt"
        ));
    }

    #[test]
    fn managed_auth_login_uses_toast_and_esc_confirmation() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::SettingsManagedAccounts;
        app.focus = Focus::Content;

        handle_managed_auth_msg(
            &mut app,
            ManagedAuthMsg::LoginStarted {
                auth_provider: "codex_oauth".to_string(),
                result: Ok(managed_auth_device()),
            },
        );

        let toast = app.toast.as_ref().expect("login toast");
        assert!(toast.persistent);
        assert!(toast.message.contains("USER-1"), "{}", toast.message);
        assert!(
            toast.message.contains("Press Esc to cancel"),
            "{}",
            toast.message
        );
        assert!(!toast.message.contains("Esc/c"), "{}", toast.message);

        let action = app.on_key(key(KeyCode::Char('c')), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(app.managed_auth_login.is_some());
        assert!(matches!(app.overlay, Overlay::None));

        let action = app.on_key(key(KeyCode::Esc), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(app.managed_auth_login.is_some());
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::ManagedAuthCancelLogin,
                ..
            })
        ));

        let action = app.on_key(key(KeyCode::Esc), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(app.managed_auth_login.is_some());
        assert!(matches!(app.overlay, Overlay::None));

        let action = app.on_key(key(KeyCode::Esc), &UiData::default());
        assert!(matches!(action, Action::None));
        let action = app.on_key(key(KeyCode::Enter), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(app.managed_auth_login.is_none());
        assert!(matches!(app.overlay, Overlay::None));
        assert!(matches!(
            app.toast.as_ref(),
            Some(toast)
                if !toast.persistent
                    && toast.message == texts::tui_toast_managed_auth_login_cancelled()
        ));

        handle_managed_auth_msg(
            &mut app,
            ManagedAuthMsg::LoginPolled {
                auth_provider: "codex_oauth".to_string(),
                device_code: "device-1".to_string(),
                result: Ok(Some(crate::services::ManagedAuthAccount {
                    id: "acc-new".to_string(),
                    provider: "codex_oauth".to_string(),
                    login: "new@example.com".to_string(),
                    avatar_url: None,
                    authenticated_at: 3,
                    is_default: true,
                })),
            },
        );
        assert!(app.managed_auth_status.is_none());
    }

    #[test]
    #[serial(home_settings)]
    fn settings_openclaw_config_dir_item_opens_text_input() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());
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
                input,
                ..
            }) if input.value == r"\\wsl$\Ubuntu\home\demo\.openclaw"
        ));
    }

    #[test]
    fn settings_icons_row_toggles_and_persists_mode() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());
        // The CC_SWITCH_ICONS override would short-circuit the persisted
        // value, so clear it to exercise the Settings path deterministically.
        let saved_icons = std::env::var_os("CC_SWITCH_ICONS");
        std::env::remove_var("CC_SWITCH_ICONS");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Settings;
        app.focus = Focus::Content;
        app.settings_idx = SettingsItem::ALL
            .iter()
            .position(|item| matches!(item, SettingsItem::Icons))
            .expect("Icons missing from SettingsItem::ALL");

        // No persisted value is Auto; Enter cycles Auto -> Emoji -> Ascii -> Auto.
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &UiData::default()),
            Action::None
        ));
        assert_eq!(crate::settings::get_icon_mode().as_deref(), Some("emoji"));
        app.on_key(key(KeyCode::Enter), &UiData::default());
        assert_eq!(crate::settings::get_icon_mode().as_deref(), Some("ascii"));
        app.on_key(key(KeyCode::Enter), &UiData::default());
        assert_eq!(crate::settings::get_icon_mode().as_deref(), Some("auto"));

        crate::test_support::restore_env("CC_SWITCH_ICONS", &saved_icons);
    }

    #[test]
    fn settings_openclaw_config_dir_text_submit_emits_action() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Settings;
        app.focus = Focus::Content;

        app.overlay = Overlay::TextInput(TextInputState {
            title: "OpenClaw Config Directory".to_string(),
            prompt: "path".to_string(),
            input: TextInput::new(r"\\wsl$\Ubuntu\home\demo\.openclaw".to_string()),
            submit: TextSubmit::SettingsOpenClawConfigDir,
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
            input: TextInput::new("   ".to_string()),
            submit: TextSubmit::SettingsOpenClawConfigDir,
        });

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());
        assert!(matches!(
            action,
            Action::SetOpenClawConfigDir { path: None }
        ));
    }

    #[test]
    #[serial(home_settings)]
    fn settings_codex_unified_session_history_item_opens_confirm_overlay() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());

        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Settings;
        app.focus = Focus::Content;
        app.settings_idx = SettingsItem::ALL
            .iter()
            .position(|item| matches!(item, SettingsItem::CodexUnifiedSessionHistory))
            .expect("CodexUnifiedSessionHistory missing from SettingsItem::ALL");

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::SettingsSetCodexUnifiedSessionHistory { enabled: true },
                ..
            })
        ));
    }

    #[test]
    #[serial(home_settings)]
    fn settings_visible_apps_item_opens_picker_overlay() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());
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
        let _env = TestEnvGuard::isolated(temp_home.path());
        crate::settings::set_visible_apps(crate::settings::VisibleApps {
            claude: true,
            codex: false,
            gemini: false,
            opencode: false,
            hermes: false,
            openclaw: false,
        })
        .expect("save visible apps");
        crate::settings::set_visible_apps_mode(crate::settings::VisibleAppsMode::Manual)
            .expect("save visible apps mode");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Settings;
        app.focus = Focus::Content;
        app.overlay = Overlay::VisibleAppsPicker {
            selected: 0,
            apps: crate::settings::get_visible_apps(),
        };

        let data = UiData::default();
        let toggle_action = app.on_key(key(KeyCode::Char(' ')), &data);
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
    #[serial(home_settings)]
    fn visible_apps_picker_x_key_does_not_toggle_selection() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());
        crate::settings::set_visible_apps(crate::settings::VisibleApps {
            claude: true,
            codex: false,
            gemini: false,
            opencode: false,
            hermes: false,
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

        let action = app.on_key(key(KeyCode::Char('x')), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::VisibleAppsPicker { apps, .. }
                if apps.claude
                    && !apps.codex
                    && !apps.gemini
                    && !apps.opencode
                    && !apps.openclaw
        ));
    }

    #[test]
    #[serial(home_settings)]
    fn visible_apps_picker_prompts_manual_switch_for_controlled_app_in_auto_mode() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());
        let mut settings = crate::settings::get_settings();
        settings.visible_apps = crate::settings::VisibleApps {
            claude: true,
            codex: true,
            gemini: true,
            opencode: false,
            hermes: false,
            openclaw: false,
        };
        settings.visible_apps_settings.mode = crate::settings::VisibleAppsMode::Auto;
        settings.visible_apps_settings.auto_prompt_decided = true;
        crate::settings::update_settings(settings).expect("save settings");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Settings;
        app.focus = Focus::Content;
        app.overlay = Overlay::VisibleAppsPicker {
            selected: app_type_picker_index(&AppType::Gemini),
            apps: crate::settings::get_visible_apps(),
        };

        let action = app.on_key(key(KeyCode::Char(' ')), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::VisibleAppsSwitchToManual { apps, selected },
                ..
            }) if !apps.gemini && *selected == app_type_picker_index(&AppType::Gemini)
        ));
        assert!(app.toast.is_none());
    }

    #[test]
    #[serial(home_settings)]
    fn visible_apps_picker_prompts_manual_switch_for_claude_codex_in_auto_mode() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());
        let mut settings = crate::settings::get_settings();
        settings.visible_apps = crate::settings::VisibleApps {
            claude: true,
            codex: true,
            gemini: false,
            opencode: false,
            hermes: false,
            openclaw: false,
        };
        settings.visible_apps_settings.mode = crate::settings::VisibleAppsMode::Auto;
        settings.visible_apps_settings.auto_prompt_decided = true;
        crate::settings::update_settings(settings).expect("save settings");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Settings;
        app.focus = Focus::Content;
        app.overlay = Overlay::VisibleAppsPicker {
            selected: app_type_picker_index(&AppType::Codex),
            apps: crate::settings::get_visible_apps(),
        };

        let action = app.on_key(key(KeyCode::Char(' ')), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::VisibleAppsSwitchToManual { apps, selected },
                ..
            }) if !apps.codex && *selected == app_type_picker_index(&AppType::Codex)
        ));
        assert!(app.toast.is_none());
    }

    #[test]
    #[serial(home_settings)]
    fn visible_apps_manual_switch_prompt_cancel_returns_to_picker_without_change() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());
        let initial = crate::settings::VisibleApps {
            claude: true,
            codex: true,
            gemini: false,
            opencode: false,
            hermes: false,
            openclaw: false,
        };
        let mut settings = crate::settings::get_settings();
        settings.visible_apps = initial.clone();
        settings.visible_apps_settings.mode = crate::settings::VisibleAppsMode::Auto;
        settings.visible_apps_settings.auto_prompt_decided = true;
        crate::settings::update_settings(settings).expect("save settings");

        let mut next = initial.clone();
        next.codex = false;

        let mut app = App::new(Some(AppType::Claude));
        app.overlay = Overlay::Confirm(ConfirmOverlay {
            title: texts::tui_visible_apps_manual_switch_prompt_title().to_string(),
            message: texts::tui_visible_apps_manual_switch_prompt_message().to_string(),
            action: ConfirmAction::VisibleAppsSwitchToManual {
                apps: next,
                selected: app_type_picker_index(&AppType::Codex),
            },
        });

        let action = app.on_key(key(KeyCode::Esc), &UiData::default());

        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::VisibleAppsPicker { selected, apps }
                if *selected == app_type_picker_index(&AppType::Codex) && apps == &initial
        ));
        assert_eq!(crate::settings::get_visible_apps(), initial);
        assert_eq!(
            crate::settings::get_visible_apps_settings().mode,
            crate::settings::VisibleAppsMode::Auto
        );
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
        data.proxy.claude_takeover = true;
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
            }) if message == "The local proxy is running. Stop it before editing listen address."
        ));
    }

    #[test]
    fn settings_proxy_port_editable_when_proxy_running_but_app_not_routed() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::SettingsProxy;
        app.focus = Focus::Content;
        app.settings_proxy_idx = LocalProxySettingsItem::ALL
            .iter()
            .position(|item| matches!(item, LocalProxySettingsItem::ListenPort))
            .expect("ListenPort missing");

        let mut data = UiData::default();
        data.proxy.running = true;
        data.proxy.claude_takeover = false;
        data.proxy.configured_listen_port = 15721;

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::SettingsProxyListenPort,
                ..
            })
        ));

        app.overlay = Overlay::TextInput(TextInputState {
            title: "Listen Port".to_string(),
            prompt: "port".to_string(),
            input: TextInput::new("15721".to_string()),
            submit: TextSubmit::SettingsProxyListenPort,
        });
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            action,
            Action::SetProxyListenPort { port } if port == 15721
        ));
    }

    #[test]
    fn settings_proxy_port_blocked_when_active_worker_exists() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::SettingsProxy;
        app.focus = Focus::Content;
        app.settings_proxy_idx = LocalProxySettingsItem::ALL
            .iter()
            .position(|item| matches!(item, LocalProxySettingsItem::ListenPort))
            .expect("ListenPort missing");

        let mut data = UiData::default();
        data.proxy.running = true;
        data.proxy.active_worker_apps =
            std::collections::HashSet::from([AppType::Claude.as_str().to_string()]);
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
            }) if message == "This app is using the proxy. Stop this app's proxy route before editing listen port."
        ));
    }

    #[test]
    fn settings_proxy_port_submit_blocked_when_active_worker_starts_before_confirm() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::SettingsProxy;
        app.focus = Focus::Content;
        app.overlay = Overlay::TextInput(TextInputState {
            title: "Listen Port".to_string(),
            prompt: "port".to_string(),
            input: TextInput::new("15721".to_string()),
            submit: TextSubmit::SettingsProxyListenPort,
        });

        let mut data = UiData::default();
        data.proxy.running = true;
        data.proxy.active_worker_apps =
            std::collections::HashSet::from([AppType::Claude.as_str().to_string()]);

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        assert!(matches!(
            app.toast.as_ref(),
            Some(Toast {
                message,
                kind: ToastKind::Info,
                ..
            }) if message == "This app is using the proxy. Stop this app's proxy route before editing listen port."
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
            input: TextInput::new("127.0.0.1".to_string()),
            submit: TextSubmit::SettingsProxyListenAddress,
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
            input: TextInput::new("15721".to_string()),
            submit: TextSubmit::SettingsProxyListenPort,
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
            input: TextInput::new("bad host".to_string()),
            submit: TextSubmit::SettingsProxyListenAddress,
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
            input: TextInput::new("80".to_string()),
            submit: TextSubmit::SettingsProxyListenPort,
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
            input: TextInput::new("127.0.0.1".to_string()),
            submit: TextSubmit::SettingsProxyListenAddress,
        });

        let mut data = UiData::default();
        data.proxy.running = true;
        data.proxy.claude_takeover = true;

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        assert!(matches!(
            app.toast.as_ref(),
            Some(Toast {
                message,
                kind: ToastKind::Info,
                ..
            }) if message == "The local proxy is running. Stop it before editing listen address."
        ));
    }

    #[test]
    fn config_webdav_settings_opens_inline_form_in_second_level_menu() {
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
        assert!(matches!(app.form.as_ref(), Some(FormState::WebDavSync(_))));
    }

    #[test]
    fn config_webdav_submenu_items_emit_expected_actions() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::ConfigWebDav;
        app.focus = Focus::Content;
        let mut data = UiData::default();
        data.config.webdav_sync = Some(crate::settings::WebDavSyncSettings {
            enabled: true,
            base_url: "https://dav.example.com".to_string(),
            ..crate::settings::WebDavSyncSettings::default()
        });

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
            Action::None
        ));
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::CloudSyncReset {
                    backend: CloudSyncBackend::WebDav
                },
                ..
            })
        ));

        assert_eq!(
            WebDavConfigItem::ALL.len(),
            7,
            "WebDav submenu should include enable/disable and Jianguoyun quick setup"
        );
    }

    #[test]
    fn config_s3_submenu_uses_preflight_and_enables_without_switching_backends() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::ConfigS3;
        app.focus = Focus::Content;
        let mut data = UiData::default();
        data.config.s3_sync = Some(crate::settings::S3SyncSettings {
            enabled: true,
            region: "us-east-1".to_string(),
            bucket: "sync-bucket".to_string(),
            access_key_id: "AKID".to_string(),
            secret_access_key: "SECRET".to_string(),
            ..crate::settings::S3SyncSettings::default()
        });

        app.config_s3_idx = S3ConfigItem::ALL
            .iter()
            .position(|item| matches!(item, S3ConfigItem::Upload))
            .expect("S3 upload action");
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::ConfigS3FetchRemoteInfo {
                intent: CloudSyncTransferIntent::Upload
            }
        ));

        data.config.s3_sync.as_mut().expect("S3 config").enabled = false;
        data.config.webdav_sync = Some(crate::settings::WebDavSyncSettings {
            enabled: true,
            base_url: "https://dav.example.com".to_string(),
            ..crate::settings::WebDavSyncSettings::default()
        });
        app.config_s3_idx = S3ConfigItem::ALL
            .iter()
            .position(|item| matches!(item, S3ConfigItem::EnableDisable))
            .expect("S3 enable action");
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::ConfigS3SetEnabled { enabled: true }
        ));
    }

    #[test]
    fn disabled_webdav_allows_connection_check_but_not_transfer_selection() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::ConfigWebDav;
        app.focus = Focus::Content;
        let mut data = UiData::default();
        data.config.webdav_sync = Some(crate::settings::WebDavSyncSettings {
            enabled: false,
            base_url: "https://dav.example.com".to_string(),
            ..crate::settings::WebDavSyncSettings::default()
        });

        app.config_webdav_idx = WebDavConfigItem::ALL
            .iter()
            .position(|item| matches!(item, WebDavConfigItem::CheckConnection))
            .expect("WebDAV connection action");
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::ConfigWebDavCheckConnection
        ));

        app.config_webdav_idx = WebDavConfigItem::ALL
            .iter()
            .position(|item| matches!(item, WebDavConfigItem::Upload))
            .expect("WebDAV upload action");
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::None
        ));
        assert_eq!(
            app.config_webdav_idx,
            WebDavConfigItem::ALL
                .iter()
                .position(|item| matches!(item, WebDavConfigItem::Settings))
                .expect("WebDAV settings action")
        );
    }

    #[test]
    fn webdav_submenu_clamps_a_stale_selection_even_when_filter_is_empty() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::ConfigWebDav;
        app.focus = Focus::Content;
        app.filter.input.set("no-such-webdav-action");
        app.config_webdav_idx = usize::MAX;

        assert!(matches!(
            app.on_config_webdav_key(key(KeyCode::Up), &UiData::default()),
            Action::None
        ));
        assert_eq!(app.config_webdav_idx, 0);
    }

    #[test]
    fn s3_form_save_reports_required_fields_then_emits_plaintext_settings() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::ConfigS3;
        app.focus = Focus::Content;
        app.form = Some(FormState::S3Sync(
            crate::cli::tui::form::S3SyncFormState::from_settings(None),
        ));
        let data = UiData::default();

        assert!(matches!(
            app.on_key(ctrl(KeyCode::Char('s')), &data),
            Action::None
        ));
        let Some(FormState::S3Sync(form)) = app.form.as_mut() else {
            panic!("S3 form should remain open after validation failure");
        };
        assert_eq!(
            form.selected_field(),
            crate::cli::tui::form::S3SyncField::Bucket
        );
        assert!(form
            .field_error(crate::cli::tui::form::S3SyncField::Bucket)
            .is_some());
        form.region.set("us-east-1");
        form.bucket.set("sync-bucket");
        form.access_key_id.set("AKID");
        form.secret_access_key.set("plain-secret");

        assert!(matches!(
            app.on_key(ctrl(KeyCode::Char('s')), &data),
            Action::ConfigS3Save { settings }
                if settings.secret_access_key == "plain-secret" && !settings.auto_sync
        ));
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
            input.input.set("demo@nutstore.com".to_string());
        }
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::WebDavJianguoyunPassword,
                ..
            })
        ));

        if let Overlay::TextInput(ref mut input) = app.overlay {
            input.input.set("app-password".to_string());
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
            input.input.set("   ".to_string());
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
            input.input.set("demo@nutstore.com".to_string());
        }
        let _ = app.on_key(key(KeyCode::Enter), &data);
        if let Overlay::TextInput(ref mut input) = app.overlay {
            input.input.set("   ".to_string());
        }
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::WebDavJianguoyunPassword,
                ..
            })
        ));
    }

    #[test]
    fn prompts_e_opens_edit_form_and_ctrl_s_submits() {
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
                description: Some("Demo description".to_string()),
                enabled: false,
                created_at: None,
                updated_at: None,
            },
        });

        let action = app.on_key(key(KeyCode::Char('e')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.form,
            Some(FormState::PromptMeta(ref form))
                if matches!(form.mode, FormMode::Edit { ref id } if id == "pr1")
                    && form.id.value == "pr1"
                    && form.name.value == "Demo"
                    && form.description.value == "Demo description"
                    && form.content.text() == "hello"
        ));

        let submit = app.on_key(ctrl(KeyCode::Char('s')), &data);
        assert!(matches!(
            submit,
            Action::PromptSave {
                old_id,
                new_id,
                name,
                description,
                content
            } if old_id.as_deref() == Some("pr1")
                && new_id == "pr1"
                && name == "Demo"
                && description.as_deref() == Some("Demo description")
                && content == "hello"
        ));
    }

    #[test]
    fn prompts_a_opens_create_metadata_form() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;

        let action = app.on_key(key(KeyCode::Char('a')), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.form,
            Some(FormState::PromptMeta(ref form))
                if matches!(form.mode, FormMode::Add)
                    && form.name.value.starts_with("Prompt ")
                    && form.id.value.starts_with("prompt-")
        ));
    }

    #[test]
    fn prompts_create_metadata_submit_returns_save_action() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;

        app.form = Some(FormState::PromptMeta(PromptMetaFormState::new(
            "prompt-one".to_string(),
            "Prompt One".to_string(),
        )));
        if let Some(FormState::PromptMeta(form)) = app.form.as_mut() {
            form.description.set("Demo description");
            form.content.replace_text("Prompt body");
        }

        let action = app.on_key(ctrl(KeyCode::Char('s')), &UiData::default());
        assert!(app.editor.is_none());
        assert!(matches!(
            action,
            Action::PromptSave {
                old_id: None,
                new_id,
                name,
                description,
                content,
            } if new_id == "prompt-one"
                    && name == "Prompt One"
                    && description.as_deref() == Some("Demo description")
                    && content == "Prompt body"
        ));
    }

    #[test]
    fn prompts_create_metadata_empty_name_keeps_form_open() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;

        app.form = Some(FormState::PromptMeta(PromptMetaFormState::new(
            "prompt-one".to_string(),
            "   ".to_string(),
        )));

        let action = app.on_key(ctrl(KeyCode::Char('s')), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.form,
            Some(FormState::PromptMeta(ref form))
                if form.focus == FormFocus::Fields
                    && form.fields().get(form.field_idx) == Some(&PromptMetaField::Name)
                    && form.field_error(PromptMetaField::Name).is_some()
        ));
        assert!(app.editor.is_none());
    }

    #[test]
    fn prompts_n_no_longer_opens_metadata_form() {
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
                description: Some("Demo description".to_string()),
                enabled: false,
                created_at: None,
                updated_at: None,
            },
        });

        let action = app.on_key(key(KeyCode::Char('n')), &data);
        assert!(matches!(action, Action::None));
        assert!(app.form.is_none());
        assert!(app.editor.is_none());
    }

    #[test]
    fn prompts_metadata_tab_switches_to_content_and_edits_body() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;
        app.last_size = Size {
            width: 120,
            height: 40,
        };

        app.form = Some(FormState::PromptMeta(PromptMetaFormState::new(
            "prompt-one".to_string(),
            "Prompt One".to_string(),
        )));

        let action = app.on_key(key(KeyCode::Tab), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.form,
            Some(FormState::PromptMeta(ref form)) if form.focus == FormFocus::Content
        ));

        app.on_key(key(KeyCode::Char('A')), &UiData::default());
        app.on_key(key(KeyCode::Enter), &UiData::default());
        app.on_key(key(KeyCode::Char('B')), &UiData::default());

        assert!(matches!(
            app.form,
            Some(FormState::PromptMeta(ref form))
                if form.focus == FormFocus::Content
                    && form.content.text().starts_with("A\nB# Write your prompt here")
        ));
    }

    #[test]
    fn prompts_metadata_content_ctrl_o_requests_external_editor() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;
        app.form = Some(FormState::PromptMeta(PromptMetaFormState::new(
            "prompt-one".to_string(),
            "Prompt One".to_string(),
        )));

        app.on_key(key(KeyCode::Tab), &UiData::default());
        let action = app.on_key(ctrl(KeyCode::Char('o')), &UiData::default());

        assert!(matches!(action, Action::PromptFormOpenExternal));
        assert!(matches!(
            app.form,
            Some(FormState::PromptMeta(ref form)) if form.focus == FormFocus::Content
        ));
    }

    #[test]
    fn prompts_metadata_content_tab_inserts_spaces() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;
        app.form = Some(FormState::PromptMeta(PromptMetaFormState::new(
            "prompt-one".to_string(),
            "Prompt One".to_string(),
        )));

        app.on_key(key(KeyCode::Tab), &UiData::default());
        app.on_key(key(KeyCode::Tab), &UiData::default());

        assert!(matches!(
            app.form,
            Some(FormState::PromptMeta(ref form))
                if form.focus == FormFocus::Content
                    && form.content.text().starts_with("  # Write your prompt here")
        ));
    }

    #[test]
    fn prompts_metadata_content_shift_tab_returns_to_fields() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;
        app.form = Some(FormState::PromptMeta(PromptMetaFormState::new(
            "prompt-one".to_string(),
            "Prompt One".to_string(),
        )));

        app.on_key(key(KeyCode::Tab), &UiData::default());
        app.on_key(key(KeyCode::BackTab), &UiData::default());

        assert!(matches!(
            app.form,
            Some(FormState::PromptMeta(ref form)) if form.focus == FormFocus::Fields
        ));
    }

    #[test]
    fn prompt_form_external_editor_helper_updates_content_buffer() {
        let mut app = App::new(Some(AppType::Claude));
        app.form = Some(FormState::PromptMeta(PromptMetaFormState::new(
            "prompt-one".to_string(),
            "Prompt One".to_string(),
        )));
        if let Some(FormState::PromptMeta(form)) = app.form.as_mut() {
            form.focus = FormFocus::Content;
            form.content.replace_text("hello");
        }

        run_external_editor_for_prompt_form_content(&mut app, |current| {
            assert_eq!(current, "hello");
            Ok("hello from external\neditor".to_string())
        })
        .expect("external editor should update prompt form content");

        assert!(matches!(
            app.form,
            Some(FormState::PromptMeta(ref form))
                if form.content.text() == "hello from external\neditor"
                    && form.content.initial_text == "# Write your prompt here\n"
                    && form.has_unsaved_changes()
        ));
    }

    #[test]
    fn prompts_metadata_empty_name_keeps_form_open() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;

        let prompt = Prompt {
            id: "pr1".to_string(),
            name: "Demo".to_string(),
            content: "hello".to_string(),
            description: None,
            enabled: false,
            created_at: None,
            updated_at: None,
        };
        let mut form = PromptMetaFormState::from_prompt(&prompt);
        form.name.set("   ");
        app.form = Some(FormState::PromptMeta(form));

        let action = app.on_key(ctrl(KeyCode::Char('s')), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(app.form, Some(FormState::PromptMeta(_))));
        assert!(app.editor.is_none());
    }

    #[test]
    fn prompts_metadata_submit_returns_action() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;

        let prompt = Prompt {
            id: "pr1".to_string(),
            name: "Demo".to_string(),
            content: "hello".to_string(),
            description: None,
            enabled: false,
            created_at: None,
            updated_at: None,
        };
        let mut form = PromptMetaFormState::from_prompt(&prompt);
        form.id.set("renamed-id");
        form.name.set("Renamed");
        form.description.set("Updated description");
        form.content.replace_text("updated body");
        app.form = Some(FormState::PromptMeta(form));

        let action = app.on_key(ctrl(KeyCode::Char('s')), &UiData::default());
        assert!(matches!(
            action,
            Action::PromptSave {
                old_id,
                new_id,
                name,
                description,
                content,
            } if old_id.as_deref() == Some("pr1")
                && new_id == "renamed-id"
                && name == "Renamed"
                && description.as_deref() == Some("Updated description")
                && content == "updated body"
        ));
    }

    #[test]
    #[serial]
    fn prompt_save_runtime_updates_metadata_and_content() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = TestEnvGuard::isolated(temp.path());
        let state = crate::AppState::try_new().expect("load state");
        PromptService::upsert_prompt(
            &state,
            AppType::Claude,
            "pr1",
            Prompt {
                id: "pr1".to_string(),
                name: "Demo".to_string(),
                content: "hello".to_string(),
                description: None,
                enabled: false,
                created_at: Some(1),
                updated_at: Some(1),
            },
        )
        .expect("seed prompt");
        state.save().expect("persist config");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;
        app.filter.input.set("demo".to_string());
        app.prompt_idx = 0;

        let mut data = UiData::load(&app.app_type).expect("load ui data");
        run_runtime_action(
            &mut app,
            &mut data,
            Action::PromptSave {
                old_id: Some("pr1".to_string()),
                new_id: "renamed-id".to_string(),
                name: "Renamed".to_string(),
                description: Some("Updated description".to_string()),
                content: "updated body".to_string(),
            },
        )
        .expect("save prompt");

        assert!(!app.filter.active);
        assert!(app.filter.input.value.is_empty());
        assert_eq!(app.prompt_idx, 0);
        assert_eq!(data.prompts.rows.len(), 1);
        assert_eq!(data.prompts.rows[0].id, "renamed-id");
        assert_eq!(data.prompts.rows[0].prompt.name, "Renamed");
        assert_eq!(
            data.prompts.rows[0].prompt.description.as_deref(),
            Some("Updated description")
        );
        assert_eq!(data.prompts.rows[0].prompt.content, "updated body");
    }

    #[test]
    #[serial]
    fn prompt_save_runtime_creates_prompt_from_one_page_form() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = TestEnvGuard::isolated(temp.path());
        let state = crate::AppState::try_new().expect("load state");
        state.save().expect("persist empty state");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;
        app.filter.input.set("focus".to_string());
        app.prompt_idx = 0;

        let mut data = UiData::load(&app.app_type).expect("load ui data");
        run_runtime_action(
            &mut app,
            &mut data,
            Action::PromptSave {
                old_id: None,
                new_id: "prompt-one".to_string(),
                name: "Prompt One".to_string(),
                description: Some("Demo description".to_string()),
                content: "body".to_string(),
            },
        )
        .expect("create prompt");

        assert!(!app.filter.active);
        assert!(app.filter.input.value.is_empty());
        assert_eq!(app.prompt_idx, 0);
        assert_eq!(data.prompts.rows.len(), 1);
        assert_eq!(data.prompts.rows[0].id, "prompt-one");
        assert_eq!(
            data.prompts.rows[0].prompt.description.as_deref(),
            Some("Demo description")
        );
        assert_eq!(data.prompts.rows[0].prompt.content, "body");
    }

    #[test]
    #[serial]
    fn prompt_import_candidate_yes_opens_prefilled_add_form_without_saving() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = TestEnvGuard::isolated(temp.path());
        let state = crate::AppState::try_new().expect("load state");
        state.save().expect("persist empty state");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;
        app.filter.active = true;
        app.filter.input.set("old filter".to_string());
        let mut data = UiData::load(&app.app_type).expect("load ui data");
        data.prompts.import_candidate = Some(prompt_import_candidate(
            "CLAUDE.md",
            "# Existing prompt\n\nBody",
        ));

        run_runtime_action(
            &mut app,
            &mut data,
            Action::PromptOpenImportCandidate {
                filename: "CLAUDE.md".to_string(),
                content: "# Existing prompt\n\nBody".to_string(),
            },
        )
        .expect("open import candidate");

        assert!(!app.filter.active);
        assert!(app.filter.input.value.is_empty());
        assert!(matches!(app.overlay, Overlay::None));
        assert_eq!(
            PromptService::get_prompts(&state, AppType::Claude)
                .expect("load prompts")
                .len(),
            0,
            "opening the import candidate should not save until the user submits the add form"
        );
        assert!(matches!(
            app.form,
            Some(FormState::PromptMeta(ref form))
                if matches!(form.mode, FormMode::Add)
                    && form.id.value == "default-prompt"
                    && form.name.value == "Default Prompt"
                    && form.description.value == "Prefilled from existing CLAUDE.md"
                    && form.content.text() == "# Existing prompt\n\nBody"
        ));
    }

    #[test]
    #[serial]
    fn prompt_create_runtime_clears_filter_when_new_prompt_is_not_visible() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = TestEnvGuard::isolated(temp.path());
        let state = crate::AppState::try_new().expect("load state");
        state.save().expect("persist empty state");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;
        app.filter.input.set("focus".to_string());
        app.prompt_idx = 0;

        let mut data = UiData::load(&app.app_type).expect("load ui data");
        run_runtime_action(
            &mut app,
            &mut data,
            Action::EditorSubmit {
                submit: EditorSubmit::PromptCreate {
                    id: "prompt-one".to_string(),
                    name: "Prompt One".to_string(),
                    description: None,
                },
                content: "body".to_string(),
            },
        )
        .expect("create prompt");

        assert!(!app.filter.active);
        assert!(app.filter.input.value.is_empty());
        assert_eq!(app.prompt_idx, 0);
        assert_eq!(data.prompts.rows.len(), 1);
        assert_eq!(data.prompts.rows[0].id, "prompt-one");
    }

    #[test]
    #[serial]
    fn prompt_rename_runtime_clears_filter_when_renamed_prompt_is_not_visible() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = TestEnvGuard::isolated(temp.path());
        let state = crate::AppState::try_new().expect("load state");
        PromptService::upsert_prompt(
            &state,
            AppType::Claude,
            "pr1",
            Prompt {
                id: "pr1".to_string(),
                name: "Demo".to_string(),
                content: "hello".to_string(),
                description: None,
                enabled: false,
                created_at: Some(1),
                updated_at: Some(1),
            },
        )
        .expect("seed prompt");
        state.save().expect("persist config");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;
        app.filter.input.set("demo".to_string());
        app.prompt_idx = 0;

        let mut data = UiData::load(&app.app_type).expect("load ui data");
        run_runtime_action(
            &mut app,
            &mut data,
            Action::PromptUpdateMetadata {
                old_id: "pr1".to_string(),
                new_id: "pr1".to_string(),
                name: "Renamed".to_string(),
                description: None,
            },
        )
        .expect("rename prompt");

        assert!(!app.filter.active);
        assert!(app.filter.input.value.is_empty());
        assert_eq!(app.prompt_idx, 0);
        assert_eq!(data.prompts.rows.len(), 1);
        assert_eq!(data.prompts.rows[0].id, "pr1");
        assert_eq!(data.prompts.rows[0].prompt.name, "Renamed");
    }

    #[test]
    fn prompts_editor_ctrl_shift_s_submits() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;
        open_prompt_editor(&mut app);

        let submit = app.on_key(
            KeyEvent::new(KeyCode::Char('S'), KeyModifiers::CONTROL),
            &UiData::default(),
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
        open_prompt_editor(&mut app);

        let submit = app.on_key(key(KeyCode::Char('\u{13}')), &UiData::default());
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
        open_prompt_editor(&mut app);
        assert!(app.editor.is_some(), "prompt editor should be opened first");

        let action = app.on_key(ctrl(KeyCode::Char('o')), &UiData::default());
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
        open_prompt_editor(&mut app);

        app.on_key(key(KeyCode::Char('x')), &UiData::default());
        let action = app.on_key(key(KeyCode::Esc), &UiData::default());
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
        open_prompt_editor(&mut app);

        app.on_key(key(KeyCode::Char('x')), &UiData::default());
        app.on_key(key(KeyCode::Esc), &UiData::default());

        let action = app.on_key(key(KeyCode::Char('y')), &UiData::default());
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
        open_prompt_editor(&mut app);

        app.on_key(key(KeyCode::Char('x')), &UiData::default());
        app.on_key(key(KeyCode::Esc), &UiData::default());

        let action = app.on_key(key(KeyCode::Char('n')), &UiData::default());
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
    fn provider_key_app_save_focuses_id_before_name_when_both_are_missing() {
        let mut app = App::new(Some(AppType::Hermes));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Hermes,
        )));

        let action = app.on_key(ctrl(KeyCode::Char('s')), &UiData::default());

        assert!(matches!(action, Action::None));
        let Some(FormState::ProviderAdd(form)) = app.form.as_ref() else {
            panic!("expected provider form");
        };
        assert_eq!(form.fields()[form.field_idx], ProviderAddField::Id);
        assert!(form.main_field_error(ProviderAddField::Id).is_some());
        assert!(form.main_field_error(ProviderAddField::Name).is_none());
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
        assert!(matches!(
            app.form,
            Some(FormState::ProviderAdd(ref form))
                if form.focus == FormFocus::Fields
                    && form.fields().get(form.field_idx) == Some(&ProviderAddField::CodexBaseUrl)
                    && form.main_field_error(ProviderAddField::CodexBaseUrl).is_some()
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
            }
            .into(),
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
            }
            .into(),
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
    fn mcp_http_form_save_accepts_client_url_template_without_command() {
        let mut app = App::new(Some(AppType::OpenCode));
        app.route = Route::Mcp;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);

        if let Some(super::super::form::FormState::McpAdd(form)) = app.form.as_mut() {
            form.id.set("docs-langchain");
            form.name.set("LangChain Docs");
            form.server_type = McpTransport::Http;
            form.url.set("{env:MCP_BASE_URL}/mcp");
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
            } if content.contains("\"type\": \"http\"")
                && content.contains("\"url\": \"{env:MCP_BASE_URL}/mcp\"")
                && !content.contains("\"command\"")
        ));
    }

    #[test]
    fn mcp_http_form_save_rejects_empty_url() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Mcp;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);

        if let Some(super::super::form::FormState::McpAdd(form)) = app.form.as_mut() {
            form.id.set("docs-langchain");
            form.name.set("LangChain Docs");
            form.server_type = McpTransport::Http;
        } else {
            panic!("expected McpAdd form");
        }

        app.on_key(key(KeyCode::Esc), &data);
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.form,
            Some(FormState::McpAdd(ref form))
                if form.focus == FormFocus::Fields
                    && form.fields().get(form.field_idx) == Some(&McpAddField::Url)
                    && form.field_error(McpAddField::Url).is_some()
        ));
    }

    #[test]
    fn mcp_add_form_rejects_duplicate_id_and_focuses_the_field() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Mcp;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.mcp.rows.push(super::super::data::McpRow {
            id: "existing".to_string(),
            server: crate::app_config::McpServer {
                id: "existing".to_string(),
                name: "Existing".to_string(),
                server: json!({"command": "existing-command", "args": []}),
                apps: crate::app_config::McpApps::default(),
                description: None,
                homepage: None,
                docs: None,
                tags: vec![],
            }
            .into(),
        });
        app.on_key(key(KeyCode::Char('a')), &data);

        if let Some(FormState::McpAdd(form)) = app.form.as_mut() {
            form.id.set("existing");
            form.name.set("Duplicate");
            form.command.set("new-command");
        } else {
            panic!("expected McpAdd form");
        }

        app.on_key(key(KeyCode::Esc), &data);
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.form,
            Some(FormState::McpAdd(ref form))
                if form.focus == FormFocus::Fields
                    && form.fields().get(form.field_idx) == Some(&McpAddField::Id)
                    && form.field_error(McpAddField::Id)
                        == Some(texts::tui_toast_mcp_id_exists())
        ));
    }

    #[test]
    fn mcp_type_picker_updates_form_transport() {
        let mut app = App::new(Some(AppType::Claude));
        let mut form = McpAddFormState::new();
        form.focus = FormFocus::Fields;
        form.field_idx = form
            .fields()
            .iter()
            .position(|field| *field == McpAddField::Type)
            .expect("Type field should exist");
        app.form = Some(FormState::McpAdd(form));

        app.on_key(key(KeyCode::Enter), &UiData::default());
        assert!(matches!(
            app.overlay,
            Overlay::McpTypePicker { selected: 0 }
        ));

        app.on_key(key(KeyCode::Down), &UiData::default());
        app.on_key(key(KeyCode::Enter), &UiData::default());

        let Some(FormState::McpAdd(form)) = app.form.as_ref() else {
            panic!("expected MCP form");
        };
        assert_eq!(form.server_type, McpTransport::Http);
        assert!(form.fields().contains(&McpAddField::Url));
        assert!(!form.fields().contains(&McpAddField::Command));
    }

    #[test]
    fn mcp_template_clears_stale_inline_errors() {
        let mut app = App::new(Some(AppType::Claude));
        let mut form = McpAddFormState::new();
        form.id.set("server");
        form.focus = FormFocus::Fields;
        app.form = Some(FormState::McpAdd(form));

        let action = app.on_key(ctrl(KeyCode::Char('s')), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.form,
            Some(FormState::McpAdd(ref form))
                if form.field_error(McpAddField::Name).is_some()
        ));

        if let Some(FormState::McpAdd(form)) = app.form.as_mut() {
            form.focus = FormFocus::Templates;
            form.template_idx = 1;
        }
        app.on_key(key(KeyCode::Enter), &UiData::default());

        assert!(matches!(
            app.form,
            Some(FormState::McpAdd(ref form))
                if form.name.value == "Filesystem" && form.field_errors.is_empty()
        ));
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
            form.clear_text_edit();
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
                column: ClaudeModelPickerColumn::Model,
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
            form.clear_text_edit();
            form.field_idx = form
                .fields()
                .iter()
                .position(|field| *field == ProviderAddField::ClaudeModelConfig)
                .expect("ClaudeModelConfig field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        app.on_key(key(KeyCode::Enter), &data);
        app.on_key(key(KeyCode::Enter), &data); // enter editing mode in overlay
        app.on_key(key(KeyCode::Char('m')), &data);
        app.on_key(key(KeyCode::Char('1')), &data);

        // The picker opens on the first role row (index 0 = Haiku).
        let model = match app.form.as_ref() {
            Some(super::super::form::FormState::ProviderAdd(form)) => {
                form.claude_haiku_model.value.clone()
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
            form.clear_text_edit();
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
            form.clear_text_edit();
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
    fn provider_claude_api_format_overlay_jk_navigates_options() {
        let mut app = App::new(Some(AppType::Claude));
        app.overlay = Overlay::ClaudeApiFormatPicker { selected: 0 };

        let action = app.on_key(key(KeyCode::Char('j')), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::ClaudeApiFormatPicker { selected: 1 }
        ));

        let action = app.on_key(key(KeyCode::Char('k')), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::ClaudeApiFormatPicker { selected: 0 }
        ));
    }

    #[test]
    fn provider_codex_local_routing_field_enter_opens_page() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data);

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.clear_text_edit();
            form.field_idx = form
                .fields()
                .iter()
                .position(|field| *field == ProviderAddField::CodexLocalRouting)
                .expect("Codex local routing field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));

        let page = match app.form.as_ref() {
            Some(super::super::form::FormState::ProviderAdd(form)) => form.page,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(
            page,
            super::super::form::ProviderFormPage::CodexLocalRouting
        );
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn provider_codex_upstream_format_chat_warns_when_proxy_not_enabled() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data);

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.clear_text_edit();
            form.codex_base_url.set("https://api.example.com/v1");
            form.field_idx = form
                .fields()
                .iter()
                .position(|field| *field == ProviderAddField::ClaudeApiFormat)
                .expect("Codex upstream-format field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        app.on_key(key(KeyCode::Enter), &data); // open upstream-format picker
        app.on_key(key(KeyCode::Down), &data); // move to Chat Completions
        let action = app.on_key(key(KeyCode::Enter), &data); // select Chat

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
    fn provider_codex_upstream_format_chat_does_not_warn_when_proxy_routes_current_app() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.proxy.running = true;
        data.proxy.codex_takeover = true;

        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data);

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.clear_text_edit();
            form.codex_base_url.set("https://api.example.com/v1");
            form.field_idx = form
                .fields()
                .iter()
                .position(|field| *field == ProviderAddField::ClaudeApiFormat)
                .expect("Codex upstream-format field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        app.on_key(key(KeyCode::Enter), &data); // open upstream-format picker
        app.on_key(key(KeyCode::Down), &data); // move to Chat Completions
        let action = app.on_key(key(KeyCode::Enter), &data); // select Chat

        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        let format = match app.form.as_ref() {
            Some(super::super::form::FormState::ProviderAdd(form)) => form.claude_api_format,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(format, super::super::form::ClaudeApiFormat::OpenAiChat);
    }

    #[test]
    fn provider_codex_prompt_cache_routing_enter_cycles_and_space_is_noop() {
        let mut app = App::new(Some(AppType::Codex));
        let mut form = ProviderAddFormState::new(AppType::Codex);
        form.focus = FormFocus::Fields;
        form.claude_api_format = super::super::form::ClaudeApiFormat::OpenAiChat;
        app.form = Some(FormState::ProviderAdd(form));
        select_provider_field(&mut app, ProviderAddField::CodexPromptCacheRouting);

        app.on_key(key(KeyCode::Char(' ')), &data());
        let mode = |app: &App| match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form.codex_prompt_cache_routing,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(mode(&app), PromptCacheRoutingMode::Auto);

        app.on_key(key(KeyCode::Enter), &data());
        assert_eq!(mode(&app), PromptCacheRoutingMode::Enabled);
        app.on_key(key(KeyCode::Enter), &data());
        assert_eq!(mode(&app), PromptCacheRoutingMode::Disabled);
        app.on_key(key(KeyCode::Enter), &data());
        assert_eq!(mode(&app), PromptCacheRoutingMode::Auto);
    }

    #[test]
    fn provider_codex_local_routing_model_catalog_edits_inline_and_adds_models() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.proxy.running = true;
        data.proxy.codex_takeover = true;

        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data);

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.clear_text_edit();
            form.codex_base_url.set("https://api.example.com/v1");
            form.field_idx = form
                .fields()
                .iter()
                .position(|field| *field == ProviderAddField::CodexLocalRouting)
                .expect("Codex local routing field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        app.on_key(key(KeyCode::Enter), &data); // open model-mapping page
        app.on_key(key(KeyCode::Enter), &data); // toggle "需要本地路由映射" on
                                                // Default upstream format is Responses, so once routing is on the inline
                                                // model-catalog table sits below the toggle; Down focuses it.
        app.on_key(key(KeyCode::Down), &data); // focus the inline catalog table
        assert!(app.editor.is_none());

        app.on_key(key(KeyCode::Char('+')), &data); // add a model row inline
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::CodexModelCatalogField {
                    row: None,
                    field: CodexModelCatalogField::Model,
                },
                ..
            })
        ));
        for ch in "deepseek-chat".chars() {
            app.on_key(key(KeyCode::Char(ch)), &data);
        }
        app.on_key(key(KeyCode::Enter), &data);

        let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_ref() else {
            panic!("expected ProviderAdd form");
        };
        assert_eq!(form.codex_model_catalog.len(), 1);
        assert_eq!(form.codex_model_catalog[0].model, "deepseek-chat");

        app.on_key(key(KeyCode::Right), &data);
        app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::CodexModelCatalogField {
                    row: Some(0),
                    field: CodexModelCatalogField::DisplayName,
                },
                ..
            })
        ));
        for ch in "DeepSeek Chat".chars() {
            app.on_key(key(KeyCode::Char(ch)), &data);
        }
        app.on_key(key(KeyCode::Enter), &data);

        app.on_key(key(KeyCode::Right), &data);
        app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::CodexModelCatalogField {
                    row: Some(0),
                    field: CodexModelCatalogField::ContextWindow,
                },
                ..
            })
        ));
        for ch in "256k".chars() {
            app.on_key(key(KeyCode::Char(ch)), &data);
        }
        app.on_key(key(KeyCode::Enter), &data);

        let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_ref() else {
            panic!("expected ProviderAdd form");
        };
        assert_eq!(form.codex_model_catalog[0].display_name, "DeepSeek Chat");
        assert_eq!(form.codex_model_catalog[0].context_window, "256k");
        assert_eq!(
            form.codex_model_catalog_field,
            CodexModelCatalogField::ContextWindow
        );

        let action = app.on_key(key(KeyCode::Char('f')), &data);
        assert!(matches!(
            action,
            Action::ProviderModelFetch {
                field: ProviderAddField::CodexLocalRouting,
                ..
            }
        ));

        app.overlay = Overlay::ModelFetchPicker {
            request_id: 1,
            field: ProviderAddField::CodexLocalRouting,
            claude_idx: None,
            input: TextInput::new("kimi-k2"),
            query: "kimi-k2".to_string(),
            fetching: false,
            models: vec!["kimi-k2".to_string()],
            filtered_indices: None,
            filter_incomplete: false,
            error: None,
            selected_idx: 0,
            selection_active: false,
        };
        app.on_key(key(KeyCode::Enter), &data);

        let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_ref() else {
            panic!("expected ProviderAdd form");
        };
        assert_eq!(form.codex_model_catalog.len(), 2);
        assert_eq!(form.codex_model_catalog[1].model, "kimi-k2");
    }

    #[test]
    fn provider_claude_model_overlay_jk_navigates_when_not_editing() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.form = Some(FormState::ProviderAdd(
            super::super::form::ProviderAddFormState::new(AppType::Claude),
        ));
        app.overlay = Overlay::ClaudeModelPicker {
            selected: 0,
            column: ClaudeModelPickerColumn::Model,
            editing: false,
        };

        let action = app.on_key(key(KeyCode::Char('j')), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::ClaudeModelPicker {
                selected: 1,
                column: ClaudeModelPickerColumn::Model,
                editing: false
            }
        ));

        let action = app.on_key(key(KeyCode::Char('k')), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::ClaudeModelPicker {
                selected: 0,
                column: ClaudeModelPickerColumn::Model,
                editing: false
            }
        ));
    }

    #[test]
    fn provider_claude_model_overlay_jk_inserts_chars_when_editing() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.form = Some(FormState::ProviderAdd(
            super::super::form::ProviderAddFormState::new(AppType::Claude),
        ));
        app.overlay = Overlay::ClaudeModelPicker {
            selected: 0,
            column: ClaudeModelPickerColumn::Model,
            editing: true,
        };

        app.on_key(key(KeyCode::Char('j')), &data());
        app.on_key(key(KeyCode::Char('k')), &data());

        // Index 0 in the model-mapping picker is the Haiku role.
        let model = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form.claude_haiku_model.value.clone(),
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(model, "jk");
        assert!(matches!(
            app.overlay,
            Overlay::ClaudeModelPicker {
                selected: 0,
                column: ClaudeModelPickerColumn::Model,
                editing: true
            }
        ));
    }

    #[test]
    fn provider_claude_model_overlay_navigates_columns_and_toggles_one_m() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        let mut form = super::super::form::ProviderAddFormState::new(AppType::Claude);
        form.claude_sonnet_model.set("model-sonnet");
        app.form = Some(FormState::ProviderAdd(form));
        app.overlay = Overlay::ClaudeModelPicker {
            selected: 1,
            column: ClaudeModelPickerColumn::Model,
            editing: false,
        };

        app.on_key(key(KeyCode::Right), &data());
        assert!(matches!(
            app.overlay,
            Overlay::ClaudeModelPicker {
                selected: 1,
                column: ClaudeModelPickerColumn::OneM,
                editing: false,
            }
        ));

        let action = app.on_key(key(KeyCode::Char(' ')), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::ClaudeModelPicker {
                column: ClaudeModelPickerColumn::OneM,
                ..
            }
        ));

        app.on_key(key(KeyCode::Enter), &data());
        let form = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form,
            _ => panic!("expected ProviderAdd form"),
        };
        assert!(form.claude_model_one_m_enabled(1));

        app.on_key(key(KeyCode::Left), &data());
        app.on_key(key(KeyCode::Enter), &data());
        assert!(matches!(
            app.overlay,
            Overlay::ClaudeModelPicker {
                selected: 1,
                column: ClaudeModelPickerColumn::Model,
                editing: true,
            }
        ));
    }

    #[test]
    fn provider_claude_model_overlay_skips_disabled_one_m_cells() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.form = Some(FormState::ProviderAdd(
            super::super::form::ProviderAddFormState::new(AppType::Claude),
        ));
        app.overlay = Overlay::ClaudeModelPicker {
            selected: 0,
            column: ClaudeModelPickerColumn::Model,
            editing: false,
        };

        app.on_key(key(KeyCode::Right), &data());
        assert!(matches!(
            app.overlay,
            Overlay::ClaudeModelPicker {
                selected: 0,
                column: ClaudeModelPickerColumn::Model,
                ..
            }
        ));

        app.overlay = Overlay::ClaudeModelPicker {
            selected: 1,
            column: ClaudeModelPickerColumn::OneM,
            editing: false,
        };
        app.on_key(key(KeyCode::Up), &data());
        assert!(matches!(
            app.overlay,
            Overlay::ClaudeModelPicker {
                selected: 0,
                column: ClaudeModelPickerColumn::Model,
                editing: false,
            }
        ));
    }

    #[test]
    fn provider_claude_model_edit_preserves_and_normalizes_one_m() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        let mut form = super::super::form::ProviderAddFormState::new(AppType::Claude);
        form.set_claude_model_from_config(1, "old-model[1M]");
        app.form = Some(FormState::ProviderAdd(form));
        app.overlay = Overlay::ClaudeModelPicker {
            selected: 1,
            column: ClaudeModelPickerColumn::Model,
            editing: true,
        };

        app.on_key(key(KeyCode::Char('x')), &data());
        app.on_key(key(KeyCode::Enter), &data());
        let form = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form,
            _ => panic!("expected ProviderAdd form"),
        };
        assert_eq!(form.claude_sonnet_model.value, "old-modelx");
        assert!(form.claude_model_one_m_enabled(1));

        let Some(FormState::ProviderAdd(form)) = app.form.as_mut() else {
            panic!("expected ProviderAdd form");
        };
        assert!(form.toggle_claude_model_one_m(1));
        form.claude_sonnet_model.set("next-model [1m]  ");
        app.overlay = Overlay::ClaudeModelPicker {
            selected: 1,
            column: ClaudeModelPickerColumn::Model,
            editing: true,
        };
        app.on_key(key(KeyCode::Esc), &data());

        let form = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form,
            _ => panic!("expected ProviderAdd form"),
        };
        assert_eq!(form.claude_sonnet_model.value, "next-model");
        assert!(form.claude_model_one_m_enabled(1));
    }

    #[test]
    fn provider_codex_oauth_model_fetch_space_uses_managed_auth_even_for_default_account() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.form = Some(FormState::ProviderAdd(claude_codex_oauth_form()));
        app.overlay = Overlay::ClaudeModelPicker {
            selected: 0,
            column: ClaudeModelPickerColumn::Model,
            editing: false,
        };

        let action = app.on_key(key(KeyCode::Char(' ')), &data());

        assert!(matches!(
            action,
            Action::ProviderModelFetch {
                codex_oauth: true,
                codex_oauth_account_id: None,
                field: ProviderAddField::ClaudeModelConfig,
                claude_idx: Some(0),
                ..
            }
        ));
    }

    #[test]
    fn managed_account_binding_picker_sets_and_clears_provider_account() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.managed_auth_status = Some(managed_auth_status());
        app.form = Some(FormState::ProviderAdd(claude_codex_oauth_form()));

        app.overlay = Overlay::ManagedAccountPicker {
            auth_provider: "codex_oauth".to_string(),
            selected: 2,
            binding: true,
            selected_account_id: None,
        };
        let action = app.on_key(key(KeyCode::Enter), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        let account_id = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form.codex_oauth_account_id.as_deref(),
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(account_id, Some("acc-alt"));

        app.overlay = Overlay::ManagedAccountPicker {
            auth_provider: "codex_oauth".to_string(),
            selected: 0,
            binding: true,
            selected_account_id: Some("acc-alt".to_string()),
        };
        let action = app.on_key(key(KeyCode::Enter), &data());
        assert!(matches!(action, Action::None));
        let account_id = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form.codex_oauth_account_id.as_deref(),
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(account_id, None);
    }

    #[test]
    fn managed_account_action_picker_emits_default_and_remove_actions() {
        let mut app = App::new(Some(AppType::Claude));

        app.overlay = Overlay::ManagedAccountActionPicker {
            auth_provider: "codex_oauth".to_string(),
            account_id: "acc-alt".to_string(),
            selected: 0,
        };
        let action = app.on_key(key(KeyCode::Enter), &data());
        assert!(matches!(
            action,
            Action::ManagedAuthSetDefault {
                auth_provider,
                account_id,
            } if auth_provider == "codex_oauth" && account_id == "acc-alt"
        ));

        app.overlay = Overlay::ManagedAccountActionPicker {
            auth_provider: "codex_oauth".to_string(),
            account_id: "acc-alt".to_string(),
            selected: 1,
        };
        let action = app.on_key(key(KeyCode::Enter), &data());
        assert!(matches!(
            action,
            Action::ManagedAuthRemove {
                auth_provider,
                account_id,
            } if auth_provider == "codex_oauth" && account_id == "acc-alt"
        ));
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
            form.clear_text_edit();
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
            form.clear_text_edit();
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
            form.clear_text_edit();
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

    fn failover_provider_row(
        id: &str,
        name: &str,
        settings_config: serde_json::Value,
        in_failover_queue: bool,
        sort_index: Option<usize>,
    ) -> ProviderRow {
        let mut provider =
            Provider::with_id(id.to_string(), name.to_string(), settings_config, None);
        provider.in_failover_queue = in_failover_queue;
        provider.sort_index = sort_index;

        ProviderRow {
            id: id.to_string(),
            provider,
            api_url: Some("https://example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: None,
            default_model_id: None,
        }
    }

    #[test]
    fn providers_space_switches_provider_when_failover_disabled() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.proxy.auto_failover_enabled = false;
        data.providers.rows.push(failover_provider_row(
            "p1",
            "Provider One",
            json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}),
            false,
            None,
        ));

        let action = app.on_key(key(KeyCode::Char(' ')), &data);
        assert!(matches!(action, Action::ProviderSwitch { id } if id == "p1"));
    }

    #[test]
    fn providers_space_explains_queue_routing_when_failover_enabled() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.proxy.auto_failover_enabled = true;
        data.providers.rows.push(failover_provider_row(
            "p1",
            "Provider One",
            json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}),
            true,
            Some(0),
        ));

        let action = app.on_key(key(KeyCode::Char(' ')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.toast.as_ref(),
            Some(toast) if toast.kind == ToastKind::Info
                && toast.message.contains("queue priority")
        ));
    }

    #[test]
    fn providers_s_alias_cannot_bypass_failover_queue_routing() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.proxy.auto_failover_enabled = true;
        data.providers.rows.push(failover_provider_row(
            "p1",
            "Provider One",
            json!({"model_provider":{"base_url":"https://example.com"}}),
            true,
            Some(0),
        ));

        let action = app.on_key(key(KeyCode::Char('s')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(app.toast.as_ref(), Some(toast) if toast.kind == ToastKind::Info));
    }

    #[test]
    fn providers_move_keys_do_not_move_failover_queue() {
        let mut app = App::new(Some(AppType::Gemini));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(failover_provider_row(
            "p1",
            "Provider One",
            json!({"baseUrl":"https://example.com"}),
            true,
            Some(0),
        ));

        assert!(matches!(
            app.on_key(key(KeyCode::Char('<')), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('>')), &data),
            Action::None
        ));
    }

    #[test]
    fn failover_queue_manager_f_toggles_auto_failover() {
        let mut app = App::new(Some(AppType::Claude));
        app.overlay = Overlay::FailoverQueueManager {
            selected_provider_id: Some("p1".to_string()),
        };

        let mut data = UiData::default();
        data.proxy.auto_failover_enabled = true;
        data.providers.rows.push(failover_provider_row(
            "p1",
            "Provider One",
            json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}),
            true,
            Some(0),
        ));

        let action = app.on_key(key(KeyCode::Char('f')), &data);
        assert!(matches!(
            action,
            Action::SetProxyAutoFailover { app_type, enabled }
                if app_type == AppType::Claude && !enabled
        ));
    }

    #[test]
    fn failover_queue_manager_f_toggles_auto_failover_when_empty() {
        let mut app = App::new(Some(AppType::Gemini));
        app.overlay = Overlay::FailoverQueueManager {
            selected_provider_id: None,
        };

        let mut data = UiData::default();
        data.proxy.auto_failover_enabled = false;

        let action = app.on_key(key(KeyCode::Char('f')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(app.toast.as_ref(), Some(toast) if toast.kind == ToastKind::Warning));
    }

    #[test]
    fn settings_proxy_auto_failover_prompts_to_enable_proxy_when_not_routed() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::SettingsProxy;
        app.focus = Focus::Content;
        app.settings_proxy_idx = LocalProxySettingsItem::ALL
            .iter()
            .position(|item| *item == LocalProxySettingsItem::AutoFailover)
            .expect("auto failover item should exist");

        let mut data = UiData::default();
        data.proxy.running = true;
        data.proxy.claude_takeover = false;
        data.providers.rows.push(failover_provider_row(
            "p1",
            "Provider One",
            json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}),
            true,
            Some(0),
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::ProxyEnableAndAutoFailover {
                    app_type: AppType::Claude
                },
                ..
            })
        ));
    }

    #[test]
    fn settings_proxy_auto_failover_toggles_when_proxy_is_routed_and_queue_exists() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::SettingsProxy;
        app.focus = Focus::Content;
        app.settings_proxy_idx = LocalProxySettingsItem::ALL
            .iter()
            .position(|item| *item == LocalProxySettingsItem::AutoFailover)
            .expect("auto failover item should exist");

        let mut data = UiData::default();
        data.proxy.running = true;
        data.proxy.claude_takeover = true;
        data.proxy.auto_failover_enabled = false;
        data.providers.rows.push(failover_provider_row(
            "p1",
            "Provider One",
            json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}),
            true,
            Some(0),
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            action,
            Action::SetProxyAutoFailover { app_type, enabled }
                if app_type == AppType::Claude && enabled
        ));
    }

    #[test]
    fn providers_f_key_opens_failover_queue_manager_for_supported_apps() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(failover_provider_row(
            "p1",
            "Provider One",
            json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}),
            false,
            None,
        ));

        let action = app.on_key(key(KeyCode::Char('f')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::FailoverQueueManager {
                selected_provider_id: Some(ref id)
            } if id == "p1"
        ));
    }

    #[test]
    fn failover_queue_manager_space_does_not_toggle_selected_provider() {
        let mut app = App::new(Some(AppType::Claude));
        app.overlay = Overlay::FailoverQueueManager {
            selected_provider_id: Some("p1".to_string()),
        };

        let mut data = UiData::default();
        data.providers.rows.push(failover_provider_row(
            "p1",
            "Provider One",
            json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}),
            false,
            None,
        ));

        let action = app.on_key(key(KeyCode::Char(' ')), &data);
        assert!(matches!(action, Action::None));
    }

    #[test]
    fn failover_queue_manager_enter_removes_selected_queued_provider() {
        let mut app = App::new(Some(AppType::Codex));
        app.overlay = Overlay::FailoverQueueManager {
            selected_provider_id: Some("p1".to_string()),
        };

        let mut data = UiData::default();
        data.providers.rows.push(failover_provider_row(
            "p1",
            "Provider One",
            json!({"model_provider":{"base_url":"https://example.com"}}),
            true,
            Some(1),
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            action,
            Action::ProviderSetFailoverQueue { id, enabled } if id == "p1" && !enabled
        ));
    }

    #[test]
    fn failover_queue_manager_ctrl_arrows_only_move_queued_provider() {
        let mut app = App::new(Some(AppType::Codex));
        app.overlay = Overlay::FailoverQueueManager {
            selected_provider_id: Some("p1".to_string()),
        };

        let mut data = UiData::default();
        data.providers.rows.push(failover_provider_row(
            "p1",
            "Provider One",
            json!({"model_provider":{"base_url":"https://example.com"}}),
            true,
            Some(1),
        ));
        data.providers.rows.push(failover_provider_row(
            "p2",
            "Provider Two",
            json!({"model_provider":{"base_url":"https://example.com"}}),
            false,
            None,
        ));

        let action = app.on_key(ctrl(KeyCode::Down), &data);
        assert!(matches!(
            action,
            Action::ProviderMoveFailoverQueue {
                id,
                direction: MoveDirection::Down,
            } if id == "p1"
        ));

        app.overlay = Overlay::FailoverQueueManager {
            selected_provider_id: Some("p2".to_string()),
        };
        assert!(matches!(
            app.on_key(ctrl(KeyCode::Down), &data),
            Action::None
        ));
    }

    #[test]
    fn failover_queue_manager_selection_follows_provider_after_membership_reorders_rows() {
        let mut app = App::new(Some(AppType::Claude));
        app.overlay = Overlay::FailoverQueueManager {
            selected_provider_id: Some("p2".to_string()),
        };

        let mut data = UiData::default();
        data.providers.rows.push(failover_provider_row(
            "p1",
            "Provider One",
            json!({"env":{"ANTHROPIC_BASE_URL":"https://one.example.com"}}),
            true,
            Some(0),
        ));
        data.providers.rows.push(failover_provider_row(
            "p2",
            "Provider Two",
            json!({"env":{"ANTHROPIC_BASE_URL":"https://two.example.com"}}),
            true,
            Some(1),
        ));
        data.providers.rows.push(failover_provider_row(
            "p3",
            "Provider Three",
            json!({"env":{"ANTHROPIC_BASE_URL":"https://three.example.com"}}),
            false,
            None,
        ));

        let remove = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            remove,
            Action::ProviderSetFailoverQueue { id, enabled: false } if id == "p2"
        ));

        let p2 = data
            .providers
            .rows
            .iter_mut()
            .find(|row| row.id == "p2")
            .expect("p2 fixture");
        p2.provider.in_failover_queue = false;
        p2.provider.sort_index = None;

        let add_again = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            add_again,
            Action::ProviderSetFailoverQueue { id, enabled: true } if id == "p2"
        ));
        assert!(matches!(
            app.overlay,
            Overlay::FailoverQueueManager {
                selected_provider_id: Some(ref id)
            } if id == "p2"
        ));
    }

    #[test]
    fn failover_queue_manager_selection_follows_provider_after_priority_reorder() {
        let mut app = App::new(Some(AppType::Codex));
        app.overlay = Overlay::FailoverQueueManager {
            selected_provider_id: Some("p2".to_string()),
        };

        let mut data = UiData::default();
        data.providers.rows.push(failover_provider_row(
            "p1",
            "Provider One",
            json!({"model_provider":{"base_url":"https://one.example.com"}}),
            true,
            Some(0),
        ));
        data.providers.rows.push(failover_provider_row(
            "p2",
            "Provider Two",
            json!({"model_provider":{"base_url":"https://two.example.com"}}),
            true,
            Some(1),
        ));

        let move_up = app.on_key(ctrl(KeyCode::Up), &data);
        assert!(matches!(
            move_up,
            Action::ProviderMoveFailoverQueue {
                id,
                direction: MoveDirection::Up,
            } if id == "p2"
        ));

        data.providers.rows[0].provider.sort_index = Some(1);
        data.providers.rows[1].provider.sort_index = Some(0);
        let move_down = app.on_key(ctrl(KeyCode::Down), &data);
        assert!(matches!(
            move_down,
            Action::ProviderMoveFailoverQueue {
                id,
                direction: MoveDirection::Down,
            } if id == "p2"
        ));
    }

    #[test]
    fn failover_queue_manager_uses_shifted_jk_not_delete_key_for_moves() {
        let mut app = App::new(Some(AppType::Codex));
        app.overlay = Overlay::FailoverQueueManager {
            selected_provider_id: Some("p1".to_string()),
        };

        let mut data = UiData::default();
        data.providers.rows.push(failover_provider_row(
            "p1",
            "Provider One",
            json!({"model_provider":{"base_url":"https://example.com"}}),
            true,
            Some(1),
        ));

        // `d` means delete on every list screen; it must not silently
        // reorder the failover queue (it used to move the entry down).
        assert!(matches!(
            app.on_key(key(KeyCode::Char('d')), &data),
            Action::None
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('u')), &data),
            Action::None
        ));

        assert!(matches!(
            app.on_key(key(KeyCode::Char('J')), &data),
            Action::ProviderMoveFailoverQueue {
                direction: MoveDirection::Down,
                ..
            }
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('K')), &data),
            Action::ProviderMoveFailoverQueue {
                direction: MoveDirection::Up,
                ..
            }
        ));
    }

    #[test]
    fn failover_queue_manager_esc_closes_overlay() {
        let mut app = App::new(Some(AppType::Claude));
        app.overlay = Overlay::FailoverQueueManager {
            selected_provider_id: None,
        };

        let action = app.on_key(key(KeyCode::Esc), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn unsupported_apps_ignore_failover_provider_keys() {
        let mut app = App::new(Some(AppType::OpenCode));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(failover_provider_row(
            "p1",
            "Provider One",
            json!({"baseUrl":"https://example.com"}),
            false,
            None,
        ));

        assert!(matches!(
            app.on_key(key(KeyCode::Char('f')), &data),
            Action::None
        ));
        assert!(matches!(app.overlay, Overlay::None));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('<')), &data),
            Action::None
        ));
    }

    #[test]
    fn settings_proxy_auto_failover_blocks_empty_queue() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::SettingsProxy;
        app.focus = Focus::Content;
        app.settings_proxy_idx = LocalProxySettingsItem::ALL
            .iter()
            .position(|item| *item == LocalProxySettingsItem::AutoFailover)
            .expect("auto failover item should exist");

        let mut data = UiData::default();
        data.proxy.running = true;
        data.proxy.auto_failover_enabled = false;

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(app.toast.as_ref(), Some(toast) if toast.kind == ToastKind::Warning));
    }

    #[test]
    fn unsupported_apps_ignore_settings_proxy_auto_failover() {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = Route::SettingsProxy;
        app.focus = Focus::Content;
        app.settings_proxy_idx = LocalProxySettingsItem::ALL
            .iter()
            .position(|item| *item == LocalProxySettingsItem::AutoFailover)
            .expect("auto failover item should exist");

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());
        assert!(matches!(action, Action::None));
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

    #[test]
    fn provider_form_jk_navigates_fields() {
        let mut app = open_provider_fields_form(AppType::Claude);

        assert_eq!(provider_field_idx(&app), 0);
        app.on_key(key(KeyCode::Char('j')), &data());
        assert_eq!(provider_field_idx(&app), 1);
        app.on_key(key(KeyCode::Char('k')), &data());
        assert_eq!(provider_field_idx(&app), 0);
    }

    #[test]
    fn provider_form_jk_inserts_chars_when_editing() {
        let mut app = open_provider_fields_form(AppType::Claude);

        app.on_key(key(KeyCode::Enter), &data());
        let (editing, _) = provider_editing_name(&app);
        assert!(editing);

        app.on_key(key(KeyCode::Char('j')), &data());
        app.on_key(key(KeyCode::Char('k')), &data());
        let (editing, value) = provider_editing_name(&app);
        assert!(editing);
        assert!(
            value.contains('j') && value.contains('k'),
            "j/k should be typed as characters in editing mode, got: {value}"
        );
    }

    #[test]
    fn provider_inline_edit_esc_restores_value_cursor_error_and_side_effects() {
        let mut app = open_provider_fields_form(AppType::Claude);
        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.name.set("Original");
            form.name.cursor = 3;
            form.id.set("stable-id");
            form.set_main_field_error(ProviderAddField::Name, "required");
        }

        app.on_key(key(KeyCode::Enter), &data());
        app.on_key(key(KeyCode::Char('X')), &data());
        app.on_key(key(KeyCode::Esc), &data());

        let Some(FormState::ProviderAdd(form)) = app.form.as_ref() else {
            panic!("expected ProviderAdd form");
        };
        assert_eq!(form.name.value, "Original");
        assert_eq!(form.name.cursor, 3);
        assert_eq!(form.id.value, "stable-id");
        assert_eq!(
            form.main_field_error(ProviderAddField::Name),
            Some("required")
        );
        assert!(!form.is_editing_main_text());
    }

    #[test]
    fn provider_inline_edit_tab_and_backtab_do_not_apply_or_move() {
        let mut app = open_provider_fields_form(AppType::Claude);
        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.name.set("A");
            form.id.set("original-id");
        }

        app.on_key(key(KeyCode::Enter), &data());
        app.on_key(key(KeyCode::Char('x')), &data());
        app.on_key(key(KeyCode::Tab), &data());
        app.on_key(key(KeyCode::BackTab), &data());

        let Some(FormState::ProviderAdd(form)) = app.form.as_ref() else {
            panic!("expected ProviderAdd form");
        };
        assert_eq!(form.name.value, "Ax");
        assert_eq!(
            form.text_edit_target(),
            Some(form::ProviderTextField::Main(ProviderAddField::Name))
        );
        assert_eq!(form.id.value, "original-id");
    }

    #[test]
    fn provider_model_enter_edits_and_f_fetches() {
        let mut app = open_provider_fields_form(AppType::Gemini);
        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.gemini_auth_type = GeminiAuthType::ApiKey;
            form.gemini_api_key.set("gemini-secret");
            form.gemini_base_url.set("https://gemini.example.test");
        }
        select_provider_field(&mut app, ProviderAddField::GeminiModel);

        let action = app.on_key(key(KeyCode::Enter), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.form,
            Some(FormState::ProviderAdd(ref form))
                if form.text_edit_target()
                    == Some(form::ProviderTextField::Main(ProviderAddField::GeminiModel))
        ));

        app.on_key(key(KeyCode::Esc), &data());
        let action = app.on_key(key(KeyCode::Char('f')), &data());
        assert!(matches!(
            action,
            Action::ProviderModelFetch {
                field: ProviderAddField::GeminiModel,
                base_url,
                api_key: Some(api_key),
                ..
            } if base_url == "https://gemini.example.test" && api_key == "gemini-secret"
        ));
    }

    #[test]
    fn provider_action_rows_ignore_space() {
        let mut app = open_provider_fields_form(AppType::Claude);
        select_provider_field(&mut app, ProviderAddField::ClaudeQuickConfig);

        app.on_key(key(KeyCode::Char(' ')), &data());

        assert!(matches!(
            app.form,
            Some(FormState::ProviderAdd(ref form))
                if matches!(form.page, form::ProviderFormPage::Main)
        ));
    }

    #[test]
    fn provider_form_down_same_as_j() {
        let mut app = open_provider_fields_form(AppType::Claude);

        app.on_key(key(KeyCode::Down), &data());
        let after_down = provider_field_idx(&app);

        app.on_key(key(KeyCode::Char('k')), &data());
        app.on_key(key(KeyCode::Char('j')), &data());
        assert_eq!(after_down, provider_field_idx(&app));
    }

    #[test]
    fn provider_form_up_same_as_k() {
        let mut app = open_provider_fields_form(AppType::Claude);

        app.on_key(key(KeyCode::Char('j')), &data());
        app.on_key(key(KeyCode::Up), &data());
        let after_up = provider_field_idx(&app);

        app.on_key(key(KeyCode::Char('j')), &data());
        app.on_key(key(KeyCode::Char('k')), &data());
        assert_eq!(after_up, provider_field_idx(&app));
    }

    #[test]
    fn mcp_form_jk_navigates_fields() {
        let mut app = open_mcp_fields_form();

        assert_eq!(mcp_field_idx(&app), 0);
        app.on_key(key(KeyCode::Char('j')), &data());
        assert_eq!(mcp_field_idx(&app), 1);
        app.on_key(key(KeyCode::Char('k')), &data());
        assert_eq!(mcp_field_idx(&app), 0);
    }

    #[test]
    fn mcp_form_jk_inserts_chars_when_editing() {
        let mut app = open_mcp_fields_form();

        app.on_key(key(KeyCode::Enter), &data());
        let (editing, _) = mcp_editing_id(&app);
        assert!(editing);

        app.on_key(key(KeyCode::Char('j')), &data());
        app.on_key(key(KeyCode::Char('k')), &data());
        let (editing, value) = mcp_editing_id(&app);
        assert!(editing);
        assert!(
            value.contains('j') && value.contains('k'),
            "j/k should be typed as characters in editing mode, got: {value}"
        );
    }

    #[test]
    fn mcp_inline_edit_esc_restores_value_cursor_and_error() {
        let mut app = open_mcp_fields_form();
        if let Some(FormState::McpAdd(form)) = app.form.as_mut() {
            form.id.set("server-id");
            form.id.cursor = 2;
            form.set_field_error(McpAddField::Id, "required");
        }

        app.on_key(key(KeyCode::Enter), &data());
        app.on_key(key(KeyCode::Char('X')), &data());
        app.on_key(key(KeyCode::Esc), &data());

        let Some(FormState::McpAdd(form)) = app.form.as_ref() else {
            panic!("expected MCP form");
        };
        assert_eq!(form.id.value, "server-id");
        assert_eq!(form.id.cursor, 2);
        assert_eq!(form.field_error(McpAddField::Id), Some("required"));
        assert!(form.text_edit.is_none());
    }

    #[test]
    fn mcp_invalid_args_tab_is_a_noop_until_enter_applies() {
        let mut app = open_mcp_fields_form();
        if let Some(FormState::McpAdd(form)) = app.form.as_mut() {
            form.command.set("node");
            let fields = form.fields();
            form.field_idx = fields
                .iter()
                .position(|field| *field == McpAddField::Args)
                .expect("Args field should exist");
            assert!(form.begin_text_edit(McpAddField::Args));
            form.args.set("'unterminated");
        }

        app.on_key(key(KeyCode::Tab), &data());

        let Some(FormState::McpAdd(form)) = app.form.as_ref() else {
            panic!("expected MCP form");
        };
        assert_eq!(form.text_edit_target(), Some(McpAddField::Args));
        assert_eq!(form.field_error(McpAddField::Args), None);
        assert_eq!(
            form.to_mcp_server_json_value()["server"]["args"],
            serde_json::json!([])
        );
    }

    #[test]
    fn mcp_form_down_same_as_j() {
        let mut app = open_mcp_fields_form();

        app.on_key(key(KeyCode::Down), &data());
        let after_down = mcp_field_idx(&app);

        app.on_key(key(KeyCode::Char('k')), &data());
        app.on_key(key(KeyCode::Char('j')), &data());
        assert_eq!(after_down, mcp_field_idx(&app));
    }

    #[test]
    fn mcp_form_up_same_as_k() {
        let mut app = open_mcp_fields_form();

        app.on_key(key(KeyCode::Char('j')), &data());
        app.on_key(key(KeyCode::Up), &data());
        let after_up = mcp_field_idx(&app);

        app.on_key(key(KeyCode::Char('j')), &data());
        app.on_key(key(KeyCode::Char('k')), &data());
        assert_eq!(after_up, mcp_field_idx(&app));
    }

    #[test]
    fn prompt_form_jk_navigates_fields() {
        let mut app = open_prompt_fields_form();

        assert_eq!(prompt_field_idx(&app), 0);
        app.on_key(key(KeyCode::Char('j')), &data());
        assert_eq!(prompt_field_idx(&app), 1);
        app.on_key(key(KeyCode::Char('k')), &data());
        assert_eq!(prompt_field_idx(&app), 0);
    }

    #[test]
    fn prompt_form_jk_inserts_chars_when_editing() {
        let mut app = open_prompt_fields_form();

        app.on_key(key(KeyCode::Enter), &data());
        let (editing, _) = prompt_editing_id(&app);
        assert!(editing);

        app.on_key(key(KeyCode::Char('j')), &data());
        app.on_key(key(KeyCode::Char('k')), &data());
        let (editing, value) = prompt_editing_id(&app);
        assert!(editing);
        assert!(
            value.contains('j') && value.contains('k'),
            "j/k should be typed as characters in editing mode, got: {value}"
        );
    }

    #[test]
    fn prompt_inline_edit_esc_restores_value_cursor_and_error() {
        let mut app = open_prompt_fields_form();
        if let Some(FormState::PromptMeta(form)) = app.form.as_mut() {
            form.id.set("prompt-id");
            form.id.cursor = 4;
            form.set_field_error(PromptMetaField::Id, "invalid id");
        }

        app.on_key(key(KeyCode::Enter), &data());
        app.on_key(key(KeyCode::Char('X')), &data());
        app.on_key(key(KeyCode::Esc), &data());

        let Some(FormState::PromptMeta(form)) = app.form.as_ref() else {
            panic!("expected PromptMeta form");
        };
        assert_eq!(form.id.value, "prompt-id");
        assert_eq!(form.id.cursor, 4);
        assert_eq!(form.field_error(PromptMetaField::Id), Some("invalid id"));
        assert!(form.text_edit.is_none());
    }

    #[test]
    fn prompt_form_down_same_as_j() {
        let mut app = open_prompt_fields_form();

        app.on_key(key(KeyCode::Down), &data());
        let after_down = prompt_field_idx(&app);

        app.on_key(key(KeyCode::Char('k')), &data());
        app.on_key(key(KeyCode::Char('j')), &data());
        assert_eq!(after_down, prompt_field_idx(&app));
    }

    #[test]
    fn prompt_form_up_same_as_k() {
        let mut app = open_prompt_fields_form();

        app.on_key(key(KeyCode::Char('j')), &data());
        app.on_key(key(KeyCode::Up), &data());
        let after_up = prompt_field_idx(&app);

        app.on_key(key(KeyCode::Char('j')), &data());
        app.on_key(key(KeyCode::Char('k')), &data());
        assert_eq!(after_up, prompt_field_idx(&app));
    }

    #[test]
    fn provider_json_preview_jk_scrolls() {
        let mut app = open_provider_fields_form(AppType::Claude);
        focus_provider_json_preview(&mut app);

        app.on_key(key(KeyCode::Char('j')), &data());
        assert_eq!(provider_json_scroll(&app), 1);
        app.on_key(key(KeyCode::Char('k')), &data());
        assert_eq!(provider_json_scroll(&app), 0);
    }

    #[test]
    fn provider_codex_json_preview_jk_scrolls() {
        let mut app = open_provider_fields_form(AppType::Codex);
        focus_provider_json_preview(&mut app);

        app.on_key(key(KeyCode::Char('j')), &data());
        assert_eq!(provider_codex_auth_scroll(&app), 1);
        app.on_key(key(KeyCode::Char('k')), &data());
        assert_eq!(provider_codex_auth_scroll(&app), 0);
    }

    #[test]
    fn mcp_json_preview_jk_scrolls() {
        let mut app = open_mcp_fields_form();
        focus_mcp_json_preview(&mut app);

        app.on_key(key(KeyCode::Char('j')), &data());
        assert_eq!(mcp_json_scroll(&app), 1);
        app.on_key(key(KeyCode::Char('k')), &data());
        assert_eq!(mcp_json_scroll(&app), 0);
    }

    #[test]
    fn context_help_global_keeps_shortcuts_and_english_eyebrow() {
        let _lang = use_test_language(Language::English);
        let mut app = App::new(Some(AppType::Claude));

        let action = app.on_key(key(KeyCode::Char('?')), &UiData::default());

        assert!(matches!(action, Action::None));
        let text = help_text(&app);
        assert!(text.contains("Help follows the focused item"), "{text}");
        assert!(text.contains("↑↓ or h/j/k/l  move"), "{text}");
        assert!(text.contains("?   toggle help"), "{text}");
        assert!(text.contains("Providers: Space switch"), "{text}");
        assert!(!text.contains("Provider Detail:"), "{text}");
        assert!(!text.contains("点在哪"), "{text}");
        assert!(!text.contains("显示/关闭帮助"), "{text}");
    }

    #[test]
    fn fullwidth_question_mark_toggles_the_help_overlay() {
        let _lang = use_test_language(Language::English);
        let mut app = App::new(Some(AppType::Claude));

        // The full-width Chinese '？' opens help, same as ASCII '?'.
        let action = app.on_key(key(KeyCode::Char('？')), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::Help(_)));
        assert!(help_text(&app).contains("?   toggle help"));

        // Pressing it again toggles the overlay closed.
        let action = app.on_key(key(KeyCode::Char('？')), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(!app.overlay.is_active(), "help overlay should close");
    }

    #[test]
    fn context_help_provider_field_tracks_focused_codex_base_url() {
        let _lang = use_test_language(Language::English);
        let mut app = App::new(Some(AppType::Codex));
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Codex,
        )));
        select_provider_field(&mut app, ProviderAddField::CodexBaseUrl);

        let action = app.on_key(key(KeyCode::Char('?')), &UiData::default());

        assert!(matches!(action, Action::None));
        let text = help_text(&app);
        assert!(text.contains("Help follows the focused item"), "{text}");
        assert!(
            text.contains("Codex natively supports OpenAI Responses API"),
            "{text}"
        );
        assert!(text.contains("keep local routing enabled"), "{text}");
        assert!(!text.contains("Codex 原生"), "{text}");
    }

    #[test]
    fn context_help_claude_fallback_model_shows_upstream_hint() {
        let _lang = use_test_language(Language::English);
        let mut app = App::new(Some(AppType::Claude));
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Claude,
        )));
        select_provider_field(&mut app, ProviderAddField::ClaudeFallbackModel);

        app.on_key(key(KeyCode::Char('?')), &UiData::default());
        let text = help_text(&app);
        assert!(text.contains("Default fallback model"), "{text}");
        assert!(
            text.contains("don't clearly map to a specific role model"),
            "{text}"
        );
        assert!(!text.contains("默认兜底"), "{text}");
    }

    #[test]
    fn context_help_claude_one_m_explains_declaration_and_restores_cell() {
        let _lang = use_test_language(Language::English);
        let mut app = App::new(Some(AppType::Claude));
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Claude,
        )));
        app.overlay = Overlay::ClaudeModelPicker {
            selected: 2,
            column: ClaudeModelPickerColumn::OneM,
            editing: false,
        };

        app.on_key(key(KeyCode::Char('?')), &UiData::default());
        let text = help_text(&app);
        assert!(text.contains("only declares"), "{text}");
        assert!(
            text.contains("does not detect upstream capability"),
            "{text}"
        );
        assert!(text.contains("secondary a shortcut"), "{text}");
        assert!(text.contains("Enter to edit"), "{text}");
        assert!(text.contains("Space to fetch"), "{text}");
        assert!(text.contains("Enter to toggle"), "{text}");
        assert!(matches!(
            app.pending_overlay,
            Some(Overlay::ClaudeModelPicker {
                selected: 2,
                column: ClaudeModelPickerColumn::OneM,
                editing: false,
            })
        ));

        app.on_key(key(KeyCode::Char('?')), &UiData::default());
        assert!(matches!(
            app.overlay,
            Overlay::ClaudeModelPicker {
                selected: 2,
                column: ClaudeModelPickerColumn::OneM,
                editing: false,
            }
        ));
    }

    #[test]
    fn context_help_codex_upstream_format_shows_format_hint() {
        let _lang = use_test_language(Language::English);
        let mut app = App::new(Some(AppType::Codex));
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Codex,
        )));
        select_provider_field(&mut app, ProviderAddField::ClaudeApiFormat);

        app.on_key(key(KeyCode::Char('?')), &UiData::default());
        let text = help_text(&app);
        assert!(text.contains("Upstream format"), "{text}");
        assert!(text.contains("natively Responses API"), "{text}");
        assert!(!text.contains("上游格式"), "{text}");
    }

    #[test]
    fn context_help_codex_prompt_cache_routing_explains_three_modes() {
        let _lang = use_test_language(Language::English);
        let mut app = App::new(Some(AppType::Codex));
        let mut form = ProviderAddFormState::new(AppType::Codex);
        form.claude_api_format = super::super::form::ClaudeApiFormat::OpenAiChat;
        app.form = Some(FormState::ProviderAdd(form));
        select_provider_field(&mut app, ProviderAddField::CodexPromptCacheRouting);

        app.on_key(key(KeyCode::Char('?')), &UiData::default());
        let text = help_text(&app);
        assert!(text.contains("Prompt cache routing"), "{text}");
        assert!(text.contains("known-compatible upstreams"), "{text}");
        assert!(text.contains("strict gateway"), "{text}");
        assert!(text.contains("stable client-provided session ID"), "{text}");
        assert!(!text.contains("提示词缓存路由"), "{text}");
    }

    #[test]
    fn context_help_codex_local_routing_fields_follow_focus() {
        let _lang = use_test_language(Language::English);
        let mut app = App::new(Some(AppType::Codex));
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Codex,
        )));
        select_provider_field(&mut app, ProviderAddField::CodexLocalRouting);

        app.on_key(key(KeyCode::Char('?')), &UiData::default());
        let main_field = help_text(&app);
        assert!(main_field.contains("Model mapping"), "{main_field}");
        assert!(main_field.contains("Chat Completions"), "{main_field}");
        assert!(main_field.contains("model-catalogs.json"), "{main_field}");
        assert!(!main_field.contains("模型映射"), "{main_field}");

        app.on_key(key(KeyCode::Char('?')), &UiData::default());
        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.open_codex_local_routing_page();
            // Reasoning capability only shows when routing is on AND format is Chat.
            form.claude_api_format = super::super::form::ClaudeApiFormat::OpenAiChat;
            form.toggle_codex_local_routing_enabled();
            form.codex_local_routing_field_idx = form
                .codex_local_routing_fields()
                .iter()
                .position(|field| *field == CodexLocalRoutingField::SupportsEffort)
                .expect("SupportsEffort field should be visible when routing is on and Chat");
        }

        app.on_key(key(KeyCode::Char('?')), &UiData::default());
        let local_routing_field = help_text(&app);
        assert!(
            local_routing_field.contains("reasoning effort"),
            "{local_routing_field}"
        );
        assert!(
            local_routing_field.contains("effort levels"),
            "{local_routing_field}"
        );

        app.on_key(key(KeyCode::Char('?')), &UiData::default());
        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.open_codex_model_catalog_page();
            form.codex_model_catalog_field = CodexModelCatalogField::ContextWindow;
        }

        app.on_key(key(KeyCode::Char('?')), &UiData::default());
        let model_catalog_field = help_text(&app);
        assert!(
            model_catalog_field.contains("Context Window"),
            "{model_catalog_field}"
        );
        assert!(
            model_catalog_field.contains("provider's actual model capability"),
            "{model_catalog_field}"
        );
    }

    #[test]
    fn context_help_provider_preview_uses_chinese_without_english_leakage() {
        let _lang = use_test_language(Language::Chinese);
        let mut app = App::new(Some(AppType::Codex));
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Codex,
        )));
        focus_provider_json_preview(&mut app);
        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.codex_preview_section = CodexPreviewSection::Config;
        }

        let action = app.on_key(key(KeyCode::Char('?')), &UiData::default());

        assert!(matches!(action, Action::None));
        let text = help_text(&app);
        assert!(text.contains("点在哪，帮助就在哪"), "{text}");
        assert!(
            text.contains("本地路由、思考能力、模型目录、目标模式、远程压缩"),
            "{text}"
        );
        assert!(!text.contains("Help follows"), "{text}");
        assert!(!text.contains("Goal mode"), "{text}");
        assert!(!text.contains("remote compaction"), "{text}");
    }

    #[test]
    fn context_help_usage_query_template_explains_exposed_templates() {
        let _lang = use_test_language(Language::English);
        let mut app = App::new(Some(AppType::Claude));
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Claude,
        )));
        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.open_usage_query_page();
            form.toggle_usage_query_enabled();
        }
        select_usage_query_field(&mut app, UsageQueryField::Template);

        let action = app.on_key(key(KeyCode::Char('?')), &UiData::default());

        assert!(matches!(action, Action::None));
        let text = help_text(&app);
        assert!(
            text.contains("custom, general, newapi, and balance"),
            "{text}"
        );
        assert!(
            text.contains("Token Plan is not shown in the TUI"),
            "{text}"
        );
        assert!(!text.contains("TUI 只提供"), "{text}");
    }

    #[test]
    fn context_help_usage_query_side_panels_follow_focus() {
        let _lang = use_test_language(Language::English);
        let mut app = App::new(Some(AppType::Claude));
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Claude,
        )));
        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.open_usage_query_page();
            form.toggle_usage_query_enabled();
            form.set_usage_query_template(UsageQueryTemplate::Custom);
            form.focus = FormFocus::JsonPreview;
        }

        let action = app.on_key(key(KeyCode::Char('?')), &UiData::default());

        assert!(matches!(action, Action::None));
        let extractor = help_text(&app);
        assert!(extractor.contains("{{baseUrl}}"), "{extractor}");
        assert!(extractor.contains("remaining quota fields"), "{extractor}");
        assert!(extractor.contains("planName"), "{extractor}");

        app.on_key(key(KeyCode::Char('?')), &UiData::default());
        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = FormFocus::Content;
        }

        let action = app.on_key(key(KeyCode::Char('?')), &UiData::default());

        assert!(matches!(action, Action::None));
        let instructions = help_text(&app);
        assert!(
            instructions.contains("{{baseUrl}}/api/usage"),
            "{instructions}"
        );
        assert!(instructions.contains("remaining"), "{instructions}");
        assert!(instructions.contains("User-Agent"), "{instructions}");
    }

    #[test]
    fn context_help_long_usage_query_instructions_scroll() {
        let _lang = use_test_language(Language::English);
        let mut app = App::new(Some(AppType::Claude));
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Claude,
        )));
        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.open_usage_query_page();
            form.toggle_usage_query_enabled();
            form.focus = FormFocus::Content;
        }

        app.on_key(key(KeyCode::Char('?')), &UiData::default());
        let Overlay::Help(help) = &app.overlay else {
            panic!("expected Help overlay, got {:?}", app.overlay);
        };
        assert!(help.content.lines.len() > 20);
        assert_eq!(help.scroll, 0);

        app.on_key(key(KeyCode::Down), &UiData::default());
        app.on_key(key(KeyCode::Down), &UiData::default());
        let Overlay::Help(help) = &app.overlay else {
            panic!("expected Help overlay, got {:?}", app.overlay);
        };
        assert_eq!(help.scroll, 2);

        app.on_key(key(KeyCode::Up), &UiData::default());
        let Overlay::Help(help) = &app.overlay else {
            panic!("expected Help overlay, got {:?}", app.overlay);
        };
        assert_eq!(help.scroll, 1);
    }

    #[test]
    fn context_help_usage_query_template_picker_restores_after_close() {
        let _lang = use_test_language(Language::English);
        let mut app = App::new(Some(AppType::Claude));
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Claude,
        )));
        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.open_usage_query_page();
            form.toggle_usage_query_enabled();
        }
        app.overlay = Overlay::UsageQueryTemplatePicker { selected: 2 };

        app.on_key(key(KeyCode::Char('?')), &UiData::default());
        let text = help_text(&app);
        assert!(
            text.contains("custom, general, newapi, and balance"),
            "{text}"
        );
        assert!(matches!(
            app.pending_overlay,
            Some(Overlay::UsageQueryTemplatePicker { selected: 2 })
        ));

        app.on_key(key(KeyCode::Char('?')), &UiData::default());
        assert!(matches!(
            app.overlay,
            Overlay::UsageQueryTemplatePicker { selected: 2 }
        ));
        assert!(app.pending_overlay.is_none());
    }

    #[test]
    fn usage_query_template_picker_space_is_noop() {
        let mut app = App::new(Some(AppType::Claude));
        let mut form = ProviderAddFormState::new(AppType::Claude);
        form.open_usage_query_page();
        form.toggle_usage_query_enabled();
        app.form = Some(FormState::ProviderAdd(form));
        app.overlay = Overlay::UsageQueryTemplatePicker { selected: 2 };

        let action = app.on_key(key(KeyCode::Char(' ')), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::UsageQueryTemplatePicker { selected: 2 }
        ));
    }

    #[test]
    fn usage_query_template_picker_enter_applies_selection() {
        let mut app = App::new(Some(AppType::Claude));
        let mut form = ProviderAddFormState::new(AppType::Claude);
        form.open_usage_query_page();
        form.toggle_usage_query_enabled();
        app.form = Some(FormState::ProviderAdd(form));
        app.overlay = Overlay::UsageQueryTemplatePicker { selected: 2 };

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        let Some(FormState::ProviderAdd(form)) = app.form.as_ref() else {
            panic!("expected provider form");
        };
        assert_eq!(form.usage_query_template, UsageQueryTemplate::NewApi);
    }

    #[test]
    fn context_help_does_not_cover_async_running_or_loading_overlays() {
        let _lang = use_test_language(Language::English);

        let mut speedtest = App::new(Some(AppType::Claude));
        speedtest.overlay = Overlay::SpeedtestRunning {
            url: "https://example.com".to_string(),
        };
        let action = speedtest.on_key(key(KeyCode::Char('?')), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            speedtest.overlay,
            Overlay::SpeedtestRunning { ref url } if url == "https://example.com"
        ));
        assert!(speedtest.pending_overlay.is_none());

        let mut stream_check = App::new(Some(AppType::Claude));
        stream_check.overlay = Overlay::StreamCheckRunning {
            provider_id: "p1".to_string(),
            provider_name: "Provider One".to_string(),
        };
        let action = stream_check.on_key(key(KeyCode::Char('?')), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            stream_check.overlay,
            Overlay::StreamCheckRunning { ref provider_id, .. } if provider_id == "p1"
        ));
        assert!(stream_check.pending_overlay.is_none());

        let mut proxy_loading = App::new(Some(AppType::Claude));
        proxy_loading.overlay = Overlay::Loading {
            kind: LoadingKind::Proxy,
            title: "Proxy".to_string(),
            message: "Working...".to_string(),
        };
        let action = proxy_loading.on_key(key(KeyCode::Char('?')), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            proxy_loading.overlay,
            Overlay::Loading {
                kind: LoadingKind::Proxy,
                ..
            }
        ));
        assert!(proxy_loading.pending_overlay.is_none());

        let mut webdav_loading = App::new(Some(AppType::Claude));
        webdav_loading.overlay = Overlay::Loading {
            kind: LoadingKind::WebDav,
            title: "WebDAV".to_string(),
            message: "Working...".to_string(),
        };
        let action = webdav_loading.on_key(key(KeyCode::Char('?')), &UiData::default());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            webdav_loading.overlay,
            Overlay::Loading {
                kind: LoadingKind::WebDav,
                ..
            }
        ));
        assert!(webdav_loading.pending_overlay.is_none());
    }

    #[test]
    fn context_help_usage_query_newapi_base_url_and_timeout_follow_upstream() {
        let _lang = use_test_language(Language::English);
        let mut app = App::new(Some(AppType::Claude));
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Claude,
        )));
        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.open_usage_query_page();
            form.toggle_usage_query_enabled();
            form.set_usage_query_template(UsageQueryTemplate::NewApi);
        }
        select_usage_query_field(&mut app, UsageQueryField::BaseUrl);

        app.on_key(key(KeyCode::Char('?')), &UiData::default());
        let base_url = help_text(&app);
        assert!(base_url.contains("NewAPI service URL"), "{base_url}");
        assert!(base_url.contains("/api/user/self"), "{base_url}");
        assert!(!base_url.contains("For the general template"), "{base_url}");

        app.on_key(key(KeyCode::Char('?')), &UiData::default());
        select_usage_query_field(&mut app, UsageQueryField::Timeout);
        app.on_key(key(KeyCode::Char('?')), &UiData::default());
        let timeout = help_text(&app);
        assert!(timeout.contains("2-30 seconds"), "{timeout}");
    }

    #[test]
    fn context_help_falls_back_when_focus_has_no_specific_help() {
        let _lang = use_test_language(Language::English);
        let mut app = App::new(Some(AppType::Claude));
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Claude,
        )));
        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = FormFocus::Content;
        }

        let action = app.on_key(key(KeyCode::Char('?')), &UiData::default());

        assert!(matches!(action, Action::None));
        let text = help_text(&app);
        assert!(text.contains("No help here"), "{text}");
        assert!(!text.contains("toggle help"), "{text}");
    }

    #[test]
    fn context_help_question_mark_stays_text_while_provider_field_is_editing() {
        let _lang = use_test_language(Language::English);
        let mut app = App::new(Some(AppType::Claude));
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Claude,
        )));
        select_provider_field(&mut app, ProviderAddField::Name);
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &UiData::default()),
            Action::None
        ));

        let action = app.on_key(key(KeyCode::Char('?')), &UiData::default());

        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        assert_eq!(provider_editing_name(&app), (true, "?".to_string()));
    }

    #[test]
    fn context_help_question_mark_stays_text_while_text_input_overlay_is_editing() {
        let _lang = use_test_language(Language::English);
        let mut app = App::new(Some(AppType::Claude));
        app.overlay = Overlay::TextInput(TextInputState {
            title: "Export".to_string(),
            prompt: "Path".to_string(),
            input: TextInput::new(""),
            submit: TextSubmit::ConfigExport,
        });

        let action = app.on_key(key(KeyCode::Char('?')), &UiData::default());

        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::TextInput(TextInputState { input, .. }) if input.value == "?"
        ));
        assert!(app.pending_overlay.is_none());
    }

    #[test]
    fn context_help_question_mark_stays_text_while_model_fetch_picker_is_editing() {
        let _lang = use_test_language(Language::English);
        let mut app = App::new(Some(AppType::Claude));
        app.overlay = Overlay::ModelFetchPicker {
            request_id: 1,
            field: ProviderAddField::ClaudeModelConfig,
            claude_idx: Some(0),
            input: TextInput::new(""),
            query: String::new(),
            fetching: false,
            models: Vec::new(),
            filtered_indices: None,
            filter_incomplete: false,
            error: None,
            selected_idx: 0,
            selection_active: false,
        };

        let action = app.on_key(key(KeyCode::Char('?')), &UiData::default());

        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::ModelFetchPicker { input, .. } if input.value == "?"
        ));
        assert!(app.pending_overlay.is_none());
    }

    #[test]
    fn context_help_common_config_mentions_re_extract_guidance() {
        let _lang = use_test_language(Language::English);
        let mut app = App::new(Some(AppType::Codex));
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Codex,
        )));
        select_provider_field(&mut app, ProviderAddField::CommonSnippet);

        app.on_key(key(KeyCode::Char('?')), &UiData::default());

        let text = help_text(&app);
        assert!(text.contains("re-extracting"), "{text}");
        assert!(text.contains("existing config"), "{text}");
    }

    #[test]
    fn context_help_settings_proxy_opens_proxy_detail_text_view() {
        let _lang = use_test_language(Language::English);
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::SettingsProxy;
        app.focus = Focus::Content;
        let mut data = UiData::default();
        data.proxy.configured_listen_address = "0.0.0.0".to_string();
        data.proxy.configured_listen_port = 15721;

        let action = app.on_key(key(KeyCode::Char('?')), &data);

        assert!(matches!(action, Action::None));
        let Overlay::TextView(view) = &app.overlay else {
            panic!("expected proxy TextView overlay, got {:?}", app.overlay);
        };
        assert_eq!(view.title, texts::tui_config_item_proxy());
        let text = view.lines.join("\n");
        assert!(text.contains("0.0.0.0:15721"), "{text}");
        assert!(text.contains("Restart routing"), "{text}");
    }

    #[test]
    fn context_help_chinese_empty_fallback_is_fully_chinese() {
        let _lang = use_test_language(Language::Chinese);
        let mut app = App::new(Some(AppType::Claude));
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Claude,
        )));
        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = FormFocus::Content;
        }

        let action = app.on_key(key(KeyCode::Char('?')), &UiData::default());

        assert!(matches!(action, Action::None));
        let text = help_text(&app);
        assert!(text.contains("点在哪，帮助就在哪"), "{text}");
        assert!(text.contains("此处无提示"), "{text}");
        assert!(!text.contains("Help follows"), "{text}");
        assert!(!text.contains("No help here"), "{text}");
    }

    #[test]
    fn provider_form_usage_query_row_opens_secondary_page_and_esc_returns() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.usage_query_notice_confirmed = true;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data);
        select_provider_usage_query_row(&mut app);

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.form,
            Some(FormState::ProviderAdd(ref form))
                if matches!(form.page, super::super::form::ProviderFormPage::UsageQuery)
        ));

        let action = app.on_key(key(KeyCode::Esc), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.form,
            Some(FormState::ProviderAdd(ref form))
                if matches!(form.page, super::super::form::ProviderFormPage::Main)
        ));
    }

    #[test]
    fn provider_form_usage_query_row_shows_first_entry_notice() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.usage_query_notice_confirmed = false;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data);
        select_provider_usage_query_row(&mut app);

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::UsageQueryNotice,
                ..
            })
        ));
        assert!(matches!(
            app.form,
            Some(FormState::ProviderAdd(ref form))
                if matches!(form.page, super::super::form::ProviderFormPage::UsageQuery)
        ));

        let action = app.on_key(key(KeyCode::Esc), &data);
        assert!(matches!(action, Action::ConfirmUsageQueryNotice));
    }

    #[test]
    fn provider_form_navigation_skips_usage_query_divider() {
        let mut app = App::new(Some(AppType::Claude));
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Claude,
        )));

        // The focusable field directly above the usage-query divider (whatever
        // the last Claude quick toggle happens to be) is our anchor.
        let field_before_divider = {
            let Some(FormState::ProviderAdd(form)) = app.form.as_mut() else {
                panic!("expected ProviderAdd form");
            };
            form.focus = FormFocus::Fields;
            form.clear_text_edit();
            let fields = form.fields();
            let divider_idx = fields
                .iter()
                .position(|field| *field == ProviderAddField::UsageQueryDivider)
                .expect("UsageQueryDivider field should exist");
            form.field_idx = divider_idx - 1;
            fields[form.field_idx]
        };

        app.on_key(key(KeyCode::Down), &UiData::default());
        assert!(matches!(
            app.form,
            Some(FormState::ProviderAdd(ref form))
                if form.fields().get(form.field_idx) == Some(&ProviderAddField::UsageQuery)
        ));

        app.on_key(key(KeyCode::Up), &UiData::default());
        assert!(matches!(
            app.form,
            Some(FormState::ProviderAdd(ref form))
                if form.fields().get(form.field_idx) == Some(&field_before_divider)
        ));
    }

    #[test]
    fn provider_form_usage_query_tab_switches_between_fields_preview_and_help() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data);

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.open_usage_query_page();
            form.toggle_usage_query_enabled();
        } else {
            panic!("expected ProviderAdd form");
        }

        app.on_key(key(KeyCode::Tab), &data);
        assert!(matches!(
            app.form,
            Some(FormState::ProviderAdd(ref form))
                if matches!(form.page, super::super::form::ProviderFormPage::UsageQuery)
                    && form.focus == FormFocus::JsonPreview
        ));

        let before = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form.usage_query_field_idx,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        app.on_key(key(KeyCode::Down), &data);
        let after = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form.usage_query_field_idx,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(after, before);

        app.on_key(key(KeyCode::Tab), &data);
        assert!(matches!(
            app.form,
            Some(FormState::ProviderAdd(ref form)) if form.focus == FormFocus::Content
        ));

        app.on_key(key(KeyCode::Tab), &data);
        assert!(matches!(
            app.form,
            Some(FormState::ProviderAdd(ref form)) if form.focus == FormFocus::Fields
        ));
    }

    #[test]
    fn provider_form_usage_query_jk_navigates_fields() {
        let mut app = App::new(Some(AppType::Claude));
        app.focus = Focus::Content;
        app.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            AppType::Claude,
        )));
        let Some(FormState::ProviderAdd(form)) = app.form.as_mut() else {
            panic!("expected ProviderAdd form");
        };
        form.open_usage_query_page();
        form.toggle_usage_query_enabled();
        form.focus = FormFocus::Fields;

        app.on_key(key(KeyCode::Char('j')), &UiData::default());
        assert!(matches!(
            app.form,
            Some(FormState::ProviderAdd(ref form)) if form.usage_query_field_idx == 1
        ));

        app.on_key(key(KeyCode::Char('k')), &UiData::default());
        assert!(matches!(
            app.form,
            Some(FormState::ProviderAdd(ref form)) if form.usage_query_field_idx == 0
        ));
    }

    #[test]
    fn provider_form_usage_query_side_panel_enter_opens_script_editor() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data);

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.open_usage_query_page();
            form.toggle_usage_query_enabled();
            form.focus = FormFocus::JsonPreview;
        } else {
            panic!("expected ProviderAdd form");
        }

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.editor.as_ref(),
            Some(editor)
                if matches!(editor.kind, EditorKind::Plain)
                    && matches!(editor.submit, EditorSubmit::ProviderFormApplyUsageScriptCode)
        ));
    }

    #[test]
    fn provider_form_usage_query_help_panel_enter_opens_read_only_detail_view() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data);

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.open_usage_query_page();
            form.toggle_usage_query_enabled();
            form.focus = FormFocus::Content;
        } else {
            panic!("expected ProviderAdd form");
        }

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(app.editor.is_none());

        let Overlay::TextView(view) = &app.overlay else {
            panic!("expected TextView overlay, got {:?}", app.overlay);
        };
        assert_eq!(view.title, texts::tui_usage_query_script_help_title());
        assert!(view
            .lines
            .iter()
            .any(|line| line.contains("{{baseUrl}}/api/usage")));
        assert!(view.action.is_none());
    }

    #[test]
    fn provider_form_usage_query_enable_then_save_writes_usage_script() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data);

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.name.set("Provider One");
            form.open_usage_query_page();
        } else {
            panic!("expected ProviderAdd form");
        }

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));

        select_usage_query_field(&mut app, UsageQueryField::BaseUrl);
        app.on_key(key(KeyCode::Enter), &data);
        for ch in "https://usage.example.test".chars() {
            app.on_key(key(KeyCode::Char(ch)), &data);
        }

        // Ctrl+S saves the whole provider draft from a sub-page as well.
        let submit = app.on_key(ctrl(KeyCode::Char('s')), &data);
        assert!(matches!(
            submit,
            Action::EditorSubmit {
                submit: EditorSubmit::ProviderAdd,
                content
            } if content.contains("\"usage_script\"")
                && content.contains("\"enabled\": true")
                && content.contains("\"templateType\": \"general\"")
                && content.contains("https://usage.example.test")
        ));
    }

    #[test]
    fn provider_save_keeps_usage_script_validation_error_on_the_preview() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        let mut form = ProviderAddFormState::new(AppType::Claude);
        form.name.set("Provider One");
        form.id.set("provider-one");
        form.open_usage_query_page();
        form.usage_query_enabled = true;
        form.usage_query_template = UsageQueryTemplate::General;
        form.usage_query_code.clear();
        app.form = Some(FormState::ProviderAdd(form));

        let action = app.on_key(ctrl(KeyCode::Char('s')), &data());

        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.form,
            Some(FormState::ProviderAdd(ref form))
                if form.page == form::ProviderFormPage::UsageQuery
                    && form.focus == FormFocus::JsonPreview
                    && form
                        .usage_query_field_error(UsageQueryField::Script)
                        .is_some_and(|message| !message.is_empty())
        ));
    }

    #[test]
    fn provider_save_keeps_usage_script_error_when_name_validation_runs_first() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        let mut form = ProviderAddFormState::new(AppType::Claude);
        form.open_usage_query_page();
        form.usage_query_enabled = true;
        form.usage_query_template = UsageQueryTemplate::General;
        form.usage_query_code = "const result = response;".to_string();
        form.set_usage_query_field_error(UsageQueryField::Script, "old script error");
        app.form = Some(FormState::ProviderAdd(form));

        let action = app.on_key(ctrl(KeyCode::Char('s')), &data());

        assert!(matches!(action, Action::None));
        let Some(FormState::ProviderAdd(form)) = app.form.as_ref() else {
            panic!("expected provider form");
        };
        assert_eq!(form.page, form::ProviderFormPage::Main);
        assert_eq!(form.fields()[form.field_idx], ProviderAddField::Name);
        assert!(form.main_field_error(ProviderAddField::Name).is_some());
        assert!(form
            .usage_query_field_error(UsageQueryField::Script)
            .is_some_and(|message| message.contains("return")));
    }

    #[test]
    fn hermes_valid_base_url_does_not_skip_usage_script_validation() {
        let mut app = App::new(Some(AppType::Hermes));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        let mut form = ProviderAddFormState::new(AppType::Hermes);
        form.id.set("hermes-provider");
        form.name.set("Hermes Provider");
        form.hermes_base_url.set("not-a-url");
        form.usage_query_enabled = true;
        form.usage_query_template = UsageQueryTemplate::General;
        form.usage_query_code = "const result = response;".to_string();
        app.form = Some(FormState::ProviderAdd(form));

        let invalid_base_action = app.on_key(ctrl(KeyCode::Char('s')), &data());
        assert!(matches!(invalid_base_action, Action::None));
        let Some(FormState::ProviderAdd(form)) = app.form.as_mut() else {
            panic!("expected provider form");
        };
        assert_eq!(
            form.fields()[form.field_idx],
            ProviderAddField::HermesBaseUrl
        );
        form.hermes_base_url.set("https://hermes.example.test/v1");

        let invalid_script_action = app.on_key(ctrl(KeyCode::Char('s')), &data());

        assert!(matches!(invalid_script_action, Action::None));
        let Some(FormState::ProviderAdd(form)) = app.form.as_ref() else {
            panic!("expected provider form");
        };
        assert_eq!(form.page, form::ProviderFormPage::UsageQuery);
        assert_eq!(form.focus, FormFocus::JsonPreview);
        assert!(form
            .usage_query_field_error(UsageQueryField::Script)
            .is_some_and(|message| message.contains("return")));
    }

    #[test]
    fn usage_query_space_only_toggles_boolean_fields() {
        let mut app = open_provider_fields_form(AppType::Claude);
        let Some(FormState::ProviderAdd(form)) = app.form.as_mut() else {
            panic!("expected ProviderAdd form");
        };
        form.open_usage_query_page();

        select_usage_query_field(&mut app, UsageQueryField::Enabled);
        app.on_key(key(KeyCode::Char(' ')), &data());
        assert!(matches!(
            app.form,
            Some(FormState::ProviderAdd(ref form)) if form.usage_query_enabled
        ));

        select_usage_query_field(&mut app, UsageQueryField::Template);
        app.on_key(key(KeyCode::Char(' ')), &data());
        assert!(matches!(app.overlay, Overlay::None));

        select_usage_query_field(&mut app, UsageQueryField::BaseUrl);
        app.on_key(key(KeyCode::Char(' ')), &data());
        assert!(matches!(
            app.form,
            Some(FormState::ProviderAdd(ref form)) if form.text_edit.is_none()
        ));
    }

    #[test]
    fn provider_form_usage_query_script_opens_plain_editor() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data);

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.open_usage_query_page();
            form.toggle_usage_query_enabled();
            form.focus = FormFocus::JsonPreview;
        } else {
            panic!("expected ProviderAdd form");
        }

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.editor.as_ref(),
            Some(editor)
                if matches!(editor.kind, EditorKind::Plain)
                    && matches!(editor.submit, EditorSubmit::ProviderFormApplyUsageScriptCode)
        ));
    }

    // ------------------------------------------------------------------
    // Claude model fill-all confirmation tests
    // ------------------------------------------------------------------

    fn setup_claude_model_picker_with_models() -> App {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.form = Some(FormState::ProviderAdd(
            super::super::form::ProviderAddFormState::new(AppType::Claude),
        ));
        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.claude_model.set("claude-sonnet-4-20250514");
            form.claude_haiku_model.set("claude-sonnet-4-20250514");
            form.claude_sonnet_model.set("");
            form.claude_opus_model.set("");
        }
        app.overlay = Overlay::ClaudeModelPicker {
            selected: 0,
            column: ClaudeModelPickerColumn::Model,
            editing: false,
        };
        app
    }

    fn session_meta(
        provider_id: &str,
        session_id: &str,
        title: &str,
        project_dir: &str,
        source_path: &str,
        resume_command: &str,
    ) -> crate::session_manager::SessionMeta {
        crate::session_manager::SessionMeta {
            provider_id: provider_id.to_string(),
            session_id: session_id.to_string(),
            title: Some(title.to_string()),
            summary: Some("Review routing".to_string()),
            project_dir: Some(project_dir.to_string()),
            created_at: Some(1_735_689_600_000),
            last_active_at: Some(1_735_732_800_000),
            source_path: Some(source_path.to_string()),
            resume_command: Some(resume_command.to_string()),
        }
    }

    fn session_meta_for_app(provider_id: &str) -> crate::session_manager::SessionMeta {
        session_meta(
            provider_id,
            "session-1",
            "Session One",
            "/tmp/project",
            "/tmp/session.jsonl",
            &match provider_id {
                "claude" => "claude --resume session-1".to_string(),
                "codex" => "codex resume session-1".to_string(),
                other => format!("{other} resume session-1"),
            },
        )
    }

    fn app_with_session_page() -> App {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Sessions;
        app.focus = Focus::Content;
        app.sessions.loaded_once = true;
        app.sessions.provider_id = Some("claude".to_string());
        app.sessions.rows.push(session_meta_for_app("claude"));
        app
    }

    #[test]
    fn session_project_picker_filters_and_applies_an_exact_scope() {
        let directory = tempfile::tempdir().expect("manifest fixture directory");
        let store =
            crate::session_manager::paged_manifest::PagedManifestStore::open_at(directory.path())
                .expect("manifest fixture store");
        let mut builder = store
            .begin_build("claude")
            .expect("manifest fixture builder");
        builder
            .push(session_meta(
                "claude",
                "alpha",
                "Alpha",
                "/repo/alpha",
                "/tmp/alpha.jsonl",
                "claude --resume alpha",
            ))
            .expect("alpha row");
        builder
            .push(session_meta(
                "claude",
                "beta",
                "Beta",
                "/repo/beta",
                "/tmp/beta.jsonl",
                "claude --resume beta",
            ))
            .expect("beta row");
        let mut unknown = session_meta(
            "claude",
            "unknown",
            "Unknown",
            "/unused",
            "/tmp/unknown.jsonl",
            "claude --resume unknown",
        );
        unknown.project_dir = None;
        builder.push(unknown).expect("unknown project row");
        let published = builder.publish().expect("publish manifest fixture");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Sessions;
        app.focus = Focus::Content;
        let _ = app.sessions.start_scan("claude".to_string());
        let scope_epoch = app.sessions.scope_epoch;
        assert!(app.sessions.remember_base_manifest(
            scope_epoch,
            "claude",
            published.generation.clone(),
            published.total_rows,
            published.reader.clone(),
        ));
        let catalog = crate::session_manager::project_scope::aggregate_project_directories(
            &published.reader,
            &|| false,
        )
        .expect("project catalog");
        let request_id = app
            .sessions
            .start_project_catalog()
            .expect("catalog request");
        assert!(app.sessions.finish_project_catalog(
            request_id,
            scope_epoch,
            "claude".to_string(),
            published.generation,
            catalog,
        ));
        app.overlay = Overlay::SessionProjectPicker(SessionProjectPickerState {
            input: crate::cli::tui::text_edit::TextInput::new(""),
            selected_idx: 0,
            path_scroll: 0,
            filtered_indices: None,
            pinned_scope: None,
            filter_error: None,
        });

        let Overlay::SessionProjectPicker(picker) = &app.overlay else {
            panic!("project picker remains open");
        };
        assert_eq!(session_project_option_count(&app.sessions, picker), 4);
        assert!(matches!(
            session_project_option_at(&app.sessions, picker, 0),
            Some(SessionProjectOption::All { .. })
        ));
        assert!(matches!(
            session_project_option_at(&app.sessions, picker, 1),
            Some(SessionProjectOption::Exact { .. })
        ));
        assert!(matches!(
            session_project_option_at(&app.sessions, picker, 2),
            Some(SessionProjectOption::Exact { .. })
        ));
        assert!(matches!(
            session_project_option_at(&app.sessions, picker, 3),
            Some(SessionProjectOption::Unknown { .. })
        ));
        let unknown_filter = session_project_filter_source(
            &app.sessions,
            picker,
            &texts::tui_sessions_unknown_project().to_lowercase(),
        )
        .expect("unknown project filter source");
        assert!(unknown_filter.fixed_matches.is_empty());
        assert_eq!(unknown_filter.project_offset, 1);
        assert_eq!(unknown_filter.trailing_matches, vec![3]);
        app.sessions.project_scope =
            crate::session_manager::project_scope::SessionProjectScope::Unknown;
        assert_eq!(
            session_project_active_option_index(&app.sessions, picker),
            3
        );
        app.sessions.project_scope =
            crate::session_manager::project_scope::SessionProjectScope::All;

        let Overlay::SessionProjectPicker(picker) = &mut app.overlay else {
            panic!("project picker remains open");
        };
        picker.selected_idx = 1;
        assert!(matches!(
            app.on_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT), &data()),
            Action::None
        ));
        let Overlay::SessionProjectPicker(picker) = &app.overlay else {
            panic!("project picker remains open");
        };
        assert_ne!(picker.path_scroll, 0);
        assert!(matches!(
            app.on_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT), &data()),
            Action::None
        ));
        let Overlay::SessionProjectPicker(picker) = &app.overlay else {
            panic!("project picker remains open");
        };
        assert_eq!(picker.path_scroll, 0);
        assert!(matches!(
            app.on_key(KeyEvent::new(KeyCode::End, KeyModifiers::SHIFT), &data()),
            Action::None
        ));
        let Overlay::SessionProjectPicker(picker) = &app.overlay else {
            panic!("project picker remains open");
        };
        assert_eq!(picker.path_scroll, usize::MAX);
        assert!(matches!(
            app.on_key(KeyEvent::new(KeyCode::Home, KeyModifiers::SHIFT), &data()),
            Action::None
        ));
        let Overlay::SessionProjectPicker(picker) = &app.overlay else {
            panic!("project picker remains open");
        };
        assert_eq!(picker.path_scroll, 0);

        assert!(matches!(
            app.on_key(key(KeyCode::Char('b')), &data()),
            Action::SessionsProjectFilter { query } if query == "b"
        ));
        let Overlay::SessionProjectPicker(picker) = &app.overlay else {
            panic!("project picker remains open");
        };
        assert!(picker
            .filtered_indices
            .as_deref()
            .is_some_and(<[_]>::is_empty));

        // Project matching is intentionally performed by the sessions worker;
        // install its raw option index before exercising Enter selection.
        let Overlay::SessionProjectPicker(picker) = &mut app.overlay else {
            panic!("project picker remains open");
        };
        picker.filtered_indices = Some(vec![2]);

        let action = app.on_key(key(KeyCode::Enter), &data());
        assert!(matches!(
            action,
            Action::SessionsProjectApply {
                scope: crate::session_manager::project_scope::SessionProjectScope::Exact {
                    ref display_path,
                    ..
                }
            } if display_path == "/repo/beta"
        ));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn project_picker_help_and_escape_restore_then_cancel_the_picker() {
        let _lang = use_test_language(Language::English);
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Sessions;
        app.overlay = Overlay::SessionProjectPicker(SessionProjectPickerState {
            input: crate::cli::tui::text_edit::TextInput::new("repo"),
            selected_idx: 0,
            path_scroll: 0,
            filtered_indices: Some(Vec::new()),
            pinned_scope: None,
            filter_error: None,
        });

        assert!(matches!(
            app.on_key(key(KeyCode::Char('?')), &data()),
            Action::None
        ));
        assert!(matches!(app.overlay, Overlay::Help(_)));
        let help = help_text(&app);
        assert!(help.contains("h/l are aliases"), "{help}");
        assert!(help.contains("PgUp/PgDn"), "{help}");
        assert!(
            help.contains("Sessions always show the current app"),
            "{help}"
        );
        assert!(matches!(
            app.pending_overlay,
            Some(Overlay::SessionProjectPicker(_))
        ));

        assert!(matches!(
            app.on_key(key(KeyCode::Esc), &data()),
            Action::None
        ));
        assert!(matches!(app.overlay, Overlay::SessionProjectPicker(_)));
        assert!(matches!(
            app.on_key(key(KeyCode::Esc), &data()),
            Action::SessionsProjectFilterCancel
        ));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn failover_queue_help_keeps_secondary_keys_and_behavior_off_the_main_surface() {
        let _lang = use_test_language(Language::English);
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.overlay = Overlay::FailoverQueueManager {
            selected_provider_id: Some("p1".to_string()),
        };

        assert!(matches!(
            app.on_key(key(KeyCode::Char('?')), &data()),
            Action::None
        ));
        let help = help_text(&app);
        assert!(help.contains("J/K are secondary move aliases"), "{help}");
        assert!(help.contains("P1, P2"), "{help}");
        assert!(help.contains("passive history"), "{help}");
        assert!(matches!(
            app.pending_overlay,
            Some(Overlay::FailoverQueueManager { .. })
        ));

        assert!(matches!(
            app.on_key(key(KeyCode::Esc), &data()),
            Action::None
        ));
        assert!(matches!(app.overlay, Overlay::FailoverQueueManager { .. }));
    }

    #[test]
    fn leaving_sessions_releases_a_cancelled_project_catalog_request() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Sessions;
        app.sessions.project_catalog_active = Some(7);
        app.sessions.project_catalog_loading = true;
        app.pending_project_catalog = true;
        let mut data = UiData::default();

        run_runtime_action(&mut app, &mut data, Action::SwitchRoute(Route::Main))
            .expect("leave sessions");

        assert_eq!(app.sessions.project_catalog_active, None);
        assert!(!app.sessions.project_catalog_loading);
        assert!(!app.pending_project_catalog);
    }

    #[test]
    fn project_scope_query_edit_cancel_preserves_the_replacement_debounce() {
        let mut app = App::new(Some(AppType::Claude));
        app.sessions.project_scope =
            crate::session_manager::project_scope::SessionProjectScope::exact("/repo/alpha")
                .expect("exact project");
        app.sessions.deep_search_query = Some("old".to_string());
        app.sessions.deep_search_pending = Some(("new".to_string(), 0));
        let mut data = UiData::default();

        run_runtime_action(&mut app, &mut data, Action::SessionsDeepSearchCancel)
            .expect("cancel superseded search");

        assert_eq!(
            app.sessions.deep_search_pending.as_ref(),
            Some(&("new".to_string(), 0))
        );
        assert!(app.sessions.deep_search_query.is_none());
        assert!(app.sessions.deep_search_active.is_none());
    }

    #[test]
    fn applying_the_active_project_does_not_cancel_its_inflight_view() {
        let mut app = App::new(Some(AppType::Claude));
        let scope =
            crate::session_manager::project_scope::SessionProjectScope::exact("/repo/alpha")
                .expect("exact project");
        app.sessions.project_scope = scope.clone();
        app.sessions.deep_search_active = Some(17);
        let mut data = UiData::default();
        let (session_tx, session_rx) = std::sync::mpsc::channel();

        run_runtime_action_with_session_tx(
            &mut app,
            &mut data,
            &session_tx,
            Action::SessionsProjectApply { scope },
        )
        .expect("apply active project");

        assert_eq!(app.sessions.deep_search_active, Some(17));
        assert!(session_rx.try_recv().is_err());
    }

    #[test]
    fn sessions_page_keys_move_selection_by_page_and_clamp() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Sessions;
        app.focus = Focus::Content;
        app.sessions.pane = SessionsPane::List;
        app.sessions.loaded_once = true;
        app.sessions.provider_id = Some("claude".to_string());
        app.last_size = Size {
            width: 120,
            height: 40,
        };
        // Keep this fixture within one logical data page. Cross-page gating is
        // covered separately below and deliberately requires a second input.
        for i in 0..80 {
            app.sessions.rows.push(session_meta(
                "claude",
                &format!("s{i}"),
                "Title",
                "/tmp/p",
                "/tmp/s.jsonl",
                "claude --resume",
            ));
        }
        // page size = last_size.height(40) - chrome(9)
        let page = 40usize - 9;

        app.on_sessions_key(key(KeyCode::PageDown), &data());
        assert_eq!(app.sessions.selected_idx, page);

        app.on_sessions_key(key(KeyCode::End), &data());
        assert_eq!(app.sessions.selected_idx, 79);

        // PageDown at the bottom is clamped to the last row.
        app.on_sessions_key(key(KeyCode::PageDown), &data());
        assert_eq!(app.sessions.selected_idx, 79);

        app.on_sessions_key(key(KeyCode::PageUp), &data());
        assert_eq!(app.sessions.selected_idx, 79 - page);

        app.on_sessions_key(key(KeyCode::Home), &data());
        assert_eq!(app.sessions.selected_idx, 0);

        // PageUp at the top stays at 0.
        app.on_sessions_key(key(KeyCode::PageUp), &data());
        assert_eq!(app.sessions.selected_idx, 0);
    }

    #[test]
    fn sessions_page_down_requires_a_second_press_at_the_logical_page_boundary() {
        use crate::cli::tui::app::paged_list::{PageBoundary, PagedListFocus};

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Sessions;
        app.focus = Focus::Content;
        app.sessions.pane = SessionsPane::List;
        app.sessions.loaded_once = true;
        app.sessions.provider_id = Some("claude".to_string());
        app.last_size = Size {
            width: 120,
            height: 40,
        };
        for i in 0..201 {
            app.sessions.rows.push(session_meta(
                "claude",
                &format!("s{i}"),
                "Title",
                "/tmp/p",
                &format!("/tmp/s{i}.jsonl"),
                "claude --resume",
            ));
        }
        app.sessions.selected_idx = 90;

        app.on_sessions_key(key(KeyCode::PageDown), &data());
        assert_eq!(app.sessions.selected_idx, 99);
        assert_eq!(
            app.sessions.pagination.focus(),
            PagedListFocus::Boundary(PageBoundary::Next)
        );

        app.on_sessions_key(key(KeyCode::PageDown), &data());
        assert_eq!(app.sessions.selected_idx, 100);
        assert!(app.sessions.pagination.is_row_focused());
    }

    #[test]
    fn sessions_wheel_requires_a_new_gesture_to_cross_page_boundary() {
        use crate::cli::tui::app::paged_list::{PageBoundary, PagedListFocus};
        use crate::cli::tui::input::{ScrollDirection, WheelGestureId};

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Sessions;
        app.focus = Focus::Content;
        app.sessions.pane = SessionsPane::List;
        app.sessions.loaded_once = true;
        app.sessions.provider_id = Some("claude".to_string());
        for i in 0..250 {
            app.sessions.rows.push(session_meta(
                "claude",
                &format!("s{i}"),
                "Title",
                "/tmp/p",
                &format!("/tmp/s{i}.jsonl"),
                "claude --resume",
            ));
        }

        let first = WheelGestureId::from_raw(1);
        app.on_sessions_wheel(ScrollDirection::Down, 10_000, first, &data());
        assert_eq!(app.sessions.selected_idx, 99);
        assert_eq!(
            app.sessions.pagination.focus(),
            PagedListFocus::Boundary(PageBoundary::Next)
        );

        app.on_sessions_wheel(ScrollDirection::Down, 10_000, first, &data());
        assert_eq!(app.sessions.selected_idx, 99);

        let second = WheelGestureId::from_raw(2);
        app.on_sessions_wheel(ScrollDirection::Down, 10_000, second, &data());
        assert_eq!(app.sessions.selected_idx, 100);
        assert!(app.sessions.pagination.is_row_focused());

        // Remaining reports from the crossing gesture cannot move within the
        // newly entered page, keeping its first row stable.
        app.on_sessions_wheel(ScrollDirection::Down, 10_000, second, &data());
        assert_eq!(app.sessions.selected_idx, 100);
    }

    #[test]
    fn sessions_wheel_keeps_visible_selection_at_tombstone_page_edge() {
        use crate::cli::tui::app::paged_list::{PageBoundary, PagedListFocus};
        use crate::cli::tui::input::{ScrollDirection, WheelGestureId};

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Sessions;
        app.focus = Focus::Content;
        app.sessions.pane = SessionsPane::List;
        let _request_id = app.sessions.start_scan("claude".to_string());
        let scope_epoch = app.sessions.scope_epoch;
        let manifest_dir = tempfile::tempdir().expect("manifest fixture directory");
        let store = crate::session_manager::paged_manifest::PagedManifestStore::open_at(
            manifest_dir.path(),
        )
        .expect("manifest fixture store");
        let mut builder = store
            .begin_build("claude")
            .expect("manifest fixture builder");
        for index in 0..101 {
            builder
                .push(session_meta(
                    "claude",
                    &format!("s{index}"),
                    "Title",
                    "/tmp/p",
                    &format!("/tmp/s{index}.jsonl"),
                    "claude --resume",
                ))
                .expect("manifest fixture row");
        }
        let published = builder.publish().expect("publish manifest fixture");
        assert!(app.sessions.apply_opened_manifest(
            scope_epoch,
            "claude",
            published.generation,
            published.total_rows,
            published.first_page.page_index,
            published.first_page.rows,
            published.reader,
        ));
        for _ in 0..10 {
            let key = crate::cli::tui::app::session_key(&app.sessions.rows[0]);
            assert!(app.sessions.remove_session_by_key(&key));
        }
        assert_eq!(app.sessions.rows.len(), 90);
        app.sessions.selected_idx = 88;
        app.sessions.pagination.select(88);
        let last_key = crate::cli::tui::app::session_key(
            app.sessions.rows.last().expect("last visible session"),
        );
        app.sessions.open_detail(last_key.clone());

        app.on_sessions_wheel(
            ScrollDirection::Down,
            100,
            WheelGestureId::from_raw(1),
            &data(),
        );

        assert_eq!(app.sessions.pagination.selected_index(), Some(99));
        assert_eq!(app.sessions.selected_idx, 89);
        assert_eq!(
            crate::cli::tui::app::session_key(&app.sessions.rows[app.sessions.selected_idx]),
            last_key
        );
        assert_eq!(app.sessions.detail_key.as_deref(), Some(last_key.as_str()));
        assert_eq!(
            app.sessions.pagination.focus(),
            PagedListFocus::Boundary(PageBoundary::Next)
        );
    }

    #[test]
    fn sessions_boundary_enter_crosses_without_opening_detail() {
        use crate::cli::tui::input::{ScrollDirection, WheelGestureId};

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Sessions;
        app.focus = Focus::Content;
        app.sessions.pane = SessionsPane::List;
        let _ = app.sessions.start_scan("claude".to_string());
        let scope_epoch = app.sessions.scope_epoch;
        let mut all_rows: Vec<_> = (0..101)
            .map(|i| {
                session_meta(
                    "claude",
                    &format!("s{i}"),
                    "Title",
                    "/tmp/p",
                    &format!("/tmp/s{i}.jsonl"),
                    "claude --resume",
                )
            })
            .collect();
        let manifest_dir = tempfile::tempdir().expect("manifest fixture directory");
        let store = crate::session_manager::paged_manifest::PagedManifestStore::open_at(
            manifest_dir.path(),
        )
        .expect("manifest fixture store");
        let mut builder = store
            .begin_build("claude")
            .expect("manifest fixture builder");
        for row in all_rows.drain(..) {
            builder.push(row).expect("manifest fixture row");
        }
        let published = builder.publish().expect("publish manifest fixture");
        let reader = store
            .open_generation("claude", &published.generation)
            .expect("manifest fixture reader");
        assert!(app.sessions.apply_opened_manifest(
            scope_epoch,
            "claude",
            published.generation,
            published.total_rows,
            published.first_page.page_index,
            published.first_page.rows,
            reader,
        ));

        app.on_sessions_wheel(
            ScrollDirection::Down,
            1_000,
            WheelGestureId::from_raw(1),
            &data(),
        );
        let action = app.on_sessions_key(key(KeyCode::Enter), &data());

        assert!(matches!(action, Action::None));
        assert_eq!(app.sessions.selected_idx, 99);
        assert_eq!(app.sessions.remote.pending_cross_page(), Some(1));
        assert!(app.sessions.remote.input_is_blocked());
        assert_eq!(app.sessions.pane, SessionsPane::List);
    }

    #[test]
    fn sessions_a_is_unbound_and_keeps_current_app_history() {
        let mut app = app_with_session_page();
        let data = UiData::default();

        let action = app.on_key(key(KeyCode::Char('a')), &data);

        assert!(matches!(action, Action::None));
        assert_eq!(app.sessions.provider_id.as_deref(), Some("claude"));
        let visible = visible_sessions_for_state(
            &app.filter,
            &app.app_type,
            app.sessions.provider_id.as_deref(),
            &app.sessions.project_scope,
            &app.sessions.rows,
            app.sessions.detail_key.as_deref(),
            app.sessions.messages_loaded,
            &app.sessions.messages,
            app.sessions.deep_search_query.as_deref(),
            &app.sessions.deep_search_results,
            app.sessions
                .materialized_view_is_current(app.filter.query_lower().as_deref()),
            app.sessions.rows_revision,
            app.sessions.messages_revision,
            app.sessions.deep_search_seq,
            &app.sessions.visibility_cache,
        );
        assert_eq!(visible.len(), 1);
        assert_eq!(
            visible.get(0).map(|row| row.provider_id.as_str()),
            Some("claude")
        );
    }

    #[test]
    fn sessions_history_with_unverified_rows_is_current_app_only() {
        let app = app_with_session_page();
        let rows = vec![
            session_meta_for_app("claude"),
            session_meta_for_app("codex"),
        ];

        let visible = visible_sessions_for_state(
            &app.filter,
            &app.app_type,
            None,
            &app.sessions.project_scope,
            &rows,
            None,
            false,
            &[],
            None,
            &[],
            false,
            0,
            0,
            0,
            &app.sessions.visibility_cache,
        );

        assert_eq!(visible.len(), 1);
        assert_eq!(
            visible.get(0).map(|row| row.provider_id.as_str()),
            Some("claude")
        );
    }

    #[test]
    fn claude_model_fill_all_a_opens_confirm_without_changing_values() {
        let mut app = setup_claude_model_picker_with_models();

        let action = app.on_key(key(KeyCode::Char('a')), &data());
        assert!(matches!(action, Action::None));

        // Should show confirm dialog, not change any values
        assert!(matches!(
            &app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::ClaudeModelFillAll { source_idx: 0 },
                ..
            })
        ));

        // Values must be unchanged
        let form = match app.form.as_ref() {
            Some(FormState::ProviderAdd(f)) => f,
            _ => panic!("expected ProviderAdd form"),
        };
        assert_eq!(form.claude_model.value, "claude-sonnet-4-20250514");
        assert_eq!(form.claude_haiku_model.value, "claude-sonnet-4-20250514");
        assert_eq!(form.claude_sonnet_model.value, "");
        assert_eq!(form.claude_opus_model.value, "");
    }

    #[test]
    fn claude_model_fill_all_confirm_fills_all_fields() {
        let mut app = setup_claude_model_picker_with_models();

        // Press 'a' to open confirm
        app.on_key(key(KeyCode::Char('a')), &data());

        // Confirm with Enter
        let action = app.on_key(key(KeyCode::Enter), &data());
        assert!(matches!(action, Action::None));

        // Should return to ClaudeModelPicker
        assert!(matches!(
            app.overlay,
            Overlay::ClaudeModelPicker {
                selected: 0,
                column: ClaudeModelPickerColumn::Model,
                editing: false
            }
        ));

        // All fields should now have the source value
        let form = match app.form.as_ref() {
            Some(FormState::ProviderAdd(f)) => f,
            _ => panic!("expected ProviderAdd form"),
        };
        assert_eq!(form.claude_model.value, "claude-sonnet-4-20250514");
        assert_eq!(form.claude_haiku_model.value, "claude-sonnet-4-20250514");
        assert_eq!(form.claude_sonnet_model.value, "claude-sonnet-4-20250514");
        assert_eq!(form.claude_opus_model.value, "claude-sonnet-4-20250514");
    }

    #[test]
    fn claude_model_fill_all_confirm_y_fills_all_fields() {
        let mut app = setup_claude_model_picker_with_models();
        app.on_key(key(KeyCode::Char('a')), &data());

        let action = app.on_key(key(KeyCode::Char('y')), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::ClaudeModelPicker {
                selected: 0,
                column: ClaudeModelPickerColumn::Model,
                editing: false
            }
        ));

        let form = match app.form.as_ref() {
            Some(FormState::ProviderAdd(f)) => f,
            _ => panic!("expected ProviderAdd form"),
        };
        assert_eq!(form.claude_haiku_model.value, "claude-sonnet-4-20250514");
        assert_eq!(form.claude_opus_model.value, "claude-sonnet-4-20250514");
    }

    #[test]
    fn claude_model_fill_all_cancel_esc_leaves_values_unchanged() {
        let mut app = setup_claude_model_picker_with_models();
        app.on_key(key(KeyCode::Char('a')), &data());

        let action = app.on_key(key(KeyCode::Esc), &data());
        assert!(matches!(action, Action::None));

        // Should return to ClaudeModelPicker
        assert!(matches!(
            app.overlay,
            Overlay::ClaudeModelPicker {
                selected: 0,
                column: ClaudeModelPickerColumn::Model,
                editing: false
            }
        ));

        // Values must be unchanged
        let form = match app.form.as_ref() {
            Some(FormState::ProviderAdd(f)) => f,
            _ => panic!("expected ProviderAdd form"),
        };
        assert_eq!(form.claude_model.value, "claude-sonnet-4-20250514");
        assert_eq!(form.claude_haiku_model.value, "claude-sonnet-4-20250514");
        assert_eq!(form.claude_opus_model.value, "");
    }

    #[test]
    fn claude_model_fill_all_cancel_n_leaves_values_unchanged() {
        let mut app = setup_claude_model_picker_with_models();
        app.on_key(key(KeyCode::Char('a')), &data());

        let action = app.on_key(key(KeyCode::Char('n')), &data());
        assert!(matches!(action, Action::None));

        assert!(matches!(
            app.overlay,
            Overlay::ClaudeModelPicker {
                selected: 0,
                column: ClaudeModelPickerColumn::Model,
                editing: false
            }
        ));

        let form = match app.form.as_ref() {
            Some(FormState::ProviderAdd(f)) => f,
            _ => panic!("expected ProviderAdd form"),
        };
        assert_eq!(form.claude_model.value, "claude-sonnet-4-20250514");
        assert_eq!(form.claude_haiku_model.value, "claude-sonnet-4-20250514");
        assert_eq!(form.claude_opus_model.value, "");
    }

    #[test]
    fn claude_model_fill_all_empty_source_is_noop() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.form = Some(FormState::ProviderAdd(
            super::super::form::ProviderAddFormState::new(AppType::Claude),
        ));
        // All role fields are empty by default; selected=0 (Haiku) is empty.
        app.overlay = Overlay::ClaudeModelPicker {
            selected: 0,
            column: ClaudeModelPickerColumn::Model,
            editing: false,
        };

        let action = app.on_key(key(KeyCode::Char('a')), &data());
        assert!(matches!(action, Action::None));

        // Should stay on ClaudeModelPicker (no confirm dialog)
        assert!(matches!(
            app.overlay,
            Overlay::ClaudeModelPicker {
                selected: 0,
                column: ClaudeModelPickerColumn::Model,
                editing: false
            }
        ));
    }

    #[test]
    fn claude_model_fill_all_from_non_first_field() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.form = Some(FormState::ProviderAdd(
            super::super::form::ProviderAddFormState::new(AppType::Claude),
        ));
        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.claude_opus_model.set("claude-opus-4-20250514");
        }
        // Select the Opus role (index 2 of the three role rows).
        app.overlay = Overlay::ClaudeModelPicker {
            selected: 2,
            column: ClaudeModelPickerColumn::Model,
            editing: false,
        };

        app.on_key(key(KeyCode::Char('a')), &data());

        // Confirm should reference source_idx=2.
        assert!(matches!(
            &app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::ClaudeModelFillAll { source_idx: 2 },
                ..
            })
        ));

        app.on_key(key(KeyCode::Enter), &data());

        let form = match app.form.as_ref() {
            Some(FormState::ProviderAdd(f)) => f,
            _ => panic!("expected ProviderAdd form"),
        };
        // Fill-all only spans the three role models; the fallback model is separate.
        assert_eq!(form.claude_haiku_model.value, "claude-opus-4-20250514");
        assert_eq!(form.claude_sonnet_model.value, "claude-opus-4-20250514");
        assert_eq!(form.claude_opus_model.value, "claude-opus-4-20250514");
        assert_eq!(form.claude_model.value, "");
    }

    #[test]
    fn claude_model_fill_all_copies_one_m_only_to_supported_roles() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        let mut form = super::super::form::ProviderAddFormState::new(AppType::Claude);
        form.set_claude_model_from_config(1, "shared-model[1M]");
        app.form = Some(FormState::ProviderAdd(form));
        app.overlay = Overlay::ClaudeModelPicker {
            selected: 1,
            column: ClaudeModelPickerColumn::Model,
            editing: false,
        };

        app.on_key(key(KeyCode::Char('a')), &data());
        app.on_key(key(KeyCode::Enter), &data());

        let form = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form,
            _ => panic!("expected ProviderAdd form"),
        };
        assert_eq!(form.claude_haiku_model.value, "shared-model");
        assert!(!form.claude_model_one_m_enabled(0));
        assert!(form.claude_model_one_m_enabled(1));
        assert!(form.claude_model_one_m_enabled(2));
    }

    // ------------------------------------------------------------------
    // ModelFetchPicker restore tests
    // ------------------------------------------------------------------

    #[test]
    fn model_fetch_picker_claude_enter_restores_to_claude_model_picker() {
        let mut app = App::new(Some(AppType::Claude));
        app.form = Some(FormState::ProviderAdd(
            super::super::form::ProviderAddFormState::new(AppType::Claude),
        ));
        app.overlay = Overlay::ModelFetchPicker {
            request_id: 1,
            field: ProviderAddField::ClaudeModelConfig,
            claude_idx: Some(0),
            input: TextInput::new("claude-haiku-4-20250514"),
            query: String::new(),
            fetching: false,
            models: vec!["claude-haiku-4-20250514".to_string()],
            filtered_indices: None,
            filter_incomplete: false,
            error: None,
            selected_idx: 0,
            selection_active: false,
        };

        let action = app.on_key(key(KeyCode::Enter), &data());
        assert!(matches!(action, Action::None));

        // Should restore to ClaudeModelPicker with the Haiku row selected.
        assert!(matches!(
            app.overlay,
            Overlay::ClaudeModelPicker {
                selected: 0,
                column: ClaudeModelPickerColumn::Model,
                editing: false
            }
        ));

        // The model field should be set
        let form = match app.form.as_ref() {
            Some(FormState::ProviderAdd(f)) => f,
            _ => panic!("expected ProviderAdd form"),
        };
        assert_eq!(form.claude_haiku_model.value, "claude-haiku-4-20250514");
    }

    #[test]
    fn model_fetch_picker_claude_preserves_or_enables_one_m() {
        let mut app = App::new(Some(AppType::Claude));
        let mut form = super::super::form::ProviderAddFormState::new(AppType::Claude);
        form.set_claude_model_from_config(1, "old-sonnet[1M]");
        app.form = Some(FormState::ProviderAdd(form));
        app.overlay = Overlay::ModelFetchPicker {
            request_id: 1,
            field: ProviderAddField::ClaudeModelConfig,
            claude_idx: Some(1),
            input: TextInput::new("new-sonnet"),
            query: String::new(),
            fetching: false,
            models: vec!["new-sonnet".to_string()],
            filtered_indices: None,
            filter_incomplete: false,
            error: None,
            selected_idx: 0,
            selection_active: false,
        };

        app.on_key(key(KeyCode::Enter), &data());
        let form = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form,
            _ => panic!("expected ProviderAdd form"),
        };
        assert_eq!(form.claude_sonnet_model.value, "new-sonnet");
        assert!(form.claude_model_one_m_enabled(1));

        app.overlay = Overlay::ModelFetchPicker {
            request_id: 2,
            field: ProviderAddField::ClaudeModelConfig,
            claude_idx: Some(2),
            input: TextInput::new("new-opus [1m]"),
            query: String::new(),
            fetching: false,
            models: vec!["new-opus [1m]".to_string()],
            filtered_indices: None,
            filter_incomplete: false,
            error: None,
            selected_idx: 0,
            selection_active: false,
        };
        app.on_key(key(KeyCode::Enter), &data());

        let form = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form,
            _ => panic!("expected ProviderAdd form"),
        };
        assert_eq!(form.claude_opus_model.value, "new-opus");
        assert!(form.claude_model_one_m_enabled(2));
        assert!(matches!(
            app.overlay,
            Overlay::ClaudeModelPicker {
                selected: 2,
                column: ClaudeModelPickerColumn::Model,
                editing: false,
            }
        ));
    }

    #[test]
    fn model_fetch_picker_claude_esc_restores_to_claude_model_picker() {
        let mut app = App::new(Some(AppType::Claude));
        app.form = Some(FormState::ProviderAdd(
            super::super::form::ProviderAddFormState::new(AppType::Claude),
        ));
        app.overlay = Overlay::ModelFetchPicker {
            request_id: 1,
            field: ProviderAddField::ClaudeModelConfig,
            claude_idx: Some(2),
            input: TextInput::new(""),
            query: String::new(),
            fetching: false,
            models: vec!["model-a".to_string()],
            filtered_indices: None,
            filter_incomplete: false,
            error: None,
            selected_idx: 0,
            selection_active: false,
        };

        let action = app.on_key(key(KeyCode::Esc), &data());
        assert!(matches!(action, Action::None));

        // Should restore to ClaudeModelPicker without setting any value
        assert!(matches!(
            app.overlay,
            Overlay::ClaudeModelPicker {
                selected: 2,
                column: ClaudeModelPickerColumn::Model,
                editing: false
            }
        ));

        let form = match app.form.as_ref() {
            Some(FormState::ProviderAdd(f)) => f,
            _ => panic!("expected ProviderAdd form"),
        };
        assert_eq!(form.claude_opus_model.value, "");
    }

    #[test]
    fn model_fetch_picker_claude_enter_empty_picks_first_model() {
        let mut app = App::new(Some(AppType::Claude));
        app.form = Some(FormState::ProviderAdd(
            super::super::form::ProviderAddFormState::new(AppType::Claude),
        ));
        app.overlay = Overlay::ModelFetchPicker {
            request_id: 1,
            field: ProviderAddField::ClaudeModelConfig,
            claude_idx: Some(0),
            input: TextInput::new(""),
            query: String::new(),
            fetching: false,
            models: vec!["model-a".to_string(), "model-b".to_string()],
            filtered_indices: None,
            filter_incomplete: false,
            error: None,
            selected_idx: 0,
            selection_active: false,
        };

        let action = app.on_key(key(KeyCode::Enter), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::ClaudeModelPicker {
                selected: 0,
                column: ClaudeModelPickerColumn::Model,
                editing: false
            }
        ));

        // Should have picked the first model from the list into index 0 (Haiku role).
        let form = match app.form.as_ref() {
            Some(FormState::ProviderAdd(f)) => f,
            _ => panic!("expected ProviderAdd form"),
        };
        assert_eq!(form.claude_haiku_model.value, "model-a");
    }

    #[test]
    fn model_fetch_picker_hermes_enter_restores_via_pending_overlay() {
        let mut app = App::new(Some(AppType::Hermes));
        app.form = Some(FormState::ProviderAdd(
            super::super::form::ProviderAddFormState::new(AppType::Hermes),
        ));
        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.open_hermes_models_picker();
            form.add_empty_hermes_model();
        }
        app.overlay = Overlay::ModelFetchPicker {
            request_id: 1,
            field: ProviderAddField::HermesModels,
            claude_idx: None,
            input: TextInput::new("gpt-5.4"),
            query: "gpt-5.4".to_string(),
            fetching: false,
            models: vec!["gpt-5.4".to_string()],
            filtered_indices: None,
            filter_incomplete: false,
            error: None,
            selected_idx: 0,
            selection_active: false,
        };
        app.pending_overlay = Some(Overlay::HermesModelsPicker { editing: false });

        let action = app.on_key(key(KeyCode::Enter), &data());
        assert!(matches!(action, Action::None));

        // Should restore to HermesModelsPicker via pending_overlay
        assert!(matches!(
            app.overlay,
            Overlay::HermesModelsPicker { editing: false }
        ));
    }

    #[test]
    fn sessions_arrow_keys_move_through_nav_list_and_detail() {
        let mut app = app_with_session_page();
        let data = UiData::default();

        app.focus = Focus::Nav;
        app.sessions.pane = SessionsPane::Detail;

        app.on_key(key(KeyCode::Right), &data);
        assert_eq!(app.focus, Focus::Content);
        assert_eq!(app.sessions.pane, SessionsPane::List);

        assert_eq!(app.sessions.pane, SessionsPane::List);
        let expected_key = session_key(&app.sessions.rows[0]);
        let action = app.on_key(key(KeyCode::Right), &data);
        assert_eq!(app.focus, Focus::Content);
        assert_eq!(app.sessions.pane, SessionsPane::Detail);
        assert!(matches!(
            action,
            Action::SessionMessagesLoad {
                ref key,
                ref provider_id,
                ref source_path
            } if key == &expected_key
                && provider_id == "claude"
                && source_path == "/tmp/session.jsonl"
        ));

        app.on_key(key(KeyCode::Left), &data);
        assert_eq!(app.focus, Focus::Content);
        assert_eq!(app.sessions.pane, SessionsPane::List);

        app.on_key(key(KeyCode::Left), &data);
        assert_eq!(app.focus, Focus::Nav);
        assert_eq!(app.sessions.pane, SessionsPane::List);
    }

    #[test]
    fn sessions_list_navigation_never_auto_loads_messages() {
        use crate::cli::tui::input::{ScrollDirection, WheelGestureId};

        let mut app = app_with_session_page();
        app.sessions.rows.push(session_meta(
            "claude",
            "session-2",
            "Session Two",
            "/tmp/project",
            "/tmp/session-2.jsonl",
            "claude --resume session-2",
        ));
        let data = UiData::default();

        let down = app.on_key(key(KeyCode::Down), &data);
        assert!(matches!(down, Action::None));
        assert_eq!(app.sessions.selected_idx, 1);
        assert!(app.sessions.detail_key.is_none());
        assert!(!app.sessions.messages_loading);

        app.sessions.selected_idx = 0;
        app.sessions.pagination.select(0);
        let page_down = app.on_key(key(KeyCode::PageDown), &data);
        assert!(matches!(page_down, Action::None));
        assert_eq!(app.sessions.selected_idx, 1);
        assert!(app.sessions.detail_key.is_none());

        app.sessions.selected_idx = 0;
        app.sessions.pagination.select(0);
        let wheel =
            app.on_sessions_wheel(ScrollDirection::Down, 1, WheelGestureId::from_raw(1), &data);
        assert!(matches!(wheel, Action::None));
        assert_eq!(app.sessions.selected_idx, 1);
        assert!(app.sessions.detail_key.is_none());
        assert!(!app.sessions.messages_loading);
    }

    #[test]
    fn sessions_tab_keys_follow_same_focus_chain_as_arrows() {
        let mut app = app_with_session_page();
        let data = UiData::default();

        app.focus = Focus::Nav;
        app.on_key(key(KeyCode::Tab), &data);
        assert_eq!(app.focus, Focus::Content);
        assert_eq!(app.sessions.pane, SessionsPane::List);

        let action = app.on_key(key(KeyCode::Tab), &data);
        assert_eq!(app.focus, Focus::Content);
        assert_eq!(app.sessions.pane, SessionsPane::Detail);
        assert!(matches!(action, Action::SessionMessagesLoad { .. }));

        app.on_key(key(KeyCode::BackTab), &data);
        assert_eq!(app.focus, Focus::Content);
        assert_eq!(app.sessions.pane, SessionsPane::List);

        let mut backtab = key(KeyCode::Tab);
        backtab.modifiers = KeyModifiers::SHIFT;
        app.on_key(backtab, &data);
        assert_eq!(app.focus, Focus::Nav);
    }

    #[test]
    fn sessions_h_l_follow_arrow_focus_chain() {
        let mut app = app_with_session_page();
        let data = UiData::default();

        let action = app.on_key(key(KeyCode::Char('l')), &data);
        assert_eq!(app.sessions.pane, SessionsPane::Detail);
        assert!(matches!(action, Action::SessionMessagesLoad { .. }));

        app.on_key(key(KeyCode::Char('h')), &data);
        assert_eq!(app.sessions.pane, SessionsPane::List);

        app.on_key(key(KeyCode::Char('h')), &data);
        assert_eq!(app.focus, Focus::Nav);
    }

    #[test]
    fn sessions_resume_shortcut_returns_resume_action() {
        let mut app = app_with_session_page();
        let action = app.on_key(key(KeyCode::Char('R')), &UiData::default());

        assert!(matches!(
            action,
            Action::SessionResume { command, cwd }
                if command == "claude --resume session-1"
                    && cwd.as_deref() == Some("/tmp/project")
        ));
    }

    #[test]
    fn sessions_delete_shortcut_opens_confirm_overlay() {
        let mut app = app_with_session_page();
        let action = app.on_key(key(KeyCode::Char('d')), &UiData::default());

        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::SessionDelete {
                    ref provider_id,
                    ref session_id,
                    ref source_path,
                    ..
                },
                ..
            }) if provider_id == "claude"
                && session_id == "session-1"
                && source_path == "/tmp/session.jsonl"
        ));
    }

    #[test]
    fn sessions_list_shortcuts_follow_highlight_not_previous_detail() {
        let mut app = app_with_session_page();
        let data = UiData::default();
        let alpha = session_meta(
            "claude",
            "alpha",
            "Alpha Plan",
            "/tmp/alpha",
            "/tmp/alpha.jsonl",
            "claude --resume alpha",
        );
        let beta = session_meta(
            "claude",
            "beta",
            "Beta Plan",
            "/tmp/beta",
            "/tmp/beta.jsonl",
            "claude --resume beta",
        );
        app.sessions.rows = vec![alpha.clone(), beta];
        app.sessions.open_detail(session_key(&alpha));
        app.sessions.pane = SessionsPane::List;
        app.sessions.selected_idx = 1;

        let action = app.on_key(key(KeyCode::Char('R')), &data);

        assert!(matches!(
            action,
            Action::SessionResume { command, cwd }
                if command == "claude --resume beta"
                    && cwd.as_deref() == Some("/tmp/beta")
        ));
    }

    #[test]
    fn sessions_filter_targets_messages_from_detail_pane() {
        let mut app = app_with_session_page();
        let data = UiData::default();
        app.sessions.pane = SessionsPane::Detail;

        app.on_key(key(KeyCode::Char('/')), &data);

        assert!(app.filter.active);
        assert_eq!(app.filter.scope, FilterScope::SessionMessages);
        assert_eq!(app.sessions.pane, SessionsPane::Detail);
    }

    #[test]
    fn sessions_filter_matches_resume_command_source_and_date() {
        let mut app = app_with_session_page();

        app.filter.input.set("claude --resume");
        let visible = visible_sessions(&app.filter, &app.app_type, &app.sessions.rows);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].session_id, "session-1");

        app.filter.input.set("session.jsonl");
        let visible = visible_sessions(&app.filter, &app.app_type, &app.sessions.rows);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].session_id, "session-1");

        app.filter.input.set("2025");
        let visible = visible_sessions(&app.filter, &app.app_type, &app.sessions.rows);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].session_id, "session-1");
    }

    #[test]
    fn sessions_filter_change_clamps_selection_and_clears_stale_detail() {
        let mut app = app_with_session_page();
        let data = UiData::default();
        let alpha = session_meta(
            "claude",
            "alpha",
            "Alpha Plan",
            "/tmp/alpha",
            "/tmp/alpha.jsonl",
            "claude --resume alpha",
        );
        let beta = session_meta(
            "claude",
            "beta",
            "Beta Plan",
            "/tmp/beta",
            "/tmp/beta.jsonl",
            "claude --resume beta",
        );
        app.sessions.rows = vec![alpha.clone(), beta];
        app.sessions.selected_idx = 1;
        app.sessions.open_detail(session_key(&alpha));
        app.sessions.messages_loaded = true;
        app.sessions.messages = vec![crate::session_manager::SessionMessage {
            role: "assistant".to_string(),
            content: "old detail".to_string(),
            ts: Some(1_735_689_900_000),
        }];
        app.sessions.pane = SessionsPane::List;

        app.on_key(key(KeyCode::Char('/')), &data);
        app.on_key(key(KeyCode::Char('b')), &data);

        assert_eq!(app.filter.input.value, "b");
        assert_eq!(app.sessions.pane, SessionsPane::List);
        assert_eq!(app.sessions.selected_idx, 0);
        assert!(app.sessions.detail_key.is_none());
        assert!(app.sessions.messages.is_empty());
        assert!(!app.sessions.messages_loaded);
    }

    #[test]
    fn sessions_filter_change_preserves_inflight_delete() {
        let mut app = app_with_session_page();
        let data = UiData::default();
        let delete_request_id = app.sessions.start_delete();

        app.on_key(key(KeyCode::Char('/')), &data);
        app.on_key(key(KeyCode::Char('x')), &data);

        assert!(app.sessions.delete_active.contains(&delete_request_id));
    }

    #[test]
    fn sessions_query_edit_and_clear_request_worker_cancellation() {
        let mut app = app_with_session_page();
        let data = UiData::default();
        app.sessions.deep_search_query = Some("old".to_string());
        app.sessions.deep_search_active = Some(7);

        app.on_key(key(KeyCode::Char('/')), &data);
        let edit_action = app.on_key(key(KeyCode::Char('n')), &data);
        assert!(matches!(edit_action, Action::SessionsDeepSearchCancel));
        assert_eq!(
            app.sessions
                .deep_search_pending
                .as_ref()
                .map(|(q, _)| q.as_str()),
            Some("n")
        );

        let clear_action = app.on_key(key(KeyCode::Esc), &data);
        assert!(matches!(clear_action, Action::SessionsDeepSearchCancel));
        assert!(app.sessions.deep_search_pending.is_none());
        assert!(app.sessions.deep_search_active.is_none());
    }

    #[test]
    fn sessions_a_does_not_cancel_an_active_search() {
        let mut app = app_with_session_page();
        app.filter.input.set("needle");
        app.sessions.deep_search_active = Some(7);

        let action = app.on_key(key(KeyCode::Char('a')), &UiData::default());

        assert!(matches!(action, Action::None));
        assert_eq!(app.sessions.deep_search_active, Some(7));
    }

    #[test]
    fn sessions_delete_tracking_allows_out_of_order_completion() {
        let mut app = app_with_session_page();
        let alpha = session_meta(
            "claude",
            "alpha",
            "Alpha Plan",
            "/tmp/alpha",
            "/tmp/alpha.jsonl",
            "claude --resume alpha",
        );
        let beta = session_meta(
            "claude",
            "beta",
            "Beta Plan",
            "/tmp/beta",
            "/tmp/beta.jsonl",
            "claude --resume beta",
        );
        app.sessions.rows = vec![alpha.clone(), beta.clone()];
        let alpha_key = session_key(&alpha);
        let beta_key = session_key(&beta);
        let alpha_request_id = app.sessions.start_delete();
        let beta_request_id = app.sessions.start_delete();

        assert!(app.sessions.finish_delete(beta_request_id, &beta_key));
        assert_eq!(app.sessions.rows.len(), 1);
        assert_eq!(app.sessions.rows[0].session_id, "alpha");
        assert!(app.sessions.finish_delete(alpha_request_id, &alpha_key));
        assert!(app.sessions.rows.is_empty());
        assert!(app.sessions.delete_active.is_empty());
    }

    #[test]
    fn usage_shortcuts_change_range_and_open_details() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Usage;
        app.focus = Focus::Content;
        let data = UiData::default();

        app.on_key(key(KeyCode::Char('1')), &data);
        assert!(matches!(app.usage.range, data::UsageRangePreset::Today));

        app.on_key(key(KeyCode::Char('3')), &data);
        assert!(matches!(
            app.usage.range,
            data::UsageRangePreset::ThirtyDays
        ));

        app.on_key(key(KeyCode::Char('m')), &data);
        assert!(matches!(app.usage.metric, UsageMetric::Cost));

        app.on_key(key(KeyCode::Tab), &data);
        assert!(matches!(app.usage.metric, UsageMetric::Tokens));

        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT), &data);
        assert!(matches!(app.usage.metric, UsageMetric::Cost));

        app.on_key(key(KeyCode::Right), &data);
        assert!(matches!(app.usage.pane, UsagePane::Models));
    }

    #[test]
    fn usage_refresh_shortcut_uses_the_usage_worker_on_usage_and_pricing_routes() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Usage;
        app.focus = Focus::Content;
        let data = UiData::default();

        let action = app.on_key(key(KeyCode::Char('r')), &data);
        assert!(matches!(action, Action::UsageRefresh));

        app.route = Route::UsageLogs;
        let action = app.on_key(key(KeyCode::Char('r')), &data);
        assert!(matches!(action, Action::UsageRefresh));

        app.route = Route::Pricing;
        let action = app.on_key(key(KeyCode::Char('r')), &data);
        assert!(matches!(action, Action::UsageRefresh));
    }

    #[test]
    fn usage_custom_range_shortcut_opens_text_input() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Usage;
        app.focus = Focus::Content;

        let action = app.on_key(key(KeyCode::Char('C')), &UiData::default());

        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::UsageCustomRange,
                input,
                ..
            }) if input.value.contains("..")
        ));
    }

    #[test]
    fn usage_range_switch_clears_the_previous_log_context() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Usage;
        app.focus = Focus::Content;
        let mut data = UiData::default();
        data.usage.logs_total = 1;
        data.usage.recent_logs.push(data::UsageLogRow {
            request_id: "old-detail".to_string(),
            ..data::UsageLogRow::default()
        });
        app.usage.sync_log_pager(
            &AppType::Claude,
            data::UsageRangePreset::SevenDays,
            &data.usage.recent_logs,
            data.usage.logs_total,
        );
        app.usage.remember_log_detail(
            AppType::Claude,
            data::UsageRangePreset::SevenDays,
            data.usage.recent_logs[0].clone(),
        );

        app.on_key(key(KeyCode::Char('1')), &data);

        assert_eq!(app.usage.range, data::UsageRangePreset::Today);
        assert!(app.usage.log_detail_snapshot.is_none());
        assert!(app.usage.log_pager.gate.is_empty());
    }

    #[test]
    fn usage_custom_range_submit_emits_action() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Usage;
        app.focus = Focus::Content;
        app.overlay = Overlay::TextInput(TextInputState {
            title: "Custom Range".to_string(),
            prompt: "Format: YYYY-MM-DD..YYYY-MM-DD".to_string(),
            input: TextInput::new("2026-06-01..2026-06-05"),
            submit: TextSubmit::UsageCustomRange,
        });

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());

        assert!(matches!(
            action,
            Action::UsageCustomRange { range } if range.label() == "2026-06-01..2026-06-05"
        ));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn usage_custom_range_invalid_submit_reopens_input() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Usage;
        app.focus = Focus::Content;
        app.overlay = Overlay::TextInput(TextInputState {
            title: "Custom Range".to_string(),
            prompt: "Format: YYYY-MM-DD..YYYY-MM-DD".to_string(),
            input: TextInput::new("2026-06-05..2026-06-01"),
            submit: TextSubmit::UsageCustomRange,
        });

        let action = app.on_key(key(KeyCode::Enter), &UiData::default());

        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::UsageCustomRange,
                input,
                ..
            }) if input.value == "2026-06-05..2026-06-01"
        ));
        assert!(matches!(
            app.toast.as_ref().map(|toast| &toast.kind),
            Some(ToastKind::Warning)
        ));
    }

    #[test]
    fn usage_details_tabs_and_log_detail_route_use_stack() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Usage;
        app.focus = Focus::Content;
        let mut data = UiData::default();
        data.usage.recent_logs.push(data::UsageLogRow {
            request_id: "req-usage-1".to_string(),
            cursor_rowid: 101,
            ..data::UsageLogRow::default()
        });

        let action = app.on_key(key(KeyCode::Char('L')), &data);
        assert!(matches!(action, Action::SwitchRoute(Route::UsageLogs)));
        assert!(matches!(app.route, Route::UsageLogs));
        assert_eq!(app.route_stack, vec![Route::Usage]);
        assert!(matches!(app.usage.pane, UsagePane::Models));

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));

        app.on_key(key(KeyCode::Tab), &data);
        assert!(matches!(app.usage.pane, UsagePane::Providers));

        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT), &data);
        assert!(matches!(app.usage.pane, UsagePane::Models));

        app.on_key(key(KeyCode::Tab), &data);
        app.on_key(key(KeyCode::Tab), &data);
        assert!(matches!(app.usage.pane, UsagePane::Recent));

        let action = app.on_key(key(KeyCode::Char('d')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(app.route, Route::UsageLogs));

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            action,
            Action::SwitchRoute(Route::UsageLogDetail { rowid }) if rowid == 101
        ));
        assert!(matches!(app.route, Route::UsageLogDetail { rowid: 101 }));
        assert_eq!(app.route_stack, vec![Route::Usage, Route::UsageLogs]);
        assert_eq!(
            app.usage
                .log_detail_snapshot
                .as_ref()
                .and_then(|snapshot| {
                    snapshot.row_for(&AppType::Claude, data::UsageRangePreset::SevenDays, 101)
                })
                .map(|row| row.request_id.as_str()),
            Some("req-usage-1")
        );
        let snapshot = app
            .usage
            .log_detail_snapshot
            .as_ref()
            .expect("detail snapshot should exist");
        assert!(snapshot
            .row_for(&AppType::Codex, data::UsageRangePreset::SevenDays, 101,)
            .is_none());
        assert!(snapshot
            .row_for(&AppType::Claude, data::UsageRangePreset::Today, 101,)
            .is_none());
    }

    #[test]
    fn usage_log_detail_refresh_targets_only_the_open_request() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::UsageLogDetail { rowid: 202 };
        app.focus = Focus::Content;

        assert!(matches!(
            app.on_key(key(KeyCode::Char('r')), &UiData::default()),
            Action::UsageLogDetailRefresh { rowid } if rowid == 202
        ));
    }

    #[test]
    fn usage_log_source_reset_keeps_gate_and_visible_selection_in_sync() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::UsageLogs;
        app.focus = Focus::Content;
        app.usage.pane = UsagePane::Recent;
        let mut data = UiData::default();
        data.usage.logs_total = 100;
        data.usage.recent_logs = (0..100)
            .map(|index| data::UsageLogRow {
                request_id: format!("row-{index}"),
                created_at: 1_000 - index,
                cursor_rowid: index as i64 + 1,
                ..data::UsageLogRow::default()
            })
            .collect();

        app.usage.sync_log_pager(
            &app.app_type,
            app.usage.range,
            &data.usage.recent_logs,
            data.usage.logs_total,
        );
        app.usage.log_pager.gate.select(57);
        app.usage.logs_idx = 57;

        // A reload may keep the same count and cursor while changing joined
        // provider metadata. Explicit invalidation must still reset both
        // selection representations together.
        app.usage.invalidate_log_pages();
        app.usage.sync_log_pager(
            &app.app_type,
            app.usage.range,
            &data.usage.recent_logs,
            data.usage.logs_total,
        );
        assert_eq!(app.usage.log_pager.gate.selected_index(), Some(0));
        assert_eq!(app.usage.logs_idx, 0);

        let action = app.on_usage_logs_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            action,
            Action::SwitchRoute(Route::UsageLogDetail { rowid }) if rowid == 1
        ));
    }

    #[test]
    fn usage_log_head_refresh_preserves_the_selected_request_anchor() {
        let mut usage = UsageState::default();
        let first_page = (0..100)
            .map(|index| data::UsageLogRow {
                request_id: format!("row-{index}"),
                created_at: 1_000 - index,
                cursor_rowid: index as i64 + 1,
                ..data::UsageLogRow::default()
            })
            .collect::<Vec<_>>();
        usage.sync_log_pager(
            &AppType::Claude,
            data::UsageRangePreset::SevenDays,
            &first_page,
            150,
        );
        usage.log_pager.gate.select(50);
        usage.logs_idx = 50;

        let mut refreshed = vec![data::UsageLogRow {
            request_id: "new-head".to_string(),
            created_at: 2_000,
            cursor_rowid: 2_000,
            ..data::UsageLogRow::default()
        }];
        refreshed.extend(first_page.into_iter().take(99));
        usage.sync_log_pager(
            &AppType::Claude,
            data::UsageRangePreset::SevenDays,
            &refreshed,
            151,
        );

        assert_eq!(usage.log_pager.gate.selected_index(), Some(51));
        assert_eq!(usage.logs_idx, 51);
        assert_eq!(
            usage.log_pager.current_rows(&refreshed)[usage.logs_idx].request_id,
            "row-50"
        );
    }

    #[test]
    fn usage_log_head_refresh_detects_content_changes_with_the_same_cursor_and_count() {
        let mut usage = UsageState::default();
        let first_page = vec![data::UsageLogRow {
            request_id: "same-request".to_string(),
            created_at: 1_000,
            provider_name: Some("Old provider".to_string()),
            total_cost_usd: 0.25,
            error_message: Some("old error".to_string()),
            ..data::UsageLogRow::default()
        }];
        usage.sync_log_pager(
            &AppType::Claude,
            data::UsageRangePreset::SevenDays,
            &first_page,
            1,
        );

        let mut refreshed = first_page.clone();
        refreshed[0].provider_name = Some("New provider".to_string());
        refreshed[0].total_cost_usd = 1.75;
        refreshed[0].error_message = Some("new error".to_string());
        usage.sync_log_pager(
            &AppType::Claude,
            data::UsageRangePreset::SevenDays,
            &refreshed,
            1,
        );

        let row = &usage.log_pager.current_rows(&refreshed)[0];
        assert_eq!(row.provider_name.as_deref(), Some("New provider"));
        assert_eq!(row.total_cost_usd.to_bits(), 1.75_f64.to_bits());
        assert_eq!(row.error_message.as_deref(), Some("new error"));
    }

    #[test]
    fn usage_log_head_fingerprint_ignores_unbounded_string_suffixes() {
        let mut pager = UsageLogPager::default();
        let prefix = "x".repeat(128);
        let first_page = vec![data::UsageLogRow {
            request_id: "same-request".to_string(),
            created_at: 1_000,
            cursor_rowid: 1,
            error_message: Some(format!("{prefix}{}", "a".repeat(100_000))),
            error_message_truncated: true,
            ..data::UsageLogRow::default()
        }];
        assert!(pager.sync_source(
            &AppType::Claude,
            data::UsageRangePreset::SevenDays,
            &first_page,
            1,
        ));

        let mut suffix_only_change = first_page.clone();
        suffix_only_change[0].error_message = Some(format!("{prefix}{}", "b".repeat(100_000)));
        assert!(!pager.sync_source(
            &AppType::Claude,
            data::UsageRangePreset::SevenDays,
            &suffix_only_change,
            1,
        ));

        suffix_only_change[0].error_message = Some(format!("y{}", "b".repeat(100_127)));
        assert!(pager.sync_source(
            &AppType::Claude,
            data::UsageRangePreset::SevenDays,
            &suffix_only_change,
            1,
        ));
    }

    #[test]
    fn usage_log_head_refresh_keeps_a_later_page_snapshot_and_detail() {
        let mut usage = UsageState::default();
        let first_page = (0..100)
            .map(|index| data::UsageLogRow {
                request_id: format!("head-{index}"),
                created_at: 1_000 - index,
                ..data::UsageLogRow::default()
            })
            .collect::<Vec<_>>();
        usage.sync_log_pager(
            &AppType::Claude,
            data::UsageRangePreset::SevenDays,
            &first_page,
            200,
        );
        let later_row = data::UsageLogRow {
            request_id: "later-detail".to_string(),
            created_at: 500,
            model: "old-model".to_string(),
            cursor_rowid: 500,
            ..data::UsageLogRow::default()
        };
        assert!(usage
            .log_pager
            .start_request(1, 7, data::UsageLogPageDirection::Older,));
        assert!(usage.log_pager.finish_request(
            1,
            7,
            data::UsageLogPageDirection::Older,
            data::UsageLogPage {
                rows: vec![later_row.clone()],
                has_more: true,
                next_cursor: Some(data::UsageLogCursor::from_row(&later_row)),
                ..data::UsageLogPage::default()
            },
        ));
        usage.log_pager.gate.select(100);
        usage.remember_log_detail(
            AppType::Claude,
            data::UsageRangePreset::SevenDays,
            later_row,
        );

        let mut refreshed_head = first_page.clone();
        refreshed_head[0].model = "changed-head".to_string();
        usage.sync_log_pager(
            &AppType::Claude,
            data::UsageRangePreset::SevenDays,
            &refreshed_head,
            200,
        );

        assert_eq!(usage.log_pager.current_page(), 1);
        assert_eq!(
            usage.log_pager.current_rows(&refreshed_head)[0].request_id,
            "later-detail"
        );
        assert_eq!(
            usage
                .log_detail_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.row_for(
                    &AppType::Claude,
                    data::UsageRangePreset::SevenDays,
                    500,
                ))
                .map(|row| row.model.as_str()),
            Some("old-model")
        );
    }

    #[test]
    fn usage_logs_wait_for_prefetched_page_and_a_new_wheel_gesture() {
        use crate::cli::tui::app::paged_list::{PageBoundary, PagedListFocus};
        use crate::cli::tui::input::{ScrollDirection, WheelGestureId};

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::UsageLogs;
        app.focus = Focus::Content;
        app.usage.pane = UsagePane::Recent;
        let mut data = UiData::default();
        data.usage.logs_total = 205;
        data.usage.recent_logs = (0..100)
            .map(|index| data::UsageLogRow {
                request_id: format!("p0-{index}"),
                created_at: 1_000 - index,
                ..data::UsageLogRow::default()
            })
            .collect();

        let first = WheelGestureId::from_raw(1);
        app.on_usage_logs_wheel(ScrollDirection::Down, 5_000, first, &data);
        assert_eq!(app.usage.logs_idx, 99);
        assert_eq!(
            app.usage.log_pager.gate.focus(),
            PagedListFocus::Boundary(PageBoundary::Next)
        );

        // A fresh gesture cannot expose a page that has not completed loading.
        let waiting = WheelGestureId::from_raw(2);
        app.on_usage_logs_wheel(ScrollDirection::Down, 5_000, waiting, &data);
        assert_eq!(app.usage.log_pager.current_page(), 0);

        let page_rows = (0..100)
            .map(|index| data::UsageLogRow {
                request_id: format!("p1-{index}"),
                created_at: 899 - index,
                ..data::UsageLogRow::default()
            })
            .collect::<Vec<_>>();
        let next_cursor = page_rows.last().map(data::UsageLogCursor::from_row);
        assert!(app
            .usage
            .log_pager
            .start_request(1, 7, data::UsageLogPageDirection::Older,));
        assert!(app.usage.log_pager.finish_request(
            1,
            7,
            data::UsageLogPageDirection::Older,
            data::UsageLogPage {
                rows: page_rows,
                next_cursor,
                has_more: true,
                ..data::UsageLogPage::default()
            },
        ));

        // If prefetch completes while the waiting gesture still has queued
        // reports, those reports cannot consume the newly ready page.
        app.on_usage_logs_wheel(ScrollDirection::Down, 5_000, waiting, &data);
        assert_eq!(app.usage.log_pager.current_page(), 0);

        let crossing = WheelGestureId::from_raw(3);
        app.on_usage_logs_wheel(ScrollDirection::Down, 5_000, crossing, &data);
        assert_eq!(app.usage.log_pager.current_page(), 1);
        assert_eq!(app.usage.logs_idx, 0);
        app.on_usage_logs_wheel(ScrollDirection::Down, 5_000, crossing, &data);
        assert_eq!(app.usage.logs_idx, 0);

        let action = app.on_usage_logs_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            action,
            Action::SwitchRoute(Route::UsageLogDetail { rowid }) if rowid == 0
        ));
        app.usage.invalidate_log_pages();
        app.usage.sync_log_pager(
            &app.app_type,
            app.usage.range,
            &data.usage.recent_logs,
            data.usage.logs_total,
        );
        assert!(app.usage.log_detail_snapshot.is_none());
    }

    #[test]
    fn usage_logs_failed_boundary_needs_one_gesture_to_retry_and_another_to_cross() {
        use crate::cli::tui::input::{ScrollDirection, WheelGestureId};

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::UsageLogs;
        app.focus = Focus::Content;
        app.usage.pane = UsagePane::Recent;
        let mut data = UiData::default();
        data.usage.logs_total = 200;
        data.usage.recent_logs = (0..100)
            .map(|index| data::UsageLogRow {
                request_id: format!("p0-{index}"),
                created_at: 1_000 - index,
                cursor_rowid: index as i64 + 1,
                ..data::UsageLogRow::default()
            })
            .collect();

        app.on_usage_logs_wheel(
            ScrollDirection::Down,
            5_000,
            WheelGestureId::from_raw(1),
            &data,
        );
        assert_eq!(app.usage.log_pager.current_page(), 0);

        assert!(app
            .usage
            .log_pager
            .start_request(1, 7, data::UsageLogPageDirection::Older,));
        let waiting = WheelGestureId::from_raw(2);
        app.on_usage_logs_wheel(ScrollDirection::Down, 5_000, waiting, &data);
        assert!(app.usage.log_pager.fail_request(
            1,
            7,
            data::UsageLogPageDirection::Older,
            "offline".to_string(),
        ));

        // More reports from the gesture that first hit the pending boundary do
        // not retry it and do not cross it.
        app.on_usage_logs_wheel(ScrollDirection::Down, 5_000, waiting, &data);
        assert_eq!(app.usage.log_pager.page_error(1), Some("offline"));
        assert_eq!(app.usage.log_pager.current_page(), 0);

        // A new gesture clears the error so the event loop can queue one retry,
        // but that gesture remains blocked at the boundary.
        let retry = WheelGestureId::from_raw(3);
        app.on_usage_logs_wheel(ScrollDirection::Down, 5_000, retry, &data);
        assert!(app.usage.log_pager.page_error(1).is_none());
        assert_eq!(app.usage.log_pager.current_page(), 0);

        let page_rows = (0..100)
            .map(|index| data::UsageLogRow {
                request_id: format!("p1-{index}"),
                created_at: 899 - index,
                cursor_rowid: index as i64 + 101,
                ..data::UsageLogRow::default()
            })
            .collect::<Vec<_>>();
        assert!(app
            .usage
            .log_pager
            .start_request(1, 8, data::UsageLogPageDirection::Older,));
        assert!(app.usage.log_pager.finish_request(
            1,
            8,
            data::UsageLogPageDirection::Older,
            data::UsageLogPage {
                next_cursor: page_rows.last().map(data::UsageLogCursor::from_row),
                rows: page_rows,
                has_more: false,
                ..data::UsageLogPage::default()
            },
        ));
        app.on_usage_logs_wheel(ScrollDirection::Down, 5_000, retry, &data);
        assert_eq!(app.usage.log_pager.current_page(), 0);

        app.on_usage_logs_wheel(
            ScrollDirection::Down,
            5_000,
            WheelGestureId::from_raw(4),
            &data,
        );
        assert_eq!(app.usage.log_pager.current_page(), 1);
    }

    #[test]
    fn usage_log_pager_extends_an_underestimated_total_and_converges_at_the_end() {
        let mut pager = UsageLogPager::default();
        let first_page = (0..100)
            .map(|index| data::UsageLogRow {
                request_id: format!("p0-{index}"),
                created_at: 1_000 - index,
                ..data::UsageLogRow::default()
            })
            .collect::<Vec<_>>();
        pager.sync_source(
            &AppType::Claude,
            data::UsageRangePreset::SevenDays,
            &first_page,
            150,
        );

        let second_page = (0..100)
            .map(|index| data::UsageLogRow {
                request_id: format!("p1-{index}"),
                created_at: 899 - index,
                ..data::UsageLogRow::default()
            })
            .collect::<Vec<_>>();
        let second_cursor = second_page.last().map(data::UsageLogCursor::from_row);
        assert!(pager.start_request(1, 7, data::UsageLogPageDirection::Older,));
        assert!(pager.finish_request(
            1,
            7,
            data::UsageLogPageDirection::Older,
            data::UsageLogPage {
                rows: second_page,
                next_cursor: second_cursor,
                has_more: true,
                ..data::UsageLogPage::default()
            },
        ));

        assert_eq!(pager.gate.len(), 201);
        assert_eq!(pager.gate.page_count(), 3);
        // Cursor requests are deliberately adjacent-only: the pager keeps no
        // O(page-count) cursor index for very large histories. Model the UI
        // crossing onto the loaded second page before requesting its successor.
        let _ = pager.gate.select(100);
        assert!(pager.page_request(2).is_some());

        let final_page = (0..20)
            .map(|index| data::UsageLogRow {
                request_id: format!("p2-{index}"),
                created_at: 798 - index,
                ..data::UsageLogRow::default()
            })
            .collect::<Vec<_>>();
        assert!(pager.start_request(2, 8, data::UsageLogPageDirection::Older,));
        assert!(pager.finish_request(
            2,
            8,
            data::UsageLogPageDirection::Older,
            data::UsageLogPage {
                rows: final_page,
                next_cursor: None,
                has_more: false,
                ..data::UsageLogPage::default()
            },
        ));
        assert_eq!(pager.gate.len(), 220);
        assert_eq!(pager.gate.page_count(), 3);
    }

    #[test]
    fn usage_log_pager_refreshes_the_current_eviction_safe_anchor_in_place() {
        let mut pager = UsageLogPager::default();
        let rows_for_page = |page: usize, label: &str| {
            (0..100)
                .map(|index| {
                    let ordinal = page.saturating_mul(100).saturating_add(index);
                    data::UsageLogRow {
                        request_id: format!("{label}-{index}"),
                        created_at: 10_000 - ordinal as i64,
                        cursor_rowid: ordinal as i64 + 1,
                        ..data::UsageLogRow::default()
                    }
                })
                .collect::<Vec<_>>()
        };
        let first_page = rows_for_page(0, "p0");
        pager.sync_source(
            &AppType::Claude,
            data::UsageRangePreset::SevenDays,
            &first_page,
            500,
        );

        for page in 1..=3 {
            let load = pager.page_request(page).expect("adjacent page request");
            assert!(pager.start_load_request(page, page as u64, load));
            let rows = rows_for_page(page, &format!("p{page}"));
            assert!(pager.finish_request(
                page,
                page as u64,
                load.direction,
                data::UsageLogPage {
                    next_cursor: rows.last().map(data::UsageLogCursor::from_row),
                    rows,
                    has_more: true,
                },
            ));
            if page < 3 {
                pager.gate.select(page.saturating_mul(100));
            }
        }

        // Loading page 3 evicts page 1 from the two-page cache. Page 2 can
        // still be refreshed because it retained its own original keyset anchor.
        pager.gate.select(200);
        let refresh = pager
            .current_page_refresh_request()
            .expect("current page should retain its load anchor");
        let old_request_id = pager.current_rows(&first_page)[0].request_id.clone();
        assert_eq!(old_request_id, "p2-0");
        assert!(pager.start_load_request(2, 40, refresh));
        assert!(!pager.start_load_request(2, 41, refresh));
        assert_eq!(pager.current_rows(&first_page)[0].request_id, "p2-0");

        let refreshed_rows = rows_for_page(2, "refreshed");
        assert!(pager.finish_request(
            2,
            40,
            refresh.direction,
            data::UsageLogPage {
                next_cursor: refreshed_rows.last().map(data::UsageLogCursor::from_row),
                rows: refreshed_rows,
                has_more: true,
            },
        ));
        assert_eq!(pager.current_page(), 2);
        assert_eq!(pager.gate.selected_index(), Some(200));
        assert!(!pager.page_is_available(3));
        assert_eq!(pager.current_rows(&first_page)[0].request_id, "refreshed-0");
    }

    #[test]
    fn usage_log_pager_discards_a_refresh_result_after_cached_page_navigation() {
        let rows_for_page = |page: usize| {
            (0..100)
                .map(|index| {
                    let ordinal = page.saturating_mul(100).saturating_add(index);
                    data::UsageLogRow {
                        request_id: format!("p{page}-{index}"),
                        created_at: 10_000 - ordinal as i64,
                        cursor_rowid: ordinal as i64 + 1,
                        ..data::UsageLogRow::default()
                    }
                })
                .collect::<Vec<_>>()
        };
        let first_page = rows_for_page(0);
        let mut pager = UsageLogPager::default();
        pager.sync_source(
            &AppType::Claude,
            data::UsageRangePreset::SevenDays,
            &first_page,
            300,
        );

        for page in 1..=2 {
            let load = pager.page_request(page).expect("adjacent page request");
            assert!(pager.start_load_request(page, page as u64, load));
            let rows = rows_for_page(page);
            assert!(pager.finish_request(
                page,
                page as u64,
                load.direction,
                data::UsageLogPage {
                    next_cursor: rows.last().map(data::UsageLogCursor::from_row),
                    rows,
                    has_more: page < 2,
                },
            ));
            pager.gate.select(page.saturating_mul(100));
        }

        let refresh = pager
            .current_page_refresh_request()
            .expect("current page should be refreshable");
        assert!(pager.start_load_request(2, 50, refresh));
        assert!(pager.has_refresh_pending());

        // Page 1 is still cached and remains interactive while the page 2
        // query runs. Its snapshot must survive the late page 2 result.
        pager.gate.select(100);
        let refreshed_rows = (0..20)
            .map(|index| data::UsageLogRow {
                request_id: format!("new-p2-{index}"),
                ..data::UsageLogRow::default()
            })
            .collect();
        assert!(pager.finish_request(
            2,
            50,
            refresh.direction,
            data::UsageLogPage {
                rows: refreshed_rows,
                has_more: false,
                ..data::UsageLogPage::default()
            },
        ));

        assert!(!pager.has_refresh_pending());
        assert_eq!(pager.current_page(), 1);
        assert_eq!(pager.current_rows(&first_page)[0].request_id, "p1-0");
        assert!(pager.page_is_available(2));
        pager.gate.select(200);
        assert_eq!(pager.current_rows(&first_page)[0].request_id, "p2-0");
    }

    #[test]
    fn usage_log_pager_keeps_page_pending_and_error_state_bounded() {
        let mut pager = UsageLogPager::default();
        let first_page = (0..100)
            .map(|index| data::UsageLogRow {
                request_id: format!("p0-{index}"),
                created_at: 10_000 - index,
                cursor_rowid: index as i64 + 1,
                ..data::UsageLogRow::default()
            })
            .collect::<Vec<_>>();
        pager.sync_source(
            &AppType::Claude,
            data::UsageRangePreset::SevenDays,
            &first_page,
            10_000,
        );

        for page in 1..50 {
            let request = pager.page_request(page).expect("adjacent page request");
            assert_eq!(request.direction, data::UsageLogPageDirection::Older);
            let request_id = page as u64;
            assert!(pager.start_request(page, request_id, request.direction));
            let rows = (0..100)
                .map(|index| {
                    let ordinal = page.saturating_mul(100).saturating_add(index);
                    data::UsageLogRow {
                        request_id: format!("p{page}-{index}"),
                        created_at: 10_000 - ordinal as i64,
                        cursor_rowid: ordinal as i64 + 1,
                        ..data::UsageLogRow::default()
                    }
                })
                .collect::<Vec<_>>();
            assert!(pager.finish_request(
                page,
                request_id,
                request.direction,
                data::UsageLogPage {
                    next_cursor: rows.last().map(data::UsageLogCursor::from_row),
                    rows,
                    has_more: true,
                    ..data::UsageLogPage::default()
                },
            ));
            pager.gate.select(page.saturating_mul(100));

            let (pages, lru, pending, errors) = pager.cache_metrics();
            assert!(pages <= 2, "cached {pages} pages");
            assert!(lru <= 2, "tracked {lru} LRU entries");
            assert!(pending <= 2, "tracked {pending} pending pages");
            assert!(errors <= 5, "tracked {errors} page errors");
            assert!(
                pager.retained_row_count(first_page.len(), true) <= 500,
                "usage pager retained too many physical row copies"
            );
        }

        assert!(pager.start_request(60, 60, data::UsageLogPageDirection::Older));
        assert!(pager.start_request(61, 61, data::UsageLogPageDirection::Older));
        assert!(!pager.start_request(62, 62, data::UsageLogPageDirection::Older));
        assert_eq!(pager.cache_metrics().2, 2);
        assert!(pager.fail_request(
            60,
            60,
            data::UsageLogPageDirection::Older,
            "first".to_string(),
        ));
        assert!(pager.fail_request(
            61,
            61,
            data::UsageLogPageDirection::Older,
            "second".to_string(),
        ));
        for page in 62..72 {
            assert!(pager.start_request(page, page as u64, data::UsageLogPageDirection::Older,));
            assert!(pager.fail_request(
                page,
                page as u64,
                data::UsageLogPageDirection::Older,
                format!("error-{page}"),
            ));
        }
        assert!(pager.cache_metrics().3 <= 5);
    }

    #[test]
    fn usage_log_prefetch_does_not_churn_an_evicted_previous_page() {
        use crate::cli::tui::app::paged_list::{PageDirection, PagedListOutcome};

        let mut pager = UsageLogPager::default();
        let first_page = (0..100)
            .map(|index| data::UsageLogRow {
                created_at: 1_000 - index,
                cursor_rowid: index + 1,
                ..data::UsageLogRow::default()
            })
            .collect::<Vec<_>>();
        pager.sync_source(
            &AppType::Claude,
            data::UsageRangePreset::SevenDays,
            &first_page,
            1_000,
        );

        for page in 1..=3 {
            let request = pager.page_request(page).expect("adjacent page request");
            let rows = (0..100)
                .map(|index| {
                    let ordinal = page * 100 + index;
                    data::UsageLogRow {
                        created_at: 1_000 - ordinal as i64,
                        cursor_rowid: ordinal as i64 + 1,
                        ..data::UsageLogRow::default()
                    }
                })
                .collect::<Vec<_>>();
            assert!(pager.start_request(page, page as u64, request.direction));
            assert!(pager.finish_request(
                page,
                page as u64,
                request.direction,
                data::UsageLogPage {
                    next_cursor: rows.last().map(data::UsageLogCursor::from_row),
                    rows,
                    has_more: true,
                    ..data::UsageLogPage::default()
                },
            ));
            if page < 3 {
                pager.gate.select(page * 100);
            }
        }

        // Page 1 was evicted while page 2 remains visible and page 3 is ready.
        // Idle prefetch must not alternate between pages 1 and 3 every tick.
        assert_eq!(pager.current_page(), 2);
        assert!(!pager.page_is_available(1));
        assert!(pager.page_is_available(3));
        assert_eq!(pager.preferred_prefetch_page(), None);

        assert!(matches!(
            pager.gate.line(PageDirection::Previous),
            PagedListOutcome::BoundaryFocused { .. }
        ));
        assert_eq!(pager.preferred_prefetch_page(), Some(1));
    }

    #[test]
    fn newer_usage_log_page_never_shrinks_the_known_total() {
        let mut pager = UsageLogPager::default();
        let first_page = vec![data::UsageLogRow {
            request_id: "head".to_string(),
            created_at: 1_000,
            cursor_rowid: 1,
            ..data::UsageLogRow::default()
        }];
        pager.sync_source(
            &AppType::Claude,
            data::UsageRangePreset::SevenDays,
            &first_page,
            500,
        );
        assert!(pager.start_request(1, 9, data::UsageLogPageDirection::Newer));
        assert!(pager.finish_request(
            1,
            9,
            data::UsageLogPageDirection::Newer,
            data::UsageLogPage {
                rows: vec![data::UsageLogRow {
                    request_id: "newer".to_string(),
                    created_at: 1_001,
                    cursor_rowid: 2,
                    ..data::UsageLogRow::default()
                }],
                next_cursor: None,
                has_more: false,
                ..data::UsageLogPage::default()
            },
        ));
        assert_eq!(pager.gate.len(), 500);
    }

    #[test]
    fn obsolete_usage_log_page_request_can_be_requeued_without_an_error() {
        let mut pager = UsageLogPager::default();
        let first_page = (0..100)
            .map(|index| data::UsageLogRow {
                request_id: format!("p0-{index}"),
                created_at: 1_000 - index,
                ..data::UsageLogRow::default()
            })
            .collect::<Vec<_>>();
        pager.sync_source(
            &AppType::Claude,
            data::UsageRangePreset::SevenDays,
            &first_page,
            205,
        );
        assert!(pager.start_request(1, 7, data::UsageLogPageDirection::Older,));

        assert!(pager.cancel_request(1, 7, data::UsageLogPageDirection::Older,));
        assert!(!pager.page_is_pending(1));
        assert!(pager.page_error(1).is_none());
        assert!(pager.page_request(1).is_some());
        assert!(!pager.cancel_request(1, 7, data::UsageLogPageDirection::Older,));
    }

    #[test]
    fn usage_pricing_shortcut_opens_pricing_as_child_route() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Usage;
        app.focus = Focus::Content;
        let mut data = UiData::default();
        data.pricing.rows = vec![data::ModelPricingRow {
            model_id: "claude-sonnet-4-5".to_string(),
            display_name: "Claude Sonnet 4.5".to_string(),
            ..data::ModelPricingRow::default()
        }];

        let action = app.on_key(key(KeyCode::Char('P')), &data);

        assert!(matches!(action, Action::SwitchRoute(Route::Pricing)));
        assert!(matches!(app.route, Route::Pricing));
        assert_eq!(app.route_stack, vec![Route::Usage]);
        assert!(matches!(app.nav_item(), NavItem::Usage));

        let action = app.on_key(key(KeyCode::Esc), &data);

        assert!(matches!(action, Action::SwitchRoute(Route::Usage)));
        assert!(matches!(app.route, Route::Usage));
        assert!(app.route_stack.is_empty());
    }

    #[test]
    fn pricing_shortcuts_select_edit_and_delete() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Pricing;
        app.focus = Focus::Content;
        let mut data = UiData::default();
        data.pricing.rows = vec![
            data::ModelPricingRow {
                model_id: "gpt-5.4".to_string(),
                display_name: "GPT 5.4".to_string(),
                ..data::ModelPricingRow::default()
            },
            data::ModelPricingRow {
                model_id: "claude-sonnet-4-5".to_string(),
                display_name: "Claude Sonnet 4.5".to_string(),
                ..data::ModelPricingRow::default()
            },
        ];

        app.on_key(key(KeyCode::Down), &data);
        assert_eq!(app.pricing.selected_idx, 1);

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        let Some(editor) = app.editor.as_ref() else {
            panic!("expected pricing editor");
        };
        assert!(matches!(
            editor.submit,
            EditorSubmit::PricingEdit { ref model_id } if model_id == "claude-sonnet-4-5"
        ));
        assert!(editor.text().contains("\"input_cost_per_million\""));
        assert!(matches!(app.route, Route::Pricing));
        assert!(app.route_stack.is_empty());

        app.editor = None;
        let action = app.on_key(key(KeyCode::Char('e')), &data);
        assert!(matches!(action, Action::None));
        assert!(
            app.editor.is_none(),
            "pricing e shortcut should be disabled"
        );

        let action = app.on_key(key(KeyCode::Char('d')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::PricingDelete { ref model_id },
                ..
            }) if model_id == "claude-sonnet-4-5"
        ));
    }
}
