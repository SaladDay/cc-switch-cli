use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};
use serde_json::json;
use std::sync::Mutex;
use unicode_width::UnicodeWidthStr;

use crate::{
    app_config::AppType,
    cli::i18n::texts,
    cli::tui::{
        app,
        app::{
            App, ConfirmAction, ConfirmOverlay, EditorKind, EditorSubmit, Focus, Overlay,
            TextInputState, TextSubmit,
        },
        data::{
            ConfigSnapshot, McpSnapshot, PromptsSnapshot, ProviderRow, ProvidersSnapshot,
            ProxySnapshot, SkillsSnapshot, UiData,
        },
        form::{FormFocus, ProviderAddField},
        route::Route,
        theme::theme_for,
    },
    provider::Provider,
    services::skill::{InstalledSkill, SkillApps, SkillRepo, SyncMethod, UnmanagedSkill},
};

#[test]
fn mask_api_key_handles_multibyte_safely() {
    let short = "你你你"; // 3 chars, 9 bytes
    let masked = super::mask_api_key(short);
    assert_eq!(masked, short);

    let long = "你".repeat(9);
    let masked = super::mask_api_key(&long);
    assert!(masked.ends_with("..."));
}

#[test]
fn provider_form_shows_full_api_key_in_table_value() {
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.claude_api_key.set("sk-test-1234567890");

    let (_label, value) = super::provider_field_label_and_value(
        &form,
        crate::cli::tui::form::ProviderAddField::ClaudeApiKey,
    );
    assert_eq!(value, "sk-test-1234567890");
}

#[test]
fn provider_field_label_and_value_renders_claude_api_format() {
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.claude_api_format = crate::cli::tui::form::ClaudeApiFormat::OpenAiChat;

    let (label, value) = super::provider_field_label_and_value(
        &form,
        crate::cli::tui::form::ProviderAddField::ClaudeApiFormat,
    );
    assert!(label.contains("API"));
    assert!(value.contains("OpenAI Chat Completions"));
    assert!(value.contains("代理") || value.contains("proxy"));
}

#[test]
fn provider_field_label_and_value_renders_claude_responses_api_format() {
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.claude_api_format = crate::cli::tui::form::ClaudeApiFormat::OpenAiResponses;

    let (_label, value) = super::provider_field_label_and_value(
        &form,
        crate::cli::tui::form::ProviderAddField::ClaudeApiFormat,
    );
    assert!(value.contains("OpenAI Responses API"));
    assert!(value.contains("代理") || value.contains("proxy"));
}

#[test]
fn provider_detail_uses_legacy_claude_api_format_for_display() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::ProviderDetail {
        id: "p1".to_string(),
    };
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows[0].provider = Provider::with_id(
        "p1".to_string(),
        "Demo Provider".to_string(),
        json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://example.com"
            },
            "api_format": "openai_chat"
        }),
        None,
    );

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains("OpenAI Chat Completions"));
}

#[test]
fn settings_local_proxy_row_shows_address_without_enabled_badge() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Settings;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.proxy.configured_listen_address = "127.0.0.1".to_string();
    data.proxy.configured_listen_port = 15722;
    data.proxy.enabled = true;

    let buf = render(&app, &data);
    let proxy_line = (0..buf.area.height)
        .map(|y| line_at(&buf, y))
        .find(|line| line.contains("Local Proxy"))
        .expect("settings view should render Local Proxy row");

    assert!(proxy_line.contains("127.0.0.1:15722"));
    assert!(!proxy_line.contains("Enabled"));
    assert!(!proxy_line.contains("Disabled"));
}

#[test]
fn settings_proxy_route_hides_edit_key_when_proxy_is_running() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::SettingsProxy;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.proxy.running = true;
    data.proxy.configured_listen_address = "127.0.0.1".to_string();
    data.proxy.configured_listen_port = 15722;

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(!all.contains("Enter Edit"));
    assert!(all.contains("Stop the local proxy before editing listen address or port"));
}

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    match ENV_LOCK.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, prev }
    }

    fn remove(key: &'static str) -> Self {
        let prev = std::env::var(key).ok();
        std::env::remove_var(key);
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.prev {
            None => std::env::remove_var(self.key),
            Some(v) => std::env::set_var(self.key, v),
        }
    }
}

fn render(app: &App, data: &UiData) -> Buffer {
    render_with_size(app, data, 120, 40)
}

fn render_with_size(app: &App, data: &UiData, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal created");
    terminal
        .draw(|f| super::render(f, app, data))
        .expect("draw ok");
    terminal.backend().buffer().clone()
}

fn line_at(buf: &Buffer, y: u16) -> String {
    let mut out = String::new();
    for x in 0..buf.area.width {
        out.push_str(buf[(x, y)].symbol());
    }
    out
}

fn all_text(buf: &Buffer) -> String {
    let mut all = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            all.push_str(buf[(x, y)].symbol());
        }
        all.push('\n');
    }
    all
}

fn minimal_data(_app_type: &AppType) -> UiData {
    let provider = Provider::with_id(
        "p1".to_string(),
        "Demo Provider".to_string(),
        json!({}),
        None,
    );
    UiData {
        providers: ProvidersSnapshot {
            current_id: "p0".to_string(),
            rows: vec![ProviderRow {
                id: "p1".to_string(),
                provider,
                api_url: Some("https://example.com".to_string()),
                is_current: false,
            }],
        },
        mcp: McpSnapshot::default(),
        prompts: PromptsSnapshot::default(),
        config: ConfigSnapshot::default(),
        skills: SkillsSnapshot::default(),
        proxy: ProxySnapshot::default(),
    }
}

fn installed_skill(directory: &str, name: &str) -> InstalledSkill {
    InstalledSkill {
        id: format!("local:{directory}"),
        name: name.to_string(),
        description: Some("Demo".to_string()),
        directory: directory.to_string(),
        readme_url: None,
        repo_owner: None,
        repo_name: None,
        repo_branch: None,
        apps: SkillApps {
            claude: true,
            codex: false,
            gemini: false,
            opencode: false,
        },
        installed_at: 1,
    }
}

#[test]
fn add_form_template_chips_are_single_row() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.form = Some(crate::cli::tui::form::FormState::ProviderAdd(
        crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude),
    ));

    let data = minimal_data(&app.app_type);
    let buf = render(&app, &data);

    let mut chips_y = None;
    for y in 0..buf.area.height {
        let line = line_at(&buf, y);
        if line.contains("Custom") && line.contains("Claude Official") {
            chips_y = Some(y);
            break;
        }
    }

    let chips_y = chips_y.expect("template chips row missing from add form");
    let next = line_at(&buf, chips_y + 1);
    assert!(
        next.contains('└'),
        "expected template block border after chips, got: {next}"
    );
}

#[test]
fn provider_form_fields_show_dashed_divider_before_common_snippet() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.form = Some(crate::cli::tui::form::FormState::ProviderAdd(
        crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude),
    ));

    let data = minimal_data(&app.app_type);
    let buf = render(&app, &data);

    // The label is clipped to the first column width; search for a stable substring.
    let common_label = "Snipp";
    let mut common_y = None;
    for y in 0..buf.area.height {
        let line = line_at(&buf, y);
        if line.contains(common_label) {
            common_y = Some(y);
            break;
        }
    }

    let common_y = common_y.expect("Common Config Snippet row missing from provider form");
    let above = line_at(&buf, common_y.saturating_sub(1));
    assert!(
        above.contains("┄┄┄"),
        "expected dashed divider row above common snippet, got: {above}"
    );
}

#[test]
fn header_is_wrapped_in_a_rect_block() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);

    // Header is at y=0..=2, and should have an outer border at (0,0).
    assert_eq!(buf[(0, 0)].symbol(), "┌");
}

