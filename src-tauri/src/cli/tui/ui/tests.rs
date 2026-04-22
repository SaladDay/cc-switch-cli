use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    backend::TestBackend,
    buffer::Buffer,
    style::{Color, Modifier},
    Terminal,
};
use serde_json::json;
use serial_test::serial;
use std::ffi::OsString;
use std::path::Path;
use std::sync::Mutex;
use tempfile::TempDir;
use unicode_width::UnicodeWidthStr;

use crate::{
    app_config::AppType,
    cli::i18n::{texts, use_test_language, Language},
    cli::tui::{
        app,
        app::{
            Action, App, ConfigItem, ConfirmAction, ConfirmOverlay, EditorKind, EditorSubmit,
            Focus, Overlay, TextInputState, TextSubmit,
        },
        data::{
            ConfigSnapshot, McpSnapshot, OpenClawWorkspaceSnapshot, PromptsSnapshot, ProviderRow,
            ProvidersSnapshot, ProxySnapshot, SkillsSnapshot, UiData,
        },
        form::{FormFocus, ProviderAddField},
        route::{NavItem, Route},
        theme::theme_for,
    },
    commands::workspace::{DailyMemoryFileInfo, ALLOWED_FILES},
    openclaw_config::write_openclaw_config_source,
    provider::Provider,
    services::skill::{InstalledSkill, SkillApps, SkillRepo, SyncMethod, UnmanagedSkill},
    test_support::{lock_test_home_and_settings, set_test_home_override, TestHomeSettingsLock},
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
fn openclaw_tui_form_masks_api_key_in_default_view() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::Providers;
    app.focus = Focus::Content;

    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::OpenClaw);
    form.focus = FormFocus::Fields;
    form.id.set("p1");
    form.name.set("Saved Snapshot Name");
    form.opencode_api_key.set("sk-openclaw-secret");
    form.opencode_base_url
        .set("https://api.openclaw.example/v1");
    form.field_idx = form
        .fields()
        .iter()
        .position(|field| *field == ProviderAddField::OpenCodeApiKey)
        .expect("OpenClaw API key field should exist");
    app.form = Some(crate::cli::tui::form::FormState::ProviderAdd(form));

    let all = all_text(&render(&app, &minimal_data(&app.app_type)));

    assert!(all.contains("[redacted]"), "{all}");
    assert!(!all.contains("sk-openclaw-secret"), "{all}");
}

#[test]
fn redact_sensitive_json_keeps_non_secret_token_count_fields_visible() {
    let value = json!({
        "env": {
            "AWS_SECRET_ACCESS_KEY": "aws-secret-value",
            "AWS_ACCESS_KEY_ID": "AKIA1234567890"
        },
        "models": [
            {
                "maxTokens": 8192,
                "apiKey": "sk-openclaw-secret"
            }
        ],
        "tokenLimit": 12345,
        "password": "hidden"
    });

    let redacted = super::redact_sensitive_json(&value);

    assert_eq!(
        redacted["env"]["AWS_SECRET_ACCESS_KEY"],
        json!("[redacted]")
    );
    assert_eq!(redacted["env"]["AWS_ACCESS_KEY_ID"], json!("[redacted]"));
    assert_eq!(redacted["models"][0]["maxTokens"], json!(8192));
    assert_eq!(redacted["tokenLimit"], json!(12345));
    assert_eq!(redacted["models"][0]["apiKey"], json!("[redacted]"));
    assert_eq!(redacted["password"], json!("[redacted]"));
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

pub(super) fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    match ENV_LOCK.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub(super) struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
}

pub(super) struct SettingsEnvGuard {
    _lock: TestHomeSettingsLock,
    old_home: Option<OsString>,
    old_userprofile: Option<OsString>,
}

impl SettingsEnvGuard {
    pub(super) fn set_home(home: &Path) -> Self {
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

impl Drop for SettingsEnvGuard {
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

impl EnvGuard {
    pub(super) fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, prev }
    }

    pub(super) fn remove(key: &'static str) -> Self {
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

pub(super) fn render(app: &App, data: &UiData) -> Buffer {
    render_with_size(app, data, 120, 40)
}

pub(super) fn render_with_size(app: &App, data: &UiData, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal created");
    terminal
        .draw(|f| super::render(f, app, data))
        .expect("draw ok");
    terminal.backend().buffer().clone()
}

pub(super) fn line_at(buf: &Buffer, y: u16) -> String {
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

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn nav_text(app: &App, buf: &Buffer) -> String {
    let theme = theme_for(&app.app_type);
    let nav_width = super::nav_pane_width(&theme);
    let mut all = String::new();
    for y in 0..buf.area.height {
        for x in 0..nav_width.min(buf.area.width) {
            all.push_str(buf[(x, y)].symbol());
        }
        all.push('\n');
    }
    all
}

fn content_text(app: &App, buf: &Buffer) -> String {
    let theme = theme_for(&app.app_type);
    let nav_width = super::nav_pane_width(&theme).min(buf.area.width);
    let mut all = String::new();
    for y in 0..buf.area.height {
        for x in nav_width..buf.area.width {
            all.push_str(buf[(x, y)].symbol());
        }
        all.push('\n');
    }
    all
}

fn nav_label_text(item: NavItem) -> String {
    buffer_cell_text(super::nav_label(item))
}

fn nav_title_text(item: NavItem) -> &'static str {
    super::split_nav_label(super::nav_label(item)).1
}

pub(super) fn spaces_before_substring(text: &str, needle: &str) -> usize {
    let idx = text.find(needle).expect("substring should exist");
    text.as_bytes()[..idx]
        .iter()
        .rev()
        .take_while(|byte| **byte == b' ')
        .count()
}

pub(super) fn buffer_cell_text(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        out.push(ch);
        if unicode_width::UnicodeWidthChar::width(ch) == Some(2) {
            out.push(' ');
        }
    }
    out
}

fn line_index(text: &str, needle: &str) -> usize {
    text.lines()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("missing `{needle}` in:\n{text}"))
}

fn line_with<'a>(text: &'a str, needle: &str) -> &'a str {
    text.lines()
        .find(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("missing `{needle}` in:\n{text}"))
}

fn column_in_line(line: &str, needle: &str) -> usize {
    line.find(needle)
        .unwrap_or_else(|| panic!("missing `{needle}` in line:\n{line}"))
}

fn display_column_in_line(line: &str, needle: &str) -> usize {
    let byte_idx = column_in_line(line, needle);
    UnicodeWidthStr::width(&line[..byte_idx])
}

fn content_origin_x(app: &App, buf: &Buffer) -> u16 {
    let theme = theme_for(&app.app_type);
    super::nav_pane_width(&theme).min(buf.area.width)
}

fn content_cell_at<'a>(
    app: &App,
    buf: &'a Buffer,
    content_x: usize,
    content_y: usize,
) -> &'a ratatui::buffer::Cell {
    &buf[(
        content_origin_x(app, buf) + content_x as u16,
        content_y as u16,
    )]
}

fn cell_style_signature(cell: &ratatui::buffer::Cell) -> (Color, Color, Modifier) {
    (cell.fg, cell.bg, cell.modifier)
}

fn block_title_needle(title: &str) -> String {
    format!("┌{}", buffer_cell_text(title))
}

fn block_label_needle(label: &str) -> String {
    format!("│ {}", buffer_cell_text(label))
}

fn has_visible_action_button_or_block(text: &str, label: &str) -> bool {
    let label = buffer_cell_text(label);
    let selected_label = format!("> {label}");
    let block_title = format!("┌{label}");

    text.lines().any(|line| {
        let trimmed = line.trim_matches(|ch| ch == ' ' || ch == '│');
        line.contains(&block_title) || trimmed == label || trimmed == selected_label
    })
}

pub(super) fn visible_tab_labels(header: &str) -> usize {
    [
        AppType::Claude.as_str(),
        AppType::Codex.as_str(),
        AppType::Gemini.as_str(),
        AppType::OpenCode.as_str(),
        AppType::OpenClaw.as_str(),
    ]
    .into_iter()
    .filter(|label| header.contains(label))
    .count()
}

pub(super) fn minimal_data(_app_type: &AppType) -> UiData {
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
                is_in_config: true,
                is_saved: true,
                is_default_model: false,
                primary_model_id: Some("claude-sonnet-4".to_string()),
                default_model_id: None,
            }],
        },
        mcp: McpSnapshot::default(),
        prompts: PromptsSnapshot::default(),
        config: ConfigSnapshot::default(),
        skills: SkillsSnapshot::default(),
        proxy: ProxySnapshot::default(),
    }
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
#[serial(home_settings)]
fn header_hides_gemini_by_default() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");
    let temp_home = TempDir::new().expect("create temp home");
    let _home = SettingsEnvGuard::set_home(temp_home.path());

    let app = App::new(Some(AppType::Claude));
    let buf = render(&app, &minimal_data(&app.app_type));
    let header = line_at(&buf, 1);

    assert!(header.contains(AppType::Claude.as_str()), "{header}");
    assert!(header.contains(AppType::Codex.as_str()), "{header}");
    assert!(!header.contains(AppType::Gemini.as_str()), "{header}");
    assert!(header.contains(AppType::OpenCode.as_str()), "{header}");
    assert!(header.contains(AppType::OpenClaw.as_str()), "{header}");
    assert_eq!(visible_tab_labels(&header), 4, "{header}");
}

#[test]
#[serial(home_settings)]
fn header_only_renders_selected_visible_apps() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");
    let temp_home = TempDir::new().expect("create temp home");
    let _home = SettingsEnvGuard::set_home(temp_home.path());
    crate::settings::set_visible_apps(crate::settings::VisibleApps {
        claude: false,
        codex: true,
        gemini: false,
        opencode: false,
        openclaw: true,
    })
    .expect("save visible apps");

    let app = App::new(Some(AppType::OpenClaw));
    let buf = render(&app, &minimal_data(&app.app_type));
    let header = line_at(&buf, 1);

    assert!(!header.contains(AppType::Claude.as_str()), "{header}");
    assert!(header.contains(AppType::Codex.as_str()), "{header}");
    assert!(!header.contains(AppType::Gemini.as_str()), "{header}");
    assert!(!header.contains(AppType::OpenCode.as_str()), "{header}");
    assert!(header.contains(AppType::OpenClaw.as_str()), "{header}");
    assert_eq!(visible_tab_labels(&header), 2, "{header}");
}

#[test]
#[serial(home_settings)]
fn header_keeps_all_app_tabs_visible_with_proxy_chip() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");
    let temp_home = TempDir::new().expect("create temp home");
    let _home = SettingsEnvGuard::set_home(temp_home.path());
    crate::settings::set_visible_apps(crate::settings::VisibleApps {
        claude: true,
        codex: true,
        gemini: true,
        opencode: true,
        openclaw: true,
    })
    .expect("save visible apps");

    let app = App::new(Some(AppType::Claude));
    let buf = render(&app, &minimal_data(&app.app_type));
    let header = line_at(&buf, 1);

    assert!(header.contains(texts::tui_app_title()), "{header}");
    assert!(header.contains(AppType::Claude.as_str()), "{header}");
    assert!(header.contains(AppType::Codex.as_str()), "{header}");
    assert!(header.contains(AppType::Gemini.as_str()), "{header}");
    assert!(header.contains(AppType::OpenCode.as_str()), "{header}");
    assert!(header.contains(AppType::OpenClaw.as_str()), "{header}");
}

#[test]
#[serial(home_settings)]
fn settings_page_shows_visible_apps_row_value() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");
    let temp_home = TempDir::new().expect("create temp home");
    let _home = SettingsEnvGuard::set_home(temp_home.path());
    crate::settings::set_visible_apps(crate::settings::VisibleApps {
        claude: true,
        codex: false,
        gemini: true,
        opencode: false,
        openclaw: true,
    })
    .expect("save visible apps");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Settings;
    app.focus = Focus::Content;

    let all = all_text(&render(&app, &minimal_data(&app.app_type)));

    assert!(
        all.contains(texts::tui_settings_visible_apps_label()),
        "{all}"
    );
    assert!(all.contains("claude, gemini, openclaw"), "{all}");
}

#[test]
#[serial(home_settings)]
fn settings_page_shows_openclaw_config_dir_default_value() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");
    let temp_home = TempDir::new().expect("create temp home");
    let _home = SettingsEnvGuard::set_home(temp_home.path());

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Settings;
    app.focus = Focus::Content;

    let all = all_text(&render(&app, &minimal_data(&app.app_type)));

    assert!(
        all.contains(texts::tui_settings_openclaw_config_dir_label()),
        "{all}"
    );
    assert!(
        all.contains(texts::tui_settings_openclaw_config_dir_default_value()),
        "{all}"
    );
}

#[test]
#[serial(home_settings)]
fn settings_page_shows_openclaw_config_dir_override_value() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");
    let temp_home = TempDir::new().expect("create temp home");
    let _home = SettingsEnvGuard::set_home(temp_home.path());
    let mut settings = crate::settings::get_settings();
    settings.openclaw_config_dir = Some(r"\\wsl$\Ubuntu\home\demo\.openclaw".to_string());
    crate::settings::update_settings(settings).expect("save openclaw override");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Settings;
    app.focus = Focus::Content;

    let all = all_text(&render(&app, &minimal_data(&app.app_type)));

    assert!(
        all.contains(texts::tui_settings_openclaw_config_dir_label()),
        "{all}"
    );
    assert!(all.contains(r"\\wsl$\Ubuntu\home\demo\.openclaw"), "{all}");
}

#[test]
fn zero_selection_warning_toast_renders_after_picker_rejection() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Settings;
    app.focus = Focus::Content;
    app.overlay = Overlay::VisibleAppsPicker {
        selected: 0,
        apps: crate::settings::VisibleApps {
            claude: false,
            codex: false,
            gemini: false,
            opencode: false,
            openclaw: false,
        },
    };
    app.push_toast(
        texts::tui_toast_visible_apps_zero_selection_warning(),
        crate::cli::tui::app::ToastKind::Warning,
    );

    let all = all_text(&render(&app, &minimal_data(&app.app_type)));

    assert!(
        all.contains(texts::tui_settings_visible_apps_title()),
        "{all}"
    );
    assert!(all.contains(AppType::OpenClaw.as_str()), "{all}");
    assert!(
        all.contains(texts::tui_toast_visible_apps_zero_selection_warning()),
        "{all}"
    );
}

#[test]
fn openclaw_agents_picker_overlay_marks_current_option_when_editing_existing_fallback() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawAgents;
    app.focus = Focus::Content;
    app.openclaw_agents_form = Some(app::OpenClawAgentsFormState {
        primary_model: "demo/primary".to_string(),
        fallbacks: vec!["demo/fallback-a".to_string(), "demo/fallback-b".to_string()],
        workspace: String::new(),
        timeout: String::new(),
        timeout_seconds_seed: None,
        context_tokens: String::new(),
        context_tokens_seed: None,
        max_concurrent: String::new(),
        max_concurrent_seed: None,
        model_catalog: None,
        defaults_extra: std::collections::HashMap::new(),
        model_extra: std::collections::HashMap::new(),
        has_legacy_timeout: false,
        section: app::OpenClawAgentsSection::FallbackModels,
        row: 1,
    });
    app.overlay = Overlay::OpenClawAgentsFallbackPicker {
        insert_at: 1,
        selected: 1,
        options: vec![
            app::OpenClawModelOption {
                value: "demo/fallback-b".to_string(),
                label: "Demo Provider / 回退 B".to_string(),
            },
            app::OpenClawModelOption {
                value: "demo/fallback-c".to_string(),
                label: "Demo Provider / 回退 C".to_string(),
            },
        ],
    };

    let all = all_text(&render(&app, &minimal_data(&app.app_type)));

    assert!(
        all.contains(&buffer_cell_text(
            texts::tui_openclaw_agents_fallback_models()
        )),
        "{all}"
    );
    assert!(
        all.contains(&buffer_cell_text(&format!(
            "{}  Demo Provider / 回退 B",
            texts::tui_marker_active()
        ))),
        "{all}"
    );
    assert!(
        all.contains(&buffer_cell_text("Demo Provider / 回退 C")),
        "{all}"
    );
}

#[test]
fn header_centers_tabs_when_room_allows() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let app = App::new(Some(AppType::Claude));
    let buf = render_with_size(&app, &minimal_data(&app.app_type), 140, 40);
    let header = line_at(&buf, 1);
    let title_idx = header
        .find(texts::tui_app_title())
        .expect("title should render");
    let title_end = title_idx + texts::tui_app_title().len();
    let proxy_idx = header
        .find(texts::tui_header_proxy_status(false).as_str())
        .expect("proxy badge should render");
    let lane = &header[title_end..proxy_idx];
    let first_label = lane
        .find(AppType::Claude.as_str())
        .expect("claude tab should render");
    let last_label_end = lane
        .rfind(AppType::OpenClaw.as_str())
        .map(|idx| idx + AppType::OpenClaw.as_str().len())
        .expect("openclaw tab should render");
    let left_gap = first_label;
    let right_gap = lane.len().saturating_sub(last_label_end);

    assert!(
        left_gap.abs_diff(right_gap) <= 2,
        "expected tabs to stay centered inside the middle lane, got: {header}"
    );
}

#[test]
fn header_keeps_title_and_right_badges_visible_without_large_gap_in_chinese() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let app = App::new(Some(AppType::Claude));
    let mut data = minimal_data(&app.app_type);
    data.providers.rows[0].is_current = true;

    let buf = render_with_size(&app, &data, 96, 40);
    let header = line_at(&buf, 1);
    let proxy_label = buffer_cell_text(&texts::tui_header_proxy_status(false));
    let provider_label = buffer_cell_text(&format!(
        "{}: {}",
        texts::provider_label().trim_end_matches([':', '：']),
        "Demo Provider"
    ));

    assert!(header.contains(texts::tui_app_title()), "{header}");
    assert!(header.contains(&proxy_label), "{header}");
    assert!(header.contains(&provider_label), "{header}");
    assert!(
        spaces_before_substring(&header, &proxy_label) <= 7,
        "expected proxy badge to stay near tabs without a fake blank block: {header}"
    );
}

