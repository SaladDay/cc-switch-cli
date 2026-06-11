use super::codex_config::parse_codex_config_snippet;
use super::provider_state::codex_model_catalog_row_from_value;
use super::{
    claude_hide_attribution_enabled, detect_balance_provider_for_usage_query,
    detect_coding_plan_provider_for_usage_query, ClaudeApiFormat, CodexWireApi,
    ProviderAddFormState, UsageQueryTemplate, OPENCLAW_DEFAULT_API_PROTOCOL,
};
use crate::app_config::AppType;
use crate::provider::Provider;
use serde_json::Value;

pub(super) fn populate_form_from_provider(
    form: &mut ProviderAddFormState,
    app_type: &AppType,
    provider: &Provider,
) {
    match app_type {
        AppType::Claude => populate_claude_form(form, provider),
        AppType::Codex => populate_codex_form(form, provider),
        AppType::Gemini => populate_gemini_form(form, provider),
        AppType::OpenCode => populate_opencode_form(form, provider),
        AppType::Hermes => populate_hermes_form(form, provider),
        AppType::OpenClaw => populate_openclaw_form(form, provider),
    }
    populate_usage_query_form(form, provider);
}

fn populate_usage_query_form(form: &mut ProviderAddFormState, provider: &Provider) {
    let Some(script) = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.usage_script.as_ref())
    else {
        form.refresh_default_usage_query_template();
        return;
    };

    form.usage_query_enabled = script.enabled;
    form.usage_query_timeout
        .set(script.timeout.unwrap_or(10).to_string());
    form.usage_query_auto_interval
        .set(script.auto_query_interval.unwrap_or(5).to_string());
    if let Some(value) = script.api_key.as_deref() {
        form.usage_query_api_key.set(value);
    }
    if let Some(value) = script.base_url.as_deref() {
        form.usage_query_base_url.set(value);
    }
    if let Some(value) = script.access_token.as_deref() {
        form.usage_query_access_token.set(value);
    }
    if let Some(value) = script.user_id.as_deref() {
        form.usage_query_user_id.set(value);
    }
    if let Some(value) = script.coding_plan_provider.as_deref() {
        form.usage_query_coding_plan_provider.set(value);
    } else if let Some(provider) =
        detect_coding_plan_provider_for_usage_query(&form.current_provider_base_url())
    {
        form.usage_query_coding_plan_provider.set(provider);
    }

    let template = script
        .template_type
        .as_deref()
        .and_then(UsageQueryTemplate::from_str)
        .or_else(|| {
            if script.access_token.is_some() || script.user_id.is_some() {
                Some(UsageQueryTemplate::NewApi)
            } else if script.api_key.is_some() || script.base_url.is_some() {
                Some(UsageQueryTemplate::General)
            } else if detect_balance_provider_for_usage_query(&form.current_provider_base_url()) {
                Some(UsageQueryTemplate::Balance)
            } else {
                Some(UsageQueryTemplate::General)
            }
        })
        .unwrap_or(UsageQueryTemplate::General);

    form.usage_query_template = template;
    form.usage_query_code = script.code.clone();
}