#[test]
fn header_renders_proxy_chip_left_of_provider() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows[0].is_current = true;
    data.proxy.running = true;
    data.proxy.claude_takeover = true;

    let buf = render(&app, &data);
    let header = line_at(&buf, 1);
    let theme = theme_for(&app.app_type);
    let proxy_label = texts::tui_header_proxy_status(true);
    let provider_label = format!(
        "{}: {}",
        texts::provider_label().trim_end_matches([':', '：']),
        "Demo Provider"
    );

    let proxy_idx = header.find(&proxy_label).expect("proxy chip should render");
    let provider_idx = header
        .find(&provider_label)
        .expect("provider chip should render");

    assert!(
        proxy_idx < provider_idx,
        "proxy chip should sit left of provider: {header}"
    );

    let proxy_cell = &buf[(proxy_idx as u16, 1)];
    assert!(
        proxy_cell.fg == theme.accent || proxy_cell.bg == theme.accent,
        "proxy chip should use theme accent, got fg={:?}, bg={:?}",
        proxy_cell.fg,
        proxy_cell.bg
    );
}

#[test]
fn header_keeps_all_app_tabs_visible_with_proxy_chip() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let app = App::new(Some(AppType::Claude));
    let buf = render(&app, &minimal_data(&app.app_type));
    let header = line_at(&buf, 1);

    assert!(header.contains(AppType::Claude.as_str()), "{header}");
    assert!(header.contains(AppType::Codex.as_str()), "{header}");
    assert!(header.contains(AppType::Gemini.as_str()), "{header}");
    assert!(header.contains(AppType::OpenCode.as_str()), "{header}");
}

#[test]
fn nav_icons_have_left_padding_from_border() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let app = App::new(Some(AppType::Claude));
    let data = minimal_data(&app.app_type);
    let buf = render(&app, &data);

    let mut home_line = None;
    for y in 0..buf.area.height {
        let line = line_at(&buf, y);
        if line.contains("Home") && line.contains("🏠") {
            home_line = Some(line);
            break;
        }
    }

    let home_line = home_line.expect("Home row missing from nav");
    let emoji_idx = home_line
        .find("🏠")
        .expect("Home emoji missing from nav row");
    let emoji_char_idx = home_line[..emoji_idx].chars().count();
    let chars: Vec<char> = home_line.chars().collect();
    assert!(
        emoji_char_idx >= 2,
        "expected at least 2 chars before emoji, got line: {home_line}"
    );
    assert_eq!(
        chars[emoji_char_idx.saturating_sub(2)],
        '│',
        "expected nav border immediately before padding space, got line: {home_line}"
    );
    assert_eq!(
        chars[emoji_char_idx.saturating_sub(1)],
        ' ',
        "expected a 1-cell padding between nav border and emoji, got line: {home_line}"
    );
}

#[test]
fn providers_pane_has_border_and_selected_row_is_accent() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let theme = theme_for(&app.app_type);

    let content = super::content_pane_rect(buf.area, &theme);
    let border_cell = &buf[(content.x, content.y)];
    assert_eq!(border_cell.symbol(), "┌");
    assert_eq!(border_cell.fg, theme.accent);

    // Selected row should be highlighted with theme accent background.
    // Layout:
    // - content pane border (1)
    // - hint row (1)
    // - table header row (1)
    // - first data row (selected) (1)
    let selected_row_cell = &buf[(
        content.x.saturating_add(2 + super::CONTENT_INSET_LEFT),
        content.y.saturating_add(1 + 1 + 1),
    )];
    assert_eq!(selected_row_cell.bg, theme.accent);
}

#[test]
fn focused_pane_border_keeps_v500_bold_style_in_ansi256_mode() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _colorterm = EnvGuard::remove("COLORTERM");
    let _color_mode = EnvGuard::set("CC_SWITCH_COLOR_MODE", "ansi256");
    let _term = EnvGuard::remove("TERM");

    let mut app = App::new(Some(AppType::Claude));
    app.focus = Focus::Content;
    let theme = theme_for(&app.app_type);

    let style = super::pane_border_style(&app, Focus::Content, &theme);
    assert!(style.add_modifier.contains(ratatui::style::Modifier::BOLD));
}

#[test]
fn inactive_pane_border_keeps_v500_dim_color_in_ansi256_mode() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _colorterm = EnvGuard::remove("COLORTERM");
    let _color_mode = EnvGuard::set("CC_SWITCH_COLOR_MODE", "ansi256");
    let _term = EnvGuard::remove("TERM");

    let mut app = App::new(Some(AppType::Claude));
    app.focus = Focus::Nav;
    let theme = theme_for(&app.app_type);

    let style = super::pane_border_style(&app, Focus::Content, &theme);
    assert_eq!(style.fg, Some(theme.dim));
}

#[test]
fn informational_overlay_border_keeps_v500_dim_color_in_ansi256_mode() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _colorterm = EnvGuard::remove("COLORTERM");
    let _color_mode = EnvGuard::set("CC_SWITCH_COLOR_MODE", "ansi256");
    let _term = EnvGuard::remove("TERM");

    let theme = theme_for(&AppType::Claude);

    let style = super::overlay_border_style(&theme, false);
    assert_eq!(style.fg, Some(theme.dim));
}

#[test]
fn focused_form_border_keeps_v500_bold_style_in_ansi256_mode() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _colorterm = EnvGuard::remove("COLORTERM");
    let _color_mode = EnvGuard::set("CC_SWITCH_COLOR_MODE", "ansi256");
    let _term = EnvGuard::remove("TERM");

    let theme = theme_for(&AppType::Claude);

    let style = super::focus_block_style(true, &theme);
    assert!(style.add_modifier.contains(ratatui::style::Modifier::BOLD));
}

#[test]
fn update_available_primary_button_uses_accent_not_success_green() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenCode));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.overlay = Overlay::UpdateAvailable {
        current: "1.0.0".to_string(),
        latest: "1.1.0".to_string(),
        selected: 0,
    };
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let theme = theme_for(&app.app_type);
    let update_label = format!("[ {} ]", texts::tui_update_btn_update());
    let row_index = (0..buf.area.height)
        .find(|&y| line_at(&buf, y).contains(&update_label))
        .expect("update button should be rendered");
    let row = line_at(&buf, row_index);
    let x = row
        .find(&update_label)
        .map(|idx| UnicodeWidthStr::width(&row[..idx]) as u16 + 2)
        .expect("update button should be locatable");
    let cell = &buf[(x, row_index)];

    assert_ne!(
        theme.accent, theme.ok,
        "test app accent must differ from success green"
    );
    assert!(
        cell.fg == theme.accent || cell.bg == theme.accent,
        "primary action should use accent, got fg={:?}, bg={:?}",
        cell.fg,
        cell.bg
    );
}

#[test]
fn editor_cursor_matches_rendered_target_line() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Config;
    app.focus = Focus::Content;

    let long = "x".repeat(400);
    let marker = "<<<TARGET>>>";
    let initial = format!("{long}\n{marker}");

    app.open_editor(
        "Demo Editor",
        EditorKind::Json,
        initial,
        EditorSubmit::ConfigCommonSnippet {
            app_type: app.app_type.clone(),
        },
    );

    let editor = app.editor.as_mut().expect("editor opened");
    editor.cursor_row = 1;
    editor.cursor_col = 0;
    editor.scroll = 0;

    let data = minimal_data(&app.app_type);
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).expect("terminal created");
    terminal
        .draw(|f| super::render(f, &app, &data))
        .expect("draw ok");

    let cursor = terminal.get_cursor_position().expect("cursor position");
    let buf = terminal.backend().buffer().clone();

    let wrap_token = "x".repeat(20);
    let wrapped_rows = (0..buf.area.height)
        .filter(|y| line_at(&buf, *y).contains(&wrap_token))
        .count();
    assert!(
        wrapped_rows >= 2,
        "expected long line to wrap onto multiple rows, got {wrapped_rows}"
    );

    let mut marker_y = None;
    for y in 0..buf.area.height {
        let line = line_at(&buf, y);
        if line.contains(marker) {
            marker_y = Some(y);
            break;
        }
    }

    let marker_y = marker_y.expect("marker line rendered");
    assert_eq!(
        cursor.y, marker_y,
        "cursor should be on the same row as the rendered marker line"
    );
}

