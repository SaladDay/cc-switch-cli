use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    backend::TestBackend,
    buffer::Buffer,
    style::{Color, Modifier, Style},
    Terminal,
};
use serde_json::{json, Value};
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
            Action, App, CodexHistoryConfirmMode, CodexHistoryConfirmState, ConfigItem,
            ConfirmAction, ConfirmOverlay, EditorKind, EditorSubmit, Focus, Overlay, SettingsItem,
            TextInputState, TextSubmit, UsagePane,
        },
        data::{
            ConfigSnapshot, McpSnapshot, ModelPricingRow, ModelPricingSnapshot,
            OpenClawWorkspaceSnapshot, PromptsSnapshot, ProviderHealthSnapshot, ProviderRow,
            ProvidersSnapshot, ProxySnapshot, ProxyTargetSnapshot, SkillsSnapshot, UiData,
            UsageDailyModelBucket, UsageLogRow, UsageProviderStatsRow, UsageRangePreset,
            UsageSnapshot, UsageSummarySnapshot, UsageTrendBucket,
        },
        form::{
            ClaudeModelPickerColumn, FormFocus, FormState, McpKeyValueKind, PromptMetaFormState,
            ProviderAddField, TextInput, UsageQueryField,
        },
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
fn visible_text_window_keeps_only_the_ascii_viewport_tail() {
    let value = format!("{}TAIL", "x".repeat(1_000_000));

    let (visible, cursor_x) = super::visible_text_window(&value, value.len(), 10);

    assert_eq!(visible, "xxxxxTAIL");
    assert_eq!(cursor_x, 9);
}

#[test]
fn visible_text_window_respects_unicode_display_width() {
    let (visible, cursor_x) = super::visible_text_window("ab你cd", "ab你".len(), 4);

    assert_eq!(visible, "b你c");
    assert_eq!(cursor_x, 3);
}

#[test]
fn visible_text_window_bounds_zero_width_unicode_output() {
    let value = "\u{0301}".repeat(1_000_000);

    let (visible, cursor_x) = super::visible_text_window(&value, value.len(), 8);

    assert!(visible.chars().count() <= 48);
    assert_eq!(cursor_x, 0);
}

#[test]
fn visible_text_window_bounds_a_million_multibyte_characters() {
    let value = format!("{}TAIL", "你".repeat(1_000_000));

    let (visible, cursor_x) = super::visible_text_window(&value, value.len(), 10);

    assert!(UnicodeWidthStr::width(visible.as_str()) <= 10);
    assert!(visible.chars().count() <= 10);
    assert!(visible.ends_with("TAIL"));
    assert_eq!(cursor_x, 8);
}

#[test]
fn inline_window_keeps_plaintext_visible_while_bounding_large_values() {
    let input = TextInput::new("x".repeat(1_000_000));

    let (visible, cursor_x) = super::inline_input_window(&input, 10);

    assert_eq!(visible, "xxxxxxxxx");
    assert_eq!(cursor_x, 9);
}

#[test]
fn inline_window_keeps_multibyte_plaintext_visible_within_display_width() {
    let input = TextInput::new("你".repeat(1_000_000));

    let (visible, cursor_x) = super::inline_input_window(&input, 10);

    assert_eq!(visible, "你你你你");
    assert_eq!(UnicodeWidthStr::width(visible.as_str()), 8);
    assert_eq!(cursor_x, 8);
}

#[test]
fn form_json_preview_scrolls_through_wrapped_segments_of_one_long_line() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");
    let backend = TestBackend::new(18, 6);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let theme = theme_for(&AppType::Claude);
    let text = format!("{{\"value\":\"{}VISIBLE_TAIL\"}}", "A".repeat(60));

    terminal
        .draw(|frame| {
            super::render_form_json_preview(frame, &text, 2, true, frame.area(), &theme);
        })
        .expect("preview should render");

    let rendered = all_text(terminal.backend().buffer());
    assert!(!rendered.contains("{\"value\""), "{rendered}");
    assert!(rendered.contains("VISIBLE_TA"), "{rendered}");
    assert!(rendered.contains("IL\"}"), "{rendered}");
}

#[test]
fn passive_text_helpers_bound_leading_space_and_zero_width_inputs() {
    let leading = format!("{}secret-tail", " ".repeat(1_000_000));
    let combining = "\u{0301}".repeat(1_000_000);

    let leading_summary = super::bounded_trimmed_text_for_display(&leading);
    let combining_summary = super::bounded_trimmed_text_for_display(&combining);
    let truncated = super::truncate_to_display_width(&combining, 8);

    assert_eq!(leading_summary, "…");
    assert!(combining_summary.chars().count() <= 2_049);
    assert!(combining_summary.ends_with('…'));
    assert!(truncated.chars().count() <= 49);
    assert!(truncated.ends_with('…'));
}

#[test]
fn span_truncation_clips_styled_lines_at_every_boundary_width() {
    use ratatui::style::{Color, Style};
    use ratatui::text::Span;

    let spans = || {
        vec![
            Span::raw("abc"),
            Span::styled("def", Style::default().fg(Color::Red)),
            Span::raw("ghi"),
        ]
    };
    let text = |spans: Vec<Span<'_>>| {
        spans
            .iter()
            .map(|span| span.content.to_string())
            .collect::<String>()
    };

    // Fits: untouched, styles and all.
    assert_eq!(super::truncate_spans_to_width(spans(), 9), spans());
    assert_eq!(super::truncate_spans_to_width(spans(), 99), spans());

    // Cut mid-span, cut on a span boundary, and cut down to the marker alone.
    // The marker follows the icon mode, which is process-global here.
    let cut = super::truncation_marker();
    assert_eq!(
        text(super::truncate_spans_to_width(spans(), 8)),
        format!("abcdefg{cut}")
    );
    assert_eq!(
        text(super::truncate_spans_to_width(spans(), 4)),
        format!("abc{cut}")
    );
    assert_eq!(text(super::truncate_spans_to_width(spans(), 1)), cut);
    assert!(super::truncate_spans_to_width(spans(), 0).is_empty());

    // The styled middle span keeps its style when it is the one clipped.
    let clipped = super::truncate_spans_to_width(spans(), 6);
    assert_eq!(text(clipped.clone()), format!("abcde{cut}"));
    assert_eq!(clipped[1].style.fg, Some(Color::Red));

    // A pathological value clips instead of walking the whole string.
    let huge = vec![Span::raw("x".repeat(70_000))];
    assert_eq!(
        super::spans_display_width(&super::truncate_spans_to_width(huge, 20)),
        20
    );
}

#[test]
fn visible_selection_window_stays_fixed_around_edges_and_selection() {
    assert_eq!(super::visible_selection_window(0, 0, 10), 0..0);
    assert_eq!(super::visible_selection_window(100, 0, 10), 0..10);
    assert_eq!(super::visible_selection_window(100, 50, 10), 45..55);
    assert_eq!(super::visible_selection_window(100, 99, 10), 90..100);
    assert_eq!(super::visible_selection_window(3, usize::MAX, 10), 0..3);
}

#[test]
fn plaintext_json_preview_bounds_a_million_byte_object_key() {
    let mut object = serde_json::Map::new();
    object.insert("K".repeat(1_000_000), json!("secret-value"));

    let preview = super::bounded_json_preview(&serde_json::Value::Object(object));
    let rendered = serde_json::to_string(&preview).expect("bounded JSON should serialize");

    assert!(rendered.len() < 1_000, "preview was not bounded");
    assert!(rendered.contains("secret-value"), "{rendered}");
    assert!(!rendered.contains("[redacted]"), "{rendered}");
}

#[test]
fn provider_form_shows_api_key_in_table_value() {
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.claude_api_key.set("sk-test-1234567890");

    let (_label, value) = super::provider_field_label_and_value(
        &form,
        crate::cli::tui::form::ProviderAddField::ClaudeApiKey,
    );
    assert_eq!(value, "sk-test-1234567890");
}

#[test]
fn provider_full_url_badge_stays_in_value_and_is_localized() {
    let _lock = lock_env();
    for (language, badge) in [
        (Language::English, "[Full URL]"),
        (Language::Chinese, "[完整 URL]"),
    ] {
        let _language = use_test_language(language);
        for (app_type, field) in [
            (AppType::Claude, ProviderAddField::ClaudeBaseUrl),
            (AppType::Codex, ProviderAddField::CodexBaseUrl),
        ] {
            let mut form = crate::cli::tui::form::ProviderAddFormState::new(app_type);
            match field {
                ProviderAddField::ClaudeBaseUrl => {
                    form.claude_base_url.set("https://relay.example/request")
                }
                ProviderAddField::CodexBaseUrl => {
                    form.codex_base_url.set("https://relay.example/request")
                }
                _ => unreachable!(),
            }
            let (label_before, value_before) = super::provider_field_label_and_value(&form, field);
            form.is_full_url = true;
            let (label_after, value_after) = super::provider_field_label_and_value(&form, field);

            assert_eq!(label_after, label_before);
            assert_eq!(value_before, "https://relay.example/request");
            assert_eq!(
                value_after,
                format!("{badge} https://relay.example/request")
            );
        }
    }
}

#[test]
fn provider_full_url_badge_remains_visible_while_editing_on_narrow_terminal() {
    let _lock = lock_env();
    let _language = use_test_language(Language::English);
    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.focus = FormFocus::Fields;
    form.claude_base_url
        .set("https://relay.example/custom/chat/completions");
    form.is_full_url = true;
    form.field_idx = form
        .fields()
        .iter()
        .position(|field| *field == ProviderAddField::ClaudeBaseUrl)
        .expect("Claude Base URL field");
    assert!(form.begin_main_text_edit(ProviderAddField::ClaudeBaseUrl));
    app.form = Some(FormState::ProviderAdd(form));

    let rendered = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        72,
        24,
    ));
    assert!(rendered.contains("[Full URL]"), "{rendered}");
    assert!(rendered.contains("pletions"), "{rendered}");
}

#[test]
fn claude_model_picker_renders_role_scoped_one_m_controls_and_dynamic_keys() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.claude_haiku_model.set("haiku-model");
    form.set_claude_model_from_config(1, "sonnet-model[1M]");
    form.set_claude_model_from_config(2, "opus-model");
    form.set_claude_model_from_config(3, "fable-model[1M]");
    form.set_claude_model_from_config(4, "subagent-model");
    app.form = Some(FormState::ProviderAdd(form));
    app.overlay = Overlay::ClaudeModelPicker {
        selected: 1,
        column: ClaudeModelPickerColumn::OneM,
        editing: false,
    };

    let one_m = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        160,
        32,
    ));
    assert!(one_m.contains("Field"), "{one_m}");
    assert!(one_m.contains("Value"), "{one_m}");
    assert!(one_m.contains("1M"), "{one_m}");
    assert!(one_m.contains("[x]"), "{one_m}");
    assert!(one_m.contains("[ ]"), "{one_m}");
    assert!(one_m.matches('—').count() >= 1, "{one_m}");
    assert!(one_m.contains("Default Haiku Model"), "{one_m}");
    assert!(one_m.contains("Default Sonnet Model"), "{one_m}");
    assert!(one_m.contains("Default Opus Model"), "{one_m}");
    assert!(one_m.contains("Default Fable Model"), "{one_m}");
    assert!(one_m.contains("Subagent Model"), "{one_m}");
    assert!(!one_m.contains("Reasoning Model"), "{one_m}");
    assert!(one_m.contains("switch column"), "{one_m}");
    assert!(one_m.contains("toggle"), "{one_m}");
    assert!(one_m.contains("Enter toggle"), "{one_m}");
    assert!(
        one_m.contains("Press Enter to toggle the 1M declaration."),
        "{one_m}"
    );
    assert!(!one_m.contains("fetch model"), "{one_m}");
    assert!(!one_m.contains("fill all"), "{one_m}");

    app.overlay = Overlay::ClaudeModelPicker {
        selected: 1,
        column: ClaudeModelPickerColumn::Model,
        editing: false,
    };
    let model = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        160,
        32,
    ));
    assert!(model.contains("fetch model"), "{model}");
    assert!(model.contains("edit"), "{model}");
    assert!(model.contains("Enter edit"), "{model}");
    assert!(model.contains("Space fetch model"), "{model}");
    assert!(
        model.contains("Press Space to auto-fetch models from API."),
        "{model}"
    );
    assert!(!model.contains("fill all"), "{model}");
}

#[test]
fn claude_model_picker_keeps_one_m_visible_and_truncates_on_narrow_terminals() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.set_claude_model_from_config(
        1,
        "an-extremely-long-provider-model-name-with-a-tail-that-must-not-show[1M]",
    );
    app.form = Some(FormState::ProviderAdd(form));
    app.overlay = Overlay::ClaudeModelPicker {
        selected: 1,
        column: ClaudeModelPickerColumn::OneM,
        editing: false,
    };

    let narrow = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        64,
        18,
    ));
    assert!(narrow.contains("1M"), "{narrow}");
    assert!(narrow.contains("[x]"), "{narrow}");
    assert!(narrow.contains('…'), "{narrow}");
    assert!(!narrow.contains("must-not-show"), "{narrow}");

    let smoke = render_with_size(&app, &minimal_data(&app.app_type), 40, 12);
    assert_eq!(smoke.area.width, 40);
    assert_eq!(smoke.area.height, 12);
}

#[test]
fn claude_model_picker_no_color_marks_only_the_focused_cell_as_reversed() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::set("NO_COLOR", "1");
    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.set_claude_model_from_config(2, "sonnet-model[1M]");
    app.form = Some(FormState::ProviderAdd(form));
    app.overlay = Overlay::ClaudeModelPicker {
        selected: 2,
        column: ClaudeModelPickerColumn::OneM,
        editing: false,
    };

    let buf = render_with_size(&app, &minimal_data(&app.app_type), 160, 32);
    let (row_y, row) = (0..buf.area.height)
        .map(|y| (y, line_at(&buf, y)))
        .find(|(_, line)| line.contains("[x]") && line.contains("sonnet-model"))
        .unwrap_or_else(|| panic!("missing Sonnet row:\n{}", all_text(&buf)));
    let check_x = cell_column_of(&buf, row_y, "[x]").expect("checkbox column");
    let model_x = cell_column_of(&buf, row_y, "sonnet-model").expect("model column");
    assert!(buf[(check_x, row_y)].modifier.contains(Modifier::REVERSED));
    assert!(!buf[(model_x, row_y)].modifier.contains(Modifier::REVERSED));
    assert!(buf[(model_x, row_y)].modifier.contains(Modifier::BOLD));
    assert!(!row.contains('>'), "{row}");
}

#[test]
fn provider_and_usage_url_rows_show_the_complete_value_without_mutating_it() {
    let raw = "https://user:pass@example.test/v1/private?api_key=P2_URL_SECRET#fragment";
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.claude_base_url.set(raw);
    form.usage_query_base_url.set(raw);

    let (_, provider_value) = super::provider_field_label_and_value(
        &form,
        crate::cli::tui::form::ProviderAddField::ClaudeBaseUrl,
    );
    let (_, usage_value) = super::usage_query_field_label_and_value(
        &form,
        crate::cli::tui::form::UsageQueryField::BaseUrl,
    );

    assert_eq!(provider_value, raw);
    assert_eq!(usage_value, raw);
    assert_eq!(form.claude_base_url.value, raw);
    assert_eq!(form.usage_query_base_url.value, raw);
}

#[test]
fn provider_inline_api_key_edit_renders_plaintext() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let secret = "sk-inline-super-secret";
    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.focus = FormFocus::Fields;
    form.name.set("Demo");
    form.claude_api_key.set(secret);
    assert!(form.begin_main_text_edit(ProviderAddField::ClaudeApiKey));
    app.form = Some(FormState::ProviderAdd(form));

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        160,
        40,
    ));
    assert!(all.contains(secret), "{all}");
    assert!(all.contains("Provider Config"), "{all}");
    assert!(!all.contains("[redacted]"), "{all}");
    assert!(!all.contains("apply & next"), "{all}");
    assert!(!all.contains("apply & previous"), "{all}");
}

#[test]
fn mcp_preview_shows_environment_values_in_plaintext() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let secret = "value-with-an-unremarkable-key";
    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Mcp;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::McpAddFormState::new();
    form.focus = FormFocus::JsonPreview;
    form.id.set("server");
    form.name.set("Server");
    form.command.set("node");
    form.env_rows.push(crate::cli::tui::form::McpKeyValueRow {
        key: "CUSTOM_VALUE".to_string(),
        value: secret.to_string(),
    });
    app.form = Some(FormState::McpAdd(form));

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        80,
        35,
    ));
    assert!(all.contains(secret), "{all}");
    assert!(all.contains("CUSTOM_VALUE"), "{all}");
    assert!(!all.contains("[redacted]"), "{all}");
}

#[test]
fn mcp_preview_shows_custom_header_values_in_plaintext() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let secret = "custom-header-super-secret";
    let server = crate::app_config::McpServer {
        id: "remote".to_string(),
        name: "Remote".to_string(),
        server: json!({
            "type": "http",
            "url": "https://example.test/mcp",
            "headers": {
                "X-Custom-Auth": secret,
                "User-Agent": "safe-but-hidden-too"
            }
        }),
        apps: crate::app_config::McpApps::default(),
        description: None,
        homepage: None,
        docs: None,
        tags: Vec::new(),
    };
    let mut form = crate::cli::tui::form::McpAddFormState::from_server(&server);
    form.focus = FormFocus::JsonPreview;

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Mcp;
    app.focus = Focus::Content;
    app.form = Some(FormState::McpAdd(form));

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        160,
        40,
    ));
    assert!(all.contains(secret), "{all}");
    assert!(all.contains("safe-but-hidden-too"), "{all}");
    assert!(all.contains("X-Custom-Auth"), "{all}");
    assert!(all.contains("User-Agent"), "{all}");
    assert!(!all.contains("********"), "{all}");
}

#[test]
fn mcp_preview_shows_legacy_codex_http_header_values_in_plaintext() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let secret = "legacy-http-header-super-secret";
    let server = crate::app_config::McpServer {
        id: "remote".to_string(),
        name: "Remote".to_string(),
        server: json!({
            "type": "http",
            "url": "https://example.test/mcp",
            "http_headers": {
                "Authorization": secret
            }
        }),
        apps: crate::app_config::McpApps::default(),
        description: None,
        homepage: None,
        docs: None,
        tags: Vec::new(),
    };
    let mut form = crate::cli::tui::form::McpAddFormState::from_server(&server);
    form.focus = FormFocus::JsonPreview;

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Mcp;
    app.focus = Focus::Content;
    app.form = Some(FormState::McpAdd(form));

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        160,
        40,
    ));
    assert!(all.contains("Authorization"), "{all}");
    assert!(!all.contains("********"), "{all}");
    assert!(all.contains(secret), "{all}");
    assert!(!all.contains("http_headers"), "{all}");
}

#[test]
fn mcp_stdio_fields_and_preview_show_command_and_argument_payloads() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Mcp;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::McpAddFormState::new();
    form.focus = FormFocus::Fields;
    form.id.set("server");
    form.name.set("Server");
    form.command.set("/usr/local/bin/node");
    form.set_args_values(vec![
        "--api-key".to_string(),
        "B6_ARGS_SECRET".to_string(),
        "positional-secret".to_string(),
    ]);
    app.form = Some(FormState::McpAdd(form));

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        160,
        40,
    ));
    assert!(all.contains("/usr/local/bin/node"), "{all}");
    assert!(all.contains("B6_ARGS_SECRET"), "{all}");
    assert!(all.contains("positional-secret"), "{all}");
    assert!(line_with(&all, "\"args\"").contains('['), "{all}");
    assert!(!all.contains("[redacted]"), "{all}");
}

#[test]
fn mcp_remote_fields_and_preview_show_the_complete_url() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Mcp;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::McpAddFormState::new();
    form.focus = FormFocus::Fields;
    form.id.set("remote");
    form.name.set("Remote");
    form.server_type = crate::cli::tui::form::McpTransport::Http;
    form.url
        .set("https://user:pass@mcp.example.test/secret-path?access_token=URL_SECRET#frag");
    app.form = Some(FormState::McpAdd(form));

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        160,
        40,
    ));
    for visible in ["user:pass", "secret-path", "URL_SECRET", "access_token"] {
        assert!(all.contains(visible), "{all}");
    }
}

#[test]
fn mcp_fields_show_invalid_urls_and_shell_commands_verbatim() {
    let mut form = crate::cli::tui::form::McpAddFormState::new();
    form.command.set("sh -c secret");
    let (_, command) =
        super::mcp_field_label_and_value(&form, crate::cli::tui::form::McpAddField::Command);
    assert_eq!(command, "sh -c secret");

    form.server_type = crate::cli::tui::form::McpTransport::Http;
    form.url.set("not-a-url-with-secret");
    let (_, url) = super::mcp_field_label_and_value(&form, crate::cli::tui::form::McpAddField::Url);
    assert_eq!(url, "not-a-url-with-secret");
}

#[test]
fn mcp_plaintext_preview_never_mutates_the_serialized_form_payload() {
    let mut form = crate::cli::tui::form::McpAddFormState::new();
    form.command.set("node");
    form.set_args_values(vec![
        "--api-key".to_string(),
        "persistence-secret".to_string(),
    ]);
    form.env_rows.push(crate::cli::tui::form::McpKeyValueRow {
        key: "CUSTOM_VALUE".to_string(),
        value: "environment-secret".to_string(),
    });

    let raw = form.to_mcp_server_json_value();
    let preview = super::bounded_json_preview(&raw);

    assert_eq!(raw["server"]["command"], json!("node"));
    assert_eq!(
        raw["server"]["args"],
        json!(["--api-key", "persistence-secret"])
    );
    assert_eq!(
        raw["server"]["env"]["CUSTOM_VALUE"],
        json!("environment-secret")
    );
    assert_eq!(preview, raw);
}

#[test]
fn mcp_env_picker_renders_environment_values_in_plaintext() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let secret = "picker-super-secret";
    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Mcp;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::McpAddFormState::new();
    form.env_rows.push(crate::cli::tui::form::McpKeyValueRow {
        key: "CUSTOM_VALUE".to_string(),
        value: secret.to_string(),
    });
    app.form = Some(FormState::McpAdd(form));
    app.overlay = Overlay::McpKeyValuePicker {
        kind: McpKeyValueKind::Env,
        selected: 0,
    };

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        100,
        35,
    ));
    assert!(all.contains(secret), "{all}");
    assert!(all.contains("CUSTOM_VALUE"), "{all}");
    assert!(!all.contains("[redacted]"), "{all}");
}

#[test]
fn mcp_headers_picker_shows_values_in_plaintext() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let secret = "Bearer picker-super-secret";
    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Mcp;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::McpAddFormState::new();
    form.header_rows
        .push(crate::cli::tui::form::McpKeyValueRow {
            key: "Authorization".to_string(),
            value: secret.to_string(),
        });
    app.form = Some(FormState::McpAdd(form));
    app.overlay = Overlay::McpKeyValuePicker {
        kind: McpKeyValueKind::Headers,
        selected: 0,
    };

    let rendered = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        100,
        35,
    ));
    assert!(rendered.contains("Authorization"), "{rendered}");
    assert!(rendered.contains(secret), "{rendered}");
    assert!(!rendered.contains("********"), "{rendered}");
    assert!(!rendered.contains("v show"), "{rendered}");
}

#[test]
fn mcp_remote_json_preview_shows_header_values_without_mutating_payload() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let secret = "Bearer preview-super-secret";
    let mut form = crate::cli::tui::form::McpAddFormState::new();
    form.focus = FormFocus::JsonPreview;
    form.server_type = crate::cli::tui::form::McpTransport::Http;
    form.url.set("https://example.com/mcp");
    form.header_rows
        .push(crate::cli::tui::form::McpKeyValueRow {
            key: "Authorization".to_string(),
            value: secret.to_string(),
        });
    assert_eq!(
        form.to_mcp_server_json_value()["server"]["headers"]["Authorization"],
        secret
    );

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Mcp;
    app.focus = Focus::Content;
    app.form = Some(FormState::McpAdd(form));

    let preview = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        160,
        40,
    ));
    assert!(preview.contains("Authorization"), "{preview}");
    assert!(!preview.contains("********"), "{preview}");
    assert!(preview.contains(secret), "{preview}");
}

#[test]
fn mcp_env_picker_materializes_only_the_selected_large_collection_window() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let selected = 9_999usize;
    let mut form = crate::cli::tui::form::McpAddFormState::new();
    form.env_rows = (0..10_000)
        .map(|index| crate::cli::tui::form::McpKeyValueRow {
            key: format!("ENV-{index:05}"),
            value: format!("secret-{index}"),
        })
        .collect();
    form.env_rows[selected].key = format!("SELECTED-ENV-{}", "X".repeat(1_000_000));

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Mcp;
    app.focus = Focus::Content;
    app.form = Some(FormState::McpAdd(form));
    app.overlay = Overlay::McpKeyValuePicker {
        kind: McpKeyValueKind::Env,
        selected,
    };

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        100,
        35,
    ));
    assert!(all.contains("SELECTED-ENV"), "{all}");
    assert!(!all.contains("ENV-00000"), "{all}");
    assert!(!all.contains("[redacted]"), "{all}");
    assert!(all.len() < 20_000, "picker frame was not bounded");
}

#[test]
fn mcp_env_value_editor_shows_the_active_value_in_plaintext() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let secret = "editor-super-secret";
    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Mcp;
    app.focus = Focus::Content;
    app.form = Some(FormState::McpAdd(
        crate::cli::tui::form::McpAddFormState::new(),
    ));
    app.overlay =
        Overlay::McpKeyValueEntryEditor(crate::cli::tui::app::McpKeyValueEntryEditorState {
            kind: McpKeyValueKind::Env,
            row: None,
            return_selected: 0,
            field: crate::cli::tui::app::McpKeyValueEditorField::Value,
            key: TextInput::new("CUSTOM_VALUE"),
            value: TextInput::new(secret),
        });

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        100,
        35,
    ));
    assert!(all.contains(secret), "{all}");
    assert!(all.contains("CUSTOM_VALUE"), "{all}");
    assert!(!all.contains("[redacted]"), "{all}");
}

#[test]
fn model_fetch_picker_renders_only_the_selected_large_collection_window() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let selected = 9_999usize;
    let mut models = (0..10_000)
        .map(|index| format!("FETCHED-{index:05}"))
        .collect::<Vec<_>>();
    models[selected] = format!("SELECTED-FETCHED-{}", "X".repeat(1_000_000));

    let mut app = App::new(Some(AppType::Claude));
    app.overlay = Overlay::ModelFetchPicker {
        request_id: 1,
        field: ProviderAddField::ClaudeModelConfig,
        claude_idx: Some(0),
        input: TextInput::new(""),
        query: String::new(),
        fetching: false,
        models,
        filtered_indices: None,
        filter_incomplete: false,
        error: None,
        selected_idx: selected,
        selection_active: false,
    };

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        100,
        35,
    ));
    assert!(all.contains("SELECTED-FETCHED"), "{all}");
    assert!(!all.contains("FETCHED-00000"), "{all}");
}

#[test]
fn model_fetch_picker_discloses_when_search_results_are_budget_limited() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.overlay = Overlay::ModelFetchPicker {
        request_id: 1,
        field: ProviderAddField::ClaudeModelConfig,
        claude_idx: Some(0),
        input: TextInput::new("model"),
        query: "model".to_string(),
        fetching: false,
        models: vec!["model-a".to_string()],
        filtered_indices: Some(vec![0]),
        filter_incomplete: true,
        error: None,
        selected_idx: 0,
        selection_active: false,
    };

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        100,
        35,
    ));
    assert!(all.contains("Search is limited; refine the query"), "{all}");
}

#[test]
fn provider_inline_cursor_and_error_stay_on_the_selected_row() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.focus = FormFocus::Fields;
    form.name.set("alpha");
    form.name.cursor = 3;
    assert!(form.begin_main_text_edit(ProviderAddField::Name));
    form.set_main_field_error(ProviderAddField::Name, "bad value");
    app.form = Some(FormState::ProviderAdd(form));

    let data = minimal_data(&app.app_type);
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).expect("terminal created");
    terminal
        .draw(|frame| super::render(frame, &app, &data))
        .expect("draw ok");
    let cursor = terminal.get_cursor_position().expect("cursor position");
    let buf = terminal.backend().buffer();

    assert!(line_at(buf, cursor.y).contains("alpha"));
    assert!(line_at(buf, cursor.y + 1).contains("! bad value"));
    assert!(!all_text(buf).contains(" Editing "));
}

#[test]
fn provider_form_uses_one_pane_when_compact_and_two_when_wide() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.focus = FormFocus::Fields;
    app.form = Some(FormState::ProviderAdd(form));
    let data = minimal_data(&app.app_type);

    let compact_fields = all_text(&render_with_size(&app, &data, 80, 35));
    assert!(compact_fields.contains("┌ Fields "), "{compact_fields}");
    assert!(
        !compact_fields.contains("┌ Provider Config "),
        "{compact_fields}"
    );

    if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
        form.focus = FormFocus::JsonPreview;
    }
    let compact_preview = all_text(&render_with_size(&app, &data, 80, 35));
    assert!(!compact_preview.contains("┌ Fields "), "{compact_preview}");
    assert!(
        compact_preview.contains("┌ Provider Config "),
        "{compact_preview}"
    );

    let wide = all_text(&render_with_size(&app, &data, 160, 40));
    assert!(wide.contains("┌ Fields "), "{wide}");
    assert!(wide.contains("┌ Provider Config "), "{wide}");
}

#[test]
fn tui_sessions_empty_state_is_localized_and_mentions_runtime_scan() {
    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Sessions;
    app.focus = Focus::Content;
    app.sessions.loaded_once = true;

    let _lang = use_test_language(Language::English);
    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        160,
        40,
    ));

    assert!(all.contains("No local sessions found"), "{all}");
    assert!(all.contains("local session files"), "{all}");
    assert!(all.contains("usage database"), "{all}");
    assert!(all.contains("Cost"), "{all}");
    assert!(!all.contains("without using the database"), "{all}");
    assert!(all.contains("switch panel"), "{all}");
    assert!(all.contains("PgUp/PgDn"), "{all}");
    assert!(all.contains(texts::tui_key_page()), "{all}");
    assert!(!all.contains("←→/h/l"), "{all}");
    assert!(!all.contains("show all"), "{all}");
}

#[test]
fn tui_usage_empty_state_renders_dashboard_shell() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Usage;
    app.focus = Focus::Content;

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        160,
        40,
    ));

    assert!(all.contains("Usage Statistics"), "{all}");
    assert!(all.contains("Usage Trend"), "{all}");
    assert!(all.contains("No usage recorded"), "{all}");
    assert!(all.contains("Today"), "{all}");
    assert!(all.contains("7 days"), "{all}");
    assert!(all.contains("30 days"), "{all}");
    assert!(all.contains("custom range"), "{all}");
    // Tab cycles the trend metric on this page; "switch panel" belongs to
    // the UsageLogs route only.
    assert!(all.contains("switch metric"), "{all}");
    assert!(!all.contains("switch panel"), "{all}");
    assert!(all.contains("details"), "{all}");
    assert!(all.contains("pricing"), "{all}");
    assert!(!all.contains("Provider Stats"), "{all}");
}

#[test]
fn tui_usage_loading_state_renders_non_blocking_placeholder() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Usage;
    app.focus = Focus::Content;
    app.usage
        .start_loading(AppType::Claude, UsageRangePreset::SevenDays);
    let data = minimal_data(&app.app_type);

    let all = all_text(&render_with_size(&app, &data, 160, 40));

    assert!(all.contains("Usage Statistics"), "{all}");
    assert!(all.contains("Loading..."), "{all}");
    assert!(all.contains("Today"), "{all}");
    assert!(all.contains("details"), "{all}");
    assert!(!all.contains("No usage recorded"), "{all}");
    assert!(!all.contains("No data for the selected range"), "{all}");
}

