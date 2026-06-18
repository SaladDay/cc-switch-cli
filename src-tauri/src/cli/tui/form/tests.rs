use super::*;
use crate::cli::commands::provider_input::{build_provider_template_seed, ProviderAddTemplate};
use crate::provider::Provider;
use serde_json::json;

fn template_index_by_label(app_type: AppType, label: &str) -> usize {
    ProviderAddFormState::new(app_type)
        .template_labels()
        .iter()
        .position(|item| *item == label)
        .expect("template should exist")
}

fn claudeapi_template_index(app_type: AppType) -> usize {
    template_index_by_label(app_type, "* ClaudeAPI")
}

fn packycode_template_index(app_type: AppType) -> usize {
    template_index_by_label(app_type, "* PackyCode")
}

fn aicodemirror_template_index(app_type: AppType) -> usize {
    template_index_by_label(app_type, "* AICodeMirror")
}

fn cubence_template_index(app_type: AppType) -> usize {
    template_index_by_label(app_type, "* Cubence")
}

fn runapi_template_index(app_type: AppType) -> usize {
    template_index_by_label(app_type, "* RunAPI")
}

fn dds_template_index(app_type: AppType) -> usize {
    template_index_by_label(app_type, "* DDS")
}

fn deepseek_template_index(app_type: AppType) -> usize {
    template_index_by_label(app_type, "DeepSeek")
}

fn normalize_template_provider_json(mut value: serde_json::Value) -> serde_json::Value {
    if let Some(obj) = value.as_object_mut() {
        obj.remove("inFailoverQueue");
        if let Some(meta) = obj.get_mut("meta").and_then(|value| value.as_object_mut()) {
            if meta.get("commonConfigEnabled") == Some(&json!(false)) {
                meta.remove("commonConfigEnabled");
            }
            if meta.is_empty() {
                obj.remove("meta");
            }
        }
    }
    value
}

fn assert_cli_template_matches_tui_serializer(
    app_type: AppType,
    template: ProviderAddTemplate,
    label: &str,
) {
    let mut form = ProviderAddFormState::new(app_type.clone());
    let existing_ids = Vec::<String>::new();
    form.apply_template(
        template_index_by_label(app_type.clone(), label),
        &existing_ids,
    );

    let cli_value = serde_json::to_value(
        build_provider_template_seed(&app_type, template, &existing_ids)
            .expect("CLI template seed should build"),
    )
    .expect("CLI provider should serialize");
    let tui_value = form.to_provider_json_value();

    assert_eq!(
        normalize_template_provider_json(cli_value),
        normalize_template_provider_json(tui_value)
    );
}

#[test]
fn provider_add_form_template_labels_use_ascii_prefix_for_packycode() {
    let form = ProviderAddFormState::new(AppType::Claude);
    let labels = form.template_labels();

    assert!(
        labels.contains(&"* PackyCode"),
        "expected PackyCode chip label to use ASCII prefix for alignment stability"
    );
    assert!(
        labels.contains(&"* ClaudeAPI"),
        "expected ClaudeAPI chip label to use ASCII prefix for alignment stability"
    );
}

#[test]
fn provider_add_form_template_labels_follow_explicit_support_matrix() {
    let claude_labels = ProviderAddFormState::new(AppType::Claude).template_labels();
    assert_eq!(
        claude_labels,
        vec![
            "Custom",
            "Claude Official",
            "Codex",
            "* ClaudeAPI",
            "* PackyCode",
            "* Cubence",
            "* RunAPI",
            "* AICodeMirror",
            "* DDS",
        ]
    );

    let codex_labels = ProviderAddFormState::new(AppType::Codex).template_labels();
    assert_eq!(
        codex_labels,
        vec![
            "Custom",
            "OpenAI Official",
            "* PackyCode",
            "* Cubence",
            "* RunAPI",
            "* AICodeMirror",
            "* DDS",
            "DeepSeek",
        ]
    );

    let gemini_labels = ProviderAddFormState::new(AppType::Gemini).template_labels();
    assert_eq!(
        gemini_labels,
        vec![
            "Custom",
            "Google OAuth",
            "* PackyCode",
            "* Cubence",
            "* AICodeMirror",
        ]
    );

    let opencode_labels = ProviderAddFormState::new(AppType::OpenCode).template_labels();
    assert_eq!(
        opencode_labels,
        vec!["Custom", "* Cubence", "* RunAPI", "* AICodeMirror"]
    );
    assert!(
        !opencode_labels.contains(&"* PackyCode"),
        "OpenCode should expose only explicitly supported sponsor presets"
    );
    assert!(
        !opencode_labels.contains(&"* ClaudeAPI"),
        "OpenCode should not expose Claude-only sponsor presets"
    );

    let hermes_labels = ProviderAddFormState::new(AppType::Hermes).template_labels();
    assert_eq!(hermes_labels, vec!["Custom", "* Cubence", "* RunAPI"]);

    let openclaw_labels = ProviderAddFormState::new(AppType::OpenClaw).template_labels();
    assert_eq!(
        openclaw_labels,
        vec!["Custom", "* Cubence", "* RunAPI", "* AICodeMirror"]
    );
    assert!(
        !openclaw_labels.contains(&"* PackyCode"),
        "OpenClaw should expose only explicitly supported sponsor presets"
    );
    assert!(
        !openclaw_labels.contains(&"* ClaudeAPI"),
        "OpenClaw should not expose Claude-only sponsor presets"
    );
}

#[test]
fn cli_provider_templates_match_tui_serializer_output() {
    for (app_type, template, label) in [
        (
            AppType::Claude,
            ProviderAddTemplate::ClaudeOfficial,
            "Claude Official",
        ),
        (AppType::Claude, ProviderAddTemplate::CodexOauth, "Codex"),
        (
            AppType::Codex,
            ProviderAddTemplate::OpenaiOfficial,
            "OpenAI Official",
        ),
        (
            AppType::Gemini,
            ProviderAddTemplate::GoogleOauth,
            "Google OAuth",
        ),
        (
            AppType::Claude,
            ProviderAddTemplate::Claudeapi,
            "* ClaudeAPI",
        ),
        (
            AppType::Claude,
            ProviderAddTemplate::Packycode,
            "* PackyCode",
        ),
        (
            AppType::Codex,
            ProviderAddTemplate::Aicodemirror,
            "* AICodeMirror",
        ),
        (AppType::Codex, ProviderAddTemplate::Runapi, "* RunAPI"),
        (AppType::Codex, ProviderAddTemplate::Deepseek, "DeepSeek"),
        (AppType::Gemini, ProviderAddTemplate::Cubence, "* Cubence"),
        (AppType::Claude, ProviderAddTemplate::Dds, "* DDS"),
        (
            AppType::OpenCode,
            ProviderAddTemplate::Aicodemirror,
            "* AICodeMirror",
        ),
        (AppType::OpenCode, ProviderAddTemplate::Cubence, "* Cubence"),
        (AppType::OpenCode, ProviderAddTemplate::Runapi, "* RunAPI"),
        (AppType::Hermes, ProviderAddTemplate::Cubence, "* Cubence"),
        (AppType::Hermes, ProviderAddTemplate::Runapi, "* RunAPI"),
        (
            AppType::OpenClaw,
            ProviderAddTemplate::Aicodemirror,
            "* AICodeMirror",
        ),
        (AppType::OpenClaw, ProviderAddTemplate::Cubence, "* Cubence"),
        (AppType::OpenClaw, ProviderAddTemplate::Runapi, "* RunAPI"),
    ] {
        assert_cli_template_matches_tui_serializer(app_type, template, label);
    }
}

#[test]
fn provider_add_form_codex_deepseek_template_matches_upstream_preset_values() {
    let mut form = ProviderAddFormState::new(AppType::Codex);
    let existing_ids = Vec::<String>::new();

    form.apply_template(deepseek_template_index(AppType::Codex), &existing_ids);

    assert_eq!(form.id.value, "deepseek");
    assert_eq!(form.name.value, "DeepSeek");
    assert_eq!(form.website_url.value, "https://platform.deepseek.com");
    assert_eq!(form.codex_base_url.value, "https://api.deepseek.com");
    assert_eq!(form.codex_model.value, "deepseek-v4-flash");
    assert_eq!(form.codex_wire_api, CodexWireApi::Responses);
    assert!(form.codex_requires_openai_auth);

    let labels = ProviderAddFormState::new(AppType::Codex).template_labels();
    assert_eq!(
        labels.last().copied(),
        Some("DeepSeek"),
        "DeepSeek should stay after all partner presets"
    );

    let fields = form.fields();
    assert!(fields.contains(&ProviderAddField::CodexBaseUrl));
    assert!(fields.contains(&ProviderAddField::CodexModel));
    assert!(fields.contains(&ProviderAddField::CodexApiKey));

    let provider = form.to_provider_json_value();
    assert_eq!(provider["category"], "cn_official");
    assert_eq!(provider["icon"], "deepseek");
    assert_eq!(provider["iconColor"], "#1E88E5");
    assert_eq!(provider["meta"]["apiFormat"], "openai_chat");
    assert_eq!(
        provider["meta"]["codexChatReasoning"],
        json!({
            "supportsThinking": true,
            "supportsEffort": true,
            "thinkingParam": "thinking",
            "effortParam": "reasoning_effort",
            "effortValueMode": "deepseek",
            "outputFormat": "reasoning_content",
        })
    );

    let cfg = provider["settingsConfig"]["config"]
        .as_str()
        .expect("settingsConfig.config should be TOML string");
    assert!(cfg.contains("model_provider = \"custom\""));
    assert!(cfg.contains("model = \"deepseek-v4-flash\""));
    assert!(cfg.contains("disable_response_storage = true"));
    assert!(cfg.contains("[model_providers.custom]"));
    assert!(cfg.contains("name = \"deepseek\""));
    assert!(cfg.contains("base_url = \"https://api.deepseek.com\""));
    assert!(cfg.contains("wire_api = \"responses\""));
    assert!(cfg.contains("requires_openai_auth = true"));
    assert!(
        !cfg.contains("https://api.deepseek.com/v1"),
        "DeepSeek Codex preset should match upstream base URL without /v1"
    );
    assert_eq!(
        provider["settingsConfig"]["modelCatalog"],
        json!({
            "models": [
                {
                    "model": "deepseek-v4-flash",
                    "displayName": "DeepSeek V4 Flash",
                    "contextWindow": 1000000,
                },
                {
                    "model": "deepseek-v4-pro",
                    "displayName": "DeepSeek V4 Pro",
                    "contextWindow": 1000000,
                },
            ],
        })
    );
}

#[test]
fn provider_add_form_codex_oauth_template_matches_upstream_contract() {
    let mut form = ProviderAddFormState::new(AppType::Claude);
    let existing_ids = Vec::<String>::new();
    let idx = template_index_by_label(AppType::Claude, "Codex");

    form.apply_template(idx, &existing_ids);

    assert!(form.is_claude_codex_oauth_provider());
    assert_eq!(form.name.value, "Codex");
    assert_eq!(form.website_url.value, "https://openai.com/chatgpt/pricing");
    assert_eq!(
        form.claude_base_url.value,
        "https://chatgpt.com/backend-api/codex"
    );
    assert_eq!(
        form.claude_api_format,
        crate::cli::tui::form::ClaudeApiFormat::OpenAiResponses
    );
    assert_eq!(form.claude_model.value, "gpt-5.4");
    assert_eq!(form.claude_haiku_model.value, "gpt-5.4-mini");
    assert!(!form.codex_fast_mode);
    assert!(form.claude_hide_attribution);

    let fields = form.fields();
    assert!(fields.contains(&ProviderAddField::CodexOAuthAccount));
    assert!(fields.contains(&ProviderAddField::CodexFastMode));
    assert!(fields.contains(&ProviderAddField::ClaudeModelConfig));
    assert!(fields.contains(&ProviderAddField::ClaudeHideAttribution));
    assert!(!fields.contains(&ProviderAddField::ClaudeBaseUrl));
    assert!(!fields.contains(&ProviderAddField::ClaudeApiFormat));
    assert!(!fields.contains(&ProviderAddField::ClaudeApiKey));

    let provider = form.to_provider_json_value();
    assert_eq!(provider["meta"]["providerType"], "codex_oauth");
    assert_eq!(provider["meta"]["apiFormat"], "openai_responses");
    assert_eq!(provider["meta"]["codexFastMode"], false);
    assert_eq!(provider["meta"]["authBinding"]["source"], "managed_account");
    assert_eq!(
        provider["meta"]["authBinding"]["authProvider"],
        "codex_oauth"
    );
    assert!(
        provider["meta"]["authBinding"].get("accountId").is_none(),
        "default-account binding should omit accountId"
    );
    assert_eq!(
        provider["settingsConfig"]["env"]["ANTHROPIC_BASE_URL"],
        "https://chatgpt.com/backend-api/codex"
    );
    assert!(
        provider["settingsConfig"]["env"]
            .get("ANTHROPIC_AUTH_TOKEN")
            .is_none(),
        "Codex OAuth providers must not persist provider API keys"
    );
}

#[test]
fn provider_edit_form_codex_oauth_loads_account_and_fast_mode() {
    let provider_value = json!({
        "id": "codex-oauth",
        "name": "Codex",
        "settingsConfig": {
            "env": {
                "ANTHROPIC_BASE_URL": "https://chatgpt.com/backend-api/codex",
                "ANTHROPIC_MODEL": "gpt-5.4",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL": "gpt-5.4-mini"
            }
        },
        "meta": {
            "providerType": "codex_oauth",
            "apiFormat": "openai_responses",
            "codexFastMode": true,
            "authBinding": {
                "source": "managed_account",
                "authProvider": "codex_oauth",
                "accountId": "acc-123"
            }
        }
    });
    let provider: Provider = serde_json::from_value(provider_value).expect("provider json valid");

    let form = ProviderAddFormState::from_provider(AppType::Claude, &provider);

    assert!(form.is_claude_codex_oauth_provider());
    assert_eq!(form.codex_oauth_account_id.as_deref(), Some("acc-123"));
    assert!(form.codex_fast_mode);
    assert_eq!(
        form.claude_api_format,
        crate::cli::tui::form::ClaudeApiFormat::OpenAiResponses
    );
}

#[test]
fn provider_add_form_aicodemirror_preset_keeps_affiliate_register_url_in_metadata() {
    let claude_presets = super::provider_templates::provider_sponsor_presets(&AppType::Claude);
    let aicodemirror = claude_presets
        .iter()
        .find(|preset| preset.id() == "aicodemirror")
        .expect("expected AICodeMirror sponsor preset for Claude");

    assert_eq!(
        aicodemirror.register_url(),
        "https://www.aicodemirror.com/register?invitecode=77V9EA"
    );
}

#[test]
fn provider_add_form_claudeapi_preset_keeps_affiliate_register_url_in_metadata() {
    let claude_presets = super::provider_templates::provider_sponsor_presets(&AppType::Claude);
    let claudeapi = claude_presets
        .iter()
        .find(|preset| preset.id() == "claudeapi")
        .expect("expected ClaudeAPI sponsor preset for Claude");

    assert_eq!(
        claudeapi.register_url(),
        "https://console.claudeapi.com/register?source=cc-switch-cli"
    );
}

#[test]
fn provider_add_form_google_oauth_template_marks_official_metadata() {
    let mut form = ProviderAddFormState::new(AppType::Gemini);
    let existing_ids = Vec::<String>::new();
    let idx = template_index_by_label(AppType::Gemini, "Google OAuth");

    form.apply_template(idx, &existing_ids);

    let provider = form.to_provider_json_value();
    assert_eq!(provider["category"], "official");
    assert_eq!(
        provider["meta"]["partnerPromotionKey"], "google-official",
        "Google OAuth should be distinguishable from stripped custom Gemini providers"
    );
}

#[test]
fn provider_add_form_dds_preset_keeps_affiliate_register_url_in_metadata() {
    let claude_presets = super::provider_templates::provider_sponsor_presets(&AppType::Claude);
    let dds = claude_presets
        .iter()
        .find(|preset| preset.id() == "dds")
        .expect("expected DDS sponsor preset for Claude");

    assert_eq!(dds.register_url(), "https://ddshub.short.gy/ccscli");
}

#[test]
fn provider_add_form_cubence_preset_keeps_affiliate_register_url_in_metadata() {
    let claude_presets = super::provider_templates::provider_sponsor_presets(&AppType::Claude);
    let cubence = claude_presets
        .iter()
        .find(|preset| preset.id() == "cubence")
        .expect("expected Cubence sponsor preset for Claude");

    assert_eq!(
        cubence.register_url(),
        "https://cubence.com/signup?code=SC3M1CAH&source=ccscli"
    );
}

#[test]
fn provider_add_form_runapi_preset_keeps_affiliate_register_url_in_metadata() {
    let claude_presets = super::provider_templates::provider_sponsor_presets(&AppType::Claude);
    let runapi = claude_presets
        .iter()
        .find(|preset| preset.id() == "runapi")
        .expect("expected RunAPI sponsor preset for Claude");

    assert_eq!(runapi.register_url(), "https://runapi.co/register?aff=kTlB");
}

#[test]
fn provider_add_form_claudeapi_template_claude_sets_base_url_and_partner_meta() {
    let mut form = ProviderAddFormState::new(AppType::Claude);
    let existing_ids = Vec::<String>::new();

    let idx = claudeapi_template_index(AppType::Claude);
    form.apply_template(idx, &existing_ids);

    let provider = form.to_provider_json_value();
    assert_eq!(provider["name"], "ClaudeAPI");
    assert_eq!(provider["websiteUrl"], "https://console.claudeapi.com");
    assert_eq!(
        provider["settingsConfig"]["env"]["ANTHROPIC_BASE_URL"],
        "https://gw.claudeapi.com"
    );
    assert_eq!(provider["meta"]["isPartner"], true);
    assert_eq!(provider["meta"]["partnerPromotionKey"], "claudeapi");
}