#[test]
fn editor_key_bar_shows_ctrl_o_external_editor_hint() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Config;
    app.focus = Focus::Content;
    app.open_editor(
        "Demo Editor",
        EditorKind::Json,
        "{\n  \"demo\": true\n}",
        EditorSubmit::ConfigCommonSnippet {
            app_type: app.app_type.clone(),
        },
    );

    let data = minimal_data(&app.app_type);
    let buf = render(&app, &data);

    let has_ctrl_o = (0..buf.area.height).any(|y| line_at(&buf, y).contains("Ctrl+O"));
    assert!(has_ctrl_o, "editor key bar should show the Ctrl+O hint");
}

#[test]
fn home_restores_main_logo_and_home_labels() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let all = all_text(&buf);
    assert!(all.contains("___  ___"));
    assert!(all.contains("\\___|\\___|"));
    assert!(all.contains("Connection Details"));
    assert!(all.contains("Use the left menu"));
}

#[test]
fn home_connection_card_labels_mcp_and_skills_with_active_counts() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.skills.installed = vec![
        crate::app_config::InstalledSkill {
            id: "local:skill-a".to_string(),
            name: "Skill A".to_string(),
            description: None,
            directory: "skill-a".to_string(),
            repo_owner: None,
            repo_name: None,
            repo_branch: None,
            readme_url: None,
            apps: crate::app_config::SkillApps {
                claude: true,
                codex: false,
                gemini: false,
                opencode: false,
            },
            installed_at: 0,
        },
        crate::app_config::InstalledSkill {
            id: "local:skill-b".to_string(),
            name: "Skill B".to_string(),
            description: None,
            directory: "skill-b".to_string(),
            repo_owner: None,
            repo_name: None,
            repo_branch: None,
            readme_url: None,
            apps: crate::app_config::SkillApps::default(),
            installed_at: 0,
        },
    ];

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains("MCP:"), "{all}");
    assert!(all.contains("Skills: [1/2 Active]"), "{all}");
}

#[test]
fn home_does_not_repeat_welcome_title_in_body() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let all = all_text(&buf);

    let needle = "CC-Switch Interactive Mode";
    let count = all.matches(needle).count();
    assert_eq!(count, 1, "expected welcome title once, got {count}");
}

#[test]
fn home_shows_local_env_check_section() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains("Local environment check"));
    assert!(!all.contains("Session Context"));
}

#[test]
fn home_shows_webdav_section() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains("WebDAV Sync"));
}

#[test]
fn home_hides_proxy_dashboard_when_proxy_is_off() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.tick = 1;
    app.route = Route::Main;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.proxy.listen_address = "127.0.0.1".to_string();
    data.proxy.listen_port = 15721;
    data.proxy.default_cost_multiplier = Some("1".to_string());

    let buf = render(&app, &data);
    let all = all_text(&buf);
    let footer = line_at(&buf, buf.area.height - 1);

    assert!(all.contains("___  ___"));
    assert!(all.contains("\\___|\\___|"));
    assert!(footer.contains("proxy on"), "{footer}");
    assert!(!all.contains("Proxy Dashboard"), "{all}");
    assert!(!all.contains("127.0.0.1:15721"), "{all}");
    assert!(!all.contains("x1.00"), "{all}");
}

#[test]
fn home_shows_proxy_dashboard_when_current_app_proxy_is_on() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.tick = 2;
    app.route = Route::Main;
    app.focus = Focus::Content;
    app.proxy_output_activity_samples = vec![0, 1, 4, 8, 4, 1, 0];
    app.proxy_input_activity_samples = vec![0, 1, 2, 4, 2, 1, 0];

    let mut data = minimal_data(&app.app_type);
    data.proxy.running = true;
    data.proxy.claude_takeover = true;
    data.proxy.listen_address = "127.0.0.1".to_string();
    data.proxy.listen_port = 3456;
    data.proxy.uptime_seconds = 3661;
    data.proxy.total_requests = 7;
    data.proxy.success_rate = Some(85.7);
    data.proxy.estimated_input_tokens_total = 1_200;
    data.proxy.estimated_output_tokens_total = 4_800;
    data.proxy.current_provider = Some("Claude Test Provider".to_string());
    data.proxy.current_app_target = Some(super::super::data::ProxyTargetSnapshot {
        provider_name: "Claude Test Provider".to_string(),
    });
    data.proxy.last_error = Some("last upstream failure".to_string());
    data.proxy.default_cost_multiplier = Some("1.5".to_string());

    let buf = render(&app, &data);
    let all = all_text(&buf);
    let footer = line_at(&buf, buf.area.height - 1);
    let local_env_idx = all
        .find("Local environment check")
        .expect("local env section should render");
    let dashboard_idx = all
        .find("Proxy Dashboard")
        .expect("proxy dashboard should render");
    let traffic_idx = all
        .find("Proxy Dashboard   ▲ ~4.8k / ▼ ~1.2k")
        .expect("proxy title badge should render inline");
    let waveform_idx = all.find('⣿').expect("waveform should render");
    let meta_rows = (0..buf.area.height)
        .filter(|y| {
            let line = line_at(&buf, *y);
            line.contains("Uptime:") || line.contains("Last proxy error:")
        })
        .collect::<Vec<_>>();

    assert!(all.contains("Proxy Dashboard"), "{all}");
    assert!(all.contains("┌ Proxy Dashboard "), "{all}");
    assert!(dashboard_idx > local_env_idx, "{all}");
    assert!(!all.contains("___  ___"), "{all}");
    assert!(all.contains("Use the left menu"), "{all}");
    assert!(traffic_idx < waveform_idx, "{all}");
    assert!(meta_rows.len() <= 2, "{all}");
    assert!(!all.contains("ACTIVE"), "{all}");
    assert!(
        !all.contains("Claude active -> Claude Test Provider"),
        "{all}"
    );
    assert!(!all.contains("x1.50"), "{all}");
    assert!(all.contains('⣿'), "{all}");
    assert!(
        all.contains('⣀') || all.contains('⣄') || all.contains('⣤'),
        "{all}"
    );
    assert!(
        all.contains('⠁')
            || all.contains('⠉')
            || all.contains('⠋')
            || all.contains('⠛')
            || all.contains('⣿'),
        "{all}"
    );
    assert!(!all.contains("[=   ]"), "{all}");
    assert!(!all.contains("[==  ]"), "{all}");
    assert!(!all.contains("[=== ]"), "{all}");
    assert!(!all.contains("[ ==]"), "{all}");
    assert!(!all.contains('▁'), "{all}");
    assert!(all.contains("127.0.0.1:3456"));
    assert!(all.contains("1h 1m 1s"));
    assert!(all.contains("▲ ~4.8k / ▼ ~1.2k"), "{all}");
    assert!(!all.contains("Traffic:"), "{all}");
    assert!(!all.contains("Claude Test Provider"), "{all}");
    assert!(all.contains("last upstream failure"), "{all}");
    assert!(!all.contains("Active target:"), "{all}");
    assert!(footer.contains("proxy off"), "{footer}");
    assert!(!all.contains("Current app takeover"));
    assert!(!all.contains("Manual routing only"));
    assert!(!all.contains("automatic failover"));
}

#[test]
fn home_footer_shows_proxy_on_shortcut_when_stopped() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.proxy.listen_address = "127.0.0.1".to_string();
    data.proxy.listen_port = 15721;

    let buf = render(&app, &data);
    let all = all_text(&buf);
    let footer = line_at(&buf, buf.area.height - 1);

    assert!(footer.contains("proxy on"), "{footer}");
    assert!(all.contains("___  ___"));
    assert!(!all.contains("Proxy Dashboard"));
}