#[test]
fn header_narrow_width_collapses_center_before_creating_fake_gap() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let app = App::new(Some(AppType::Claude));
    let buf = render_with_size(&app, &minimal_data(&app.app_type), 32, 20);
    let header = line_at(&buf, 1);
    let proxy_label = buffer_cell_text(&texts::tui_header_proxy_status(false));

    assert!(header.contains(texts::tui_app_title()), "{header}");
    assert!(header.contains(&proxy_label), "{header}");
    assert!(
        spaces_before_substring(&header, &proxy_label) <= 4,
        "expected center tabs to collapse before a fake blank gap appears: {header}"
    );
}

#[test]
fn header_sacrifices_tabs_before_truncating_right_badges() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let app = App::new(Some(AppType::Claude));
    let mut data = minimal_data(&app.app_type);
    data.providers.rows[0].is_current = true;

    let title_width = UnicodeWidthStr::width(format!("  {}", texts::tui_app_title()).as_str());
    let proxy_badge_width =
        UnicodeWidthStr::width(format!("  {}  ", texts::tui_header_proxy_status(false)).as_str());
    let provider_badge_width = UnicodeWidthStr::width(
        format!(
            "  {}: {}  ",
            texts::provider_label().trim_end_matches([':', '：']),
            "Demo Provider"
        )
        .as_str(),
    );
    let total_width = (title_width + proxy_badge_width + 1 + provider_badge_width + 2) as u16;

    let buf = render_with_size(&app, &data, total_width, 40);
    let header = line_at(&buf, 1);
    let proxy_label = texts::tui_header_proxy_status(false);
    let provider_label = format!(
        "{}: {}",
        texts::provider_label().trim_end_matches([':', '：']),
        "Demo Provider"
    );

    assert!(header.contains(texts::tui_app_title()), "{header}");
    assert!(header.contains(&proxy_label), "{header}");
    assert!(header.contains(&provider_label), "{header}");
    assert_eq!(
        visible_tab_labels(&header),
        0,
        "expected tabs to yield before right badges truncate: {header}"
    );
}

#[test]
fn header_keeps_proxy_visible_and_truncates_long_provider_name() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let app = App::new(Some(AppType::Claude));
    let mut data = minimal_data(&app.app_type);
    data.providers.rows[0].is_current = true;
    data.providers.rows[0].provider = Provider::with_id(
        "p1".to_string(),
        "Demo Provider With An Extremely Long Name That Must Truncate Before It Hides The Proxy Badge"
            .to_string(),
        json!({}),
        None,
    );

    let buf = render_with_size(&app, &data, 80, 40);
    let header = line_at(&buf, 1);
    let proxy_label = texts::tui_header_proxy_status(false);

    assert!(header.contains(texts::tui_app_title()), "{header}");
    assert!(header.contains(&proxy_label), "{header}");
    assert!(header.contains("Provider:"), "{header}");
    assert!(header.contains("Demo"), "{header}");
    assert!(header.contains('…'), "{header}");
    assert!(
        !header.contains(
            "Demo Provider With An Extremely Long Name That Must Truncate Before It Hides The Proxy Badge"
        ),
        "{header}"
    );
    assert!(
        spaces_before_substring(&header, &proxy_label) <= 6,
        "expected long provider names to truncate instead of reserving a fake gap: {header}"
    );
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
                hermes: false,
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
                    hermes: false,
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
                    hermes: false,
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
fn form_save_before_close_confirm_overlay_shows_save_exit_and_cancel_actions() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);

    let prev = std::env::var("NO_COLOR").ok();
    std::env::set_var("NO_COLOR", "1");
    let _restore_no_color = EnvGuard {
        key: "NO_COLOR",
        prev,
    };

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.overlay = Overlay::Confirm(ConfirmOverlay {
        title: texts::tui_editor_save_before_close_title().to_string(),
        message: texts::tui_editor_save_before_close_message().to_string(),
        action: ConfirmAction::FormSaveBeforeClose,
    });
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let theme = theme_for(&app.app_type);
    let content = super::content_pane_rect(buf.area, &theme);
    let area = super::centered_rect_fixed(
        super::OVERLAY_FIXED_MD.0,
        super::OVERLAY_FIXED_MD.1,
        content,
    );

    let key_bar_row = (area.y..area.y.saturating_add(area.height))
        .map(|y| line_at(&buf, y))
        .find(|line| line.contains("Enter") && line.contains("Esc"))
        .expect("confirm overlay should render a key-bar row");
    let enter_hint = format!("Enter={}", texts::tui_key_save_and_exit());
    let n_hint = format!("N={}", texts::tui_key_exit_without_save());
    let esc_hint = format!("Esc={}", texts::tui_key_cancel());

    assert!(key_bar_row.contains(&enter_hint), "{key_bar_row}");
    assert!(key_bar_row.contains(&n_hint), "{key_bar_row}");
    assert!(key_bar_row.contains(&esc_hint), "{key_bar_row}");
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
fn provider_switch_first_use_overlay_renders_three_actions_with_padding() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.overlay = Overlay::ProviderSwitchFirstUseConfirm {
        provider_id: "p1".to_string(),
        title: texts::tui_provider_switch_first_use_title().to_string(),
        message: texts::tui_provider_switch_first_use_message("~/.claude/settings.json"),
        selected: 0,
    };

    let data = minimal_data(&app.app_type);
    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(
        all.contains(texts::tui_provider_switch_first_use_import_button()),
        "expected import action in warning overlay: {all}"
    );
    assert!(
        all.contains(texts::tui_provider_switch_first_use_continue_button()),
        "expected continue action in warning overlay: {all}"
    );
    assert!(
        all.contains(texts::tui_provider_switch_first_use_cancel_button()),
        "expected cancel action in warning overlay: {all}"
    );

    let theme = theme_for(&app.app_type);
    let content = super::content_pane_rect(buf.area, &theme);
    let area = super::centered_rect_fixed(72, 12, content);

    assert_eq!(buf[(area.x, area.y)].symbol(), "┌");
    assert_eq!(
        buf[(
            area.x.saturating_add(area.width.saturating_sub(1)),
            area.y.saturating_add(area.height.saturating_sub(1))
        )]
            .symbol(),
        "┘"
    );

    let button_row = (0..buf.area.height)
        .find(|&y| {
            let row = line_at(&buf, y);
            row.contains(texts::tui_provider_switch_first_use_import_button())
                && row.contains(texts::tui_provider_switch_first_use_continue_button())
                && row.contains(texts::tui_provider_switch_first_use_cancel_button())
        })
        .expect("warning overlay buttons should be rendered");
    assert!(
        button_row > area.y.saturating_add(3),
        "buttons should not hug the top border"
    );
    assert!(
        area.y
            .saturating_add(area.height)
            .saturating_sub(button_row)
            >= 3,
        "buttons should keep visible bottom margin"
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
fn openclaw_config_route_render_uses_dedicated_env_page() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawEnv;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_env = Some(crate::openclaw_config::OpenClawEnvConfig {
        vars: std::collections::HashMap::from([
            ("OPENCLAW_ENV_TOKEN".to_string(), json!("demo-token")),
            ("OPENCLAW_ENV_MODE".to_string(), json!("development")),
        ]),
    });
    data.config.openclaw_warnings = Some(vec![crate::openclaw_config::OpenClawHealthWarning {
        code: "stringified_env_vars".to_string(),
        message: "env.vars should be an object".to_string(),
        path: Some("env.vars".to_string()),
    }]);

    let buf = render(&app, &data);
    let all = all_text(&buf);
    let content = content_text(&app, &buf);

    assert!(matches!(
        ConfigItem::from_openclaw_route(&app.route),
        Some(ConfigItem::OpenClawEnv)
    ));
    assert!(all.contains(
        ConfigItem::OpenClawEnv
            .detail_title()
            .expect("OpenClaw Env route should have a title")
    ));
    assert!(all.contains(texts::tui_openclaw_config_env_description()));
    assert!(
        line_with(&all, "OPENCLAW_ENV_TOKEN").contains("[redacted]"),
        "{all}"
    );
    assert!(
        !line_with(&all, "OPENCLAW_ENV_TOKEN").contains("demo-token"),
        "{all}"
    );
    assert!(
        line_with(&all, "OPENCLAW_ENV_MODE").contains("development"),
        "{all}"
    );
    assert!(
        !has_visible_action_button_or_block(&content, texts::tui_openclaw_tools_save_label()),
        "{all}"
    );
    assert!(all.contains(texts::tui_openclaw_config_warning_title()));
    assert!(all.contains("env.vars"));
    assert!(
        !all.contains(texts::tui_openclaw_config_file_label()),
        "{all}"
    );
    assert!(
        !all.contains(texts::tui_openclaw_config_section_label()),
        "{all}"
    );
    assert!(
        !all.contains(texts::tui_openclaw_config_env_editor_title()),
        "{all}"
    );
    assert!(!all.contains("\"OPENCLAW_ENV_TOKEN\""), "{all}");
    assert!(!all.contains(super::config_item_label(&ConfigItem::ShowFull)));
}

#[test]
fn openclaw_env_route_render_aligns_redacted_and_plain_values_in_two_columns() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawEnv;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_env = Some(crate::openclaw_config::OpenClawEnvConfig {
        vars: std::collections::HashMap::from([
            ("OPENCLAW_ENV_TOKEN".to_string(), json!("demo-token")),
            ("MODE".to_string(), json!("development")),
        ]),
    });

    let buf = render_with_size(&app, &data, 120, 24);
    let content = content_text(&app, &buf);
    let redacted_line = line_with(&content, "OPENCLAW_ENV_TOKEN");
    let plain_line = line_with(&content, "MODE");

    assert_eq!(
        display_column_in_line(redacted_line, "[redacted]"),
        display_column_in_line(plain_line, "development"),
        "{content}"
    );
}

#[test]
fn openclaw_env_route_render_uses_explicit_empty_state_copy() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawEnv;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_env = Some(crate::openclaw_config::OpenClawEnvConfig {
        vars: std::collections::HashMap::new(),
    });

    let content = content_text(&app, &render_with_size(&app, &data, 120, 24));

    assert!(
        content.contains("No environment variables configured"),
        "{content}"
    );
    assert!(
        !content.contains(&format!("  {}", texts::none())),
        "{content}"
    );
}

#[test]
fn openclaw_env_route_render_keeps_warning_banner_separated_above_description_and_rows() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawEnv;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_env = Some(crate::openclaw_config::OpenClawEnvConfig {
        vars: std::collections::HashMap::from([(
            "OPENCLAW_ENV_MODE".to_string(),
            json!("development"),
        )]),
    });
    data.config.openclaw_warnings = Some(vec![crate::openclaw_config::OpenClawHealthWarning {
        code: "stringified_env_vars".to_string(),
        message: "Environment variables warning order contract".to_string(),
        path: Some("env.vars".to_string()),
    }]);

    let buf = render_with_size(&app, &data, 120, 24);
    let content = content_text(&app, &buf);
    let warning_title_line = line_index(&content, texts::tui_openclaw_config_warning_title());
    let warning_detail_line = line_index(&content, "Environment variables warning order contract");
    let description_line = line_index(&content, texts::tui_openclaw_config_env_description());
    let env_row_line = line_index(&content, "OPENCLAW_ENV_MODE");

    assert!(warning_title_line < description_line, "{content}");
    assert!(warning_detail_line + 1 < description_line, "{content}");
    assert!(description_line < env_row_line, "{content}");
}

#[test]
fn openclaw_env_route_render_keeps_description_spacer_before_env_block() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawEnv;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_env = Some(crate::openclaw_config::OpenClawEnvConfig {
        vars: std::collections::HashMap::from([(
            "OPENCLAW_ENV_MODE".to_string(),
            json!("development"),
        )]),
    });

    let rendered = render_with_size(&app, &data, 120, 24);
    let content = content_text(&app, &rendered);
    let lines = content.lines().collect::<Vec<_>>();
    let description_index = line_index(&content, texts::tui_openclaw_config_env_description());
    let block_index = lines
        .iter()
        .enumerate()
        .skip(description_index + 1)
        .find_map(|(index, line)| line.contains('┌').then_some(index))
        .expect("env block should render after description");
    let spacer = lines[description_index + 1];

    assert!(description_index + 1 < block_index, "{content}");
    assert!(spacer.chars().all(|ch| ch == ' ' || ch == '│'), "{content}");
}

#[test]
fn openclaw_env_description_uses_explicit_muted_style() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawEnv;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_env = Some(crate::openclaw_config::OpenClawEnvConfig {
        vars: std::collections::HashMap::from([(
            "OPENCLAW_ENV_MODE".to_string(),
            json!("development"),
        )]),
    });

    let buf = render_with_size(&app, &data, 120, 24);
    let content = content_text(&app, &buf);
    let theme = theme_for(&app.app_type);

    let description_row = line_with(&content, texts::tui_openclaw_config_env_description());
    let description_row_index = line_index(&content, texts::tui_openclaw_config_env_description());
    let description_start_col = column_in_line(
        description_row,
        texts::tui_openclaw_config_env_description(),
    );
    let description_end_col = column_in_line(description_row, "env.vars.");
    let description_start_cell =
        content_cell_at(&app, &buf, description_start_col, description_row_index);
    let description_end_cell =
        content_cell_at(&app, &buf, description_end_col, description_row_index);
    let description_start_style = cell_style_signature(description_start_cell);
    let description_end_style = cell_style_signature(description_end_cell);

    assert_eq!(description_start_style, description_end_style, "{content}");
    assert_eq!(description_start_cell.fg, theme.comment, "{content}");
    assert_eq!(description_start_cell.bg, Color::Reset, "{content}");
    assert_eq!(
        description_start_cell.modifier,
        Modifier::empty(),
        "{content}"
    );
}

#[test]
fn openclaw_env_route_render_styles_redacted_values_as_protected_tokens() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawEnv;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_env = Some(crate::openclaw_config::OpenClawEnvConfig {
        vars: std::collections::HashMap::from([
            ("OPENCLAW_ENV_TOKEN".to_string(), json!("demo-token")),
            ("OPENCLAW_ENV_MODE".to_string(), json!("development")),
        ]),
    });

    let buf = render_with_size(&app, &data, 120, 24);
    let content = content_text(&app, &buf);
    let redacted_row_index = line_index(&content, "[redacted]");
    let redacted_row = line_with(&content, "[redacted]");
    let redacted_label_col = column_in_line(redacted_row, "OPENCLAW_ENV_TOKEN");
    let redacted_col = column_in_line(redacted_row, "[redacted]");
    let redacted_label_cell = content_cell_at(&app, &buf, redacted_label_col, redacted_row_index);
    let redacted_cell = content_cell_at(&app, &buf, redacted_col, redacted_row_index);

    let plain_row_index = line_index(&content, "development");
    let plain_row = line_with(&content, "development");
    let plain_label_col = column_in_line(plain_row, "OPENCLAW_ENV_MODE");
    let plain_col = column_in_line(plain_row, "development");
    let plain_label_cell = content_cell_at(&app, &buf, plain_label_col, plain_row_index);
    let plain_cell = content_cell_at(&app, &buf, plain_col, plain_row_index);

    assert_eq!(
        cell_style_signature(plain_cell),
        cell_style_signature(plain_label_cell),
        "{content}"
    );
    assert_ne!(
        cell_style_signature(redacted_cell),
        cell_style_signature(redacted_label_cell),
        "{content}"
    );
    assert_ne!(
        cell_style_signature(redacted_cell),
        cell_style_signature(plain_cell),
        "{content}"
    );
}

#[test]
fn openclaw_env_no_color_keeps_protected_placeholder_visible() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::set("NO_COLOR", "1");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawEnv;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_env = Some(crate::openclaw_config::OpenClawEnvConfig {
        vars: std::collections::HashMap::from([
            ("OPENCLAW_ENV_TOKEN".to_string(), json!("demo-token")),
            ("OPENCLAW_ENV_MODE".to_string(), json!("development")),
        ]),
    });

    let content = content_text(&app, &render_with_size(&app, &data, 120, 24));
    let redacted_line = line_with(&content, "OPENCLAW_ENV_TOKEN");
    let plain_line = line_with(&content, "OPENCLAW_ENV_MODE");

    assert!(redacted_line.contains("[redacted]"), "{content}");
    assert!(!redacted_line.contains("demo-token"), "{content}");
    assert!(plain_line.contains("development"), "{content}");
    assert!(!plain_line.contains("[redacted]"), "{content}");
}

#[test]
fn openclaw_env_editor_keeps_ctrl_s_save_hint() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawEnv;
    app.focus = Focus::Content;
    app.open_editor(
        texts::tui_openclaw_config_env_title(),
        EditorKind::Json,
        "{}\n",
        EditorSubmit::ConfigOpenClawEnv,
    );

    let all = all_text(&render(&app, &minimal_data(&app.app_type)));

    assert!(all.contains("Ctrl+S"), "{all}");
    assert!(all.contains(texts::tui_key_save()), "{all}");
}

#[test]
fn openclaw_tools_and_agents_routes_hide_ctrl_s_save_hint() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    for route in [Route::ConfigOpenClawTools, Route::ConfigOpenClawAgents] {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = route;
        app.focus = Focus::Content;

        let all = all_text(&render(&app, &minimal_data(&app.app_type)));

        assert!(!all.contains("Ctrl+S"), "{all}");
        assert!(!all.contains(texts::tui_key_save()), "{all}");
    }
}