#[test]
fn provider_add_form_dds_template_claude_sets_base_url_and_partner_meta() {
    let mut form = ProviderAddFormState::new(AppType::Claude);
    let existing_ids = Vec::<String>::new();

    let idx = dds_template_index(AppType::Claude);
    form.apply_template(idx, &existing_ids);

    let provider = form.to_provider_json_value();
    assert_eq!(provider["name"], "DDS");
    assert_eq!(provider["websiteUrl"], "https://www.ddshub.cc");
    assert_eq!(
        provider["settingsConfig"]["env"]["ANTHROPIC_BASE_URL"],
        "https://www.ddshub.cc"
    );
    let meta = provider["meta"]
        .as_object()
        .expect("meta should be an object");
    assert_eq!(
        meta.get("isPartner").and_then(|value| value.as_bool()),
        Some(true),
        "expected DDS sponsor to set meta.isPartner"
    );
    assert_eq!(
        meta.get("partnerPromotionKey")
            .and_then(|value| value.as_str()),
        Some("dds"),
        "expected DDS sponsor to set meta.partnerPromotionKey"
    );
}

#[test]
fn provider_add_form_dds_template_codex_sets_base_url_and_partner_meta() {
    let mut form = ProviderAddFormState::new(AppType::Codex);
    let existing_ids = Vec::<String>::new();

    let idx = dds_template_index(AppType::Codex);
    form.apply_template(idx, &existing_ids);

    let provider = form.to_provider_json_value();
    assert_eq!(provider["name"], "DDS");
    assert_eq!(provider["websiteUrl"], "https://www.ddshub.cc");
    let cfg = provider["settingsConfig"]["config"]
        .as_str()
        .expect("settingsConfig.config should be string");
    assert!(cfg.contains("base_url = \"https://www.ddshub.cc\""));
    assert!(cfg.contains("model = \"gpt-5.4\""));
    assert!(cfg.contains("wire_api = \"responses\""));
    assert!(cfg.contains("requires_openai_auth = true"));
    let meta = provider["meta"]
        .as_object()
        .expect("meta should be an object");
    assert_eq!(
        meta.get("isPartner").and_then(|value| value.as_bool()),
        Some(true),
        "expected DDS sponsor to set meta.isPartner"
    );
    assert_eq!(
        meta.get("partnerPromotionKey")
            .and_then(|value| value.as_str()),
        Some("dds"),
        "expected DDS sponsor to set meta.partnerPromotionKey"
    );
}

#[test]
fn provider_add_form_cubence_template_claude_sets_base_url_and_partner_meta() {
    let mut form = ProviderAddFormState::new(AppType::Claude);
    let existing_ids = Vec::<String>::new();

    let idx = cubence_template_index(AppType::Claude);
    form.apply_template(idx, &existing_ids);

    let provider = form.to_provider_json_value();
    assert_eq!(provider["name"], "Cubence");
    assert_eq!(provider["websiteUrl"], "https://cubence.com");
    assert_eq!(
        provider["settingsConfig"]["env"]["ANTHROPIC_BASE_URL"],
        "https://api.cubence.com"
    );
    let meta = provider["meta"]
        .as_object()
        .expect("meta should be an object");
    assert_eq!(
        meta.get("isPartner").and_then(|value| value.as_bool()),
        Some(true),
        "expected Cubence sponsor to set meta.isPartner"
    );
    assert_eq!(
        meta.get("partnerPromotionKey")
            .and_then(|value| value.as_str()),
        Some("cubence"),
        "expected Cubence sponsor to set meta.partnerPromotionKey"
    );
}

#[test]
fn provider_add_form_cubence_template_codex_sets_base_url_and_partner_meta() {
    let mut form = ProviderAddFormState::new(AppType::Codex);
    let existing_ids = Vec::<String>::new();

    let idx = cubence_template_index(AppType::Codex);
    form.apply_template(idx, &existing_ids);

    let provider = form.to_provider_json_value();
    assert_eq!(provider["name"], "Cubence");
    assert_eq!(provider["websiteUrl"], "https://cubence.com");
    let cfg = provider["settingsConfig"]["config"]
        .as_str()
        .expect("settingsConfig.config should be string");
    assert!(cfg.contains("base_url = \"https://api.cubence.com/v1\""));
    let meta = provider["meta"]
        .as_object()
        .expect("meta should be an object");
    assert_eq!(
        meta.get("isPartner").and_then(|value| value.as_bool()),
        Some(true),
        "expected Cubence sponsor to set meta.isPartner"
    );
    assert_eq!(
        meta.get("partnerPromotionKey")
            .and_then(|value| value.as_str()),
        Some("cubence"),
        "expected Cubence sponsor to set meta.partnerPromotionKey"
    );
}

#[test]
fn provider_add_form_cubence_template_gemini_sets_base_url_and_partner_meta() {
    let mut form = ProviderAddFormState::new(AppType::Gemini);

    form.apply_template(cubence_template_index(AppType::Gemini), &[]);

    let provider = form.to_provider_json_value();
    assert_eq!(provider["name"], "Cubence");
    assert_eq!(provider["websiteUrl"], "https://cubence.com");
    assert_eq!(
        provider["settingsConfig"]["env"]["GOOGLE_GEMINI_BASE_URL"],
        "https://api.cubence.com"
    );
    assert_eq!(provider["meta"]["isPartner"], true);
    assert_eq!(provider["meta"]["partnerPromotionKey"], "cubence");
}

#[test]
fn provider_add_form_cubence_template_opencode_sets_base_url_and_partner_meta() {
    let mut form = ProviderAddFormState::new(AppType::OpenCode);

    form.apply_template(cubence_template_index(AppType::OpenCode), &[]);

    let provider = form.to_provider_json_value();
    assert_eq!(provider["name"], "Cubence");
    assert_eq!(provider["websiteUrl"], "https://cubence.com");
    assert_eq!(
        provider["settingsConfig"]["npm"],
        "@ai-sdk/openai-compatible"
    );
    assert_eq!(
        provider["settingsConfig"]["options"]["baseURL"],
        "https://api.cubence.com/v1"
    );
    assert_eq!(provider["meta"]["isPartner"], true);
    assert_eq!(provider["meta"]["partnerPromotionKey"], "cubence");
}

#[test]
fn provider_add_form_cubence_template_hermes_sets_base_url_and_partner_meta() {
    let mut form = ProviderAddFormState::new(AppType::Hermes);

    form.apply_template(cubence_template_index(AppType::Hermes), &[]);

    let provider = form.to_provider_json_value();
    let settings = provider["settingsConfig"].as_object().unwrap();
    assert_eq!(provider["name"], "Cubence");
    assert_eq!(provider["websiteUrl"], "https://cubence.com");
    assert_eq!(settings.get("api_mode"), Some(&json!("chat_completions")));
    assert_eq!(
        settings.get("base_url"),
        Some(&json!("https://api.cubence.com"))
    );
    assert_eq!(provider["meta"]["isPartner"], true);
    assert_eq!(provider["meta"]["partnerPromotionKey"], "cubence");
}

#[test]
fn provider_add_form_cubence_template_openclaw_sets_base_url_and_partner_meta() {
    let mut form = ProviderAddFormState::new(AppType::OpenClaw);

    form.apply_template(cubence_template_index(AppType::OpenClaw), &[]);

    let provider = form.to_provider_json_value();
    assert_eq!(provider["name"], "Cubence");
    assert_eq!(provider["websiteUrl"], "https://cubence.com");
    assert_eq!(
        provider["settingsConfig"]["baseUrl"],
        "https://api.cubence.com"
    );
    assert_eq!(provider["settingsConfig"]["api"], "openai-completions");
    assert_eq!(provider["meta"]["isPartner"], true);
    assert_eq!(provider["meta"]["partnerPromotionKey"], "cubence");
}

#[test]
fn provider_add_form_runapi_template_claude_sets_upstream_partner_shape() {
    let mut form = ProviderAddFormState::new(AppType::Claude);

    form.apply_template(runapi_template_index(AppType::Claude), &[]);

    let provider = form.to_provider_json_value();
    assert_eq!(provider["name"], "RunAPI");
    assert_eq!(provider["websiteUrl"], "https://runapi.co");
    assert_eq!(provider["category"], "aggregator");
    assert_eq!(provider["icon"], "runapi");
    assert_eq!(
        provider["settingsConfig"]["env"]["ANTHROPIC_BASE_URL"],
        "https://runapi.co"
    );
    assert_eq!(provider["meta"]["isPartner"], true);
    assert_eq!(provider["meta"]["partnerPromotionKey"], "runapi");
}

#[test]
fn provider_add_form_runapi_template_codex_sets_v1_base_url() {
    let mut form = ProviderAddFormState::new(AppType::Codex);

    form.apply_template(runapi_template_index(AppType::Codex), &[]);

    let provider = form.to_provider_json_value();
    let cfg = provider["settingsConfig"]["config"]
        .as_str()
        .expect("settingsConfig.config should be string");
    assert_eq!(provider["name"], "RunAPI");
    assert_eq!(provider["category"], "aggregator");
    assert_eq!(provider["icon"], "runapi");
    assert!(cfg.contains("model_provider = \"runapi\""));
    assert!(cfg.contains("[model_providers.runapi]"));
    assert!(cfg.contains("base_url = \"https://runapi.co/v1\""));
    assert!(cfg.contains("model = \"gpt-5.4\""));
    assert!(cfg.contains("wire_api = \"responses\""));
    assert_eq!(provider["meta"]["partnerPromotionKey"], "runapi");
}

#[test]
fn provider_add_form_runapi_template_opencode_matches_upstream_anthropic_shape() {
    let mut form = ProviderAddFormState::new(AppType::OpenCode);

    form.apply_template(runapi_template_index(AppType::OpenCode), &[]);

    let provider = form.to_provider_json_value();
    let settings = &provider["settingsConfig"];
    assert_eq!(provider["name"], "RunAPI");
    assert_eq!(provider["category"], "aggregator");
    assert_eq!(provider["icon"], "runapi");
    assert_eq!(settings["npm"], "@ai-sdk/anthropic");
    assert_eq!(settings["name"], "RunAPI");
    assert_eq!(settings["options"]["baseURL"], "https://runapi.co");
    assert_eq!(settings["options"]["setCacheKey"], true);
    assert!(
        settings["options"].get("apiKey").is_none(),
        "blank OpenCode API keys should be omitted on save"
    );
    assert_eq!(
        settings["models"]["claude-sonnet-4-6"]["name"],
        "Claude Sonnet 4.6"
    );
    assert_eq!(
        settings["models"]["claude-opus-4-8"]["name"],
        "Claude Opus 4.8"
    );
    assert_eq!(
        settings["models"]["claude-haiku-4-5"]["name"],
        "Claude Haiku 4.5"
    );
    assert_eq!(provider["meta"]["partnerPromotionKey"], "runapi");
}

#[test]
fn provider_add_form_runapi_template_hermes_matches_upstream_anthropic_shape() {
    let mut form = ProviderAddFormState::new(AppType::Hermes);

    form.apply_template(runapi_template_index(AppType::Hermes), &[]);

    let provider = form.to_provider_json_value();
    let settings = &provider["settingsConfig"];
    assert_eq!(provider["name"], "RunAPI");
    assert_eq!(provider["category"], "aggregator");
    assert_eq!(provider["icon"], "runapi");
    assert_eq!(settings["name"], "runapi");
    assert_eq!(settings["base_url"], "https://runapi.co");
    assert_eq!(settings["api_mode"], "anthropic_messages");
    assert!(
        settings.get("api_key").is_none(),
        "blank Hermes API keys should be omitted on save"
    );
    assert_eq!(
        settings["models"],
        json!([
            { "id": "claude-opus-4-8", "name": "Claude Opus 4.8" },
            { "id": "claude-sonnet-4-6", "name": "Claude Sonnet 4.6" },
            { "id": "claude-haiku-4-5", "name": "Claude Haiku 4.5" },
        ])
    );
    assert_eq!(provider["meta"]["partnerPromotionKey"], "runapi");
}

#[test]
fn provider_add_form_runapi_template_openclaw_matches_upstream_anthropic_shape() {
    let mut form = ProviderAddFormState::new(AppType::OpenClaw);

    form.apply_template(runapi_template_index(AppType::OpenClaw), &[]);

    let provider = form.to_provider_json_value();
    let settings = &provider["settingsConfig"];
    assert_eq!(provider["name"], "RunAPI");
    assert_eq!(provider["category"], "aggregator");
    assert_eq!(provider["icon"], "runapi");
    assert_eq!(settings["baseUrl"], "https://runapi.co");
    assert_eq!(settings["api"], "anthropic-messages");
    assert!(
        settings.get("apiKey").is_none(),
        "blank OpenClaw API keys should be omitted on save"
    );
    assert_eq!(
        settings["models"],
        json!([
            {
                "id": "claude-opus-4-8",
                "name": "Claude Opus 4.8",
                "contextWindow": 1000000,
            },
            {
                "id": "claude-sonnet-4-6",
                "name": "Claude Sonnet 4.6",
                "contextWindow": 1000000,
            },
            {
                "id": "claude-haiku-4-5",
                "name": "Claude Haiku 4.5",
                "contextWindow": 200000,
            },
        ])
    );
    assert_eq!(provider["meta"]["partnerPromotionKey"], "runapi");
}

#[test]
fn provider_add_form_fields_include_notes() {
    for app_type in AppType::all() {
        let form = ProviderAddFormState::new(app_type.clone());
        let fields = form.fields();

        let website_idx = fields
            .iter()
            .position(|field| *field == ProviderAddField::WebsiteUrl)
            .expect("WebsiteUrl field should exist");
        let notes_idx = fields
            .iter()
            .position(|field| *field == ProviderAddField::Notes)
            .expect("Notes field should exist");
        assert!(
            notes_idx > website_idx,
            "Notes field should appear after WebsiteUrl for {:?}",
            app_type
        );
    }
}

#[test]
fn provider_add_form_claude_fields_include_model_config_entry() {
    let form = ProviderAddFormState::new(AppType::Claude);
    let fields = form.fields();
    let api_key_idx = fields
        .iter()
        .position(|field| *field == ProviderAddField::ClaudeApiKey)
        .expect("ClaudeApiKey field should exist");
    let model_cfg_idx = fields
        .iter()
        .position(|field| *field == ProviderAddField::ClaudeModelConfig)
        .expect("ClaudeModelConfig field should exist");
    assert!(
        model_cfg_idx > api_key_idx,
        "ClaudeModelConfig should appear after ClaudeApiKey"
    );
}

#[test]
fn provider_add_form_claude_fields_include_hide_attribution_entry() {
    let form = ProviderAddFormState::new(AppType::Claude);
    let fields = form.fields();
    let model_cfg_idx = fields
        .iter()
        .position(|field| *field == ProviderAddField::ClaudeModelConfig)
        .expect("ClaudeModelConfig field should exist");
    let hide_attribution_idx = fields
        .iter()
        .position(|field| *field == ProviderAddField::ClaudeHideAttribution)
        .expect("ClaudeHideAttribution field should exist");
    let common_divider_idx = fields
        .iter()
        .position(|field| *field == ProviderAddField::CommonConfigDivider)
        .expect("CommonConfigDivider field should exist");

    assert!(
        hide_attribution_idx > model_cfg_idx,
        "hide attribution should appear after Claude model config"
    );
    assert!(
        hide_attribution_idx < common_divider_idx,
        "hide attribution should stay with Claude-specific fields"
    );
}

#[test]
fn provider_add_form_claude_official_keeps_hide_attribution_field_visible() {
    let mut provider = Provider::with_id(
        "official".to_string(),
        "Claude Official".to_string(),
        json!({"env": {}}),
        Some("https://www.anthropic.com/claude-code".to_string()),
    );
    provider.category = Some("official".to_string());

    let form = ProviderAddFormState::from_provider(AppType::Claude, &provider);
    let fields = form.fields();

    assert!(!fields.contains(&ProviderAddField::ClaudeBaseUrl));
    assert!(!fields.contains(&ProviderAddField::ClaudeApiFormat));
    assert!(!fields.contains(&ProviderAddField::ClaudeApiKey));
    assert!(!fields.contains(&ProviderAddField::ClaudeModelConfig));
    assert!(fields.contains(&ProviderAddField::ClaudeHideAttribution));
}

#[test]
fn provider_add_form_claude_hide_attribution_writes_upstream_shape() {
    let mut form = ProviderAddFormState::new(AppType::Claude);
    form.id.set("p1");
    form.name.set("Provider One");
    form.toggle_claude_hide_attribution();

    let provider = form.to_provider_json_value();

    assert_eq!(
        provider["settingsConfig"]["attribution"],
        json!({
            "commit": "",
            "pr": ""
        })
    );
}

#[test]
fn provider_add_form_claude_hide_attribution_round_trips_and_removes_when_toggled_off() {
    let provider = Provider::with_id(
        "p1".to_string(),
        "Provider One".to_string(),
        json!({
            "env": {},
            "attribution": {
                "commit": "",
                "pr": ""
            }
        }),
        None,
    );

    let mut form = ProviderAddFormState::from_provider(AppType::Claude, &provider);
    assert!(form.claude_hide_attribution);

    form.toggle_claude_hide_attribution();
    let out = form.to_provider_json_value();

    assert!(
        out["settingsConfig"]
            .as_object()
            .is_some_and(|settings| !settings.contains_key("attribution")),
        "unchecked hide attribution should remove the upstream attribution object"
    );
}

#[test]
fn provider_add_form_claude_preserves_existing_hidden_attribution_when_untouched() {
    let provider = Provider::with_id(
        "p1".to_string(),
        "Provider One".to_string(),
        json!({
            "env": {},
            "attribution": {
                "commit": "",
                "pr": "",
                "extra": "keep"
            }
        }),
        None,
    );

    let form = ProviderAddFormState::from_provider(AppType::Claude, &provider);
    assert!(form.claude_hide_attribution);

    let out = form.to_provider_json_value();

    assert_eq!(
        out["settingsConfig"]["attribution"],
        json!({
            "commit": "",
            "pr": "",
            "extra": "keep"
        })
    );
}

#[test]
fn provider_add_form_claude_preserves_custom_attribution_when_untouched() {
    let provider = Provider::with_id(
        "p1".to_string(),
        "Provider One".to_string(),
        json!({
            "env": {},
            "attribution": {
                "commit": "custom",
                "pr": "custom"
            }
        }),
        None,
    );

    let form = ProviderAddFormState::from_provider(AppType::Claude, &provider);
    assert!(!form.claude_hide_attribution);

    let out = form.to_provider_json_value();

    assert_eq!(
        out["settingsConfig"]["attribution"],
        json!({
            "commit": "custom",
            "pr": "custom"
        })
    );
}