#[test]
fn home_proxy_dashboard_keeps_current_app_off_semantics_when_another_app_is_active() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.tick = 1;
    app.route = Route::Main;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.proxy.running = true;
    data.proxy.managed_runtime = true;
    data.proxy.codex_takeover = true;
    data.proxy.listen_address = "127.0.0.1".to_string();
    data.proxy.listen_port = 15721;
    data.proxy.default_cost_multiplier = Some("1".to_string());

    let buf = render(&app, &data);
    let all = all_text(&buf);
    let footer = line_at(&buf, buf.area.height - 1);

    assert!(footer.contains("proxy on"), "{footer}");
    assert!(all.contains("___  ___"), "{all}");
    assert!(!all.contains("Proxy Dashboard"), "{all}");
    assert!(!all.contains("Shared runtime ready"), "{all}");
    assert!(!all.contains("x1.00"), "{all}");
}

#[test]
fn home_proxy_dashboard_hides_attach_cta_for_foreground_runtime_owned_elsewhere() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.proxy.running = true;
    data.proxy.managed_runtime = false;
    data.proxy.codex_takeover = true;
    data.proxy.listen_address = "127.0.0.1".to_string();
    data.proxy.listen_port = 15721;

    let buf = render(&app, &data);
    let all = all_text(&buf);
    let footer = line_at(&buf, buf.area.height - 1);

    assert!(!footer.contains("proxy on"), "{footer}");
    assert!(all.contains("___  ___"), "{all}");
    assert!(!all.contains("Proxy Dashboard"), "{all}");
}

#[test]
fn home_proxy_dashboard_shows_idle_baseline_without_header_copy() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.tick = 1;
    app.route = Route::Main;
    app.focus = Focus::Content;

    let mut active = minimal_data(&app.app_type);
    active.proxy.running = true;
    active.proxy.managed_runtime = true;
    active.proxy.claude_takeover = true;
    active.proxy.estimated_input_tokens_total = 0;
    active.proxy.estimated_output_tokens_total = 0;
    active.proxy.default_cost_multiplier = Some("1.25".to_string());
    active.proxy.current_app_target = Some(super::super::data::ProxyTargetSnapshot {
        provider_name: "Claude Test Provider".to_string(),
    });

    let active_buf = render(&app, &active);
    let active_text = all_text(&active_buf);
    assert!(!active_text.contains("x1.25"), "{active_text}");
    assert!(!active_text.contains("ACTIVE"), "{active_text}");
    assert!(
        active_text.contains('⡀') || active_text.contains('⠁'),
        "{active_text}"
    );
    assert!(!active_text.contains("[=   ]"), "{active_text}");
    assert!(!active_text.contains("[==  ]"), "{active_text}");
    assert!(!active_text.contains("[=== ]"), "{active_text}");
    assert!(!active_text.contains("[ ==]"), "{active_text}");
    assert!(active_text.contains("Proxy Dashboard"));
    assert!(active_text.contains("▲ ~0 / ▼ ~0"), "{active_text}");
    assert!(!active_text.contains("Traffic:"), "{active_text}");

    let mut shared_runtime = minimal_data(&app.app_type);
    shared_runtime.proxy.running = true;
    shared_runtime.proxy.managed_runtime = true;
    shared_runtime.proxy.codex_takeover = true;
    shared_runtime.proxy.default_cost_multiplier = Some("1.25".to_string());

    let shared_buf = render(&app, &shared_runtime);
    let shared_text = all_text(&shared_buf);
    let shared_footer = line_at(&shared_buf, shared_buf.area.height - 1);
    assert!(shared_text.contains("___  ___"), "{shared_text}");
    assert!(!shared_text.contains("Proxy Dashboard"), "{shared_text}");
    assert!(!shared_text.contains("x1.25"), "{shared_text}");
    assert!(shared_footer.contains("proxy on"), "{shared_footer}");
}

#[test]
fn home_proxy_dashboard_stacks_text_on_narrow_terminals() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.proxy.running = true;
    data.proxy.claude_takeover = true;
    data.proxy.listen_address = "127.0.0.1".to_string();
    data.proxy.listen_port = 3456;
    data.proxy.total_requests = 12;
    data.proxy.success_rate = Some(91.7);
    data.proxy.uptime_seconds = 3661;
    data.proxy.current_app_target = Some(super::super::data::ProxyTargetSnapshot {
        provider_name: "Claude Test Provider With A Very Long Name".to_string(),
    });
    data.proxy.last_error = Some(
        "last upstream failure with a much longer detail that should truncate cleanly".to_string(),
    );

    let buf = render_with_size(&app, &data, 80, 24);
    let all = all_text(&buf);

    assert!(all.contains("▲ ~0 / ▼ ~0"), "{all}");
    assert!(all.contains("Listen"), "{all}");
    assert!(all.contains("Uptime"), "{all}");
    assert!(all.contains("proxy") && all.contains("error"), "{all}");
    assert!(!all.contains("Active target"), "{all}");
    assert!(all.contains('⡀') || all.contains('⠁'), "{all}");
}

#[test]
fn transition_effect_changes_dashboard_cells_during_proxy_start() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;

    let off = minimal_data(&app.app_type);
    app.observe_proxy_visual_state(&off);

    let mut on = minimal_data(&app.app_type);
    on.proxy.running = true;
    on.proxy.claude_takeover = true;
    on.proxy.default_cost_multiplier = None;
    on.proxy.current_app_target = Some(super::super::data::ProxyTargetSnapshot {
        provider_name: "Demo Provider".to_string(),
    });

    app.observe_proxy_visual_state(&on);

    app.on_tick();
    app.on_tick();
    app.on_tick();
    app.on_tick();

    let transition_buf = render(&app, &on);
    let transition_text = all_text(&transition_buf);

    for _ in 0..app::PROXY_HERO_TRANSITION_TICKS {
        app.on_tick();
    }

    let settled_buf = render(&app, &on);
    let settled_text = all_text(&settled_buf);
    let content_y = (0..settled_buf.area.height)
        .find(|y| line_at(&settled_buf, *y).contains("Listen:"))
        .expect("dashboard metadata line should render after transition");
    let padding_x = (70..settled_buf.area.width.saturating_sub(2))
        .rev()
        .find(|x| {
            transition_buf[(*x, content_y)].symbol() == " "
                && settled_buf[(*x, content_y)].symbol() == " "
        })
        .expect("should find padded blank cell inside dashboard line");
    assert!(settled_text.contains("Proxy Dashboard"), "{settled_text}");
    assert_eq!(transition_text, settled_text);
    assert!(!transition_text.contains("___  ___"), "{transition_text}");
    assert!(!transition_text.contains("/ __|"), "{transition_text}");
    assert!(!transition_text.contains("| (__"), "{transition_text}");
    assert_eq!(
        transition_buf[(padding_x, content_y)].bg,
        settled_buf[(padding_x, content_y)].bg,
        "transition should not paint a background plate into dashboard padding"
    );
    assert!(!settled_text.contains("___  ___"), "{settled_text}");
}

#[test]
fn proxy_activity_wave_uses_real_request_history() {
    let flat = super::main_page::proxy_activity_wave(8, true, &[0, 0, 0, 0]);
    let burst = super::main_page::proxy_activity_wave(8, true, &[0, 1, 4, 8]);

    assert_eq!(flat, "⡀⡀⡀⡀⡀⡀⡀⡀");
    assert_ne!(burst, flat);
    assert!(burst.contains('⡀'), "{burst}");
    assert!(burst.contains('⣿'), "{burst}");
    assert!(
        burst.contains('⣀') || burst.contains('⣄') || burst.contains('⣤'),
        "{burst}"
    );
}