#[test]
fn openclaw_tools_route_shows_edit_and_delete_shortcuts_for_structured_rows() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawTools;
    app.focus = Focus::Content;

    let all = all_text(&render(&app, &minimal_data(&app.app_type)));

    assert!(all.contains("Enter"), "{all}");
    assert!(all.contains("e"), "{all}");
    assert!(all.contains("Del/Backspace"), "{all}");
    assert!(all.contains(texts::tui_key_edit()), "{all}");
    assert!(all.contains(texts::tui_key_delete()), "{all}");
}

#[test]
fn openclaw_tui_config_routes_redact_sensitive_preserved_fields() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    for (route, expected_label, setup) in [
        (
            Route::ConfigOpenClawTools,
            texts::tui_openclaw_tools_extra_fields_label(),
            json!({
                "demo": {
                    "apiKey": "tools-secret"
                }
            }),
        ),
        (
            Route::ConfigOpenClawAgents,
            texts::tui_openclaw_agents_preserved_fields_label(),
            json!({
                "default": {
                    "Authorization": "Bearer agents-secret"
                }
            }),
        ),
    ] {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = route;
        app.focus = Focus::Content;

        let mut data = minimal_data(&app.app_type);
        match app.route {
            Route::ConfigOpenClawTools => {
                data.config.openclaw_tools =
                    Some(serde_json::from_value(setup.clone()).expect("valid tools section"));
            }
            Route::ConfigOpenClawAgents => {
                data.config.openclaw_agents_defaults =
                    Some(serde_json::from_value(setup.clone()).expect("valid agents section"));
            }
            _ => unreachable!(),
        }

        let all = all_text(&render(&app, &data));
        assert!(all.contains(expected_label), "{all}");
        assert!(all.contains("[redacted]"), "{all}");
        assert!(!all.contains("tools-secret"), "{all}");
        assert!(!all.contains("agents-secret"), "{all}");
    }
}

#[test]
fn openclaw_config_warning_banner_shows_backend_warning_copy() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawEnv;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_warnings = Some(vec![crate::openclaw_config::OpenClawHealthWarning {
        code: "stringified_env_vars".to_string(),
        message: "backend warning copy from scanner".to_string(),
        path: Some("env.vars".to_string()),
    }]);

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains(texts::tui_openclaw_config_warning_title()));
    assert!(all.contains("backend warning copy from scanner"));
    assert!(all.contains("env.vars"));
}

#[test]
fn openclaw_config_warning_banner_hides_when_health_scan_is_clean() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawTools;
    app.focus = Focus::Content;

    let data = minimal_data(&app.app_type);
    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(!all.contains(texts::tui_openclaw_config_warning_title()));
}

#[test]
fn openclaw_config_warning_global_banner_is_visible_on_all_subroutes() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    for route in [
        Route::ConfigOpenClawEnv,
        Route::ConfigOpenClawTools,
        Route::ConfigOpenClawAgents,
    ] {
        let config_path = "/tmp/openclaw/openclaw.json";
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = route;
        app.focus = Focus::Content;

        let mut data = minimal_data(&app.app_type);
        data.config.openclaw_config_path = Some(std::path::PathBuf::from(config_path));
        data.config.openclaw_warnings = Some(vec![crate::openclaw_config::OpenClawHealthWarning {
            code: "config_parse_failed".to_string(),
            message: "OpenClaw config could not be parsed as JSON5: trailing comma".to_string(),
            path: Some(config_path.to_string()),
        }]);

        let all = all_text(&render(&app, &data));

        assert!(all.contains(texts::tui_openclaw_config_warning_title()));
        assert!(all.contains("OpenClaw config could not be parsed as JSON5"));
        assert!(all.contains(config_path));
    }
}

#[test]
fn openclaw_config_warning_banner_wraps_multiple_long_warnings_without_clipping() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    for route in [
        Route::ConfigOpenClawEnv,
        Route::ConfigOpenClawTools,
        Route::ConfigOpenClawAgents,
    ] {
        let config_path = "/tmp/openclaw/openclaw.json";
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = route;
        app.focus = Focus::Content;

        let mut data = minimal_data(&app.app_type);
        data.config.openclaw_config_path = Some(std::path::PathBuf::from(config_path));
        data.config.openclaw_warnings = Some(vec![
            crate::openclaw_config::OpenClawHealthWarning {
                code: "config_parse_failed".to_string(),
                message: "OpenClaw config warning copy keeps wrapping across narrow terminals until marker tail-one is reached".to_string(),
                path: Some(config_path.to_string()),
            },
            crate::openclaw_config::OpenClawHealthWarning {
                code: "config_parse_failed".to_string(),
                message: "A second wrapped warning should remain fully visible too so marker tail-two still renders".to_string(),
                path: Some(config_path.to_string()),
            },
        ]);

        let all = all_text(&render_with_size(&app, &data, 72, 24));

        assert!(
            all.contains(texts::tui_openclaw_config_warning_title()),
            "{all}"
        );
        assert!(all.contains("tail-one"), "{all}");
        assert!(all.contains("tail-two"), "{all}");
    }
}

#[test]
fn openclaw_tools_route_renders_upstream_copy_profile_row_and_add_rows_without_raw_json() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawTools;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
        profile: Some("coding".to_string()),
        allow: vec!["Read".to_string()],
        deny: vec!["Exec".to_string()],
        extra: std::collections::HashMap::new(),
    });

    let buf = render(&app, &data);
    let all = all_text(&buf);
    let content = content_text(&app, &buf);

    assert!(
        all.contains("Manage tool permissions in openclaw.json (allow/deny lists)"),
        "{all}"
    );
    assert!(
        line_with(&content, &buffer_cell_text("Profile: Coding")).contains("Profile: Coding"),
        "{all}"
    );
    assert!(all.contains("Allow List"), "{all}");
    assert!(all.contains("Deny List"), "{all}");
    assert!(all.contains("Read"), "{all}");
    assert!(all.contains("Exec"), "{all}");
    assert!(all.contains("+ Add allow rule"), "{all}");
    assert!(all.contains("+ Add deny rule"), "{all}");
    assert!(all.contains("Permission Profile"), "{all}");
    assert!(all.contains("Rule Lists"), "{all}");
    assert!(
        !all.contains("Not set | Minimal | Coding | Messaging | Full"),
        "{all}"
    );
    assert!(
        !all.contains(texts::tui_openclaw_config_file_label()),
        "{all}"
    );
    assert!(
        !all.contains(texts::tui_openclaw_config_section_label()),
        "{all}"
    );
    assert!(!all.contains("\"profile\""), "{all}");
}

#[test]
fn openclaw_tools_route_renders_grouped_profile_and_rules_blocks() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawTools;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
        profile: Some("unsupported-profile".to_string()),
        allow: vec!["Read".to_string(), "Glob".to_string()],
        deny: vec!["Exec".to_string()],
        extra: std::collections::HashMap::new(),
    });

    let buf = render(&app, &data);
    let all = all_text(&buf);
    let content = content_text(&app, &buf);
    let profile_block = line_index(&content, &block_title_needle("权限档位"));
    let rules_block = line_index(&content, &block_title_needle("规则列表"));
    let profile_row = line_index(
        &content,
        &buffer_cell_text("配置档位: unsupported-profile (不受支持)"),
    );
    let allow_label = line_index(
        &content,
        &block_label_needle(texts::tui_openclaw_tools_allow_list_label()),
    );
    let allow_row = line_index(&content, &buffer_cell_text("Read"));
    let allow_add = line_index(&content, &buffer_cell_text("+ 添加允许规则"));
    let allow_deny_separator = line_index(&content, &buffer_cell_text("- - - - -"));
    let deny_label = line_index(
        &content,
        &block_label_needle(texts::tui_openclaw_tools_deny_list_label()),
    );
    let deny_row = line_index(&content, &buffer_cell_text("Exec"));
    let deny_add = line_index(&content, &buffer_cell_text("+ 添加拒绝规则"));
    let unsupported_title = line_index(
        &content,
        &buffer_cell_text(texts::tui_openclaw_tools_unsupported_profile_title()),
    );

    assert!(
        content.contains(&buffer_cell_text(texts::tui_openclaw_tools_description())),
        "{all}"
    );
    assert!(profile_block < rules_block, "{all}");
    assert!(
        profile_block < profile_row
            && profile_row < unsupported_title
            && unsupported_title < rules_block,
        "{all}"
    );
    assert!(
        rules_block < allow_label
            && allow_label < allow_row
            && allow_row < allow_add
            && allow_add < allow_deny_separator
            && allow_deny_separator < deny_label
            && deny_label < deny_row
            && deny_row < deny_add,
        "{all}"
    );
    assert!(
        !all.contains("未设置 | 最小权限 | 编码 | 对话 | 完全访问"),
        "{all}"
    );
    assert!(
        !has_visible_action_button_or_block(&content, texts::tui_openclaw_tools_save_label()),
        "{all}"
    );
}

#[test]
fn openclaw_tools_route_render_keeps_description_spacer_before_profile_block() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawTools;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
        profile: Some("coding".to_string()),
        allow: vec!["Read".to_string()],
        deny: vec!["Exec".to_string()],
        extra: std::collections::HashMap::new(),
    });

    let rendered = render_with_size(&app, &data, 120, 24);
    let content = content_text(&app, &rendered);
    let lines = content.lines().collect::<Vec<_>>();
    let description_index = line_index(&content, texts::tui_openclaw_tools_description());
    let profile_block_index = line_index(
        &content,
        &block_title_needle(texts::tui_openclaw_tools_profile_block_title()),
    );
    let spacer = lines[description_index + 1];

    assert!(description_index + 1 < profile_block_index, "{content}");
    assert!(spacer.chars().all(|ch| ch == ' ' || ch == '│'), "{content}");
}

#[test]
fn openclaw_tools_description_uses_explicit_muted_style() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawTools;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
        profile: Some("coding".to_string()),
        allow: vec!["Read".to_string()],
        deny: Vec::new(),
        extra: std::collections::HashMap::new(),
    });

    let buf = render_with_size(&app, &data, 120, 24);
    let content = content_text(&app, &buf);
    let theme = theme_for(&app.app_type);

    let description_row = line_with(&content, texts::tui_openclaw_tools_description());
    let description_row_index = line_index(&content, texts::tui_openclaw_tools_description());
    let description_start_col =
        column_in_line(description_row, texts::tui_openclaw_tools_description());
    let description_end_col = column_in_line(description_row, "allow/deny lists)");
    let description_start_cell =
        content_cell_at(&app, &buf, description_start_col, description_row_index);
    let description_end_cell =
        content_cell_at(&app, &buf, description_end_col, description_row_index);
    let description_start_style = cell_style_signature(description_start_cell);
    let description_end_style = cell_style_signature(description_end_cell);

    assert_eq!(description_start_style, description_end_style, "{content}");
    assert_eq!(description_start_cell.fg, theme.comment, "{content}");
    assert_eq!(description_start_cell.bg, Color::Reset, "{content}");
    assert_eq!(
        description_start_cell.modifier,
        Modifier::empty(),
        "{content}"
    );
}

#[test]
fn openclaw_tools_primary_rules_block_styles_stay_subordinate_to_outer_pane() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawTools;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
        profile: Some("coding".to_string()),
        allow: vec!["Read".to_string()],
        deny: vec!["Exec".to_string()],
        extra: std::collections::HashMap::new(),
    });

    let buf = render_with_size(&app, &data, 120, 24);
    let content = content_text(&app, &buf);
    let theme = theme_for(&app.app_type);
    let content_rect = super::content_pane_rect(buf.area, &theme);
    let rules_title = texts::tui_openclaw_tools_rules_block_title();
    let rules_block = block_title_needle(rules_title);
    let rules_block_line = line_with(&content, &rules_block);
    let rules_block_row_index = line_index(&content, &rules_block);
    let rules_border_col = display_column_in_line(rules_block_line, &rules_block);
    let rules_title_col = display_column_in_line(rules_block_line, &buffer_cell_text(rules_title));
    let outer_border_cell = &buf[(content_rect.x, content_rect.y)];
    let rules_border_cell = content_cell_at(&app, &buf, rules_border_col, rules_block_row_index);
    let rules_title_cell = content_cell_at(&app, &buf, rules_title_col, rules_block_row_index);

    assert_eq!(outer_border_cell.fg, theme.accent, "{content}");
    assert_eq!(rules_border_cell.symbol(), "┌");
    assert_eq!(rules_border_cell.fg, theme.dim, "{content}");
    assert_ne!(rules_border_cell.fg, theme.accent, "{content}");
    assert!(
        rules_border_cell.modifier.contains(Modifier::BOLD),
        "{content}"
    );

    assert_eq!(rules_title_cell.fg, theme.comment, "{content}");
    assert_ne!(rules_title_cell.fg, theme.accent, "{content}");
    assert!(
        rules_title_cell.modifier.contains(Modifier::BOLD),
        "{content}"
    );
}

#[test]
fn openclaw_tools_route_selected_profile_row_does_not_use_literal_marker_prefix() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawTools;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::app::OpenClawToolsFormState::from_snapshot(None);
    form.profile = Some("coding".to_string());
    form.allow = vec!["Read".to_string()];
    form.deny = vec!["Exec".to_string()];
    form.section = crate::cli::tui::app::OpenClawToolsSection::Profile;
    form.row = 0;
    app.openclaw_tools_form = Some(form);

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
        profile: Some("coding".to_string()),
        allow: vec!["Read".to_string()],
        deny: vec!["Exec".to_string()],
        extra: std::collections::HashMap::new(),
    });

    let rendered = render(&app, &data);
    let all = all_text(&rendered);
    let content = content_text(&app, &rendered);
    let profile_row = line_with(&content, &buffer_cell_text("Profile: Coding"));
    let profile_row_index = line_index(&content, &buffer_cell_text("Profile: Coding"));
    let profile_row_col = display_column_in_line(profile_row, &buffer_cell_text("Profile: Coding"));
    let profile_cell = content_cell_at(&app, &rendered, profile_row_col, profile_row_index);
    let allow_row = line_with(&content, &buffer_cell_text("Read"));
    let allow_row_index = line_index(&content, &buffer_cell_text("Read"));
    let allow_row_col = display_column_in_line(allow_row, &buffer_cell_text("Read"));
    let allow_cell = content_cell_at(&app, &rendered, allow_row_col, allow_row_index);

    assert!(!profile_row.contains('>'), "{all}");
    assert!(!profile_row.contains("| Profile: Coding"), "{all}");
    assert_ne!(
        cell_style_signature(profile_cell),
        cell_style_signature(allow_cell)
    );

    let _no_color = EnvGuard::set("NO_COLOR", "1");
    let no_color_rendered = render(&app, &data);
    let no_color_content = content_text(&app, &no_color_rendered);
    let no_color_profile_row = line_with(&no_color_content, &buffer_cell_text("Profile: Coding"));
    let no_color_profile_row_index =
        line_index(&no_color_content, &buffer_cell_text("Profile: Coding"));
    let no_color_profile_row_col =
        display_column_in_line(no_color_profile_row, &buffer_cell_text("Profile: Coding"));
    let no_color_profile_cell = content_cell_at(
        &app,
        &no_color_rendered,
        no_color_profile_row_col,
        no_color_profile_row_index,
    );
    let no_color_allow_row = line_with(&no_color_content, &buffer_cell_text("Read"));
    let no_color_allow_row_index = line_index(&no_color_content, &buffer_cell_text("Read"));
    let no_color_allow_row_col =
        display_column_in_line(no_color_allow_row, &buffer_cell_text("Read"));
    let no_color_allow_cell = content_cell_at(
        &app,
        &no_color_rendered,
        no_color_allow_row_col,
        no_color_allow_row_index,
    );

    assert!(
        !no_color_profile_row.contains("| Profile: Coding"),
        "{no_color_content}"
    );
    assert!(
        no_color_profile_cell.modifier.contains(Modifier::REVERSED),
        "{no_color_content}"
    );
    assert!(
        !no_color_allow_cell.modifier.contains(Modifier::REVERSED),
        "{no_color_content}"
    );
}

#[test]
fn openclaw_tools_route_selected_rule_row_does_not_use_literal_chevron_prefix() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawTools;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::app::OpenClawToolsFormState::from_snapshot(None);
    form.profile = Some("coding".to_string());
    form.allow = vec!["Read".to_string(), "Glob".to_string()];
    form.deny = vec!["Exec".to_string()];
    form.section = crate::cli::tui::app::OpenClawToolsSection::Allow;
    form.row = 0;
    app.openclaw_tools_form = Some(form);

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
        profile: Some("coding".to_string()),
        allow: vec!["Read".to_string(), "Glob".to_string()],
        deny: vec!["Exec".to_string()],
        extra: std::collections::HashMap::new(),
    });

    let rendered = render(&app, &data);
    let all = all_text(&rendered);
    let content = content_text(&app, &rendered);
    let profile_row = line_with(&content, &buffer_cell_text("配置档位: 编码"));
    let allow_label = line_with(
        &content,
        &block_label_needle(texts::tui_openclaw_tools_allow_list_label()),
    );
    let allow_row = line_with(&content, &buffer_cell_text("Read"));
    let allow_row_index = line_index(&content, &buffer_cell_text("Read"));
    let allow_row_col = display_column_in_line(allow_row, &buffer_cell_text("Read"));
    let allow_cell = content_cell_at(&app, &rendered, allow_row_col, allow_row_index);
    let sibling_row = line_with(&content, &buffer_cell_text("Glob"));
    let sibling_row_index = line_index(&content, &buffer_cell_text("Glob"));
    let sibling_row_col = display_column_in_line(sibling_row, &buffer_cell_text("Glob"));
    let sibling_cell = content_cell_at(&app, &rendered, sibling_row_col, sibling_row_index);

    assert!(!profile_row.contains('>'), "{all}");
    assert!(!allow_label.contains('>'), "{all}");
    assert!(!allow_row.contains('>'), "{all}");
    assert_ne!(
        cell_style_signature(allow_cell),
        cell_style_signature(sibling_cell)
    );

    let _no_color = EnvGuard::set("NO_COLOR", "1");
    let no_color_rendered = render(&app, &data);
    let no_color_content = content_text(&app, &no_color_rendered);
    let no_color_allow_row = line_with(&no_color_content, &buffer_cell_text("Read"));
    let no_color_allow_row_index = line_index(&no_color_content, &buffer_cell_text("Read"));
    let no_color_allow_row_col =
        display_column_in_line(no_color_allow_row, &buffer_cell_text("Read"));
    let no_color_allow_cell = content_cell_at(
        &app,
        &no_color_rendered,
        no_color_allow_row_col,
        no_color_allow_row_index,
    );
    let no_color_sibling_row = line_with(&no_color_content, &buffer_cell_text("Glob"));
    let no_color_sibling_row_index = line_index(&no_color_content, &buffer_cell_text("Glob"));
    let no_color_sibling_row_col =
        display_column_in_line(no_color_sibling_row, &buffer_cell_text("Glob"));
    let no_color_sibling_cell = content_cell_at(
        &app,
        &no_color_rendered,
        no_color_sibling_row_col,
        no_color_sibling_row_index,
    );

    assert!(!no_color_allow_row.contains('>'), "{no_color_content}");
    assert!(
        no_color_allow_cell.modifier.contains(Modifier::REVERSED),
        "{no_color_content}"
    );
    assert!(
        !no_color_sibling_cell.modifier.contains(Modifier::REVERSED),
        "{no_color_content}"
    );
}