#[test]
fn provider_add_form_packycode_template_claude_sets_partner_meta_and_base_url() {
    let mut form = ProviderAddFormState::new(AppType::Claude);
    let existing_ids = Vec::<String>::new();

    let idx = packycode_template_index(AppType::Claude);
    form.apply_template(idx, &existing_ids);

    let provider = form.to_provider_json_value();
    assert_eq!(provider["name"], "PackyCode");
    assert_eq!(provider["websiteUrl"], "https://www.packyapi.com");
    assert_eq!(
        provider["settingsConfig"]["env"]["ANTHROPIC_BASE_URL"],
        "https://www.packyapi.com"
    );
    assert_eq!(provider["meta"]["isPartner"], true);
    assert_eq!(provider["meta"]["partnerPromotionKey"], "packycode");
}

#[test]
fn provider_add_form_packycode_template_codex_sets_partner_meta_and_base_url() {
    let mut form = ProviderAddFormState::new(AppType::Codex);
    let existing_ids = Vec::<String>::new();

    let idx = packycode_template_index(AppType::Codex);
    form.apply_template(idx, &existing_ids);

    let provider = form.to_provider_json_value();
    assert_eq!(provider["name"], "PackyCode");
    assert_eq!(provider["websiteUrl"], "https://www.packyapi.com");
    let cfg = provider["settingsConfig"]["config"]
        .as_str()
        .expect("settingsConfig.config should be string");
    assert!(cfg.contains("model_provider ="));
    assert!(cfg.contains("[model_providers."));
    assert!(cfg.contains("base_url = \"https://www.packyapi.com/v1\""));
    assert!(cfg.contains("wire_api = \"responses\""));
    assert!(cfg.contains("requires_openai_auth = true"));
    assert_eq!(provider["meta"]["isPartner"], true);
    assert_eq!(provider["meta"]["partnerPromotionKey"], "packycode");
}

#[test]
fn provider_add_form_codex_template_switch_clears_local_routing_state() {
    let mut form = ProviderAddFormState::new(AppType::Codex);
    form.claude_api_format = ClaudeApiFormat::OpenAiChat;
    form.codex_chat_reasoning.supports_thinking = Some(true);
    form.codex_chat_reasoning.supports_effort = Some(true);
    form.codex_local_routing_field_idx = 3;
    form.apply_codex_model_catalog_value(json!([
        { "model": "deepseek-chat", "displayName": "DeepSeek Chat" }
    ]))
    .expect("catalog should apply");

    form.apply_template(packycode_template_index(AppType::Codex), &[]);

    assert!(!form.codex_local_routing_enabled());
    assert_eq!(form.codex_local_routing_field_idx, 0);
    assert_eq!(
        form.codex_local_routing_fields(),
        vec![CodexLocalRoutingField::Enabled]
    );
    assert_eq!(form.codex_chat_reasoning, Default::default());
    assert!(form.codex_model_catalog.is_empty());

    let provider = form.to_provider_json_value();
    assert_eq!(provider["meta"]["apiFormat"], "openai_responses");
    assert!(provider["meta"].get("codexChatReasoning").is_none());
    assert!(provider["settingsConfig"].get("modelCatalog").is_none());

    form.claude_api_format = ClaudeApiFormat::OpenAiChat;
    form.codex_chat_reasoning.supports_thinking = Some(true);
    form.apply_codex_model_catalog_value(json!([{ "model": "qwen-coder" }]))
        .expect("catalog should apply");

    form.apply_template(1, &[]);

    assert!(form.is_codex_official_provider());
    assert!(!form.codex_local_routing_enabled());
    assert_eq!(form.codex_chat_reasoning, Default::default());
    assert!(form.codex_model_catalog.is_empty());
    let official_provider = form.to_provider_json_value();
    assert!(official_provider["meta"].get("apiFormat").is_none());
    assert!(official_provider["meta"]
        .get("codexChatReasoning")
        .is_none());
    assert!(official_provider["settingsConfig"]
        .get("modelCatalog")
        .is_none());
}

#[test]
fn provider_add_form_packycode_template_gemini_sets_partner_meta_and_base_url() {
    let mut form = ProviderAddFormState::new(AppType::Gemini);
    let existing_ids = Vec::<String>::new();

    let idx = packycode_template_index(AppType::Gemini);
    form.apply_template(idx, &existing_ids);

    let provider = form.to_provider_json_value();
    assert_eq!(provider["name"], "PackyCode");
    assert_eq!(provider["websiteUrl"], "https://www.packyapi.com");
    assert_eq!(
        provider["settingsConfig"]["env"]["GOOGLE_GEMINI_BASE_URL"],
        "https://www.packyapi.com"
    );
    assert_eq!(provider["meta"]["isPartner"], true);
    assert_eq!(provider["meta"]["partnerPromotionKey"], "packycode");
}

#[test]
fn provider_add_form_aicodemirror_template_claude_sets_partner_meta_and_base_url() {
    let mut form = ProviderAddFormState::new(AppType::Claude);

    form.apply_template(aicodemirror_template_index(AppType::Claude), &[]);

    let provider = form.to_provider_json_value();
    assert_eq!(provider["name"], "AICodeMirror");
    assert_eq!(provider["websiteUrl"], "https://www.aicodemirror.com");
    assert_eq!(
        provider["settingsConfig"]["env"]["ANTHROPIC_BASE_URL"],
        "https://api.aicodemirror.com/api/claudecode"
    );
    assert_eq!(provider["meta"]["isPartner"], true);
    assert_eq!(provider["meta"]["partnerPromotionKey"], "aicodemirror");
}

#[test]
fn provider_add_form_aicodemirror_template_codex_preserves_third_party_auth_behavior() {
    let mut form = ProviderAddFormState::new(AppType::Codex);

    form.apply_template(aicodemirror_template_index(AppType::Codex), &[]);

    let provider = form.to_provider_json_value();
    assert_eq!(provider["name"], "AICodeMirror");
    assert_eq!(provider["websiteUrl"], "https://www.aicodemirror.com");
    let cfg = provider["settingsConfig"]["config"]
        .as_str()
        .expect("settingsConfig.config should be string");
    assert!(cfg.contains("base_url = \"https://api.aicodemirror.com/api/codex/backend-api/codex\""));
    assert!(cfg.contains("model = \"gpt-5.4\""));
    assert!(cfg.contains("wire_api = \"responses\""));
    assert!(cfg.contains("requires_openai_auth = true"));
    assert_eq!(provider["meta"]["isPartner"], true);
    assert_eq!(provider["meta"]["partnerPromotionKey"], "aicodemirror");

    let fields = form.fields();
    assert!(
        fields.contains(&ProviderAddField::CodexApiKey),
        "third-party Codex presets should still show the API Key field"
    );
    assert!(
        !fields.contains(&ProviderAddField::CodexEnvKey),
        "Codex env key should stay hidden for sponsor presets"
    );
}

#[test]
fn provider_add_form_codex_custom_defaults_to_blank_base_url_and_gpt_5_4() {
    let form = ProviderAddFormState::new(AppType::Codex);

    assert_eq!(form.codex_base_url.value, "");
    assert_eq!(form.codex_model.value, "gpt-5.4");
}

#[test]
fn provider_add_form_aicodemirror_template_gemini_sets_partner_meta_and_base_url() {
    let mut form = ProviderAddFormState::new(AppType::Gemini);

    form.apply_template(aicodemirror_template_index(AppType::Gemini), &[]);

    let provider = form.to_provider_json_value();
    assert_eq!(provider["name"], "AICodeMirror");
    assert_eq!(provider["websiteUrl"], "https://www.aicodemirror.com");
    assert_eq!(
        provider["settingsConfig"]["env"]["GOOGLE_GEMINI_BASE_URL"],
        "https://api.aicodemirror.com/api/gemini"
    );
    assert_eq!(provider["meta"]["isPartner"], true);
    assert_eq!(provider["meta"]["partnerPromotionKey"], "aicodemirror");
}

#[test]
fn provider_add_form_claude_builds_env_settings() {
    let mut form = ProviderAddFormState::new(AppType::Claude);
    form.id.set("p1");
    form.name.set("Provider One");
    form.claude_api_key.set("token");
    form.claude_base_url.set("https://claude.example");

    let provider = form.to_provider_json_value();
    assert_eq!(provider["id"], "p1");
    assert_eq!(provider["name"], "Provider One");
    assert_eq!(
        provider["settingsConfig"]["env"]["ANTHROPIC_AUTH_TOKEN"],
        "token"
    );
    assert_eq!(
        provider["settingsConfig"]["env"]["ANTHROPIC_BASE_URL"],
        "https://claude.example"
    );
}

#[test]
fn provider_add_form_claude_api_format_writes_openai_chat_meta() {
    let mut form = ProviderAddFormState::new(AppType::Claude);
    form.id.set("p1");
    form.name.set("Provider One");
    form.claude_api_format = ClaudeApiFormat::OpenAiChat;

    let provider = form.to_provider_json_value();
    assert_eq!(provider["meta"]["apiFormat"], "openai_chat");
}

#[test]
fn provider_add_form_claude_api_format_restores_openai_chat_meta() {
    let mut provider = Provider::with_id(
        "p1".to_string(),
        "Provider One".to_string(),
        json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://example.com"
            }
        }),
        None,
    );
    provider.meta = Some(crate::provider::ProviderMeta {
        api_format: Some("openai_chat".to_string()),
        ..Default::default()
    });

    let form = ProviderAddFormState::from_provider(AppType::Claude, &provider);
    assert_eq!(form.claude_api_format, ClaudeApiFormat::OpenAiChat);
}

#[test]
fn provider_add_form_claude_api_format_round_trips_openai_responses_meta() {
    let mut provider = Provider::with_id(
        "p1".to_string(),
        "Provider One".to_string(),
        json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://example.com"
            }
        }),
        None,
    );
    provider.meta = Some(crate::provider::ProviderMeta {
        api_format: Some("openai_responses".to_string()),
        ..Default::default()
    });

    let form = ProviderAddFormState::from_provider(AppType::Claude, &provider);
    assert_eq!(form.claude_api_format.as_str(), "openai_responses");

    let saved = form.to_provider_json_value();
    assert_eq!(saved["meta"]["apiFormat"], "openai_responses");
}

#[test]
fn provider_add_form_claude_api_format_round_trips_gemini_native_meta() {
    let mut provider = Provider::with_id(
        "p1".to_string(),
        "Provider One".to_string(),
        json!({
            "api_format": "openai_chat",
            "apiFormat": "openai_chat",
            "openrouter_compat_mode": true,
            "env": {
                "ANTHROPIC_BASE_URL": "https://generativelanguage.googleapis.com"
            }
        }),
        None,
    );
    provider.meta = Some(crate::provider::ProviderMeta {
        api_format: Some("gemini_native".to_string()),
        is_full_url: Some(true),
        ..Default::default()
    });

    let form = ProviderAddFormState::from_provider(AppType::Claude, &provider);
    assert_eq!(form.claude_api_format, ClaudeApiFormat::GeminiNative);

    let saved = form.to_provider_json_value();
    assert_eq!(saved["meta"]["apiFormat"], "gemini_native");
    assert_eq!(saved["meta"]["isFullUrl"], true);
    assert!(saved["settingsConfig"].get("api_format").is_none());
    assert!(saved["settingsConfig"].get("apiFormat").is_none());
    assert!(saved["settingsConfig"]
        .get("openrouter_compat_mode")
        .is_none());
}

#[test]
fn provider_add_form_claude_from_provider_backfills_models_with_legacy_fallback() {
    let provider = Provider::with_id(
        "p1".to_string(),
        "Provider One".to_string(),
        json!({
            "env": {
                "ANTHROPIC_MODEL": "model-main",
                "ANTHROPIC_REASONING_MODEL": "model-reasoning",
                "ANTHROPIC_SMALL_FAST_MODEL": "model-small-fast",
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "model-sonnet-explicit",
            }
        }),
        None,
    );

    let form = ProviderAddFormState::from_provider(AppType::Claude, &provider);
    assert_eq!(form.claude_model.value, "model-main");
    assert_eq!(form.claude_reasoning_model.value, "model-reasoning");
    assert_eq!(form.claude_haiku_model.value, "model-small-fast");
    assert_eq!(form.claude_sonnet_model.value, "model-sonnet-explicit");
    assert_eq!(form.claude_opus_model.value, "model-main");
}

#[test]
fn provider_add_form_claude_writes_new_model_keys_and_removes_small_fast() {
    let mut form = ProviderAddFormState::new(AppType::Claude);
    form.id.set("p1");
    form.name.set("Provider One");
    form.extra = json!({
        "settingsConfig": {
            "env": {
                "ANTHROPIC_SMALL_FAST_MODEL": "legacy-small",
                "FOO": "bar"
            }
        }
    });
    form.claude_model.set("model-main");
    form.claude_reasoning_model.set("model-reasoning");
    form.claude_haiku_model.set("model-haiku");
    form.claude_sonnet_model.set("model-sonnet");
    form.claude_opus_model.set("model-opus");
    form.mark_claude_model_config_touched();

    let provider = form.to_provider_json_value();
    let env = provider["settingsConfig"]["env"]
        .as_object()
        .expect("settingsConfig.env should be object");
    assert_eq!(
        env.get("ANTHROPIC_MODEL").and_then(|value| value.as_str()),
        Some("model-main")
    );
    assert_eq!(
        env.get("ANTHROPIC_REASONING_MODEL")
            .and_then(|value| value.as_str()),
        Some("model-reasoning")
    );
    assert_eq!(
        env.get("ANTHROPIC_DEFAULT_HAIKU_MODEL")
            .and_then(|value| value.as_str()),
        Some("model-haiku")
    );
    assert_eq!(
        env.get("ANTHROPIC_DEFAULT_SONNET_MODEL")
            .and_then(|value| value.as_str()),
        Some("model-sonnet")
    );
    assert_eq!(
        env.get("ANTHROPIC_DEFAULT_OPUS_MODEL")
            .and_then(|value| value.as_str()),
        Some("model-opus")
    );
    assert!(env.get("ANTHROPIC_SMALL_FAST_MODEL").is_none());
    assert_eq!(env.get("FOO").and_then(|value| value.as_str()), Some("bar"));
}

#[test]
fn provider_add_form_claude_empty_model_fields_remove_env_keys() {
    let mut form = ProviderAddFormState::new(AppType::Claude);
    form.id.set("p1");
    form.name.set("Provider One");
    form.extra = json!({
        "settingsConfig": {
            "env": {
                "ANTHROPIC_MODEL": "old-main",
                "ANTHROPIC_REASONING_MODEL": "old-reasoning",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL": "old-haiku",
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "old-sonnet",
                "ANTHROPIC_DEFAULT_OPUS_MODEL": "old-opus",
                "ANTHROPIC_SMALL_FAST_MODEL": "old-small-fast",
            }
        }
    });
    form.mark_claude_model_config_touched();

    let provider = form.to_provider_json_value();
    let env = provider["settingsConfig"]["env"]
        .as_object()
        .expect("settingsConfig.env should be object");
    assert!(env.get("ANTHROPIC_MODEL").is_none());
    assert!(env.get("ANTHROPIC_REASONING_MODEL").is_none());
    assert!(env.get("ANTHROPIC_DEFAULT_HAIKU_MODEL").is_none());
    assert!(env.get("ANTHROPIC_DEFAULT_SONNET_MODEL").is_none());
    assert!(env.get("ANTHROPIC_DEFAULT_OPUS_MODEL").is_none());
    assert!(env.get("ANTHROPIC_SMALL_FAST_MODEL").is_none());
}

#[test]
fn provider_add_form_claude_untouched_model_popup_keeps_model_keys() {
    let provider = Provider::with_id(
        "p1".to_string(),
        "Provider One".to_string(),
        json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token-old",
                "ANTHROPIC_BASE_URL": "https://claude.example",
                "ANTHROPIC_MODEL": "model-main",
                "ANTHROPIC_SMALL_FAST_MODEL": "model-small-fast",
            }
        }),
        None,
    );

    let mut form = ProviderAddFormState::from_provider(AppType::Claude, &provider);
    form.name.set("Provider One Updated");

    let out = form.to_provider_json_value();
    let env = out["settingsConfig"]["env"]
        .as_object()
        .expect("settingsConfig.env should be object");
    assert_eq!(
        env.get("ANTHROPIC_MODEL").and_then(|value| value.as_str()),
        Some("model-main")
    );
    assert_eq!(
        env.get("ANTHROPIC_SMALL_FAST_MODEL")
            .and_then(|value| value.as_str()),
        Some("model-small-fast")
    );
    assert_eq!(
        env.get("ANTHROPIC_DEFAULT_HAIKU_MODEL")
            .and_then(|value| value.as_str()),
        None
    );
    assert_eq!(
        env.get("ANTHROPIC_DEFAULT_SONNET_MODEL")
            .and_then(|value| value.as_str()),
        None
    );
    assert_eq!(
        env.get("ANTHROPIC_DEFAULT_OPUS_MODEL")
            .and_then(|value| value.as_str()),
        None
    );
}

#[test]
fn provider_add_form_codex_builds_full_toml_config() {
    let mut form = ProviderAddFormState::new(AppType::Codex);
    form.id.set("c1");
    form.name.set("Codex Provider");
    form.codex_base_url.set("https://api.openai.com/v1");
    form.codex_model.set("gpt-5.4");
    form.codex_api_key.set("sk-test");

    let provider = form.to_provider_json_value();
    assert_eq!(
        provider["settingsConfig"]["auth"]["OPENAI_API_KEY"],
        "sk-test"
    );
    let cfg = provider["settingsConfig"]["config"]
        .as_str()
        .expect("settingsConfig.config should be string");
    assert!(cfg.contains("model_provider ="));
    assert!(cfg.contains("[model_providers."));
    assert!(cfg.contains("base_url = \"https://api.openai.com/v1\""));
    assert!(cfg.contains("model = \"gpt-5.4\""));
    assert!(cfg.contains("wire_api = \"responses\""));
    assert!(cfg.contains("requires_openai_auth = true"));
    assert!(cfg.contains("disable_response_storage = true"));
}