#[test]
fn tui_usage_loading_state_ignores_shared_fixed_recent_logs() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Usage;
    app.focus = Focus::Content;
    app.usage
        .start_loading(AppType::Claude, UsageRangePreset::SevenDays);
    let mut data = minimal_data(&app.app_type);
    data.usage.recent_logs.push(UsageLogRow {
        request_id: "old-log".to_string(),
        created_at: 1_780_617_600,
        app_type: "claude".to_string(),
        provider_id: "p1".to_string(),
        model: "claude-sonnet-4".to_string(),
        status_code: 200,
        ..UsageLogRow::default()
    });
    data.usage.logs_total = 1;

    let all = all_text(&render_with_size(&app, &data, 160, 40));

    assert!(all.contains("Usage Statistics"), "{all}");
    assert!(all.contains("Loading..."), "{all}");
    assert!(!all.contains("No usage recorded"), "{all}");
}

#[test]
fn tui_usage_loading_state_keeps_existing_data_visible() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Usage;
    app.focus = Focus::Content;
    app.usage
        .start_loading(AppType::Claude, UsageRangePreset::SevenDays);
    let mut data = minimal_data(&app.app_type);
    data.usage.summary_7d = UsageSummarySnapshot {
        total_requests: 2,
        total_cost_usd: 0.42,
        total_tokens: 800,
        ..UsageSummarySnapshot::default()
    };
    data.usage.trends_7d = vec![UsageTrendBucket {
        key: "2026-06-05".to_string(),
        label: "06/05".to_string(),
        request_count: 2,
        total_tokens: 800,
        total_cost_usd: 0.42,
        error_count: 0,
    }];

    let all = all_text(&render_with_size(&app, &data, 160, 40));

    assert!(all.contains("$0.420"), "{all}");
    assert!(all.contains("800"), "{all}");
    assert!(all.contains("06/05"), "{all}");
    assert!(!all.contains("Loading..."), "{all}");
}

#[test]
fn tui_usage_zero_cost_metric_does_not_hide_real_usage() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Codex));
    app.route = Route::Usage;
    app.focus = Focus::Content;
    let mut data = minimal_data(&app.app_type);
    data.usage.summary_7d = UsageSummarySnapshot {
        total_requests: 2,
        success_count: 2,
        total_tokens: 800,
        ..UsageSummarySnapshot::default()
    };
    data.usage.trends_7d = vec![UsageTrendBucket {
        key: "2026-06-05".to_string(),
        label: "06/05".to_string(),
        request_count: 2,
        total_tokens: 800,
        ..UsageTrendBucket::default()
    }];

    let all = all_text(&render_with_size(&app, &data, 160, 40));

    assert!(all.contains("06/05"), "{all}");
    assert!(!all.contains("No usage recorded"), "{all}");
}

#[test]
fn tui_usage_marks_unreported_cache_writes_without_hiding_reported_values() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Codex));
    app.route = Route::Usage;
    app.focus = Focus::Content;
    let mut data = minimal_data(&app.app_type);
    data.usage.summary_7d = UsageSummarySnapshot {
        total_requests: 1,
        success_count: 1,
        total_tokens: 1_000,
        cache_read_tokens: 500,
        ..UsageSummarySnapshot::default()
    };
    data.usage.trends_7d = vec![UsageTrendBucket {
        key: "2026-06-05".to_string(),
        label: "06/05".to_string(),
        request_count: 1,
        total_tokens: 1_000,
        ..UsageTrendBucket::default()
    }];

    let unavailable = all_text(&render_with_size(&app, &data, 180, 42));
    assert!(unavailable.contains("Cache Write"), "{unavailable}");
    assert!(unavailable.contains("N/A"), "{unavailable}");

    data.usage.summary_7d.cache_creation_tokens = 1_250;
    let reported = all_text(&render_with_size(&app, &data, 180, 42));
    assert!(reported.contains("1.2k"), "{reported}");
    assert!(!reported.contains("N/A"), "{reported}");

    app.app_type = AppType::Claude;
    data.usage.summary_7d.cache_creation_tokens = 0;
    let reported_zero = all_text(&render_with_size(&app, &data, 180, 42));
    assert!(reported_zero.contains("Cache Write"), "{reported_zero}");
    assert!(!reported_zero.contains("N/A"), "{reported_zero}");
}

#[test]
fn tui_usage_renders_summary_and_trend() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Usage;
    app.focus = Focus::Content;
    let mut data = minimal_data(&app.app_type);
    data.usage = UsageSnapshot {
        summary_7d: UsageSummarySnapshot {
            total_requests: 4,
            success_count: 3,
            total_cost_usd: 1.25,
            total_tokens: 1_800,
            input_tokens: 1_000,
            output_tokens: 500,
            cache_read_tokens: 250,
            cache_creation_tokens: 50,
            avg_latency_ms: Some(420),
        },
        trends_7d: vec![UsageTrendBucket {
            key: "2026-06-05".to_string(),
            label: "06/05".to_string(),
            request_count: 4,
            total_tokens: 1_800,
            total_cost_usd: 1.25,
            error_count: 1,
        }],
        top_providers_7d: vec![UsageProviderStatsRow {
            provider_id: "p1".to_string(),
            provider_name: Some("Demo Provider".to_string()),
            request_count: 4,
            success_count: 3,
            total_tokens: 1_800,
            total_cost_usd: 1.25,
            avg_latency_ms: Some(420),
        }],
        recent_logs: vec![UsageLogRow {
            request_id: "req-1".to_string(),
            created_at: 1_780_617_600,
            app_type: "claude".to_string(),
            provider_id: "p1".to_string(),
            provider_name: Some("Demo Provider".to_string()),
            model: "claude-sonnet-4".to_string(),
            status_code: 200,
            input_tokens: 1_000,
            output_tokens: 500,
            total_cost_usd: 1.25,
            latency_ms: 420,
            ..UsageLogRow::default()
        }],
        logs_total: 1,
        ..UsageSnapshot::default()
    };

    let all = all_text(&render_with_size(&app, &data, 180, 42));

    assert!(all.contains("$1.250"), "{all}");
    assert!(all.contains("1.8k"), "{all}");
    assert!(all.contains("Real Tokens"), "{all}");
    assert!(all.contains("Input"), "{all}");
    assert!(all.contains("Output"), "{all}");
    assert!(all.contains("Cache Read"), "{all}");
    assert!(all.contains("Cache Write"), "{all}");
    assert!(all.contains("Errors"), "{all}");
    assert!(all.contains("Avg Latency"), "{all}");
    assert!(all.contains("Cache Tokens"), "{all}");
    assert!(all.contains("Cost / Req"), "{all}");
    assert!(all.contains("Cache Hit"), "{all}");
    assert!(all.contains("19%"), "{all}");
    assert!(all.contains("06/05"), "{all}");
    assert!(!all.contains("Latest"), "{all}");
    assert!(!all.contains("Peak"), "{all}");
    assert!(!all.contains("Total"), "{all}");
    assert!(!all.contains("Range"), "{all}");
    assert!(!all.contains("Demo Provider"), "{all}");
}

#[test]
fn tui_usage_overview_keeps_metric_values_near_labels() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Usage;
    app.focus = Focus::Content;
    let mut data = minimal_data(&app.app_type);
    data.usage.summary_7d = UsageSummarySnapshot {
        total_requests: 4,
        success_count: 3,
        total_cost_usd: 1.25,
        total_tokens: 1_800,
        input_tokens: 1_000,
        output_tokens: 500,
        cache_read_tokens: 250,
        cache_creation_tokens: 50,
        avg_latency_ms: Some(420),
    };

    let all = all_text(&render_with_size(&app, &data, 180, 42));
    let primary = line_with(&all, "Real Tokens");
    let secondary = line_with(&all, "Input");
    let tertiary = line_with(&all, "Errors");

    assert!(primary.contains("Real Tokens  1.8k"), "{all}");
    assert!(primary.contains("Requests  4"), "{all}");
    assert!(secondary.contains("Input  1.0k"), "{all}");
    assert!(secondary.contains("Output  500"), "{all}");
    assert!(tertiary.contains("Errors  1"), "{all}");
    assert!(tertiary.contains("Cost / Req  $0.312"), "{all}");

    let primary_y = line_index(&all, "Real Tokens");
    let secondary_y = line_index(&all, "Input");
    let tertiary_y = line_index(&all, "Errors");
    // The cache box now uses the shared plain border; locate its top
    // border as the first box corner after the metrics rows.
    let cache_border_y = tertiary_y
        + 1
        + all
            .lines()
            .skip(tertiary_y + 1)
            .position(|line| line.contains("┌"))
            .unwrap_or_else(|| panic!("missing cache border in:\n{all}"));
    assert_eq!(secondary_y - primary_y, 1, "{all}");
    assert_eq!(tertiary_y - secondary_y, 1, "{all}");
    assert_eq!(cache_border_y - tertiary_y, 1, "{all}");
}

#[test]
fn tui_usage_overview_short_height_keeps_even_spacing() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Usage;
    app.focus = Focus::Content;
    let mut data = minimal_data(&app.app_type);
    data.usage.summary_7d = UsageSummarySnapshot {
        total_requests: 4,
        success_count: 3,
        total_cost_usd: 1.25,
        total_tokens: 1_800,
        input_tokens: 1_000,
        output_tokens: 500,
        cache_read_tokens: 250,
        cache_creation_tokens: 50,
        avg_latency_ms: Some(420),
    };

    let all = all_text(&render_with_size(&app, &data, 180, 24));

    assert!(all.contains("Real Tokens  1.8k"), "{all}");
    assert!(all.contains("Input  1.0k"), "{all}");
    assert!(all.contains("Cost / Req  $0.312"), "{all}");

    let primary_y = line_index(&all, "Real Tokens");
    let secondary_y = line_index(&all, "Input");
    let tertiary_y = line_index(&all, "Errors");
    // The cache box now uses the shared plain border; locate its top
    // border as the first box corner after the metrics rows.
    let cache_border_y = tertiary_y
        + 1
        + all
            .lines()
            .skip(tertiary_y + 1)
            .position(|line| line.contains("┌"))
            .unwrap_or_else(|| panic!("missing cache border in:\n{all}"));
    assert_eq!(secondary_y - primary_y, 1, "{all}");
    assert_eq!(tertiary_y - secondary_y, 1, "{all}");
    assert_eq!(cache_border_y - tertiary_y, 1, "{all}");
}

#[test]
fn tui_usage_metric_spacing_keeps_narrow_rows_drawable() {
    assert_eq!(super::usage_metric_row_spacing(35), Some((1, 8)));
    assert_eq!(super::usage_metric_row_spacing(36), Some((1, 8)));
    assert_eq!(super::usage_metric_row_spacing(37), Some((1, 8)));
}

#[test]
fn tui_usage_compact_trend_omits_text_summary() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Usage;
    app.focus = Focus::Content;
    let mut data = minimal_data(&app.app_type);
    data.usage.trends_7d = vec![
        UsageTrendBucket {
            key: "2026-06-04".to_string(),
            label: "06/04".to_string(),
            request_count: 2,
            total_tokens: 800,
            total_cost_usd: 0.5,
            error_count: 0,
        },
        UsageTrendBucket {
            key: "2026-06-05".to_string(),
            label: "06/05".to_string(),
            request_count: 4,
            total_tokens: 1_800,
            total_cost_usd: 1.25,
            error_count: 1,
        },
    ];

    let all = all_text(&render_with_size(&app, &data, 80, 24));

    assert!(all.contains("Usage Trend"), "{all}");
    assert!(!all.contains("Latest"), "{all}");
    assert!(!all.contains("Peak"), "{all}");
    assert!(!all.contains("Total"), "{all}");
    assert!(!all.contains("Range"), "{all}");
    assert!(!all.contains("06/04 -> 06/05"), "{all}");
}

#[test]
fn tui_usage_trend_renders_midpoint_axis_labels() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Usage;
    app.focus = Focus::Content;
    let mut data = minimal_data(&app.app_type);
    data.usage.trends_7d = (1..=7)
        .map(|day| UsageTrendBucket {
            key: format!("2026-06-{day:02}"),
            label: format!("06/{day:02}"),
            request_count: day as u64,
            total_tokens: day as u64 * 100,
            total_cost_usd: day as f64 / 10.0,
            error_count: 0,
        })
        .collect();

    let all = all_text(&render_with_size(&app, &data, 180, 42));

    assert!(all.contains("06/01"), "{all}");
    assert!(all.contains("06/04"), "{all}");
    assert!(all.contains("06/07"), "{all}");
    assert!(all.contains("$0.500"), "{all}");
    assert!(all.contains("$1.000"), "{all}");
}

#[test]
fn tui_usage_tiny_height_omits_overview_title_without_content() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Usage;
    app.focus = Focus::Content;
    let mut data = minimal_data(&app.app_type);
    data.usage.summary_7d = UsageSummarySnapshot {
        total_requests: 4,
        success_count: 3,
        total_cost_usd: 1.25,
        total_tokens: 1_800,
        input_tokens: 1_000,
        output_tokens: 500,
        cache_read_tokens: 250,
        cache_creation_tokens: 50,
        avg_latency_ms: Some(420),
    };

    let all = all_text(&render_with_size(&app, &data, 120, 10));

    assert!(all.contains("Usage Statistics"), "{all}");
    assert!(all.contains("$1.250"), "{all}");
    assert!(!all.contains("Overview"), "{all}");
}

#[test]
fn tui_usage_details_tables_follow_selected_range() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::UsageLogs;
    app.focus = Focus::Content;
    app.usage.pane = UsagePane::Providers;

    let mut data = minimal_data(&app.app_type);
    data.usage = UsageSnapshot {
        top_providers_today: vec![UsageProviderStatsRow {
            provider_id: "today".to_string(),
            provider_name: Some("Today Provider".to_string()),
            request_count: 2,
            success_count: 2,
            total_tokens: 200,
            total_cost_usd: 0.2,
            avg_latency_ms: Some(120),
        }],
        top_providers_7d: vec![UsageProviderStatsRow {
            provider_id: "week".to_string(),
            provider_name: Some("Week Provider".to_string()),
            request_count: 8,
            success_count: 7,
            total_tokens: 800,
            total_cost_usd: 0.8,
            avg_latency_ms: Some(180),
        }],
        ..UsageSnapshot::default()
    };

    let week = all_text(&render_with_size(&app, &data, 160, 40));
    assert!(week.contains("Usage Statistics › Details"), "{week}");
    assert!(week.contains("Model Stats"), "{week}");
    assert!(week.contains("Provider Stats"), "{week}");
    assert!(week.contains("Request Logs"), "{week}");
    assert!(week.contains("Week Provider"), "{week}");
    assert!(!week.contains("Today Provider"), "{week}");

    app.usage.range = UsageRangePreset::Today;
    let today = all_text(&render_with_size(&app, &data, 160, 40));
    assert!(today.contains("Today Provider"), "{today}");
    assert!(!today.contains("Week Provider"), "{today}");
}

#[test]
fn tui_usage_narrow_width_renders_without_losing_primary_sections() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Usage;
    app.focus = Focus::Content;

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        80,
        28,
    ));

    assert!(all.contains("Usage Statistics"), "{all}");
    assert!(all.contains("Overview"), "{all}");
    assert!(all.contains("Usage Trend"), "{all}");
    assert!(!all.contains("Top Providers"), "{all}");
}

#[test]
fn tui_manual_usage_refresh_status_stays_visible_at_eighty_columns() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _lang = use_test_language(Language::English);
    let mut app = App::new(Some(AppType::Claude));
    app.focus = Focus::Content;
    app.usage
        .start_loading(AppType::Claude, UsageRangePreset::SevenDays);
    let mut data = minimal_data(&app.app_type);
    data.usage.summary_7d.total_requests = 42;
    data.pricing.rows.push(ModelPricingRow {
        model_id: "gpt-5.4".to_string(),
        display_name: "GPT 5.4".to_string(),
        recent_request_count: 1,
        ..ModelPricingRow::default()
    });

    app.route = Route::Usage;
    let usage = render_with_size(&app, &data, 80, 28);
    assert_refresh_status_is_visible_and_accented(&app, &usage);

    let wide_usage = render_with_size(&app, &data, 160, 28);
    let wide_text = all_text(&wide_usage);
    let summary_row = line_at(&wide_usage, line_index(&wide_text, "Refreshing") as u16);
    assert!(
        summary_row.contains("avg latency · ⠋ Refreshing"),
        "the unified refresh status should immediately follow the summary: {summary_row}"
    );

    app.route = Route::UsageLogs;
    let logs = render_with_size(&app, &data, 80, 28);
    assert_refresh_status_is_visible_and_accented(&app, &logs);

    app.route = Route::Pricing;
    let pricing = all_text(&render_with_size(&app, &data, 80, 28));
    assert!(pricing.contains("Refreshing"), "{pricing}");
}

fn assert_refresh_status_is_visible_and_accented(app: &App, buf: &Buffer) {
    let all = all_text(buf);
    let row = line_index(&all, "Refreshing") as u16;
    let refresh_x = cell_column_of(buf, row, "Refreshing")
        .unwrap_or_else(|| panic!("missing Refreshing in:\n{all}"));

    let theme = theme_for(&app.app_type);
    assert_eq!(
        buf[(refresh_x, row)].fg,
        theme.accent,
        "refresh status should use the theme accent"
    );
}

#[test]
fn tui_pricing_renders_catalog_and_recent_usage_context() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Pricing;
    app.focus = Focus::Content;
    let mut data = minimal_data(&app.app_type);
    data.pricing = ModelPricingSnapshot {
        rows: vec![
            ModelPricingRow {
                model_id: "gpt-5.4".to_string(),
                display_name: "GPT 5.4".to_string(),
                input_cost_per_million: "2".to_string(),
                output_cost_per_million: "8".to_string(),
                cache_read_cost_per_million: "0.125".to_string(),
                cache_creation_cost_per_million: "1".to_string(),
                recent_request_count: 12,
                recent_total_tokens: 1_500,
                recent_total_cost_usd: 0.42,
                last_used_at: Some(1_780_617_600),
            },
            ModelPricingRow {
                model_id: "claude-sonnet-4-5".to_string(),
                display_name: "Claude Sonnet 4.5".to_string(),
                input_cost_per_million: "3".to_string(),
                output_cost_per_million: "15".to_string(),
                cache_read_cost_per_million: "0.3".to_string(),
                cache_creation_cost_per_million: "3.75".to_string(),
                ..ModelPricingRow::default()
            },
        ],
        recent_unknown_models: 1,
        recent_unmatched_total_tokens: 500,
        recent_unmatched_total_cost_usd: 0.12,
        ..ModelPricingSnapshot::default()
    };

    let all = all_text(&render_with_size(&app, &data, 180, 36));

    assert!(all.contains("Model Pricing"), "{all}");
    assert!(all.contains("2 catalog"), "{all}");
    assert!(all.contains("1 used 30d"), "{all}");
    assert!(all.contains("1 unmatched models 30d"), "{all}");
    assert!(all.contains("2.0k tokens"), "{all}");
    assert!(all.contains("$0.540 total"), "{all}");
    assert!(all.contains("$0.120 unmatched"), "{all}");
    assert!(all.contains("gpt-5.4"), "{all}");
    assert!(all.contains("GPT 5.4"), "{all}");
    assert!(all.contains("$2.00"), "{all}");
    assert!(all.contains("$8.00"), "{all}");
    assert!(all.contains("$0.420"), "{all}");
    assert!(
        all.contains("Enter=edit") || all.contains("Enter edit"),
        "{all}"
    );
    assert!(!all.contains("Enter/e=edit"), "{all}");
    assert!(
        all.contains("d=delete") || all.contains("d delete"),
        "{all}"
    );
    assert!(
        all.contains("Esc=close") || all.contains("Esc close"),
        "{all}"
    );
    assert!(!all.contains("details"), "{all}");
}

#[test]
fn tui_pricing_loading_state_uses_usage_pricing_pending_signal() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Pricing;
    app.focus = Focus::Content;
    app.usage
        .start_loading(AppType::Claude, UsageRangePreset::SevenDays);
    let data = minimal_data(&app.app_type);

    let all = all_text(&render_with_size(&app, &data, 160, 36));

    assert!(all.contains("Model Pricing"), "{all}");
    assert!(all.contains("Loading..."), "{all}");
    assert!(!all.contains("No model pricing rows found"), "{all}");
}

#[test]
fn tui_pricing_loading_state_keeps_unmatched_context_visible() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Pricing;
    app.focus = Focus::Content;
    app.usage
        .start_loading(AppType::Claude, UsageRangePreset::SevenDays);
    let mut data = minimal_data(&app.app_type);
    data.pricing = ModelPricingSnapshot {
        recent_unknown_models: 1,
        recent_unmatched_total_tokens: 500,
        recent_unmatched_total_cost_usd: 0.12,
        ..ModelPricingSnapshot::default()
    };

    let all = all_text(&render_with_size(&app, &data, 160, 36));

    assert!(all.contains("Model Pricing"), "{all}");
    assert!(all.contains("1 unmatched models 30d"), "{all}");
    assert!(all.contains("500 tokens"), "{all}");
    assert!(all.contains("$0.120 total"), "{all}");
    assert!(all.contains("$0.120 unmatched"), "{all}");
    assert!(all.contains("No model pricing rows found"), "{all}");
    assert!(!all.contains("Loading..."), "{all}");
}

#[test]
fn tui_sessions_renders_split_detail_and_message_preview() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Sessions;
    app.focus = Focus::Content;
    app.sessions.loaded_once = true;
    app.sessions.rows.push(crate::session_manager::SessionMeta {
        provider_id: "claude".to_string(),
        session_id: "abcdef123456".to_string(),
        title: Some("Refactor proxy routing".to_string()),
        summary: Some("Tighten worker routing".to_string()),
        project_dir: Some("/tmp/demo-project".to_string()),
        created_at: Some(1_735_689_600_000),
        source_mtime_ns: None,
        last_active_at: Some(1_735_689_900_000),
        source_path: Some("/tmp/session.jsonl".to_string()),
        usage: Some(crate::session_manager::SessionUsageSummary {
            input_tokens: 4_800,
            output_tokens: 1_900_000,
            cache_read_tokens: 347_700_000,
            cache_creation_tokens: 12_200_000,
            estimated_cost_usd: Some(0.42),
        }),
        resume_command: Some("codex resume abcdef123456".to_string()),
    });
    app.sessions
        .open_detail(app::session_key(&app.sessions.rows[0]));
    app.sessions.pane = app::SessionsPane::Detail;
    app.sessions.messages_loaded = true;
    app.sessions
        .messages
        .push(crate::session_manager::SessionMessage {
            role: "user".to_string(),
            content: "Please review this module".to_string(),
            ts: Some(1_735_689_900_000),
        });

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        160,
        40,
    ));

    assert!(all.contains("Overview"), "{all}");
    assert!(all.contains("Time"), "{all}");
    assert!(all.contains("Work Dir"), "{all}");
    assert!(all.contains("Title"), "{all}");
    // Tokens and Cost each own a labelled Overview row, aligned with the rest
    // of the pane instead of sharing one bare line.
    let tokens_line = line_with(&all, "In: 4.8k");
    assert!(tokens_line.contains("Tokens"), "{tokens_line}");
    for value in ["Out: 1.9M", "CR: 347.7M"] {
        assert!(
            tokens_line.contains(value),
            "{value} missing in {tokens_line}"
        );
    }
    assert!(
        !tokens_line.contains("$0.42"),
        "the Cost value now lives on its own row: {tokens_line}"
    );
    let cost_line = all
        .lines()
        .find(|line| line.contains("Cost") && line.contains("$0.42"))
        .unwrap_or_else(|| panic!("missing labelled Overview Cost row in:\n{all}"));
    assert!(cost_line.contains("$0.42"), "{cost_line}");
    assert!(all.contains("Resume Command"), "{all}");
    assert!(all.contains("Refactor proxy routing"), "{all}");
    assert!(!all.contains("Tighten worker routing"), "{all}");
    assert!(all.contains("demo-project"), "{all}");
    assert!(all.contains("codex resume abcdef123456"), "{all}");
    assert!(all.contains("Please review"), "{all}");
}

#[test]
fn tui_sessions_transcript_footer_reports_the_complete_logical_history() {
    let _lang = use_test_language(Language::English);
    let temp = tempfile::tempdir().expect("transcript fixture directory");
    let source = temp.path().join("session.jsonl");
    let body = (0_usize..205)
        .map(|index| {
            serde_json::json!({
                "type": "response_item",
                "timestamp": index as i64,
                "payload": {
                    "type": "message",
                    "role": if index.is_multiple_of(2) { "user" } else { "assistant" },
                    "content": [{
                        "type": "input_text",
                        "text": format!("complete-message-{index}")
                    }]
                }
            })
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&source, body).expect("write transcript fixture");
    let source_text = source.to_string_lossy().into_owned();
    let (reader, _) = crate::session_manager::transcript::open_transcript_at(
        &temp.path().join("config"),
        "codex",
        &source_text,
    )
    .expect("open transcript fixture");
    let first = reader
        .load_page(0, &|| false)
        .expect("first transcript page");

    let mut app = App::new(Some(AppType::Codex));
    app.route = Route::Sessions;
    app.focus = Focus::Content;
    app.sessions.loaded_once = true;
    app.sessions.rows.push(crate::session_manager::SessionMeta {
        provider_id: "codex".to_string(),
        session_id: "complete-history".to_string(),
        title: Some("Complete history".to_string()),
        source_path: Some(source_text),
        ..crate::session_manager::SessionMeta::default()
    });
    let key = app::session_key(&app.sessions.rows[0]);
    app.sessions.open_detail(key.clone());
    app.sessions.pane = app::SessionsPane::Detail;
    let request_id = app.sessions.start_message_load(key.clone());
    assert!(app
        .sessions
        .finish_message_load(request_id, &key, reader, first));
    app.sessions.messages_truncated = true;
    assert_eq!(app.sessions.selected_message_absolute(), 0);

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        180,
        40,
    ));

    assert!(all.contains("┌ Messages ─"), "{all}");
    assert!(!all.contains("Messages ·"), "{all}");
    assert!(!all.contains("Full history"), "{all}");
    assert!(!all.contains("Body preview shortened"), "{all}");
    assert!(all.contains("Page 1 · 1–100 of 205"), "{all}");
    assert!(!all.contains("End of list · 205 total"), "{all}");
    assert!(all.contains("complete-message-0"), "{all}");
    assert!(!all.contains("complete-message-100"), "{all}");

    let previous_gate = app.sessions.message_pagination.clone();
    app.sessions.message_pagination.select(199);
    assert!(!app
        .sessions
        .begin_message_page_cross(1, previous_gate, None));
    let (page_request, generation, _refresh_page, _refresh_key, _reader) = app
        .sessions
        .next_message_page_request(1)
        .expect("message page request");
    assert!(app.sessions.fail_message_page_request(
        page_request,
        &key,
        &generation,
        1,
        "offline".to_string(),
    ));
    let failed = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        180,
        40,
    ));
    assert!(
        failed.contains("Load failed · Move again to retry"),
        "{failed}"
    );
    assert!(!failed.contains("Enter to retry"), "{failed}");
}

#[test]
fn tui_sessions_seamless_page_cross_uses_loading_footer() {
    use crate::cli::tui::input::{ScrollDirection, WheelGestureId};

    let _lang = use_test_language(Language::English);
    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Sessions;
    app.focus = Focus::Content;
    let _request_id = app.sessions.start_scan("claude".to_string());
    let scope_epoch = app.sessions.scope_epoch;
    let manifest_dir = tempfile::tempdir().expect("manifest fixture directory");
    let store =
        crate::session_manager::paged_manifest::PagedManifestStore::open_at(manifest_dir.path())
            .expect("manifest fixture store");
    let mut builder = store
        .begin_build("claude")
        .expect("manifest fixture builder");
    for index in 0..101 {
        builder
            .push(crate::session_manager::SessionMeta {
                provider_id: "claude".to_string(),
                session_id: format!("session-{index}"),
                title: Some(format!("Session {index}")),
                summary: None,
                project_dir: Some("/tmp/project".to_string()),
                created_at: Some(1_735_689_600_000 - index),
                source_mtime_ns: None,
                last_active_at: Some(1_735_689_900_000 - index),
                source_path: Some(format!("/tmp/session-{index}.jsonl")),
                usage: None,
                resume_command: Some(format!("claude --resume session-{index}")),
            })
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
    app.on_sessions_wheel(
        ScrollDirection::Down,
        10_000,
        WheelGestureId::from_raw(1),
        &minimal_data(&app.app_type),
    );

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        180,
        40,
    ));
    assert!(all.contains("Loading page 2"), "{all}");
    assert!(!all.contains("Enter to load"), "{all}");
    assert!(all.contains("Session 0"), "{all}");
    assert!(!all.contains("Session 100"), "{all}");
}

#[test]
fn tui_usage_log_footer_keeps_current_page_during_prefetch_then_shows_cta() {
    use crate::cli::tui::input::{ScrollDirection, WheelGestureId};

    let _lang = use_test_language(Language::English);
    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::UsageLogs;
    app.focus = Focus::Content;
    app.usage.pane = UsagePane::Recent;
    let mut data = minimal_data(&app.app_type);
    data.usage.logs_total = 205;
    data.usage.recent_logs = (0..100)
        .map(|index| UsageLogRow {
            request_id: format!("p0-{index}"),
            model: format!("model-{index}"),
            created_at: 1_000 - index,
            cursor_rowid: index as i64 + 1,
            ..UsageLogRow::default()
        })
        .collect();
    app.on_usage_logs_wheel(
        ScrollDirection::Down,
        10_000,
        WheelGestureId::from_raw(1),
        &data,
    );
    assert!(app.usage.log_pager.start_request(
        1,
        9,
        crate::cli::tui::data::UsageLogPageDirection::Older,
    ));

    let loading = all_text(&render_with_size(&app, &data, 180, 40));
    assert!(loading.contains("Loading page 2"), "{loading}");
    assert!(loading.contains("model-99"), "{loading}");

    let page_rows = (0..50)
        .map(|index| UsageLogRow {
            request_id: format!("p1-{index}"),
            model: format!("next-{index}"),
            created_at: 899 - index,
            ..UsageLogRow::default()
        })
        .collect::<Vec<_>>();
    let next_cursor = page_rows
        .last()
        .map(crate::cli::tui::data::UsageLogCursor::from_row);
    assert!(app.usage.log_pager.finish_request(
        1,
        9,
        crate::cli::tui::data::UsageLogPageDirection::Older,
        crate::cli::tui::data::UsageLogPage {
            rows: page_rows,
            next_cursor,
            has_more: false,
            ..crate::cli::tui::data::UsageLogPage::default()
        },
    ));

    let ready = all_text(&render_with_size(&app, &data, 180, 40));
    assert!(ready.contains("Scroll again"), "{ready}");
    assert!(ready.contains("next page"), "{ready}");
    assert!(ready.contains("model-99"), "{ready}");
    assert!(!ready.contains("next-0"), "{ready}");
    assert!(ready.contains("150 total rows"), "{ready}");
    assert!(!ready.contains("205 total rows"), "{ready}");
}