#[test]
fn openclaw_tools_route_aligns_allow_and_deny_rule_rows_with_add_rows() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawTools;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::app::OpenClawToolsFormState::from_snapshot(None);
    form.profile = Some("coding".to_string());
    form.allow = vec!["Read".to_string(), "Glob".to_string()];
    form.deny = vec!["Exec".to_string()];
    form.section = crate::cli::tui::app::OpenClawToolsSection::Allow;
    form.row = 0;
    app.openclaw_tools_form = Some(form);

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
        profile: Some("coding".to_string()),
        allow: vec!["Read".to_string(), "Glob".to_string()],
        deny: vec!["Exec".to_string()],
        extra: std::collections::HashMap::new(),
    });

    let rendered = render(&app, &data);
    let content = content_text(&app, &rendered);
    let allow_label = line_with(
        &content,
        &block_label_needle(texts::tui_openclaw_tools_allow_list_label()),
    );
    let allow_row = line_with(&content, &buffer_cell_text("Read"));
    let allow_label_col = display_column_in_line(
        allow_label,
        &buffer_cell_text(texts::tui_openclaw_tools_allow_list_label()),
    );
    let deny_row = line_with(&content, &buffer_cell_text("Exec"));
    let allow_add = line_with(&content, &buffer_cell_text("+ 添加允许规则"));
    let separator = line_with(&content, &buffer_cell_text("- - - - -"));
    let deny_label = line_with(
        &content,
        &block_label_needle(texts::tui_openclaw_tools_deny_list_label()),
    );
    let deny_add = line_with(&content, &buffer_cell_text("+ 添加拒绝规则"));
    let deny_label_col = display_column_in_line(
        deny_label,
        &buffer_cell_text(texts::tui_openclaw_tools_deny_list_label()),
    );

    assert_eq!(
        allow_label_col,
        display_column_in_line(allow_row, &buffer_cell_text("Read")),
        "{content}"
    );
    assert_eq!(
        allow_label_col,
        display_column_in_line(allow_add, &buffer_cell_text("+ 添加允许规则")),
        "{content}"
    );
    assert_eq!(
        allow_label_col,
        display_column_in_line(separator, &buffer_cell_text("- - - - -")),
        "{content}"
    );
    assert_eq!(
        deny_label_col,
        display_column_in_line(deny_row, &buffer_cell_text("Exec")),
        "{content}"
    );
    assert_eq!(
        deny_label_col,
        display_column_in_line(deny_add, &buffer_cell_text("+ 添加拒绝规则")),
        "{content}"
    );
}

#[test]
fn openclaw_tools_route_keeps_selected_rule_visible_in_short_viewport() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawTools;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
        profile: Some("coding".to_string()),
        allow: (1..=6).map(|index| format!("allow-{index:02}")).collect(),
        deny: (1..=8).map(|index| format!("deny-{index:02}")).collect(),
        extra: std::collections::HashMap::new(),
    });

    let mut form = crate::cli::tui::app::OpenClawToolsFormState::from_snapshot(
        data.config.openclaw_tools.as_ref(),
    );
    form.section = crate::cli::tui::app::OpenClawToolsSection::Deny;
    form.row = 7;
    app.openclaw_tools_form = Some(form);

    let rendered = render_with_size(&app, &data, 72, 15);
    let content = content_text(&app, &rendered);
    assert!(content.contains(&buffer_cell_text("deny-08")), "{content}");

    let _no_color = EnvGuard::set("NO_COLOR", "1");
    let no_color_rendered = render_with_size(&app, &data, 72, 15);
    let no_color_content = content_text(&app, &no_color_rendered);
    let selected_row = line_with(&no_color_content, &buffer_cell_text("deny-08"));
    let selected_row_index = line_index(&no_color_content, &buffer_cell_text("deny-08"));
    let selected_row_col = display_column_in_line(selected_row, &buffer_cell_text("deny-08"));
    let selected_cell = content_cell_at(
        &app,
        &no_color_rendered,
        selected_row_col,
        selected_row_index,
    );
    assert!(
        selected_cell.modifier.contains(Modifier::REVERSED),
        "{no_color_content}"
    );
}

#[test]
fn openclaw_tools_route_wraps_long_rule_values_in_narrow_width() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let tail = "TAILMARK";
    let tail_fragment = "MARK";
    let long_rule = format!("allow.rules.segment.segment.segment.{tail}");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawTools;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
        profile: Some("coding".to_string()),
        allow: vec![long_rule.clone()],
        deny: vec!["Exec".to_string()],
        extra: std::collections::HashMap::new(),
    });

    let mut form = crate::cli::tui::app::OpenClawToolsFormState::from_snapshot(
        data.config.openclaw_tools.as_ref(),
    );
    form.section = crate::cli::tui::app::OpenClawToolsSection::Allow;
    form.row = 0;
    app.openclaw_tools_form = Some(form);

    let rendered = render_with_size(&app, &data, 72, 18);
    let content = content_text(&app, &rendered);
    assert!(
        content.contains(&buffer_cell_text(tail_fragment)),
        "{content}"
    );

    let _no_color = EnvGuard::set("NO_COLOR", "1");
    let no_color_rendered = render_with_size(&app, &data, 72, 18);
    let no_color_content = content_text(&app, &no_color_rendered);
    assert!(
        no_color_content.contains(&buffer_cell_text(tail_fragment)),
        "{no_color_content}"
    );

    let tail_row = line_with(&no_color_content, &buffer_cell_text(tail_fragment));
    let tail_row_index = line_index(&no_color_content, &buffer_cell_text(tail_fragment));
    let tail_row_col = display_column_in_line(tail_row, &buffer_cell_text(tail_fragment));
    let tail_cell = content_cell_at(&app, &no_color_rendered, tail_row_col, tail_row_index);
    let add_row = line_with(&no_color_content, &buffer_cell_text("+ Add allow rule"));
    let add_row_index = line_index(&no_color_content, &buffer_cell_text("+ Add allow rule"));
    let add_row_col = display_column_in_line(add_row, &buffer_cell_text("+ Add allow rule"));
    let add_cell = content_cell_at(&app, &no_color_rendered, add_row_col, add_row_index);

    assert!(
        tail_cell.modifier.contains(Modifier::REVERSED),
        "{no_color_content}"
    );
    assert!(
        !add_cell.modifier.contains(Modifier::REVERSED),
        "{no_color_content}"
    );
}

#[test]
fn openclaw_tools_route_renders_unsupported_profile_warning_and_label() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawTools;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
        profile: Some("unsupported-profile".to_string()),
        allow: vec!["Read".to_string()],
        deny: Vec::new(),
        extra: std::collections::HashMap::new(),
    });

    let all = all_text(&render(&app, &data));

    assert!(all.contains("Unsupported tools profile detected"), "{all}");
    assert!(
        all.contains("The current tools.profile value 'unsupported-profile'"),
        "{all}"
    );
    assert!(
        all.contains("list. It will be preserved until you choose a new value."),
        "{all}"
    );
    assert!(all.contains("unsupported-profile"), "{all}");
    assert!(all.contains("unsupported"), "{all}");
}

#[test]
fn openclaw_tools_route_renders_unsupported_profile_inline_without_placeholder_rows() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawTools;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
        profile: Some("unsupported-profile".to_string()),
        allow: Vec::new(),
        deny: Vec::new(),
        extra: std::collections::HashMap::new(),
    });

    let all = all_text(&render(&app, &data));
    let has_inline_profile_value = all
        .lines()
        .any(|line| line.contains("Profile: unsupported-profile (unsupported)"));
    let has_separate_unsupported_choice = all.lines().any(|line| {
        line.contains("unsupported-profile (unsupported)") && !line.contains("Profile:")
    });

    assert!(
        has_inline_profile_value,
        "expected unsupported profile to stay on the profile row, got:\n{all}"
    );
    assert!(!has_separate_unsupported_choice, "{all}");
    assert!(all.contains("+ Add allow rule"), "{all}");
    assert!(all.contains("+ Add deny rule"), "{all}");
    assert!(
        !all.contains(texts::tui_openclaw_tools_pattern_placeholder()),
        "{all}"
    );
}

#[test]
fn openclaw_tools_profile_picker_overlay_renders_supported_choices_in_order() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawTools;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
        profile: Some("coding".to_string()),
        allow: vec!["Read".to_string()],
        deny: Vec::new(),
        extra: std::collections::HashMap::new(),
    });

    assert!(matches!(
        app.on_key(key(KeyCode::Enter), &data),
        Action::None
    ));

    let all = all_text(&render(&app, &data));
    let labels = [
        texts::tui_openclaw_tools_profile_unset(),
        texts::tui_openclaw_tools_profile_minimal(),
        texts::tui_openclaw_tools_profile_coding(),
        texts::tui_openclaw_tools_profile_messaging(),
        texts::tui_openclaw_tools_profile_full(),
    ];
    let choice_lines = all
        .lines()
        .filter(|line| {
            labels.iter().any(|label| line.contains(label))
                && line.contains('│')
                && !line.contains("Profile:")
                && !line.contains('|')
                && !line.contains('>')
        })
        .collect::<Vec<_>>();

    assert_eq!(choice_lines.len(), labels.len(), "{all}");
    for (line, label) in choice_lines.iter().zip(labels) {
        assert!(line.contains(label), "{all}");
    }
    assert!(
        choice_lines[2].contains(texts::tui_marker_active()),
        "{all}"
    );
}

#[test]
fn openclaw_tools_profile_picker_overlay_unsupported_state_requires_selection_before_apply() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawTools;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
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

    let all = all_text(&render(&app, &data));
    let labels = [
        texts::tui_openclaw_tools_profile_unset(),
        texts::tui_openclaw_tools_profile_minimal(),
        texts::tui_openclaw_tools_profile_coding(),
        texts::tui_openclaw_tools_profile_messaging(),
        texts::tui_openclaw_tools_profile_full(),
    ];
    let choice_lines = all
        .lines()
        .filter(|line| {
            labels.iter().any(|label| line.contains(label))
                && line.contains('│')
                && !line.contains('|')
                && !line.contains('>')
        })
        .collect::<Vec<_>>();

    assert_eq!(choice_lines.len(), labels.len(), "{all}");
    assert!(
        choice_lines
            .iter()
            .all(|line| !line.contains("unsupported-profile")),
        "{all}"
    );
    assert!(
        choice_lines
            .iter()
            .all(|line| !line.contains(texts::tui_marker_active())),
        "{all}"
    );
    assert!(
        !all.contains(&format!("Enter {}", texts::tui_key_apply())),
        "{all}"
    );
}

#[test]
#[serial(home_settings)]
fn openclaw_tools_route_shows_real_parse_warning_without_default_seeded_form_values() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let temp_home = TempDir::new().expect("create temp home");
    let openclaw_dir = temp_home.path().join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let _home = SettingsEnvGuard::set_home(temp_home.path());

    write_openclaw_config_source(
        r#"{
  tools: {
    profile: 'coding',
    allow: 'Read',
  },
}"#,
    )
    .expect("write malformed openclaw config");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawTools;
    app.focus = Focus::Content;
    let data = UiData::load(&AppType::OpenClaw).expect("load openclaw ui data");

    let all = all_text(&render(&app, &data));

    assert!(
        all.contains(texts::tui_openclaw_config_warning_title()),
        "{all}"
    );
    assert!(all.contains("Failed to parse tools config"), "{all}");
    assert!(!all.contains("Profile: Not set"), "{all}");
}

#[test]
fn openclaw_tools_load_failed_description_uses_muted_comment_style() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawTools;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_tools = None;
    data.config.openclaw_warnings = Some(vec![crate::openclaw_config::OpenClawHealthWarning {
        code: "config_parse_failed".to_string(),
        message: "Failed to parse tools.profile".to_string(),
        path: Some("tools.profile".to_string()),
    }]);

    let buf = render_with_size(&app, &data, 120, 16);
    let content = content_text(&app, &buf);
    let theme = theme_for(&app.app_type);

    let description_row = line_with(&content, texts::tui_openclaw_tools_description());
    let description_row_index = line_index(&content, texts::tui_openclaw_tools_description());
    let description_start_col =
        column_in_line(description_row, texts::tui_openclaw_tools_description());
    let description_end_col = column_in_line(description_row, "allow/deny lists)");
    let description_start_cell =
        content_cell_at(&app, &buf, description_start_col, description_row_index);
    let description_end_cell =
        content_cell_at(&app, &buf, description_end_col, description_row_index);
    let description_start_style = cell_style_signature(description_start_cell);
    let description_end_style = cell_style_signature(description_end_cell);

    assert_eq!(description_start_style, description_end_style, "{content}");
    assert_eq!(description_start_cell.fg, theme.comment, "{content}");
    assert_eq!(description_start_cell.bg, Color::Reset, "{content}");
    assert_eq!(
        description_start_cell.modifier,
        Modifier::empty(),
        "{content}"
    );
}

#[test]
#[serial(home_settings)]
fn openclaw_agents_route_shows_real_parse_warning_without_default_seeded_form_values() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let temp_home = TempDir::new().expect("create temp home");
    let openclaw_dir = temp_home.path().join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let _home = SettingsEnvGuard::set_home(temp_home.path());

    write_openclaw_config_source(
        r#"{
  agents: {
    defaults: 'broken-defaults',
  },
}"#,
    )
    .expect("write malformed openclaw config");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawAgents;
    app.focus = Focus::Content;
    let data = UiData::load(&AppType::OpenClaw).expect("load openclaw ui data");

    let all = all_text(&render(&app, &data));

    assert!(
        all.contains(texts::tui_openclaw_config_warning_title()),
        "{all}"
    );
    assert!(all.contains("Failed to parse agents.defaults"), "{all}");
    assert!(
        all.contains("The current agents.defaults section could not be loaded."),
        "{all}"
    );
    assert!(all.contains("parse warning above"), "{all}");
    assert!(!all.contains("Default Model: Not set"), "{all}");
}

#[test]
#[serial(home_settings)]
fn openclaw_tools_route_ignores_unrelated_agents_parse_warning_and_keeps_save_available() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let temp_home = TempDir::new().expect("create temp home");
    let openclaw_dir = temp_home.path().join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let _home = SettingsEnvGuard::set_home(temp_home.path());

    write_openclaw_config_source(
        r#"{
  agents: {
    defaults: 'broken-defaults',
  },
  tools: {
    profile: 'coding',
    allow: ['Read'],
  },
}"#,
    )
    .expect("write openclaw config with unrelated agents parse failure");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawTools;
    app.focus = Focus::Content;
    let data = UiData::load(&AppType::OpenClaw).expect("load openclaw ui data");

    let all = all_text(&render(&app, &data));

    assert!(
        !all.contains("Failed to parse agents.defaults"),
        "tools route should not render unrelated agents parse warnings:\n{all}"
    );
    assert!(
        !all.contains(texts::tui_openclaw_config_warning_title()),
        "tools route should stay clean for unrelated section warnings:\n{all}"
    );

    assert!(matches!(
        app.on_key(key(KeyCode::Down), &data),
        crate::cli::tui::app::Action::None
    ));

    let action = app.on_key(key(KeyCode::Enter), &data);

    assert!(matches!(action, crate::cli::tui::app::Action::None));
    assert!(matches!(app.overlay, Overlay::TextInput(_)));
    assert!(
        app.toast.is_none(),
        "save should not be blocked: {action:?}"
    );
}

#[test]
fn openclaw_config_item_and_route_titles_follow_i18n_texts() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut config_app = App::new(Some(AppType::OpenClaw));
    config_app.route = Route::Config;
    config_app.focus = Focus::Content;
    config_app.filter.buffer = "openclaw".to_string();
    let config_labels = super::config_items_filtered(&config_app)
        .into_iter()
        .map(|item| super::config_item_label(&item))
        .collect::<Vec<_>>();

    assert!(!config_labels.contains(&texts::tui_config_item_openclaw_env()));
    assert!(!config_labels.contains(&texts::tui_config_item_openclaw_tools()));
    assert!(!config_labels.contains(&texts::tui_config_item_openclaw_agents()));
    assert!(!config_labels.contains(&"OpenClaw Env"));
    assert!(!config_labels.contains(&"OpenClaw Tools"));

    let mut route_app = App::new(Some(AppType::OpenClaw));
    route_app.route = Route::ConfigOpenClawAgents;
    route_app.focus = Focus::Content;
    assert_eq!(
        ConfigItem::OpenClawAgents.detail_title(),
        Some(texts::tui_openclaw_config_agents_title())
    );
    assert_ne!(
        ConfigItem::OpenClawAgents.detail_title(),
        Some("OpenClaw Agents Defaults")
    );
}

