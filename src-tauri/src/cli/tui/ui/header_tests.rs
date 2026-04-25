use tempfile::TempDir;
use unicode_width::UnicodeWidthStr;

use crate::{
    app_config::AppType,
    cli::i18n::{texts, use_test_language, Language},
    cli::tui::app::App,
    openclaw_config::{OpenClawAgentsDefaults, OpenClawDefaultModel, OpenClawHealthWarning},
};

#[test]
fn header_openclaw_shows_default_model_badge_and_hides_proxy_badge() {
    let _ctx = TestContext::new();
    let _lang = use_test_language(Language::English);
    let _no_color = super::tests::EnvGuard::remove("NO_COLOR");

    let app = App::new(Some(AppType::OpenClaw));
    let mut data = super::tests::minimal_data(&app.app_type);
    data.config.openclaw_agents_defaults = Some(OpenClawAgentsDefaults {
        model: Some(OpenClawDefaultModel {
            primary: "gpt-4.1".to_string(),
            fallbacks: Vec::new(),
            extra: std::collections::HashMap::new(),
        }),
        models: None,
        extra: std::collections::HashMap::new(),
    });

    let header = super::tests::line_at(&super::tests::render(&app, &data), 1);

    assert!(header.contains("Default Model: gpt-4.1"), "{header}");
    assert_openclaw_provider_hidden_en(&header);
    assert_proxy_hidden_en(&header);
}

#[test]
fn header_openclaw_shows_none_when_default_model_is_missing() {
    let _ctx = TestContext::new();
    let _lang = use_test_language(Language::English);
    let _no_color = super::tests::EnvGuard::remove("NO_COLOR");

    let app = App::new(Some(AppType::OpenClaw));
    let mut data = super::tests::minimal_data(&app.app_type);
    data.config.openclaw_agents_defaults = Some(OpenClawAgentsDefaults::default());

    let header = super::tests::line_at(&super::tests::render(&app, &data), 1);

    assert!(header.contains("Default Model: None"), "{header}");
    assert_openclaw_provider_hidden_en(&header);
    assert_proxy_hidden_en(&header);
}

#[test]
fn header_openclaw_shows_config_error_for_agents_section_parse_warning() {
    let _ctx = TestContext::new();
    let _lang = use_test_language(Language::English);
    let _no_color = super::tests::EnvGuard::remove("NO_COLOR");

    let app = App::new(Some(AppType::OpenClaw));
    let mut data = super::tests::minimal_data(&app.app_type);
    data.config.openclaw_warnings = Some(vec![OpenClawHealthWarning {
        code: "config_parse_failed".to_string(),
        message: "bad agents".to_string(),
        path: Some("agents.defaults".to_string()),
    }]);

    let header = super::tests::line_at(&super::tests::render(&app, &data), 1);

    assert!(header.contains("Default Model: Config Error"), "{header}");
    assert_openclaw_provider_hidden_en(&header);
    assert_proxy_hidden_en(&header);
}

#[test]
fn header_openclaw_shows_config_error_for_full_file_parse_warning() {
    let _ctx = TestContext::new();
    let _lang = use_test_language(Language::English);
    let _no_color = super::tests::EnvGuard::remove("NO_COLOR");

    let app = App::new(Some(AppType::OpenClaw));
    let mut data = super::tests::minimal_data(&app.app_type);
    data.config.openclaw_config_path = Some(std::path::PathBuf::from("/tmp/openclaw.json"));
    data.config.openclaw_warnings = Some(vec![OpenClawHealthWarning {
        code: "config_parse_failed".to_string(),
        message: "bad file".to_string(),
        path: Some("/tmp/openclaw.json".to_string()),
    }]);

    let header = super::tests::line_at(&super::tests::render(&app, &data), 1);

    assert!(header.contains("Default Model: Config Error"), "{header}");
    assert_openclaw_provider_hidden_en(&header);
    assert_proxy_hidden_en(&header);
}

#[test]
fn header_openclaw_treats_blank_primary_model_as_none() {
    let _ctx = TestContext::new();
    let _lang = use_test_language(Language::English);
    let _no_color = super::tests::EnvGuard::remove("NO_COLOR");

    let app = App::new(Some(AppType::OpenClaw));
    let mut data = super::tests::minimal_data(&app.app_type);
    data.config.openclaw_agents_defaults = Some(OpenClawAgentsDefaults {
        model: Some(OpenClawDefaultModel {
            primary: " \t ".to_string(),
            fallbacks: Vec::new(),
            extra: std::collections::HashMap::new(),
        }),
        models: None,
        extra: std::collections::HashMap::new(),
    });

    let header = super::tests::line_at(&super::tests::render(&app, &data), 1);

    assert!(header.contains("Default Model: None"), "{header}");
    assert_openclaw_provider_hidden_en(&header);
    assert_proxy_hidden_en(&header);
}