#[test]
fn tui_usage_background_prefetch_error_does_not_advertise_enter_retry_on_a_row() {
    let _lang = use_test_language(Language::English);
    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::UsageLogs;
    app.focus = Focus::Content;
    app.usage.pane = UsagePane::Recent;
    let mut data = minimal_data(&app.app_type);
    data.usage.logs_total = 200;
    data.usage.recent_logs = (0..100)
        .map(|index| UsageLogRow {
            request_id: format!("p0-{index}"),
            model: format!("model-{index}"),
            created_at: 1_000 - index,
            cursor_rowid: index + 1,
            ..UsageLogRow::default()
        })
        .collect();
    app.usage.sync_log_pager(
        &app.app_type,
        app.usage.range,
        &data.usage.recent_logs,
        data.usage.logs_total,
    );
    assert!(app.usage.log_pager.start_request(
        1,
        11,
        crate::cli::tui::data::UsageLogPageDirection::Older,
    ));
    assert!(app.usage.log_pager.fail_request(
        1,
        11,
        crate::cli::tui::data::UsageLogPageDirection::Older,
        "offline".to_string(),
    ));

    let rendered = all_text(&render_with_size(&app, &data, 180, 40));
    assert!(!rendered.contains("Enter to retry"), "{rendered}");
    assert!(rendered.contains("model-0"), "{rendered}");

    let action = app.on_usage_logs_key(
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ),
        &data,
    );
    assert!(matches!(
        action,
        Action::SwitchRoute(Route::UsageLogDetail { rowid }) if rowid == 1
    ));
}

#[test]
fn tui_usage_log_detail_marks_a_capped_error_message() {
    let _lang = use_test_language(Language::English);
    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::UsageLogDetail { rowid: 99 };
    app.focus = Focus::Content;
    app.usage.remember_log_detail(
        AppType::Claude,
        UsageRangePreset::SevenDays,
        UsageLogRow {
            request_id: "long-error".to_string(),
            cursor_rowid: 99,
            error_message: Some("upstream response failed".to_string()),
            error_message_truncated: true,
            ..UsageLogRow::default()
        },
    );

    let rendered = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        180,
        40,
    ));
    assert!(rendered.contains("upstream response failed"), "{rendered}");
    assert!(
        rendered.contains("display capped at 4096 characters"),
        "{rendered}",
    );
}

#[test]
fn tui_sessions_list_time_column_uses_relative_time_before_date() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Sessions;
    app.focus = Focus::Content;
    app.sessions.loaded_once = true;
    app.sessions.time_anchor_ms = 1_735_689_900_000;
    app.sessions.rows.push(crate::session_manager::SessionMeta {
        provider_id: "claude".to_string(),
        session_id: "abcdef123456".to_string(),
        title: Some("Recent session".to_string()),
        summary: None,
        project_dir: Some("/tmp/demo-project".to_string()),
        created_at: Some(1_735_689_600_000),
        source_mtime_ns: None,
        last_active_at: Some(1_735_689_600_000),
        source_path: Some("/tmp/recent.jsonl".to_string()),
        usage: None,
        resume_command: Some("claude --resume abcdef123456".to_string()),
    });
    app.sessions.rows.push(crate::session_manager::SessionMeta {
        provider_id: "claude".to_string(),
        session_id: "old-session".to_string(),
        title: Some("Old session".to_string()),
        summary: None,
        project_dir: Some("/tmp/demo-project".to_string()),
        created_at: Some(1_735_084_800_000),
        source_mtime_ns: None,
        last_active_at: Some(1_735_084_800_000),
        source_path: Some("/tmp/old.jsonl".to_string()),
        usage: None,
        resume_command: Some("claude --resume old-session".to_string()),
    });

    let content = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        160,
        40,
    ));
    let header_row = line_with(&content, "Title");
    assert!(
        header_row.contains("│ Title")
            && header_row.contains("Time")
            && header_row.contains("Cost │"),
        "session table should render Title, Time, and Cost columns:\n{content}"
    );
    let recent_row = line_with(&content, "Recent session");
    assert!(recent_row.contains("5 min ago"), "{recent_row}");
    assert!(
        recent_row.contains(" - │"),
        "a session without metrics should render an empty Cost marker: {recent_row}"
    );

    let old_row = line_with(&content, "Old session");
    let expected = chrono::DateTime::from_timestamp_millis(1_735_084_800_000)
        .expect("timestamp should be valid")
        .with_timezone(&chrono::Local);
    assert!(
        old_row.contains(&expected.format("%Y/%m/%d").to_string()),
        "{old_row}"
    );
    assert!(
        !old_row.contains(&expected.format("%Y/%m/%d %H:%M").to_string()),
        "{old_row}"
    );
}

#[test]
fn tui_sessions_cost_column_keeps_the_title_visible_at_eighty_columns() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Codex));
    app.route = Route::Sessions;
    app.focus = Focus::Content;
    app.sessions.loaded_once = true;
    app.sessions.rows.push(crate::session_manager::SessionMeta {
        provider_id: "codex".to_string(),
        session_id: "session-1".to_string(),
        title: Some("Fix routing".to_string()),
        usage: Some(crate::session_manager::SessionUsageSummary {
            input_tokens: 1_000,
            output_tokens: 500,
            estimated_cost_usd: Some(1.25),
            ..crate::session_manager::SessionUsageSummary::default()
        }),
        ..crate::session_manager::SessionMeta::default()
    });

    let rendered = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        80,
        24,
    ));
    // Title outranks every other column, so an 80-column pane spends its width
    // on the whole Title instead of squeezing it next to Cost.
    let session_row = line_with(&rendered, "Fix routing");
    assert!(
        session_row.contains("Fix routing"),
        "the 80-column session row should retain the complete Title: {session_row}"
    );
    let header_row = line_with(&rendered, "Title");
    let list_header = header_row
        .split('│')
        .find(|segment| segment.contains("Title"))
        .unwrap_or_else(|| panic!("missing session list header in:\n{rendered}"));
    assert!(
        list_header.contains("Time"),
        "Time outranks Cost when the list narrows: {header_row}"
    );
    assert!(
        !list_header.contains("Cost"),
        "Cost is the first column to yield: {header_row}"
    );
    // The exact amount stays one pane away, on its own Overview row.
    let overview_tokens = line_with(&rendered, "In:");
    assert!(
        overview_tokens.contains("In:"),
        "the Overview keeps a token breakdown even when narrow: {overview_tokens}"
    );
    let overview_cost = line_with(&rendered, "$1.25");
    assert!(
        overview_cost.contains("$1.25"),
        "the Overview keeps the exact cost: {overview_cost}"
    );
}

#[test]
fn tui_sessions_filters_rows_by_current_app() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Sessions;
    app.focus = Focus::Content;
    app.sessions.loaded_once = true;
    app.sessions.rows.push(crate::session_manager::SessionMeta {
        provider_id: "claude".to_string(),
        session_id: "claude-session".to_string(),
        title: Some("Claude visible".to_string()),
        summary: None,
        project_dir: Some("/tmp/claude-project".to_string()),
        created_at: Some(1_735_689_600_000),
        source_mtime_ns: None,
        last_active_at: Some(1_735_689_900_000),
        source_path: Some("/tmp/claude.jsonl".to_string()),
        usage: None,
        resume_command: Some("claude --resume claude-session".to_string()),
    });
    app.sessions.rows.push(crate::session_manager::SessionMeta {
        provider_id: "codex".to_string(),
        session_id: "codex-session".to_string(),
        title: Some("Codex hidden".to_string()),
        summary: None,
        project_dir: Some("/tmp/codex-project".to_string()),
        created_at: Some(1_735_689_600_000),
        source_mtime_ns: None,
        last_active_at: Some(1_735_689_900_000),
        source_path: Some("/tmp/codex.jsonl".to_string()),
        usage: None,
        resume_command: Some("codex resume codex-session".to_string()),
    });

    let claude = all_text(&render(&app, &minimal_data(&app.app_type)));
    assert!(claude.contains("Claude visible"), "{claude}");
    assert!(!claude.contains("Codex hidden"), "{claude}");

    app.app_type = AppType::Codex;
    let codex = all_text(&render(&app, &minimal_data(&app.app_type)));
    assert!(!codex.contains("Claude visible"), "{codex}");
    assert!(codex.contains("Codex hidden"), "{codex}");
}

#[test]
fn tui_sessions_slash_search_filters_user_role_messages() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Sessions;
    app.focus = Focus::Content;
    app.sessions.loaded_once = true;
    app.sessions.rows.push(crate::session_manager::SessionMeta {
        provider_id: "claude".to_string(),
        session_id: "abcdef123456".to_string(),
        title: Some("Debug deploy".to_string()),
        summary: None,
        project_dir: Some("/tmp/demo-project".to_string()),
        created_at: Some(1_735_689_600_000),
        source_mtime_ns: None,
        last_active_at: Some(1_735_689_900_000),
        source_path: Some("/tmp/session.jsonl".to_string()),
        usage: None,
        resume_command: Some("claude --resume abcdef123456".to_string()),
    });
    app.sessions
        .open_detail(app::session_key(&app.sessions.rows[0]));
    app.sessions.pane = app::SessionsPane::Detail;
    app.sessions.messages_loaded = true;
    app.sessions.messages = vec![
        crate::session_manager::SessionMessage {
            role: "user".to_string(),
            content: "How do I deploy this service?".to_string(),
            ts: Some(1_735_689_900_000),
        },
        crate::session_manager::SessionMessage {
            role: "assistant".to_string(),
            content: "The user should use the release workflow.".to_string(),
            ts: Some(1_735_689_901_000),
        },
        crate::session_manager::SessionMessage {
            role: "tool".to_string(),
            content: "cargo test".to_string(),
            ts: Some(1_735_689_902_000),
        },
    ];

    let data = minimal_data(&app.app_type);
    let action = app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE), &data);
    assert!(matches!(action, Action::None));
    for ch in "User".chars() {
        let action = app.on_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE), &data);
        assert!(matches!(action, Action::None));
    }

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        160,
        40,
    ));
    assert!(!all.contains("Search: User"), "{all}");
    assert!(all.contains("How do I deploy"), "{all}");
    assert!(!all.contains("release workflow"), "{all}");
    assert!(!all.contains("cargo test"), "{all}");
}

#[test]
fn tui_sessions_search_filters_loaded_message_content() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Sessions;
    app.focus = Focus::Content;
    app.sessions.loaded_once = true;
    app.sessions.rows.push(crate::session_manager::SessionMeta {
        provider_id: "claude".to_string(),
        session_id: "abcdef123456".to_string(),
        title: Some("Debug deploy".to_string()),
        summary: None,
        project_dir: Some("/tmp/demo-project".to_string()),
        created_at: Some(1_735_689_600_000),
        source_mtime_ns: None,
        last_active_at: Some(1_735_689_900_000),
        source_path: Some("/tmp/session.jsonl".to_string()),
        usage: None,
        resume_command: Some("claude --resume abcdef123456".to_string()),
    });
    app.sessions
        .open_detail(app::session_key(&app.sessions.rows[0]));
    app.sessions.messages_loaded = true;
    app.sessions.messages = vec![
        crate::session_manager::SessionMessage {
            role: "user".to_string(),
            content: "How do I deploy this service?".to_string(),
            ts: Some(1_735_689_900_000),
        },
        crate::session_manager::SessionMessage {
            role: "assistant".to_string(),
            content: "Use the release workflow.".to_string(),
            ts: Some(1_735_689_901_000),
        },
    ];
    app.sessions.message_filter.set("deploy");

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        160,
        40,
    ));
    assert!(all.contains("How do I deploy"), "{all}");
    assert!(!all.contains("Use the release workflow"), "{all}");
}

#[test]
fn tui_sessions_search_matches_localized_user_role_label() {
    let _lang = use_test_language(Language::Chinese);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Sessions;
    app.focus = Focus::Content;
    app.sessions.loaded_once = true;
    app.sessions.rows.push(crate::session_manager::SessionMeta {
        provider_id: "claude".to_string(),
        session_id: "abcdef123456".to_string(),
        title: Some("Debug deploy".to_string()),
        summary: None,
        project_dir: Some("/tmp/demo-project".to_string()),
        created_at: Some(1_735_689_600_000),
        source_mtime_ns: None,
        last_active_at: Some(1_735_689_900_000),
        source_path: Some("/tmp/session.jsonl".to_string()),
        usage: None,
        resume_command: Some("claude --resume abcdef123456".to_string()),
    });
    app.sessions
        .open_detail(app::session_key(&app.sessions.rows[0]));
    app.sessions.messages_loaded = true;
    app.sessions.messages = vec![
        crate::session_manager::SessionMessage {
            role: "user".to_string(),
            content: "How do I deploy this service?".to_string(),
            ts: Some(1_735_689_900_000),
        },
        crate::session_manager::SessionMessage {
            role: "assistant".to_string(),
            content: "提醒用户使用发布流程。".to_string(),
            ts: Some(1_735_689_901_000),
        },
    ];
    app.sessions.message_filter.set("用户");

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        160,
        40,
    ));
    assert!(all.contains("How do I deploy"), "{all}");
    assert!(!all.contains("发布流程"), "{all}");
}

#[test]
fn tui_sessions_session_and_message_filters_are_independent() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Sessions;
    app.focus = Focus::Content;
    app.sessions.loaded_once = true;
    app.sessions.rows.push(crate::session_manager::SessionMeta {
        provider_id: "claude".to_string(),
        session_id: "ai-session".to_string(),
        title: Some("AI incident".to_string()),
        summary: None,
        project_dir: Some("/tmp/ai-project".to_string()),
        created_at: Some(1_735_689_600_000),
        source_mtime_ns: None,
        last_active_at: Some(1_735_689_900_000),
        source_path: Some("/tmp/ai.jsonl".to_string()),
        usage: None,
        resume_command: Some("claude --resume ai-session".to_string()),
    });
    app.sessions.rows.push(crate::session_manager::SessionMeta {
        provider_id: "claude".to_string(),
        session_id: "billing-session".to_string(),
        title: Some("Billing question".to_string()),
        summary: None,
        project_dir: Some("/tmp/billing-project".to_string()),
        created_at: Some(1_735_689_600_000),
        source_mtime_ns: None,
        last_active_at: Some(1_735_689_800_000),
        source_path: Some("/tmp/billing.jsonl".to_string()),
        usage: None,
        resume_command: Some("claude --resume billing-session".to_string()),
    });
    app.sessions
        .open_detail(app::session_key(&app.sessions.rows[0]));
    app.sessions.pane = app::SessionsPane::Detail;
    app.sessions.messages_loaded = true;
    app.sessions.messages = vec![
        crate::session_manager::SessionMessage {
            role: "user".to_string(),
            content: "How do I inspect the deployment?".to_string(),
            ts: Some(1_735_689_900_000),
        },
        crate::session_manager::SessionMessage {
            role: "assistant".to_string(),
            content: "AI response for the user.".to_string(),
            ts: Some(1_735_689_901_000),
        },
    ];
    app.filter.input.set("AI");
    app.sessions.message_filter.set("User");

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        160,
        40,
    ));
    assert!(all.contains("AI incident"), "{all}");
    assert!(!all.contains("Billing question"), "{all}");
    assert!(all.contains("How do I inspect"), "{all}");
    assert!(!all.contains("AI response"), "{all}");
}

#[test]
fn openclaw_tui_form_shows_api_key_in_default_view() {
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

    assert!(all.contains("sk-openclaw-secret"), "{all}");
    assert!(!all.contains("[redacted]"), "{all}");
}

#[test]
fn plaintext_json_preview_preserves_values_regardless_of_field_names() {
    let value = json!({
        "env": {
            "AWS_SECRET_ACCESS_KEY": "aws-secret-value",
            "AWS_ACCESS_KEY_ID": "AKIA1234567890",
            "GITHUB_PAT": "ghp-visible-secret",
            "DATABASE_URL": "postgres://user:password@example.test/db"
        },
        "models": [
            {
                "maxTokens": 8192,
                "max_output_tokens": 4096,
                "apiKey": "sk-openclaw-secret"
            }
        ],
        "tokenLimit": 12345,
        "auths": { "name": "auths-container-secret" },
        "oauths": { "maxTokens": 7777 },
        "pats": { "tokenLimit": 8888 },
        "unknown": {
            "name": "unknown-parent-secret",
            "maxTokens": 9999
        },
        "password": "hidden",
        "cookies": { "session": "cookie-container-secret" },
        "credentials": { "value": "credential-container-secret" },
        "passwords": ["password-container-secret"],
        "api_keys": { "primary": "api-key-container-secret" },
        "auth_tokens": { "primary": "auth-token-container-secret" },
        "headers": {
            "X-Custom-Auth": "custom-header-secret",
            "User-Agent": "safe-but-hidden"
        },
        "env_http_headers": {
            "Authorization": "AUTH_HEADER_ENV",
            "nested": {
                "env": {
                    "GITHUB_PAT": "nested-env-header-secret"
                }
            }
        },
        "evil_env_http_headers": {
            "Authorization": "Bearer disguised-header-secret"
        },
        "PRIVATE_KEY": "private-key-value",
        "SECRET_FOO": "secret-prefix-value",
        "API_SECRET_VALUE": "secret-middle-value"
    });

    let preview = super::bounded_json_preview(&value);

    assert_eq!(preview, value);
    assert_eq!(
        preview["env"]["AWS_SECRET_ACCESS_KEY"],
        json!("aws-secret-value")
    );
    assert_eq!(
        preview["headers"]["X-Custom-Auth"],
        json!("custom-header-secret")
    );
}

#[test]
fn plaintext_json_preview_preserves_nonstandard_token_metric_types() {
    let value = json!({
        "models": [{
            "maxTokens": "8192-secret",
            "minTokens": -1,
            "inputTokens": 1.5,
            "outputTokens": true,
            "promptTokens": { "value": 2 },
            "completionTokens": [3]
        }],
        "tokenLimit": "12345-secret"
    });

    let preview = super::bounded_json_preview(&value);

    assert_eq!(preview, value);
}

#[test]
fn plaintext_json_preview_bounds_all_large_collections() {
    let models = (0..10_000)
        .map(|index| {
            json!({
                "id": format!("model-{index}"),
                "apiKey": format!("secret-{index}")
            })
        })
        .collect::<Vec<_>>();
    let value = json!({
        "args": (0..10_000).map(|index| format!("arg-secret-{index}")).collect::<Vec<_>>(),
        "models": models,
    });

    let preview = super::bounded_json_preview(&value);
    let rendered_args = preview["args"]
        .as_array()
        .expect("args should retain a bounded preview array");
    let rendered_models = preview["models"]
        .as_array()
        .expect("models should retain a bounded preview array");

    assert!(rendered_args.len() <= 129);
    assert!(rendered_models.len() <= 129);
    for collection in [rendered_args, rendered_models] {
        assert!(collection
            .last()
            .and_then(|value| value.as_str())
            .is_some_and(|value| value.contains("preview truncated")));
    }
    let text = serde_json::to_string(&preview).expect("bounded preview should serialize");
    assert!(text.contains("arg-secret-0"), "{text}");
    assert!(text.contains("secret-0"), "{text}");
    assert!(!text.contains("arg-secret-9999"));
    assert!(!text.contains("secret-9999"));
    assert!(!text.contains("[redacted]"), "{text}");
}

#[test]
fn plaintext_json_preview_charges_depth_cutoff_nodes_to_the_global_budget() {
    fn node_count(value: &Value) -> usize {
        1 + match value {
            Value::Array(items) => items.iter().map(node_count).sum::<usize>(),
            Value::Object(map) => map.values().map(node_count).sum::<usize>(),
            _ => 0,
        }
    }

    let leaf = Value::Object(
        (0..128)
            .map(|index| (format!("key-{index}"), json!(format!("value-{index}"))))
            .collect(),
    );
    let mut value = Value::Array((0..128).map(|_| leaf.clone()).collect());
    for _ in 0..14 {
        value = Value::Array(vec![value]);
    }

    let preview = super::bounded_json_preview(&value);

    assert!(
        node_count(&preview) <= 2_100,
        "depth placeholders escaped the global node budget"
    );
    assert!(serde_json::to_string(&preview)
        .expect("preview should serialize")
        .contains("preview truncated"));
}

#[test]
fn codex_auth_preview_preserves_every_leaf_in_plaintext() {
    let auth = json!({
        "id": "public-looking-secret",
        "name": "another-secret",
        "maxTokens": 8192,
        "nested": { "enabled": true }
    });

    let preview = super::bounded_json_preview(&auth);

    assert_eq!(preview, auth);
}

#[test]
fn provider_json_preview_renders_all_values_in_plaintext() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let secret = "ghp-live-preview-secret";
    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.focus = FormFocus::JsonPreview;
    form.extra = json!({
        "settingsConfig": {
            "env": {
                "GITHUB_PAT": secret,
                "NON_STANDARD_CREDENTIAL_NAME": "another-secret"
            },
            "cookies": {
                "session": "provider-cookie-secret"
            },
            "credentials": {
                "value": "provider-credential-secret"
            }
        }
    });
    app.form = Some(FormState::ProviderAdd(form));

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        160,
        40,
    ));
    assert!(all.contains(secret), "{all}");
    assert!(all.contains("another-secret"), "{all}");
    assert!(all.contains("provider-cookie-secret"), "{all}");
    assert!(all.contains("provider-credential-secret"), "{all}");
    assert!(!all.contains("[redacted]"), "{all}");
}

#[test]
fn provider_json_preview_omits_million_byte_scalar_while_field_stays_bounded_plaintext() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.focus = FormFocus::JsonPreview;
    form.claude_api_key.set("S".repeat(1_000_000));
    app.form = Some(FormState::ProviderAdd(form));

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        160,
        40,
    ));
    assert!(
        all.contains(texts::tui_preview_omitted_too_large()),
        "{all}"
    );
    assert!(all.contains("SSSSSSSS"), "{all}");
    assert!(!all.contains("[redacted]"), "{all}");
    assert!(all.len() < 20_000, "provider frame was not bounded");
}

#[test]
fn provider_passive_fields_bound_million_byte_classifiers_and_validation() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    for app_type in [AppType::Claude, AppType::Codex, AppType::Hermes] {
        let mut app = App::new(Some(app_type.clone()));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        let mut form = crate::cli::tui::form::ProviderAddFormState::new(app_type);
        form.focus = FormFocus::Fields;
        form.name.set(" ".repeat(1_000_000));
        form.website_url.set("W".repeat(1_000_000));
        form.claude_base_url.set("C".repeat(1_000_000));
        form.custom_user_agent.set("U".repeat(1_000_000));
        form.hermes_api_mode = "H".repeat(1_000_000);
        app.form = Some(FormState::ProviderAdd(form));

        let all = all_text(&render_with_size(
            &app,
            &minimal_data(&app.app_type),
            160,
            40,
        ));
        assert!(all.len() < 20_000, "passive frame was not bounded");
    }

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.custom_user_agent.set("U".repeat(1_000_000));
    form.open_local_proxy_settings_page();
    app.form = Some(FormState::ProviderAdd(form));
    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        160,
        40,
    ));
    assert!(all.len() < 20_000, "validation frame was not bounded");
}

#[test]
fn codex_model_catalog_renders_only_the_selected_large_collection_window() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let selected = 9_999usize;
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Codex);
    form.codex_model_catalog = (0..10_000)
        .map(|index| crate::cli::tui::form::CodexModelCatalogRow {
            model: format!("MODEL-{index:05}"),
            display_name: format!("Display {index:05}"),
            context_window: "128k".to_string(),
            supports_parallel_tool_calls: None,
            input_modalities: Vec::new(),
            base_instructions: String::new(),
        })
        .collect();
    form.codex_model_catalog[selected].model = format!("SELECTED-CODEX-{}", "X".repeat(1_000_000));
    form.codex_model_catalog_idx = selected;
    form.open_codex_model_catalog_page();

    let mut app = App::new(Some(AppType::Codex));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.form = Some(FormState::ProviderAdd(form));

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        160,
        40,
    ));
    assert!(all.contains("SELECTED-CODEX"), "{all}");
    assert!(!all.contains("MODEL-00000"), "{all}");
}

#[test]
fn provider_json_preview_omits_oversized_nested_source_and_common_snippet() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    for (extra, common_snippet, include_common_config) in [
        (
            json!({
                "settingsConfig": {
                    "env": { "TOKEN": "N".repeat(1_000_000) }
                }
            }),
            String::new(),
            true,
        ),
        (json!({}), " ".repeat(1_000_000), true),
        // The save projection calls `trim()` before it checks this toggle, so
        // an inactive snippet must still be rejected by the render preflight.
        (json!({}), " ".repeat(1_000_000), false),
    ] {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
        form.focus = FormFocus::JsonPreview;
        form.include_common_config = include_common_config;
        form.extra = extra;
        app.form = Some(FormState::ProviderAdd(form));

        let mut data = minimal_data(&app.app_type);
        data.config.common_snippet = common_snippet;
        let all = all_text(&render_with_size(&app, &data, 160, 40));
        assert!(
            all.contains(texts::tui_preview_omitted_too_large()),
            "{all}"
        );
        assert!(!all.contains("NNNNNNNN"), "{all}");
    }
}

#[test]
fn codex_toml_preview_keeps_all_literal_values_in_plaintext() {
    let source = r#"experimental_bearer_token = "top-level-secret"
max_tokens = 8192
token_limit = 12345

[model_providers.relay]
experimental_bearer_token = "provider-secret"

[model_providers.relay.http_headers]
Authorization = "Bearer header-secret"
X-Custom-Auth = "custom-header-secret"
User-Agent = "cc-switch-test"

[mcp_servers.demo.env]
PRIVATE_KEY = "private-env-secret"
SECRET_FOO = "other-env-secret"

[mcp_servers.demo.env_http_headers]
Authorization = "AUTH_HEADER_ENV"

[mcp_servers.demo.env_http_headers.nested.env]
GITHUB_PAT = "nested-env-header-secret"

[evil_env_http_headers]
Authorization = "Bearer disguised-header-secret"

[cookies]
session = "cookie-container-secret"

[credentials]
value = "credential-container-secret"

[passwords]
primary = "password-container-secret"

[api_keys]
primary = "api-key-container-secret"

[auth_tokens]
primary = "auth-token-container-secret"
"#;

    let preview = super::bounded_toml_preview(source);

    for secret in [
        "top-level-secret",
        "provider-secret",
        "header-secret",
        "custom-header-secret",
        "private-env-secret",
        "other-env-secret",
        "AUTH_HEADER_ENV",
        "nested-env-header-secret",
        "disguised-header-secret",
        "cookie-container-secret",
        "credential-container-secret",
        "password-container-secret",
        "api-key-container-secret",
        "auth-token-container-secret",
    ] {
        assert!(preview.contains(secret), "{preview}");
    }
    assert!(!preview.contains("[redacted]"), "{preview}");
    assert!(preview.contains("max_tokens = 8192"), "{preview}");
    assert!(preview.contains("token_limit = 12345"), "{preview}");
    assert!(preview.contains("cc-switch-test"), "{preview}");
    assert!(preview.contains("env_http_headers"), "{preview}");
}

#[test]
fn codex_toml_preview_keeps_invalid_source_visible_for_correction() {
    let source = "experimental_bearer_token = \"unterminated-secret";
    let preview = super::bounded_toml_preview(source);

    assert_eq!(preview, source);
    assert!(!preview.contains("[redacted]"));
}

#[test]
fn codex_toml_preview_omits_oversized_input_before_parsing() {
    let source = " ".repeat(super::TOML_PREVIEW_MAX_INPUT_BYTES + 1);
    let preview = super::bounded_toml_preview(&source);

    assert_eq!(preview, texts::tui_preview_omitted_too_large());
}

#[test]
fn codex_toml_preview_bounds_known_collections_and_depth() {
    let models = (0..500)
        .map(|index| format!("{{ name = \"model-{index}\", apiKey = \"secret-{index}\" }}"))
        .collect::<Vec<_>>()
        .join(", ");
    let preview = super::bounded_toml_preview(&format!("models = [{models}]"));

    assert!(preview.contains("preview truncated"), "{preview}");
    assert!(preview.contains("secret-0"), "{preview}");
    assert!(!preview.contains("secret-499"), "{preview}");
    assert!(!preview.contains("[redacted]"), "{preview}");
    assert!(preview.len() <= super::TOML_PREVIEW_MAX_INPUT_BYTES);

    let deep_key = std::iter::repeat_n("models", 20)
        .collect::<Vec<_>>()
        .join(".");
    let deep = super::bounded_toml_preview(&format!("{deep_key} = {{ name = \"deep-secret\" }}"));
    assert!(deep.contains("preview truncated"), "{deep}");
    assert!(!deep.contains("deep-secret"), "{deep}");
    assert!(!deep.contains("[redacted]"), "{deep}");
}

#[test]
fn compact_codex_preview_builds_only_the_selected_bounded_section() {
    let hidden_config_secret = "hidden-config-secret";
    let hidden_auth_secret = "hidden-auth-secret";
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Codex);
    form.id.set("relay");
    form.name.set("Relay");
    form.codex_api_key.set("current-api-key-secret");
    form.extra = json!({
        "settingsConfig": {
            "auth": { "CUSTOM_AUTH": hidden_auth_secret },
            "config": format!(
                "token = \"{hidden_config_secret}\"\n# {}",
                "x".repeat(super::TOML_PREVIEW_MAX_INPUT_BYTES)
            )
        }
    });

    let auth = super::codex_auth_preview_text(&form);
    assert!(auth.contains("CUSTOM_AUTH"), "{auth}");
    assert!(auth.contains("OPENAI_API_KEY"), "{auth}");
    assert!(auth.contains(hidden_auth_secret), "{auth}");
    assert!(auth.contains("current-api-key-secret"), "{auth}");
    assert!(!auth.contains(hidden_config_secret), "{auth}");
    assert!(!auth.contains("[redacted]"), "{auth}");
    assert!(
        !auth.contains(texts::tui_preview_omitted_too_large()),
        "{auth}"
    );

    let config = super::codex_config_preview_text(&form, "");
    assert_eq!(config, texts::tui_preview_omitted_too_large());
    assert!(!config.contains(hidden_config_secret));
}

#[test]
fn codex_toml_preview_includes_enabled_common_config() {
    let common_snippet = "[features]\ngoals = true\n";
    let mut form = crate::cli::tui::form::ProviderAddFormState::new_with_common_snippet(
        AppType::Codex,
        common_snippet,
    );
    form.extra = json!({
        "settingsConfig": {
            "config": "model = \"gpt-5.4\"\n"
        }
    });

    let config = super::codex_config_preview_text(&form, common_snippet);

    assert!(config.contains("[features]"), "{config}");
    assert!(config.contains("goals = true"), "{config}");
}

#[test]
fn codex_toml_preview_preserves_values_regardless_of_metric_types_or_containers() {
    let source = r#"max_tokens = "8192-secret"
min_tokens = -1
input_tokens = 1.5
output_tokens = true
prompt_tokens = [3]
token_limit = "12345-secret"
max_output_tokens = 4096

[unknown]
max_tokens = 9999

[auths]
name = "auths-container-secret"

[oauths]
max_tokens = 7777

[pats]
token_limit = 8888
"#;

    let preview = super::bounded_toml_preview(source);

    assert!(preview.contains("max_output_tokens = 4096"), "{preview}");
    for visible in [
        "8192-secret",
        "12345-secret",
        "auths-container-secret",
        "max_tokens = 9999",
        "max_tokens = 7777",
        "token_limit = 8888",
    ] {
        assert!(preview.contains(visible), "{preview}");
    }
    assert!(!preview.contains("[redacted]"), "{preview}");
}

#[test]
fn bounded_display_helper_keeps_urls_and_commands_in_plaintext() {
    let url = "https://user:pass@[2001:db8::1]:8443/path/token?secret=value#fragment";
    assert_eq!(super::bounded_trimmed_text_for_display(url), url);
    assert_eq!(
        super::bounded_trimmed_text_for_display("file:///tmp/secret"),
        "file:///tmp/secret"
    );
    assert_eq!(
        super::bounded_trimmed_text_for_display("/usr/local/bin/node"),
        "/usr/local/bin/node"
    );
    assert_eq!(
        super::bounded_trimmed_text_for_display(r"C:\tools\node.exe"),
        r"C:\tools\node.exe"
    );
    assert_eq!(
        super::bounded_trimmed_text_for_display("API_KEY=secret node"),
        "API_KEY=secret node"
    );
    assert_eq!(
        super::bounded_trimmed_text_for_display("sh -c secret"),
        "sh -c secret"
    );
}