#[test]
fn provider_add_form_codex_preserves_existing_config_toml_custom_keys() {
    let provider = crate::provider::Provider::with_id(
        "c1".to_string(),
        "Codex Provider".to_string(),
        json!({
            "auth": {
                "OPENAI_API_KEY": "sk-test"
            },
            "config": r#"
model_provider = "custom"
model = "gpt-5.2-codex"
network_access = true

[model_providers.custom]
name = "custom"
base_url = "https://api.example.com/v1"
wire_api = "responses"
requires_openai_auth = true
"#,
        }),
        None,
    );

    let mut form = ProviderAddFormState::from_provider(AppType::Codex, &provider);
    form.codex_base_url.set("https://changed.example/v1");

    let out = form.to_provider_json_value();
    let cfg = out["settingsConfig"]["config"]
        .as_str()
        .expect("settingsConfig.config should be string");
    assert!(
        cfg.contains("network_access = true"),
        "existing Codex config.toml keys should be preserved"
    );
    assert!(
        cfg.contains("base_url = \"https://changed.example/v1\""),
        "Codex base_url form field should still update config.toml"
    );
}

#[test]
fn provider_add_form_codex_custom_includes_api_key_and_hides_advanced_fields() {
    let form = ProviderAddFormState::new(AppType::Codex);
    let fields = form.fields();

    assert!(
        fields.contains(&ProviderAddField::CodexApiKey),
        "custom Codex provider should include API Key field"
    );
    assert!(
        fields.contains(&ProviderAddField::CodexLocalRouting),
        "custom Codex provider should expose Local Routing on its secondary page"
    );
    assert!(
        !fields.contains(&ProviderAddField::ClaudeApiFormat),
        "custom Codex provider should not expose the old API Format selector"
    );
    assert!(
        !fields.contains(&ProviderAddField::CodexWireApi),
        "Codex wire_api should not be configurable in the UI"
    );
    assert!(
        !fields.contains(&ProviderAddField::CodexRequiresOpenaiAuth),
        "Codex auth mode should not be configurable in the UI"
    );
    assert!(
        !fields.contains(&ProviderAddField::CodexEnvKey),
        "Codex env key should not be configurable in the UI"
    );
}

#[test]
fn provider_add_form_codex_local_routing_writes_meta_without_chat_wire_api() {
    let mut form = ProviderAddFormState::new(AppType::Codex);
    form.id.set("custom");
    form.name.set("Custom");
    form.codex_base_url.set("https://api.example.com/v1");
    form.codex_model.set("deepseek-chat");
    form.claude_api_format = ClaudeApiFormat::OpenAiChat;

    let provider = form.to_provider_json_value();
    let config = provider["settingsConfig"]["config"]
        .as_str()
        .expect("Codex config should be serialized");

    assert_eq!(provider["meta"]["apiFormat"], "openai_chat");
    assert!(
        config.contains("wire_api = \"responses\""),
        "Codex wire_api should stay Responses; meta.apiFormat controls local route mapping"
    );
    assert!(
        !config.contains("wire_api = \"chat\""),
        "TUI should not persist Chat as the Codex wire_api"
    );
}

#[test]
fn provider_add_form_codex_local_routing_is_off_by_default_and_persisted() {
    let mut form = ProviderAddFormState::new(AppType::Codex);
    form.id.set("custom");
    form.name.set("Custom");

    let provider = form.to_provider_json_value();

    assert!(!form.codex_local_routing_enabled());
    assert_eq!(provider["meta"]["apiFormat"], "openai_responses");
}

#[test]
fn provider_add_form_codex_local_routing_restores_meta_chat_format() {
    let mut provider = Provider::with_id(
        "custom".to_string(),
        "Custom".to_string(),
        json!({
            "config": r#"
model_provider = "custom"
model = "deepseek-chat"

[model_providers.custom]
name = "custom"
base_url = "https://api.example.com/v1"
wire_api = "responses"
requires_openai_auth = true
"#,
        }),
        None,
    );
    provider.meta = Some(crate::provider::ProviderMeta {
        api_format: Some("openai_chat".to_string()),
        ..Default::default()
    });

    let form = ProviderAddFormState::from_provider(AppType::Codex, &provider);

    assert!(form.codex_local_routing_enabled());
}

#[test]
fn provider_add_form_codex_legacy_chat_wire_api_loads_as_local_route_mapping() {
    let provider = Provider::with_id(
        "custom".to_string(),
        "Custom".to_string(),
        json!({
            "config": r#"
model_provider = "custom"
model = "deepseek-chat"

[model_providers.custom]
name = "custom"
base_url = "https://api.example.com/v1"
wire_api = "chat"
requires_openai_auth = true
"#,
        }),
        None,
    );

    let form = ProviderAddFormState::from_provider(AppType::Codex, &provider);
    let saved = form.to_provider_json_value();
    let config = saved["settingsConfig"]["config"]
        .as_str()
        .expect("Codex config should be serialized");

    assert!(form.codex_local_routing_enabled());
    assert_eq!(saved["meta"]["apiFormat"], "openai_chat");
    assert!(config.contains("wire_api = \"responses\""));
    assert!(!config.contains("wire_api = \"chat\""));
}

#[test]
fn provider_add_form_codex_local_routing_saves_normalized_reasoning() {
    let mut form = ProviderAddFormState::new(AppType::Codex);
    form.id.set("custom");
    form.name.set("Custom");
    form.codex_base_url.set("https://api.example.com/v1");
    form.claude_api_format = ClaudeApiFormat::OpenAiChat;

    form.toggle_codex_reasoning_effort();

    let provider = form.to_provider_json_value();
    let reasoning = &provider["meta"]["codexChatReasoning"];

    assert_eq!(reasoning["supportsThinking"], true);
    assert_eq!(reasoning["supportsEffort"], true);
    assert_eq!(reasoning["thinkingParam"], "thinking");
    assert_eq!(reasoning["effortParam"], "reasoning_effort");
    assert_eq!(reasoning["effortValueMode"], "passthrough");
    assert_eq!(reasoning["outputFormat"], "auto");
}

#[test]
fn provider_add_form_codex_responses_removes_reasoning_and_model_catalog() {
    let mut provider = Provider::with_id(
        "custom".to_string(),
        "Custom".to_string(),
        json!({
            "config": r#"
model_provider = "custom"
model = "deepseek-chat"

[model_providers.custom]
name = "custom"
base_url = "https://api.example.com/v1"
wire_api = "responses"
requires_openai_auth = true
"#,
            "modelCatalog": {
                "models": [
                    { "model": "deepseek-chat", "displayName": "DeepSeek Chat" }
                ]
            }
        }),
        None,
    );
    provider.meta = Some(crate::provider::ProviderMeta {
        api_format: Some("openai_chat".to_string()),
        codex_chat_reasoning: Some(crate::provider::CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    });

    let mut form = ProviderAddFormState::from_provider(AppType::Codex, &provider);
    form.toggle_codex_local_routing_enabled();

    let saved = form.to_provider_json_value();

    assert_eq!(saved["meta"]["apiFormat"], "openai_responses");
    assert!(saved["meta"].get("codexChatReasoning").is_none());
    assert!(saved["settingsConfig"].get("modelCatalog").is_none());
}

#[test]
fn provider_add_form_codex_model_catalog_saves_normalized_models_and_syncs_primary_model() {
    let mut form = ProviderAddFormState::new(AppType::Codex);
    form.id.set("custom");
    form.name.set("Custom");
    form.codex_base_url.set("https://api.example.com/v1");
    form.codex_model.set("fallback-model");
    form.claude_api_format = ClaudeApiFormat::OpenAiChat;
    form.apply_codex_model_catalog_value(json!([
        { "model": " deepseek-chat ", "displayName": " DeepSeek Chat ", "contextWindow": "128000 tokens" },
        { "model": "deepseek-chat", "displayName": "Duplicate" },
        { "model": "kimi-k2", "contextWindow": "256k" },
        { "model": "qwen-coder", "contextWindow": "invalid" },
        { "model": "" }
    ]))
    .expect("catalog should apply");

    let saved = form.to_provider_json_value();
    let config = saved["settingsConfig"]["config"]
        .as_str()
        .expect("Codex config should be serialized");
    let models = saved["settingsConfig"]["modelCatalog"]["models"]
        .as_array()
        .expect("modelCatalog.models should be an array");

    assert_eq!(models.len(), 3);
    assert_eq!(models[0]["model"], "deepseek-chat");
    assert_eq!(models[0]["displayName"], "DeepSeek Chat");
    assert_eq!(models[0]["contextWindow"], 128000);
    assert_eq!(models[1]["model"], "kimi-k2");
    assert_eq!(models[1]["contextWindow"], 256000);
    assert_eq!(models[2]["model"], "qwen-coder");
    assert!(models[2].get("contextWindow").is_none());
    assert!(
        config.contains("model = \"deepseek-chat\""),
        "first normalized catalog model should become the active Codex model"
    );
    assert!(config.contains("wire_api = \"responses\""));
}

#[test]
fn provider_add_form_codex_openai_official_hides_provider_specific_fields() {
    let mut form = ProviderAddFormState::new(AppType::Codex);
    let existing_ids = Vec::<String>::new();

    form.apply_template(1, &existing_ids);

    assert_eq!(form.website_url.value, "https://chatgpt.com/codex");
    let fields = form.fields();
    assert!(
        !fields.contains(&ProviderAddField::CodexBaseUrl),
        "official Codex provider should not expose Base URL input"
    );
    assert!(
        !fields.contains(&ProviderAddField::CodexModel),
        "official Codex provider should not expose model override input"
    );
    assert!(
        !fields.contains(&ProviderAddField::CodexApiKey),
        "official Codex provider should preserve auth.json snapshots instead of rewriting them via API Key"
    );
}

#[test]
fn provider_add_form_claude_official_sets_upstream_website_and_hides_non_official_fields() {
    let mut form = ProviderAddFormState::new(AppType::Claude);
    let existing_ids = Vec::<String>::new();

    form.apply_template(1, &existing_ids);

    assert_eq!(
        form.website_url.value,
        "https://www.anthropic.com/claude-code"
    );
    assert_eq!(form.claude_base_url.value, "");

    let fields = form.fields();
    assert!(
        !fields.contains(&ProviderAddField::ClaudeBaseUrl),
        "official Claude provider should not show Base URL input"
    );
    assert!(
        !fields.contains(&ProviderAddField::ClaudeApiFormat),
        "official Claude provider should not show API format input"
    );
    assert!(
        !fields.contains(&ProviderAddField::ClaudeApiKey),
        "official Claude provider should not require API Key input"
    );
    assert!(
        !fields.contains(&ProviderAddField::ClaudeModelConfig),
        "official Claude provider should not show model override input"
    );
}

#[test]
fn provider_add_form_claude_official_save_preserves_existing_env_keys_like_upstream() {
    let mut provider = Provider::with_id(
        "claude-official".to_string(),
        "Claude Official".to_string(),
        json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token-old",
                "ANTHROPIC_BASE_URL": "https://relay.example",
                "ANTHROPIC_MODEL": "model-main",
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "model-sonnet"
            }
        }),
        None,
    );
    provider.website_url = Some("https://www.anthropic.com/claude-code".to_string());
    provider.category = Some("official".to_string());
    provider.meta = Some(crate::provider::ProviderMeta {
        api_format: Some("openai_chat".to_string()),
        ..Default::default()
    });

    let form = ProviderAddFormState::from_provider(AppType::Claude, &provider);
    let out = form.to_provider_json_value();
    let env = out["settingsConfig"]["env"]
        .as_object()
        .expect("settingsConfig.env should be object");
    let meta = out.get("meta").and_then(|value| value.as_object());

    assert_eq!(
        env.get("ANTHROPIC_AUTH_TOKEN")
            .and_then(|value| value.as_str()),
        Some("token-old")
    );
    assert_eq!(
        env.get("ANTHROPIC_BASE_URL")
            .and_then(|value| value.as_str()),
        Some("https://relay.example")
    );
    assert_eq!(
        env.get("ANTHROPIC_MODEL").and_then(|value| value.as_str()),
        Some("model-main")
    );
    assert_eq!(
        env.get("ANTHROPIC_DEFAULT_SONNET_MODEL")
            .and_then(|value| value.as_str()),
        Some("model-sonnet")
    );
    assert!(
        meta.is_none_or(|meta| meta.get("apiFormat").is_none()),
        "official Claude provider should not serialize a default apiFormat"
    );
    assert_eq!(out["category"], "official");
}

#[test]
fn provider_add_form_claude_without_official_category_keeps_third_party_fields_visible() {
    let mut provider = Provider::with_id(
        "claude-official-like".to_string(),
        "Claude Official".to_string(),
        json!({"env": {"ANTHROPIC_BASE_URL": "https://relay.example"}}),
        Some("https://www.anthropic.com/claude-code".to_string()),
    );
    provider.category = None;

    let form = ProviderAddFormState::from_provider(AppType::Claude, &provider);
    let fields = form.fields();

    assert!(fields.contains(&ProviderAddField::ClaudeBaseUrl));
    assert!(fields.contains(&ProviderAddField::ClaudeApiFormat));
    assert!(fields.contains(&ProviderAddField::ClaudeApiKey));
    assert!(fields.contains(&ProviderAddField::ClaudeModelConfig));
}

#[test]
fn provider_add_form_codex_packycode_hides_env_key_field() {
    let mut form = ProviderAddFormState::new(AppType::Codex);
    let existing_ids = Vec::<String>::new();

    let idx = packycode_template_index(AppType::Codex);
    form.apply_template(idx, &existing_ids);

    let fields = form.fields();
    assert!(
        fields.contains(&ProviderAddField::CodexApiKey),
        "PackyCode Codex provider should include API Key field"
    );
    assert!(
        !fields.contains(&ProviderAddField::CodexEnvKey),
        "Codex env key should not be configurable for PackyCode"
    );
}

#[test]
fn provider_add_form_codex_official_roundtrip_preserves_auth_and_strips_provider_config() {
    let mut provider = Provider::with_id(
        "codex-official".to_string(),
        "OpenAI Official".to_string(),
        json!({
            "auth": {
                "access_token": "oauth-token",
                "refresh_token": "refresh-token"
            },
            "config": "model_provider = \"openai\"\nmodel = \"gpt-5.2-codex\"\nmodel_reasoning_effort = \"high\"\n\n[model_providers.openai]\nbase_url = \"https://api.openai.com/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
        }),
        Some("https://chatgpt.com/codex".to_string()),
    );
    provider.category = Some("official".to_string());
    provider.meta = Some(crate::provider::ProviderMeta {
        codex_official: Some(true),
        ..Default::default()
    });

    let form = ProviderAddFormState::from_provider(AppType::Codex, &provider);
    let fields = form.fields();
    assert!(
        !fields.contains(&ProviderAddField::CodexBaseUrl),
        "official Codex provider should stay out of the third-party endpoint flow"
    );

    let out = form.to_provider_json_value();
    assert_eq!(
        out["settingsConfig"]["auth"], provider.settings_config["auth"],
        "official Codex provider should keep the stored auth snapshot"
    );
    assert_eq!(
        out["settingsConfig"]["config"], "model_reasoning_effort = \"high\"",
        "official Codex provider should drop provider-level base_url/model settings on save"
    );
}

#[test]
fn provider_add_form_codex_official_seed_roundtrip_keeps_empty_auth_and_config() {
    let mut form = ProviderAddFormState::new(AppType::Codex);
    let existing_ids = Vec::<String>::new();

    form.apply_template(1, &existing_ids);

    let out = form.to_provider_json_value();
    assert_eq!(out["settingsConfig"]["auth"], json!({}));
    assert_eq!(out["settingsConfig"]["config"], "");
}

#[test]
fn provider_add_form_gemini_builds_env_settings() {
    let mut form = ProviderAddFormState::new(AppType::Gemini);
    form.id.set("g1");
    form.name.set("Gemini Provider");
    form.gemini_auth_type = GeminiAuthType::ApiKey;
    form.gemini_api_key.set("AIza...");
    form.gemini_base_url
        .set("https://generativelanguage.googleapis.com");

    let provider = form.to_provider_json_value();
    assert_eq!(
        provider["settingsConfig"]["env"]["GEMINI_API_KEY"],
        "AIza..."
    );
    assert_eq!(
        provider["settingsConfig"]["env"]["GOOGLE_GEMINI_BASE_URL"],
        "https://generativelanguage.googleapis.com"
    );
}

#[test]
fn provider_add_form_gemini_includes_model_in_env_when_set() {
    let mut form = ProviderAddFormState::new(AppType::Gemini);
    form.id.set("g1");
    form.name.set("Gemini Provider");
    form.gemini_auth_type = GeminiAuthType::ApiKey;
    form.gemini_api_key.set("AIza...");
    form.gemini_base_url
        .set("https://generativelanguage.googleapis.com");
    form.gemini_model.set("gemini-3-pro-preview");

    let provider = form.to_provider_json_value();
    assert_eq!(
        provider["settingsConfig"]["env"]["GEMINI_MODEL"],
        "gemini-3-pro-preview"
    );
}

#[test]
fn provider_add_form_gemini_oauth_does_not_include_model_or_api_key_env() {
    let mut form = ProviderAddFormState::new(AppType::Gemini);
    form.id.set("g1");
    form.name.set("Gemini Provider");
    form.gemini_auth_type = GeminiAuthType::OAuth;
    form.gemini_model.set("gemini-3-pro-preview");

    let provider = form.to_provider_json_value();
    let env = provider["settingsConfig"]["env"]
        .as_object()
        .expect("settingsConfig.env should be an object");
    assert!(env.get("GEMINI_API_KEY").is_none());
    assert!(env.get("GOOGLE_GEMINI_BASE_URL").is_none());
    assert!(env.get("GEMINI_BASE_URL").is_none());
    assert!(env.get("GEMINI_MODEL").is_none());
}

#[test]
fn mcp_add_form_builds_server_and_apps() {
    let mut form = McpAddFormState::new();
    form.id.set("m1");
    form.name.set("Server One");
    form.command.set("npx");
    form.args
        .set("-y @modelcontextprotocol/server-filesystem /tmp");
    form.apps.claude = true;
    form.apps.codex = false;
    form.apps.gemini = true;
    form.apps.hermes = true;

    let server = form.to_mcp_server_json_value();
    assert_eq!(server["id"], "m1");
    assert_eq!(server["name"], "Server One");
    assert_eq!(server["server"]["type"], "stdio");
    assert_eq!(server["server"]["command"], "npx");
    assert_eq!(server["server"]["args"][0], "-y");
    assert_eq!(server["apps"]["claude"], true);
    assert_eq!(server["apps"]["codex"], false);
    assert_eq!(server["apps"]["gemini"], true);
    assert_eq!(server["apps"]["opencode"], false);
    assert_eq!(server["apps"]["hermes"], true);
}