fn populate_claude_form(form: &mut ProviderAddFormState, provider: &Provider) {
    form.claude_api_format = parse_claude_api_format(provider);
    form.claude_api_key_field = crate::provider::ClaudeApiKeyField::from_meta_and_settings(
        provider.meta.as_ref(),
        &provider.settings_config,
    );
    form.claude_hide_attribution = claude_hide_attribution_enabled(&provider.settings_config);
    if provider
        .meta
        .as_ref()
        .and_then(|meta| meta.provider_type.as_deref())
        == Some("codex_oauth")
    {
        form.claude_api_format = ClaudeApiFormat::OpenAiResponses;
        form.codex_oauth_account_id = provider
            .meta
            .as_ref()
            .and_then(|meta| meta.managed_account_id_for("codex_oauth"));
        form.codex_fast_mode = provider
            .meta
            .as_ref()
            .map(|meta| meta.codex_fast_mode_enabled())
            .unwrap_or(false);
    }
    if let Some(env) = provider
        .settings_config
        .get("env")
        .and_then(|value| value.as_object())
    {
        if let Some(token) = env
            .get(form.claude_api_key_field.as_env_key())
            .and_then(|value| value.as_str())
            .or_else(|| {
                env.get(form.claude_api_key_field.alternate_env_key())
                    .and_then(|value| value.as_str())
            })
        {
            form.claude_api_key.set(token);
        }
        if let Some(url) = env
            .get("ANTHROPIC_BASE_URL")
            .and_then(|value| value.as_str())
        {
            form.claude_base_url.set(url);
        }
        if let Some(model) = env.get("ANTHROPIC_MODEL").and_then(|value| value.as_str()) {
            form.claude_model.set(model);
        }
        if let Some(reasoning) = env
            .get("ANTHROPIC_REASONING_MODEL")
            .and_then(|value| value.as_str())
        {
            form.claude_reasoning_model.set(reasoning);
        }

        let model = env.get("ANTHROPIC_MODEL").and_then(|value| value.as_str());
        let small_fast = env
            .get("ANTHROPIC_SMALL_FAST_MODEL")
            .and_then(|value| value.as_str());

        if let Some(haiku) = env
            .get("ANTHROPIC_DEFAULT_HAIKU_MODEL")
            .and_then(|value| value.as_str())
            .or(small_fast)
            .or(model)
        {
            form.claude_haiku_model.set(haiku);
        }
        if let Some(sonnet) = env
            .get("ANTHROPIC_DEFAULT_SONNET_MODEL")
            .and_then(|value| value.as_str())
            .or(model)
            .or(small_fast)
        {
            form.claude_sonnet_model.set(sonnet);
        }
        if let Some(opus) = env
            .get("ANTHROPIC_DEFAULT_OPUS_MODEL")
            .and_then(|value| value.as_str())
            .or(model)
            .or(small_fast)
        {
            form.claude_opus_model.set(opus);
        }
    }
}

fn populate_codex_form(form: &mut ProviderAddFormState, provider: &Provider) {
    let mut parsed_wire_api = None;
    if let Some(config) = provider
        .settings_config
        .get("config")
        .and_then(|value| value.as_str())
    {
        let parsed = parse_codex_config_snippet(config);
        if let Some(base_url) = parsed.base_url {
            form.codex_base_url.set(base_url);
        }
        if let Some(model) = parsed.model {
            form.codex_model.set(model);
        }
        if let Some(wire_api) = parsed.wire_api {
            parsed_wire_api = Some(wire_api);
        }
        if let Some(requires_openai_auth) = parsed.requires_openai_auth {
            form.codex_requires_openai_auth = requires_openai_auth;
        }
        if let Some(env_key) = parsed.env_key {
            form.codex_env_key.set(env_key);
        }
    }
    if let Some(auth) = provider
        .settings_config
        .get("auth")
        .and_then(|value| value.as_object())
    {
        if let Some(key) = auth.get("OPENAI_API_KEY").and_then(|value| value.as_str()) {
            form.codex_api_key.set(key);
        }
    }
    form.claude_api_format = parse_codex_api_format(provider, parsed_wire_api);
    form.codex_wire_api = CodexWireApi::Responses;
    form.codex_chat_reasoning = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.codex_chat_reasoning.clone())
        .unwrap_or_default();
    form.codex_model_catalog = provider
        .settings_config
        .get("modelCatalog")
        .and_then(|catalog| catalog.get("models"))
        .and_then(Value::as_array)
        .map(|models| {
            models
                .iter()
                .filter_map(codex_model_catalog_row_from_value)
                .collect()
        })
        .unwrap_or_default();
}