#[test]
fn codex_provider_form_renders_toml_values_in_plaintext() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let secret = "sk-live-preview-secret";
    let plural_container_secret = "codex-cookie-container-secret";
    let mut app = App::new(Some(AppType::Codex));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Codex);
    form.focus = FormFocus::JsonPreview;
    form.codex_preview_section = crate::cli::tui::form::CodexPreviewSection::Config;
    form.extra = json!({
        "settingsConfig": {
            "auth": {},
            "config": format!(
                "experimental_bearer_token = \"{secret}\"\nmodel = \"gpt-5.4\"\n\n[cookies]\nsession = \"{plural_container_secret}\"\n"
            )
        }
    });
    form.codex_model.set("gpt-5.4");
    app.form = Some(FormState::ProviderAdd(form));

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        160,
        40,
    ));
    assert!(all.contains(secret), "{all}");
    assert!(all.contains(plural_container_secret), "{all}");
    assert!(!all.contains("[redacted]"), "{all}");
    assert!(all.contains("gpt-5.4"), "{all}");
}

#[test]
fn usage_query_script_validation_error_is_persistent_beside_preview() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let error = "Script must contain return";
    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.open_usage_query_page();
    form.usage_query_enabled = true;
    form.focus = FormFocus::JsonPreview;
    form.set_usage_query_field_error(crate::cli::tui::form::UsageQueryField::Script, error);
    app.form = Some(FormState::ProviderAdd(form));

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        80,
        35,
    ));
    assert!(all.contains(&format!("! {error}")), "{all}");
}

#[test]
fn usage_query_custom_script_preview_shows_literals_in_plaintext() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let secret = "hardcoded-script-secret";
    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.open_usage_query_page();
    form.usage_query_enabled = true;
    form.usage_query_template = crate::cli::tui::form::UsageQueryTemplate::Custom;
    form.usage_query_api_key.set("");
    form.usage_query_access_token.set("");
    form.usage_query_code = format!(
        "fetch(url, {{ headers: {{ Authorization: \"Bearer {secret}\" }} }}); return {{}};"
    );
    form.focus = FormFocus::JsonPreview;
    app.form = Some(FormState::ProviderAdd(form));

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        160,
        40,
    ));
    assert!(all.contains(secret), "{all}");
    assert!(!all.contains("[redacted]"), "{all}");
}

#[test]
fn usage_query_preview_omits_a_million_byte_script_without_unbounded_rendering() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.open_usage_query_page();
    form.usage_query_enabled = true;
    form.usage_query_template = crate::cli::tui::form::UsageQueryTemplate::Custom;
    form.usage_query_code = " ".repeat(1_000_000);
    form.focus = FormFocus::JsonPreview;
    app.form = Some(FormState::ProviderAdd(form));

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        160,
        40,
    ));
    assert!(
        all.contains(texts::tui_preview_omitted_too_large()),
        "{all}"
    );
    assert!(!all.contains("[redacted]"), "{all}");
    assert!(all.len() < 20_000, "script preview frame was not bounded");
}

#[test]
fn usage_query_exact_builtin_script_remains_previewable() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.open_usage_query_page();
    form.usage_query_enabled = true;
    form.usage_query_template = crate::cli::tui::form::UsageQueryTemplate::General;
    form.usage_query_code =
        crate::cli::tui::form::ProviderAddFormState::USAGE_QUERY_GENERAL_PRESET.to_string();
    form.focus = FormFocus::JsonPreview;
    app.form = Some(FormState::ProviderAdd(form));

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        160,
        40,
    ));
    assert!(all.contains("response.balance"), "{all}");
}

#[test]
fn prompt_long_wrapped_line_keeps_tail_and_cursor_visible() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let content = format!("{}TAIL", "x".repeat(500));
    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Prompts;
    app.focus = Focus::Content;
    let mut form = PromptMetaFormState::new("long-line".to_string(), "Long line".to_string());
    form.focus = FormFocus::Content;
    form.content.replace_text(&content);
    form.content.cursor_col = content.chars().count();
    app.form = Some(FormState::PromptMeta(form));

    let data = minimal_data(&app.app_type);
    let backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).expect("terminal created");
    terminal
        .draw(|frame| super::render(frame, &app, &data))
        .expect("draw ok");
    let cursor = terminal.get_cursor_position().expect("cursor position");
    let buf = terminal.backend().buffer();

    assert!(all_text(buf).contains("TAIL"), "{}", all_text(buf));
    assert!(
        line_at(buf, cursor.y).contains("TAIL"),
        "{}",
        line_at(buf, cursor.y)
    );
}

#[test]
fn prompt_passive_render_jumps_a_stale_origin_near_a_distant_cursor() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let line_count = 100_000usize;
    let content = (0..line_count)
        .map(|index| {
            if index + 1 == line_count {
                "DISTANT-CURSOR-TARGET".to_string()
            } else {
                format!("line-{index:05}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut form = PromptMetaFormState::new("large-prompt".to_string(), "Large".to_string());
    form.focus = FormFocus::Content;
    form.content.replace_text(content);
    form.content.cursor_row = line_count - 1;
    form.content.cursor_col = "DISTANT-CURSOR-TARGET".len();
    form.content.scroll = 0;

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Prompts;
    app.focus = Focus::Content;
    app.form = Some(FormState::PromptMeta(form));

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        80,
        20,
    ));
    assert!(all.contains("DISTANT-CURSOR-TARGET"), "{all}");
    assert!(!all.contains("line-00000"), "{all}");
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
fn provider_field_label_and_value_renders_claude_hide_attribution_toggle() {
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.toggle_claude_hide_attribution();

    let (label, value) = super::provider_field_label_and_value(
        &form,
        crate::cli::tui::form::ProviderAddField::ClaudeHideAttribution,
    );

    assert!(label.contains("署名") || label.contains("Attribution"));
    assert_eq!(value, "[✓]");
}

#[test]
fn provider_field_label_and_value_renders_na_for_blank_hermes_rate_limit_delay() {
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Hermes);

    let (label, value) = super::provider_field_label_and_value(
        &form,
        crate::cli::tui::form::ProviderAddField::HermesRateLimitDelay,
    );
    assert_eq!(label, texts::tui_label_hermes_rate_limit_delay());
    assert_eq!(value, texts::tui_na());

    form.hermes_rate_limit_delay.set("0.5");
    let (_label, value) = super::provider_field_label_and_value(
        &form,
        crate::cli::tui::form::ProviderAddField::HermesRateLimitDelay,
    );
    assert_eq!(value, "0.5");
}

#[test]
fn provider_form_fields_show_dashed_divider_before_hermes_rate_limit_delay() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Hermes));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.form = Some(crate::cli::tui::form::FormState::ProviderAdd(
        crate::cli::tui::form::ProviderAddFormState::new(AppType::Hermes),
    ));

    let data = minimal_data(&app.app_type);
    let buf = render(&app, &data);

    let mut rate_limit_y = None;
    for y in 0..buf.area.height {
        let line = line_at(&buf, y);
        if line.contains("Rate limit") || line.contains("请求间隔") {
            rate_limit_y = Some(y);
            break;
        }
    }

    let rate_limit_y =
        rate_limit_y.expect("Hermes rate limit delay row missing from provider form");
    let above = line_at(&buf, rate_limit_y.saturating_sub(1));
    assert!(
        above.contains("┄┄┄"),
        "expected dashed divider row above Hermes rate limit delay, got: {above}"
    );
}

#[test]
fn provider_form_disabled_entries_show_only_disabled_while_enabled_entries_keep_summaries() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;

    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.focus = FormFocus::Fields;
    form.name.set("Demo Provider");

    let (_label, disabled_usage) = super::provider_field_label_and_value(
        &form,
        crate::cli::tui::form::ProviderAddField::UsageQuery,
    );
    assert_eq!(disabled_usage, texts::disabled());

    form.usage_query_enabled = true;
    let (_label, enabled_usage) = super::provider_field_label_and_value(
        &form,
        crate::cli::tui::form::ProviderAddField::UsageQuery,
    );
    assert!(enabled_usage.contains(texts::enabled()), "{enabled_usage}");
    assert!(
        enabled_usage.contains(form.usage_query_template_value()),
        "{enabled_usage}"
    );

    let mut codex_form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Codex);
    codex_form.id.set("relay");
    let (_label, disabled_routing) = super::provider_field_label_and_value(
        &codex_form,
        crate::cli::tui::form::ProviderAddField::CodexLocalRouting,
    );
    assert_eq!(disabled_routing, texts::disabled());

    codex_form.toggle_codex_local_routing_enabled();
    let (_label, enabled_routing) = super::provider_field_label_and_value(
        &codex_form,
        crate::cli::tui::form::ProviderAddField::CodexLocalRouting,
    );
    assert!(
        enabled_routing.contains(texts::enabled()),
        "{enabled_routing}"
    );
    assert!(enabled_routing.contains('·'), "{enabled_routing}");

    app.form = Some(FormState::ProviderAdd(form));
    let all = all_text(&render(&app, &minimal_data(&app.app_type)));
    assert!(all.contains("Usage Query"), "{all}");
}

#[test]
fn provider_form_usage_query_page_omits_duplicate_side_panel_status_and_inline_key_hint() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;

    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.focus = FormFocus::Fields;
    form.claude_base_url.set("https://relay.example.test");
    form.open_usage_query_page();
    form.toggle_usage_query_enabled();
    app.form = Some(FormState::ProviderAdd(form));

    let all = all_text(&render(&app, &minimal_data(&app.app_type)));

    assert!(all.contains("Preset template"), "{all}");
    assert!(all.contains("Enable usage query"), "{all}");
    assert!(all.contains("Extractor code | Return object"), "{all}");
    assert!(!all.contains("Extractor code          open"), "{all}");
    assert!(!all.contains("Preset template:"), "{all}");
    assert!(!all.contains("Enable usage query:"), "{all}");
    assert!(!all.contains("Enter edits text"), "{all}");
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
    data.proxy.active_worker_apps =
        std::collections::HashSet::from([AppType::Claude.as_str().to_string()]);
    data.proxy.configured_listen_address = "127.0.0.1".to_string();
    data.proxy.configured_listen_port = 15722;

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(!all.contains("Enter edit"));
    assert!(all.contains("Listen address: stop the proxy to edit"));
    assert!(all.contains("Listen port: stop this app's route to edit"));
}

#[test]
fn settings_proxy_shows_edit_key_when_running_but_app_not_routed() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::SettingsProxy;
    app.focus = Focus::Content;
    app.settings_proxy_idx = app::LocalProxySettingsItem::ALL
        .iter()
        .position(|item| matches!(item, app::LocalProxySettingsItem::ListenPort))
        .expect("ListenPort missing");

    let mut data = minimal_data(&app.app_type);
    data.proxy.running = true;
    data.proxy.claude_takeover = false;
    data.proxy.configured_listen_address = "127.0.0.1".to_string();
    data.proxy.configured_listen_port = 15722;

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains("Enter edit"));
    assert!(all.contains("Listen port can be edited"));
    assert!(!all.contains("Listen port: stop this app's route to edit"));
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
    old_config_dir: Option<OsString>,
}

impl SettingsEnvGuard {
    pub(super) fn set_home(home: &Path) -> Self {
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
        match &self.old_config_dir {
            Some(value) => std::env::set_var("CC_SWITCH_CONFIG_DIR", value),
            None => std::env::remove_var("CC_SWITCH_CONFIG_DIR"),
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

fn cell_column_of(buf: &Buffer, y: u16, needle: &str) -> Option<u16> {
    let cells = needle.chars().map(|ch| ch.to_string()).collect::<Vec<_>>();
    if cells.is_empty() {
        return Some(0);
    }

    (0..buf.area.width).find(|&x| {
        cells.iter().enumerate().all(|(offset, symbol)| {
            let cell_x = x.saturating_add(offset as u16);
            cell_x < buf.area.width && buf[(cell_x, y)].symbol() == symbol
        })
    })
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

fn contains_continuous_divider_row(line: &str) -> bool {
    line.split('│').any(|segment| {
        let divider = segment.trim();
        divider.contains("────") && divider.chars().all(|ch| ch == '─')
    })
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
    // Block titles carry one column of padding on each side.
    format!("┌ {}", buffer_cell_text(title))
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
            live_ids: Default::default(),
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
            loading: false,
        },
        mcp: McpSnapshot::default(),
        prompts: PromptsSnapshot::default(),
        config: ConfigSnapshot::default(),
        skills: SkillsSnapshot::default(),
        proxy: ProxySnapshot::default(),
        usage: UsageSnapshot::default(),
        pricing: Default::default(),
        quota: Default::default(),
        reload_token: Default::default(),
    }
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

fn failover_provider_row(
    id: &str,
    name: &str,
    is_current: bool,
    in_failover_queue: bool,
    sort_index: Option<usize>,
) -> ProviderRow {
    let mut provider = Provider::with_id(id.to_string(), name.to_string(), json!({}), None);
    provider.in_failover_queue = in_failover_queue;
    provider.sort_index = sort_index;

    ProviderRow {
        id: id.to_string(),
        provider,
        api_url: Some("https://example.com".to_string()),
        is_current,
        is_in_config: true,
        is_saved: true,
        is_default_model: false,
        primary_model_id: Some("claude-sonnet-4".to_string()),
        default_model_id: None,
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
            hermes: false,
        },
        installed_at: 1,
        content_hash: None,
        updated_at: 0,
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
fn hermes_models_overlay_separates_models_with_dashed_divider() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Hermes));
    app.route = Route::Providers;
    app.focus = Focus::Content;

    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Hermes);
    form.focus = FormFocus::Fields;
    form.hermes_models = vec![
        json!({ "id": "model-a", "name": "Model A" }),
        json!({ "id": "model-b", "name": "Model B" }),
    ];
    form.open_hermes_models_picker();
    app.form = Some(FormState::ProviderAdd(form));
    app.overlay = Overlay::HermesModelsPicker { editing: false };

    let content = all_text(&render(&app, &minimal_data(&app.app_type)));
    let first_model = line_index(&content, &buffer_cell_text("model-a"));
    let divider = line_index(&content, "┄┄┄");
    let second_model = line_index(&content, &buffer_cell_text("model-b"));

    assert!(
        first_model < divider && divider < second_model,
        "expected dashed divider between Hermes models, got:\n{content}"
    );
}

#[test]
fn hermes_models_overlay_materializes_only_the_selected_large_collection_window() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let selected = 9_999usize;
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Hermes);
    form.focus = FormFocus::Fields;
    form.hermes_models = (0..10_000)
        .map(|index| {
            json!({
                "id": format!("HERMES-{index:05}"),
                "name": format!("Hermes {index:05}"),
                "context_length": 131_072
            })
        })
        .collect();
    form.hermes_models[selected]["id"] =
        json!(format!("SELECTED-HERMES-{}", "X".repeat(1_000_000)));
    form.hermes_models_field_idx = selected.saturating_mul(3);

    let mut app = App::new(Some(AppType::Hermes));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.form = Some(FormState::ProviderAdd(form));
    app.overlay = Overlay::HermesModelsPicker { editing: false };

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        120,
        40,
    ));
    assert!(all.contains("SELECTED-HERMES"), "{all}");
    assert!(!all.contains("HERMES-00000"), "{all}");
}

#[test]
fn provider_form_json_preview_highlights_common_config_lines() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.focus = FormFocus::JsonPreview;
    form.include_common_config = true;
    form.claude_api_key.set("sk-provider");
    app.form = Some(FormState::ProviderAdd(form));

    let mut data = minimal_data(&app.app_type);
    data.config.common_snippet = r#"{
        "env": {
            "COMMON_FLAG": "1"
        }
    }"#
    .to_string();

    let buf = render(&app, &data);
    let theme = theme_for(&app.app_type);
    let mut common_bg = None;
    let mut common_indent_bg = None;
    let mut provider_key_bg = None;

    for y in 0..buf.area.height {
        if let Some(x) = cell_column_of(&buf, y, "\"COMMON_FLAG\"") {
            common_bg = Some(buf[(x, y)].bg);
            common_indent_bg = x.checked_sub(1).map(|indent_x| buf[(indent_x, y)].bg);
        }
        if let Some(x) = cell_column_of(&buf, y, "\"ANTHROPIC_AUTH_TOKEN\"") {
            provider_key_bg = Some(buf[(x, y)].bg);
        }
    }

    assert_eq!(common_bg, Some(theme.surface));
    assert_ne!(common_indent_bg, Some(theme.surface));
    assert_ne!(provider_key_bg, Some(theme.surface));
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
fn header_renders_failover_indicator_inside_proxy_chip() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows[0].is_current = true;
    data.proxy.running = true;
    data.proxy.claude_takeover = true;
    data.proxy.auto_failover_enabled = true;

    let buf = render(&app, &data);
    let header = line_at(&buf, 1);
    let proxy_label = texts::tui_header_proxy_status_with_failover(true, true);

    assert!(header.contains(&proxy_label), "{header}");
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
        hermes: false,
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
        hermes: false,
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
        hermes: false,
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
fn settings_page_shows_visible_apps_mode_row_value() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");
    let temp_home = TempDir::new().expect("create temp home");
    let _home = SettingsEnvGuard::set_home(temp_home.path());
    crate::settings::set_visible_apps_mode(crate::settings::VisibleAppsMode::Manual)
        .expect("save visible apps mode");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Settings;
    app.focus = Focus::Content;

    let all = all_text(&render(&app, &minimal_data(&app.app_type)));

    assert!(
        all.contains(texts::tui_settings_visible_apps_mode_label()),
        "{all}"
    );
    assert!(
        all.contains(texts::tui_settings_visible_apps_mode_manual()),
        "{all}"
    );
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
#[serial(home_settings)]
fn settings_page_shows_preferred_editor_and_truncates_long_command() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let temp_home = TempDir::new().expect("create temp home");
    let _home = SettingsEnvGuard::set_home(temp_home.path());
    let command = format!(
        "code --wait --reuse-window --profile {}UNSEEN_TAIL",
        "x".repeat(160)
    );
    crate::settings::set_preferred_editor(Some(command)).expect("save preferred editor");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Settings;
    app.focus = Focus::Content;

    let buf = render_with_size(&app, &minimal_data(&app.app_type), 72, 32);
    let editor_line = (0..buf.area.height)
        .map(|y| line_at(&buf, y))
        .find(|line| line.contains("External Editor"))
        .expect("settings should render the External Editor row");

    assert!(editor_line.contains('…'), "{editor_line}");
    assert!(!editor_line.contains("UNSEEN_TAIL"), "{editor_line}");
}

#[test]
#[serial(home_settings)]
fn settings_page_shows_external_editor_as_not_set_by_default() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let temp_home = TempDir::new().expect("create temp home");
    let _home = SettingsEnvGuard::set_home(temp_home.path());

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Settings;
    app.focus = Focus::Content;

    let buf = render_with_size(&app, &minimal_data(&app.app_type), 96, 32);
    let editor_line = (0..buf.area.height)
        .map(|y| line_at(&buf, y))
        .find(|line| line.contains("External Editor"))
        .expect("settings should render the External Editor row");

    assert!(editor_line.contains("Not set"), "{editor_line}");
    assert!(!editor_line.contains("Automatic"), "{editor_line}");
}

#[test]
#[serial(home_settings)]
fn external_editor_picker_renders_detected_and_fallback_options() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::set("NO_COLOR", "1");
    let temp_home = TempDir::new().expect("create temp home");
    let _home = SettingsEnvGuard::set_home(temp_home.path());
    crate::settings::set_preferred_editor(Some("code --wait".to_string()))
        .expect("save preferred editor");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Settings;
    app.focus = Focus::Content;
    app.overlay = Overlay::ExternalEditorPicker {
        selected: 0,
        editors: vec![
            crate::cli::editor::DetectedEditor {
                label: "Visual Studio Code".to_string(),
                command: "code --wait".to_string(),
            },
            crate::cli::editor::DetectedEditor {
                label: "Neovim".to_string(),
                command: "nvim".to_string(),
            },
        ],
    };

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        100,
        28,
    ));

    assert!(all.contains("External Editor"), "{all}");
    assert!(!all.contains("Automatic"), "{all}");
    assert!(all.contains("Visual Studio Code  ·  code --wait"), "{all}");
    assert!(all.contains("Neovim  ·  nvim"), "{all}");
    assert!(all.contains("Custom command…"), "{all}");
    assert!(all.contains("Enter=apply"), "{all}");
    assert!(!all.contains("j/k"), "{all}");
}

#[test]
fn settings_page_shows_managed_accounts_summary() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Settings;
    app.focus = Focus::Content;
    app.managed_auth_status = Some(managed_auth_status());

    let all = all_text(&render(&app, &minimal_data(&app.app_type)));

    assert!(
        all.contains(texts::tui_settings_managed_accounts_title()),
        "{all}"
    );
    assert!(all.contains("default@example.com"), "{all}");
}

#[test]
fn configuration_page_groups_actions_with_three_unlabeled_dividers() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Config;
    app.focus = Focus::Content;

    let buf = render_with_size(&app, &minimal_data(&app.app_type), 100, 32);
    let content = content_text(&app, &buf);
    let dividers = content
        .lines()
        .enumerate()
        .filter_map(|(idx, line)| contains_continuous_divider_row(line).then_some(idx))
        .collect::<Vec<_>>();
    let path = line_index(&content, texts::tui_config_item_show_path());
    let show_full = line_index(&content, texts::tui_config_item_show_full());
    let export = line_index(&content, texts::tui_config_item_export());
    let validate = line_index(&content, texts::tui_config_item_validate());
    let snippet = line_index(&content, texts::tui_config_item_common_snippet());
    let cloud_sync = line_index(&content, texts::tui_config_item_cloud_sync());
    let reset = line_index(&content, texts::tui_config_item_reset());

    assert_eq!(dividers.len(), 3, "{content}");
    assert!(path < show_full && show_full < dividers[0], "{content}");
    assert!(dividers[0] < export && validate < dividers[1], "{content}");
    assert!(
        dividers[1] < snippet && cloud_sync < dividers[2],
        "{content}"
    );
    assert!(dividers[2] < reset, "{content}");
}

#[test]
fn configuration_page_hides_dividers_while_filtering() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Config;
    app.focus = Focus::Content;
    app.filter.input.set("config".to_string());

    let content = content_text(
        &app,
        &render_with_size(&app, &minimal_data(&app.app_type), 80, 32),
    );

    assert!(
        content.contains(texts::tui_config_item_show_full()),
        "{content}"
    );
    assert!(
        content.contains(texts::tui_config_item_common_snippet()),
        "{content}"
    );
    assert!(
        content.contains(texts::tui_config_item_reset()),
        "{content}"
    );
    assert!(
        !content.lines().any(contains_continuous_divider_row),
        "filtered results should not contain dividers:\n{content}"
    );
}

#[test]
fn configuration_dividers_do_not_shift_narrow_terminal_selection() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::set("NO_COLOR", "1");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Config;
    app.focus = Focus::Content;
    app.config_idx = super::config_items_filtered(&app)
        .iter()
        .position(|item| matches!(item, ConfigItem::Reset))
        .expect("Reset missing from visible config items");

    let buf = render_with_size(&app, &minimal_data(&app.app_type), 50, 18);
    let reset_y = (0..buf.area.height)
        .find(|y| line_at(&buf, *y).contains(texts::tui_config_item_reset()))
        .unwrap_or_else(|| panic!("Reset should scroll into view:\n{}", all_text(&buf)));
    let divider_y = (0..buf.area.height)
        .filter(|y| contains_continuous_divider_row(&line_at(&buf, *y)))
        .next_back()
        .unwrap_or_else(|| panic!("last divider should stay visible:\n{}", all_text(&buf)));

    assert!(
        (0..buf.area.width).any(|x| buf[(x, reset_y)].modifier.contains(Modifier::REVERSED)),
        "Reset should own the highlight:\n{}",
        all_text(&buf)
    );
    assert!(
        (0..buf.area.width).all(|x| !buf[(x, divider_y)].modifier.contains(Modifier::REVERSED)),
        "divider rows must not be selectable:\n{}",
        all_text(&buf)
    );
}

#[test]
fn settings_page_groups_items_with_unlabeled_dividers() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Settings;
    app.focus = Focus::Content;

    let buf = render_with_size(&app, &minimal_data(&app.app_type), 100, 32);
    let content = content_text(&app, &buf);
    let dividers = content
        .lines()
        .enumerate()
        .filter_map(|(idx, line)| contains_continuous_divider_row(line).then_some(idx))
        .collect::<Vec<_>>();
    let managed_accounts = line_index(&content, texts::tui_settings_managed_accounts_title());
    let editor = line_index(&content, texts::tui_settings_preferred_editor_label());
    let visible_apps = line_index(&content, texts::tui_settings_visible_apps_mode_label());
    let openclaw_dir = line_index(&content, texts::tui_settings_openclaw_config_dir_label());
    let claude_integration = line_index(&content, texts::enable_claude_plugin_integration_label());
    let codex_login = line_index(&content, texts::codex_preserve_official_auth_label());
    let codex_history = line_index(&content, texts::codex_unified_session_history_label());
    let proxy = line_index(&content, texts::tui_config_item_proxy());

    assert_eq!(dividers.len(), 3, "{content}");
    assert!(
        managed_accounts < editor && editor < dividers[0],
        "{content}"
    );
    assert!(
        dividers[0] < visible_apps && openclaw_dir < dividers[1],
        "{content}"
    );
    assert!(
        dividers[1] < claude_integration
            && claude_integration < codex_login
            && codex_login < codex_history
            && codex_history < dividers[2],
        "{content}"
    );
    assert!(dividers[2] < proxy, "{content}");
    assert!(
        !content.lines().any(|line| {
            line.contains(texts::tui_settings_header_setting())
                && line.contains(texts::tui_settings_header_value())
        }),
        "table header should be absent:\n{content}"
    );
    for label in ["General", "Applications", "Integrations", "System"] {
        assert!(!content.contains(label), "unexpected {label}:\n{content}");
    }
}

#[test]
fn settings_section_rows_do_not_shift_selection_or_break_narrow_scrolling() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::set("NO_COLOR", "1");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Settings;
    app.focus = Focus::Content;
    app.settings_idx = SettingsItem::ALL
        .iter()
        .position(|item| matches!(item, SettingsItem::CheckForUpdates))
        .expect("CheckForUpdates missing from SettingsItem::ALL");

    let buf = render_with_size(&app, &minimal_data(&app.app_type), 50, 18);
    let content_x = content_origin_x(&app, &buf);
    let version_major = env!("CARGO_PKG_VERSION")
        .split('.')
        .next()
        .expect("package version should have a major component");
    let version_prefix = format!("v{version_major}");
    let selected_y = (0..buf.area.height)
        .find(|y| {
            line_at(&buf, *y).contains(&version_prefix)
                && (content_x..buf.area.width)
                    .any(|x| buf[(x, *y)].modifier.contains(Modifier::REVERSED))
        })
        .unwrap_or_else(|| {
            panic!(
                "selected setting should scroll into view:\n{}",
                all_text(&buf)
            )
        });
    let selected_line = line_at(&buf, selected_y);
    assert!(
        selected_line.contains("Check"),
        "selected update row should remain identifiable when truncated:\n{}",
        all_text(&buf)
    );
    let divider_y = (0..buf.area.height)
        .filter(|y| contains_continuous_divider_row(&line_at(&buf, *y)))
        .next_back()
        .unwrap_or_else(|| panic!("last divider should stay visible:\n{}", all_text(&buf)));

    assert!(
        (0..buf.area.width).any(|x| buf[(x, selected_y)].modifier.contains(Modifier::REVERSED)),
        "selected setting should own the highlight:\n{}",
        all_text(&buf)
    );
    assert!(
        (0..buf.area.width).all(|x| !buf[(x, divider_y)].modifier.contains(Modifier::REVERSED)),
        "section rows must not be selectable:\n{}",
        all_text(&buf)
    );
}

#[test]
fn settings_managed_accounts_page_renders_multi_account_manager() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::SettingsManagedAccounts;
    app.focus = Focus::Content;
    app.managed_auth_status = Some(managed_auth_status());

    let all = all_text(&render(&app, &minimal_data(&app.app_type)));

    assert!(
        all.contains(texts::tui_settings_managed_accounts_title()),
        "{all}"
    );
    assert!(
        all.contains(texts::tui_managed_accounts_provider_column()),
        "{all}"
    );
    assert!(
        all.contains(texts::tui_managed_accounts_chatgpt_provider()),
        "{all}"
    );
    assert!(
        all.contains(texts::tui_managed_accounts_details_title()),
        "{all}"
    );
    assert!(
        all.contains(texts::tui_managed_accounts_authenticated()),
        "{all}"
    );
    assert!(all.contains("default@example.com"), "{all}");
    assert!(all.contains("alt@example.com"), "{all}");
    assert!(all.contains("acc-default"), "{all}");
    assert!(all.contains(texts::tui_managed_accounts_default()), "{all}");
    assert!(all.contains(texts::tui_key_add_account()), "{all}");
    assert!(all.contains(texts::tui_key_switch()), "{all}");
    assert!(
        !all.contains(texts::tui_managed_accounts_login_status()),
        "{all}"
    );
    assert!(
        !all.contains(texts::tui_managed_accounts_login_waiting()),
        "{all}"
    );
    assert!(
        !all.contains(texts::tui_managed_accounts_login_idle()),
        "{all}"
    );
}

#[test]
fn settings_managed_accounts_login_renders_only_toast() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::SettingsManagedAccounts;
    app.focus = Focus::Content;
    app.managed_auth_login = Some(crate::cli::tui::app::ManagedAuthLoginState {
        auth_provider: "codex_oauth".to_string(),
        device_code: "device-1".to_string(),
        expires_at_tick: 900,
        poll_interval_ticks: 5,
        next_poll_tick: 0,
    });
    app.push_persistent_toast(
        texts::tui_toast_managed_auth_login_in_progress(
            "USER-1",
            "https://auth.example.test/device",
        ),
        crate::cli::tui::app::ToastKind::Info,
    );

    let buf = render(&app, &minimal_data(&app.app_type));
    let all = all_text(&buf);

    let rows = (0..buf.area.height)
        .map(|y| line_at(&buf, y))
        .collect::<Vec<_>>();
    let title_row = rows
        .iter()
        .position(|row| row.contains("ChatGPT login in progress"))
        .expect("login toast should render title line");
    let code_row = rows
        .iter()
        .position(|row| row.contains("Code: USER-1"))
        .expect("login toast should render code line");
    let url_row = rows
        .iter()
        .position(|row| row.contains("Verification URL: https://auth.example.test/device"))
        .expect("login toast should render URL line");
    let cancel_row = rows
        .iter()
        .position(|row| row.contains("Press Esc to cancel"))
        .expect("login toast should render cancel hint line");
    assert!(title_row < code_row && code_row < url_row && url_row < cancel_row);
    assert!(!rows[title_row].contains("USER-1"), "{}", rows[title_row]);
    assert!(all.contains("USER-1"), "{all}");
    assert!(all.contains("Press Esc to cancel"), "{all}");
    assert!(!all.contains("Esc/c"), "{all}");
    assert!(
        !all.contains(texts::tui_managed_accounts_login_status()),
        "{all}"
    );
    assert!(
        !all.contains(texts::tui_managed_accounts_login_waiting()),
        "{all}"
    );
    assert!(
        !all.contains(texts::tui_managed_accounts_login_idle()),
        "{all}"
    );
}