#[test]
fn workspace_openclaw_nav_uses_app_specific_labels_and_hides_generic_entries() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let app = App::new(Some(AppType::OpenClaw));
    let buf = render(&app, &minimal_data(&app.app_type));
    let all = nav_text(&app, &buf);
    let expected = [
        NavItem::Main,
        NavItem::Providers,
        NavItem::OpenClawWorkspace,
        NavItem::OpenClawEnv,
        NavItem::OpenClawTools,
        NavItem::OpenClawAgents,
        NavItem::Settings,
        NavItem::Exit,
    ]
    .map(nav_label_text);
    let positions = expected
        .iter()
        .map(|label| all.find(label).expect("OpenClaw nav label should render"))
        .collect::<Vec<_>>();

    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{all}");
    assert!(!all.contains(&nav_label_text(NavItem::Mcp)), "{all}");
    assert!(!all.contains(&nav_label_text(NavItem::Skills)), "{all}");
    assert!(!all.contains(&nav_label_text(NavItem::Prompts)), "{all}");
    assert!(!all.contains(&nav_label_text(NavItem::Config)), "{all}");
}

#[test]
fn workspace_non_openclaw_nav_keeps_generic_labels() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let app = App::new(Some(AppType::Claude));
    let all = nav_text(&app, &render(&app, &minimal_data(&app.app_type)));

    for item in [
        NavItem::Main,
        NavItem::Providers,
        NavItem::Mcp,
        NavItem::Skills,
        NavItem::Prompts,
        NavItem::Config,
    ] {
        assert!(all.contains(&nav_label_text(item)), "{all}");
    }
    for item in [
        NavItem::OpenClawWorkspace,
        NavItem::OpenClawEnv,
        NavItem::OpenClawTools,
        NavItem::OpenClawAgents,
    ] {
        assert!(!all.contains(&nav_label_text(item)), "{all}");
    }
}

#[test]
fn workspace_route_render_shows_workspace_files_and_daily_memory_entry() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawWorkspace;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_workspace = OpenClawWorkspaceSnapshot {
        directory_path: std::path::PathBuf::from("/tmp/.openclaw/workspace"),
        file_exists: std::collections::HashMap::from([
            ("AGENTS.md".to_string(), true),
            ("SOUL.md".to_string(), false),
        ]),
        daily_memory_files: vec![DailyMemoryFileInfo {
            filename: "2026-03-20.md".to_string(),
            date: "2026-03-20".to_string(),
            size_bytes: 12,
            modified_at: 1,
            preview: "remember this".to_string(),
        }],
    };

    let all = all_text(&render(&app, &data));

    assert!(all.contains(nav_title_text(NavItem::OpenClawWorkspace)));
    assert!(all.contains(texts::tui_openclaw_workspace_directory_label()));
    assert!(all.contains("/tmp/.openclaw/workspace"));
    assert!(all.contains(ALLOWED_FILES[0]));
    assert!(all.contains(texts::tui_openclaw_workspace_daily_memory_label()));
    assert!(all.contains("1 file"), "{all}");
    assert!(!all.contains("1 files"), "{all}");
    assert!(all.contains(texts::tui_key_open_directory()), "{all}");
    assert!(
        !all.contains("Press Enter to browse files, or press o to open the memory directory."),
        "{all}"
    );
    assert!(!all.contains("press o there"), "{all}");
}

#[test]
fn workspace_route_render_places_directory_summary_above_workspace_files_block_and_keeps_status_column_aligned(
) {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawWorkspace;
    app.focus = Focus::Content;
    app.workspace_idx = 1;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_workspace = OpenClawWorkspaceSnapshot {
        directory_path: std::path::PathBuf::from("/tmp/.openclaw/workspace"),
        file_exists: std::collections::HashMap::from([
            ("AGENTS.md".to_string(), true),
            ("SOUL.md".to_string(), false),
        ]),
        daily_memory_files: vec![DailyMemoryFileInfo {
            filename: "2026-03-20.md".to_string(),
            date: "2026-03-20".to_string(),
            size_bytes: 12,
            modified_at: 1,
            preview: "remember this".to_string(),
        }],
    };

    let rendered = render(&app, &data);
    let content = content_text(&app, &rendered);
    let key_bar_line = line_index(
        &content,
        &format!(
            "Enter {}  o {}",
            texts::tui_key_open(),
            texts::tui_key_open_directory()
        ),
    );
    let directory_summary = line_index(
        &content,
        &buffer_cell_text(texts::tui_openclaw_workspace_directory_label()),
    );
    let workspace_files_block = content
        .lines()
        .enumerate()
        .find_map(|(index, line)| {
            (index > key_bar_line
                && line.contains(&block_title_needle(
                    texts::tui_openclaw_workspace_files_block_title(),
                )))
            .then_some(index)
        })
        .unwrap_or_else(|| panic!("missing inner workspace files block in:\n{content}"));
    assert!(directory_summary < workspace_files_block, "{content}");

    let exists_row = line_with(&content, &buffer_cell_text("AGENTS.md"));
    let missing_row = line_with(&content, &buffer_cell_text("SOUL.md"));
    assert_eq!(
        display_column_in_line(
            exists_row,
            &buffer_cell_text(texts::tui_openclaw_workspace_status_exists())
        ),
        display_column_in_line(
            missing_row,
            &buffer_cell_text(texts::tui_openclaw_workspace_status_missing())
        ),
        "{content}"
    );
}

#[test]
fn workspace_route_render_keeps_selected_workspace_row_visible_in_short_viewport() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawWorkspace;
    app.focus = Focus::Content;
    app.workspace_idx = ALLOWED_FILES.len() - 1;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_workspace = OpenClawWorkspaceSnapshot {
        directory_path: std::path::PathBuf::from("/tmp/.openclaw/workspace"),
        file_exists: std::collections::HashMap::from([(
            ALLOWED_FILES[ALLOWED_FILES.len() - 1].to_string(),
            true,
        )]),
        daily_memory_files: vec![DailyMemoryFileInfo {
            filename: "2026-03-20.md".to_string(),
            date: "2026-03-20".to_string(),
            size_bytes: 12,
            modified_at: 1,
            preview: "remember this".to_string(),
        }],
    };

    let rendered = render_with_size(&app, &data, 72, 12);
    let content = content_text(&app, &rendered);

    let selected_row = line_with(
        &content,
        &buffer_cell_text(ALLOWED_FILES[ALLOWED_FILES.len() - 1]),
    );
    assert!(
        selected_row.contains(&buffer_cell_text(
            texts::tui_openclaw_workspace_status_exists()
        )),
        "{content}"
    );
}

#[test]
fn workspace_route_render_selected_rows_do_not_use_literal_chevron_prefix() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawWorkspace;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_workspace = OpenClawWorkspaceSnapshot {
        directory_path: std::path::PathBuf::from("/tmp/.openclaw/workspace"),
        file_exists: std::collections::HashMap::from([(ALLOWED_FILES[0].to_string(), true)]),
        daily_memory_files: vec![DailyMemoryFileInfo {
            filename: "2026-03-20.md".to_string(),
            date: "2026-03-20".to_string(),
            size_bytes: 12,
            modified_at: 1,
            preview: "remember this".to_string(),
        }],
    };

    app.workspace_idx = 0;
    let file_rendered = render(&app, &data);
    let file_content = content_text(&app, &file_rendered);
    let file_row_index = line_index(&file_content, &buffer_cell_text(ALLOWED_FILES[0]));
    let file_row = line_with(&file_content, &buffer_cell_text(ALLOWED_FILES[0]));
    let file_value_col = column_in_line(file_row, &buffer_cell_text(ALLOWED_FILES[0]));
    let file_cell = content_cell_at(&app, &file_rendered, file_value_col, file_row_index);
    let missing_row_index = line_index(&file_content, &buffer_cell_text(ALLOWED_FILES[1]));
    let missing_row = line_with(&file_content, &buffer_cell_text(ALLOWED_FILES[1]));
    let missing_value_col = column_in_line(missing_row, &buffer_cell_text(ALLOWED_FILES[1]));
    let missing_cell = content_cell_at(&app, &file_rendered, missing_value_col, missing_row_index);
    assert!(!file_row.contains('>'), "{file_content}");
    assert_ne!(
        cell_style_signature(file_cell),
        cell_style_signature(missing_cell)
    );

    app.workspace_idx = ALLOWED_FILES.len();
    let daily_memory_rendered = render(&app, &data);
    let daily_memory_content = content_text(&app, &daily_memory_rendered);
    let daily_memory_value = buffer_cell_text(&format!(
        "{}: {}",
        texts::tui_openclaw_workspace_daily_memory_label(),
        texts::tui_openclaw_workspace_daily_memory_count(
            data.config.openclaw_workspace.daily_memory_files.len()
        )
    ));
    let daily_memory_row_index = line_index(&daily_memory_content, &daily_memory_value);
    let daily_memory_row = line_with(&daily_memory_content, &daily_memory_value);
    let daily_memory_value_col = column_in_line(daily_memory_row, &daily_memory_value);
    let daily_memory_cell = content_cell_at(
        &app,
        &daily_memory_rendered,
        daily_memory_value_col,
        daily_memory_row_index,
    );
    let file_row_index = line_index(&daily_memory_content, &buffer_cell_text(ALLOWED_FILES[0]));
    let file_row = line_with(&daily_memory_content, &buffer_cell_text(ALLOWED_FILES[0]));
    let file_value_col = column_in_line(file_row, &buffer_cell_text(ALLOWED_FILES[0]));
    let file_cell = content_cell_at(&app, &daily_memory_rendered, file_value_col, file_row_index);
    assert!(!daily_memory_row.contains('>'), "{daily_memory_content}");
    assert_ne!(
        cell_style_signature(daily_memory_cell),
        cell_style_signature(file_cell)
    );

    let _no_color = EnvGuard::set("NO_COLOR", "1");
    app.workspace_idx = 0;
    let no_color_rendered = render(&app, &data);
    let no_color_content = content_text(&app, &no_color_rendered);
    let no_color_selected_row_index =
        line_index(&no_color_content, &buffer_cell_text(ALLOWED_FILES[0]));
    let no_color_selected_row = line_with(&no_color_content, &buffer_cell_text(ALLOWED_FILES[0]));
    let no_color_selected_col =
        column_in_line(no_color_selected_row, &buffer_cell_text(ALLOWED_FILES[0]));
    let no_color_selected_cell = content_cell_at(
        &app,
        &no_color_rendered,
        no_color_selected_col,
        no_color_selected_row_index,
    );
    let no_color_unselected_row_index =
        line_index(&no_color_content, &buffer_cell_text(ALLOWED_FILES[1]));
    let no_color_unselected_row = line_with(&no_color_content, &buffer_cell_text(ALLOWED_FILES[1]));
    let no_color_unselected_col =
        column_in_line(no_color_unselected_row, &buffer_cell_text(ALLOWED_FILES[1]));
    let no_color_unselected_cell = content_cell_at(
        &app,
        &no_color_rendered,
        no_color_unselected_col,
        no_color_unselected_row_index,
    );
    assert!(!no_color_selected_row.contains('>'), "{no_color_content}");
    assert!(
        no_color_selected_cell.modifier.contains(Modifier::REVERSED),
        "{no_color_content}"
    );
    assert!(
        !no_color_unselected_cell
            .modifier
            .contains(Modifier::REVERSED),
        "{no_color_content}"
    );
}

#[test]
fn workspace_route_render_does_not_leave_an_unused_gap_before_body_content() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawWorkspace;
    app.focus = Focus::Content;

    let rendered = render(&app, &minimal_data(&app.app_type));
    let content = content_text(&app, &rendered);
    let key_bar_line = line_index(
        &content,
        &format!(
            "Enter {}  o {}",
            texts::tui_key_open(),
            texts::tui_key_open_directory()
        ),
    );
    let first_body_content_line = content
        .lines()
        .enumerate()
        .find_map(|(index, line)| {
            (index > key_bar_line
                && (line.contains(&buffer_cell_text(
                    texts::tui_openclaw_workspace_directory_label(),
                )) || line.contains(&block_title_needle(
                    texts::tui_openclaw_workspace_files_block_title(),
                ))))
            .then_some(index)
        })
        .unwrap_or_else(|| panic!("missing workspace body content in:\n{content}"));

    assert_eq!(first_body_content_line - key_bar_line, 1, "{content}");
}

#[test]
fn workspace_route_render_wraps_long_summary_and_daily_memory_values_in_narrow_width() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawWorkspace;
    app.focus = Focus::Content;

    let long_workspace_tail = "workspace-tail-marker";
    let long_memory_tail = "workspace-tail-marker/memory";
    let long_preview_tail = "preview-tail-marker";

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_workspace = OpenClawWorkspaceSnapshot {
        directory_path: std::path::PathBuf::from(format!(
            "/tmp/.openclaw/workspace/teams/alpha/project/nested/{long_workspace_tail}"
        )),
        file_exists: std::collections::HashMap::from([(ALLOWED_FILES[0].to_string(), true)]),
        daily_memory_files: vec![DailyMemoryFileInfo {
            filename: "2026-03-20.md".to_string(),
            date: "2026-03-20".to_string(),
            size_bytes: 12,
            modified_at: 1,
            preview: format!(
                "handoff notes for the workspace redesign should keep {long_preview_tail} visible"
            ),
        }],
    };

    let rendered = render_with_size(&app, &data, 76, 28);
    let content = content_text(&app, &rendered);

    assert!(content.contains(long_workspace_tail), "{content}");
    assert!(content.contains(long_memory_tail), "{content}");
    assert!(content.contains(long_preview_tail), "{content}");
}

#[test]
fn workspace_route_render_long_summary_still_keeps_selected_block_visible_in_short_viewport() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawWorkspace;
    app.focus = Focus::Content;
    app.workspace_idx = ALLOWED_FILES.len() - 1;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_workspace = OpenClawWorkspaceSnapshot {
        directory_path: std::path::PathBuf::from(
            "/tmp/.openclaw/workspace/teams/alpha/project/deeply/nested/summary-wrap-visibility-marker",
        ),
        file_exists: std::collections::HashMap::from([(
            ALLOWED_FILES[ALLOWED_FILES.len() - 1].to_string(),
            true,
        )]),
        daily_memory_files: vec![DailyMemoryFileInfo {
            filename: "2026-03-20.md".to_string(),
            date: "2026-03-20".to_string(),
            size_bytes: 12,
            modified_at: 1,
            preview: "preview-hidden-by-summary-marker".to_string(),
        }],
    };

    let content = content_text(&app, &render_with_size(&app, &data, 48, 10));

    assert!(
        content.contains(&buffer_cell_text(ALLOWED_FILES[ALLOWED_FILES.len() - 1])),
        "{content}"
    );
    assert!(
        !content.contains("preview-hidden-by-summary-marker"),
        "{content}"
    );
}

#[test]
fn workspace_route_render_tight_height_prioritizes_selected_block() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawWorkspace;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_workspace = OpenClawWorkspaceSnapshot {
        directory_path: std::path::PathBuf::from("/w"),
        file_exists: std::collections::HashMap::from([(
            ALLOWED_FILES[ALLOWED_FILES.len() - 1].to_string(),
            true,
        )]),
        daily_memory_files: vec![DailyMemoryFileInfo {
            filename: "2026-03-20.md".to_string(),
            date: "2026-03-20".to_string(),
            size_bytes: 12,
            modified_at: 1,
            preview: "preview-priority-marker".to_string(),
        }],
    };

    app.workspace_idx = ALLOWED_FILES.len() - 1;
    let file_selected = content_text(&app, &render_with_size(&app, &data, 72, 17));
    assert!(
        file_selected.contains(&buffer_cell_text("BOOTSTRAP.md")),
        "{file_selected}"
    );
    assert!(
        !file_selected.contains("preview-priority-marker"),
        "{file_selected}"
    );

    app.workspace_idx = ALLOWED_FILES.len();
    let daily_selected = content_text(&app, &render_with_size(&app, &data, 72, 17));
    assert!(
        daily_selected.contains("preview-priority-marker"),
        "{daily_selected}"
    );
    assert!(
        !daily_selected.contains(&buffer_cell_text("BOOTSTRAP.md")),
        "{daily_selected}"
    );
}

#[test]
fn openclaw_polish_keeps_route_header_chrome_unchanged() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let cases = [
        (
            Route::ConfigOpenClawWorkspace,
            texts::tui_openclaw_workspace_title(),
        ),
        (
            Route::ConfigOpenClawEnv,
            texts::tui_openclaw_config_env_title(),
        ),
        (
            Route::ConfigOpenClawTools,
            texts::tui_openclaw_config_tools_title(),
        ),
        (
            Route::ConfigOpenClawAgents,
            texts::tui_openclaw_config_agents_title(),
        ),
    ];

    for (route, title) in cases {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = route;
        app.focus = Focus::Content;

        let all = all_text(&render(&app, &minimal_data(&app.app_type)));
        assert!(
            all.contains(&buffer_cell_text(title)),
            "missing title `{title}` in:\n{all}"
        );
    }

    let mut provider_app = App::new(Some(AppType::OpenClaw));
    provider_app.route = Route::Providers;
    provider_app.focus = Focus::Content;
    let provider_buf = render(&provider_app, &minimal_data(&provider_app.app_type));
    let provider_render = content_text(&provider_app, &provider_buf);
    assert!(
        provider_render.contains("Demo Provider"),
        "{provider_render}"
    );
    assert!(
        !provider_render.contains(&buffer_cell_text("权限档位")),
        "{provider_render}"
    );
    assert!(
        !provider_render.contains(&buffer_cell_text("规则列表")),
        "{provider_render}"
    );
}