#[test]
fn home_proxy_dashboard_marks_unsupported_apps_without_proxy_cta() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenCode));
    app.route = Route::Main;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.proxy.listen_address = "127.0.0.1".to_string();
    data.proxy.listen_port = 15721;
    data.proxy.default_cost_multiplier = Some("1.25".to_string());
    data.proxy.running = true;
    data.proxy.managed_runtime = true;
    data.proxy.claude_takeover = true;
    data.proxy.current_provider = Some("Claude Test Provider".to_string());

    let buf = render(&app, &data);
    let all = all_text(&buf);
    let footer = line_at(&buf, buf.area.height - 1);

    assert!(!all.contains("start proxy"));
    assert!(!all.contains("stop proxy"));
    assert!(!footer.contains("proxy on"), "{footer}");
    assert!(all.contains("___  ___"), "{all}");
    assert!(!all.contains("Proxy Dashboard"), "{all}");
    assert!(!all.contains("Claude Test Provider"), "{all}");
}

#[test]
fn home_proxy_dashboard_shows_proxy_off_shortcut_when_current_app_is_active() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.proxy.running = true;
    data.proxy.claude_takeover = true;
    data.proxy.listen_address = "127.0.0.1".to_string();
    data.proxy.listen_port = 3456;

    let buf = render(&app, &data);
    let all = all_text(&buf);
    let footer = line_at(&buf, buf.area.height - 1);

    assert!(footer.contains("proxy off"), "{footer}");
    assert!(!all.contains("ACTIVE"), "{all}");
    assert!(all.contains("Proxy Dashboard"));
}

#[test]
fn home_proxy_dashboard_keeps_current_app_route_separate_from_global_proxy_route() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.proxy.running = true;
    data.proxy.managed_runtime = true;
    data.proxy.codex_takeover = true;
    data.proxy.listen_address = "127.0.0.1".to_string();
    data.proxy.listen_port = 3456;
    data.proxy.total_requests = 9;
    data.proxy.success_rate = Some(100.0);
    data.proxy.current_provider = Some("Gemini Production Route".to_string());

    let buf = render(&app, &data);
    let all = all_text(&buf);
    let footer = line_at(&buf, buf.area.height - 1);

    assert!(all.contains("___  ___"), "{all}");
    assert!(!all.contains("Proxy Dashboard"), "{all}");
    assert!(footer.contains("proxy on"), "{footer}");
    assert!(!all.contains("Latest proxy route"));
    assert!(!all.contains("Gemini Production Route"));
}

#[test]
fn home_proxy_dashboard_hides_internal_target_identifiers() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.proxy.running = true;
    data.proxy.listen_address = "127.0.0.1".to_string();
    data.proxy.listen_port = 3456;
    data.proxy.current_app_target = Some(super::super::data::ProxyTargetSnapshot {
        provider_name: "Claude Test Provider".to_string(),
    });

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains("___  ___"));
    assert!(!all.contains("Proxy Dashboard"));
    assert!(!all.contains("Claude Test Provider"));
    assert!(!all.contains("Current app route"));
    assert!(!all.contains("claude-provider"));
    assert!(!all.contains("claude ->"));
}

#[test]
fn home_connection_card_does_not_claim_online_or_offline_without_health_check() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;

    let data = minimal_data(&app.app_type);
    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(!all.contains("Online"));
    assert!(!all.contains("Offline"));
}

#[test]
fn home_webdav_not_configured_does_not_show_error() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.webdav_sync = Some(crate::settings::WebDavSyncSettings {
        enabled: true,
        ..Default::default()
    });

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains("Not configured"));
    assert!(!all.contains("Last error"));
    assert!(!all.contains("Enabled"));
}

#[test]
fn home_webdav_failure_shows_error_details() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    let mut webdav = crate::settings::WebDavSyncSettings {
        enabled: true,
        ..Default::default()
    };
    webdav.base_url = "https://dav.example".to_string();
    webdav.username = "demo".to_string();
    webdav.password = "app-pass".to_string();
    webdav.status.last_error = Some("auth failed".to_string());
    data.config.webdav_sync = Some(webdav);

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains("Error (auth failed)"));
    assert!(!all.contains("Last error"));
    assert!(!all.contains("Enabled"));
}

#[test]
fn webdav_sync_time_formats_to_minute() {
    let formatted = super::format_sync_time_local_to_minute(1_735_689_600)
        .expect("timestamp should be formatable");
    assert_eq!(formatted.len(), 16);
    assert_eq!(&formatted[4..5], "/");
    assert_eq!(&formatted[7..8], "/");
    assert_eq!(&formatted[10..11], " ");
    assert_eq!(&formatted[13..14], ":");
}

#[test]
fn nav_does_not_show_manage_prefix_or_view_config() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Nav;
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(
        !all.contains("Manage "),
        "expected nav to not include Manage prefix"
    );
    assert!(
        !all.contains("View Current Configuration"),
        "expected nav to not include View Current Configuration"
    );
}

#[test]
fn skills_page_renders_sync_method_and_installed_rows() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Skills;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.skills.sync_method = SyncMethod::Copy;
    data.skills.installed = vec![installed_skill("hello-skill", "Hello Skill")];

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains(&texts::tui_skills_installed_counts(1, 0, 0, 0)));
    assert!(!all.contains(texts::tui_header_directory()));
    assert!(!all.contains("hello-skill"));
    assert!(all.contains("Hello Skill"));
}

#[test]
fn skills_page_prefers_full_name_over_directory() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Skills;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.skills.installed = vec![installed_skill("cxgo", "CXGO - C/C++ to Go")];

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains("CXGO - C/C++ to Go"));
    assert!(!all.contains("cxgo"));
}

#[test]
fn skills_page_key_bar_shows_apps_and_uninstall_actions() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Skills;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.skills.installed = vec![installed_skill("hello-skill", "Hello Skill")];

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains(texts::tui_key_apps()));
    assert!(all.contains(texts::tui_key_uninstall()));
}

#[test]
fn skills_page_shows_opencode_summary() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenCode));
    app.route = Route::Skills;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    let mut skill = installed_skill("hello-skill", "Hello Skill");
    skill.apps = SkillApps {
        claude: false,
        codex: false,
        gemini: false,
        opencode: true,
    };
    data.skills.installed = vec![skill];

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains("OpenCode: 1"));
}

#[test]
fn skill_detail_page_shows_opencode_enabled_state() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenCode));
    app.route = Route::SkillDetail {
        directory: "hello-skill".to_string(),
    };
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    let mut skill = installed_skill("hello-skill", "Hello Skill");
    skill.apps = SkillApps {
        claude: false,
        codex: false,
        gemini: false,
        opencode: true,
    };
    data.skills.installed = vec![skill];

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains(texts::tui_label_enabled_for()));
    assert!(all.contains("OpenCode"));
    assert!(!all.contains("opencode=true"));
}

#[test]
fn skills_import_overlay_uses_friendly_copy() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Skills;
    app.focus = Focus::Content;
    app.overlay = Overlay::SkillsImportPicker {
        skills: vec![UnmanagedSkill {
            directory: "hello-skill".to_string(),
            name: "Hello Skill".to_string(),
            description: Some("A local skill".to_string()),
            found_in: vec!["claude".to_string()],
        }],
        selected_idx: 0,
        selected: std::iter::once("hello-skill".to_string()).collect(),
    };

    let data = minimal_data(&app.app_type);
    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains(texts::tui_skills_import_title()));
    assert!(all.contains(texts::tui_skills_import_description()));
    assert!(!all.contains("SSOT"));
    assert!(!all.contains("unmanaged"));
}

#[test]
fn mcp_page_renders_opencode_column() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenCode));
    app.route = Route::Mcp;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.mcp.rows = vec![super::super::data::McpRow {
        id: "m1".to_string(),
        server: crate::app_config::McpServer {
            id: "m1".to_string(),
            name: "Server".to_string(),
            server: json!({}),
            apps: crate::app_config::McpApps {
                claude: false,
                codex: false,
                gemini: false,
                opencode: true,
            },
            description: None,
            homepage: None,
            docs: None,
            tags: vec![],
        },
    }];

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains("opencode"));
}