#[test]
fn managed_auth_cancel_confirm_renders_above_login_toast() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::SettingsManagedAccounts;
    app.focus = Focus::Content;
    app.managed_auth_login = Some(crate::cli::tui::app::ManagedAuthLoginState {
        auth_provider: "codex_oauth".to_string(),
        device_code: "device-1".to_string(),
        expires_at_tick: 900,
        poll_interval_ticks: 5,
        next_poll_tick: 0,
    });
    app.push_persistent_toast(
        texts::tui_toast_managed_auth_login_in_progress(
            "USER-1",
            "https://auth.example.test/device",
        ),
        crate::cli::tui::app::ToastKind::Info,
    );
    app.overlay = Overlay::Confirm(ConfirmOverlay {
        title: texts::tui_confirm_managed_auth_cancel_title().to_string(),
        message: texts::tui_confirm_managed_auth_cancel_message().to_string(),
        action: ConfirmAction::ManagedAuthCancelLogin,
    });

    let all = all_text(&render(&app, &minimal_data(&app.app_type)));

    assert!(
        all.contains(texts::tui_confirm_managed_auth_cancel_title()),
        "{all}"
    );
    // The message may wrap at any word boundary, so match it on the
    // border-stripped, whitespace-normalized text instead of a single row.
    let flattened = all
        .chars()
        .map(|ch| match ch {
            '│' | '┌' | '┐' | '└' | '┘' | '─' | '├' | '┤' => ' ',
            other => other,
        })
        .collect::<String>();
    let flattened = flattened.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flattened.contains("Press Enter to cancel, or Esc to keep waiting."),
        "{all}"
    );

    let key_bar_row = all
        .lines()
        .find(|line| line.contains("Enter cancel login"))
        .expect("managed auth cancel confirm should render a specific key bar");
    assert!(key_bar_row.contains("Esc keep waiting"), "{key_bar_row}");
    assert!(!key_bar_row.contains("Enter confirm"), "{key_bar_row}");
    assert!(!key_bar_row.contains("Esc cancel"), "{key_bar_row}");
}

#[test]
fn managed_account_binding_picker_renders_follow_default_and_accounts() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.managed_auth_status = Some(managed_auth_status());
    app.overlay = Overlay::ManagedAccountPicker {
        auth_provider: "codex_oauth".to_string(),
        selected: 0,
        binding: true,
        selected_account_id: None,
    };

    let all = all_text(&render(&app, &minimal_data(&app.app_type)));

    assert!(all.contains(texts::tui_label_chatgpt_account()), "{all}");
    assert!(
        all.contains(texts::tui_managed_accounts_follow_default()),
        "{all}"
    );
    assert!(all.contains("default@example.com"), "{all}");
    assert!(all.contains("alt@example.com"), "{all}");
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
            hermes: false,
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
    assert!(all.contains(AppType::Hermes.as_str()), "{all}");
    assert!(all.contains(AppType::OpenClaw.as_str()), "{all}");
    assert!(
        all.contains(texts::tui_toast_visible_apps_zero_selection_warning()),
        "{all}"
    );
}

#[test]
fn visible_apps_picker_uses_space_toggle_key() {
    let _lock = lock_env();
    let _no_color = EnvGuard::set("NO_COLOR", "1");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Settings;
    app.focus = Focus::Content;
    app.overlay = Overlay::VisibleAppsPicker {
        selected: 0,
        apps: crate::settings::VisibleApps {
            claude: true,
            codex: false,
            gemini: false,
            opencode: false,
            hermes: false,
            openclaw: false,
        },
    };

    let all = all_text(&render(&app, &minimal_data(&app.app_type)));

    assert!(all.contains("Space=toggle"), "{all}");
    assert!(!all.contains("x=toggle"), "{all}");
}

#[test]
#[serial(home_settings)]
fn visible_apps_picker_auto_mode_does_not_append_auto_suffix_to_apps() {
    let _lock = lock_env();
    let _no_color = EnvGuard::set("NO_COLOR", "1");
    let temp_home = TempDir::new().expect("create temp home");
    let _home = SettingsEnvGuard::set_home(temp_home.path());
    crate::settings::set_visible_apps_mode(crate::settings::VisibleAppsMode::Auto)
        .expect("save visible apps mode");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Settings;
    app.focus = Focus::Content;
    app.overlay = Overlay::VisibleAppsPicker {
        selected: 2,
        apps: crate::settings::VisibleApps {
            claude: true,
            codex: true,
            gemini: true,
            opencode: true,
            hermes: true,
            openclaw: true,
        },
    };

    let all = all_text(&render(&app, &minimal_data(&app.app_type)));

    assert!(all.contains(AppType::Gemini.as_str()), "{all}");
    assert!(
        !all.contains(&format!(
            "{}  {}",
            AppType::Gemini.as_str(),
            texts::tui_settings_visible_apps_mode_auto()
        )),
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
        active: Some(0),
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
fn openclaw_agents_picker_bounds_large_options_and_keeps_edge_selections_visible() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let middle = 5_000usize;
    let last = 9_999usize;
    let mut options = (0..10_000)
        .map(|index| app::OpenClawModelOption {
            value: format!("demo/model-{index:05}"),
            label: format!("MODEL-{index:05}"),
        })
        .collect::<Vec<_>>();
    options[0].label = "FIRST-OPTION".to_string();
    options[middle].label = format!("SELECTED-MIDDLE-{}", "X".repeat(2 * 1024 * 1024));
    options[last].label = "LAST-OPTION".to_string();

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawAgents;
    app.focus = Focus::Content;
    app.overlay = Overlay::OpenClawAgentsFallbackPicker {
        insert_at: 0,
        selected: 0,
        active: None,
        options,
    };

    for (selected, visible, hidden) in [
        (0, "FIRST-OPTION", "SELECTED-MIDDLE"),
        (middle, "SELECTED-MIDDLE", "FIRST-OPTION"),
        (last, "LAST-OPTION", "SELECTED-MIDDLE"),
    ] {
        let Overlay::OpenClawAgentsFallbackPicker {
            selected: picker_selected,
            ..
        } = &mut app.overlay
        else {
            panic!("expected OpenClaw model picker");
        };
        *picker_selected = selected;

        let all = all_text(&render_with_size(
            &app,
            &minimal_data(&app.app_type),
            100,
            35,
        ));
        assert!(all.contains(visible), "selected={selected}: {all}");
        assert!(!all.contains(hidden), "selected={selected}: {all}");
        assert!(all.len() < 20_000, "picker frame was not bounded");
    }

    let Overlay::OpenClawAgentsFallbackPicker { options, .. } = &app.overlay else {
        panic!("expected OpenClaw model picker");
    };
    assert_eq!(options.len(), 10_000);
    assert_eq!(
        options[middle].label.len(),
        "SELECTED-MIDDLE-".len() + 2 * 1024 * 1024,
        "display truncation must not mutate the saved option"
    );
}

#[test]
#[serial(home_settings)]
fn header_centers_tabs_when_room_allows() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");
    let temp_home = TempDir::new().expect("create temp home");
    let _home = SettingsEnvGuard::set_home(temp_home.path());
    crate::settings::set_visible_apps(crate::settings::VisibleApps {
        claude: true,
        codex: true,
        gemini: true,
        opencode: true,
        hermes: true,
        openclaw: true,
    })
    .expect("save visible apps");

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
#[serial(home_settings)]
fn header_keeps_title_and_right_badges_visible_without_large_gap_in_chinese() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let temp_home = TempDir::new().expect("create temp home");
    let _home = SettingsEnvGuard::set_home(temp_home.path());
    crate::settings::set_visible_apps(crate::settings::VisibleApps {
        claude: true,
        codex: true,
        gemini: true,
        opencode: true,
        hermes: true,
        openclaw: true,
    })
    .expect("save visible apps");

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
    // This test is specifically about the emoji icon column, so pin emoji
    // mode rather than depend on the ambient locale (auto-detection).
    let _icons = EnvGuard::set("CC_SWITCH_ICONS", "emoji");

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
fn providers_empty_state_matches_gui_copy_in_chinese() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;

    let all = all_text(&render(&app, &UiData::default()));
    let compact = all.replace(' ', "");

    assert!(compact.contains("还没有添加任何供应商"), "{all}");
    assert!(
        compact.contains(
            "如果你已有配置，请点击\"导入当前配置\"，所有数据将安全保存在default供应商中"
        ),
        "{all}"
    );
    assert!(compact.contains("Enter导入当前配置"), "{all}");
    assert!(compact.contains("a添加供应商"), "{all}");
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
            source: crate::cli::tui::app::CommonSnippetViewSource::Global,
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
            source: crate::cli::tui::app::CommonSnippetViewSource::Global,
        },
    );

    let data = minimal_data(&app.app_type);
    let buf = render(&app, &data);

    let has_ctrl_o = (0..buf.area.height).any(|y| line_at(&buf, y).contains("Ctrl+O"));
    assert!(has_ctrl_o, "editor key bar should show the Ctrl+O hint");
}

#[test]
fn common_snippet_editor_key_bar_shows_format_and_extract_hints() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.open_editor(
        "Common Snippet",
        EditorKind::Json,
        r#"{"env":{"COMMON_FLAG":"1"}}"#,
        EditorSubmit::ConfigCommonSnippet {
            app_type: app.app_type.clone(),
            source: crate::cli::tui::app::CommonSnippetViewSource::ProviderForm,
        },
    );

    let data = minimal_data(&app.app_type);
    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(
        all.contains("F2"),
        "editor should show format shortcut: {all}"
    );
    assert!(
        all.contains("format"),
        "editor should show format label: {all}"
    );
    assert!(
        all.contains("F4"),
        "editor should show extract shortcut: {all}"
    );
    assert!(
        all.contains("extract"),
        "editor should show extract label: {all}"
    );
}

#[test]
fn prompt_form_content_key_bar_shows_ctrl_o_external_editor_hint() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Prompts;
    app.focus = Focus::Content;
    let mut form = PromptMetaFormState::new("prompt-one".to_string(), "Prompt One".to_string());
    form.focus = FormFocus::Content;
    app.form = Some(FormState::PromptMeta(form));

    let data = minimal_data(&app.app_type);
    let buf = render(&app, &data);

    let has_ctrl_o = (0..buf.area.height).any(|y| line_at(&buf, y).contains("Ctrl+O"));
    assert!(
        has_ctrl_o,
        "prompt content editor key bar should show the Ctrl+O hint"
    );
}

#[test]
fn home_replaces_the_logo_hero_with_the_usage_chart() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let all = all_text(&buf);
    assert!(!all.contains("___  ___"), "{all}");
    assert!(all.contains("Usage · 30d"), "{all}");
    assert!(all.contains("Connection Details"));
    assert_eq!(
        line_index(&all, "Connection Details"),
        line_index(&all, "CC-Switch") + 1,
        "connection details should start directly below the home title"
    );
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
                hermes: false,
            },
            installed_at: 0,
            content_hash: None,
            updated_at: 0,
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
            content_hash: None,
            updated_at: 0,
        },
    ];

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains("MCP:"), "{all}");
    assert!(all.contains("Skills: [1/2 Active]"), "{all}");
}

#[test]
fn home_opencode_reports_configured_provider_count_instead_of_current_provider_none() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenCode));
    app.route = Route::Main;
    app.focus = Focus::Content;
    let mut data = minimal_data(&app.app_type);
    data.providers.current_id.clear();
    data.providers.rows[0].is_current = false;
    data.providers.rows[0].is_in_config = true;

    let all = all_text(&render(&app, &data));

    assert!(all.contains("Provider"), "{all}");
    assert!(all.contains("1/1 in config"), "{all}");
    assert!(!all.contains("None"), "{all}");
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

    assert!(!all.contains("Interactive Mode"), "{all}");
    let needle = "CC-Switch";
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
    for tool_name in [
        "Claude", "Codex", "Gemini", "OpenCode", "Hermes", "OpenClaw",
    ] {
        assert!(all.contains(tool_name), "missing {tool_name} in:\n{all}");
    }
    assert!(!all.contains("Session Context"));
}

#[test]
fn home_updates_local_tool_versions_independently() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    use crate::services::local_env_check::{LocalTool, ToolCheckResult, ToolCheckStatus};

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;
    app.tick = 0;
    app.local_env_pending.remove(&LocalTool::Hermes);
    app.local_env_results.push(ToolCheckResult {
        tool: LocalTool::Hermes,
        display_name: LocalTool::Hermes.display_name(),
        status: ToolCheckStatus::Ok {
            version: "1.2.3".to_string(),
        },
    });
    let data = minimal_data(&app.app_type);

    let first = all_text(&render(&app, &data));
    let claude_line = first
        .lines()
        .find(|line| line.contains("Claude"))
        .expect("Claude row should render");
    let hermes_line = first
        .lines()
        .find(|line| line.contains("Hermes"))
        .expect("Hermes row should render");
    assert!(claude_line.contains('⠋'), "{claude_line}");
    assert!(hermes_line.contains('✓'), "{hermes_line}");
    assert!(first.contains("checking version…"), "{first}");
    assert!(first.contains("1.2.3"), "{first}");

    app.tick = 1;
    let second = all_text(&render(&app, &data));
    let claude_line = second
        .lines()
        .find(|line| line.contains("Claude"))
        .expect("Claude row should render");
    assert!(claude_line.contains('⠙'), "{claude_line}");
}

#[test]
fn home_treats_version_timeout_as_installed_but_unavailable() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    use crate::services::local_env_check::{LocalTool, ToolCheckResult, ToolCheckStatus};

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;
    app.stop_local_env_refresh();
    app.local_env_results.push(ToolCheckResult {
        tool: LocalTool::Hermes,
        display_name: LocalTool::Hermes.display_name(),
        status: ToolCheckStatus::VersionUnavailable {
            reason: "version check timed out after 10s".to_string(),
        },
    });
    let data = minimal_data(&app.app_type);

    let all = all_text(&render(&app, &data));
    let hermes_line = all
        .lines()
        .find(|line| line.contains("Hermes"))
        .expect("Hermes row should render");
    assert!(hermes_line.contains('•'), "{hermes_line}");
    assert!(!hermes_line.contains('!'), "{hermes_line}");
    assert!(all.contains("installed · version unavailable"), "{all}");
    assert!(!all.contains("timed out after 10s"), "{all}");
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

    assert!(!all.contains("___  ___"), "{all}");
    assert!(all.contains("Usage · 30d"), "{all}");
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
        provider_id: "claude-test".to_string(),
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
    assert!(!footer.contains("NAV"), "{footer}");
    assert!(!footer.contains("ACT"), "{footer}");
    assert!(!all.contains("___  ___"), "{all}");
    assert!(all.contains("Usage · 30d"), "{all}");
    assert!(!all.contains("Proxy Dashboard"));
}

#[test]
fn home_footer_keeps_proxy_shortcut_visible_on_narrow_chinese_terminal() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _lang = use_test_language(Language::Chinese);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;

    let data = minimal_data(&app.app_type);
    let buf = render_with_size(&app, &data, 80, 24);
    let footer = line_at(&buf, buf.area.height - 1);
    let compact_footer = footer.replace(' ', "");

    assert!(footer.contains("P"), "{footer}");
    assert!(compact_footer.contains("P代理开"), "{footer}");
    assert!(!compact_footer.contains("导航"), "{footer}");
    assert!(!compact_footer.contains("功能"), "{footer}");
}

#[test]
fn home_footer_keeps_proxy_shortcut_visible_on_narrow_chinese_no_color_terminal() {
    let _lock = lock_env();
    let _no_color = EnvGuard::set("NO_COLOR", "1");
    let _lang = use_test_language(Language::Chinese);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;

    let data = minimal_data(&app.app_type);
    let buf = render_with_size(&app, &data, 80, 24);
    let footer = line_at(&buf, buf.area.height - 1);
    let compact_footer = footer.replace(' ', "");

    assert!(footer.contains("P"), "{footer}");
    assert!(compact_footer.contains("P代理开"), "{footer}");
    assert!(!compact_footer.contains("导航"), "{footer}");
    assert!(!compact_footer.contains("功能"), "{footer}");
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
    assert!(!all.contains("___  ___"), "{all}");
    assert!(all.contains("Usage · 30d"), "{all}");
    assert!(!all.contains("Proxy Dashboard"), "{all}");
    assert!(!all.contains("Shared runtime ready"), "{all}");
    assert!(!all.contains("x1.00"), "{all}");
}

#[test]
fn home_proxy_dashboard_stays_off_for_current_worker_without_takeover() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.tick = 1;
    app.route = Route::Main;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.proxy.running = true;
    data.proxy.managed_runtime = true;
    data.proxy.active_worker_apps =
        std::collections::HashSet::from([AppType::Claude.as_str().to_string()]);
    data.proxy.claude_takeover = false;
    data.proxy.codex_takeover = false;
    data.proxy.listen_address = "127.0.0.1".to_string();
    data.proxy.listen_port = 15721;

    let buf = render(&app, &data);
    let all = all_text(&buf);
    let header = line_at(&buf, 1);
    let footer = line_at(&buf, buf.area.height - 1);

    assert!(
        header.contains(&texts::tui_header_proxy_status(false)),
        "{header}"
    );
    assert!(footer.contains("proxy on"), "{footer}");
    assert!(!all.contains("Proxy Dashboard"), "{all}");
    assert!(!all.contains("___  ___"), "{all}");
    assert!(all.contains("Usage · 30d"), "{all}");
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
    assert!(!all.contains("___  ___"), "{all}");
    assert!(all.contains("Usage · 30d"), "{all}");
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
        provider_id: "claude-test".to_string(),
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
    assert!(!shared_text.contains("___  ___"), "{shared_text}");
    assert!(shared_text.contains("Usage · 30d"), "{shared_text}");
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
        provider_id: "claude-test".to_string(),
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
    app.stop_local_env_refresh();

    let off = minimal_data(&app.app_type);
    app.observe_proxy_visual_state(&off);

    let mut on = minimal_data(&app.app_type);
    on.proxy.running = true;
    on.proxy.claude_takeover = true;
    on.proxy.default_cost_multiplier = None;
    on.proxy.current_app_target = Some(super::super::data::ProxyTargetSnapshot {
        provider_id: "p1".to_string(),
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
    assert!(!all.contains("___  ___"), "{all}");
    assert!(all.contains("Usage · 30d"), "{all}");
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

    assert!(!all.contains("___  ___"), "{all}");
    assert!(all.contains("Usage · 30d"), "{all}");
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
        provider_id: "claude-test".to_string(),
        provider_name: "Claude Test Provider".to_string(),
    });

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(!all.contains("___  ___"), "{all}");
    assert!(all.contains("Usage · 30d"), "{all}");
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
fn cloud_sync_backend_page_shows_plain_disabled_status_without_suffix() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::ConfigCloudSync;
    app.focus = Focus::Content;
    let mut data = minimal_data(&app.app_type);
    data.config.webdav_sync = Some(crate::settings::WebDavSyncSettings {
        enabled: false,
        base_url: "https://dav.example.com".to_string(),
        ..crate::settings::WebDavSyncSettings::default()
    });
    data.config.s3_sync = Some(crate::settings::S3SyncSettings {
        enabled: true,
        region: "us-east-1".to_string(),
        bucket: "sync-bucket".to_string(),
        access_key_id: "AKID".to_string(),
        secret_access_key: "SECRET".to_string(),
        ..crate::settings::S3SyncSettings::default()
    });

    let content = content_text(&app, &render_with_size(&app, &data, 72, 16));
    let webdav = line_with(&content, "WebDAV");
    let s3 = line_with(&content, "S3 Compatible");
    assert!(webdav.contains("Disabled"), "{content}");
    assert!(!webdav.contains('·'), "{content}");
    assert!(s3.contains("Enabled"), "{content}");
}

#[test]
fn s3_form_ellipsizes_long_plaintext_and_survives_tiny_terminals() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::ConfigS3;
    app.focus = Focus::Content;
    let secret = format!("plain-secret-{}-tail", "x".repeat(160));
    let settings = crate::settings::S3SyncSettings {
        region: "us-east-1".to_string(),
        bucket: "sync-bucket".to_string(),
        access_key_id: "AKID".to_string(),
        secret_access_key: secret.clone(),
        ..crate::settings::S3SyncSettings::default()
    };
    app.form = Some(FormState::S3Sync(
        crate::cli::tui::form::S3SyncFormState::from_settings(Some(&settings)),
    ));
    let data = minimal_data(&app.app_type);

    let _ = render_with_size(&app, &data, 20, 8);
    let content = content_text(&app, &render_with_size(&app, &data, 78, 18));
    assert!(content.contains("plain-secret-"), "{content}");
    assert!(content.contains('…'), "{content}");
    assert!(!content.contains(&secret), "{content}");
    assert!(!content.contains("[redacted]"), "{content}");
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

    assert!(all.contains(&texts::tui_skills_installed_counts(1, 0, 0, 0, 0)));
    assert!(!all.contains(texts::tui_header_directory()));
    assert!(all.contains(AppType::Claude.as_str()));
    assert!(all.contains(AppType::Codex.as_str()));
    assert!(all.contains(AppType::Gemini.as_str()));
    assert!(all.contains(AppType::OpenCode.as_str()));
    assert!(all.contains(AppType::Hermes.as_str()));
    assert!(!all.contains("hello-skill"));
    assert!(all.contains("Hello Skill"));
}

#[test]
fn skills_page_marks_updates_found_in_current_tui_session() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Skills;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    let skill = installed_skill("hello-skill", "Hello Skill");
    app.skill_updates.insert(
        skill.id.clone(),
        crate::services::skill::SkillUpdateInfo {
            id: skill.id.clone(),
            name: skill.name.clone(),
            directory: skill.directory.clone(),
            current_hash: Some("old".to_string()),
            remote_hash: "new".to_string(),
        },
    );
    data.skills.installed = vec![skill];

    let all = all_text(&render(&app, &data));

    assert!(all.contains(texts::tui_skills_update_marker()), "{all}");
    assert!(
        all.contains(&texts::tui_skills_updates_available(1)),
        "{all}"
    );
    assert!(all.contains(texts::tui_key_update()), "{all}");
}

#[test]
fn skills_page_empty_state_keeps_summary_and_shows_guidance() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Skills;
    app.focus = Focus::Content;

    let data = minimal_data(&app.app_type);
    let buf = render(&app, &data);
    let all = all_text(&buf);

    // The summary bar stays; the blank table body is replaced with the
    // shared empty-state guidance (same style as MCP/Prompts/Providers).
    assert!(all.contains(&texts::tui_skills_installed_counts(0, 0, 0, 0, 0)));
    assert!(all.contains(texts::tui_skills_empty_title()));
    assert!(all.contains(texts::tui_skills_empty_subtitle()));
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
        hermes: false,
    };
    data.skills.installed = vec![skill];

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains("OpenCode: 1"));
}

#[test]
fn skills_page_shows_hermes_column_and_summary() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Hermes));
    app.route = Route::Skills;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    let mut skill = installed_skill("hello-skill", "Hello Skill");
    skill.apps = SkillApps {
        claude: false,
        codex: false,
        gemini: false,
        opencode: false,
        hermes: true,
    };
    data.skills.installed = vec![skill];

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains(AppType::Hermes.as_str()));
    assert!(all.contains("Hermes: 1"));
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
        hermes: false,
    };
    data.skills.installed = vec![skill];

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains(texts::tui_label_enabled_for()));
    assert!(all.contains("OpenCode"));
    assert!(!all.contains("opencode=true"));
}

#[test]
fn skill_detail_page_shows_hermes_enabled_state() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Hermes));
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
        opencode: false,
        hermes: true,
    };
    data.skills.installed = vec![skill];

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains(texts::tui_label_enabled_for()));
    assert!(all.contains("Hermes"));
    assert!(!all.contains("hermes=true"));
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
            path: "/tmp/hello-skill".to_string(),
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
        }
        .into(),
    }];

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains("opencode"));
}

#[test]
fn mcp_page_renders_hermes_column() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Hermes));
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
                opencode: false,
                hermes: true,
            },
            description: None,
            homepage: None,
            docs: None,
            tags: vec![],
        }
        .into(),
    }];

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains("hermes"));
    assert!(all.contains("Hermes: 1"));
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
fn mcp_page_uses_space_toggle_key() {
    let _lock = lock_env();
    let _no_color = EnvGuard::set("NO_COLOR", "1");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Mcp;
    app.focus = Focus::Content;

    // Row-dependent chips (toggle) only show when a row is selectable.
    let mut data = minimal_data(&app.app_type);
    data.mcp.rows = vec![super::super::data::McpRow {
        id: "m1".to_string(),
        server: crate::app_config::McpServer {
            id: "m1".to_string(),
            name: "Server".to_string(),
            server: json!({}),
            apps: crate::app_config::McpApps {
                claude: true,
                codex: false,
                gemini: false,
                opencode: false,
                hermes: false,
            },
            description: None,
            homepage: None,
            docs: None,
            tags: vec![],
        }
        .into(),
    }];
    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains("Space=toggle"), "{all}");
    assert!(!all.contains("x=toggle"), "{all}");
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

fn global_help_text(app_type: AppType) -> String {
    let app = App::new(Some(app_type));
    let data = minimal_data(&app.app_type);
    crate::cli::tui::help::context_help_for_app(&app, &data)
        .lines
        .join("\n")
}

#[test]
fn help_text_mentions_import_existing_for_mcp() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    // MCP and Skills both generate their import line from the same label
    // (tui_*_action_import_existing); assert the wording on BOTH lines so a
    // regression on either page is caught, not just its presence somewhere.
    let help = global_help_text(AppType::Claude);
    let mcp_line = help
        .lines()
        .find(|line| line.starts_with("- MCP:"))
        .unwrap_or_else(|| panic!("no MCP help line: {help}"));
    let skills_line = help
        .lines()
        .find(|line| line.starts_with("- Skills:"))
        .unwrap_or_else(|| panic!("no Skills help line: {help}"));
    assert!(
        mcp_line.contains("i Import Existing"),
        "MCP help line missing shared import wording: {mcp_line}"
    );
    assert!(
        skills_line.contains("i Import Existing"),
        "Skills help line missing shared import wording: {skills_line}"
    );
}

#[test]
fn help_text_shows_space_as_the_mcp_toggle_key() {
    let _lock = lock_env();
    for lang in [Language::English, Language::Chinese] {
        let _lang = use_test_language(lang);
        for app_type in [AppType::Claude, AppType::Hermes] {
            let help = global_help_text(app_type.clone());
            // The MCP toggle handler listens for Space (content_entities.rs);
            // the help sheet used to claim `x` here.
            assert!(
                help.contains("MCP: Space") || help.contains("MCP：Space"),
                "help for {app_type:?}/{lang:?} should document Space as the MCP toggle"
            );
            assert!(
                !help.contains("x toggle current"),
                "stale `x` toggle wording in help for {app_type:?}/{lang:?}"
            );
        }
    }
}

#[test]
fn generated_help_swaps_prompts_for_memory_on_hermes() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);

    let claude = global_help_text(AppType::Claude);
    assert!(claude.contains("- Prompts: "), "{claude}");
    assert!(!claude.contains("- Memory:"), "{claude}");
    assert!(claude.contains("- Config: "), "{claude}");

    let hermes = global_help_text(AppType::Hermes);
    assert!(hermes.contains("- Memory: "), "{hermes}");
    assert!(!hermes.contains("- Prompts:"), "{hermes}");
    // Hermes has no Config route, so no Config line in its help sheet.
    assert!(!hermes.contains("- Config:"), "{hermes}");
}

#[test]
fn generated_usage_help_omits_the_hidden_reverse_tab_alias() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let help = global_help_text(AppType::Claude);
    // Shift+Tab (reverse metric) is a never-shown alias, so it is skipped in
    // the generated help even though Tab (forward) is listed.
    assert!(help.contains("Tab switch metric"), "{help}");
    assert!(!help.contains("Shift+Tab"), "{help}");
}

#[test]
fn page_key_bar_stays_visible_while_nav_has_focus() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let prev = std::env::var("NO_COLOR").ok();
    std::env::set_var("NO_COLOR", "1");
    let _restore_no_color = EnvGuard {
        key: "NO_COLOR",
        prev,
    };

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Mcp;
    app.focus = Focus::Nav;
    let data = minimal_data(&app.app_type);

    // Page keys used to vanish whenever the nav pane had focus, hiding the
    // available actions exactly when a user is deciding where to go.
    let buf = render(&app, &data);
    let all = all_text(&buf);
    assert!(
        all.contains("a=add"),
        "key bar should stay visible (dimmed) with nav focus: {all}"
    );
}

#[test]
fn breadcrumb_title_strips_leading_emoji_in_ascii_mode() {
    let _env_lock = lock_env();
    // Share the crate-wide lock with the other CC_SWITCH_ICONS tests so env
    // mutations don't race across modules.
    let _icons_lock = lock_test_home_and_settings();

    let _emoji = EnvGuard::set("CC_SWITCH_ICONS", "emoji");
    assert!(super::breadcrumb_title(&["🔧 Settings", "Proxy"]).contains('🔧'));

    let _ascii = EnvGuard::set("CC_SWITCH_ICONS", "ascii");
    let ascii = super::breadcrumb_title(&["🔧 Settings", "Proxy"]);
    assert!(!ascii.contains('🔧'), "{ascii}");
    assert!(ascii.contains("Settings › Proxy"), "{ascii}");
}

#[test]
fn nav_respects_icon_mode_and_home_title_stays_plain() {
    let _lock = lock_env();
    let _icons_lock = lock_test_home_and_settings();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::set("NO_COLOR", "1");
    let app = App::new(Some(AppType::Claude));
    let data = minimal_data(&app.app_type);

    let _emoji = EnvGuard::set("CC_SWITCH_ICONS", "emoji");
    let emoji_view = all_text(&render(&app, &data));
    assert!(
        emoji_view.contains('🔑'),
        "emoji mode should render the nav provider icon: {emoji_view}"
    );
    assert!(
        !emoji_view.contains('🎯'),
        "the home title should stay plain in emoji mode: {emoji_view}"
    );

    let _ascii = EnvGuard::set("CC_SWITCH_ICONS", "ascii");
    let ascii_view = all_text(&render(&app, &data));
    assert!(
        !ascii_view.contains('🔑'),
        "ascii mode should drop the nav emoji: {ascii_view}"
    );
    assert!(
        !ascii_view.contains('🎯'),
        "ascii mode should drop the home title emoji: {ascii_view}"
    );
    // The label text still renders — only the decorative glyph is gone.
    assert!(ascii_view.contains("Providers"), "{ascii_view}");
    assert!(ascii_view.contains("CC-Switch"), "{ascii_view}");
}

#[test]
fn empty_lists_show_guidance_instead_of_blank_space() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    for (route, needle) in [
        (Route::Mcp, "No MCP servers yet"),
        (Route::Prompts, "No prompts yet"),
        (Route::Skills, "No installed skills"),
    ] {
        let mut app = App::new(Some(AppType::Claude));
        app.route = route;
        app.focus = Focus::Content;
        let data = minimal_data(&app.app_type);
        let buf = render(&app, &data);
        let all = all_text(&buf);
        assert!(
            all.contains(needle),
            "empty list should show guidance ({needle}): {all}"
        );
    }
}

#[test]
fn provider_form_truncates_overflowing_values_with_ellipsis() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    let long_name = "provider-name-that-is-deliberately-too-long-for-the-value-column";
    form.name.set(long_name);
    app.form = Some(crate::cli::tui::form::FormState::ProviderAdd(form));
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let row = (0..buf.area.height)
        .map(|y| line_at(&buf, y))
        .find(|line| line.contains("provider-name-that"))
        .expect("name row should render");

    // The value column is too narrow for the full label at 120 cols; it
    // used to be cut dead ("…Messages (Nativ") with no truncation marker.
    assert!(
        row.contains('…'),
        "cut-off value should end with an ellipsis: {row:?}"
    );
    assert!(
        !row.contains(long_name),
        "value should be truncated: {row:?}"
    );
}