#[test]
fn workspace_openclaw_route_titles_follow_nav_wording() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");

    assert_eq!(
        texts::tui_openclaw_workspace_title(),
        nav_title_text(NavItem::OpenClawWorkspace)
    );
    assert_eq!(
        texts::tui_openclaw_config_env_title(),
        nav_title_text(NavItem::OpenClawEnv)
    );
    assert_eq!(
        texts::tui_openclaw_config_tools_title(),
        nav_title_text(NavItem::OpenClawTools)
    );
    assert_eq!(
        texts::tui_openclaw_config_agents_title(),
        nav_title_text(NavItem::OpenClawAgents)
    );

    let cases = vec![
        (
            Route::ConfigOpenClawEnv,
            nav_title_text(NavItem::OpenClawEnv),
            vec![buffer_cell_text("OPENCLAW_ENV_TOKEN")],
        ),
        (
            Route::ConfigOpenClawTools,
            nav_title_text(NavItem::OpenClawTools),
            vec![
                buffer_cell_text(texts::tui_openclaw_tools_description()),
                buffer_cell_text(texts::tui_openclaw_tools_profile_label()),
            ],
        ),
        (
            Route::ConfigOpenClawAgents,
            nav_title_text(NavItem::OpenClawAgents),
            vec![
                buffer_cell_text(texts::tui_openclaw_agents_description()),
                buffer_cell_text(texts::tui_openclaw_agents_model_section()),
                buffer_cell_text(texts::tui_openclaw_agents_runtime_section()),
            ],
        ),
    ];

    for (route, title, expected_content) in cases {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = route.clone();
        app.focus = Focus::Content;

        let mut data = minimal_data(&app.app_type);
        match route {
            Route::ConfigOpenClawEnv => {
                data.config.openclaw_env = Some(crate::openclaw_config::OpenClawEnvConfig {
                    vars: std::collections::HashMap::from([(
                        "OPENCLAW_ENV_TOKEN".to_string(),
                        json!("demo-token"),
                    )]),
                });
            }
            Route::ConfigOpenClawTools => {
                data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
                    profile: Some("coding".to_string()),
                    allow: vec!["Read".to_string()],
                    deny: Vec::new(),
                    extra: std::collections::HashMap::new(),
                });
            }
            Route::ConfigOpenClawAgents => {
                data.config.openclaw_agents_defaults =
                    Some(crate::openclaw_config::OpenClawAgentsDefaults {
                        model: Some(crate::openclaw_config::OpenClawDefaultModel {
                            primary: "gpt-4.1".to_string(),
                            fallbacks: vec!["gpt-4o-mini".to_string()],
                            extra: std::collections::HashMap::new(),
                        }),
                        models: None,
                        extra: std::collections::HashMap::new(),
                    });
            }
            _ => unreachable!(),
        }

        let all = all_text(&render(&app, &data));
        assert!(all.contains(&buffer_cell_text(title)), "{all}");
        for expected in expected_content {
            assert!(all.contains(&expected), "missing `{expected}` in:\n{all}");
        }
    }
}

#[test]
fn openclaw_agents_route_render_shows_upstream_sections_and_warning_copy() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawAgents;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows = vec![openclaw_provider_row(
        "catalog",
        "目录供应商",
        &[("primary", "主模型"), ("fallback-a", "回退 A")],
    )];
    data.config.openclaw_agents_defaults = Some(crate::openclaw_config::OpenClawAgentsDefaults {
        model: Some(crate::openclaw_config::OpenClawDefaultModel {
            primary: "missing/current-primary".to_string(),
            fallbacks: vec!["catalog/fallback-a".to_string()],
            extra: std::collections::HashMap::new(),
        }),
        models: None,
        extra: std::collections::HashMap::from([
            ("workspace".to_string(), json!("./workspace")),
            ("timeout".to_string(), json!(42)),
            ("contextTokens".to_string(), json!(8192)),
            ("maxConcurrent".to_string(), json!(2)),
        ]),
    });

    let all = all_text(&render(&app, &data));

    for expected in [
        "管理 openclaw.json 中的 agents.defaults 配置（默认模型、运行参数等）",
        "模型配置",
        "运行参数",
        "默认模型",
        "回退模型",
        "工作区路径",
        "超时时间（秒）",
        "上下文 Token 数",
        "最大并发数",
        "检测到旧版超时字段",
        "当前配置仍在使用 agents.defaults.timeout。保存本页面时会迁移为 timeoutSeconds。",
    ] {
        assert!(
            all.contains(&buffer_cell_text(expected)),
            "missing `{expected}` in:\n{all}"
        );
    }
}

#[test]
fn openclaw_agents_route_render_groups_model_runtime_and_save_sections() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawAgents;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows = vec![openclaw_provider_row(
        "catalog",
        "目录供应商",
        &[("fallback-a", "回退 A")],
    )];
    data.config.openclaw_agents_defaults = Some(crate::openclaw_config::OpenClawAgentsDefaults {
        model: Some(crate::openclaw_config::OpenClawDefaultModel {
            primary: "missing/current-primary".to_string(),
            fallbacks: vec![
                "catalog/fallback-a".to_string(),
                "missing/off-catalog".to_string(),
            ],
            extra: std::collections::HashMap::from([("temperature".to_string(), json!(0.2))]),
        }),
        models: None,
        extra: std::collections::HashMap::from([
            ("workspace".to_string(), json!("./workspace")),
            ("timeout".to_string(), json!(42)),
            ("contextTokens".to_string(), json!(false)),
            ("maxConcurrent".to_string(), json!(2)),
        ]),
    });

    let rendered = render(&app, &data);
    let all = all_text(&rendered);
    let content = content_text(&app, &rendered);
    let model_block = line_index(
        &content,
        &block_title_needle(texts::tui_openclaw_agents_model_section()),
    );
    let runtime_block = line_index(
        &content,
        &block_title_needle(texts::tui_openclaw_agents_runtime_section()),
    );
    let default_model = line_index(
        &content,
        &buffer_cell_text("missing/current-primary (供应商未配置)"),
    );
    let fallbacks = line_index(&content, &buffer_cell_text("目录供应商 / 回退 A"));
    let off_catalog_fallback = line_index(&content, &buffer_cell_text("missing/off-catalog"));
    let add_fallback = line_index(
        &content,
        &buffer_cell_text(texts::tui_openclaw_agents_add_fallback_disabled()),
    );
    let legacy_title = line_index(
        &content,
        &buffer_cell_text(texts::tui_openclaw_agents_legacy_timeout_title()),
    );
    let preserved_notice = line_index(
        &content,
        &buffer_cell_text(texts::tui_openclaw_agents_preserved_runtime_notice()),
    );
    let preserved_fields = line_index(
        &content,
        &buffer_cell_text(texts::tui_openclaw_agents_preserved_fields_label()),
    );
    assert!(
        content.contains(&buffer_cell_text(texts::tui_openclaw_agents_description())),
        "{all}"
    );
    assert!(model_block < runtime_block, "{all}");
    assert!(
        model_block < default_model && default_model < fallbacks,
        "{all}"
    );
    assert!(
        fallbacks < off_catalog_fallback && off_catalog_fallback < add_fallback,
        "{all}"
    );
    assert!(add_fallback < runtime_block, "{all}");
    assert!(
        model_block < preserved_fields && preserved_fields < runtime_block,
        "{all}"
    );
    assert!(
        runtime_block < legacy_title && legacy_title < preserved_notice,
        "{all}"
    );
    assert!(
        !has_visible_action_button_or_block(&content, texts::tui_openclaw_agents_save_label()),
        "{all}"
    );
}

#[test]
fn openclaw_agents_route_render_load_failed_branch_keeps_description_spacer_and_message() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawAgents;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_agents_defaults = None;
    data.config.openclaw_warnings = Some(vec![crate::openclaw_config::OpenClawHealthWarning {
        code: "config_parse_failed".to_string(),
        message: "Failed to parse agents.defaults".to_string(),
        path: Some("agents.defaults".to_string()),
    }]);

    let rendered = render_with_size(&app, &data, 100, 16);
    let content = content_text(&app, &rendered);
    let description_index = line_index(
        &content,
        &buffer_cell_text("Manage agents.defaults in openclaw.json"),
    );
    let message_index = line_index(
        &content,
        &buffer_cell_text("The current agents.defaults section could not be loaded."),
    );
    let lines = content.lines().collect::<Vec<_>>();
    let spacer = lines[description_index + 1];

    assert!(description_index + 1 < message_index, "{content}");
    assert!(spacer.chars().all(|ch| ch == ' ' || ch == '│'), "{content}");
}

#[test]
fn openclaw_agents_route_render_uses_single_line_model_and_runtime_rows() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawAgents;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows = vec![openclaw_provider_row(
        "catalog",
        "目录供应商",
        &[
            ("primary", "主模型"),
            ("fallback-a", "回退 A"),
            ("fallback-b", "回退 B"),
        ],
    )];
    data.config.openclaw_agents_defaults = Some(crate::openclaw_config::OpenClawAgentsDefaults {
        model: Some(crate::openclaw_config::OpenClawDefaultModel {
            primary: "catalog/primary".to_string(),
            fallbacks: vec!["catalog/fallback-a".to_string()],
            extra: std::collections::HashMap::new(),
        }),
        models: None,
        extra: std::collections::HashMap::from([
            ("workspace".to_string(), json!("./workspace")),
            ("timeoutSeconds".to_string(), json!(42)),
            ("contextTokens".to_string(), json!(8192)),
            ("maxConcurrent".to_string(), json!(2)),
        ]),
    });

    let rendered = render(&app, &data);
    let all = all_text(&rendered);
    let content = content_text(&app, &rendered);
    let primary_row = line_with(&content, &buffer_cell_text("目录供应商 / 主模型"));
    let fallback_row = line_with(&content, &buffer_cell_text("目录供应商 / 回退 A"));
    let action_row = line_with(
        &content,
        &format!(
            "+ {}",
            buffer_cell_text(texts::tui_openclaw_agents_add_fallback())
        ),
    );
    let workspace_row = line_with(&content, &buffer_cell_text("[./workspace]"));

    assert!(
        primary_row.contains(&buffer_cell_text("目录供应商 / 主模型")),
        "{all}"
    );
    assert!(
        primary_row.contains(&buffer_cell_text(texts::tui_openclaw_agents_primary_model())),
        "{all}"
    );
    assert!(
        fallback_row.contains(&buffer_cell_text("目录供应商 / 回退 A")),
        "{all}"
    );
    assert!(
        fallback_row.contains(&buffer_cell_text(
            texts::tui_openclaw_agents_fallback_models()
        )),
        "{all}"
    );
    assert!(action_row.contains('+'), "{all}");
    assert!(
        workspace_row.contains(&buffer_cell_text("./workspace")),
        "{all}"
    );
    assert!(
        workspace_row.contains(&buffer_cell_text(texts::tui_openclaw_agents_workspace())),
        "{all}"
    );
    assert!(workspace_row.contains('['), "{all}");
    assert!(workspace_row.contains(']'), "{all}");
}

#[test]
fn openclaw_agents_route_render_renders_runtime_value_rows_with_bracketed_values() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawAgents;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_agents_defaults = Some(crate::openclaw_config::OpenClawAgentsDefaults {
        model: Some(crate::openclaw_config::OpenClawDefaultModel {
            primary: "demo/primary".to_string(),
            fallbacks: Vec::new(),
            extra: std::collections::HashMap::new(),
        }),
        models: None,
        extra: std::collections::HashMap::from([
            ("workspace".to_string(), json!("./workspace")),
            ("timeoutSeconds".to_string(), json!(42)),
            ("contextTokens".to_string(), json!(false)),
            ("maxConcurrent".to_string(), json!(2)),
            ("preservedField".to_string(), json!("kept")),
        ]),
    });

    let rendered = render_with_size(&app, &data, 140, 40);
    let content = content_text(&app, &rendered);
    let workspace_row = line_with(&content, &buffer_cell_text("[./workspace]"));
    let timeout_row = line_with(&content, &buffer_cell_text("[42]"));
    let context_row = line_with(
        &content,
        &buffer_cell_text("[false (preserved non-standard value)]"),
    );
    let max_row = line_with(&content, &buffer_cell_text("[2]"));

    assert!(workspace_row.contains('['), "{content}");
    assert!(timeout_row.contains('['), "{content}");
    assert!(context_row.contains('['), "{content}");
    assert!(max_row.contains('['), "{content}");
    assert!(
        workspace_row.contains(&buffer_cell_text(texts::tui_openclaw_agents_workspace())),
        "{content}"
    );
    assert!(
        timeout_row.contains(&buffer_cell_text(texts::tui_openclaw_agents_timeout())),
        "{content}"
    );
    assert!(
        context_row.contains(&buffer_cell_text(
            texts::tui_openclaw_agents_context_tokens()
        )),
        "{content}"
    );
    assert!(
        max_row.contains(&buffer_cell_text(
            texts::tui_openclaw_agents_max_concurrent()
        )),
        "{content}"
    );
    assert!(
        content.contains(texts::tui_openclaw_agents_preserved_runtime_notice()),
        "{content}"
    );
    assert!(
        content.contains(texts::tui_openclaw_agents_preserved_fields_label()),
        "{content}"
    );
}

#[test]
fn openclaw_agents_route_render_shifts_rows_two_columns_left_and_keeps_value_columns_aligned() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawAgents;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows = vec![openclaw_provider_row(
        "demo",
        "Demo Provider",
        &[
            ("primary", "Primary"),
            ("fallback-a", "Fallback A"),
            ("fallback-b", "Fallback B"),
        ],
    )];
    data.config.openclaw_agents_defaults = Some(crate::openclaw_config::OpenClawAgentsDefaults {
        model: Some(crate::openclaw_config::OpenClawDefaultModel {
            primary: "demo/primary".to_string(),
            fallbacks: vec!["demo/fallback-a".to_string()],
            extra: std::collections::HashMap::new(),
        }),
        models: None,
        extra: std::collections::HashMap::from([
            ("workspace".to_string(), json!("./workspace")),
            ("timeoutSeconds".to_string(), json!(42)),
            ("contextTokens".to_string(), json!(8192)),
            ("maxConcurrent".to_string(), json!(2)),
        ]),
    });

    let rendered = render_with_size(&app, &data, 140, 40);
    let content = content_text(&app, &rendered);
    let primary_row = line_with(&content, &buffer_cell_text("Demo Provider / Primary"));
    let fallback_row = line_with(&content, &buffer_cell_text("Demo Provider / Fallback A"));
    let workspace_row = line_with(&content, &buffer_cell_text("[./workspace]"));
    let add_row = line_with(
        &content,
        &buffer_cell_text(&format!("+ {}", texts::tui_openclaw_agents_add_fallback())),
    );

    let primary_label_start = display_column_in_line(
        primary_row,
        &buffer_cell_text(texts::tui_openclaw_agents_primary_model()),
    );
    let fallback_label_start = display_column_in_line(
        fallback_row,
        &buffer_cell_text(texts::tui_openclaw_agents_fallback_models()),
    );
    let workspace_label_start = display_column_in_line(
        workspace_row,
        &buffer_cell_text(texts::tui_openclaw_agents_workspace()),
    );
    let primary_value_start =
        display_column_in_line(primary_row, &buffer_cell_text("Demo Provider / Primary"));
    let fallback_value_start = display_column_in_line(
        fallback_row,
        &buffer_cell_text("Demo Provider / Fallback A"),
    );
    let workspace_value_start =
        display_column_in_line(workspace_row, &buffer_cell_text("[./workspace]"));
    let add_value_start = display_column_in_line(
        add_row,
        &buffer_cell_text(&format!("+ {}", texts::tui_openclaw_agents_add_fallback())),
    );
    let primary_block_border_start = UnicodeWidthStr::width(
        &primary_row[..primary_row[..column_in_line(
            primary_row,
            &buffer_cell_text(texts::tui_openclaw_agents_primary_model()),
        )]
            .rfind('│')
            .expect("primary row should include section block border")],
    );
    let workspace_block_border_start = UnicodeWidthStr::width(
        &workspace_row[..workspace_row[..column_in_line(
            workspace_row,
            &buffer_cell_text(texts::tui_openclaw_agents_workspace()),
        )]
            .rfind('│')
            .expect("workspace row should include section block border")],
    );

    assert_eq!(primary_label_start, fallback_label_start, "{content}");
    assert_eq!(primary_label_start, workspace_label_start, "{content}");
    assert_eq!(primary_value_start, fallback_value_start, "{content}");
    assert_eq!(primary_value_start, workspace_value_start, "{content}");
    assert_eq!(primary_value_start, add_value_start, "{content}");
    assert_eq!(
        primary_label_start - primary_block_border_start,
        4,
        "{content}"
    );
    assert_eq!(
        workspace_label_start - workspace_block_border_start,
        4,
        "{content}"
    );
}

#[test]
fn openclaw_agents_route_render_keeps_primary_row_visible_in_short_viewport() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawAgents;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows = vec![openclaw_provider_row(
        "d",
        "D",
        &[
            ("p", "P"),
            ("f1", "F1"),
            ("f2", "F2"),
            ("f3", "F3"),
            ("f4", "F4"),
            ("f5", "F5"),
            ("f6", "F6"),
        ],
    )];
    data.config.openclaw_agents_defaults = Some(crate::openclaw_config::OpenClawAgentsDefaults {
        model: Some(crate::openclaw_config::OpenClawDefaultModel {
            primary: "d/p".to_string(),
            fallbacks: (1..=6).map(|index| format!("d/f{index}")).collect(),
            extra: std::collections::HashMap::new(),
        }),
        models: None,
        extra: std::collections::HashMap::from([
            ("workspace".to_string(), json!("./workspace")),
            ("timeoutSeconds".to_string(), json!(42)),
            ("contextTokens".to_string(), json!(8192)),
            ("maxConcurrent".to_string(), json!(2)),
        ]),
    });

    let mut form = crate::cli::tui::app::OpenClawAgentsFormState::from_snapshot(
        data.config.openclaw_agents_defaults.as_ref(),
    );
    form.section = crate::cli::tui::app::OpenClawAgentsSection::PrimaryModel;
    form.row = 0;
    app.openclaw_agents_form = Some(form);

    let rendered = render_with_size(&app, &data, 72, 12);
    let content = content_text(&app, &rendered);

    let primary_row = line_with(&content, &buffer_cell_text("D / P"));
    assert!(
        primary_row.contains(&buffer_cell_text(texts::tui_openclaw_agents_primary_model())),
        "{content}"
    );
}