#[test]
fn mcp_add_form_builds_http_server_without_stdio_fields() {
    let mut form = McpAddFormState::new();
    form.id.set("docs-langchain");
    form.name.set("LangChain Docs");
    form.server_type = McpTransport::Http;
    form.command.set("ignored");
    form.args.set("--ignored");
    form.url.set("https://docs.langchain.com/mcp");

    let server = form.to_mcp_server_json_value();
    assert_eq!(server["server"]["type"], "http");
    assert_eq!(server["server"]["url"], "https://docs.langchain.com/mcp");
    assert!(
        server["server"].get("command").is_none(),
        "http MCP should not serialize stdio command"
    );
    assert!(
        server["server"].get("args").is_none(),
        "http MCP should not serialize stdio args"
    );
}

#[test]
fn mcp_add_form_builds_sse_server_without_stdio_fields() {
    let mut form = McpAddFormState::new();
    form.id.set("remote-sse");
    form.name.set("Remote SSE");
    form.server_type = McpTransport::Sse;
    form.command.set("ignored");
    form.url.set("https://example.com/sse");

    let server = form.to_mcp_server_json_value();
    assert_eq!(server["server"]["type"], "sse");
    assert_eq!(server["server"]["url"], "https://example.com/sse");
    assert!(server["server"].get("command").is_none());
}

#[test]
fn mcp_env_form_restores_sorted_rows() {
    let server = crate::app_config::McpServer {
        id: "m1".to_string(),
        name: "Server One".to_string(),
        server: json!({
            "command": "npx",
            "args": ["-y", "@scope/server"],
            "env": {
                "Z_TOKEN": "tail",
                "A_TOKEN": ""
            }
        }),
        apps: crate::app_config::McpApps::default(),
        description: None,
        homepage: None,
        docs: None,
        tags: Vec::new(),
    };

    let form = McpAddFormState::from_server(&server);

    assert_eq!(
        form.env_rows,
        vec![
            McpEnvVarRow {
                key: "A_TOKEN".to_string(),
                value: "".to_string(),
            },
            McpEnvVarRow {
                key: "Z_TOKEN".to_string(),
                value: "tail".to_string(),
            },
        ]
    );
}

#[test]
fn mcp_env_form_serializes_rows_and_skips_empty_object() {
    let mut form = McpAddFormState::new();
    form.id.set("m1");
    form.name.set("Server One");
    form.command.set("npx");
    form.env_rows = vec![
        McpEnvVarRow {
            key: "API_KEY".to_string(),
            value: "secret".to_string(),
        },
        McpEnvVarRow {
            key: "PROJECT_ROOT".to_string(),
            value: "".to_string(),
        },
    ];

    let saved = form.to_mcp_server_json_value();
    assert_eq!(saved["server"]["env"]["API_KEY"], "secret");
    assert_eq!(saved["server"]["env"]["PROJECT_ROOT"], "");

    form.env_rows.clear();
    let saved = form.to_mcp_server_json_value();
    assert!(
        saved["server"].get("env").is_none(),
        "empty env rows should remove server.env instead of serializing {{}}"
    );
}

#[test]
fn mcp_env_form_summary_uses_none_one_and_many_copy() {
    let mut form = McpAddFormState::new();
    assert_eq!(form.env_summary(), crate::cli::i18n::texts::none());

    form.env_rows.push(McpEnvVarRow {
        key: "API_KEY".to_string(),
        value: "secret".to_string(),
    });
    assert_eq!(
        form.env_summary(),
        crate::cli::i18n::texts::tui_mcp_env_entry_count(1)
    );

    form.env_rows.push(McpEnvVarRow {
        key: "PROJECT_ROOT".to_string(),
        value: "".to_string(),
    });
    assert_eq!(
        form.env_summary(),
        crate::cli::i18n::texts::tui_mcp_env_entry_count(2)
    );
}

#[test]
fn mcp_env_form_places_env_between_args_and_apps() {
    let form = McpAddFormState::new();
    let fields = form.fields();
    assert!(
        fields.contains(&McpAddField::Env),
        "MCP form fields should expose Env section"
    );

    let args_idx = fields
        .iter()
        .position(|field| *field == McpAddField::Args)
        .expect("MCP Args field should exist");
    let env_idx = fields
        .iter()
        .position(|field| *field == McpAddField::Env)
        .expect("MCP Env field should exist");
    let app_claude_idx = fields
        .iter()
        .position(|field| *field == McpAddField::AppClaude)
        .expect("MCP AppClaude field should exist");

    assert!(
        args_idx < env_idx && env_idx < app_claude_idx,
        "MCP Env field should appear between Args and AppClaude"
    );
    assert!(form.input(McpAddField::Env).is_none());
}

#[test]
fn mcp_http_form_replaces_stdio_fields_with_url() {
    let mut form = McpAddFormState::new();
    form.server_type = McpTransport::Http;

    let fields = form.fields();
    assert!(fields.contains(&McpAddField::Type));
    assert!(fields.contains(&McpAddField::Url));
    assert!(!fields.contains(&McpAddField::Command));
    assert!(!fields.contains(&McpAddField::Args));
    assert!(!fields.contains(&McpAddField::Env));
    assert!(fields.contains(&McpAddField::AppOpenCode));
    assert!(fields.contains(&McpAddField::AppHermes));
}

#[test]
fn mcp_form_restores_remote_server_type_and_url() {
    let server = crate::app_config::McpServer {
        id: "docs-langchain".to_string(),
        name: "LangChain Docs".to_string(),
        server: json!({
            "type": "http",
            "url": "https://docs.langchain.com/mcp",
            "headers": {
                "Authorization": "Bearer token"
            }
        }),
        apps: crate::app_config::McpApps::default(),
        description: None,
        homepage: None,
        docs: None,
        tags: Vec::new(),
    };

    let form = McpAddFormState::from_server(&server);
    assert_eq!(form.server_type, McpTransport::Http);
    assert_eq!(form.url.value, "https://docs.langchain.com/mcp");

    let roundtrip = form.to_mcp_server_json_value();
    assert_eq!(roundtrip["server"]["type"], "http");
    assert_eq!(roundtrip["server"]["url"], "https://docs.langchain.com/mcp");
    assert_eq!(
        roundtrip["server"]["headers"]["Authorization"],
        "Bearer token"
    );
}

#[test]
fn mcp_form_infers_http_only_when_url_has_no_type() {
    let server = crate::app_config::McpServer {
        id: "docs-langchain".to_string(),
        name: "LangChain Docs".to_string(),
        server: json!({
            "url": "https://docs.langchain.com/mcp"
        }),
        apps: crate::app_config::McpApps::default(),
        description: None,
        homepage: None,
        docs: None,
        tags: Vec::new(),
    };

    let form = McpAddFormState::from_server(&server);
    assert_eq!(form.server_type, McpTransport::Http);

    let server = crate::app_config::McpServer {
        server: json!({
            "type": "websocket",
            "url": "https://docs.langchain.com/mcp"
        }),
        ..server
    };

    let form = McpAddFormState::from_server(&server);
    assert_eq!(form.server_type, McpTransport::Stdio);
}

#[test]
fn provider_add_form_switching_back_to_custom_clears_template_values() {
    let mut form = ProviderAddFormState::new(AppType::Claude);
    let existing_ids = Vec::<String>::new();

    form.apply_template(1, &existing_ids);
    assert_eq!(form.name.value, "Claude Official");
    assert_eq!(
        form.website_url.value,
        "https://www.anthropic.com/claude-code"
    );
    assert_eq!(form.claude_base_url.value, "");
    assert_eq!(form.id.value, "claude-official");

    form.apply_template(0, &existing_ids);
    assert_eq!(form.name.value, "");
    assert_eq!(form.website_url.value, "");
    assert_eq!(form.claude_base_url.value, "");
    assert_eq!(form.id.value, "");
}

#[test]
fn mcp_add_form_switching_back_to_custom_clears_template_values() {
    let mut form = McpAddFormState::new();
    form.id.set("m1");

    form.apply_template(1);
    assert_eq!(form.name.value, "Filesystem");
    assert_eq!(form.command.value, "npx");
    assert!(form
        .args
        .value
        .contains("@modelcontextprotocol/server-filesystem"));

    form.apply_template(0);
    assert_eq!(form.id.value, "m1");
    assert_eq!(form.name.value, "");
    assert_eq!(form.command.value, "");
    assert_eq!(form.args.value, "");
}

#[test]
fn form_state_has_unsaved_changes_is_clean_on_open() {
    let provider_form = FormState::ProviderAdd(ProviderAddFormState::new(AppType::Claude));
    assert!(!provider_form.has_unsaved_changes());

    let mcp_form = FormState::McpAdd(McpAddFormState::new());
    assert!(!mcp_form.has_unsaved_changes());
}

#[test]
fn provider_add_form_has_unsaved_changes_after_edit() {
    let mut form = ProviderAddFormState::new(AppType::Claude);
    assert!(!form.has_unsaved_changes());

    form.name.set("Provider One");
    assert!(form.has_unsaved_changes());
}

#[test]
fn mcp_add_form_has_unsaved_changes_after_env_edit() {
    let mut form = McpAddFormState::new();
    assert!(!form.has_unsaved_changes());

    form.upsert_env_row(None, "API_KEY".to_string(), "secret".to_string());
    assert!(form.has_unsaved_changes());
}

#[test]
fn provider_add_form_has_unsaved_changes_roundtrip_edit_returns_clean() {
    let provider = Provider::with_id(
        "p1".to_string(),
        "Provider One".to_string(),
        json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token"
            }
        }),
        None,
    );
    let mut form = ProviderAddFormState::from_provider(AppType::Claude, &provider);
    assert!(!form.has_unsaved_changes());

    let original_name = form.name.value.clone();
    form.name.set("Provider Two");
    assert!(form.has_unsaved_changes());

    form.name.set(original_name);
    assert!(!form.has_unsaved_changes());
}

#[test]
fn provider_add_form_common_config_json_merges_into_preview_but_not_raw_submit_payload() {
    let mut form = ProviderAddFormState::new(AppType::Claude);
    form.id.set("p1");
    form.name.set("Provider One");
    form.include_common_config = true;
    form.claude_base_url.set("https://provider.example");
    form.claude_api_key.set("sk-provider");

    let raw = form.to_provider_json_value();
    let raw_settings = raw
        .get("settingsConfig")
        .expect("settingsConfig should exist");

    assert!(
        raw_settings.get("alwaysThinkingEnabled").is_none(),
        "raw submit payload should not include common snippet scalar keys"
    );
    assert_eq!(
        raw_settings["env"]["ANTHROPIC_BASE_URL"], "https://provider.example",
        "raw submit payload should still include provider-specific fields"
    );
    assert_eq!(raw_settings["env"]["ANTHROPIC_AUTH_TOKEN"], "sk-provider");

    let merged = form
        .to_provider_json_value_with_common_config(
            r#"{
                "alwaysThinkingEnabled": false,
                "env": {
                    "ANTHROPIC_BASE_URL": "https://common.example",
                    "COMMON_FLAG": "1"
                }
            }"#,
        )
        .expect("common config should merge");
    let settings = merged
        .get("settingsConfig")
        .expect("settingsConfig should exist");

    assert_eq!(settings["alwaysThinkingEnabled"], false);
    assert_eq!(settings["env"]["COMMON_FLAG"], "1");
    assert_eq!(
        settings["env"]["ANTHROPIC_BASE_URL"], "https://common.example",
        "common config should follow backend merge precedence"
    );
    assert_eq!(settings["env"]["ANTHROPIC_AUTH_TOKEN"], "sk-provider");
    assert_eq!(merged["meta"]["commonConfigEnabled"], true);
}

#[test]
fn provider_add_form_removes_legacy_apply_common_config_alias_from_meta() {
    let mut form = ProviderAddFormState::new(AppType::Claude);
    form.id.set("p1");
    form.name.set("Provider One");
    form.include_common_config = true;
    form.extra = json!({
        "meta": {
            "applyCommonConfig": false,
            "endpointAutoSelect": true
        }
    });

    let provider = form.to_provider_json_value();
    let meta = provider["meta"].as_object().expect("meta should be object");

    assert_eq!(meta.get("commonConfigEnabled"), Some(&json!(true)));
    assert!(
        !meta.contains_key("applyCommonConfig"),
        "legacy alias should be removed before serializing the provider"
    );
    assert_eq!(meta.get("endpointAutoSelect"), Some(&json!(true)));
}

#[test]
fn provider_add_form_opencode_preview_matches_raw_submit_payload_when_common_snippet_exists() {
    let mut form = ProviderAddFormState::new(AppType::OpenCode);
    form.id.set("p1");
    form.name.set("Provider One");
    form.include_common_config = true;
    form.opencode_npm_package.set("@ai-sdk/openai-compatible");
    form.opencode_api_key.set("sk-provider");
    form.opencode_base_url.set("https://provider.example/v1");
    form.opencode_model_id.set("gpt-4.1-mini");

    let raw = form.to_provider_json_value();
    let preview = form
        .to_provider_json_value_with_common_config(
            r#"{
                "apiKey": "sk-common",
                "baseURL": "https://common.example/v1"
            }"#,
        )
        .expect("OpenCode preview should accept object common snippet");

    assert_eq!(preview, raw, "OpenCode preview should match the raw submit payload because live save does not apply the common snippet");
}

#[test]
fn provider_add_form_apply_provider_json_updates_fields_and_preserves_include_toggle() {
    let mut form = ProviderAddFormState::new(AppType::Claude);
    form.include_common_config = false;
    form.extra = json!({
        "category": "custom"
    });

    let parsed = Provider::with_id(
        "json-id".to_string(),
        "JSON Provider".to_string(),
        json!({
            "alwaysThinkingEnabled": false,
            "env": {
                "ANTHROPIC_BASE_URL": "https://json.example"
            }
        }),
        Some("https://site.example".to_string()),
    );

    form.apply_provider_json_to_fields(&parsed);

    assert_eq!(form.id.value, "json-id");
    assert_eq!(form.name.value, "JSON Provider");
    assert_eq!(form.website_url.value, "https://site.example");
    assert_eq!(form.claude_base_url.value, "https://json.example");
    assert!(
        !form.include_common_config,
        "include_common_config should be preserved when editor JSON omits meta.applyCommonConfig"
    );
    assert_eq!(form.extra["category"], "custom");
    assert_eq!(form.extra["settingsConfig"]["alwaysThinkingEnabled"], false);
}

#[test]
fn provider_add_form_defaults_common_config_from_effective_snippet_only() {
    let no_snippet = ProviderAddFormState::new(AppType::Claude);
    assert!(
        !no_snippet.include_common_config,
        "new provider should not attach common config when no usable snippet exists"
    );

    let comment_only_codex = ProviderAddFormState::new_with_common_snippet(
        AppType::Codex,
        "# comments do not create an effective snippet\n",
    );
    assert!(
        !comment_only_codex.include_common_config,
        "comment-only Codex snippets should not default-enable common config"
    );

    let with_snippet = ProviderAddFormState::new_with_common_snippet(
        AppType::Claude,
        r#"{"permissions":{"allow":["Bash"]}}"#,
    );
    assert!(
        with_snippet.include_common_config,
        "new provider should attach common config when a usable snippet exists"
    );
}

#[test]
fn provider_edit_form_preserves_missing_common_config_meta_until_toggle() {
    let provider = Provider::with_id(
        "legacy-provider".to_string(),
        "Legacy Provider".to_string(),
        json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://provider.example"
            }
        }),
        None,
    );

    let mut form = ProviderAddFormState::from_provider(AppType::Claude, &provider);
    let raw = form.to_provider_json_value();
    assert!(
        raw.get("meta")
            .and_then(|meta| meta.get("commonConfigEnabled"))
            .is_none(),
        "editing a missing-meta provider should preserve upstream missing-meta semantics"
    );

    form.name.set("Renamed Provider");
    let renamed = form.to_provider_json_value();
    assert!(
        renamed
            .get("meta")
            .and_then(|meta| meta.get("commonConfigEnabled"))
            .is_none(),
        "unrelated edits must not force commonConfigEnabled=true"
    );

    form.toggle_include_common_config(
        r#"{"env":{"ANTHROPIC_BASE_URL":"https://provider.example"}}"#,
    )
    .expect("toggle should succeed");
    let toggled = form.to_provider_json_value();
    assert_eq!(toggled["meta"]["commonConfigEnabled"], true);
}

#[test]
fn provider_edit_form_missing_meta_inferrs_common_config_from_subset() {
    let provider = Provider::with_id(
        "legacy-provider".to_string(),
        "Legacy Provider".to_string(),
        json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://provider.example",
                "CC_SWITCH_SHARED": "1"
            }
        }),
        None,
    );

    let form = ProviderAddFormState::from_provider_with_common_snippet(
        AppType::Claude,
        &provider,
        r#"{"env":{"CC_SWITCH_SHARED":"1"}}"#,
    );

    assert!(
        form.include_common_config,
        "missing-meta edit should infer common config usage when provider settings contain the snippet"
    );
    let raw = form.to_provider_json_value();
    assert!(
        raw.get("meta")
            .and_then(|meta| meta.get("commonConfigEnabled"))
            .is_none(),
        "inferred missing-meta usage should not force explicit meta until the user toggles"
    );
}

#[test]
fn provider_edit_form_preserves_other_meta_without_forcing_common_config_meta() {
    let mut provider = Provider::with_id(
        "provider-with-meta".to_string(),
        "Provider With Meta".to_string(),
        json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://provider.example"
            }
        }),
        None,
    );
    provider.meta = Some(crate::provider::ProviderMeta {
        endpoint_auto_select: Some(true),
        ..Default::default()
    });

    let form = ProviderAddFormState::from_provider(AppType::Claude, &provider);
    let raw = form.to_provider_json_value();

    assert_eq!(raw["meta"]["endpointAutoSelect"], true);
    assert!(
        raw["meta"].get("commonConfigEnabled").is_none(),
        "preserving unrelated meta should not synthesize commonConfigEnabled"
    );
}