#[test]
fn key_bar_shows_more_hint_when_chips_overflow() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let prev = std::env::var("NO_COLOR").ok();
    std::env::set_var("NO_COLOR", "1");
    let _restore_no_color = EnvGuard {
        key: "NO_COLOR",
        prev,
    };

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Usage;
    app.focus = Focus::Content;
    let data = minimal_data(&app.app_type);

    // At 80 columns the Usage key bar cannot hold all chips; it used to be
    // cut mid-chip. It must now end with a "? more" hint instead.
    let buf = render_with_size(&app, &data, 80, 35);
    let all = all_text(&buf);
    assert!(
        all.contains("?=more"),
        "overflowing key bar should end with a more-hint: {all}"
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
            }
            .into(),
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
            }
            .into(),
        },
    ];

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains("Installed"));
    assert!(all.contains("Claude: 1"));
}

#[test]
fn skills_discover_page_shows_empty_state() {
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

    assert!(all.contains(texts::tui_skills_discover_empty()));
    assert!(!all.contains(texts::tui_skills_discover_hint()));
}

#[test]
fn skills_discover_page_shows_inline_loading() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::SkillsDiscover;
    app.focus = Focus::Content;
    app.skills_discover_loading = true;

    let data = minimal_data(&app.app_type);
    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains(texts::tui_loading()), "{all}");
}

#[test]
fn skills_discover_marketplace_prompts_for_search() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::SkillsDiscover;
    app.focus = Focus::Content;
    app.skills_discover_source = crate::cli::tui::app::SkillsDiscoverSource::Marketplace;

    let data = minimal_data(&app.app_type);
    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(
        all.contains(texts::tui_skills_skillssh_search_prompt()),
        "{all}"
    );
}

#[test]
fn skills_discover_page_renders_source_tabs() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::SkillsDiscover;
    app.focus = Focus::Content;

    let data = minimal_data(&app.app_type);
    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains(texts::tui_skills_source_repos()), "{all}");
    assert!(
        all.contains(texts::tui_skills_source_marketplace()),
        "{all}"
    );
    assert!(
        all.contains(texts::tui_skills_source_switch_hint()),
        "{all}"
    );
    assert!(all.contains(texts::tui_key_refresh()), "{all}");
    assert!(all.contains(texts::tui_key_repo_manager()), "{all}");
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
        input: TextInput::new("hello".to_string()),
        submit: TextSubmit::ConfigBackupName,
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
fn prompts_page_uses_space_toggle_and_add_key() {
    let _lock = lock_env();
    let _no_color = EnvGuard::set("NO_COLOR", "1");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Prompts;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.prompts.rows.push(crate::cli::tui::data::PromptRow {
        id: "work".to_string(),
        prompt: crate::prompt::Prompt {
            id: "work".to_string(),
            name: "Work Prompt".to_string(),
            content: "Use concise answers.".to_string(),
            description: None,
            enabled: true,
            created_at: None,
            updated_at: None,
        },
    });

    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(all.contains("Prompts"));
    assert!(all.contains(AppType::Claude.as_str()));
    assert!(all.contains("Space=toggle"));
    assert!(all.contains("a=add"));
    assert!(!all.contains("c=create"));
    assert!(!all.contains("x=deactivate"));
    assert!(all.contains(&texts::tui_prompts_summary(1, "Work Prompt")));
}

#[test]
fn codex_history_enable_confirm_renders_warning_and_inline_action_list() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let mut app = App::new(Some(AppType::Codex));
    app.route = Route::Settings;
    app.focus = Focus::Content;
    app.overlay = Overlay::CodexHistoryConfirm(CodexHistoryConfirmState {
        mode: CodexHistoryConfirmMode::Enable,
        show_restore_checkbox: false,
        restore_checked: false,
    });

    let rendered = render_with_size(&app, &minimal_data(&app.app_type), 120, 32);
    let all = all_text(&rendered);
    let flattened = all.split_whitespace().collect::<Vec<_>>().join(" ");

    assert!(all.contains("Unified Codex session history"), "{all}");
    assert!(flattened.contains("encrypted_content reasoning"), "{all}");
    assert!(flattened.contains("Enter enable"), "{all}");
    assert!(flattened.contains("Y migrate and enable"), "{all}");
    assert!(flattened.contains("Esc cancel"), "{all}");
    assert!(
        !all.contains("Space=toggle") && !all.contains("Space toggle"),
        "{all}"
    );

    let theme = theme_for(&app.app_type);
    let button_widths = [
        ("Enter", "enable"),
        ("Y", "migrate and enable"),
        ("Esc", "cancel"),
    ]
    .map(|(key, label)| {
        let y = (0..rendered.area.height)
            .find(|&y| {
                let row = line_at(&rendered, y);
                row.contains(key) && row.contains(label)
            })
            .unwrap_or_else(|| panic!("missing {key} action row in:\n{all}"));
        let row = line_at(&rendered, y);
        let byte = row
            .find(key)
            .unwrap_or_else(|| panic!("missing {key} in row: {row}"));
        let x = UnicodeWidthStr::width(&row[..byte]) as u16;
        assert_eq!(rendered[(x, y)].bg, theme.surface, "{row}");

        let mut left = x;
        while left > 0 && rendered[(left - 1, y)].bg == theme.surface {
            left -= 1;
        }
        let mut right = x;
        while right < rendered.area.width && rendered[(right, y)].bg == theme.surface {
            right += 1;
        }
        right - left
    });
    assert_eq!(button_widths, [button_widths[0]; 3], "{all}");
    assert!(
        button_widths[0] <= 30,
        "action button backgrounds should stay compact: {all}"
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
fn common_config_notice_overlay_shows_all_paragraphs_without_mid_word_breaks() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.overlay = Overlay::Confirm(ConfirmOverlay {
        title: texts::tui_common_config_notice_title().to_string(),
        message: texts::tui_common_config_notice_message("claude"),
        action: ConfirmAction::CommonConfigNotice,
    });
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let all = all_text(&buf);

    // The dialog grows with its message: the last paragraph (F4 / Ctrl+S
    // instructions) used to be clipped by the fixed-height overlay.
    assert!(
        all.contains("F4"),
        "expected the extract-shortcut paragraph to be visible"
    );
    assert!(
        all.contains("Ctrl+S"),
        "expected the save-shortcut paragraph to be visible"
    );

    // Word wrapping keeps words intact: under the old per-character wrap,
    // these words were split across rows ("setting" / "s shared",
    // "default t" / "o attaching") and never appeared whole on any row.
    for word in ["settings", "attaching", "providers."] {
        assert!(
            (0..buf.area.height).any(|y| line_at(&buf, y).contains(word)),
            "expected {word:?} to render unbroken on a single row"
        );
    }
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
    // Height tracks the option count (borders + key bar + gaps + one row per choice).
    let choices = crate::cli::tui::form::ClaudeApiFormat::choices_for_app(&AppType::Claude);
    let height = choices.len() as u16 + 5;
    let area = super::centered_rect_fixed(58, height, content);

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
fn local_proxy_settings_page_is_compact_and_scannable_at_narrow_width() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.name.set("Relay");
    form.custom_user_agent
        .set("claude-cli/2.1.161 (external, cli)");
    form.local_proxy_header_overrides
        .insert("user-agent".to_string(), "relay-agent".to_string());
    form.local_proxy_header_overrides
        .insert("x-provider".to_string(), "cc-switch".to_string());
    form.local_proxy_body_override = Some(json!({ "store": false }));
    form.open_local_proxy_settings_page();
    app.form = Some(FormState::ProviderAdd(form));

    let buf = render_with_size(&app, &minimal_data(&app.app_type), 72, 20);
    let all = all_text(&buf);
    for expected in [
        "Local Proxy Settings",
        "Custom User-Agent",
        "Header Overrides",
        "Body Overrides",
        "2 headers",
        "1 field",
        "claude-cl",
    ] {
        assert!(all.contains(expected), "missing {expected:?}: {all}");
    }
    assert!(!all.contains("u Custom User-Agent"), "{all}");
    assert!(!all.contains("[Select]"), "{all}");
    assert!(!all.contains("[Edit]"), "{all}");
    assert!(!all.contains(r#"{"user-agent"#), "{all}");
    assert!(!all.contains("Only takes effect"), "{all}");

    let field_rows = (0..buf.area.height)
        .map(|y| line_at(&buf, y))
        .filter(|line| {
            line.contains("Custom User-Agent")
                || line.contains("Header Overrides")
                || line.contains("Body Overrides")
        })
        .count();
    assert_eq!(field_rows, 3, "each setting should keep its own row: {all}");
}

#[test]
fn local_proxy_user_agent_row_shows_value_without_action_or_detail_panel() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.custom_user_agent.set(format!(
        "{}claude-cli/2.1.161 (external, cli)",
        "long-prefix-".repeat(8)
    ));
    form.open_local_proxy_settings_page();
    app.form = Some(FormState::ProviderAdd(form));

    let buf = render_with_size(&app, &minimal_data(&app.app_type), 100, 20);

    let user_agent_row = (0..buf.area.height)
        .find(|y| line_at(&buf, *y).contains("Custom User-Agent"))
        .expect("User-Agent row should render");
    let row = line_at(&buf, user_agent_row);
    assert!(row.contains("long-prefix-"), "{row}");
    assert!(!row.contains("[Select]"), "{row}");
    assert!(!all_text(&buf).contains("[Edit]"));
    assert!(!all_text(&buf).contains("Only takes effect"));
}

#[test]
fn user_agent_picker_renders_custom_clear_and_all_upstream_presets() {
    let _lang = use_test_language(Language::English);

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.form = Some(FormState::ProviderAdd(
        crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude),
    ));
    app.overlay = Overlay::UserAgentPicker { selected: 0 };

    let all = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        90,
        24,
    ));
    assert!(all.contains("Select User-Agent"), "{all}");
    assert!(all.contains("Custom..."), "{all}");
    assert!(all.contains("Do not override"), "{all}");
    assert!(all.contains("Presets"), "{all}");
    assert!(!all.contains("? Help"), "{all}");
    for preset in crate::cli::tui::form::USER_AGENT_PRESETS {
        assert!(all.contains(preset), "missing preset {preset:?}: {all}");
    }

    let custom_pos = all.find("Custom...").expect("custom option should render");
    let heading_pos = all.find("Presets").expect("preset heading should render");
    let mut search_from = heading_pos + "Presets".len();
    for preset in crate::cli::tui::form::USER_AGENT_PRESETS {
        let relative_position = all[search_from..]
            .find(preset)
            .unwrap_or_else(|| panic!("missing preset {preset:?}: {all}"));
        search_from += relative_position + preset.len();
    }
    let clear_pos = all
        .find("Do not override")
        .expect("clear option should render");
    assert!(
        custom_pos < heading_pos,
        "custom should precede presets: {all}"
    );
    assert!(
        search_from <= clear_pos,
        "clear option should follow presets: {all}"
    );
}

#[test]
fn provider_api_format_proxy_notice_overlay_uses_single_close_action() {
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
    assert!(
        !all.contains("Esc close"),
        "should not repeat the close action: {all}"
    );
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
fn common_config_notice_overlay_shows_single_close_action() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.overlay = Overlay::Confirm(ConfirmOverlay {
        title: texts::tui_common_config_notice_title().to_string(),
        message: texts::tui_common_config_notice_message(AppType::Claude.as_str()),
        action: ConfirmAction::CommonConfigNotice,
    });

    let data = minimal_data(&app.app_type);
    let buf = render(&app, &data);
    let all = all_text(&buf);

    assert!(
        all.contains("Enter close"),
        "expected Enter close hint: {all}"
    );
    assert!(
        !all.contains("Esc close"),
        "should not show duplicate Esc close hint: {all}"
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
    app.overlay = Overlay::TextView(crate::cli::tui::app::TextViewState {
        title: "Common Snippet".to_string(),
        lines: vec!["{}".to_string()],
        scroll: 0,
        action: None,
    });
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let footer = line_at(&buf, buf.area.height - 1);

    assert!(
        footer.contains("switch app") && footer.contains("/ filter"),
        "expected footer to show global actions; got: {footer:?}"
    );
    assert!(!footer.contains("NAV"), "{footer}");
    assert!(!footer.contains("ACT"), "{footer}");
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
fn copyable_toast_keeps_command_and_action_inside_compact_toast_layout() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Codex));
    app.route = Route::Sessions;
    app.focus = Focus::Content;
    let command = "codex resume session-1";
    app.push_copyable_toast(
        "Could not open a terminal",
        crate::cli::tui::app::ToastKind::Warning,
        command,
    );
    let data = minimal_data(&app.app_type);

    let buf = render(&app, &data);
    let rendered = all_text(&buf);
    assert!(rendered.contains(command), "{rendered}");
    assert!(
        rendered.contains(&format!("c {}", texts::tui_key_copy())),
        "{rendered}"
    );

    let theme = theme_for(&app.app_type);
    let content = super::content_pane_rect(buf.area, &theme);
    let toast = app.toast.as_ref().expect("copyable toast");
    let message = format!(
        "{} {}",
        texts::tui_toast_prefix_warning().trim(),
        toast.message
    );
    let layout_text = super::toast_layout_text(toast, &message);
    let area = super::toast_rect(content, &layout_text);
    assert_eq!(area.height, 6, "copyable toast should stay compact");

    let action_label = format!("c {}", texts::tui_key_copy());
    let action_row = line_index(&rendered, &action_label) as u16;
    let message_row = line_index(&rendered, "Could not open a terminal") as u16;
    let command_row = line_index(&rendered, command) as u16;
    assert_eq!(
        action_row,
        area.y + area.height - 2,
        "toast action should occupy the final row inside the border"
    );
    assert!(
        message_row < command_row && command_row + 1 < action_row,
        "toast action should render below the body with a blank row between:\n{rendered}"
    );

    assert_ne!(
        theme.accent, theme.warn,
        "Codex accent should differ from the warning color"
    );
    let border_cell = &buf[(area.x, area.y + area.height / 2)];
    assert_eq!(border_cell.fg, theme.accent);
    let action_cell = (area.x..area.x.saturating_add(area.width))
        .map(|x| &buf[(x, action_row)])
        .find(|cell| cell.symbol() == "c")
        .expect("copy shortcut cell");
    assert_eq!(action_cell.bg, theme.accent);
}

#[test]
fn copyable_toast_layout_keeps_the_complete_action_text() {
    let mut app = App::new(Some(AppType::Codex));
    let command = format!("codex resume {}", "session-segment-".repeat(12));
    app.push_copyable_toast(
        "Could not open a terminal",
        crate::cli::tui::app::ToastKind::Warning,
        command.clone(),
    );
    let toast = app.toast.as_ref().expect("copyable toast");

    let layout = super::toast_layout_text(toast, "Could not open a terminal");

    assert!(layout.contains(&command), "{layout}");
    assert!(!layout.contains('…'), "{layout}");
}

#[test]
fn copyable_toast_keeps_action_single_row_and_prioritizes_command_when_narrow() {
    let mut app = App::new(Some(AppType::Codex));
    app.push_copyable_toast(
        "Could not open a terminal",
        crate::cli::tui::app::ToastKind::Warning,
        "codex resume session-1",
    );
    let toast = app.toast.as_ref().expect("copyable toast");
    let theme = theme_for(&app.app_type);

    let lines = super::toast_content_lines(
        toast,
        "Could not open a terminal",
        6,
        3,
        &theme,
        Style::default(),
    );
    let text = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(text.len(), 3, "{text:?}");
    assert!(text.iter().all(|line| line.width() <= 6), "{text:?}");
    assert!(text[2].starts_with(" c"), "{text:?}");
    assert!(text[0].contains("codex"), "{text:?}");
    assert!(text[1].is_empty(), "{text:?}");
    assert!(
        !text[..2].join("").contains("Could"),
        "the recovery command should win a constrained body budget: {text:?}"
    );
}

#[test]
fn copyable_toast_is_hidden_while_its_action_is_unavailable() {
    let mut app = App::new(Some(AppType::Codex));
    app.route = Route::Sessions;
    app.push_copyable_toast(
        "Could not open a terminal",
        crate::cli::tui::app::ToastKind::Warning,
        "codex resume session-1",
    );
    app.filter.active = true;
    let data = minimal_data(&app.app_type);

    let rendered = all_text(&render(&app, &data));

    assert!(
        !rendered.contains("Could not open a terminal"),
        "{rendered}"
    );
    assert!(!rendered.contains("codex resume session-1"), "{rendered}");
}

#[test]
fn copyable_toast_is_not_rendered_outside_its_origin_context() {
    let mut app = App::new(Some(AppType::Codex));
    app.route = Route::Sessions;
    let command = "codex resume session-1";
    app.push_copyable_toast(
        "Could not open a terminal",
        crate::cli::tui::app::ToastKind::Warning,
        command,
    );
    app.route = Route::Providers;
    let data = minimal_data(&app.app_type);

    let rendered = all_text(&render(&app, &data));

    assert!(!rendered.contains(command), "{rendered}");
    assert!(
        !rendered.contains("Could not open a terminal"),
        "{rendered}"
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

    // The compact rect must leave room for every line: the last one
    // ("Status: …") used to be clipped by a one-row chrome undercount.
    let status_line = texts::tui_speedtest_line_status("200");
    assert!(
        (0..buf.area.height).any(|y| line_at(&buf, y).contains(status_line.trim())),
        "last compact overlay line should be visible"
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
            ("MODEL".to_string(), json!("P2_ENV_SECRET")),
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
        line_with(&all, "OPENCLAW_ENV_TOKEN").contains("demo-token"),
        "{all}"
    );
    assert!(
        line_with(&all, "OPENCLAW_ENV_MODE").contains("development"),
        "{all}"
    );
    assert!(line_with(&all, "MODEL").contains("P2_ENV_SECRET"), "{all}");
    assert!(!all.contains("[redacted]"), "{all}");
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
fn openclaw_env_route_render_aligns_all_plaintext_values_in_two_columns() {
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
    let token_line = line_with(&content, "OPENCLAW_ENV_TOKEN");
    let mode_line = line_with(&content, "MODE");

    assert_eq!(
        display_column_in_line(token_line, "demo-token"),
        display_column_in_line(mode_line, "development"),
        "{content}"
    );
    assert!(!content.contains("[redacted]"), "{content}");
}

#[test]
fn openclaw_env_route_bounds_large_entries_to_the_visible_terminal_window() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawEnv;
    app.focus = Focus::Content;

    let vars = (0..500)
        .map(|index| {
            (
                format!("KEY_{index:04}_{}", "K".repeat(256)),
                json!(format!("plaintext-{index}-{}", "x".repeat(2_048))),
            )
        })
        .collect();
    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_env = Some(crate::openclaw_config::OpenClawEnvConfig { vars });

    // The smallest render verifies that width/height arithmetic saturates.
    let _ = render_with_size(&app, &data, 12, 8);
    let content = content_text(&app, &render_with_size(&app, &data, 80, 12));

    assert!(content.contains("more entries"), "{content}");
    assert!(content.contains('…'), "{content}");
    assert!(!content.contains("[redacted]"), "{content}");
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
fn openclaw_env_route_render_uses_default_style_for_plaintext_values() {
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
    let token_row_index = line_index(&content, "OPENCLAW_ENV_TOKEN");
    let token_row = line_with(&content, "OPENCLAW_ENV_TOKEN");
    let token_label_col = column_in_line(token_row, "OPENCLAW_ENV_TOKEN");
    let token_col = column_in_line(token_row, "demo-token");
    let token_label_cell = content_cell_at(&app, &buf, token_label_col, token_row_index);
    let token_cell = content_cell_at(&app, &buf, token_col, token_row_index);

    let second_row_index = line_index(&content, "OPENCLAW_ENV_MODE");
    let second_row = line_with(&content, "OPENCLAW_ENV_MODE");
    let second_label_col = column_in_line(second_row, "OPENCLAW_ENV_MODE");
    let second_value_col = column_in_line(second_row, "development");
    let second_label_cell = content_cell_at(&app, &buf, second_label_col, second_row_index);
    let second_value_cell = content_cell_at(&app, &buf, second_value_col, second_row_index);

    assert_eq!(
        cell_style_signature(second_value_cell),
        cell_style_signature(second_label_cell),
        "{content}"
    );
    assert_eq!(
        cell_style_signature(token_cell),
        cell_style_signature(token_label_cell),
        "{content}"
    );
    assert_eq!(
        cell_style_signature(token_cell),
        cell_style_signature(second_value_cell),
        "{content}"
    );
}

#[test]
fn openclaw_env_no_color_keeps_plaintext_values_visible() {
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
    let token_line = line_with(&content, "OPENCLAW_ENV_TOKEN");
    let mode_line = line_with(&content, "OPENCLAW_ENV_MODE");

    assert!(token_line.contains("demo-token"), "{content}");
    assert!(mode_line.contains("development"), "{content}");
    assert!(!content.contains("[redacted]"), "{content}");
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
    assert!(all.contains("Del/Backspace"), "{all}");
    assert!(all.contains(texts::tui_key_edit()), "{all}");
    assert!(all.contains(texts::tui_key_delete()), "{all}");
}

#[test]
fn openclaw_tui_config_routes_show_preserved_fields_in_plaintext() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    for (route, expected_label, expected_value, setup) in [
        (
            Route::ConfigOpenClawTools,
            texts::tui_openclaw_tools_extra_fields_label(),
            "tools-secret",
            json!({
                "demo": {
                    "apiKey": "tools-secret"
                }
            }),
        ),
        (
            Route::ConfigOpenClawAgents,
            texts::tui_openclaw_agents_preserved_fields_label(),
            "Bearer agents-secret",
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
        assert!(all.contains(expected_value), "{all}");
        assert!(!all.contains("[redacted]"), "{all}");
    }
}

#[test]
fn openclaw_tools_and_agents_extreme_preserved_rows_render_in_narrow_terminals() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let huge_multiline = "preserved-plaintext-value\n".repeat(10_000);

    let mut tools_app = App::new(Some(AppType::OpenClaw));
    tools_app.route = Route::ConfigOpenClawTools;
    tools_app.focus = Focus::Content;
    let mut tools_data = minimal_data(&tools_app.app_type);
    tools_data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig {
        profile: Some("coding".to_string()),
        allow: vec![huge_multiline.clone()],
        deny: vec![huge_multiline.clone()],
        extra: std::collections::HashMap::from([(
            "preserved".to_string(),
            json!({ "payload": huge_multiline.clone() }),
        )]),
    });

    let mut agents_app = App::new(Some(AppType::OpenClaw));
    agents_app.route = Route::ConfigOpenClawAgents;
    agents_app.focus = Focus::Content;
    let mut agents_data = minimal_data(&agents_app.app_type);
    agents_data.config.openclaw_agents_defaults =
        Some(crate::openclaw_config::OpenClawAgentsDefaults {
            model: None,
            models: None,
            extra: std::collections::HashMap::from([(
                "preserved".to_string(),
                json!({ "payload": huge_multiline }),
            )]),
        });

    for (app, data) in [(&tools_app, &tools_data), (&agents_app, &agents_data)] {
        for (width, height) in [(32, 7), (40, 10), (64, 12)] {
            let rendered = render_with_size(app, data, width, height);
            assert_eq!(rendered.area.width, width);
            assert_eq!(rendered.area.height, height);
        }
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
fn openclaw_config_extreme_multiline_warnings_keep_narrow_routes_renderable() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let config_path = "/tmp/openclaw/openclaw.json";
    let huge_multiline = "parse warning segment\n\n".repeat(10_000);
    let warnings = (0..40)
        .map(|index| crate::openclaw_config::OpenClawHealthWarning {
            code: "config_parse_failed".to_string(),
            message: format!("warning-{index}\n{huge_multiline}"),
            path: Some(config_path.to_string()),
        })
        .collect::<Vec<_>>();

    for (route, description) in [
        (
            Route::ConfigOpenClawEnv,
            texts::tui_openclaw_config_env_description(),
        ),
        (
            Route::ConfigOpenClawTools,
            texts::tui_openclaw_tools_description(),
        ),
        (
            Route::ConfigOpenClawAgents,
            texts::tui_openclaw_agents_description(),
        ),
    ] {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = route;
        app.focus = Focus::Content;

        let mut data = minimal_data(&app.app_type);
        data.config.openclaw_config_path = Some(std::path::PathBuf::from(config_path));
        data.config.openclaw_warnings = Some(warnings.clone());
        data.config.openclaw_env = Some(crate::openclaw_config::OpenClawEnvConfig::default());
        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig::default());
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults::default());

        for (width, height) in [(32, 7), (40, 10), (64, 12)] {
            let rendered = render_with_size(&app, &data, width, height);
            assert_eq!(rendered.area.width, width);
            assert_eq!(rendered.area.height, height);
        }

        let content = content_text(&app, &render_with_size(&app, &data, 80, 18));
        assert!(
            content.contains(texts::tui_openclaw_config_warning_title()),
            "{content}"
        );
        let description_prefix = description.chars().take(24).collect::<String>();
        assert!(content.contains(&description_prefix), "{content}");
    }
}

#[test]
fn openclaw_config_extreme_path_does_not_block_warning_matching_or_rendering() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let huge_path = std::path::PathBuf::from("p".repeat(2 * 1024 * 1024));
    for (route, description) in [
        (
            Route::ConfigOpenClawEnv,
            texts::tui_openclaw_config_env_description(),
        ),
        (
            Route::ConfigOpenClawTools,
            texts::tui_openclaw_tools_description(),
        ),
        (
            Route::ConfigOpenClawAgents,
            texts::tui_openclaw_agents_description(),
        ),
    ] {
        let mut app = App::new(Some(AppType::OpenClaw));
        app.route = route;
        app.focus = Focus::Content;

        let mut data = minimal_data(&app.app_type);
        data.config.openclaw_config_path = Some(huge_path.clone());
        data.config.openclaw_env = Some(crate::openclaw_config::OpenClawEnvConfig::default());
        data.config.openclaw_tools = Some(crate::openclaw_config::OpenClawToolsConfig::default());
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults::default());

        let content = content_text(&app, &render_with_size(&app, &data, 80, 18));
        let description_prefix = description.chars().take(24).collect::<String>();
        assert!(content.contains(&description_prefix), "{content}");
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
fn openclaw_tools_route_ellipsizes_long_rule_values_in_narrow_width() {
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
    assert!(content.contains("allow.rules.segment"), "{content}");
    assert!(content.contains('…'), "{content}");
    assert!(!content.contains(tail_fragment), "{content}");

    let _no_color = EnvGuard::set("NO_COLOR", "1");
    let no_color_rendered = render_with_size(&app, &data, 72, 18);
    let no_color_content = content_text(&app, &no_color_rendered);
    assert!(
        no_color_content.contains("allow.rules.segment"),
        "{no_color_content}"
    );
    assert!(no_color_content.contains('…'), "{no_color_content}");
    assert!(
        !no_color_content.contains(tail_fragment),
        "{no_color_content}"
    );

    let rule_prefix = buffer_cell_text("allow.rules.segment");
    let rule_row = line_with(&no_color_content, &rule_prefix);
    let rule_row_index = line_index(&no_color_content, &rule_prefix);
    let rule_row_col = display_column_in_line(rule_row, &rule_prefix);
    let rule_cell = content_cell_at(&app, &no_color_rendered, rule_row_col, rule_row_index);
    let add_row = line_with(&no_color_content, &buffer_cell_text("+ Add allow rule"));
    let add_row_index = line_index(&no_color_content, &buffer_cell_text("+ Add allow rule"));
    let add_row_col = display_column_in_line(add_row, &buffer_cell_text("+ Add allow rule"));
    let add_cell = content_cell_at(&app, &no_color_rendered, add_row_col, add_row_index);

    assert!(
        rule_cell.modifier.contains(Modifier::REVERSED),
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
    config_app.filter.input.set("openclaw".to_string());
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
        NavItem::Sessions,
        NavItem::OpenClawWorkspace,
        NavItem::OpenClawEnv,
        NavItem::OpenClawTools,
        NavItem::OpenClawAgents,
        NavItem::Usage,
        NavItem::Config,
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
    assert!(
        !all.contains(&buffer_cell_text(texts::menu_pricing())),
        "{all}"
    );
    assert!(all.contains(&nav_label_text(NavItem::Config)), "{all}");
}

#[test]
fn workspace_non_openclaw_nav_keeps_generic_labels() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let app = App::new(Some(AppType::Claude));
    let all = nav_text(&app, &render(&app, &minimal_data(&app.app_type)));

    let expected = [
        NavItem::Main,
        NavItem::Providers,
        NavItem::Mcp,
        NavItem::Skills,
        NavItem::Sessions,
        NavItem::Prompts,
        NavItem::Usage,
        NavItem::Config,
    ]
    .map(nav_label_text);
    let positions = expected
        .iter()
        .map(|label| all.find(label).expect("generic nav label should render"))
        .collect::<Vec<_>>();

    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{all}");
    assert!(
        !all.contains(&buffer_cell_text(texts::menu_pricing())),
        "{all}"
    );
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
fn workspace_route_extreme_wrapped_heights_render_in_narrow_terminals() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawWorkspace;
    app.focus = Focus::Content;
    app.workspace_idx = 0;

    let huge_component = "workspace-height-segment/".repeat(10_000);
    let huge_preview = "daily-memory-height-segment\n".repeat(10_000);
    let mut data = minimal_data(&app.app_type);
    data.config.openclaw_workspace = OpenClawWorkspaceSnapshot {
        directory_path: std::path::PathBuf::from(format!("/{huge_component}")),
        file_exists: std::collections::HashMap::from([(ALLOWED_FILES[0].to_string(), true)]),
        daily_memory_files: vec![DailyMemoryFileInfo {
            filename: "2026-03-20.md".to_string(),
            date: "2026-03-20".to_string(),
            size_bytes: 12,
            modified_at: 1,
            preview: huge_preview,
        }],
    };

    for (width, height) in [(32, 7), (40, 10), (64, 12), (72, 17)] {
        let rendered = render_with_size(&app, &data, width, height);
        assert_eq!(rendered.area.width, width);
        assert_eq!(rendered.area.height, height);
    }
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

    // Field labels use the theme's strong foreground (was hardcoded white).
    let expected = theme_for(&app.app_type).fg_strong;
    assert_eq!(primary_label_cell.fg, expected);
    assert_eq!(workspace_label_cell.fg, expected);
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
fn openclaw_agents_route_wraps_runtime_notes_and_shows_unknown_preview_values() {
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
    let unknown_line = line_index(&content, "visibilityNote");
    let retained_value_line = line_index(&content, "retained after wrap");

    assert!(warning_tail < workspace_line, "{content}");
    assert_eq!(timeout_line, workspace_line + 1, "{content}");
    assert_eq!(context_line, timeout_line + 1, "{content}");
    assert_eq!(max_line, context_line + 1, "{content}");
    assert!(unknown_line > max_line, "{content}");
    assert!(retained_value_line >= unknown_line, "{content}");
    assert!(!content.contains("[redacted]"), "{content}");
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
    // The workspace name is allowed (and expected) in the breadcrumb title.
    assert!(
        content.contains(&buffer_cell_text(&format!(
            "{} › {}",
            texts::tui_openclaw_workspace_title(),
            texts::tui_openclaw_daily_memory_title()
        ))),
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
    app.filter.input.set("focus".to_string());
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
fn provider_form_model_field_hints_enter_edit_and_f_fetch() {
    let keys =
        super::add_form_key_items(FormFocus::Fields, false, Some(ProviderAddField::CodexModel));
    let enter_label = keys
        .iter()
        .find(|(key, _label)| *key == "Enter")
        .map(|(_key, label)| *label);
    assert_eq!(enter_label, Some(texts::tui_key_edit_mode()));
    assert_eq!(
        keys.iter()
            .find(|(key, _label)| *key == "f")
            .map(|(_key, label)| *label),
        Some(texts::tui_key_fetch_model())
    );
}

#[test]
fn provider_base_url_key_bar_advertises_full_url_shortcut() {
    for field in [
        ProviderAddField::ClaudeBaseUrl,
        ProviderAddField::CodexBaseUrl,
    ] {
        let keys = super::add_form_key_items(FormFocus::Fields, false, Some(field));
        assert_eq!(
            keys.iter()
                .find(|(key, _label)| *key == "f")
                .map(|(_key, label)| *label),
            Some(texts::tui_full_url_label())
        );
    }

    let editing = super::add_form_key_items(
        FormFocus::Fields,
        true,
        Some(ProviderAddField::ClaudeBaseUrl),
    );
    assert!(editing.iter().all(|(key, _label)| *key != "f"));
}

#[test]
fn provider_base_url_key_bar_keeps_full_url_shortcut_visible_when_narrow() {
    let _lock = lock_env();
    let _language = use_test_language(Language::English);
    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
    form.focus = FormFocus::Fields;
    form.field_idx = form
        .fields()
        .iter()
        .position(|field| *field == ProviderAddField::ClaudeBaseUrl)
        .expect("Claude Base URL field");
    app.form = Some(FormState::ProviderAdd(form));

    let rendered = all_text(&render_with_size(
        &app,
        &minimal_data(&app.app_type),
        72,
        24,
    ));
    assert!(
        rendered.contains("f Full URL") || rendered.contains("f=Full URL"),
        "{rendered}"
    );
}

#[test]
fn form_toggle_key_bars_show_enter_without_space() {
    let cases = [
        super::add_form_key_items(
            FormFocus::Fields,
            false,
            Some(ProviderAddField::IncludeCommonConfig),
        ),
        super::quick_config_form_key_items(),
        super::usage_query_form_key_items(
            FormFocus::Fields,
            false,
            Some(UsageQueryField::Enabled),
            false,
        ),
    ];

    for keys in cases {
        assert!(
            keys.iter()
                .any(|(key, label)| { *key == "Enter" && *label == texts::tui_key_toggle() }),
            "missing Enter toggle hint: {keys:?}"
        );
        assert!(
            keys.iter().all(|(key, _)| *key != "Space"),
            "form toggle should not advertise Space: {keys:?}"
        );
    }
}

#[test]
fn openclaw_provider_list_key_bar_shows_test_hint_only() {
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

    assert!(all.contains("t test"));
    assert!(!all.contains("speedtest"));
    assert!(!all.contains("stream check"));
}

#[test]
fn provider_test_menu_renders_supported_test_actions() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.overlay = Overlay::ProviderTestMenu {
        provider_id: "p1".to_string(),
        selected: 0,
    };
    let data = minimal_data(&app.app_type);

    let all = all_text(&render(&app, &data));

    assert!(all.contains("Test"), "{all}");
    assert!(all.contains("speedtest"), "{all}");
    assert!(all.contains("stream check"), "{all}");
}

#[test]
fn openclaw_provider_test_menu_hides_stream_check() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.overlay = Overlay::ProviderTestMenu {
        provider_id: "p1".to_string(),
        selected: 0,
    };
    let data = minimal_data(&app.app_type);

    let all = all_text(&render(&app, &data));

    assert!(all.contains("speedtest"), "{all}");
    assert!(!all.contains("stream check"), "{all}");
}

#[test]
fn openclaw_provider_list_key_bar_uses_common_provider_actions() {
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

    assert!(all.contains("Space add/remove"), "{all}");
    assert!(all.contains("c copy"), "{all}");
    assert!(all.contains("t test"), "{all}");
    assert!(all.contains("x set default"), "{all}");
    assert!(!all.contains("s add/remove"), "{all}");
}

#[test]
fn failover_provider_list_key_bar_hides_switch_while_queue_controls_routing() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut data = minimal_data(&app.app_type);

    let disabled_text = all_text(&render_with_size(&app, &data, 180, 40));
    let disabled_keys = line_with(&disabled_text, "manage failover");
    assert!(disabled_keys.contains("Space"), "{disabled_keys}");
    assert!(!disabled_keys.contains("</>"), "{disabled_keys}");

    data.proxy.auto_failover_enabled = true;
    let enabled_text = all_text(&render_with_size(&app, &data, 180, 40));
    let enabled_keys = line_with(&enabled_text, "manage failover");
    assert!(!enabled_keys.contains("Space"), "{enabled_keys}");
    assert!(!enabled_keys.contains("</>"), "{enabled_keys}");
}