#[test]
fn openclaw_agents_route_render_keeps_selected_fallback_row_visible_in_short_viewport() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawAgents;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    let models = std::iter::once(("primary", "Primary"))
        .chain((1..=10).map(|index| {
            let id = format!("fallback-{index}");
            let label = format!("F{index:02}");
            (
                Box::leak(id.into_boxed_str()) as &str,
                Box::leak(label.into_boxed_str()) as &str,
            )
        }))
        .collect::<Vec<_>>();
    data.providers.rows = vec![openclaw_provider_row("d", "D", &models)];
    data.config.openclaw_agents_defaults = Some(crate::openclaw_config::OpenClawAgentsDefaults {
        model: Some(crate::openclaw_config::OpenClawDefaultModel {
            primary: "d/primary".to_string(),
            fallbacks: (1..=9).map(|index| format!("d/fallback-{index}")).collect(),
            extra: std::collections::HashMap::new(),
        }),
        models: None,
        extra: std::collections::HashMap::new(),
    });

    let mut form = crate::cli::tui::app::OpenClawAgentsFormState::from_snapshot(
        data.config.openclaw_agents_defaults.as_ref(),
    );
    form.section = crate::cli::tui::app::OpenClawAgentsSection::FallbackModels;
    form.row = 8;
    app.openclaw_agents_form = Some(form);

    let rendered = render_with_size(&app, &data, 72, 13);
    let content = content_text(&app, &rendered);

    let fallback_row = line_with(&content, &buffer_cell_text("D / F09"));
    assert!(
        fallback_row.contains(&buffer_cell_text(
            texts::tui_openclaw_agents_fallback_models()
        )),
        "{content}"
    );
}

#[test]
fn openclaw_agents_route_render_keeps_add_fallback_row_visible_in_short_viewport() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawAgents;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    let models = std::iter::once(("primary", "主"))
        .chain((1..=10).map(|index| {
            let id = format!("fallback-{index}");
            let label = format!("回{index}");
            (
                Box::leak(id.into_boxed_str()) as &str,
                Box::leak(label.into_boxed_str()) as &str,
            )
        }))
        .collect::<Vec<_>>();
    data.providers.rows = vec![openclaw_provider_row("d", "目", &models)];
    data.config.openclaw_agents_defaults = Some(crate::openclaw_config::OpenClawAgentsDefaults {
        model: Some(crate::openclaw_config::OpenClawDefaultModel {
            primary: "d/primary".to_string(),
            fallbacks: (1..=8).map(|index| format!("d/fallback-{index}")).collect(),
            extra: std::collections::HashMap::new(),
        }),
        models: None,
        extra: std::collections::HashMap::new(),
    });

    let mut form = crate::cli::tui::app::OpenClawAgentsFormState::from_snapshot(
        data.config.openclaw_agents_defaults.as_ref(),
    );
    form.section = crate::cli::tui::app::OpenClawAgentsSection::FallbackModels;
    form.row = form.fallbacks.len();
    app.openclaw_agents_form = Some(form);

    let rendered = render_with_size(&app, &data, 68, 13);
    let content = content_text(&app, &rendered);

    line_with(
        &content,
        &buffer_cell_text(&format!("+ {}", texts::tui_openclaw_agents_add_fallback())),
    );
}

#[test]
fn openclaw_agents_route_render_selected_rows_do_not_use_literal_chevron_prefix() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawAgents;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows = vec![openclaw_provider_row(
        "demo",
        "Demo Provider",
        &[
            ("primary", "Primary"),
            ("fallback-a", "Fallback A"),
            ("fallback-b", "Fallback B"),
        ],
    )];
    data.config.openclaw_agents_defaults = Some(crate::openclaw_config::OpenClawAgentsDefaults {
        model: Some(crate::openclaw_config::OpenClawDefaultModel {
            primary: "demo/primary".to_string(),
            fallbacks: vec!["demo/fallback-a".to_string()],
            extra: std::collections::HashMap::new(),
        }),
        models: None,
        extra: std::collections::HashMap::from([
            ("workspace".to_string(), json!("./workspace")),
            ("timeoutSeconds".to_string(), json!(42)),
            ("contextTokens".to_string(), json!(8192)),
            ("maxConcurrent".to_string(), json!(2)),
        ]),
    });

    let mut form = crate::cli::tui::app::OpenClawAgentsFormState::from_snapshot(
        data.config.openclaw_agents_defaults.as_ref(),
    );
    form.section = crate::cli::tui::app::OpenClawAgentsSection::PrimaryModel;
    form.row = 0;
    app.openclaw_agents_form = Some(form);

    let primary_rendered = render_with_size(&app, &data, 120, 40);
    let primary_content = content_text(&app, &primary_rendered);
    let primary_row = line_with(
        &primary_content,
        &buffer_cell_text("Demo Provider / Primary"),
    );
    assert!(!primary_row.contains('>'), "{primary_content}");

    let mut form = crate::cli::tui::app::OpenClawAgentsFormState::from_snapshot(
        data.config.openclaw_agents_defaults.as_ref(),
    );
    form.section = crate::cli::tui::app::OpenClawAgentsSection::FallbackModels;
    form.row = 0;
    app.openclaw_agents_form = Some(form);

    let fallback_rendered = render_with_size(&app, &data, 120, 40);
    let fallback_content = content_text(&app, &fallback_rendered);
    let fallback_row = line_with(
        &fallback_content,
        &buffer_cell_text("Demo Provider / Fallback A"),
    );
    assert!(!fallback_row.contains('>'), "{fallback_content}");

    let mut form = crate::cli::tui::app::OpenClawAgentsFormState::from_snapshot(
        data.config.openclaw_agents_defaults.as_ref(),
    );
    form.section = crate::cli::tui::app::OpenClawAgentsSection::FallbackModels;
    form.row = form.fallbacks.len();
    app.openclaw_agents_form = Some(form);

    let add_rendered = render_with_size(&app, &data, 120, 40);
    let add_content = content_text(&app, &add_rendered);
    let add_row = line_with(
        &add_content,
        &buffer_cell_text(&format!("+ {}", texts::tui_openclaw_agents_add_fallback())),
    );
    assert!(!add_row.contains('>'), "{add_content}");

    let mut form = crate::cli::tui::app::OpenClawAgentsFormState::from_snapshot(
        data.config.openclaw_agents_defaults.as_ref(),
    );
    form.section = crate::cli::tui::app::OpenClawAgentsSection::Runtime;
    form.row = 0;
    app.openclaw_agents_form = Some(form);

    let runtime_rendered = render_with_size(&app, &data, 120, 40);
    let runtime_content = content_text(&app, &runtime_rendered);
    let runtime_row = line_with(&runtime_content, &buffer_cell_text("[./workspace]"));
    assert!(!runtime_row.contains('>'), "{runtime_content}");
}

#[test]
fn openclaw_agents_route_render_selected_rows_keep_surface_and_rail_emphasis() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawAgents;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows = vec![openclaw_provider_row(
        "demo",
        "Demo",
        &[("primary", "Primary"), ("fallback-a", "Fallback A")],
    )];
    data.config.openclaw_agents_defaults = Some(crate::openclaw_config::OpenClawAgentsDefaults {
        model: Some(crate::openclaw_config::OpenClawDefaultModel {
            primary: "demo/primary".to_string(),
            fallbacks: vec!["demo/fallback-a".to_string()],
            extra: std::collections::HashMap::new(),
        }),
        models: None,
        extra: std::collections::HashMap::from([
            ("workspace".to_string(), json!("./workspace")),
            ("timeoutSeconds".to_string(), json!(42)),
            ("contextTokens".to_string(), json!(8192)),
            ("maxConcurrent".to_string(), json!(2)),
        ]),
    });

    let mut form = crate::cli::tui::app::OpenClawAgentsFormState::from_snapshot(
        data.config.openclaw_agents_defaults.as_ref(),
    );
    form.section = crate::cli::tui::app::OpenClawAgentsSection::PrimaryModel;
    form.row = 0;
    app.openclaw_agents_form = Some(form);

    let buf = render_with_size(&app, &data, 120, 40);
    let content = content_text(&app, &buf);
    let theme = theme_for(&app.app_type);
    let selected_value = buffer_cell_text("Demo / Primary");
    let selected_row_index = line_index(&content, &selected_value);
    let selected_row = line_with(&content, &selected_value);
    let selected_value_col = column_in_line(selected_row, &selected_value);
    let selected_cell = content_cell_at(&app, &buf, selected_value_col, selected_row_index);

    assert_eq!(selected_cell.bg, theme.surface);

    let row_width = buf.area.width.saturating_sub(content_origin_x(&app, &buf));
    let saw_accent_rail = (0..row_width).any(|content_x| {
        content_cell_at(&app, &buf, content_x as usize, selected_row_index).bg == theme.accent
    });
    assert!(saw_accent_rail, "{content}");

    let fallback_value = buffer_cell_text("Demo / Fallback A");
    let fallback_row_index = line_index(&content, &fallback_value);
    let fallback_row = line_with(&content, &fallback_value);
    let fallback_value_col = column_in_line(fallback_row, &fallback_value);
    let fallback_cell = content_cell_at(&app, &buf, fallback_value_col, fallback_row_index);

    assert_ne!(fallback_cell.bg, theme.surface);
}

#[test]
fn openclaw_agents_route_render_selected_rows_keep_reversed_emphasis_in_no_color_mode() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::set("NO_COLOR", "1");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawAgents;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows = vec![openclaw_provider_row(
        "demo",
        "Demo",
        &[("primary", "Primary"), ("fallback-a", "Fallback A")],
    )];
    data.config.openclaw_agents_defaults = Some(crate::openclaw_config::OpenClawAgentsDefaults {
        model: Some(crate::openclaw_config::OpenClawDefaultModel {
            primary: "demo/primary".to_string(),
            fallbacks: vec!["demo/fallback-a".to_string()],
            extra: std::collections::HashMap::new(),
        }),
        models: None,
        extra: std::collections::HashMap::from([
            ("workspace".to_string(), json!("./workspace")),
            ("timeoutSeconds".to_string(), json!(42)),
            ("contextTokens".to_string(), json!(8192)),
            ("maxConcurrent".to_string(), json!(2)),
        ]),
    });

    let mut form = crate::cli::tui::app::OpenClawAgentsFormState::from_snapshot(
        data.config.openclaw_agents_defaults.as_ref(),
    );
    form.section = crate::cli::tui::app::OpenClawAgentsSection::PrimaryModel;
    form.row = 0;
    app.openclaw_agents_form = Some(form);

    let buf = render_with_size(&app, &data, 120, 40);
    let content = content_text(&app, &buf);
    let selected_value = buffer_cell_text("Demo / Primary");
    let selected_row_index = line_index(&content, &selected_value);
    let selected_row = line_with(&content, &selected_value);
    let selected_value_col = column_in_line(selected_row, &selected_value);
    let selected_cell = content_cell_at(&app, &buf, selected_value_col, selected_row_index);

    assert!(selected_cell.modifier.contains(Modifier::REVERSED));

    let fallback_value = buffer_cell_text("Demo / Fallback A");
    let fallback_row_index = line_index(&content, &fallback_value);
    let fallback_row = line_with(&content, &fallback_value);
    let fallback_value_col = column_in_line(fallback_row, &fallback_value);
    let fallback_cell = content_cell_at(&app, &buf, fallback_value_col, fallback_row_index);

    assert!(!fallback_cell.modifier.contains(Modifier::REVERSED));
}

#[test]
fn openclaw_agents_route_render_field_labels_use_white_foreground() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawAgents;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows = vec![openclaw_provider_row(
        "demo",
        "Demo",
        &[("primary", "Primary"), ("fallback-a", "Fallback A")],
    )];
    data.config.openclaw_agents_defaults = Some(crate::openclaw_config::OpenClawAgentsDefaults {
        model: Some(crate::openclaw_config::OpenClawDefaultModel {
            primary: "demo/primary".to_string(),
            fallbacks: vec!["demo/fallback-a".to_string()],
            extra: std::collections::HashMap::new(),
        }),
        models: None,
        extra: std::collections::HashMap::from([
            ("workspace".to_string(), json!("./workspace")),
            ("timeoutSeconds".to_string(), json!(42)),
            ("contextTokens".to_string(), json!(8192)),
            ("maxConcurrent".to_string(), json!(2)),
        ]),
    });

    let mut form = crate::cli::tui::app::OpenClawAgentsFormState::from_snapshot(
        data.config.openclaw_agents_defaults.as_ref(),
    );
    form.section = crate::cli::tui::app::OpenClawAgentsSection::PrimaryModel;
    form.row = 0;
    app.openclaw_agents_form = Some(form);

    let buf = render_with_size(&app, &data, 120, 40);
    let content = content_text(&app, &buf);

    let primary_row = line_with(&content, &buffer_cell_text("Demo / Primary"));
    let primary_row_index = line_index(&content, &buffer_cell_text("Demo / Primary"));
    let primary_label_col = column_in_line(
        primary_row,
        &buffer_cell_text(texts::tui_openclaw_agents_primary_model()),
    );
    let primary_label_cell = content_cell_at(&app, &buf, primary_label_col, primary_row_index);

    let workspace_row = line_with(&content, &buffer_cell_text("[./workspace]"));
    let workspace_row_index = line_index(&content, &buffer_cell_text("[./workspace]"));
    let workspace_label_col = column_in_line(
        workspace_row,
        &buffer_cell_text(texts::tui_openclaw_agents_workspace()),
    );
    let workspace_label_cell =
        content_cell_at(&app, &buf, workspace_label_col, workspace_row_index);

    assert_eq!(primary_label_cell.fg, Color::White);
    assert_eq!(workspace_label_cell.fg, Color::White);
}

#[test]
fn openclaw_agents_route_render_keeps_runtime_rows_single_line_when_space_is_tight() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawAgents;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_agents_defaults = Some(crate::openclaw_config::OpenClawAgentsDefaults {
        model: Some(crate::openclaw_config::OpenClawDefaultModel {
            primary: "demo/primary".to_string(),
            fallbacks: Vec::new(),
            extra: std::collections::HashMap::new(),
        }),
        models: None,
        extra: std::collections::HashMap::from([
            (
                "workspace".to_string(),
                json!("./workspace-path-that-would-wrap"),
            ),
            ("timeoutSeconds".to_string(), json!(42)),
            ("contextTokens".to_string(), json!(8192)),
            ("maxConcurrent".to_string(), json!(2)),
        ]),
    });

    let rendered = render_with_size(&app, &data, 58, 40);
    let content = content_text(&app, &rendered);
    let workspace_line = line_index(
        &content,
        &buffer_cell_text(texts::tui_openclaw_agents_workspace()),
    );
    let timeout_line = line_index(
        &content,
        &buffer_cell_text(texts::tui_openclaw_agents_timeout()),
    );
    let context_line = line_index(
        &content,
        &buffer_cell_text(texts::tui_openclaw_agents_context_tokens()),
    );
    let max_line = line_index(
        &content,
        &buffer_cell_text(texts::tui_openclaw_agents_max_concurrent()),
    );

    assert_eq!(timeout_line, workspace_line + 1, "{content}");
    assert_eq!(context_line, timeout_line + 1, "{content}");
    assert_eq!(max_line, context_line + 1, "{content}");
}

#[test]
fn openclaw_agents_route_wraps_runtime_notes_without_wrapping_value_rows() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawAgents;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_agents_defaults = Some(crate::openclaw_config::OpenClawAgentsDefaults {
        model: Some(crate::openclaw_config::OpenClawDefaultModel {
            primary: "demo/primary".to_string(),
            fallbacks: Vec::new(),
            extra: std::collections::HashMap::new(),
        }),
        models: None,
        extra: std::collections::HashMap::from([
            (
                "workspace".to_string(),
                json!("./workspace-path-that-would-wrap"),
            ),
            ("timeout".to_string(), json!(42)),
            ("contextTokens".to_string(), json!(8192)),
            ("maxConcurrent".to_string(), json!(2)),
            ("visibilityNote".to_string(), json!("retained after wrap")),
        ]),
    });

    let rendered = render_with_size(&app, &data, 58, 50);
    let content = content_text(&app, &rendered);
    let workspace_line = line_index(
        &content,
        &buffer_cell_text(texts::tui_openclaw_agents_workspace()),
    );
    let timeout_line = line_index(
        &content,
        &buffer_cell_text(texts::tui_openclaw_agents_timeout()),
    );
    let context_line = line_index(
        &content,
        &buffer_cell_text(texts::tui_openclaw_agents_context_tokens()),
    );
    let max_line = line_index(
        &content,
        &buffer_cell_text(texts::tui_openclaw_agents_max_concurrent()),
    );
    let warning_tail = line_index(&content, "timeoutSeconds.");
    let json_tail = line_index(&content, "after wrap\"");

    assert!(warning_tail < workspace_line, "{content}");
    assert_eq!(timeout_line, workspace_line + 1, "{content}");
    assert_eq!(context_line, timeout_line + 1, "{content}");
    assert_eq!(max_line, context_line + 1, "{content}");
    assert!(json_tail > max_line, "{content}");
}