#[test]
fn mcp_page_key_bar_hides_validate_action() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Mcp;
    app.focus = Focus::Content;

    let data = minimal_data(&app.app_type);
    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(!all.contains("validate"));
    assert!(!all.contains("校验"));
}

#[test]
fn mcp_page_uses_import_existing_label() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Mcp;
    app.focus = Focus::Content;

    let data = minimal_data(&app.app_type);
    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains(texts::tui_mcp_action_import_existing()));
}

#[test]
fn help_text_mentions_import_existing_for_mcp() {
    let help = texts::tui_help_text();

    assert!(
        help.contains("i import existing") || help.contains("i 导入已有"),
        "help text should use the same import wording for MCP and Skills"
    );
}

#[test]
fn mcp_page_shows_summary_bar() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenCode));
    app.route = Route::Mcp;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.mcp.rows = vec![
        super::super::data::McpRow {
            id: "m1".to_string(),
            server: crate::app_config::McpServer {
                id: "m1".to_string(),
                name: "Server 1".to_string(),
                server: json!({}),
                apps: crate::app_config::McpApps {
                    claude: true,
                    codex: false,
                    gemini: false,
                    opencode: true,
                },
                description: None,
                homepage: None,
                docs: None,
                tags: vec![],
            },
        },
        super::super::data::McpRow {
            id: "m2".to_string(),
            server: crate::app_config::McpServer {
                id: "m2".to_string(),
                name: "Server 2".to_string(),
                server: json!({}),
                apps: crate::app_config::McpApps {
                    claude: false,
                    codex: true,
                    gemini: false,
                    opencode: false,
                },
                description: None,
                homepage: None,
                docs: None,
                tags: vec![],
            },
        },
    ];

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains("Installed"));
    assert!(all.contains("Claude: 1"));
}

#[test]
fn skills_discover_page_shows_hint_when_empty() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::SkillsDiscover;
    app.focus = Focus::Content;
    app.skills_discover_results = vec![];
    app.skills_discover_query = String::new();

    let data = minimal_data(&app.app_type);
    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains(texts::tui_skills_discover_hint()));
}

#[test]
fn skills_repos_page_renders_repo_rows() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::SkillsRepos;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.skills.repos = vec![SkillRepo {
        owner: "anthropics".to_string(),
        name: "skills".to_string(),
        branch: "main".to_string(),
        enabled: true,
    }];

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains("anthropics/skills"));
}

#[test]
fn text_input_overlay_renders_inner_input_box() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Config;
    app.focus = Focus::Content;
    app.overlay = Overlay::TextInput(TextInputState {
        title: "Demo".to_string(),
        prompt: "Enter value".to_string(),
        buffer: "hello".to_string(),
        submit: TextSubmit::ConfigBackupName,
        secret: false,
    });
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);

    let theme = theme_for(&app.app_type);
    let content = super::content_pane_rect(buf.area, &theme);
    let area = super::centered_rect_fixed(super::OVERLAY_FIXED_LG.0, 12, content);
    let area_x = area.x;
    let area_y = area.y;
    let area_w = area.width;
    let area_h = area.height;

    // Outer border exists at (18,13). We also expect an inner input field border (another ┌)
    // somewhere inside the overlay.
    let mut inner_top_left_count = 0usize;
    for y in area_y..area_y.saturating_add(area_h) {
        for x in area_x..area_x.saturating_add(area_w) {
            if x == area_x && y == area_y {
                continue;
            }
            if buf[(x, y)].symbol() == "┌" {
                inner_top_left_count += 1;
            }
        }
    }

    assert!(
        inner_top_left_count >= 1,
        "expected an inner input box border in TextInput overlay"
    );
}

#[test]
fn editor_unsaved_changes_confirm_overlay_shows_three_actions_and_is_compact() {
    let _lock = lock_env();

    let prev = std::env::var("NO_COLOR").ok();
    std::env::set_var("NO_COLOR", "1");
    let _restore_no_color = EnvGuard {
        key: "NO_COLOR",
        prev,
    };

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Prompts;
    app.focus = Focus::Content;
    app.overlay = Overlay::Confirm(ConfirmOverlay {
        title: texts::tui_editor_save_before_close_title().to_string(),
        message: texts::tui_editor_save_before_close_message().to_string(),
        action: ConfirmAction::EditorSaveBeforeClose,
    });
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(
        all.contains("Enter=save & exit"),
        "expected save action hint in confirm overlay key bar"
    );
    assert!(
        all.contains("N=exit w/o save"),
        "expected discard action hint in confirm overlay key bar"
    );
    assert!(
        all.contains("Esc=cancel"),
        "expected cancel action hint in confirm overlay key bar"
    );

    let theme = theme_for(&app.app_type);
    let content = super::content_pane_rect(buf.area, &theme);
    let area = super::centered_rect_fixed(
        super::OVERLAY_FIXED_MD.0,
        super::OVERLAY_FIXED_MD.1,
        content,
    );

    assert_eq!(buf[(area.x, area.y)].symbol(), "┌");
    assert_eq!(
        buf[(
            area.x.saturating_add(area.width.saturating_sub(1)),
            area.y.saturating_add(area.height.saturating_sub(1))
        )]
            .symbol(),
        "┘"
    );
}

#[test]
fn claude_api_format_picker_overlay_is_compact_and_padded() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.form = Some(crate::cli::tui::form::FormState::ProviderAdd(
        crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude),
    ));
    app.overlay = Overlay::ClaudeApiFormatPicker { selected: 1 };

    let data = minimal_data(&app.app_type);
    let buf = render(&app, &data);

    let theme = theme_for(&app.app_type);
    let content = super::content_pane_rect(buf.area, &theme);
    let area = super::centered_rect_fixed(58, 10, content);

    assert_eq!(buf[(area.x, area.y)].symbol(), "┌");
    assert_eq!(
        buf[(
            area.x.saturating_add(area.width.saturating_sub(1)),
            area.y.saturating_add(area.height.saturating_sub(1))
        )]
            .symbol(),
        "┘"
    );

    let message = "OpenAI Chat Completions";
    let row_index = (0..buf.area.height)
        .find(|&y| line_at(&buf, y).contains(message))
        .expect("API format option should be rendered");
    let row = line_at(&buf, row_index);
    let msg_start = row.find(message).expect("message should be present");
    let left_border = row[..msg_start]
        .rfind('│')
        .expect("message row should have left border");
    let right_border_offset = row[msg_start + message.len()..]
        .find('│')
        .expect("message row should have right border");
    let right_border = msg_start + message.len() + right_border_offset;

    assert!(
        msg_start.saturating_sub(left_border) >= 4,
        "option should keep comfortable left padding: {row:?}"
    );
    assert!(
        right_border.saturating_sub(msg_start + message.len()) >= 3,
        "option should keep comfortable right padding: {row:?}"
    );
    assert!(
        row_index > area.y.saturating_add(1),
        "options should not hug the top border"
    );
    assert!(
        area.y.saturating_add(area.height).saturating_sub(row_index) >= 4,
        "options should keep visible bottom margin"
    );
}

#[test]
fn provider_api_format_proxy_notice_overlay_uses_close_actions() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.overlay = Overlay::Confirm(ConfirmOverlay {
        title: texts::tui_claude_api_format_requires_proxy_title().to_string(),
        message: texts::tui_claude_api_format_requires_proxy_message("openai_chat"),
        action: ConfirmAction::ProviderApiFormatProxyNotice,
    });

    let data = minimal_data(&app.app_type);
    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(
        all.contains("Enter close"),
        "expected Enter close hint: {all}"
    );
    assert!(all.contains("Esc close"), "expected Esc close hint: {all}");
    assert!(
        !all.contains("Enter confirm"),
        "should not show confirm hint: {all}"
    );
    assert!(
        !all.contains("Esc cancel"),
        "should not show cancel hint: {all}"
    );
}