fn populate_gemini_form(form: &mut ProviderAddFormState, provider: &Provider) {
    if let Some(env) = provider
        .settings_config
        .get("env")
        .and_then(|value| value.as_object())
    {
        if let Some(key) = env.get("GEMINI_API_KEY").and_then(|value| value.as_str()) {
            form.gemini_auth_type = super::GeminiAuthType::ApiKey;
            form.gemini_api_key.set(key);
        } else {
            form.gemini_auth_type = super::GeminiAuthType::OAuth;
        }

        if let Some(url) = env
            .get("GOOGLE_GEMINI_BASE_URL")
            .or_else(|| env.get("GEMINI_BASE_URL"))
            .and_then(|value| value.as_str())
        {
            form.gemini_base_url.set(url);
        }

        if let Some(model) = env.get("GEMINI_MODEL").and_then(|value| value.as_str()) {
            form.gemini_model.set(model);
        }
    } else {
        form.gemini_auth_type = super::GeminiAuthType::OAuth;
    }
}

fn populate_opencode_form(form: &mut ProviderAddFormState, provider: &Provider) {
    if let Some(npm) = provider
        .settings_config
        .get("npm")
        .and_then(|value| value.as_str())
    {
        form.opencode_npm_package.set(npm);
    }
    if let Some(options) = provider
        .settings_config
        .get("options")
        .and_then(|value| value.as_object())
    {
        if let Some(api_key) = options.get("apiKey").and_then(|value| value.as_str()) {
            form.opencode_api_key.set(api_key);
        }
        if let Some(base_url) = options.get("baseURL").and_then(|value| value.as_str()) {
            form.opencode_base_url.set(base_url);
        }
    }
    if let Some(models) = provider
        .settings_config
        .get("models")
        .and_then(|value| value.as_object())
    {
        if let Some((model_id, model_value)) =
            models.iter().max_by(|(id_a, model_a), (id_b, model_b)| {
                opencode_model_rank(model_a)
                    .cmp(&opencode_model_rank(model_b))
                    .then_with(|| id_b.cmp(id_a))
            })
        {
            form.opencode_model_original_id = Some(model_id.clone());
            form.opencode_model_id.set(model_id);
            if let Some(name) = model_value.get("name").and_then(|value| value.as_str()) {
                form.opencode_model_name.set(name);
            } else {
                form.opencode_model_name.set(model_id);
            }
            if let Some(limit) = model_value.get("limit").and_then(|value| value.as_object()) {
                if let Some(context) = limit.get("context").and_then(|value| value.as_u64()) {
                    form.opencode_model_context_limit.set(context.to_string());
                }
                if let Some(output) = limit.get("output").and_then(|value| value.as_u64()) {
                    form.opencode_model_output_limit.set(output.to_string());
                }
            }
        }
    }
}

fn populate_hermes_form(form: &mut ProviderAddFormState, provider: &Provider) {
    let settings = &provider.settings_config;

    if let Some(api_mode) = settings
        .get("api_mode")
        .or_else(|| settings.get("apiMode"))
        .and_then(|value| value.as_str())
    {
        if super::HERMES_API_MODES.iter().any(|mode| *mode == api_mode) {
            form.hermes_api_mode = api_mode.to_string();
        }
    }

    if let Some(base_url) = settings
        .get("base_url")
        .or_else(|| settings.get("baseUrl"))
        .or_else(|| settings.get("baseURL"))
        .or_else(|| settings.get("endpoint"))
        .and_then(|value| value.as_str())
    {
        form.hermes_base_url.set(base_url);
    }
    if let Some(api_key) = settings
        .get("api_key")
        .or_else(|| settings.get("apiKey"))
        .or_else(|| settings.get("auth_token"))
        .and_then(|value| value.as_str())
    {
        form.hermes_api_key.set(api_key);
    }
    if let Some(models) = settings.get("models").and_then(|value| value.as_array()) {
        form.hermes_models = models.clone();
    }
    if let Some(delay) = settings
        .get("rate_limit_delay")
        .and_then(|value| value.as_f64())
    {
        if delay.is_finite() && delay >= 0.0 {
            form.hermes_rate_limit_delay.set(delay.to_string());
        }
    }
}