#[test]
fn failover_provider_list_marks_queue_entries_when_enabled() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut data = minimal_data(&app.app_type);
    data.proxy.auto_failover_enabled = true;
    data.providers.current_id = "current".to_string();
    data.providers.rows = vec![
        failover_provider_row("current", "Current Provider", true, false, None),
        failover_provider_row("queued", "Queued Provider", false, true, Some(1)),
    ];

    let buf = render(&app, &data);
    let current_line = (0..buf.area.height)
        .map(|y| line_at(&buf, y))
        .find(|line| line.contains("Current Provider") && line.contains("https://example.com"))
        .expect("current provider row rendered");
    let queued_line = (0..buf.area.height)
        .map(|y| line_at(&buf, y))
        .find(|line| line.contains("Queued Provider") && line.contains("https://example.com"))
        .expect("queued provider row rendered");

    assert!(!current_line.contains("#"), "{current_line}");
    assert!(queued_line.contains("#1"), "{queued_line}");
}

#[test]
fn failover_provider_list_uses_current_marker_when_disabled() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut data = minimal_data(&app.app_type);
    data.proxy.auto_failover_enabled = false;
    data.providers.current_id = "current".to_string();
    data.providers.rows = vec![
        failover_provider_row("current", "Current Provider", true, false, None),
        failover_provider_row("queued", "Queued Provider", false, true, Some(1)),
    ];

    let buf = render(&app, &data);
    let current_line = (0..buf.area.height)
        .map(|y| line_at(&buf, y))
        .find(|line| line.contains("Current Provider") && line.contains("https://example.com"))
        .expect("current provider row rendered");

    assert!(
        current_line.contains(texts::tui_marker_active()),
        "{current_line}"
    );
}

#[test]
fn failover_queue_overlay_renders_enabled_state_and_toggle_hint() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.overlay = Overlay::FailoverQueueManager {
        selected_provider_id: Some("p1".to_string()),
    };
    let mut data = minimal_data(&app.app_type);
    data.proxy.auto_failover_enabled = true;

    let all = all_text(&render(&app, &data));

    assert!(all.contains("Auto: on"), "{all}");
    assert!(all.contains("f auto failover"), "{all}");
    assert!(all.contains("Enter add/remove"), "{all}");
    assert!(all.contains("Ctrl+↑↓ move"), "{all}");
    assert!(!all.contains("K/J move"), "{all}");
    assert!(
        all.lines().any(|line| line.contains("Demo Provider")
            && line.contains("not queued")
            && !line.contains("https://example.com")),
        "{all}"
    );
}

#[test]
fn failover_queue_overlay_renders_disabled_state_and_toggle_hint() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.overlay = Overlay::FailoverQueueManager {
        selected_provider_id: Some("p1".to_string()),
    };
    let mut data = minimal_data(&app.app_type);
    data.proxy.auto_failover_enabled = false;

    let all = all_text(&render(&app, &data));

    assert!(all.contains("Auto: off"), "{all}");
    assert!(all.contains("f auto failover"), "{all}");
    assert!(all.contains("not queued"), "{all}");
}

#[test]
fn failover_queue_overlay_renders_active_target_and_honest_health_states() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.overlay = Overlay::FailoverQueueManager {
        selected_provider_id: Some("active".to_string()),
    };
    let mut data = minimal_data(&app.app_type);
    data.proxy.auto_failover_enabled = true;
    data.providers.rows = vec![
        failover_provider_row("active", "Active Provider", false, true, Some(0)),
        failover_provider_row("unknown", "Unknown Provider", false, true, Some(1)),
        failover_provider_row("normal", "Normal Provider", false, true, Some(2)),
        failover_provider_row("warning", "Warning Provider", false, true, Some(3)),
        failover_provider_row("unhealthy", "Unhealthy Provider", false, true, Some(4)),
        failover_provider_row("unused", "Unused Provider", false, false, None),
    ];
    data.proxy.current_app_target = Some(ProxyTargetSnapshot {
        provider_id: "active".to_string(),
        provider_name: "Stale Display Name".to_string(),
    });
    std::sync::Arc::make_mut(&mut data.proxy.provider_health).insert(
        "normal".to_string(),
        ProviderHealthSnapshot {
            is_healthy: true,
            consecutive_failures: 0,
        },
    );
    std::sync::Arc::make_mut(&mut data.proxy.provider_health).insert(
        "warning".to_string(),
        ProviderHealthSnapshot {
            is_healthy: true,
            consecutive_failures: 2,
        },
    );
    std::sync::Arc::make_mut(&mut data.proxy.provider_health).insert(
        "unhealthy".to_string(),
        ProviderHealthSnapshot {
            is_healthy: false,
            consecutive_failures: 4,
        },
    );

    let all = all_text(&render_with_size(&app, &data, 140, 36));

    assert!(all.contains("Active target: P1 Active Provider"), "{all}");
    assert!(all.contains("target"), "{all}");
    assert!(all.contains("no record"), "{all}");
    assert!(all.contains("normal"), "{all}");
    assert!(all.contains("failures (2)"), "{all}");
    assert!(all.contains("unhealthy (4)"), "{all}");
    assert!(all.contains("not queued"), "{all}");
}

#[test]
fn failover_queue_overlay_keeps_priority_name_and_status_on_narrow_terminals() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    app.overlay = Overlay::FailoverQueueManager {
        selected_provider_id: Some("p1".to_string()),
    };
    let mut data = minimal_data(&app.app_type);
    data.providers.rows[0].provider.in_failover_queue = true;
    data.providers.rows[0].provider.sort_index = Some(0);
    std::sync::Arc::make_mut(&mut data.proxy.provider_health).insert(
        "p1".to_string(),
        ProviderHealthSnapshot {
            is_healthy: false,
            consecutive_failures: 4,
        },
    );

    let all = all_text(&render_with_size(&app, &data, 64, 22));

    assert!(all.contains("P1"), "{all}");
    assert!(all.contains("Demo"), "{all}");
    assert!(all.contains("unhealthy"), "{all}");
    assert!(
        all.lines().any(|line| line.contains("Demo")
            && line.contains("unhealthy")
            && !line.contains("https://example.com")),
        "{all}"
    );
}

#[test]
fn opencode_provider_list_key_bar_uses_config_membership_actions() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenCode));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let data = minimal_data(&app.app_type);

    let all = all_text(&render(&app, &data));

    assert!(all.contains("Space add/remove"), "{all}");
    assert!(all.contains("t test"), "{all}");
    assert!(!all.contains("s add/remove"), "{all}");
    assert!(!all.contains("c stream check"), "{all}");
    assert!(!all.contains("x set default"), "{all}");
}

#[test]
fn opencode_provider_list_marks_rows_in_config_without_current_marker() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenCode));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut data = minimal_data(&app.app_type);
    data.providers.rows[0].is_in_config = true;
    data.providers.rows[0].is_current = false;

    let buf = render(&app, &data);
    let provider_line = (0..buf.area.height)
        .map(|y| line_at(&buf, y))
        .find(|line| line.contains("Demo Provider"))
        .expect("provider row rendered");

    assert!(provider_line.contains("+"), "{provider_line}");
    assert!(!provider_line.contains("*"), "{provider_line}");
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

    assert!(all.contains("Enter edit"), "{all}");
    assert!(!all.contains("e edit"), "{all}");
    assert!(all.contains("x set default"), "{all}");
}

#[test]
fn hermes_provider_list_key_bar_hides_edit_delete_for_read_only_provider() {
    let _lock = lock_env();
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Hermes));
    app.route = Route::Providers;
    app.focus = Focus::Content;
    let mut data = minimal_data(&app.app_type);
    data.providers.rows[0].provider.settings_config = json!({
        "_cc_source": crate::hermes_config::PROVIDER_SOURCE_DICT,
        "base_url": "https://example.com",
        "models": [{"id": "main"}]
    });

    let all = all_text(&render(&app, &data));

    let keys = line_with(&all, "Space add/remove");
    assert!(keys.contains("t test"), "{keys}");
    assert!(!keys.contains("Enter edit"), "{keys}");
    assert!(!keys.contains("e edit"), "{keys}");
    assert!(!keys.contains("d delete"), "{keys}");
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
fn openclaw_tui_provider_search_uses_saved_name_not_model_name() {
    let mut app = App::new(Some(AppType::OpenClaw));
    app.filter.input.set("live model".to_string());

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

    app.filter.input.set("saved snapshot".to_string());
    assert_eq!(super::provider_rows_filtered(&app, &data).len(), 1);
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

    assert!(compact.contains("Space添加/移除"), "{all}");
    assert!(compact.contains("t测试"), "{all}");
    assert!(compact.contains("x设为默认"), "{all}");
    assert!(!compact.contains("Space切换"), "{all}");
    assert!(!compact.contains("s添加/移除"), "{all}");
    assert!(!all.contains("add/remove"), "{all}");
    assert!(!all.contains("set default"), "{all}");
}

#[test]
fn openclaw_tools_render_windows_ten_thousand_rules_without_mutating_full_values() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawTools;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::app::OpenClawToolsFormState::from_snapshot(None);
    form.profile = Some("coding".to_string());
    form.allow = (0..10_000)
        .map(|index| format!("allow-{index:05}"))
        .collect();
    form.deny = (0..10_000)
        .map(|index| format!("deny-{index:05}"))
        .collect();
    form.extra
        .insert("large".to_string(), json!("x".repeat(1_000_000)));
    form.section = crate::cli::tui::app::OpenClawToolsSection::Deny;
    form.row = 9_999;
    app.openclaw_tools_form = Some(form);

    let data = minimal_data(&app.app_type);
    let content = content_text(&app, &render_with_size(&app, &data, 72, 15));

    assert!(content.contains("deny-09999"), "{content}");
    assert!(!content.contains("deny-00000"), "{content}");
    let form = app
        .openclaw_tools_form
        .as_ref()
        .expect("tools form retained");
    assert_eq!(form.allow.len(), 10_000);
    assert_eq!(form.deny.len(), 10_000);
    assert_eq!(
        form.extra
            .get("large")
            .and_then(Value::as_str)
            .map(str::len),
        Some(1_000_000)
    );
}

#[test]
fn openclaw_agents_render_windows_ten_thousand_fallbacks_and_borrows_huge_extra() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::OpenClaw));
    app.route = Route::ConfigOpenClawAgents;
    app.focus = Focus::Content;
    let mut form = crate::cli::tui::app::OpenClawAgentsFormState::from_snapshot(None);
    form.primary_model = "provider/primary".to_string();
    form.fallbacks = (0..10_000)
        .map(|index| format!("fallback-{index:05}"))
        .collect();
    form.model_extra.insert(
        "large-model-extra".to_string(),
        json!("m".repeat(1_000_000)),
    );
    form.defaults_extra.insert(
        "large-default-extra".to_string(),
        json!("d".repeat(1_000_000)),
    );
    form.section = crate::cli::tui::app::OpenClawAgentsSection::FallbackModels;
    form.row = 9_999;
    app.openclaw_agents_form = Some(form);

    let data = minimal_data(&app.app_type);
    let content = content_text(&app, &render_with_size(&app, &data, 80, 15));

    assert!(content.contains("fallback-09999"), "{content}");
    assert!(!content.contains("fallback-00000"), "{content}");
    let form = app
        .openclaw_agents_form
        .as_ref()
        .expect("agents form retained");
    assert_eq!(form.fallbacks.len(), 10_000);
    assert_eq!(
        form.model_extra
            .get("large-model-extra")
            .and_then(Value::as_str)
            .map(str::len),
        Some(1_000_000)
    );
    assert_eq!(
        form.defaults_extra
            .get("large-default-extra")
            .and_then(Value::as_str)
            .map(str::len),
        Some(1_000_000)
    );
}

// ---------------------------------------------------------------------------
// Home usage chart
// ---------------------------------------------------------------------------

/// A 30-day axis with the supplied models spread over the three most recent
/// days. Each entry is `(model, billable tokens/day, USD/day)`; both scale with
/// the day multiplier, so the ranking is the same on every day of the window.
///
/// Costs are deliberately *not* proportional to tokens: the card ranks by cost
/// and sizes bars by real tokens, and a fixture where the two agree could not
/// tell a correct implementation from one that confuses them.
fn usage_with_daily_models(models: &[(&str, u64, f64)]) -> UsageSnapshot {
    use chrono::{Days, Local};

    let today = Local::now().date_naive();
    let start = today.checked_sub_days(Days::new(29)).unwrap_or(today);
    let trends_30d = (0..30)
        .map(|offset| {
            let date = start.checked_add_days(Days::new(offset)).unwrap_or(start);
            UsageTrendBucket {
                key: date.format("%Y-%m-%d").to_string(),
                label: date.format("%m/%d").to_string(),
                ..UsageTrendBucket::default()
            }
        })
        .collect::<Vec<_>>();

    let mut daily_models = Vec::new();
    for (offset, bucket) in trends_30d.iter().rev().take(3).enumerate() {
        let multiplier = offset as u64 + 1;
        for (model, tokens, cost) in models {
            let day_tokens = tokens.saturating_mul(multiplier);
            daily_models.push(UsageDailyModelBucket {
                date_key: bucket.key.clone(),
                model: (*model).to_string(),
                is_other: false,
                total_tokens: day_tokens,
                total_cost_usd: cost * multiplier as f64,
                // A third input, two thirds output, with cache traffic an order
                // of magnitude larger — the shape a real Claude day has.
                input_tokens: day_tokens / 3,
                output_tokens: day_tokens - day_tokens / 3,
                cache_read_tokens: day_tokens.saturating_mul(20),
                cache_creation_tokens: day_tokens.saturating_mul(4),
            });
        }
    }

    UsageSnapshot {
        trends_30d,
        daily_models,
        last_synced_at: Some(chrono::Local::now().timestamp() - 7_200),
        ..UsageSnapshot::default()
    }
}

/// The standard five-model fixture: `claude-opus` is small in tokens and by far
/// the most expensive, so a token-ranked implementation would fold it into
/// "Other" and a cost-ranked one puts it first.
const HOME_CHART_MODELS: [(&str, u64, f64); 5] = [
    ("claude-opus", 1_000, 6.0),
    ("claude-sonnet", 5_000, 3.0),
    ("claude-haiku", 4_000, 1.5),
    ("gpt-5.4", 3_000, 0.75),
    ("mystery-model", 2_000, 0.25),
];

fn row_of(buf: &Buffer, needle: &str) -> Option<u16> {
    (0..buf.area.height).find(|y| line_at(buf, *y).contains(needle))
}

/// Ratatui draws box-drawing rails in every icon mode, so a whole rendered row
/// is never ASCII. ASCII-mode spinner assertions check the braille block
/// instead: no `⠋⠙⠹⠸` may survive the downgrade.
fn contains_braille(text: &str) -> bool {
    text.chars().any(|c| ('\u{2800}'..='\u{28FF}').contains(&c))
}

/// Card title as it renders for the active icon mode.
fn usage_card_title() -> String {
    if crate::cli::tui::icons::use_emoji() {
        "Usage · 30d".to_string()
    } else {
        "Usage - 30d".to_string()
    }
}

/// The Chinese card title as the test backend lays it out: ratatui pads every
/// double-width grapheme with a blank cell.
fn zh_usage_card_title() -> String {
    let separator = if crate::cli::tui::icons::use_emoji() {
        " · "
    } else {
        " - "
    };
    buffer_cell_text(&format!("用量{separator}近 30 天"))
}

/// Text strictly inside the home usage card, both rails excluded. Ratatui
/// draws box-drawing borders in every icon mode, so ASCII assertions have to
/// look at the card body rather than the whole region.
fn usage_card_inner_text(buf: &Buffer) -> String {
    usage_card_inner_text_with_title(buf, &usage_card_title())
}

fn usage_card_inner_text_with_title(buf: &Buffer, title: &str) -> String {
    let top = row_of(buf, title).expect("usage card title row");
    let left = (0..buf.area.width)
        .find(|x| buf[(*x, top)].symbol() == "┌")
        .expect("usage card left rail");
    let right = ((left + 1)..buf.area.width)
        .find(|x| buf[(*x, top)].symbol() == "┐")
        .expect("usage card right rail");
    let bottom = ((top + 1)..buf.area.height)
        .find(|y| buf[(left, *y)].symbol() == "└")
        .expect("usage card bottom rail");

    let mut out = String::new();
    for y in (top + 1)..bottom {
        for x in (left + 1)..right {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[test]
fn home_usage_chart_renders_title_bars_and_legend() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.usage = usage_with_daily_models(&HOME_CHART_MODELS);

    let buf = render(&app, &data);

    // The card chrome carries the title and the status, like its siblings.
    let title_row = line_at(&buf, row_of(&buf, "Usage · 30d").expect("card title row"));
    assert!(title_row.contains("┌ Usage · 30d ─"), "{title_row}");
    assert!(title_row.contains("Last updated: 2h ago"), "{title_row}");

    let card = usage_card_inner_text(&buf);
    assert!(card.contains('█'), "bars should render:\n{card}");

    // Breathing room: a blank row under the title rail and a blank column on
    // both sides of every content row.
    let rows = card.lines().collect::<Vec<_>>();
    assert!(
        rows[0].trim().is_empty(),
        "the card opens with a blank row:\n{card}"
    );
    assert!(
        rows.iter()
            .all(|row| row.starts_with(' ') && row.ends_with(' ')),
        "content never touches the side rails:\n{card}"
    );

    // 120 columns is wide enough for the models column.
    assert!(card.contains("Models by Cost"), "{card}");
    assert!(card.contains("● claude-opus"), "{card}");
    assert!(
        card.contains("● Other"),
        "the list needs a residual bucket:\n{card}"
    );
    let opus_row = card
        .lines()
        .find(|line| line.contains("● claude-opus"))
        .expect("a model row");
    assert!(opus_row.contains('%'), "{opus_row}");
    assert!(opus_row.contains('$'), "{opus_row}");

    // Near the split floor the expanded list yields enough room to keep the
    // chart readable, remaining under the detail line's floor: the
    // rows stay one-liners.
    assert!(!card.contains("In: "), "{card}");
}

#[test]
fn home_usage_chart_lists_the_token_breakdown_under_each_model() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.usage = usage_with_daily_models(&HOME_CHART_MODELS);

    // 160 columns leaves the list its revised 59-column cap: ample detail room.
    let card = usage_card_inner_text(&render_with_size(&app, &data, 160, 45));

    let opus = card
        .lines()
        .position(|line| line.contains("● claude-opus"))
        .expect("the opus row");
    let detail = card.lines().nth(opus + 1).expect("its detail line");
    for label in ["In: ", "Out: ", "CR: ", "CW: "] {
        assert!(detail.contains(label), "{label} missing in {detail:?}");
    }
    assert!(detail.contains('•'), "{detail:?}");
    // Indented under the name and confined to the list column: everything left
    // of the list rule still belongs to the chart.
    let (chart_half, list_half) = detail.rsplit_once('│').expect("the list rule");
    assert!(!chart_half.contains("In: "), "{detail:?}");
    assert!(
        list_half.starts_with("    In: "),
        "the detail line hangs under the model name: {detail:?}"
    );

    // The residual bucket aggregates its folded models, so it has a line too.
    let other = card
        .lines()
        .position(|line| line.contains("● Other"))
        .expect("the residual row");
    assert!(
        card.lines()
            .nth(other + 1)
            .is_some_and(|line| line.contains("In: ") && line.contains("CW: ")),
        "the residual row carries a detail line too:\n{card}"
    );
}

#[test]
fn home_usage_chart_drops_only_the_detail_lines_when_the_list_is_short() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.usage = usage_with_daily_models(&HOME_CHART_MODELS);

    // Wide enough for the detail line, one card row too short for
    // the header plus two rows per model. Main no longer reserves a blank row
    // above the connection card, so the terminal fixture is one row shorter.
    let card = usage_card_inner_text(&render_with_size(&app, &data, 160, 32));

    assert!(card.contains("Models by Cost"), "{card}");
    for model in ["● claude-opus", "● gpt-5.4", "● Other"] {
        assert!(card.contains(model), "{model} missing:\n{card}");
    }
    assert!(
        !card.contains("In: "),
        "the names survive, the detail lines go:\n{card}"
    );
}

#[test]
fn home_usage_chart_bars_span_the_whole_card_width() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.usage = usage_with_daily_models(&[("claude-opus", 5_000, 6.0)]);

    let buf = render_with_size(&app, &data, 160, 45);
    let card = usage_card_inner_text(&buf);
    let axis = card
        .lines()
        .find(|line| line.contains('└'))
        .expect("the chart axis");
    // y label column + the axis corner + one column per body cell, and the
    // whole chart column ends where the list rule starts.
    let rule = axis.chars().filter(|ch| *ch == '│').count();
    assert_eq!(rule, 1, "the list rule spans the card height:\n{card}");
    let drawn = axis.chars().filter(|ch| *ch == '─').count();
    assert!(
        drawn >= 58,
        "a 160-column terminal should leave the revised-list chart ~60 columns, got {drawn}:\n{card}"
    );
}

#[test]
fn home_usage_chart_shows_the_live_badge_while_the_proxy_routes_this_app() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.usage = usage_with_daily_models(&[("claude-opus", 5_000, 6.0)]);
    data.proxy.running = true;
    data.proxy.claude_takeover = true;

    let all = all_text(&render(&app, &data));

    assert!(all.contains("• live"), "{all}");
    assert!(!all.contains("Last updated"), "{all}");
}

#[test]
fn home_usage_chart_shows_an_empty_hint_per_app_family() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;
    let data = minimal_data(&app.app_type);
    let all = all_text(&render(&app, &data));
    assert!(all.contains("No usage yet - first sync pending"), "{all}");

    let mut openclaw = App::new(Some(AppType::OpenClaw));
    openclaw.route = Route::Main;
    openclaw.focus = Focus::Content;
    let openclaw_data = minimal_data(&openclaw.app_type);
    let openclaw_text = all_text(&render(&openclaw, &openclaw_data));
    assert!(
        openclaw_text.contains("only records proxy traffic"),
        "{openclaw_text}"
    );
}

#[test]
fn home_usage_chart_spins_while_the_first_aggregate_loads() {
    let _lock = lock_env();
    let _icons_lock = lock_test_home_and_settings();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _emoji = EnvGuard::set("CC_SWITCH_ICONS", "emoji");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;
    app.tick = 3;
    app.usage
        .start_loading(AppType::Claude, UsageRangePreset::ThirtyDays);

    let data = minimal_data(&app.app_type);
    let all = all_text(&render(&app, &data));

    assert!(
        all.contains("⠸ Refreshing"),
        "the first aggregate uses the shared refresh indicator:\n{all}"
    );
}

#[test]
fn home_usage_chart_degrades_on_small_terminals_without_panicking() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;
    let mut data = minimal_data(&app.app_type);
    data.usage =
        usage_with_daily_models(&[("claude-opus", 5_000, 6.0), ("claude-haiku", 1_000, 1.0)]);

    // 80x23 leaves the card two body rows: the top pad plus one content row.
    // The list header survives; the graph disappears first.
    let small_buf = render_with_size(&app, &data, 80, 23);
    let small = all_text(&small_buf);
    assert!(small.contains("Usage · 30d"), "{small}");
    assert!(small.contains("Models by Cost"), "{small}");
    let small_card = usage_card_inner_text(&small_buf);
    assert_eq!(small_card.lines().count(), 2, "{small_card}");
    assert!(
        small_card
            .lines()
            .next()
            .is_some_and(|row| row.trim().is_empty()),
        "the card opens with its blank pad row:\n{small_card:?}"
    );
    assert!(
        !small_card.contains('█') && !small_card.contains('└'),
        "the compact card must not draw a monochrome graph:\n{small_card:?}"
    );

    // Extra height reveals the model rows and detail lines, but horizontal
    // space still belongs to the list rather than a chart-only fallback.
    let tall_buf = render_with_size(&app, &data, 80, 40);
    let tall = all_text(&tall_buf);
    assert!(tall.contains("Models by Cost"), "{tall}");
    let tall_card = usage_card_inner_text(&tall_buf);
    assert!(tall_card.contains("● claude-opus"), "{tall_card}");
    assert!(
        !tall_card.contains('█') && !tall_card.contains('└'),
        "narrow terminals keep the list and omit every graph form:\n{tall_card}"
    );

    // Anything smaller must still render without panicking.
    for (width, height) in [(60u16, 20u16), (40, 14), (30, 10), (20, 8)] {
        let buf = render_with_size(&app, &data, width, height);
        assert_eq!(buf.area.width, width);
    }
}

#[test]
fn home_usage_chart_keeps_the_chart_region_ascii_in_ascii_icon_mode() {
    let _lock = lock_env();
    let _icons_lock = lock_test_home_and_settings();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _ascii = EnvGuard::set("CC_SWITCH_ICONS", "ascii");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;
    app.tick = 5;

    let mut data = minimal_data(&app.app_type);
    data.usage = usage_with_daily_models(&[
        ("claude-opus", 5_000, 6.0),
        ("claude-sonnet", 4_000, 3.0),
        ("claude-haiku", 3_000, 1.5),
        (
            "a-very-long-model-identifier-that-needs-truncating",
            2_500,
            0.75,
        ),
        ("gpt-5.4", 2_000, 0.25),
    ]);

    // A wide terminal so the assertion covers the models column too.
    let buf = render_with_size(&app, &data, 160, 45);
    let chart = usage_card_inner_text(&buf);

    assert!(
        chart.is_ascii(),
        "ascii icon mode must not leak unicode glyphs:\n{chart}"
    );
    assert!(chart.contains('#'), "ascii bars should render:\n{chart}");
    assert!(chart.contains("Models by Cost"), "{chart}");
    // Model rows of the list column: `<glyph> <name> <share> <cost>`.
    let list_rows = chart
        .lines()
        .filter_map(|line| line.rsplit_once('|').map(|(_, list)| list))
        .filter_map(|list| {
            let rest = list.strip_prefix(' ')?;
            let mut chars = rest.chars();
            let glyph = chars.next()?;
            // A dot is followed by a space; headers and detail lines are not.
            (chars.next() == Some(' ') && !glyph.is_whitespace())
                .then(|| (glyph, rest[1..].trim().to_string()))
        })
        .collect::<Vec<_>>();

    // Cost ranks the list, and claude-opus is the costliest here despite
    // carrying the fewest tokens.
    assert!(
        list_rows
            .first()
            .is_some_and(|(_, text)| text.starts_with("claude-opus")),
        "the costliest model leads the list, got {list_rows:?}:\n{chart}"
    );

    // Without color the dot is the only thing telling two series apart, so
    // every slot has to draw its own glyph.
    let list_dots = list_rows
        .iter()
        .map(|(glyph, _)| *glyph)
        .collect::<Vec<_>>();
    let distinct = list_dots
        .iter()
        .collect::<std::collections::HashSet<_>>()
        .len();
    assert!(
        distinct >= 2,
        "ascii mode must not collapse every series onto one dot, got {list_dots:?}:\n{chart}"
    );
    assert!(
        list_dots.iter().all(char::is_ascii),
        "{list_dots:?} must stay ascii"
    );

    let long_row = chart
        .lines()
        .find(|line| line.contains("a-very-long-model-identifier-that-"))
        .expect("the long model row");
    assert!(
        long_row.contains('~'),
        "long names use the ascii ellipsis:\n{long_row}"
    );
    // The detail line falls back to the ascii separator.
    let detail = chart
        .lines()
        .find(|line| line.contains("In: "))
        .expect("a detail line");
    assert!(detail.contains(" - Out: "), "{detail:?}");
    assert!(detail.contains(" - CW: "), "{detail:?}");

    // The loading spinner shares the gate.
    let mut loading = App::new(Some(AppType::Claude));
    loading.route = Route::Main;
    loading.focus = Focus::Content;
    loading.tick = 5;
    loading
        .usage
        .start_loading(AppType::Claude, UsageRangePreset::ThirtyDays);
    let loading_buf = render_with_size(&loading, &minimal_data(&loading.app_type), 160, 45);
    let loading_region = usage_card_inner_text(&loading_buf);
    assert!(
        loading_region.contains("/ Refreshing"),
        "the ascii body uses the shared refresh indicator:\n{loading_region}"
    );
    assert!(
        loading_region.is_ascii(),
        "ascii spinner must stay ascii:\n{loading_region}"
    );
}