#[test]
fn provider_edit_form_apply_provider_json_keeps_locked_id() {
    let original = Provider::with_id(
        "locked-id".to_string(),
        "Original".to_string(),
        json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://before.example"
            }
        }),
        None,
    );
    let mut form = ProviderAddFormState::from_provider(AppType::Claude, &original);

    let edited = Provider::with_id(
        "changed-id".to_string(),
        "Edited Name".to_string(),
        json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://after.example"
            }
        }),
        None,
    );

    form.apply_provider_json_to_fields(&edited);

    assert_eq!(form.id.value, "locked-id");
    assert_eq!(form.name.value, "Edited Name");
    assert_eq!(form.claude_base_url.value, "https://after.example");
}

#[test]
fn provider_add_form_disabling_common_config_strips_common_fields_from_json() {
    let mut form = ProviderAddFormState::new(AppType::Claude);
    form.id.set("p1");
    form.name.set("Provider One");
    form.include_common_config = true;

    let parsed = Provider::with_id(
        "p1".to_string(),
        "Provider One".to_string(),
        json!({
            "alwaysThinkingEnabled": false,
            "statusLine": {
                "type": "command",
                "command": "~/.claude/statusline.sh",
                "padding": 0
            },
            "env": {
                "ANTHROPIC_BASE_URL": "https://provider.example"
            }
        }),
        None,
    );
    form.apply_provider_json_to_fields(&parsed);

    let common = r#"{
        "alwaysThinkingEnabled": false,
        "statusLine": {
            "type": "command",
            "command": "~/.claude/statusline.sh",
            "padding": 0
        }
    }"#;
    form.toggle_include_common_config(common)
        .expect("toggle should succeed");

    assert!(
        !form.include_common_config,
        "toggle should disable include_common_config"
    );
    let provider = form.to_provider_json_value();
    let settings = provider
        .get("settingsConfig")
        .expect("settingsConfig should exist");
    assert!(
        settings.get("alwaysThinkingEnabled").is_none(),
        "common scalar field should be removed after disabling common config"
    );
    assert!(
        settings.get("statusLine").is_none(),
        "common nested field should be removed after disabling common config"
    );
}

#[test]
fn provider_add_form_disabling_common_config_preserves_provider_specific_env_keys() {
    let mut form = ProviderAddFormState::new(AppType::Claude);
    form.id.set("p1");
    form.name.set("Provider One");
    form.include_common_config = true;

    let parsed = Provider::with_id(
        "p1".to_string(),
        "Provider One".to_string(),
        json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://common.example",
                "ANTHROPIC_AUTH_TOKEN": "sk-provider"
            }
        }),
        None,
    );
    form.apply_provider_json_to_fields(&parsed);

    form.toggle_include_common_config(r#"{"env":{"ANTHROPIC_BASE_URL":"https://common.example"}}"#)
        .expect("toggle should succeed");

    let provider = form.to_provider_json_value();
    let env = provider
        .get("settingsConfig")
        .and_then(|settings| settings.get("env"))
        .and_then(|value| value.as_object())
        .expect("env should exist");

    assert!(
        env.get("ANTHROPIC_BASE_URL").is_none(),
        "common env keys should be removed"
    );
    assert_eq!(
        env.get("ANTHROPIC_AUTH_TOKEN")
            .and_then(|value| value.as_str()),
        Some("sk-provider"),
        "provider-specific env keys should be preserved"
    );
}

#[test]
fn provider_add_form_opencode_exposes_supported_sponsor_presets() {
    let form = ProviderAddFormState::new(AppType::OpenCode);
    let labels = form.template_labels();

    assert_eq!(
        labels,
        vec!["Custom", "* Cubence", "* RunAPI", "* AICodeMirror"]
    );
    assert!(
        !labels.contains(&"* PackyCode"),
        "OpenCode should expose only explicitly supported sponsor presets"
    );
}

#[test]
fn provider_add_form_openclaw_uses_dedicated_template_defs() {
    let openclaw_defs =
        super::provider_templates::provider_builtin_template_defs(&AppType::OpenClaw);
    let opencode_defs =
        super::provider_templates::provider_builtin_template_defs(&AppType::OpenCode);
    let openclaw_labels = ProviderAddFormState::new(AppType::OpenClaw).template_labels();

    assert_eq!(
        openclaw_labels,
        vec!["Custom", "* Cubence", "* RunAPI", "* AICodeMirror"]
    );
    assert!(
        !std::ptr::eq(openclaw_defs, opencode_defs),
        "OpenClaw should keep its own template mapping instead of aliasing OpenCode"
    );
}

#[test]
fn provider_add_form_hermes_exposes_upstream_provider_fields_only() {
    let form = ProviderAddFormState::new(AppType::Hermes);
    let fields = form.fields();

    assert_eq!(
        fields,
        vec![
            ProviderAddField::Id,
            ProviderAddField::Name,
            ProviderAddField::WebsiteUrl,
            ProviderAddField::Notes,
            ProviderAddField::HermesApiMode,
            ProviderAddField::HermesBaseUrl,
            ProviderAddField::HermesApiKey,
            ProviderAddField::HermesModels,
            ProviderAddField::HermesAdvancedDivider,
            ProviderAddField::HermesRateLimitDelay,
            ProviderAddField::UsageQueryDivider,
            ProviderAddField::UsageQuery,
        ]
    );
    assert!(
        !fields.contains(&ProviderAddField::CommonSnippet),
        "Hermes provider form should not expose common config controls"
    );
}

#[test]
fn provider_add_form_hermes_rate_limit_delay_is_editable() {
    let mut form = ProviderAddFormState::new(AppType::Hermes);
    let fields = form.fields();
    assert!(fields.contains(&ProviderAddField::HermesRateLimitDelay));
    assert!(form.input(ProviderAddField::HermesRateLimitDelay).is_some());
    assert!(form
        .input_mut(ProviderAddField::HermesRateLimitDelay)
        .is_some());
    assert!(
        form.input(ProviderAddField::HermesAdvancedDivider)
            .is_none(),
        "Hermes advanced divider must not be editable"
    );

    form.hermes_rate_limit_delay.set("0.5");
    assert_eq!(
        form.input(ProviderAddField::HermesRateLimitDelay)
            .map(|input| input.value.as_str()),
        Some("0.5")
    );
}

#[test]
fn provider_add_form_hermes_builds_upstream_snake_case_settings() {
    let mut form = ProviderAddFormState::new(AppType::Hermes);
    form.id.set("openrouter");
    form.name.set("OpenRouter");
    form.hermes_api_mode = "anthropic_messages".to_string();
    form.hermes_base_url
        .set(" https://openrouter.ai/api/v1/// ");
    form.hermes_api_key.set(" sk-or-test ");
    form.hermes_models = vec![json!({
        "id": "anthropic/claude-opus-4-7",
        "name": "Claude Opus 4.7",
        "context_length": 1000000,
    })];
    form.hermes_rate_limit_delay.set("0.5");

    let provider = form.to_provider_json_value();
    let settings = provider["settingsConfig"].as_object().unwrap();
    assert_eq!(settings.get("api_mode"), Some(&json!("anthropic_messages")));
    assert_eq!(
        settings.get("base_url"),
        Some(&json!("https://openrouter.ai/api/v1"))
    );
    assert_eq!(settings.get("api_key"), Some(&json!("sk-or-test")));
    assert_eq!(settings.get("rate_limit_delay"), Some(&json!(0.5)));
    assert_eq!(settings["models"][0]["id"], "anthropic/claude-opus-4-7");
    for legacy_key in ["api", "apiKey", "apiMode", "baseUrl", "baseURL", "endpoint"] {
        assert!(
            !settings.contains_key(legacy_key),
            "Hermes save should drop legacy alias {legacy_key}"
        );
    }
}

#[test]
fn provider_add_form_hermes_omits_optional_blank_values_but_writes_default_mode() {
    let mut form = ProviderAddFormState::new(AppType::Hermes);
    form.id.set("custom");
    form.name.set("Custom Hermes");

    let provider = form.to_provider_json_value();
    let settings = provider["settingsConfig"].as_object().unwrap();
    assert_eq!(settings.get("api_mode"), Some(&json!("chat_completions")));
    assert!(settings.get("base_url").is_none());
    assert!(settings.get("api_key").is_none());
    assert!(settings.get("models").is_none());
    assert!(settings.get("rate_limit_delay").is_none());
}

#[test]
fn provider_add_form_hermes_loads_legacy_aliases_and_saves_canonical_shape() {
    let provider = Provider::with_id(
        "legacy".to_string(),
        "Legacy Hermes".to_string(),
        json!({
            "apiMode": "bedrock_converse",
            "baseUrl": "https://legacy.example/v1",
            "apiKey": "sk-legacy",
            "api": "openai-completions",
            "models": [
                { "id": "legacy-model", "name": "Legacy Model" }
            ],
        }),
        None,
    );

    let form = ProviderAddFormState::from_provider(AppType::Hermes, &provider);
    assert_eq!(form.hermes_api_mode_value(), "bedrock_converse");
    assert_eq!(form.hermes_base_url.value, "https://legacy.example/v1");
    assert_eq!(form.hermes_api_key.value, "sk-legacy");
    assert_eq!(form.hermes_models[0]["id"], "legacy-model");

    let roundtrip = form.to_provider_json_value();
    let settings = roundtrip["settingsConfig"].as_object().unwrap();
    assert_eq!(settings.get("api_mode"), Some(&json!("bedrock_converse")));
    assert_eq!(
        settings.get("base_url"),
        Some(&json!("https://legacy.example/v1"))
    );
    assert_eq!(settings.get("api_key"), Some(&json!("sk-legacy")));
    assert!(settings.get("api").is_none());
    assert!(settings.get("apiMode").is_none());
    assert!(settings.get("baseUrl").is_none());
    assert!(settings.get("apiKey").is_none());
}

#[test]
fn provider_add_form_aicodemirror_template_opencode_matches_serializer_and_loader_semantics() {
    let mut form = ProviderAddFormState::new(AppType::OpenCode);

    form.apply_template(aicodemirror_template_index(AppType::OpenCode), &[]);

    let provider = form.to_provider_json_value();
    assert_eq!(provider["name"], "AICodeMirror");
    assert_eq!(provider["websiteUrl"], "https://www.aicodemirror.com");
    assert_eq!(provider["meta"]["isPartner"], true);
    assert_eq!(provider["meta"]["partnerPromotionKey"], "aicodemirror");
    assert_eq!(provider["settingsConfig"]["npm"], "@ai-sdk/anthropic");
    assert!(
        provider["settingsConfig"]["options"]
            .get("apiKey")
            .is_none(),
        "blank OpenCode API keys should be omitted on save"
    );
    assert_eq!(
        provider["settingsConfig"]["options"]["baseURL"],
        "https://api.aicodemirror.com/api/claudecode"
    );
    assert_eq!(
        provider["settingsConfig"]["models"]["claude-sonnet-4.6"]["name"],
        "Claude Sonnet 4.6"
    );
    assert_eq!(
        provider["settingsConfig"]["models"]["claude-opus-4.6"]["name"],
        "Claude Opus 4.6"
    );

    let mut parsed = Provider::with_id(
        "opencode-aicodemirror".to_string(),
        "AICodeMirror".to_string(),
        provider["settingsConfig"].clone(),
        Some("https://www.aicodemirror.com".to_string()),
    );
    parsed.meta = Some(crate::provider::ProviderMeta {
        is_partner: Some(true),
        partner_promotion_key: Some("aicodemirror".to_string()),
        ..Default::default()
    });

    let roundtrip_form = ProviderAddFormState::from_provider(AppType::OpenCode, &parsed);
    assert_eq!(
        roundtrip_form.opencode_npm_package.value,
        "@ai-sdk/anthropic"
    );
    assert!(roundtrip_form.opencode_api_key.value.is_empty());
    assert_eq!(
        roundtrip_form.opencode_base_url.value,
        "https://api.aicodemirror.com/api/claudecode"
    );
    assert_eq!(roundtrip_form.opencode_model_id.value, "claude-opus-4.6");
    assert_eq!(roundtrip_form.opencode_model_name.value, "Claude Opus 4.6");

    let roundtrip = roundtrip_form.to_provider_json_value();
    assert!(
        roundtrip["settingsConfig"]["options"]
            .get("apiKey")
            .is_none(),
        "OpenCode roundtrip should still omit blank API keys"
    );
    assert_eq!(
        roundtrip["settingsConfig"]["models"]["claude-sonnet-4.6"]["name"],
        "Claude Sonnet 4.6"
    );
    assert_eq!(
        roundtrip["settingsConfig"]["models"]["claude-opus-4.6"]["name"],
        "Claude Opus 4.6"
    );
}

#[test]
fn provider_add_form_openclaw_switching_to_custom_resets_openclaw_specific_fields() {
    let mut form = ProviderAddFormState::new(AppType::OpenClaw);

    form.id.set("oclaw1");
    form.name.set("OpenClaw Provider");
    form.opencode_npm_package.set("anthropic-messages");
    form.opencode_api_key.set("sk-openclaw");
    form.opencode_base_url
        .set("https://api.openclaw.example/v1");
    form.openclaw_user_agent = true;
    form.openclaw_models = vec![json!({
        "id": "primary-model",
        "name": "Primary Model",
        "contextWindow": 128000,
    })];

    form.apply_template(0, &[]);

    assert_eq!(form.id.value, "");
    assert_eq!(form.name.value, "");
    assert_eq!(
        form.opencode_npm_package.value,
        OPENCLAW_DEFAULT_API_PROTOCOL
    );
    assert_eq!(form.opencode_api_key.value, "");
    assert_eq!(form.opencode_base_url.value, "");
    assert!(!form.openclaw_user_agent);
    assert!(form.openclaw_models.is_empty());
}

#[test]
fn provider_add_form_opencode_includes_dedicated_fields() {
    let form = ProviderAddFormState::new(AppType::OpenCode);
    let fields = form.fields();

    assert!(
        fields.len() > 6,
        "OpenCode should expose dedicated provider/model fields instead of only common metadata"
    );
    assert!(
        !fields.contains(&ProviderAddField::CommonSnippet),
        "OpenCode should not expose common config controls; upstream common config is limited to Claude/Codex/Gemini"
    );
}

#[test]
fn provider_add_form_opencode_builds_settings_from_dedicated_fields() {
    let mut form = ProviderAddFormState::new(AppType::OpenCode);
    form.id.set("oc1");
    form.name.set("OpenCode Provider");
    form.opencode_npm_package.set("@ai-sdk/openai-compatible");
    form.opencode_api_key.set("sk-oc");
    form.opencode_base_url.set("https://api.example.com/v1");
    form.opencode_model_id.set("gpt-4.1-mini");
    form.opencode_model_name.set("GPT 4.1 Mini");
    form.opencode_model_context_limit.set("128000");
    form.opencode_model_output_limit.set("8192");

    let provider = form.to_provider_json_value();
    assert_eq!(provider["id"], "oc1");
    assert_eq!(
        provider["settingsConfig"]["npm"],
        "@ai-sdk/openai-compatible"
    );
    assert_eq!(provider["settingsConfig"]["options"]["apiKey"], "sk-oc");
    assert_eq!(
        provider["settingsConfig"]["options"]["baseURL"],
        "https://api.example.com/v1"
    );
    assert_eq!(
        provider["settingsConfig"]["models"]["gpt-4.1-mini"]["name"],
        "GPT 4.1 Mini"
    );
    assert_eq!(
        provider["settingsConfig"]["models"]["gpt-4.1-mini"]["limit"]["context"],
        128000
    );
    assert_eq!(
        provider["settingsConfig"]["models"]["gpt-4.1-mini"]["limit"]["output"],
        8192
    );
}

#[test]
fn provider_add_form_opencode_from_provider_backfills_and_preserves_extra_settings() {
    let provider = Provider::with_id(
        "oc1".to_string(),
        "OpenCode Provider".to_string(),
        json!({
            "npm": "@ai-sdk/openai-compatible",
            "options": {
                "apiKey": "sk-oc",
                "baseURL": "https://api.example.com/v1",
                "headers": {
                    "X-Test": "1"
                },
                "timeout": 30
            },
            "models": {
                "gpt-4.1-mini": {
                    "name": "GPT 4.1 Mini",
                    "limit": {
                        "context": 128000,
                        "output": 8192
                    },
                    "options": {
                        "reasoningEffort": "medium"
                    }
                },
                "gpt-4.1": {
                    "name": "GPT 4.1"
                }
            }
        }),
        Some("https://provider.example".to_string()),
    );

    let form = ProviderAddFormState::from_provider(AppType::OpenCode, &provider);
    assert_eq!(form.opencode_npm_package.value, "@ai-sdk/openai-compatible");
    assert_eq!(form.opencode_api_key.value, "sk-oc");
    assert_eq!(form.opencode_base_url.value, "https://api.example.com/v1");
    assert_eq!(form.opencode_model_id.value, "gpt-4.1-mini");
    assert_eq!(form.opencode_model_name.value, "GPT 4.1 Mini");
    assert_eq!(form.opencode_model_context_limit.value, "128000");
    assert_eq!(form.opencode_model_output_limit.value, "8192");

    let roundtrip = form.to_provider_json_value();
    assert_eq!(
        roundtrip["settingsConfig"]["options"]["headers"]["X-Test"],
        "1"
    );
    assert_eq!(roundtrip["settingsConfig"]["options"]["timeout"], 30);
    assert_eq!(
        roundtrip["settingsConfig"]["models"]["gpt-4.1"]["name"],
        "GPT 4.1"
    );
    assert_eq!(
        roundtrip["settingsConfig"]["models"]["gpt-4.1-mini"]["options"]["reasoningEffort"],
        "medium"
    );
}