fn populate_openclaw_form(form: &mut ProviderAddFormState, provider: &Provider) {
    if let Some(api_key) = provider
        .settings_config
        .get("apiKey")
        .and_then(|value| value.as_str())
    {
        form.opencode_api_key.set(api_key);
    }
    if let Some(base_url) = provider
        .settings_config
        .get("baseUrl")
        .and_then(|value| value.as_str())
    {
        form.opencode_base_url.set(base_url);
    }
    if let Some(api) = provider
        .settings_config
        .get("api")
        .and_then(|value| value.as_str())
    {
        form.opencode_npm_package.set(api);
    } else {
        form.opencode_npm_package.set(OPENCLAW_DEFAULT_API_PROTOCOL);
    }
    if provider
        .settings_config
        .get("headers")
        .and_then(|value| value.as_object())
        .is_some_and(|headers| headers.contains_key("User-Agent"))
    {
        form.openclaw_user_agent = true;
    }
    if let Some(models) = provider
        .settings_config
        .get("models")
        .and_then(|value| value.as_array())
    {
        form.openclaw_models = models.clone();
    }
    if let Some(model) = form.openclaw_models.first() {
        if let Some(id) = model.get("id").and_then(|value| value.as_str()) {
            form.opencode_model_original_id = Some(id.to_string());
            form.opencode_model_id.set(id);
        }
        if let Some(name) = model.get("name").and_then(|value| value.as_str()) {
            form.opencode_model_name.set(name);
        }
        if let Some(context_window) = model.get("contextWindow").and_then(|value| value.as_u64()) {
            form.opencode_model_context_limit
                .set(context_window.to_string());
        }
    }
}

fn parse_claude_api_format(provider: &Provider) -> ClaudeApiFormat {
    if let Some(api_format) = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.api_format.as_deref())
    {
        return ClaudeApiFormat::from_raw(api_format);
    }

    if let Some(api_format) = provider
        .settings_config
        .get("api_format")
        .and_then(|value| value.as_str())
    {
        return ClaudeApiFormat::from_raw(api_format);
    }

    let compat_enabled = match provider.settings_config.get("openrouter_compat_mode") {
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_i64().unwrap_or(0) != 0,
        Some(Value::String(value)) => {
            let normalized = value.trim().to_ascii_lowercase();
            normalized == "true" || normalized == "1"
        }
        _ => false,
    };

    if compat_enabled {
        ClaudeApiFormat::OpenAiChat
    } else {
        ClaudeApiFormat::Anthropic
    }
}

fn parse_codex_api_format(provider: &Provider, wire_api: Option<CodexWireApi>) -> ClaudeApiFormat {
    if let Some(api_format) = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.api_format.as_deref())
        .or_else(|| {
            provider
                .settings_config
                .get("api_format")
                .and_then(|value| value.as_str())
        })
        .or_else(|| {
            provider
                .settings_config
                .get("apiFormat")
                .and_then(|value| value.as_str())
        })
    {
        return match api_format {
            "openai_chat" => ClaudeApiFormat::OpenAiChat,
            _ => ClaudeApiFormat::OpenAiResponses,
        };
    }

    match wire_api {
        Some(CodexWireApi::Chat) => ClaudeApiFormat::OpenAiChat,
        _ => ClaudeApiFormat::OpenAiResponses,
    }
}

fn opencode_model_rank(model: &Value) -> usize {
    let mut score = 0;
    if model
        .get("limit")
        .and_then(|value| value.as_object())
        .map(|limit| !limit.is_empty())
        .unwrap_or(false)
    {
        score += 1;
    }
    if model
        .get("options")
        .and_then(|value| value.as_object())
        .map(|options| !options.is_empty())
        .unwrap_or(false)
    {
        score += 1;
    }
    score
}