#[test]
fn home_connection_card_carries_the_webdav_status_line() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.config.webdav_sync = Some(crate::settings::WebDavSyncSettings {
        enabled: true,
        base_url: "https://dav.example.com".to_string(),
        remote_root: "cc-switch-sync".to_string(),
        profile: "default".to_string(),
        username: "user".to_string(),
        password: "secret".to_string(),
        auto_sync: false,
        status: crate::settings::WebDavSyncStatus {
            last_sync_at: Some(1_700_000_000),
            ..crate::settings::WebDavSyncStatus::default()
        },
    });

    let buf = render(&app, &data);
    let all = all_text(&buf);
    let webdav_line = all
        .lines()
        .find(|line| line.contains("WebDAV Sync"))
        .expect("the connection card should carry a WebDAV line");

    assert!(webdav_line.contains('✓'), "{webdav_line}");
    assert!(webdav_line.contains("OK"), "{webdav_line}");
    assert!(webdav_line.contains("2023/11"), "{webdav_line}");
    // The standalone card is gone: no second WebDAV section title.
    assert_eq!(all.matches("WebDAV Sync").count(), 1, "{all}");
}

/// The connection card is sized from its line *count*, so it must not wrap:
/// ratatui word-wraps, and any estimate of that would eventually clip the last
/// line — the WebDAV one — out of the card.
#[test]
fn home_connection_card_truncates_instead_of_wrapping_on_narrow_terminals() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows[0].is_current = true;
    data.providers.rows[0].api_url = Some(format!(
        "https://{}.example.com/v1",
        "long-host-".repeat(12)
    ));

    let buf = render_with_size(&app, &data, 90, 30);
    let all = all_text(&buf);

    let api_line = all
        .lines()
        .find(|line| line.contains("API URL"))
        .expect("the API URL row");
    assert!(
        api_line.contains('…'),
        "the oversized URL is clipped, not wrapped:\n{api_line}"
    );
    assert!(
        !all.contains("long-host-long-host-long-host-long-host-"),
        "no second row carries the overflow:\n{all}"
    );
    // Every card line is still on screen, the WebDAV one included. The labels
    // are padded to a fixed column, which also keeps the header's own
    // "Provider:" badge out of the count.
    for label in ["Provider      :", "API URL       :", "WebDAV Sync   :"] {
        assert_eq!(
            all.lines().filter(|line| line.contains(label)).count(),
            1,
            "{label} must appear exactly once:\n{all}"
        );
    }
}

#[test]
fn home_keeps_webdav_visible_below_a_long_quota_on_narrow_terminals() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;
    let mut data = minimal_data(&app.app_type);
    data.providers.rows[0].is_current = true;
    data.providers.rows[0].provider.settings_config =
        json!({"env": {"ANTHROPIC_BASE_URL": "https://relay.example.test"}});
    data.providers.rows[0].provider.meta = Some(crate::provider::ProviderMeta {
        usage_script: Some(crate::provider::UsageScript {
            enabled: true,
            language: "javascript".to_string(),
            code: String::new(),
            timeout: None,
            api_key: None,
            base_url: None,
            access_token: None,
            user_id: None,
            template_type: Some("general".to_string()),
            auto_query_interval: None,
            coding_plan_provider: None,
        }),
        ..crate::provider::ProviderMeta::default()
    });
    let target =
        crate::cli::tui::data::quota_target_for_provider(&app.app_type, &data.providers.rows[0])
            .expect("usage script provides a quota target");
    data.quota.finish(
        target,
        crate::cli::provider_quota::ProviderUsageQuota::Script(crate::provider::UsageResult {
            success: true,
            data: Some(vec![crate::provider::UsageData {
                plan_name: Some("an-extremely-long-provider-controlled-plan-name".to_string()),
                extra: None,
                is_valid: Some(true),
                invalid_message: None,
                total: Some(100.0),
                used: Some(42.0),
                remaining: Some(58.0),
                unit: Some("%".to_string()),
            }]),
            error: None,
        }),
    );
    let mut webdav = crate::settings::WebDavSyncSettings {
        enabled: true,
        base_url: "https://dav.example.com".to_string(),
        remote_root: "cc-switch-sync".to_string(),
        profile: "default".to_string(),
        username: "user".to_string(),
        password: "secret".to_string(),
        auto_sync: false,
        status: crate::settings::WebDavSyncStatus::default(),
    };
    webdav.status.last_error = Some("auth failed".to_string());
    data.config.webdav_sync = Some(webdav);

    let all = all_text(&render_with_size(&app, &data, 70, 30));
    let quota_row = all
        .lines()
        .find(|line| line.contains("Quota"))
        .expect("quota row");
    let webdav_row = all
        .lines()
        .find(|line| line.contains("WebDAV Sync"))
        .expect("WebDAV keeps its own row");

    assert!(quota_row.contains("an-extremely"), "{quota_row}");
    assert!(webdav_row.contains("Error"), "{webdav_row}");
    assert!(webdav_row.contains("auth failed"), "{webdav_row}");
}

/// A pathological value must clip, not drive the layout through a wrapped
/// `u16`. In a debug build the old `line.width() as u16 + 2` would also be one
/// overflow away from a panic.
#[test]
fn home_connection_card_survives_an_absurdly_long_url() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.providers.rows[0].is_current = true;
    data.providers.rows[0].api_url = Some(format!("https://{}", "u".repeat(70_000)));

    for (width, height) in [(160u16, 45u16), (120, 40), (60, 20), (30, 12)] {
        let buf = render_with_size(&app, &data, width, height);
        assert_eq!(buf.area.width, width);
        let all = all_text(&buf);
        // The card keeps its normal footprint: three labelled rows, one each.
        assert_eq!(
            all.lines().filter(|line| line.contains("WebDAV")).count(),
            usize::from(width >= 60),
            "the last card line must survive at {width}x{height}:\n{all}"
        );
    }
}

/// NoColor terminals get the same per-slot glyphs as ASCII ones: `●` in one ink
/// is four identical dots.
#[test]
fn home_usage_chart_uses_distinct_slot_glyphs_without_color() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::set("NO_COLOR", "1");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.usage = usage_with_daily_models(&HOME_CHART_MODELS);

    let card = usage_card_inner_text(&render_with_size(&app, &data, 160, 45));

    assert!(
        !card.contains('●'),
        "a single shape carries no information without color:\n{card}"
    );
    let dots = ['*', '#', '%', '@', '.']
        .into_iter()
        .filter(|glyph| {
            card.contains(&format!("{glyph} claude")) || card.contains(&format!("{glyph} Other"))
        })
        .collect::<Vec<_>>();
    assert!(
        dots.len() >= 2,
        "expected distinct slot glyphs, got {dots:?}:\n{card}"
    );
}

/// The card has to survive a Chinese locale: the heading is wider, and the
/// layout must neither panic nor lose the list.
#[test]
fn home_usage_chart_renders_in_chinese() {
    let _lock = lock_env();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;

    let mut data = minimal_data(&app.app_type);
    data.usage = usage_with_daily_models(&HOME_CHART_MODELS);

    let buf = render_with_size(&app, &data, 160, 45);
    let all = all_text(&buf);
    assert!(all.contains(&buffer_cell_text("用量")), "{all}");
    assert!(all.contains(&buffer_cell_text("最近更新")), "{all}");

    let card = usage_card_inner_text_with_title(&buf, &zh_usage_card_title());
    assert!(
        card.contains(&buffer_cell_text("模型花费")),
        "the ZH list heading:\n{card}"
    );
    assert!(
        card.contains(&buffer_cell_text("其他")),
        "the ZH residual bucket:\n{card}"
    );
    assert!(card.contains('█'), "bars still render:\n{card}");
    assert!(
        card.lines()
            .all(|row| row.starts_with(' ') && row.ends_with(' ')),
        "content never touches the side rails:\n{card}"
    );

    // 130x40 is the other size the card is eyeballed at; it must not panic.
    let narrow = all_text(&render_with_size(&app, &data, 130, 40));
    assert!(narrow.contains(&buffer_cell_text("用量")), "{narrow}");
}

#[test]
fn spinner_frame_cycles_four_frames_per_alphabet() {
    let _lock = lock_env();
    let _icons_lock = lock_test_home_and_settings();

    let _emoji = EnvGuard::set("CC_SWITCH_ICONS", "emoji");
    let unicode = (0..5).map(super::spinner_frame).collect::<Vec<_>>();
    assert_eq!(unicode, vec!["⠋", "⠙", "⠹", "⠸", "⠋"]);

    let _ascii = EnvGuard::set("CC_SWITCH_ICONS", "ascii");
    let ascii = (0..5).map(super::spinner_frame).collect::<Vec<_>>();
    assert_eq!(ascii, vec!["|", "/", "-", "\\", "|"]);
    assert!(
        ascii.iter().all(|frame| frame.is_ascii()),
        "no braille leaks into ascii mode: {ascii:?}"
    );
}

#[test]
fn refresh_indicator_carries_the_accent_and_the_shared_label() {
    let _lock = lock_env();
    let _icons_lock = lock_test_home_and_settings();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _emoji = EnvGuard::set("CC_SWITCH_ICONS", "emoji");
    let theme = theme_for(&AppType::Claude);

    let spans = super::refresh_indicator_spans(1, &theme, None);
    let text = spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert_eq!(text, "⠙ Refreshing");
    assert!(
        spans.iter().all(|span| span.style.fg == Some(theme.accent)),
        "the glyph and its label share the accent: {spans:?}"
    );

    // The escalation percentage rides the same indicator.
    let escalated = super::refresh_indicator_spans(1, &theme, Some(42));
    let escalated_text = escalated
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert_eq!(escalated_text, "⠙ Refreshing 42%");

    // Tight slots take the glyph alone, off the same frame source.
    let glyph = super::refresh_spinner_span(1, &theme);
    assert_eq!(glyph.content.as_ref(), "⠙");
    assert_eq!(glyph.style.fg, Some(theme.accent));
}

#[test]
fn inline_refresh_indicator_follows_the_summary_and_keeps_its_full_label() {
    let _lock = lock_env();
    let _icons_lock = lock_test_home_and_settings();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _emoji = EnvGuard::set("CC_SWITCH_ICONS", "emoji");
    let theme = theme_for(&AppType::Claude);

    let spans =
        super::summary_with_refresh_indicator("1234567890".to_string(), true, 1, &theme, None, 20);
    let text = spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert_eq!(text, "1234… · ⠙ Refreshing");
    assert_eq!(super::inline_refresh_indicator_width(1, &theme, None), 15);
    assert_eq!(super::spans_display_width(&spans), 20);
    assert_eq!(
        spans.last().and_then(|span| span.style.fg),
        Some(theme.accent)
    );
}

#[test]
fn refresh_indicator_reuses_the_chinese_refresh_wording() {
    let _lock = lock_env();
    let _icons_lock = lock_test_home_and_settings();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _emoji = EnvGuard::set("CC_SWITCH_ICONS", "emoji");
    let theme = theme_for(&AppType::Claude);

    let text = super::refresh_indicator_spans(2, &theme, Some(42))
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert_eq!(text, "⠹ 正在刷新 42%");
}

#[test]
fn refresh_indicator_drops_its_hue_under_no_color() {
    let _lock = lock_env();
    let _icons_lock = lock_test_home_and_settings();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::set("NO_COLOR", "1");
    let _emoji = EnvGuard::set("CC_SWITCH_ICONS", "emoji");
    let theme = theme_for(&AppType::Claude);

    let spans = super::refresh_indicator_spans(0, &theme, None);
    assert!(
        spans.iter().all(|span| span.style.fg.is_none()),
        "NoColor keeps the motion and drops the hue: {spans:?}"
    );
    assert!(
        spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
            .contains("Refreshing"),
        "the label survives NoColor: {spans:?}"
    );
}

#[test]
fn sync_escalation_stays_wordless_until_a_round_outlives_ten_seconds() {
    // 50 ticks at the 200ms tick rate.
    assert_eq!(
        super::sync_escalation_percent(Some(0), 49, Some((5, 10))),
        None
    );
    assert_eq!(
        super::sync_escalation_percent(Some(0), 50, Some((5, 10))),
        Some(50)
    );
    // The threshold is measured from the round's own start tick.
    assert_eq!(
        super::sync_escalation_percent(Some(10), 59, Some((5, 10))),
        None
    );
    assert_eq!(
        super::sync_escalation_percent(Some(10), 60, Some((5, 10))),
        Some(50)
    );

    // No round in flight, and rounds that do not know their total yet, stay
    // wordless however long they run.
    assert_eq!(
        super::sync_escalation_percent(None, 5_000, Some((5, 10))),
        None
    );
    assert_eq!(
        super::sync_escalation_percent(Some(0), 5_000, Some((5, 0))),
        None
    );
    assert_eq!(super::sync_escalation_percent(Some(0), 5_000, None), None);

    // Floors, and never overshoots on a total that shrank mid-round.
    assert_eq!(
        super::sync_escalation_percent(Some(0), 50, Some((1, 3))),
        Some(33)
    );
    assert_eq!(
        super::sync_escalation_percent(Some(0), 50, Some((7, 3))),
        Some(100)
    );
}

#[test]
fn sync_round_tracking_starts_on_the_first_live_tick_and_resets_between_rounds() {
    let mut app = App::new(Some(AppType::Claude));

    app.tick = 7;
    app.note_session_sync_round(false);
    assert_eq!(app.usage_sync_round_started_tick, None);

    app.note_session_sync_round(true);
    assert_eq!(app.usage_sync_round_started_tick, Some(7));

    app.tick = 30;
    app.note_session_sync_round(true);
    assert_eq!(
        app.usage_sync_round_started_tick,
        Some(7),
        "a running round keeps the tick it started on"
    );

    app.note_session_sync_round(false);
    assert_eq!(app.usage_sync_round_started_tick, None);

    app.tick = 42;
    app.note_session_sync_round(true);
    assert_eq!(
        app.usage_sync_round_started_tick,
        Some(42),
        "the next round starts its own clock"
    );
}

#[test]
fn sync_escalation_reads_the_live_round_against_the_app_clock() {
    let _override = super::SyncProgressOverride::set(Some((21, 50)));

    let mut app = App::new(Some(AppType::Claude));
    app.tick = 10;
    app.note_session_sync_round(true);

    app.tick = 59;
    assert_eq!(super::sync_escalation(&app), None);

    app.tick = 60;
    assert_eq!(super::sync_escalation(&app), Some(42));
}

#[test]
fn home_usage_card_rail_spins_without_a_counter_while_syncing() {
    let _lock = lock_env();
    let _icons_lock = lock_test_home_and_settings();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _emoji = EnvGuard::set("CC_SWITCH_ICONS", "emoji");
    let _override = super::SyncProgressOverride::set(Some((1_234, 18_704)));

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;
    app.tick = 3;
    app.note_session_sync_round(true);

    let mut data = minimal_data(&app.app_type);
    // Data on screen keeps the body quiet, so the rail owns the indicator.
    data.usage = usage_with_daily_models(&[("claude-opus", 5_000, 6.0)]);

    let buf = render_with_size(&app, &data, 160, 45);
    let title_row = row_of(&buf, &usage_card_title()).expect("usage card title row");
    let rail = line_at(&buf, title_row);

    assert!(
        rail.contains("⠸ Refreshing"),
        "the spinner and its label own the rail:\n{rail}"
    );
    assert!(!rail.contains("1234/18704"), "{rail}");
    assert!(!rail.contains("18704"), "{rail}");
    assert!(!rail.to_lowercase().contains("sync"), "{rail}");
    assert!(
        !rail.contains('%'),
        "a short round earns no number:\n{rail}"
    );
    // The rail is exclusive: no stale-timestamp text behind the indicator.
    assert!(!rail.contains("Last updated"), "{rail}");
}

#[test]
fn home_usage_card_rail_spins_while_existing_data_refreshes() {
    let _lock = lock_env();
    let _icons_lock = lock_test_home_and_settings();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _emoji = EnvGuard::set("CC_SWITCH_ICONS", "emoji");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;
    app.tick = 3;
    app.usage
        .start_loading(AppType::Claude, UsageRangePreset::ThirtyDays);

    let mut data = minimal_data(&app.app_type);
    data.usage = usage_with_daily_models(&[("claude-opus", 5_000, 6.0)]);

    let buf = render_with_size(&app, &data, 160, 45);
    let title_row = row_of(&buf, &usage_card_title()).expect("usage card title row");
    let rail = line_at(&buf, title_row);
    let card = usage_card_inner_text(&buf);

    assert!(rail.contains("⠸ Refreshing"), "{rail}");
    assert!(!rail.contains("Last updated"), "{rail}");
    assert!(
        !card.contains("Refreshing"),
        "existing data remains visible while the rail owns the indicator:\n{card}"
    );
}

#[test]
fn home_usage_card_rail_earns_a_percentage_after_ten_seconds() {
    let _lock = lock_env();
    let _icons_lock = lock_test_home_and_settings();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _emoji = EnvGuard::set("CC_SWITCH_ICONS", "emoji");
    let _override = super::SyncProgressOverride::set(Some((1_234, 18_704)));

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;
    app.tick = 4;
    app.note_session_sync_round(true);
    app.tick = 4 + super::SYNC_ESCALATION_TICKS;

    let mut data = minimal_data(&app.app_type);
    data.usage = usage_with_daily_models(&[("claude-opus", 5_000, 6.0)]);

    let buf = render_with_size(&app, &data, 160, 45);
    let title_row = row_of(&buf, &usage_card_title()).expect("usage card title row");
    let rail = line_at(&buf, title_row);

    assert!(
        rail.contains("Refreshing 6%"),
        "a slow round appends its share to the one indicator:\n{rail}"
    );
    assert!(!rail.contains("1234/18704"), "{rail}");
}

#[test]
fn home_usage_card_shows_one_indicator_across_its_rail_and_body() {
    let _lock = lock_env();
    let _icons_lock = lock_test_home_and_settings();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _emoji = EnvGuard::set("CC_SWITCH_ICONS", "emoji");
    let _override = super::SyncProgressOverride::set(Some((1_234, 18_704)));

    // An import round *and* the card's own first aggregate, both in flight.
    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;
    app.tick = 3;
    app.note_session_sync_round(true);
    app.usage
        .start_loading(AppType::Claude, UsageRangePreset::ThirtyDays);

    let buf = render_with_size(&app, &minimal_data(&app.app_type), 160, 45);
    let card = usage_card_inner_text(&buf);
    let title_row = row_of(&buf, &usage_card_title()).expect("usage card title row");
    let rail = line_at(&buf, title_row);

    // The body owns the shared indicator, so the rail steps back to its
    // resting status.
    assert!(card.contains("⠸ Refreshing"), "{card}");
    assert!(
        !rail.contains('⠸'),
        "the rail must not spin alongside the body:\n{rail}"
    );
    assert!(!rail.contains("Refreshing"), "{rail}");
    assert_eq!(
        card.matches('⠸').count(),
        1,
        "exactly one spinner inside the card:\n{card}"
    );
}

#[test]
fn home_usage_card_rail_keeps_its_spinner_ascii_in_ascii_icon_mode() {
    let _lock = lock_env();
    let _icons_lock = lock_test_home_and_settings();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _ascii = EnvGuard::set("CC_SWITCH_ICONS", "ascii");
    let _override = super::SyncProgressOverride::set(Some((1_234, 18_704)));

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;
    app.tick = 3;
    app.note_session_sync_round(true);

    let mut data = minimal_data(&app.app_type);
    data.usage = usage_with_daily_models(&[("claude-opus", 5_000, 6.0)]);

    let buf = render_with_size(&app, &data, 160, 45);
    let title_row = row_of(&buf, &usage_card_title()).expect("usage card title row");
    let rail = line_at(&buf, title_row);

    assert!(rail.contains("\\ Refreshing"), "{rail}");
    assert!(
        !contains_braille(&rail),
        "no braille leaks into ascii mode:\n{rail}"
    );
}

#[test]
fn usage_summary_bar_merges_a_manual_refresh_and_a_background_import() {
    let _lock = lock_env();
    let _icons_lock = lock_test_home_and_settings();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _emoji = EnvGuard::set("CC_SWITCH_ICONS", "emoji");
    let _override = super::SyncProgressOverride::set(Some((1_234, 18_704)));

    // Both pipelines live at once: this bar used to animate twice.
    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Usage;
    app.focus = Focus::Content;
    app.tick = 1;
    app.note_session_sync_round(true);
    app.usage.start_manual_session_refresh();
    let data = minimal_data(&app.app_type);

    let buf = render_with_size(&app, &data, 120, 40);
    let summary_row = row_of(&buf, "avg latency").expect("the usage summary bar");
    let summary = line_at(&buf, summary_row);
    assert!(summary.contains("avg latency · ⠙ Refreshing"), "{summary}");
    assert_eq!(
        summary.matches("Refreshing").count(),
        1,
        "one bar, one indicator:\n{summary}"
    );
    assert_eq!(
        summary.matches('⠙').count(),
        1,
        "one bar, one spinner:\n{summary}"
    );
    assert!(!summary.contains("importing local usage"), "{summary}");
    assert!(!summary.contains("1234/18704"), "{summary}");

    // The detail bar merges the same two pipelines.
    app.route = Route::UsageLogs;
    let logs_buf = render_with_size(&app, &data, 120, 40);
    let logs_row = row_of(&logs_buf, "model stats").expect("the usage detail summary bar");
    let logs = line_at(&logs_buf, logs_row);
    assert_eq!(logs.matches("Refreshing").count(), 1, "{logs}");
    assert_eq!(logs.matches('⠙').count(), 1, "{logs}");

    // Idle again: the summary bar carries no indicator at all.
    let _idle = super::SyncProgressOverride::set(None);
    let mut idle_app = App::new(Some(AppType::Claude));
    idle_app.route = Route::Usage;
    idle_app.focus = Focus::Content;
    idle_app.tick = 1;
    idle_app.note_session_sync_round(false);
    let idle_buf = render_with_size(&idle_app, &data, 120, 40);
    let idle_row = row_of(&idle_buf, "avg latency").expect("the usage summary bar");
    let idle = line_at(&idle_buf, idle_row);
    assert!(!idle.contains("Refreshing"), "{idle}");
    assert!(!idle.contains('⠙') && !idle.contains('⠋'), "{idle}");
}

#[test]
fn usage_summary_bar_spins_for_a_background_import_alone() {
    let _lock = lock_env();
    let _icons_lock = lock_test_home_and_settings();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _emoji = EnvGuard::set("CC_SWITCH_ICONS", "emoji");
    let _override = super::SyncProgressOverride::set(Some((21, 50)));

    // No manual refresh: the import alone still earns the bar's indicator.
    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Usage;
    app.focus = Focus::Content;
    app.tick = 4;
    app.note_session_sync_round(true);

    // One tick short of the threshold: the indicator is there, the number is not.
    app.tick = 3 + super::SYNC_ESCALATION_TICKS;
    let early = render_with_size(&app, &minimal_data(&app.app_type), 120, 40);
    let early_row = row_of(&early, "avg latency").expect("the usage summary bar");
    let early_summary = line_at(&early, early_row);
    assert!(early_summary.contains(" · ⠙ Refreshing"), "{early_summary}");
    assert!(
        !early_summary.contains('%'),
        "a round under ten seconds earns no number:\n{early_summary}"
    );

    // 50 ticks in, the percentage joins the same indicator.
    app.tick = 4 + super::SYNC_ESCALATION_TICKS;
    let buf = render_with_size(&app, &minimal_data(&app.app_type), 120, 40);
    let summary_row = row_of(&buf, "avg latency").expect("the usage summary bar");
    let summary = line_at(&buf, summary_row);
    assert!(
        summary.contains(" · ⠹ Refreshing 42%"),
        "the escalation rides the merged indicator:\n{summary}"
    );
    assert_eq!(summary.matches("Refreshing").count(), 1, "{summary}");
    assert_eq!(summary.matches('%').count(), 1, "{summary}");
}

#[test]
fn usage_summary_bar_keeps_its_spinner_ascii_in_ascii_icon_mode() {
    let _lock = lock_env();
    let _icons_lock = lock_test_home_and_settings();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _ascii = EnvGuard::set("CC_SWITCH_ICONS", "ascii");
    let _override = super::SyncProgressOverride::set(Some((1_234, 18_704)));

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Usage;
    app.focus = Focus::Content;
    app.tick = 1;
    app.note_session_sync_round(true);

    let buf = render_with_size(&app, &minimal_data(&app.app_type), 120, 40);
    let summary_row = row_of(&buf, "avg latency").expect("the usage summary bar");
    let summary = line_at(&buf, summary_row);
    assert!(summary.contains("/ Refreshing"), "{summary}");
    assert!(
        !contains_braille(&summary),
        "no braille leaks into ascii mode:\n{summary}"
    );
}

#[test]
fn sessions_scope_line_spins_while_a_scan_is_in_flight() {
    let _lock = lock_env();
    let _icons_lock = lock_test_home_and_settings();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _emoji = EnvGuard::set("CC_SWITCH_ICONS", "emoji");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Sessions;
    app.focus = Focus::Content;
    app.tick = 1;
    app.sessions.loading = true;
    let data = minimal_data(&app.app_type);

    let buf = render_with_size(&app, &data, 120, 40);
    let scope_row = row_of(&buf, "Scope").expect("the sessions scope line");
    let scope = line_at(&buf, scope_row);
    assert_eq!(
        scope.matches('⠙').count(),
        1,
        "one scope line, one spinner:\n{scope}"
    );
    assert!(
        scope.contains(" · ⠙ Refreshing"),
        "the sessions scope uses the shared labelled indicator:\n{scope}"
    );

    // A finished scan drops the indicator entirely.
    app.sessions.loading = false;
    app.sessions.loaded_once = true;
    let done_buf = render_with_size(&app, &data, 120, 40);
    let done_row = row_of(&done_buf, "Scope").expect("the sessions scope line");
    let done = line_at(&done_buf, done_row);
    assert!(!done.contains('⠙'), "{done}");
    assert!(!done.contains("Refreshing"), "{done}");
}

#[test]
fn sessions_scope_line_keeps_its_spinner_ascii_in_ascii_icon_mode() {
    let _lock = lock_env();
    let _icons_lock = lock_test_home_and_settings();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _ascii = EnvGuard::set("CC_SWITCH_ICONS", "ascii");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Sessions;
    app.focus = Focus::Content;
    app.tick = 1;
    app.sessions.loading = true;

    let buf = render_with_size(&app, &minimal_data(&app.app_type), 120, 40);
    let scope_row = row_of(&buf, "Scope").expect("the sessions scope line");
    let scope = line_at(&buf, scope_row);
    assert!(scope.contains("/ Refreshing"), "{scope}");
    assert!(
        !contains_braille(&scope),
        "no braille leaks into ascii mode:\n{scope}"
    );
}

#[test]
fn pagination_footer_busy_state_uses_the_shared_spinner_glyph() {
    use crate::cli::tui::input::{ScrollDirection, WheelGestureId};

    let _lock = lock_env();
    let _icons_lock = lock_test_home_and_settings();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _emoji = EnvGuard::set("CC_SWITCH_ICONS", "emoji");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::UsageLogs;
    app.focus = Focus::Content;
    app.usage.pane = UsagePane::Recent;
    app.tick = 2;
    let mut data = minimal_data(&app.app_type);
    data.usage.logs_total = 205;
    data.usage.recent_logs = (0..100)
        .map(|index| UsageLogRow {
            request_id: format!("p0-{index}"),
            model: format!("model-{index}"),
            created_at: 1_000 - index,
            cursor_rowid: index + 1,
            ..UsageLogRow::default()
        })
        .collect();
    app.on_usage_logs_wheel(
        ScrollDirection::Down,
        10_000,
        WheelGestureId::from_raw(1),
        &data,
    );
    assert!(app.usage.log_pager.start_request(
        1,
        9,
        crate::cli::tui::data::UsageLogPageDirection::Older,
    ));

    let buf = render_with_size(&app, &data, 180, 40);
    let row = row_of(&buf, "Loading page 2").expect("the paging footer");
    let footer = line_at(&buf, row);
    assert_eq!(
        footer.matches('⠹').count(),
        1,
        "one footer, one spinner:\n{footer}"
    );
    // A footer cell is too tight for a second label next to "Loading page 2".
    assert!(!footer.contains("Refreshing"), "{footer}");
    // The range label still shares the rail with the busy action.
    assert!(footer.contains("Page 1"), "{footer}");
}

#[test]
fn pagination_footer_keeps_its_spinner_ascii_in_ascii_icon_mode() {
    use crate::cli::tui::input::{ScrollDirection, WheelGestureId};

    let _lock = lock_env();
    let _icons_lock = lock_test_home_and_settings();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _ascii = EnvGuard::set("CC_SWITCH_ICONS", "ascii");

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::UsageLogs;
    app.focus = Focus::Content;
    app.usage.pane = UsagePane::Recent;
    app.tick = 2;
    let mut data = minimal_data(&app.app_type);
    data.usage.logs_total = 205;
    data.usage.recent_logs = (0..100)
        .map(|index| UsageLogRow {
            request_id: format!("p0-{index}"),
            model: format!("model-{index}"),
            created_at: 1_000 - index,
            cursor_rowid: index + 1,
            ..UsageLogRow::default()
        })
        .collect();
    app.on_usage_logs_wheel(
        ScrollDirection::Down,
        10_000,
        WheelGestureId::from_raw(1),
        &data,
    );
    assert!(app.usage.log_pager.start_request(
        1,
        9,
        crate::cli::tui::data::UsageLogPageDirection::Older,
    ));

    let buf = render_with_size(&app, &data, 180, 40);
    let row = row_of(&buf, "Loading page 2").expect("the paging footer");
    let footer = line_at(&buf, row);
    assert!(footer.contains("- Loading page 2"), "{footer}");
    assert!(
        !contains_braille(&footer),
        "no braille leaks into ascii mode:\n{footer}"
    );
}

#[test]
fn home_usage_card_rail_falls_back_to_the_glyph_when_the_label_will_not_fit() {
    let _lock = lock_env();
    let _icons_lock = lock_test_home_and_settings();
    let _lang = use_test_language(Language::English);
    let _no_color = EnvGuard::remove("NO_COLOR");
    let _emoji = EnvGuard::set("CC_SWITCH_ICONS", "emoji");
    let _override = super::SyncProgressOverride::set(Some((1_234, 18_704)));

    let mut app = App::new(Some(AppType::Claude));
    app.route = Route::Main;
    app.focus = Focus::Content;
    app.tick = 3;
    app.note_session_sync_round(true);
    let data = minimal_data(&app.app_type);

    // 52 columns leave the rail too little room for " ⠸ Refreshing ": it keeps
    // the glyph rather than dropping the signal.
    let narrow = render_with_size(&app, &data, 52, 45);
    let narrow_row = row_of(&narrow, &usage_card_title()).expect("usage card title row");
    let narrow_rail = line_at(&narrow, narrow_row);
    assert!(narrow_rail.contains('⠸'), "{narrow_rail}");
    assert!(!narrow_rail.contains("Refreshing"), "{narrow_rail}");

    // Eight more columns and the label comes back.
    let wide = render_with_size(&app, &data, 60, 45);
    let wide_row = row_of(&wide, &usage_card_title()).expect("usage card title row");
    let wide_rail = line_at(&wide, wide_row);
    assert!(wide_rail.contains("⠸ Refreshing"), "{wide_rail}");
}