#[test]
fn header_openclaw_preserves_non_empty_primary_spacing() {
    let _ctx = TestContext::new();
    let _lang = use_test_language(Language::English);
    let _no_color = super::tests::EnvGuard::remove("NO_COLOR");

    let app = App::new(Some(AppType::OpenClaw));
    let mut data = super::tests::minimal_data(&app.app_type);
    data.config.openclaw_agents_defaults = Some(OpenClawAgentsDefaults {
        model: Some(OpenClawDefaultModel {
            primary: "  gpt-4.1  ".to_string(),
            fallbacks: Vec::new(),
            extra: std::collections::HashMap::new(),
        }),
        models: None,
        extra: std::collections::HashMap::new(),
    });

    let header = super::tests::line_at(&super::tests::render(&app, &data), 1);

    assert!(header.contains("Default Model:   gpt-4.1  "), "{header}");
    assert_openclaw_provider_hidden_en(&header);
    assert_proxy_hidden_en(&header);
}

#[test]
fn header_openclaw_localizes_none_in_chinese() {
    let _ctx = TestContext::new();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = super::tests::EnvGuard::remove("NO_COLOR");

    let app = App::new(Some(AppType::OpenClaw));

    let empty_header = super::tests::line_at(
        &super::tests::render(&app, &super::tests::minimal_data(&app.app_type)),
        1,
    );
    assert!(
        empty_header.contains(&super::tests::buffer_cell_text("默认模型: 无")),
        "{empty_header}"
    );
    assert_openclaw_provider_hidden_zh(&empty_header);
    assert_proxy_hidden_zh(&empty_header);
}

#[test]
fn header_openclaw_localizes_config_error_in_chinese() {
    let _ctx = TestContext::new();
    let _lang = use_test_language(Language::Chinese);
    let _no_color = super::tests::EnvGuard::remove("NO_COLOR");

    let app = App::new(Some(AppType::OpenClaw));

    let mut warning_data = super::tests::minimal_data(&app.app_type);
    warning_data.config.openclaw_warnings = Some(vec![OpenClawHealthWarning {
        code: "config_parse_failed".to_string(),
        message: "bad agents".to_string(),
        path: Some("agents.defaults".to_string()),
    }]);

    let warning_header = super::tests::line_at(&super::tests::render(&app, &warning_data), 1);
    assert!(
        warning_header.contains(&super::tests::buffer_cell_text("默认模型: 配置错误")),
        "{warning_header}"
    );
    assert_openclaw_provider_hidden_zh(&warning_header);
    assert_proxy_hidden_zh(&warning_header);
}

#[test]
fn header_openclaw_sacrifices_tabs_before_losing_the_only_status_badge() {
    let _ctx = TestContext::new().with_visible_apps(crate::settings::VisibleApps {
        claude: true,
        codex: true,
        gemini: true,
        opencode: true,
        openclaw: true,
    });
    let _lang = use_test_language(Language::English);
    let _no_color = super::tests::EnvGuard::remove("NO_COLOR");

    let app = App::new(Some(AppType::OpenClaw));
    let mut data = super::tests::minimal_data(&app.app_type);
    data.config.openclaw_agents_defaults = Some(OpenClawAgentsDefaults {
        model: Some(OpenClawDefaultModel {
            primary: "gpt-4.1".to_string(),
            fallbacks: Vec::new(),
            extra: std::collections::HashMap::new(),
        }),
        models: None,
        extra: std::collections::HashMap::new(),
    });

    let title_width = UnicodeWidthStr::width(format!("  {}", texts::tui_app_title()).as_str());
    let status_badge_width = UnicodeWidthStr::width("  Default Model: gpt-4.1  ");
    let total_width = (title_width + status_badge_width + 2) as u16;

    let header = super::tests::line_at(
        &super::tests::render_with_size(&app, &data, total_width, 20),
        1,
    );

    assert!(header.contains("Default Model: gpt-4.1"), "{header}");
    assert_openclaw_provider_hidden_en(&header);
    assert_proxy_hidden_en(&header);
    assert_eq!(super::tests::visible_tab_labels(&header), 0, "{header}");
}