#[test]
fn provider_add_form_openclaw_exposes_minimal_dedicated_fields() {
    let form = ProviderAddFormState::new(AppType::OpenClaw);
    let fields = form.fields();

    assert_eq!(fields.first(), Some(&ProviderAddField::Id));
    assert!(fields.contains(&ProviderAddField::OpenClawApiProtocol));
    assert!(fields.contains(&ProviderAddField::OpenCodeApiKey));
    assert!(fields.contains(&ProviderAddField::OpenCodeBaseUrl));
    assert!(fields.contains(&ProviderAddField::OpenClawUserAgent));
    assert!(fields.contains(&ProviderAddField::OpenClawModels));
    assert!(
        !fields.contains(&ProviderAddField::OpenCodeModelOutputLimit),
        "OpenClaw should not expose the OpenCode-only output limit field"
    );
    assert!(
        !fields.contains(&ProviderAddField::OpenCodeNpmPackage),
        "OpenClaw should expose a dedicated API protocol picker instead of the OpenCode npm field"
    );
    assert!(
        !fields.contains(&ProviderAddField::OpenCodeModelId),
        "OpenClaw should use a dedicated models editor instead of a single primary model id field"
    );
    assert!(
        !fields.contains(&ProviderAddField::OpenCodeModelName),
        "OpenClaw should use a dedicated models editor instead of a single primary model name field"
    );
    assert!(
        !fields.contains(&ProviderAddField::OpenCodeModelContextLimit),
        "OpenClaw should use a dedicated models editor instead of a single primary context field"
    );
    assert!(
        !fields.contains(&ProviderAddField::CommonConfigDivider),
        "OpenClaw should not expose the Common Config block"
    );
    assert!(
        !fields.contains(&ProviderAddField::CommonSnippet),
        "OpenClaw should not expose the Common Config editor"
    );
    assert!(
        !fields.contains(&ProviderAddField::IncludeCommonConfig),
        "OpenClaw should not expose the Common Config toggle"
    );
}

#[test]
fn provider_add_form_openclaw_generates_upstream_provider_key() {
    let mut form = ProviderAddFormState::new(AppType::OpenClaw);
    form.name.set("OpenClaw Provider");

    assert!(form.ensure_generated_id(&[]));
    assert_eq!(form.id.value, "openclaw-provider");
}

#[test]
fn provider_edit_form_openclaw_keeps_provider_key_visible_but_locked() {
    let provider = Provider::with_id(
        "openclaw-provider".to_string(),
        "OpenClaw Provider".to_string(),
        json!({
            "baseUrl": "https://api.openclaw.example/v1"
        }),
        None,
    );

    let form = ProviderAddFormState::from_provider(AppType::OpenClaw, &provider);

    assert!(
        form.fields().contains(&ProviderAddField::Id),
        "OpenClaw edit form should still show provider key"
    );
    assert!(
        !form.is_id_editable(),
        "editing an existing OpenClaw provider should keep provider key immutable"
    );
}

#[test]
fn provider_copy_form_additive_apps_hide_provider_key() {
    for app_type in [AppType::OpenClaw, AppType::Hermes] {
        let provider = Provider::with_id(
            "source-provider".to_string(),
            "Source Provider".to_string(),
            json!({
                "baseUrl": "https://api.example/v1",
                "apiKey": "sk-demo",
            }),
            None,
        );

        let form = ProviderAddFormState::copy_from_provider_with_common_snippet(
            app_type.clone(),
            &provider,
            "",
            &[],
        );
        let fields = form.fields();

        assert_eq!(form.copy_source_id.as_deref(), Some("source-provider"));
        assert!(
            !fields.contains(&ProviderAddField::Id),
            "{app_type:?} copy form should not expose a provider key that is regenerated on save"
        );
        assert!(
            !form.is_id_editable(),
            "{app_type:?} copy form should not allow editing the regenerated provider key"
        );
    }
}

#[test]
fn provider_add_form_openclaw_uses_upstream_default_api_protocol() {
    let mut form = ProviderAddFormState::new(AppType::OpenClaw);
    form.id.set("oclaw1");
    form.name.set("OpenClaw Provider");
    form.opencode_api_key.set("sk-openclaw");
    form.opencode_base_url
        .set("https://api.openclaw.example/v1");

    let provider = form.to_provider_json_value();
    assert_eq!(provider["settingsConfig"]["apiKey"], "sk-openclaw");
    assert_eq!(
        provider["settingsConfig"]["baseUrl"],
        "https://api.openclaw.example/v1"
    );
    assert_eq!(provider["settingsConfig"]["api"], "openai-completions");
}

#[test]
fn provider_add_form_aicodemirror_template_openclaw_matches_serializer_and_loader_semantics() {
    let mut form = ProviderAddFormState::new(AppType::OpenClaw);

    form.apply_template(aicodemirror_template_index(AppType::OpenClaw), &[]);

    let provider = form.to_provider_json_value();
    assert_eq!(provider["name"], "AICodeMirror");
    assert_eq!(provider["websiteUrl"], "https://www.aicodemirror.com");
    assert_eq!(provider["meta"]["isPartner"], true);
    assert_eq!(provider["meta"]["partnerPromotionKey"], "aicodemirror");
    assert!(
        provider["settingsConfig"].get("apiKey").is_none(),
        "blank OpenClaw API keys should be omitted on save"
    );
    assert_eq!(
        provider["settingsConfig"]["baseUrl"],
        "https://api.aicodemirror.com/api/claudecode"
    );
    assert_eq!(provider["settingsConfig"]["api"], "anthropic-messages");
    assert_eq!(
        provider["settingsConfig"]["models"],
        json!([
            {
                "id": "claude-opus-4-6",
                "name": "Claude Opus 4.6",
                "contextWindow": 200000,
                "cost": {
                    "input": 5,
                    "output": 25
                }
            },
            {
                "id": "claude-sonnet-4-6",
                "name": "Claude Sonnet 4.6",
                "contextWindow": 200000,
                "cost": {
                    "input": 3,
                    "output": 15
                }
            }
        ])
    );

    let mut parsed = Provider::with_id(
        "openclaw-aicodemirror".to_string(),
        "AICodeMirror".to_string(),
        provider["settingsConfig"].clone(),
        Some("https://www.aicodemirror.com".to_string()),
    );
    parsed.meta = Some(crate::provider::ProviderMeta {
        is_partner: Some(true),
        partner_promotion_key: Some("aicodemirror".to_string()),
        ..Default::default()
    });

    let roundtrip_form = ProviderAddFormState::from_provider(AppType::OpenClaw, &parsed);
    assert_eq!(
        roundtrip_form.opencode_npm_package.value,
        "anthropic-messages"
    );
    assert!(roundtrip_form.opencode_api_key.value.is_empty());
    assert_eq!(
        roundtrip_form.opencode_base_url.value,
        "https://api.aicodemirror.com/api/claudecode"
    );
    assert_eq!(roundtrip_form.opencode_model_id.value, "claude-opus-4-6");
    assert_eq!(roundtrip_form.opencode_model_name.value, "Claude Opus 4.6");
    assert_eq!(roundtrip_form.opencode_model_context_limit.value, "200000");

    let roundtrip = roundtrip_form.to_provider_json_value();
    assert!(
        roundtrip["settingsConfig"].get("apiKey").is_none(),
        "OpenClaw roundtrip should still omit blank API keys"
    );
    assert_eq!(
        roundtrip["settingsConfig"]["models"],
        provider["settingsConfig"]["models"]
    );
}

#[test]
fn provider_add_form_openclaw_roundtrip_restores_protocol_and_user_agent_toggle() {
    let provider = Provider::with_id(
        "oclaw1".to_string(),
        "OpenClaw Provider".to_string(),
        json!({
            "api": "anthropic-messages",
            "apiKey": "sk-openclaw",
            "baseUrl": "https://api.openclaw.example/v1",
            "headers": {
                "User-Agent": "Mozilla/5.0 custom",
                "X-Test": "1"
            },
            "models": [
                {
                    "id": "primary-model",
                    "name": "Primary Model",
                    "contextWindow": 128000
                }
            ]
        }),
        None,
    );

    let form = ProviderAddFormState::from_provider(AppType::OpenClaw, &provider);
    assert_eq!(form.opencode_npm_package.value, "anthropic-messages");
    assert!(
        form.openclaw_user_agent,
        "OpenClaw form should restore the User-Agent toggle from headers"
    );

    let roundtrip = form.to_provider_json_value();
    assert_eq!(roundtrip["settingsConfig"]["api"], "anthropic-messages");
    assert_eq!(
        roundtrip["settingsConfig"]["headers"]["User-Agent"],
        "Mozilla/5.0 custom"
    );
    assert_eq!(roundtrip["settingsConfig"]["headers"]["X-Test"], "1");
}

#[test]
fn provider_add_form_openclaw_enabling_user_agent_adds_default_header() {
    let mut form = ProviderAddFormState::new(AppType::OpenClaw);
    form.id.set("oclaw1");
    form.name.set("OpenClaw Provider");
    form.openclaw_user_agent = true;

    let provider = form.to_provider_json_value();
    assert_eq!(provider["settingsConfig"]["api"], "openai-completions");
    assert_eq!(
        provider["settingsConfig"]["headers"]["User-Agent"],
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:148.0) Gecko/20100101 Firefox/148.0"
    );
}

#[test]
fn provider_add_form_openclaw_from_provider_preserves_other_models_on_roundtrip() {
    let provider = Provider::with_id(
        "oclaw1".to_string(),
        "OpenClaw Provider".to_string(),
        json!({
            "api": "openai-responses",
            "apiKey": "sk-openclaw",
            "baseUrl": "https://api.openclaw.example/v1",
            "headers": {
                "X-Test": "1"
            },
            "models": [
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
            ]
        }),
        None,
    );

    let form = ProviderAddFormState::from_provider(AppType::OpenClaw, &provider);
    assert_eq!(form.opencode_npm_package.value, "openai-responses");
    assert_eq!(form.opencode_model_id.value, "primary-model");
    assert_eq!(form.opencode_model_name.value, "Primary Model");
    assert_eq!(form.opencode_model_context_limit.value, "128000");

    let roundtrip = form.to_provider_json_value();
    let models = roundtrip["settingsConfig"]["models"]
        .as_array()
        .expect("OpenClaw models should remain an array");
    assert_eq!(
        models.len(),
        2,
        "editing one model should not drop the others"
    );
    assert_eq!(models[1]["id"], "fallback-model");
    assert_eq!(models[0]["providerHint"], "reasoning");
    assert_eq!(roundtrip["settingsConfig"]["headers"]["X-Test"], "1");
}

#[test]
fn provider_add_form_openclaw_roundtrip_preserves_unknown_model_fields() {
    let provider = Provider::with_id(
        "oclaw1".to_string(),
        "OpenClaw Provider".to_string(),
        json!({
            "api": "openai-responses",
            "models": [
                {
                    "id": "gpt-4.1",
                    "name": "GPT-4.1",
                    "reasoning": true,
                    "input": ["text", "image"],
                    "contextWindow": 128000,
                    "maxTokens": 8192,
                    "cost": {
                        "input": 2.0,
                        "output": 8.0,
                        "cacheRead": 1.0,
                        "cacheWrite": 4.0
                    }
                }
            ]
        }),
        None,
    );

    let form = ProviderAddFormState::from_provider(AppType::OpenClaw, &provider);
    let roundtrip = form.to_provider_json_value();

    assert_eq!(roundtrip["settingsConfig"]["models"][0]["reasoning"], true);
    assert_eq!(
        roundtrip["settingsConfig"]["models"][0]["input"],
        json!(["text", "image"])
    );
    assert_eq!(roundtrip["settingsConfig"]["models"][0]["maxTokens"], 8192);
    assert_eq!(
        roundtrip["settingsConfig"]["models"][0]["cost"]["cacheRead"],
        1.0
    );
    assert_eq!(
        roundtrip["settingsConfig"]["models"][0]["cost"]["cacheWrite"],
        4.0
    );
}

#[test]
fn provider_add_form_openclaw_editing_primary_model_keeps_full_models_array_and_unknown_fields() {
    let provider = Provider::with_id(
        "oclaw1".to_string(),
        "OpenClaw Provider".to_string(),
        json!({
            "api": "openai-completions",
            "models": [
                {
                    "id": "primary-model",
                    "name": "Primary Model",
                    "contextWindow": 128000,
                    "reasoning": true,
                    "cost": {
                        "cacheRead": 1.0,
                        "cacheWrite": 4.0
                    }
                },
                {
                    "id": "fallback-model",
                    "name": "Fallback Model",
                    "contextWindow": 64000,
                    "providerHint": "fallback",
                    "input": ["text"]
                }
            ]
        }),
        None,
    );

    let mut form = ProviderAddFormState::from_provider(AppType::OpenClaw, &provider);
    form.opencode_model_name.set("Primary Model Updated");
    form.opencode_model_context_limit.set("256000");

    let roundtrip = form.to_provider_json_value();
    let models = roundtrip["settingsConfig"]["models"]
        .as_array()
        .expect("OpenClaw models should remain an array");

    assert_eq!(
        models.len(),
        2,
        "editing the primary model should keep fallbacks"
    );
    assert_eq!(models[0]["id"], "primary-model");
    assert_eq!(models[0]["name"], "Primary Model Updated");
    assert_eq!(models[0]["contextWindow"], 256000);
    assert_eq!(models[0]["reasoning"], true);
    assert_eq!(models[0]["cost"]["cacheRead"], 1.0);
    assert_eq!(models[0]["cost"]["cacheWrite"], 4.0);
    assert_eq!(models[1]["id"], "fallback-model");
    assert_eq!(models[1]["providerHint"], "fallback");
    assert_eq!(models[1]["input"], json!(["text"]));
}

#[test]
fn provider_add_form_openclaw_clearing_model_id_removes_model_instead_of_using_name() {
    let provider = Provider::with_id(
        "oclaw1".to_string(),
        "OpenClaw Provider".to_string(),
        json!({
            "api": "openai-completions",
            "models": [
                {
                    "id": "primary-model",
                    "name": "Primary Model",
                    "contextWindow": 128000
                }
            ]
        }),
        None,
    );

    let mut form = ProviderAddFormState::from_provider(AppType::OpenClaw, &provider);
    form.opencode_model_id.set("");
    form.opencode_model_name.set("Display Name Only");

    let roundtrip = form.to_provider_json_value();
    assert!(
        roundtrip["settingsConfig"].get("models").is_none(),
        "OpenClaw should require an explicit model id instead of falling back to the display name"
    );
}

#[test]
fn provider_add_form_openclaw_renaming_primary_model_to_existing_fallback_deduplicates_ids() {
    let provider = Provider::with_id(
        "oclaw1".to_string(),
        "OpenClaw Provider".to_string(),
        json!({
            "api": "openai-completions",
            "models": [
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
            ]
        }),
        None,
    );

    let mut form = ProviderAddFormState::from_provider(AppType::OpenClaw, &provider);
    form.opencode_model_id.set("fallback-model");
    form.opencode_model_name.set("Merged Primary");
    form.opencode_model_context_limit.set("256000");

    let roundtrip = form.to_provider_json_value();
    let models = roundtrip["settingsConfig"]["models"]
        .as_array()
        .expect("OpenClaw models should remain an array");

    assert_eq!(
        models.len(),
        1,
        "renaming a model to an existing id should not leave duplicate OpenClaw model ids"
    );
    assert_eq!(models[0]["id"], "fallback-model");
    assert_eq!(models[0]["name"], "Merged Primary");
    assert_eq!(models[0]["contextWindow"], 256000);
    assert_eq!(models[0]["providerHint"], "reasoning");
}

#[test]
fn provider_add_form_openclaw_ignores_legacy_api_aliases_when_loading() {
    let provider = Provider::with_id(
        "oclaw1".to_string(),
        "OpenClaw Provider".to_string(),
        json!({
            "api_key": "sk-legacy-openclaw",
            "base_url": "https://legacy.openclaw.example/v1",
            "headers": {
                "X-Test": "1"
            }
        }),
        None,
    );

    let form = ProviderAddFormState::from_provider(AppType::OpenClaw, &provider);
    assert!(form.opencode_api_key.value.is_empty());
    assert!(form.opencode_base_url.value.is_empty());

    let roundtrip = form.to_provider_json_value();
    assert!(roundtrip["settingsConfig"].get("apiKey").is_none());
    assert!(roundtrip["settingsConfig"].get("baseUrl").is_none());
    assert!(
        roundtrip["settingsConfig"].get("api_key").is_none(),
        "saving OpenClaw providers should not preserve legacy api_key aliases"
    );
    assert!(
        roundtrip["settingsConfig"].get("base_url").is_none(),
        "saving OpenClaw providers should not preserve legacy base_url aliases"
    );
    assert_eq!(roundtrip["settingsConfig"]["headers"]["X-Test"], "1");
}

#[test]
fn provider_add_form_openclaw_ignores_legacy_context_window_alias_when_loading() {
    let provider = Provider::with_id(
        "oclaw1".to_string(),
        "OpenClaw Provider".to_string(),
        json!({
            "api": "openai-completions",
            "models": [
                {
                    "id": "primary-model",
                    "name": "Primary Model",
                    "context_window": 128000,
                    "providerHint": "reasoning"
                }
            ]
        }),
        None,
    );

    let form = ProviderAddFormState::from_provider(AppType::OpenClaw, &provider);
    assert_eq!(form.opencode_model_id.value, "primary-model");
    assert!(form.opencode_model_context_limit.value.is_empty());

    let roundtrip = form.to_provider_json_value();
    assert!(
        roundtrip["settingsConfig"]["models"][0]
            .get("contextWindow")
            .is_none(),
        "OpenClaw form should not promote legacy context_window to canonical contextWindow"
    );
    assert!(
        roundtrip["settingsConfig"]["models"][0]
            .get("context_window")
            .is_none(),
        "OpenClaw form should not preserve legacy context_window aliases"
    );
    assert_eq!(
        roundtrip["settingsConfig"]["models"][0]["providerHint"],
        "reasoning"
    );
}

#[test]
fn provider_add_form_openclaw_does_not_coerce_opencode_models_object_shape() {
    let provider = Provider::with_id(
        "oclaw1".to_string(),
        "OpenClaw Provider".to_string(),
        json!({
            "api": "openai-completions",
            "models": {
                "gpt-4.1-mini": {
                    "name": "GPT 4.1 Mini"
                }
            }
        }),
        None,
    );

    let form = ProviderAddFormState::from_provider(AppType::OpenClaw, &provider);
    assert!(form.openclaw_models.is_empty());
    assert!(form.opencode_model_id.value.is_empty());

    let roundtrip = form.to_provider_json_value();
    assert!(
        roundtrip["settingsConfig"].get("models").is_none(),
        "OpenClaw form JSON should drop additive models objects instead of coercing them into a one-element array"
    );
}