#[test]
fn footer_shows_only_global_actions() {
    let _lock = lock_env();

    let prev = std::env::var("NO_COLOR").ok();
    std::env::set_var("NO_COLOR", "1");
    let _restore_no_color = EnvGuard {
        key: "NO_COLOR",
        prev,
    };

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Config;
    app.focus = Focus::Content;
    app.overlay = Overlay::CommonSnippetView {
        app_type: AppType::Claude,
        view: crate::cli::tui::app::TextViewState {
            title: "Common Snippet".to_string(),
            lines: vec!["{}".to_string()],
            scroll: 0,
            action: None,
        },
    };
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let footer = line_at(&buf, buf.area.height - 1);

    assert!(
        footer.contains("switch app") && footer.contains("/ filter"),
        "expected footer to show global actions; got: {footer:?}"
    );
    assert!(
        !footer.contains("clear") && !footer.contains("apply"),
        "expected footer to not show overlay/page actions; got: {footer:?}"
    );
}

#[test]
fn footer_uses_terminal_palette_in_ansi256_mode() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _colorterm = EnvGuard::remove("COLORTERM");
    let _color_mode = EnvGuard::set("CC_SWITCH_COLOR_MODE", "ansi256");
    let _term = EnvGuard::remove("TERM");

    let app = App::new(Some(AppType::Claude));
    let data = minimal_data(&app.app_type);
    let buf = render(&app, &data);
    let footer_y = buf.area.height - 1;

    let mut saw_indexed_bg = false;
    for x in 0..buf.area.width {
        let cell = &buf[(x, footer_y)];
        assert!(
            !matches!(cell.fg, ratatui::style::Color::Rgb(_, _, _)),
            "footer should not emit RGB foregrounds in ansi256 mode: {:?}",
            cell.fg
        );
        assert!(
            !matches!(cell.bg, ratatui::style::Color::Rgb(_, _, _)),
            "footer should not emit RGB backgrounds in ansi256 mode: {:?}",
            cell.bg
        );
        saw_indexed_bg |= matches!(cell.bg, ratatui::style::Color::Indexed(_));
    }

    assert!(
        saw_indexed_bg,
        "footer should render indexed background cells"
    );
}

#[test]
fn toast_renders_as_centered_overlay() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.push_toast("Toast message", crate::cli::tui::app::ToastKind::Success);
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let footer = line_at(&buf, buf.area.height - 1);
    assert!(
        !footer.contains("Toast message"),
        "toast should not be rendered in footer: {footer:?}"
    );

    let toast_row = (0..buf.area.height)
        .find(|&y| line_at(&buf, y).contains("Toast message"))
        .expect("toast message should be rendered");
    let theme = theme_for(&app.app_type);
    let content = super::content_pane_rect(buf.area, &theme);
    let content_mid = content.y + content.height / 2;
    assert!(
            toast_row.abs_diff(content_mid) <= 2,
            "toast should render near the content center, got row {toast_row}, content mid {content_mid}"
        );

    let row = line_at(&buf, toast_row);
    let msg_start = row
        .find("Toast message")
        .expect("toast row should contain message");
    let left_border = row[..msg_start]
        .rfind('│')
        .expect("toast row should have a left border");
    let right_border = row[msg_start + "Toast message".len()..]
        .find('│')
        .expect("toast row should have a right border");

    assert!(
        msg_start.saturating_sub(left_border) > 2,
        "toast message should not hug the left border: {row:?}"
    );
    assert!(
        right_border > 2,
        "toast message should not hug the right border: {row:?}"
    );
}

#[test]
fn info_toast_uses_app_accent_border_color() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenCode));
    app.route = Route::Mcp;
    app.focus = Focus::Content;
    app.push_toast(
        texts::tui_toast_mcp_imported(0),
        crate::cli::tui::app::ToastKind::Info,
    );
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let theme = theme_for(&app.app_type);
    assert_ne!(
        theme.accent, theme.ok,
        "OpenCode accent should differ from success green"
    );

    let message = format!(
        "{} {}",
        texts::tui_toast_prefix_info().trim(),
        texts::tui_toast_mcp_imported(0)
    );
    let content = super::content_pane_rect(buf.area, &theme);
    let area = super::toast_rect(content, &message);
    let border_cell = &buf[(area.x, area.y + area.height / 2)];

    assert_eq!(border_cell.symbol(), "│");
    assert_eq!(border_cell.fg, theme.accent);
}

#[test]
fn success_toast_uses_app_accent_border_color() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenCode));
    app.route = Route::Main;
    app.focus = Focus::Content;
    app.push_toast(
        texts::tui_toast_proxy_managed_current_app_updated("Claude", false),
        crate::cli::tui::app::ToastKind::Success,
    );
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let theme = theme_for(&app.app_type);
    assert_ne!(
        theme.accent, theme.ok,
        "OpenCode accent should differ from success green"
    );

    let message = format!(
        "{} {}",
        texts::tui_toast_prefix_success().trim(),
        texts::tui_toast_proxy_managed_current_app_updated("Claude", false)
    );
    let content = super::content_pane_rect(buf.area, &theme);
    let area = super::toast_rect(content, &message);
    let border_cell = &buf[(area.x, area.y + area.height / 2)];

    assert_eq!(border_cell.symbol(), "│");
    assert_eq!(border_cell.fg, theme.accent);
}

#[test]
fn update_result_success_overlay_uses_app_accent_border_color() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenCode));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.overlay = Overlay::UpdateResult {
        success: true,
        message: "Updated successfully".to_string(),
    };
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let theme = theme_for(&app.app_type);
    assert_ne!(
        theme.accent, theme.ok,
        "OpenCode accent should differ from success green"
    );

    let content = super::content_pane_rect(buf.area, &theme);
    let area = super::centered_rect_fixed(
        super::OVERLAY_FIXED_SM.0,
        super::OVERLAY_FIXED_SM.1,
        content,
    );
    let border_cell = &buf[(area.x, area.y + area.height / 2)];

    assert_eq!(border_cell.symbol(), "│");
    assert_eq!(border_cell.fg, theme.accent);
}

#[test]
fn speedtest_running_overlay_is_compact_and_centered() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.overlay = Overlay::SpeedtestRunning {
        url: "https://x.y".to_string(),
    };
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let message = texts::tui_speedtest_running("https://x.y");
    let row_index = (0..buf.area.height)
        .find(|&y| line_at(&buf, y).contains(&message))
        .expect("speedtest running message should be rendered");
    let row = line_at(&buf, row_index);
    let msg_start = row.find(&message).expect("message should be present");
    let left_border = row[..msg_start]
        .rfind('│')
        .expect("message row should have left border");
    let right_border_offset = row[msg_start + message.len()..]
        .find('│')
        .expect("message row should have right border");
    let right_border = msg_start + message.len() + right_border_offset;
    let overlay_width = right_border.saturating_sub(left_border).saturating_add(1);

    assert!(
        msg_start.saturating_sub(left_border) > 2,
        "message should not hug left border: {row:?}"
    );
    assert!(
        right_border.saturating_sub(msg_start + message.len()) > 2,
        "message should not hug right border: {row:?}"
    );
    assert!(
        overlay_width < super::OVERLAY_FIXED_MD.0 as usize,
        "short running overlay should be compact, got width {overlay_width}"
    );
}