#[test]
fn header_openclaw_truncates_long_default_model_without_fake_proxy_gap() {
    let _ctx = TestContext::new().with_visible_apps(crate::settings::VisibleApps {
        claude: true,
        codex: true,
        gemini: true,
        opencode: true,
        openclaw: true,
    });
    let _lang = use_test_language(Language::English);
    let _no_color = super::tests::EnvGuard::remove("NO_COLOR");

    let app = App::new(Some(AppType::OpenClaw));
    let mut data = super::tests::minimal_data(&app.app_type);
    data.config.openclaw_agents_defaults = Some(OpenClawAgentsDefaults {
        model: Some(OpenClawDefaultModel {
            primary: "very-long-provider/very-long-model-name-that-must-truncate".to_string(),
            fallbacks: Vec::new(),
            extra: std::collections::HashMap::new(),
        }),
        models: None,
        extra: std::collections::HashMap::new(),
    });

    let header = super::tests::line_at(&super::tests::render_with_size(&app, &data, 72, 20), 1);

    assert!(header.contains("Default Model:"), "{header}");
    assert_openclaw_provider_hidden_en(&header);
    assert!(header.contains("very-long-provider/"), "{header}");
    assert!(header.contains('…'), "{header}");
    assert!(
        !header.contains("very-long-provider/very-long-model-name-that-must-truncate"),
        "{header}"
    );
    assert_proxy_hidden_en(&header);
}

#[test]
fn header_opencode_hides_proxy_badge_and_reports_config_membership() {
    let _ctx = TestContext::new();
    let _lang = use_test_language(Language::English);
    let _no_color = super::tests::EnvGuard::remove("NO_COLOR");

    let app = App::new(Some(AppType::OpenCode));
    let mut data = super::tests::minimal_data(&app.app_type);
    data.providers.current_id.clear();
    data.providers.rows[0].is_current = false;
    data.providers.rows[0].is_in_config = true;

    let header = super::tests::line_at(&super::tests::render(&app, &data), 1);

    assert!(
        header.contains("OpenCode Config: 1/1 in config"),
        "{header}"
    );
    assert_proxy_hidden_en(&header);
}

#[test]
fn header_opencode_hides_proxy_badge_and_reports_empty_config_membership() {
    let _ctx = TestContext::new();
    let _lang = use_test_language(Language::English);
    let _no_color = super::tests::EnvGuard::remove("NO_COLOR");

    let app = App::new(Some(AppType::OpenCode));
    let mut data = super::tests::minimal_data(&app.app_type);
    data.providers.current_id.clear();
    data.providers.rows[0].is_current = false;
    data.providers.rows[0].is_in_config = false;
    let header = super::tests::line_at(&super::tests::render(&app, &data), 1);

    assert!(
        header.contains("OpenCode Config: 0/1 in config"),
        "{header}"
    );
    assert_proxy_hidden_en(&header);
}

struct TestContext {
    _env: std::sync::MutexGuard<'static, ()>,
    _temp_home: TempDir,
    _home: super::tests::SettingsEnvGuard,
}

impl TestContext {
    fn new() -> Self {
        let env = super::tests::lock_env();
        let temp_home = TempDir::new().expect("create temp home");
        let home = super::tests::SettingsEnvGuard::set_home(temp_home.path());
        Self {
            _env: env,
            _temp_home: temp_home,
            _home: home,
        }
    }

    fn with_visible_apps(self, visible_apps: crate::settings::VisibleApps) -> Self {
        crate::settings::set_visible_apps(visible_apps).expect("save visible apps");
        self
    }
}

fn assert_proxy_hidden_en(header: &str) {
    assert!(!header.contains("Proxy: Off"), "{header}");
    assert!(!header.contains("Proxy: On"), "{header}");
}

fn assert_proxy_hidden_zh(header: &str) {
    assert!(
        !header.contains(&super::tests::buffer_cell_text("代理: 关")),
        "{header}"
    );
    assert!(
        !header.contains(&super::tests::buffer_cell_text("代理: 开")),
        "{header}"
    );
}

fn assert_openclaw_provider_hidden_en(header: &str) {
    assert!(!header.contains("Provider:"), "{header}");
}

fn assert_openclaw_provider_hidden_zh(header: &str) {
    assert!(
        !header.contains(&super::tests::buffer_cell_text("供应商:")),
        "{header}"
    );
}