#[test]
fn provider_add_form_openclaw_common_config_does_not_apply_legacy_aliases() {
    let mut form = ProviderAddFormState::new(AppType::OpenClaw);
    form.id.set("oclaw1");
    form.name.set("OpenClaw Provider");
    form.opencode_api_key.set("sk-provider-openclaw");
    form.opencode_base_url
        .set("https://provider.openclaw.example/v1");

    let provider = form
        .to_provider_json_value_with_common_config(
            r#"{
                "api_key": "sk-common-openclaw",
                "base_url": "https://common.openclaw.example/v1",
                "headers": {
                    "X-Common": "1"
                }
            }"#,
        )
        .expect("OpenClaw common config should be ignored cleanly");

    assert_eq!(provider["settingsConfig"]["apiKey"], "sk-provider-openclaw");
    assert_eq!(
        provider["settingsConfig"]["baseUrl"],
        "https://provider.openclaw.example/v1"
    );
    assert!(
        provider["settingsConfig"].get("api_key").is_none(),
        "ignored OpenClaw common config should not reintroduce api_key"
    );
    assert!(
        provider["settingsConfig"].get("base_url").is_none(),
        "ignored OpenClaw common config should not reintroduce base_url"
    );
    assert!(
        provider["settingsConfig"].get("headers").is_none(),
        "ignored OpenClaw common config should not inject headers"
    );
}

#[test]
fn provider_add_form_openclaw_ignores_common_config_snippet() {
    let mut form = ProviderAddFormState::new(AppType::OpenClaw);
    form.id.set("oclaw1");
    form.name.set("OpenClaw Provider");
    form.opencode_base_url
        .set("https://provider.openclaw.example/v1");

    assert!(
        !form.include_common_config,
        "OpenClaw should default to not attaching Common Config"
    );

    let provider = form
        .to_provider_json_value_with_common_config(
            r#"{
                "baseUrl": "https://common.openclaw.example/v1",
                "headers": {
                    "X-Common": "1"
                }
            }"#,
        )
        .expect("OpenClaw common config should be ignored cleanly");

    assert_eq!(
        provider["settingsConfig"]["baseUrl"],
        "https://provider.openclaw.example/v1"
    );
    assert!(
        provider["settingsConfig"].get("headers").is_none(),
        "OpenClaw should not inherit Common Config headers"
    );
}

#[test]
fn populate_claude_form_reads_anthropic_api_key_when_auth_token_missing() {
    let provider = Provider {
        id: "mimo".to_string(),
        name: "Xiaomi Mimo".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_API_KEY": "sk-from-import",
                "ANTHROPIC_BASE_URL": "https://api.example.com/anthropic"
            }
        }),
        website_url: None,
        category: None,
        created_at: None,
        sort_index: None,
        notes: None,
        meta: None,
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };

    let form = ProviderAddFormState::from_provider(AppType::Claude, &provider);
    assert_eq!(form.claude_api_key.value, "sk-from-import");
    assert_eq!(
        form.claude_api_key_field,
        crate::provider::ClaudeApiKeyField::ApiKey
    );
}

#[test]
fn populate_claude_form_honors_auth_token_field_meta_over_api_key() {
    use crate::provider::ProviderMeta;

    let provider = Provider {
        id: "claude".to_string(),
        name: "Claude".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "sk-token",
                "ANTHROPIC_API_KEY": "sk-key"
            }
        }),
        website_url: None,
        category: None,
        created_at: None,
        sort_index: None,
        notes: None,
        meta: Some(ProviderMeta {
            api_key_field: Some("ANTHROPIC_AUTH_TOKEN".to_string()),
            ..Default::default()
        }),
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };

    let form = ProviderAddFormState::from_provider(AppType::Claude, &provider);
    assert_eq!(form.claude_api_key.value, "sk-token");
    assert_eq!(
        form.claude_api_key_field,
        crate::provider::ClaudeApiKeyField::AuthToken
    );
}

#[test]
fn provider_edit_form_roundtrip_no_duplicate_common_config_key() {
    // Issue #71: editing a Claude provider and saving fails with
    // "duplicate field `commonConfigEnabled`" because extra (from
    // serde_json::to_value) has commonConfigEnabled while
    // to_provider_json_value inserts applyCommonConfig.
    use crate::provider::ProviderMeta;

    let provider = Provider {
        id: "test-provider".to_string(),
        name: "Test Provider".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "sk-test"
            }
        }),
        website_url: None,
        category: None,
        created_at: None,
        sort_index: None,
        notes: None,
        meta: Some(ProviderMeta {
            apply_common_config: Some(true),
            ..Default::default()
        }),
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };

    let form = ProviderAddFormState::from_provider(AppType::Claude, &provider);
    let json_value = form.to_provider_json_value();
    let json_str = serde_json::to_string_pretty(&json_value).unwrap();

    // The roundtrip: deserialize back to Provider (this is what submit_provider_edit does)
    let roundtrip: Provider = serde_json::from_str(&json_str)
        .expect("roundtrip deserialization should succeed without duplicate field error");
    assert_eq!(roundtrip.id, "test-provider");
    assert_eq!(roundtrip.name, "Test Provider");
}

#[test]
fn provider_copy_form_uses_new_record_identity_without_queue_state() {
    use crate::provider::ProviderMeta;

    let mut provider = Provider {
        id: "test-provider".to_string(),
        name: "Test Provider".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "sk-test"
            }
        }),
        website_url: Some("https://example.com".to_string()),
        category: Some("third_party".to_string()),
        created_at: Some(123),
        sort_index: Some(7),
        notes: Some("Keep visible notes".to_string()),
        meta: Some(ProviderMeta {
            endpoint_auto_select: Some(true),
            ..Default::default()
        }),
        icon: Some("anthropic".to_string()),
        icon_color: Some("#111111".to_string()),
        in_failover_queue: true,
    };
    provider.meta.as_mut().unwrap().apply_common_config = Some(true);

    let form = ProviderAddFormState::copy_from_provider_with_common_snippet(
        AppType::Claude,
        &provider,
        "",
        &[
            "test-provider".to_string(),
            "test-provider-copy".to_string(),
        ],
    );
    let copied = form.to_provider_json_value();

    assert!(matches!(form.mode, FormMode::Add));
    assert_eq!(form.copy_source_id.as_deref(), Some("test-provider"));
    assert_eq!(copied["id"], "test-provider-copy-2");
    assert_eq!(copied["name"], "Test Provider copy");
    assert!(copied.get("createdAt").is_none());
    assert_eq!(copied["sortIndex"], 7);
    assert!(copied.get("inFailoverQueue").is_none());
    assert_eq!(
        copied["settingsConfig"]["env"]["ANTHROPIC_AUTH_TOKEN"],
        "sk-test"
    );
    assert_eq!(copied["notes"], "Keep visible notes");
    assert_eq!(copied["category"], "third_party");
    assert_eq!(copied["meta"]["endpointAutoSelect"], true);
    assert_eq!(copied["meta"]["commonConfigEnabled"], true);
}

#[test]
fn provider_edit_form_roundtrip_preserves_upstream_meta_auth_and_type_fields() {
    let provider_value = json!({
        "id": "provider-1",
        "name": "Provider One",
        "settingsConfig": {
            "env": {
                "ANTHROPIC_BASE_URL": "https://example.com"
            }
        },
        "meta": {
            "authBinding": {
                "source": "managed_account",
                "authProvider": "github_copilot",
                "accountId": "acc-1"
            },
            "apiKeyField": "ANTHROPIC_AUTH_TOKEN",
            "providerType": "github_copilot",
            "githubAccountId": "gh-123"
        }
    });
    let provider: Provider = serde_json::from_value(provider_value).expect("provider json valid");

    let form = ProviderAddFormState::from_provider(AppType::Claude, &provider);
    let roundtrip = form.to_provider_json_value();
    let meta = roundtrip["meta"]
        .as_object()
        .expect("meta should be object");

    assert_eq!(
        meta.get("authBinding")
            .and_then(|value| value.get("source"))
            .and_then(|value| value.as_str()),
        Some("managed_account")
    );
    assert_eq!(
        meta.get("authBinding")
            .and_then(|value| value.get("authProvider"))
            .and_then(|value| value.as_str()),
        Some("github_copilot")
    );
    assert_eq!(
        meta.get("authBinding")
            .and_then(|value| value.get("accountId"))
            .and_then(|value| value.as_str()),
        Some("acc-1")
    );
    assert!(
        meta.get("apiKeyField").is_none(),
        "upstream omits apiKeyField when the default ANTHROPIC_AUTH_TOKEN field is selected"
    );
    assert_eq!(
        meta.get("providerType").and_then(|value| value.as_str()),
        Some("github_copilot")
    );
    assert_eq!(
        meta.get("githubAccountId").and_then(|value| value.as_str()),
        Some("gh-123")
    );
}

#[test]
fn provider_edit_form_roundtrip_preserves_claude_api_key_field_shape() {
    let provider_value = json!({
        "id": "provider-1",
        "name": "Provider One",
        "settingsConfig": {
            "env": {
                "ANTHROPIC_BASE_URL": "https://api.example.com",
                "ANTHROPIC_API_KEY": "sk-api-key",
                "ANTHROPIC_AUTH_TOKEN": "stale-token",
                "EXTRA_ENV": "keep"
            }
        },
        "meta": {
            "apiKeyField": "ANTHROPIC_API_KEY",
            "endpointAutoSelect": true
        }
    });
    let provider: Provider = serde_json::from_value(provider_value).expect("provider json valid");

    let mut form = ProviderAddFormState::from_provider(AppType::Claude, &provider);
    assert_eq!(form.claude_api_key.value, "sk-api-key");
    form.claude_api_key.set("sk-updated");
    let roundtrip = form.to_provider_json_value();

    assert_eq!(
        roundtrip["settingsConfig"]["env"]["ANTHROPIC_API_KEY"],
        "sk-updated"
    );
    assert!(
        roundtrip["settingsConfig"]["env"]
            .get("ANTHROPIC_AUTH_TOKEN")
            .is_none(),
        "saving an API_KEY provider should not recreate the default auth-token field"
    );
    assert_eq!(roundtrip["settingsConfig"]["env"]["EXTRA_ENV"], "keep");
    assert_eq!(roundtrip["meta"]["apiKeyField"], "ANTHROPIC_API_KEY");
    assert_eq!(roundtrip["meta"]["endpointAutoSelect"], true);
}

#[test]
fn provider_edit_form_infers_claude_api_key_field_from_env_when_meta_missing() {
    let provider_value = json!({
        "id": "provider-1",
        "name": "Provider One",
        "settingsConfig": {
            "env": {
                "ANTHROPIC_BASE_URL": "https://api.example.com",
                "ANTHROPIC_API_KEY": "sk-api-key"
            }
        }
    });
    let provider: Provider = serde_json::from_value(provider_value).expect("provider json valid");

    let form = ProviderAddFormState::from_provider(AppType::Claude, &provider);
    let roundtrip = form.to_provider_json_value();

    assert_eq!(form.claude_api_key.value, "sk-api-key");
    assert_eq!(
        roundtrip["settingsConfig"]["env"]["ANTHROPIC_API_KEY"],
        "sk-api-key"
    );
    assert_eq!(
        roundtrip["meta"]["apiKeyField"], "ANTHROPIC_API_KEY",
        "upstream saves non-default Claude auth field in provider meta"
    );
}

#[test]
fn provider_add_form_does_not_write_usage_script_until_touched() {
    let mut form = ProviderAddFormState::new(AppType::Claude);
    form.name.set("Provider One");

    form.open_usage_query_page();
    let provider = form.to_provider_json_value();

    assert!(
        provider
            .get("meta")
            .and_then(|meta| meta.get("usage_script"))
            .is_none(),
        "default Usage Query state should not create provider meta"
    );
}

#[test]
fn provider_add_form_usage_query_defaults_match_upstream() {
    let mut form = ProviderAddFormState::new(AppType::Claude);

    assert!(!form.usage_query_enabled);
    assert_eq!(form.usage_query_template, UsageQueryTemplate::General);
    assert_eq!(form.usage_query_timeout.value, "10");
    assert_eq!(form.usage_query_auto_interval.value, "5");
    assert_eq!(
        form.usage_query_code,
        ProviderAddFormState::USAGE_QUERY_GENERAL_PRESET
    );

    form.claude_base_url.set("https://api.deepseek.com");
    form.open_usage_query_page();
    assert!(!form.usage_query_enabled);
    assert_eq!(form.usage_query_template, UsageQueryTemplate::Balance);
    assert!(form.usage_query_code.is_empty());
}

#[test]
fn provider_add_form_usage_query_balance_default_uses_app_specific_base_url() {
    let mut codex = ProviderAddFormState::new(AppType::Codex);
    codex.codex_base_url.set("https://openrouter.ai/api/v1");
    codex.open_usage_query_page();
    assert_eq!(codex.usage_query_template, UsageQueryTemplate::Balance);

    let mut openclaw = ProviderAddFormState::new(AppType::OpenClaw);
    openclaw
        .opencode_base_url
        .set("https://api.novita.ai/v3/openai");
    openclaw.open_usage_query_page();
    assert_eq!(openclaw.usage_query_template, UsageQueryTemplate::Balance);
}

#[test]
fn provider_add_form_writes_usage_script_after_enable_toggle() {
    let mut form = ProviderAddFormState::new(AppType::Claude);
    form.name.set("Provider One");
    form.toggle_usage_query_enabled();

    let provider = form.to_provider_json_value();
    let script = &provider["meta"]["usage_script"];

    assert_eq!(script["enabled"], true);
    assert_eq!(script["language"], "javascript");
    assert_eq!(script["templateType"], "general");
    assert_eq!(script["timeout"], 10);
    assert_eq!(script["autoQueryInterval"], 5);
}

#[test]
fn provider_add_form_usage_query_template_fields_match_upstream_visibility() {
    let mut form = ProviderAddFormState::new(AppType::Claude);

    assert_eq!(form.usage_query_fields(), vec![UsageQueryField::Enabled]);

    form.toggle_usage_query_enabled();
    form.set_usage_query_template(UsageQueryTemplate::Custom);
    assert_eq!(
        form.usage_query_fields(),
        vec![
            UsageQueryField::Enabled,
            UsageQueryField::Template,
            UsageQueryField::Timeout,
            UsageQueryField::AutoInterval,
            UsageQueryField::Script,
        ]
    );

    form.set_usage_query_template(UsageQueryTemplate::Balance);
    assert_eq!(
        form.usage_query_fields(),
        vec![
            UsageQueryField::Enabled,
            UsageQueryField::Template,
            UsageQueryField::Timeout,
            UsageQueryField::AutoInterval,
            UsageQueryField::Script,
        ]
    );

    assert_eq!(
        form.available_usage_query_templates(),
        vec![
            UsageQueryTemplate::Custom,
            UsageQueryTemplate::General,
            UsageQueryTemplate::NewApi,
            UsageQueryTemplate::Balance,
        ]
    );
    assert!(!form
        .available_usage_query_templates()
        .contains(&UsageQueryTemplate::TokenPlan));
}

#[test]
fn provider_add_form_usage_query_table_fields_hide_script_row() {
    let mut form = ProviderAddFormState::new(AppType::Claude);
    form.toggle_usage_query_enabled();
    form.set_usage_query_template(UsageQueryTemplate::General);

    assert!(form.usage_query_fields().contains(&UsageQueryField::Script));
    assert!(!form
        .usage_query_table_fields()
        .contains(&UsageQueryField::Script));
}

#[test]
fn provider_add_form_usage_query_custom_template_includes_dynamic_variable_comments() {
    let mut form = ProviderAddFormState::new(AppType::Claude);
    form.claude_base_url.set("https://nowcoding.ai/v1");
    form.claude_api_key
        .set("sk-S56IHzWqgbyW6Yfp9F5qc2A0RpdYFmqCfop3FkPFFKCrzmJq");

    form.set_usage_query_template(UsageQueryTemplate::Custom);

    assert!(form.usage_query_code.starts_with(
        "// 支持的变量\n\
// {{baseUrl}}\n\
// =\n\
// https://nowcoding.ai/v1\n\
// {{apiKey}}\n\
// =\n\
// sk-S56IHzWqgbyW6Yfp9F5qc2A0RpdYFmqCfop3FkPFFKCrzmJq\n\n"
    ));
    assert!(form
        .usage_query_code
        .contains(ProviderAddFormState::USAGE_QUERY_CUSTOM_PRESET));
}

#[test]
fn provider_add_form_usage_query_custom_variable_comments_refresh_without_replacing_body() {
    let mut form = ProviderAddFormState::new(AppType::Codex);
    form.codex_base_url.set("https://old.example/v1");
    form.codex_api_key.set("sk-old");
    form.set_usage_query_template(UsageQueryTemplate::Custom);
    form.usage_query_code.push_str("\n// custom user edit");

    form.codex_base_url.set("https://new.example/v1");
    form.codex_api_key.set("sk-new");
    form.refresh_usage_query_custom_variable_comment();

    assert!(form.usage_query_code.starts_with(
        "// 支持的变量\n\
// {{baseUrl}}\n\
// =\n\
// https://new.example/v1\n\
// {{apiKey}}\n\
// =\n\
// sk-new\n\n"
    ));
    assert!(form.usage_query_code.ends_with("// custom user edit"));
}

#[test]
fn provider_add_form_usage_query_numeric_fields_match_upstream_normalization() {
    let mut form = ProviderAddFormState::new(AppType::Claude);
    form.name.set("Provider One");
    form.toggle_usage_query_enabled();
    form.usage_query_timeout.set("12.9");
    form.usage_query_auto_interval.set("1440.8");

    let provider = form.to_provider_json_value();
    let script = &provider["meta"]["usage_script"];

    assert_eq!(script["timeout"], 12);
    assert_eq!(script["autoQueryInterval"], 1440);

    form.usage_query_timeout.set("-1");
    form.usage_query_auto_interval.set("");
    let provider = form.to_provider_json_value();
    let script = &provider["meta"]["usage_script"];

    assert_eq!(script["timeout"], 10);
    assert_eq!(script["autoQueryInterval"], 0);
}