#[test]
fn stream_check_running_overlay_is_compact_and_centered() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::ProviderDetail {
        id: "p1".to_string(),
    };
    app.focus = Focus::Content;
    app.overlay = Overlay::StreamCheckRunning {
        provider_id: "p1".to_string(),
        provider_name: "Demo".to_string(),
    };
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let message = texts::tui_stream_check_running("Demo");
    let row_index = (0..buf.area.height)
        .find(|&y| line_at(&buf, y).contains(&message))
        .expect("stream check running message should be rendered");
    let row = line_at(&buf, row_index);
    let msg_start = row.find(&message).expect("message should be present");
    let left_border = row[..msg_start]
        .rfind('│')
        .expect("message row should have left border");
    let right_border_offset = row[msg_start + message.len()..]
        .find('│')
        .expect("message row should have right border");
    let right_border = msg_start + message.len() + right_border_offset;
    let overlay_width = right_border.saturating_sub(left_border).saturating_add(1);

    assert!(
        msg_start.saturating_sub(left_border) > 2,
        "message should not hug left border: {row:?}"
    );
    assert!(
        right_border.saturating_sub(msg_start + message.len()) > 2,
        "message should not hug right border: {row:?}"
    );
    assert!(
        overlay_width < super::OVERLAY_FIXED_MD.0 as usize,
        "short running overlay should be compact, got width {overlay_width}"
    );
}

#[test]
fn speedtest_result_overlay_is_compact_when_lines_are_short() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.overlay = Overlay::SpeedtestResult {
        url: "https://ww.packyapi.com".to_string(),
        lines: vec![
            texts::tui_speedtest_line_url("https://ww.packyapi.com"),
            String::new(),
            texts::tui_speedtest_line_latency("367 ms"),
            texts::tui_speedtest_line_status("200"),
        ],
        scroll: 0,
    };
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let row_index = (0..buf.area.height)
        .find(|&y| line_at(&buf, y).contains("https://ww.packyapi.com"))
        .expect("speedtest result URL should be rendered");
    let row = line_at(&buf, row_index);
    let msg_start = row
        .find("https://ww.packyapi.com")
        .expect("message should be present");
    let left_border = row[..msg_start]
        .rfind('│')
        .expect("message row should have left border");
    let right_border_offset = row[msg_start + "https://ww.packyapi.com".len()..]
        .find('│')
        .expect("message row should have right border");
    let right_border = msg_start + "https://ww.packyapi.com".len() + right_border_offset;
    let overlay_width = right_border.saturating_sub(left_border).saturating_add(1);

    assert!(
        msg_start.saturating_sub(left_border) > 2,
        "result should not hug left border: {row:?}"
    );
    assert!(
        right_border.saturating_sub(msg_start + "https://ww.packyapi.com".len()) > 2,
        "result should not hug right border: {row:?}"
    );
    assert!(
        overlay_width < 70,
        "short result overlay should be compact, got width {overlay_width}"
    );
}

#[test]
fn stream_check_result_overlay_is_compact_when_lines_are_short() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::ProviderDetail {
        id: "p1".to_string(),
    };
    app.focus = Focus::Content;
    app.overlay = Overlay::StreamCheckResult {
        provider_name: "Packy".to_string(),
        lines: vec![
            texts::tui_stream_check_line_provider("Packy"),
            texts::tui_stream_check_line_status("OK"),
            texts::tui_stream_check_line_response_time("367 ms"),
            texts::tui_stream_check_line_http_status("200"),
        ],
        scroll: 0,
    };
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let row_index = (0..buf.area.height)
        .find(|&y| line_at(&buf, y).contains("367 ms"))
        .expect("stream check result should be rendered");
    let row = line_at(&buf, row_index);
    let msg_start = row.find("367 ms").expect("message should be present");
    let left_border = row[..msg_start]
        .rfind('│')
        .expect("message row should have left border");
    let right_border_offset = row[msg_start + "367 ms".len()..]
        .find('│')
        .expect("message row should have right border");
    let right_border = msg_start + "367 ms".len() + right_border_offset;
    let overlay_width = right_border.saturating_sub(left_border).saturating_add(1);

    assert!(
        msg_start.saturating_sub(left_border) > 2,
        "result should not hug left border: {row:?}"
    );
    assert!(
        right_border.saturating_sub(msg_start + "367 ms".len()) > 2,
        "result should not hug right border: {row:?}"
    );
    assert!(
        overlay_width < 70,
        "short result overlay should be compact, got width {overlay_width}"
    );
}

#[test]
fn speedtest_result_overlay_leaves_gap_below_keybar() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.overlay = Overlay::SpeedtestResult {
        url: "https://ww.packyapi.com".to_string(),
        lines: vec![
            texts::tui_speedtest_line_url("https://ww.packyapi.com"),
            String::new(),
            texts::tui_speedtest_line_latency("367 ms"),
            texts::tui_speedtest_line_status("200"),
        ],
        scroll: 0,
    };
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let key_row = (0..buf.area.height)
        .find(|&y| line_at(&buf, y).contains("Esc"))
        .expect("key row should be rendered");
    let content_row = (0..buf.area.height)
        .find(|&y| line_at(&buf, y).contains("https://ww.packyapi.com"))
        .expect("content row should be rendered");

    assert!(
            content_row > key_row + 1,
            "content should leave a blank row below key hints: key_row={key_row}, content_row={content_row}"
        );
}

#[test]
fn stream_check_running_overlay_leaves_gap_below_keybar() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::ProviderDetail {
        id: "p1".to_string(),
    };
    app.focus = Focus::Content;
    app.overlay = Overlay::StreamCheckRunning {
        provider_id: "p1".to_string(),
        provider_name: "Demo".to_string(),
    };
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let message = texts::tui_stream_check_running("Demo");
    let key_row = (0..buf.area.height)
        .find(|&y| line_at(&buf, y).contains("Esc"))
        .expect("key row should be rendered");
    let content_row = (0..buf.area.height)
        .find(|&y| line_at(&buf, y).contains(&message))
        .expect("content row should be rendered");

    assert!(
            content_row > key_row + 1,
            "content should leave a blank row below key hints: key_row={key_row}, content_row={content_row}"
        );
}

#[test]
fn backup_picker_overlay_shows_hint() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Config;
    app.focus = Focus::Content;
    app.overlay = Overlay::BackupPicker { selected: 0 };

    let mut data = minimal_data(&app.app_type);
    data.config.backups = vec![crate::services::config::BackupInfo {
        id: "b1".to_string(),
        path: std::path::PathBuf::from("/tmp/b1.json"),
        timestamp: "20260131_000000".to_string(),
        display_name: "backup".to_string(),
    }];

    let buf = render(&app, &data);

    let mut all = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            all.push_str(buf[(x, y)].symbol());
        }
        all.push('\n');
    }

    assert!(
        all.contains("Enter")
            && all.contains("Esc")
            && (all.contains("restore") || all.contains("恢复")),
        "expected BackupPicker to show Enter/Esc restore hint"
    );
}

#[test]
fn provider_form_model_field_enter_hint_uses_fetch_model() {
    let keys =
        super::add_form_key_items(FormFocus::Fields, false, Some(ProviderAddField::CodexModel));
    let enter_label = keys
        .iter()
        .find(|(key, _label)| *key == "Enter")
        .map(|(_key, label)| *label);
    assert_eq!(enter_label, Some(texts::tui_key_fetch_model()));
}

#[test]
fn provider_detail_key_bar_shows_stream_check_hint() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::ProviderDetail {
        id: "p1".to_string(),
    };
    app.focus = Focus::Content;
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let mut all = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            all.push_str(buf[(x, y)].symbol());
        }
        all.push('\n');
    }

    assert!(all.contains("stream check"));
}

#[test]
fn provider_detail_keys_line_does_not_include_q_back() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::ProviderDetail {
        id: "p1".to_string(),
    };
    app.focus = Focus::Content;
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let mut all = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            all.push_str(buf[(x, y)].symbol());
        }
        all.push('\n');
    }

    assert!(all.contains("speedtest"));
    assert!(
        !all.contains("q=back"),
        "provider detail inline keys should not include q=back"
    );
}