#[test]
fn openclaw_agents_route_warns_when_legacy_timeout_is_not_migratable() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawAgents;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_agents_defaults = Some(crate::openclaw_config::OpenClawAgentsDefaults {
        model: Some(crate::openclaw_config::OpenClawDefaultModel {
            primary: "demo/primary".to_string(),
            fallbacks: Vec::new(),
            extra: std::collections::HashMap::new(),
        }),
        models: None,
        extra: std::collections::HashMap::from([("timeout".to_string(), json!("manual-value"))]),
    });

    let all = all_text(&render(&app, &data));

    assert!(
        all.contains(texts::tui_openclaw_agents_legacy_timeout_title()),
        "{all}"
    );
    assert!(all.contains("manual-value"), "{all}");
}

#[test]
fn openclaw_agents_route_render_keeps_selected_runtime_row_visible_with_wrapped_notes() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawAgents;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_agents_defaults = Some(crate::openclaw_config::OpenClawAgentsDefaults {
        model: Some(crate::openclaw_config::OpenClawDefaultModel {
            primary: "demo/primary".to_string(),
            fallbacks: vec!["demo/fallback-a".to_string()],
            extra: std::collections::HashMap::from([(
                "retainedModelField".to_string(),
                json!("retained model field that keeps wrapping in short terminals"),
            )]),
        }),
        models: None,
        extra: std::collections::HashMap::from([
            (
                "workspace".to_string(),
                json!("./workspace-path-that-keeps-wrapping-through-the-runtime-block"),
            ),
            ("timeout".to_string(), json!(42)),
            ("contextTokens".to_string(), json!(false)),
            ("maxConcurrent".to_string(), json!(2)),
            (
                "retainedRuntimeField".to_string(),
                json!("retained runtime field that also wraps after the selected row"),
            ),
        ]),
    });

    let mut form = crate::cli::tui::app::OpenClawAgentsFormState::from_snapshot(
        data.config.openclaw_agents_defaults.as_ref(),
    );
    form.section = crate::cli::tui::app::OpenClawAgentsSection::Runtime;
    form.row = 3;
    app.openclaw_agents_form = Some(form);

    let rendered = render_with_size(&app, &data, 72, 16);
    let content = content_text(&app, &rendered);

    let max_row = line_with(&content, &buffer_cell_text("[2]"));
    assert!(
        max_row.contains(&buffer_cell_text(
            texts::tui_openclaw_agents_max_concurrent()
        )),
        "{content}"
    );
}

#[test]
fn openclaw_agents_route_render_keeps_off_catalog_values_visible_without_raw_json_view() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawAgents;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows = vec![openclaw_provider_row(
        "catalog",
        "目录供应商",
        &[("fallback-a", "回退 A")],
    )];
    data.config.openclaw_agents_defaults = Some(crate::openclaw_config::OpenClawAgentsDefaults {
        model: Some(crate::openclaw_config::OpenClawDefaultModel {
            primary: "missing/current-primary".to_string(),
            fallbacks: vec![
                "catalog/fallback-a".to_string(),
                "missing/off-catalog".to_string(),
            ],
            extra: std::collections::HashMap::new(),
        }),
        models: None,
        extra: std::collections::HashMap::new(),
    });

    let rendered = render(&app, &data);
    let all = all_text(&rendered);
    let content = content_text(&app, &rendered);
    let primary_row = line_with(
        &content,
        &buffer_cell_text("missing/current-primary (供应商未配置)"),
    );
    let fallback_row = line_with(
        &content,
        &buffer_cell_text("missing/off-catalog (供应商未配置)"),
    );

    assert!(
        primary_row.contains(&buffer_cell_text("missing/current-primary (供应商未配置)")),
        "{all}"
    );
    assert!(
        fallback_row.contains(&buffer_cell_text("missing/off-catalog (供应商未配置)")),
        "{all}"
    );
    assert!(
        all.contains(&buffer_cell_text(
            texts::tui_openclaw_agents_add_fallback_disabled()
        )),
        "{all}"
    );
    assert!(
        !all.contains("\"primary\": \"missing/current-primary\""),
        "agents route should not fall back to a raw JSON section view: {all}"
    );
}

#[test]
fn openclaw_agents_route_render_shows_disabled_add_fallback_row_when_no_models_remain() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawAgents;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows = vec![openclaw_provider_row(
        "demo",
        "Demo Provider",
        &[("primary", "Primary"), ("fallback-a", "Fallback A")],
    )];
    data.config.openclaw_agents_defaults = Some(crate::openclaw_config::OpenClawAgentsDefaults {
        model: Some(crate::openclaw_config::OpenClawDefaultModel {
            primary: "demo/primary".to_string(),
            fallbacks: vec!["demo/fallback-a".to_string()],
            extra: std::collections::HashMap::new(),
        }),
        models: None,
        extra: std::collections::HashMap::new(),
    });

    let all = all_text(&render(&app, &data));

    assert!(
        all.contains(&buffer_cell_text("No fallback models available to add")),
        "{all}"
    );
    assert!(
        !all.contains(&buffer_cell_text("+ No fallback models available to add")),
        "{all}"
    );
    assert!(
        !all.contains(&buffer_cell_text("+ Add fallback model")),
        "{all}"
    );
    assert!(
        !all.contains(&buffer_cell_text("> No fallback models available to add")),
        "{all}"
    );
}

#[test]
fn openclaw_agents_route_surfaces_preserved_non_string_runtime_values() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawAgents;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_agents_defaults = Some(crate::openclaw_config::OpenClawAgentsDefaults {
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

    let all = all_text(&render(&app, &data));

    assert!(
        all.contains("false (preserved non-standard value)"),
        "{all}"
    );
    assert!(all.contains("null (preserved non-standard value)"), "{all}");
    assert!(
        all.contains("{\"raw\":3} (preserved non-standard value)"),
        "{all}"
    );
    assert!(
        all.contains("Non-standard runtime values are preserved until you replace them."),
        "{all}"
    );
    assert!(!all.contains("Timeout (seconds): Not set"), "{all}");
    assert!(!all.contains("Context Tokens: Not set"), "{all}");
    assert!(!all.contains("Max Concurrent: Not set"), "{all}");
}

#[test]
fn workspace_daily_memory_route_render_shows_memory_files_and_directory_label() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawDailyMemory;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_workspace = OpenClawWorkspaceSnapshot {
        directory_path: std::path::PathBuf::from("/tmp/.openclaw/workspace/"),
        file_exists: std::collections::HashMap::new(),
        daily_memory_files: vec![DailyMemoryFileInfo {
            filename: "2026-03-20.md".to_string(),
            date: "2026-03-20".to_string(),
            size_bytes: 12,
            modified_at: 1,
            preview: "remember this".to_string(),
        }],
    };

    let all = all_text(&render(&app, &data));

    assert!(all.contains(texts::tui_openclaw_daily_memory_title()));
    assert!(all.contains(texts::tui_openclaw_daily_memory_directory_label()));
    assert!(all.contains("/tmp/.openclaw/workspace/memory"), "{all}");
    assert!(!all.contains("/tmp/.openclaw/workspace//memory"), "{all}");
    assert!(all.contains("2026-03-20.md"));
    assert!(all.contains("remember this"));
}

#[test]
fn workspace_daily_memory_route_render_keeps_existing_structure() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawDailyMemory;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_workspace = OpenClawWorkspaceSnapshot {
        directory_path: std::path::PathBuf::from("/tmp/.openclaw/workspace/"),
        file_exists: std::collections::HashMap::new(),
        daily_memory_files: vec![DailyMemoryFileInfo {
            filename: "2026-03-20.md".to_string(),
            date: "2026-03-20".to_string(),
            size_bytes: 12,
            modified_at: 1,
            preview: "remember this".to_string(),
        }],
    };

    let buf = render(&app, &data);
    let all = all_text(&buf);
    let content = content_text(&app, &buf);

    assert!(
        all.contains(texts::tui_openclaw_daily_memory_title()),
        "{all}"
    );
    assert!(
        all.contains(texts::tui_openclaw_daily_memory_directory_label()),
        "{all}"
    );
    assert!(all.contains("2026-03-20.md"), "{all}");
    assert!(all.contains("remember this"), "{all}");
    assert!(
        !content.contains(&buffer_cell_text("Workspace 文件")),
        "{content}"
    );
    assert!(
        !content.contains(&buffer_cell_text(texts::tui_openclaw_workspace_title())),
        "{content}"
    );
}

#[test]
fn workspace_daily_memory_route_render_shows_search_results_when_query_is_active() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawDailyMemory;
    app.focus = Focus::Content;
    app.filter.buffer = "focus".to_string();
    app.openclaw_daily_memory_search_query = "focus".to_string();
    app.openclaw_daily_memory_search_results =
        vec![crate::commands::workspace::DailyMemorySearchResult {
            filename: "2026-03-18.md".to_string(),
            date: "2026-03-18".to_string(),
            size_bytes: 10,
            modified_at: 1,
            snippet: "focus snippet".to_string(),
            match_count: 2,
        }];

    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_workspace = OpenClawWorkspaceSnapshot {
        directory_path: std::path::PathBuf::from("/tmp/.openclaw/workspace"),
        file_exists: std::collections::HashMap::new(),
        daily_memory_files: vec![],
    };

    let all = all_text(&render(&app, &data));

    assert!(all.contains("2026-03-18.md"));
    assert!(all.contains("focus snippet"));
    assert!(!all.contains(texts::tui_openclaw_daily_memory_empty()));
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
fn openclaw_provider_list_key_bar_hides_stream_check_hint() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::Providers;
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
    assert!(!all.contains("stream check"));
}

#[test]
fn openclaw_provider_list_key_bar_uses_additive_mode_actions() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::Providers;
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

    assert!(all.contains("s add/remove"));
    assert!(all.contains("x set default"));
    assert!(!all.contains("s switch"));
}

#[test]
fn openclaw_provider_detail_key_bar_hides_stream_check_hint() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
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
    assert!(!all.contains("stream check"));
}

#[test]
fn openclaw_provider_detail_key_bar_uses_additive_mode_actions() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
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

    assert!(all.contains("s add/remove"));
    assert!(all.contains("x set default"));
    assert!(!all.contains("s switch"));
}

#[test]
fn openclaw_provider_list_key_bar_shows_edit_for_tracked_provider() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::Providers;
    app.focus = Focus::Content;

    let buf = render(&app, &minimal_data(&app.app_type));
    let all = all_text(&buf);

    assert!(all.contains("e edit"), "{all}");
    assert!(all.contains("x set default"), "{all}");
}

#[test]
fn openclaw_provider_detail_key_bar_shows_edit_for_tracked_provider() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ProviderDetail {
        id: "p1".to_string(),
    };
    app.focus = Focus::Content;

    let buf = render(&app, &minimal_data(&app.app_type));
    let all = all_text(&buf);

    assert!(all.contains("e edit"), "{all}");
    assert!(all.contains("x set default"), "{all}");
}

#[test]
fn openclaw_provider_detail_shows_actual_default_model_id() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ProviderDetail {
        id: "p1".to_string(),
    };
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows[0].is_default_model = true;
    data.providers.rows[0].primary_model_id = Some("primary-model".to_string());
    data.providers.rows[0].default_model_id = Some("fallback-model".to_string());

    let buf = render(&app, &data);
    let mut all = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            all.push_str(buf[(x, y)].symbol());
        }
        all.push('\n');
    }

    assert!(all.contains("fallback-model"));
    assert!(!all.contains("Model: primary-model"));
}

#[test]
fn openclaw_tui_provider_list_uses_saved_name_not_model_name() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::Providers;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows[0].provider = Provider::with_id(
        "p1".to_string(),
        "Saved Snapshot Name".to_string(),
        json!({
            "api": "openai-completions",
            "models": [
                {"id": "live-model", "name": "Live Model Name"}
            ]
        }),
        None,
    );

    let all = all_text(&render(&app, &data));

    assert!(all.contains("Saved Snapshot Name"), "{all}");
    assert!(!all.contains("Live Model Name"), "{all}");
}

#[test]
fn openclaw_tui_provider_detail_uses_saved_name_and_keeps_model_separate() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ProviderDetail {
        id: "p1".to_string(),
    };
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows[0].provider = Provider::with_id(
        "p1".to_string(),
        "Saved Snapshot Name".to_string(),
        json!({
            "api": "openai-completions",
            "models": [
                {"id": "live-model", "name": "Live Model Name"}
            ]
        }),
        None,
    );
    data.providers.rows[0].primary_model_id = Some("live-model".to_string());

    let all = all_text(&render(&app, &data));

    assert!(all.contains("Saved Snapshot Name"), "{all}");
    assert!(all.contains("live-model"), "{all}");
    assert!(!all.contains("Name: Live Model Name"), "{all}");
}

#[test]
fn openclaw_tui_provider_search_uses_saved_name_not_model_name() {
    let mut app = App::new(Some(AppType::OpenClaw));
    app.filter.buffer = "live model".to_string();

    let mut data = minimal_data(&app.app_type);
    data.providers.rows[0].provider = Provider::with_id(
        "p1".to_string(),
        "Saved Snapshot Name".to_string(),
        json!({
            "api": "openai-completions",
            "models": [
                {"id": "live-model", "name": "Live Model Name"}
            ]
        }),
        None,
    );

    assert!(super::provider_rows_filtered(&app, &data).is_empty());

    app.filter.buffer = "saved snapshot".to_string();
    assert_eq!(super::provider_rows_filtered(&app, &data).len(), 1);
}

#[test]
fn openclaw_provider_detail_localizes_status_copy_in_chinese() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ProviderDetail {
        id: "p1".to_string(),
    };
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows[0].is_default_model = true;
    data.providers.rows[0].default_model_id = Some("fallback-model".to_string());

    let all = all_text(&render(&app, &data));
    let compact = all.replace(' ', "");

    assert!(compact.contains("状态:默认"), "{all}");
    assert!(compact.contains("模型:fallback-model"), "{all}");
    assert!(!all.contains("Status"), "{all}");
    assert!(!all.contains("Model:"), "{all}");
}

#[test]
fn openclaw_provider_detail_localizes_tracked_status_in_chinese() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ProviderDetail {
        id: "p1".to_string(),
    };
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows[0].is_default_model = false;
    data.providers.rows[0].is_in_config = true;
    data.providers.rows[0].is_saved = true;

    let all = all_text(&render(&app, &data));
    let compact = all.replace(' ', "");

    assert!(compact.contains("状态:配置中+已保存"), "{all}");
    assert!(!all.contains("Status"), "{all}");
}

#[test]
fn openclaw_provider_detail_treats_live_only_status_as_tracked_copy() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ProviderDetail {
        id: "p1".to_string(),
    };
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows[0].is_default_model = false;
    data.providers.rows[0].is_in_config = true;
    data.providers.rows[0].is_saved = false;

    let all = all_text(&render(&app, &data));

    assert!(
        all.contains(texts::tui_openclaw_status_in_config_and_saved()),
        "{all}"
    );
    assert!(
        !all.contains(texts::tui_openclaw_status_live_only()),
        "{all}"
    );
}

#[test]
fn openclaw_provider_detail_reports_saved_only_status_truthfully() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ProviderDetail {
        id: "p1".to_string(),
    };
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows[0].is_default_model = false;
    data.providers.rows[0].is_in_config = false;
    data.providers.rows[0].is_saved = true;

    let all = all_text(&render(&app, &data));

    assert!(
        all.contains(texts::tui_openclaw_status_saved_only()),
        "{all}"
    );
    assert!(
        !all.contains(texts::tui_openclaw_status_in_config_and_saved()),
        "{all}"
    );
}

#[test]
fn openclaw_provider_list_hides_marker_for_removed_row() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::Providers;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows[0].is_default_model = false;
    data.providers.rows[0].is_in_config = false;
    data.providers.rows[0].is_saved = true;

    let buf = render(&app, &data);
    let provider_line = (0..buf.area.height)
        .map(|y| line_at(&buf, y))
        .find(|line| line.contains("Demo Provider"))
        .expect("provider row rendered");

    assert!(provider_line.contains("Demo Provider"), "{provider_line}");
    assert!(!provider_line.contains("+"), "{provider_line}");
}

#[test]
fn openclaw_provider_list_treats_live_only_marker_as_tracked_marker() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::Providers;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows[0].is_default_model = false;
    data.providers.rows[0].is_in_config = true;
    data.providers.rows[0].is_saved = false;

    let buf = render(&app, &data);
    let provider_line = (0..buf.area.height)
        .map(|y| line_at(&buf, y))
        .find(|line| line.contains("Demo Provider"))
        .expect("provider row rendered");

    assert!(provider_line.contains("+"), "{provider_line}");
    assert!(!provider_line.contains("~"), "{provider_line}");
}

#[test]
fn openclaw_provider_list_key_bar_localizes_actions_in_chinese() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::Providers;
    app.focus = Focus::Content;

    let all = all_text(&render(&app, &minimal_data(&app.app_type)));
    let compact = all.replace(' ', "");

    assert!(compact.contains("s添加/移除"), "{all}");
    assert!(compact.contains("x设为默认"), "{all}");
    assert!(!all.contains("add/remove"), "{all}");
    assert!(!all.contains("set default"), "{all}");
}

#[test]
fn openclaw_provider_detail_key_bar_localizes_actions_in_chinese() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ProviderDetail {
        id: "p1".to_string(),
    };
    app.focus = Focus::Content;

    let all = all_text(&render(&app, &minimal_data(&app.app_type)));
    let compact = all.replace(' ', "");

    assert!(compact.contains("s添加/移除"), "{all}");
    assert!(compact.contains("x设为默认"), "{all}");
    assert!(!all.contains("add/remove"), "{all}");
    assert!(!all.contains("set default"), "{all}");
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
