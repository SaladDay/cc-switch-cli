//! DSH provider profiles are Cordis patches; credentials remain in the native store.
use crate::{dsh_config, error::AppError};
use serde_json::{json, Value};
use std::{fs, io::Write, path::PathBuf, str::FromStr, time::Duration};

pub const DSH_API_PROTOCOLS: &[&str] = &[
    "deepseek",
    "openai-completions",
    "openai-responses",
    "anthropic-messages",
];
pub const DEFAULT_BASE_URL: &str = "https://api.deepseek.com/anthropic";
const PI_ROUTE: &str = "cc-switch";

fn invalid(message: &str) -> AppError {
    AppError::InvalidInput(message.into())
}

pub fn default_settings(api_key: &str) -> Value {
    json!({"apiKey": api_key, "profile": "web", "api": "deepseek",
        "baseUrl": DEFAULT_BASE_URL,
        "models": [{"id": "deepseek-flash"}, {"id": "deepseek-v4-pro"}],
        "defaultModel": "deepseek-flash"})
}

pub fn is_legacy(settings: &Value) -> bool {
    settings
        .as_object()
        .is_some_and(|map| map.len() == 1 && map.contains_key("apiKey"))
}

pub fn profile_name(settings: &Value) -> &str {
    settings
        .get("profile")
        .and_then(Value::as_str)
        .unwrap_or("web")
}

fn valid_profile_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

fn profile_dir(name: &str) -> Result<PathBuf, AppError> {
    if !valid_profile_name(name) {
        return Err(invalid("DSH profile must be a single directory name"));
    }
    let root = dsh_config::get_dsh_dir().join("profiles");
    let directory = root.join(name);
    let canonical_root = fs::canonicalize(&root).map_err(|e| AppError::io(&root, e))?;
    let canonical = fs::canonicalize(&directory).map_err(|e| AppError::io(&directory, e))?;
    if canonical.parent() != Some(canonical_root.as_path()) {
        return Err(invalid("DSH profile may not escape the profiles directory"));
    }
    let manifest_path = directory.join("package.json");
    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(&manifest_path).map_err(|e| AppError::io(&manifest_path, e))?,
    )
    .map_err(|_| invalid("Invalid DSH profile package.json"))?;
    if !manifest
        .pointer("/dsh/profile/bundles")
        .and_then(Value::as_array)
        .is_some_and(|bundles| {
            bundles
                .iter()
                .any(|b| b.as_str() == Some("@deepseek-ai/dsh-base"))
        })
    {
        return Err(invalid(
            "DSH provider management requires an initialized dsh-base profile",
        ));
    }
    Ok(directory)
}

pub fn validate_settings(settings: &Value) -> Result<(), AppError> {
    let object = settings
        .as_object()
        .ok_or_else(|| invalid("DSH settings must be an object"))?;
    if is_legacy(settings) {
        return Ok(());
    }
    let allowed = [
        "apiKey",
        "profile",
        "api",
        "baseUrl",
        "models",
        "defaultModel",
        "reasoningEffort",
        "providerConfig",
    ];
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(invalid(
            "Unknown DSH provider field; put native advanced options in providerConfig",
        ));
    }
    if object
        .get("profile")
        .is_some_and(|v| !v.as_str().is_some_and(valid_profile_name))
    {
        return Err(invalid("Invalid DSH profile name"));
    }
    let api = settings
        .get("api")
        .and_then(Value::as_str)
        .unwrap_or("deepseek");
    if !DSH_API_PROTOCOLS.contains(&api) {
        return Err(invalid("Unsupported DSH API protocol"));
    }
    if object.get("api").is_some_and(|v| !v.is_string())
        || object.get("baseUrl").is_some_and(|v| !v.is_string())
    {
        return Err(invalid("DSH protocol and Base URL must be strings"));
    }
    let url = settings
        .get("baseUrl")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_BASE_URL);
    let url = url::Url::parse(url)
        .map_err(|_| invalid("DSH Base URL must be an absolute HTTP(S) URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid(
            "DSH Base URL must use HTTP(S) without credentials, query or fragment",
        ));
    }
    let models = settings
        .get("models")
        .and_then(Value::as_array)
        .filter(|models| !models.is_empty())
        .ok_or_else(|| invalid("DSH requires a non-empty model catalog"))?;
    let mut ids = std::collections::HashSet::new();
    for model in models {
        let id = model
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| invalid("DSH models require a non-empty id"))?;
        if !ids.insert(id) {
            return Err(invalid("DSH model ids must be unique"));
        }
        for field in ["contextWindow", "maxTokens"] {
            if model
                .get(field)
                .is_some_and(|v| v.as_u64().is_none_or(|v| v == 0))
            {
                return Err(invalid("DSH model capacities must be positive integers"));
            }
        }
        let allowed = if api == "deepseek" {
            &[
                "id",
                "name",
                "description",
                "contextWindow",
                "maxTokens",
                "inputModalities",
                "imagePixelBudget",
                "imageMaxBytes",
                "systemPromptUpdate",
                "toolUpdate",
            ][..]
        } else {
            &[
                "id",
                "name",
                "contextWindow",
                "maxTokens",
                "input",
                "reasoningEfforts",
                "compat",
            ][..]
        };
        if model
            .as_object()
            .unwrap()
            .keys()
            .any(|key| !allowed.contains(&key.as_str()))
        {
            return Err(invalid(
                "Unsupported field in DSH model catalog for this protocol",
            ));
        }
        for field in ["name", "description"] {
            if model.get(field).is_some_and(|v| !v.is_string()) {
                return Err(invalid("DSH model names and descriptions must be strings"));
            }
        }
        if model.get("name").is_some_and(|v| v.as_str() == Some("")) {
            return Err(invalid("DSH model names may not be empty"));
        }
        validate_modalities(model.get(if api == "deepseek" {
            "inputModalities"
        } else {
            "input"
        }))?;
        if api == "deepseek" {
            if (model.get("imagePixelBudget").is_some() || model.get("imageMaxBytes").is_some())
                && !model
                    .get("inputModalities")
                    .and_then(Value::as_array)
                    .is_some_and(|items| items.iter().any(|item| item == "image"))
            {
                return Err(invalid("DSH text-only models cannot declare image limits"));
            }
            if model.get("imagePixelBudget").is_some_and(|v| {
                v.as_str() != Some("low")
                    && v.as_u64()
                        .is_none_or(|n| n == 0 || n > 9_007_199_254_740_991)
            }) || model.get("imageMaxBytes").is_some_and(|v| {
                v.as_u64()
                    .is_none_or(|n| n == 0 || n > 9_007_199_254_740_991)
            }) || model
                .get("systemPromptUpdate")
                .is_some_and(|v| v.as_str() != Some("in-history"))
                || model
                    .get("toolUpdate")
                    .is_some_and(|v| !matches!(v.as_str(), Some("in-history" | "addition-only")))
            {
                return Err(invalid("Invalid DSH native model options"));
            }
        } else {
            validate_compat(model.get("compat"), api)?;
            if let Some(efforts) = model.get("reasoningEfforts") {
                if efforts != &json!(false)
                    && !efforts.as_object().is_some_and(|map| {
                        map.keys().any(|key| key != "off")
                            && map.iter().all(|(key, value)| {
                                ["off", "minimal", "low", "medium", "high", "xhigh", "max"]
                                    .contains(&key.as_str())
                                    && (value.as_str().is_some_and(|s| !s.is_empty())
                                        || (key == "off" && value.is_null()))
                            })
                    })
                {
                    return Err(invalid("Invalid DSH model reasoning efforts"));
                }
            }
        }
    }
    if let Some(model) = settings.get("defaultModel") {
        if !model.as_str().is_some_and(|id| ids.contains(id)) {
            return Err(invalid("DSH defaultModel must be in the model catalog"));
        }
    }
    if settings.get("reasoningEffort").is_some_and(|v| {
        !v.as_str().is_some_and(|s| {
            ["off", "minimal", "low", "medium", "high", "xhigh", "max"].contains(&s)
        })
    }) {
        return Err(invalid("Invalid DSH reasoning effort"));
    }
    if api == "deepseek"
        && settings
            .get("reasoningEffort")
            .and_then(Value::as_str)
            .is_some_and(|s| !["off", "low", "high", "max"].contains(&s))
    {
        return Err(invalid(
            "DeepSeek supports reasoning effort off, low, high or max",
        ));
    }
    if let Some(config) = settings.get("providerConfig") {
        let config = config
            .as_object()
            .ok_or_else(|| invalid("DSH providerConfig must be an object"))?;
        if [
            "apiKey",
            "apiKeyEnv",
            "api",
            "baseURL",
            "models",
            "modelOverrides",
            "provider",
            "defaultModel",
        ]
        .iter()
        .any(|key| config.contains_key(*key))
        {
            return Err(invalid(
                "DSH managed fields may not be overridden in providerConfig",
            ));
        }
        validate_native_options(config, api == "deepseek")?;
        validate_compat(config.get("compat"), api)?;
        if api == "deepseek"
            && config.get("thinking").and_then(Value::as_str) == Some("disabled")
            && settings
                .get("reasoningEffort")
                .is_some_and(|effort| effort.as_str() != Some("off"))
        {
            return Err(invalid(
                "DSH disabled thinking only permits default reasoning effort off",
            ));
        }
    }
    if api != "deepseek" {
        let default_model = settings
            .get("defaultModel")
            .and_then(Value::as_str)
            .unwrap_or_else(|| models[0]["id"].as_str().unwrap());
        let effort = settings
            .get("reasoningEffort")
            .or_else(|| settings.pointer("/providerConfig/reasoning"))
            .and_then(Value::as_str);
        let model = models
            .iter()
            .find(|model| model["id"].as_str() == Some(default_model))
            .unwrap();
        if let Some(effort) = effort {
            // The managed cc-switch route has no installed catalog to inherit.
            // Undeclared/false capabilities support only off; a declared map
            // pins every omitted level (including off) to unsupported.
            let supported = match model.get("reasoningEfforts").and_then(Value::as_object) {
                Some(efforts) => efforts.contains_key(effort),
                None => effort == "off",
            };
            if !supported {
                return Err(invalid(
                    "DSH default model does not support the selected reasoning effort; declare its native reasoningEfforts or choose a supported level",
                ));
            }
        }
    }
    Ok(())
}

fn validate_modalities(value: Option<&Value>) -> Result<(), AppError> {
    if value.is_some_and(|value| {
        !value.as_array().is_some_and(|items| {
            !items.is_empty()
                && items
                    .iter()
                    .enumerate()
                    .all(|(i, item)| !items[..i].contains(item))
                && items
                    .iter()
                    .all(|item| matches!(item.as_str(), Some("text" | "image")))
        })
    }) {
        return Err(invalid(
            "DSH model input must be a non-empty list of text/image modalities",
        ));
    }
    Ok(())
}

fn validate_native_options(
    config: &serde_json::Map<String, Value>,
    direct: bool,
) -> Result<(), AppError> {
    let direct_fields = [
        "thinking",
        "reasoningEffort",
        "maxTokens",
        "defaultContextWindow",
        "streamIdleTimeoutMs",
        "maxRequestFilesBytes",
        "maxInlineRequestImageBytes",
        "maxImagesPerRequest",
        "imageOffloadByteQuantum",
        "inlineImageOffloadByteQuantum",
        "imageOffloadCountQuantum",
        "filesApiTimeoutMs",
        "fileExpiresAfterSeconds",
        "fileRefreshMarginSeconds",
        "fileQuotaCleanupBatch",
        "retryPolicy",
    ];
    let pi_fields = [
        "displayName",
        "compat",
        "defaultContextWindow",
        "defaultMaxTokens",
        "defaultInput",
        "headers",
        "reasoning",
        "thinkingBudgets",
        "cacheRetention",
        "transport",
        "timeoutMs",
        "websocketConnectTimeoutMs",
        "streamIdleTimeoutMs",
        "maxRequestImageBytes",
        "requestImagePixelBudget",
        "requestImageMaxBytes",
        "retryPolicy",
    ];
    let allowed = if direct {
        &direct_fields[..]
    } else {
        &pi_fields[..]
    };
    if config.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(invalid("Unsupported DSH native option for this protocol"));
    }
    for (key, value) in config {
        if matches!(key.as_str(), "streamIdleTimeoutMs" | "filesApiTimeoutMs") {
            if value
                .as_f64()
                .is_none_or(|n| n <= 0.0 || n > 2_147_483_647.0)
            {
                return Err(invalid(
                    "DSH timers must be positive and at most 2147483647 ms",
                ));
            }
            continue;
        }
        if key.ends_with("Ms")
            || key.ends_with("Bytes")
            || key.ends_with("Quantum")
            || [
                "maxTokens",
                "defaultMaxTokens",
                "defaultContextWindow",
                "maxImagesPerRequest",
                "requestImagePixelBudget",
                "fileExpiresAfterSeconds",
                "fileRefreshMarginSeconds",
                "fileQuotaCleanupBatch",
            ]
            .contains(&key.as_str())
        {
            let minimum = u64::from(
                ![
                    "timeoutMs",
                    "websocketConnectTimeoutMs",
                    "fileRefreshMarginSeconds",
                ]
                .contains(&key.as_str()),
            );
            if value
                .as_u64()
                .is_none_or(|number| number < minimum || number > 9_007_199_254_740_991)
            {
                return Err(invalid(
                    "DSH native capacities and timeouts must be valid nonnegative integers",
                ));
            }
        }
    }
    for (field, minimum, maximum) in [
        ("fileExpiresAfterSeconds", 3600, 2_592_000),
        ("fileQuotaCleanupBatch", 1, 1000),
    ] {
        if config
            .get(field)
            .is_some_and(|v| v.as_u64().is_none_or(|n| n < minimum || n > maximum))
        {
            return Err(invalid(
                "DSH file expiry or cleanup batch is outside the native range",
            ));
        }
    }
    if direct {
        if config.get("thinking").and_then(Value::as_str) == Some("disabled")
            && config
                .get("reasoningEffort")
                .is_some_and(|v| v.as_str() != Some("off"))
        {
            return Err(invalid(
                "DSH disabled thinking only permits reasoning effort off",
            ));
        }
        let number =
            |key: &str, default: u64| config.get(key).and_then(Value::as_u64).unwrap_or(default);
        for (quantum, default_quantum, limit, default_limit) in [
            (
                "imageOffloadByteQuantum",
                67_108_864,
                "maxRequestFilesBytes",
                134_217_728,
            ),
            (
                "inlineImageOffloadByteQuantum",
                10_485_760,
                "maxInlineRequestImageBytes",
                20_971_520,
            ),
            ("imageOffloadCountQuantum", 20, "maxImagesPerRequest", 600),
        ] {
            if number(quantum, default_quantum) > number(limit, default_limit) {
                return Err(invalid(
                    "DSH image offload quantum may not exceed its request limit",
                ));
            }
        }
        if number("fileRefreshMarginSeconds", 3600) >= number("fileExpiresAfterSeconds", 604800) {
            return Err(invalid("DSH file refresh margin must be below its expiry"));
        }
    }
    for (field, choices) in [
        ("thinking", &["enabled", "disabled"][..]),
        ("reasoningEffort", &["off", "low", "high", "max"][..]),
        (
            "reasoning",
            &["off", "minimal", "low", "medium", "high", "xhigh", "max"][..],
        ),
        ("cacheRetention", &["none", "short", "long"][..]),
        (
            "transport",
            &["sse", "websocket", "websocket-cached", "auto"][..],
        ),
    ] {
        if config
            .get(field)
            .is_some_and(|v| !v.as_str().is_some_and(|s| choices.contains(&s)))
        {
            return Err(invalid("Invalid DSH native option value"));
        }
    }
    if config
        .get("displayName")
        .is_some_and(|v| !v.as_str().is_some_and(|name| !name.is_empty()))
    {
        return Err(invalid("DSH displayName must be a non-empty string"));
    }
    validate_modalities(config.get("defaultInput"))?;
    if let Some(headers) = config.get("headers") {
        let headers = headers
            .as_object()
            .ok_or_else(|| invalid("DSH headers must be an object of strings"))?;
        for (key, value) in headers {
            reqwest::header::HeaderName::from_bytes(key.as_bytes())
                .map_err(|_| invalid("Invalid DSH header name"))?;
            reqwest::header::HeaderValue::from_str(
                value
                    .as_str()
                    .ok_or_else(|| invalid("DSH header values must be strings"))?,
            )
            .map_err(|_| invalid("Invalid DSH header value"))?;
        }
    }
    if let Some(budgets) = config.get("thinkingBudgets") {
        if !budgets.as_object().is_some_and(|map| {
            map.iter().all(|(key, value)| {
                ["minimal", "low", "medium", "high"].contains(&key.as_str())
                    && value.as_f64().is_some_and(|n| n >= 0.0)
            })
        }) {
            return Err(invalid("Invalid DSH thinking budgets"));
        }
    }
    if let Some(retry) = config.get("retryPolicy") {
        let retry = retry
            .as_object()
            .ok_or_else(|| invalid("Invalid DSH retry policy"))?;
        let mode = retry.get("mode").and_then(Value::as_str);
        if !matches!(mode, Some("normal" | "always"))
            || retry.keys().any(|key| {
                !["mode", "maxRetries", "retryableCodes", "backoff"].contains(&key.as_str())
            })
            || (mode == Some("always")
                && (retry.contains_key("maxRetries") || retry.contains_key("retryableCodes")))
            || retry
                .get("maxRetries")
                .is_some_and(|v| v.as_u64().is_none())
            || retry.get("retryableCodes").is_some_and(|v| {
                !v.as_array()
                    .is_some_and(|items| items.iter().all(Value::is_string))
            })
        {
            return Err(invalid("Invalid DSH retry policy"));
        }
        if let Some(backoff) = retry.get("backoff") {
            let backoff = backoff
                .as_object()
                .ok_or_else(|| invalid("Invalid DSH retry backoff"))?;
            for (key, value) in backoff {
                let maximum = match key.as_str() {
                    "initialDelayMs" | "maxDelayMs" => 2_147_483_647.0,
                    "jitterRatio" => 1.0,
                    _ => return Err(invalid("Invalid DSH retry backoff field")),
                };
                if value.as_f64().is_none_or(|number| {
                    number < 0.0 || number > maximum || (key != "jitterRatio" && number == 0.0)
                }) {
                    return Err(invalid("Invalid DSH retry backoff value"));
                }
            }
            let initial = backoff
                .get("initialDelayMs")
                .and_then(Value::as_f64)
                .unwrap_or(500.0);
            let maximum = backoff
                .get("maxDelayMs")
                .and_then(Value::as_f64)
                .unwrap_or(10_000.0);
            if initial > maximum {
                return Err(invalid(
                    "DSH retry initial delay may not exceed its maximum",
                ));
            }
        }
    }
    Ok(())
}

fn validate_compat(value: Option<&Value>, api: &str) -> Result<(), AppError> {
    let Some(value) = value else {
        return Ok(());
    };
    let map = value
        .as_object()
        .ok_or_else(|| invalid("DSH compat must be an object"))?;
    let booleans = [
        "supportsStore",
        "supportsDeveloperRole",
        "supportsReasoningEffort",
        "supportsUsageInStreaming",
        "supportsFinishReason",
        "requiresToolResultName",
        "requiresAssistantAfterToolResult",
        "requiresThinkingAsText",
        "requiresReasoningContentOnAssistantMessages",
        "supportsThinkingTokenBudget",
        "supportsMaxOutputTokens",
        "supportsStrictMode",
        "supportsLongCacheRetention",
        "supportsEagerToolInputStreaming",
        "supportsCacheControlOnTools",
        "supportsTemperature",
        "forceAdaptiveThinking",
        "allowEmptySignature",
        "supportsStrictTools",
    ];
    for (key, value) in map {
        let protocol_fields: &[&str] = match api {
            "openai-responses" => &[
                "supportsDeveloperRole",
                "supportsMaxOutputTokens",
                "supportsStrictMode",
                "supportsLongCacheRetention",
            ],
            "anthropic-messages" => &[
                "supportsEagerToolInputStreaming",
                "supportsLongCacheRetention",
                "supportsCacheControlOnTools",
                "supportsTemperature",
                "forceAdaptiveThinking",
                "allowEmptySignature",
                "supportsStrictTools",
            ],
            "openai-completions" => &[
                "supportsStore",
                "supportsDeveloperRole",
                "supportsReasoningEffort",
                "supportsUsageInStreaming",
                "supportsFinishReason",
                "maxTokensField",
                "requiresToolResultName",
                "requiresAssistantAfterToolResult",
                "requiresThinkingAsText",
                "requiresReasoningContentOnAssistantMessages",
                "thinkingFormat",
                "chatTemplateKwargs",
                "chatTemplateArgs",
                "supportsThinkingTokenBudget",
                "thinkingTokenBudgetField",
                "vllmPriority",
                "supportsStrictMode",
                "cacheControlFormat",
                "supportsLongCacheRetention",
            ],
            _ => &[],
        };
        if !protocol_fields.contains(&key.as_str()) {
            return Err(invalid(
                "DSH compatibility field is not supported by this protocol",
            ));
        }
        let valid = if booleans.contains(&key.as_str()) {
            value.is_boolean()
        } else {
            match key.as_str() {
                "maxTokensField" => {
                    matches!(value.as_str(), Some("max_completion_tokens" | "max_tokens"))
                }
                "thinkingTokenBudgetField" => matches!(
                    value.as_str(),
                    Some("thinking_token_budget" | "thinking_budget" | "thinking_budget_tokens")
                ),
                "cacheControlFormat" => value.as_str() == Some("anthropic"),
                "thinkingFormat" => value.as_str().is_some_and(|s| {
                    [
                        "openai",
                        "deepseek",
                        "openrouter",
                        "together",
                        "baseten",
                        "zai",
                        "qwen",
                        "chat-template",
                        "qwen-chat-template",
                        "string-thinking",
                        "ant-ling",
                    ]
                    .contains(&s)
                }),
                "vllmPriority" => value.as_i64().is_some(),
                "chatTemplateKwargs" | "chatTemplateArgs" => {
                    value.as_object().is_some_and(|args| {
                        args.values().all(|arg| {
                            arg.is_null()
                                || arg.is_string()
                                || arg.is_number()
                                || arg.is_boolean()
                                || arg.as_object().is_some_and(|variable| {
                                    variable
                                        .keys()
                                        .all(|k| ["$var", "omitWhenOff"].contains(&k.as_str()))
                                        && variable.get("$var").and_then(Value::as_str).is_some_and(
                                            |s| {
                                                [
                                                    "thinking.enabled",
                                                    "thinking.effort",
                                                    "thinking.budget",
                                                ]
                                                .contains(&s)
                                            },
                                        )
                                        && variable.get("omitWhenOff").is_none_or(Value::is_boolean)
                                })
                        })
                    })
                }
                _ => false,
            }
        };
        if !valid {
            return Err(invalid("Invalid DSH compatibility option"));
        }
    }
    Ok(())
}

fn parse_patch(source: &str) -> Result<serde_yaml::Value, AppError> {
    // serde_yaml discards unknown tags in the YAML global namespace (including
    // DSH's !!js). Normalize actual tag tokens for semantic inspection only;
    // the lossless editor still receives the original source.
    let semantic: String = yaml_edit::lex(source)
        .into_iter()
        .map(|(kind, text)| {
            if kind == yaml_edit::SyntaxKind::TAG
                && matches!(text, "!!js" | "!<tag:yaml.org,2002:js>")
            {
                "!dsh_dynamic"
            } else {
                text
            }
        })
        .collect();
    let value: serde_yaml::Value = serde_yaml::from_str(&semantic)
        .map_err(|_| invalid("Cannot parse DSH profile patch YAML"))?;
    if value.is_null() {
        return Ok(serde_yaml::Value::Sequence(vec![]));
    }
    if !value.is_sequence() {
        return Err(invalid("DSH profile patch must be a YAML sequence"));
    }
    Ok(value)
}

fn read_source(path: &std::path::Path) -> Result<String, AppError> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(invalid("DSH configuration files may not be symbolic links"));
    }
    match fs::read_to_string(path) {
        Ok(source) => Ok(source),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(AppError::io(path, e)),
    }
}

fn has_dynamic(value: &serde_yaml::Value) -> bool {
    match value {
        serde_yaml::Value::Tagged(_) => true,
        serde_yaml::Value::Sequence(items) => items.iter().any(has_dynamic),
        serde_yaml::Value::Mapping(map) => {
            map.iter().any(|(k, v)| has_dynamic(k) || has_dynamic(v))
        }
        _ => false,
    }
}

// Resolve installed bundles without running Node or evaluating user !!js expressions.
fn bundle_source(directory: &std::path::Path, bundle: &str) -> Result<Option<String>, AppError> {
    if bundle
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
        || bundle.contains('\\')
        || bundle.starts_with('/')
    {
        return Err(invalid("Invalid DSH bundle package name"));
    }
    let mut anchors = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            if let Ok(binary) =
                fs::canonicalize(directory.join(if cfg!(windows) { "dsh.cmd" } else { "dsh" }))
            {
                if let Some(parent) = binary.parent() {
                    anchors.push(parent.to_path_buf());
                }
                break;
            }
        }
    }
    anchors.push(directory.to_path_buf());
    for anchor in anchors {
        for parent in anchor.ancestors() {
            let package = parent.join("node_modules").join(bundle);
            let manifest_path = package.join("package.json");
            if manifest_path.exists() {
                let manifest: Value = serde_json::from_str(&read_source(&manifest_path)?)
                    .map_err(|_| invalid("Invalid DSH bundle package.json"))?;
                let patches = match manifest.pointer("/dsh/bundle/patch") {
                    Some(Value::String(path)) => vec![path.as_str()],
                    Some(Value::Array(paths)) => paths
                        .iter()
                        .map(|path| {
                            path.as_str()
                                .ok_or_else(|| invalid("Invalid DSH bundle patch path"))
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    None => return Ok(None),
                    _ => return Err(invalid("Invalid DSH bundle patch paths")),
                };
                let mut rows = Vec::new();
                for path in patches {
                    let source = read_source(&package.join(path))?;
                    rows.extend(parse_patch(&source)?.as_sequence().unwrap().iter().cloned());
                }
                return serde_yaml::to_string(&rows)
                    .map(Some)
                    .map_err(|_| invalid("Cannot read DSH bundle patches"));
            }
        }
    }
    // These shipped bundles supply the documented base defaults and no route overrides.
    if [
        "@deepseek-ai/dsh-base",
        "@deepseek-ai/dsh-web",
        "@deepseek-ai/dsh-cli",
        "@deepseek-ai/dsh-tui",
        "@deepseek-ai/dsh-desktop",
    ]
    .contains(&bundle)
    {
        return Ok(None);
    }
    Err(invalid(
        "Cannot resolve DSH profile bundle; install its dependencies before importing or switching",
    ))
}

fn apply_managed_rows(source: &str, configs: &mut serde_yaml::Mapping) -> Result<(), AppError> {
    fn apply(
        rows: &[serde_yaml::Value],
        configs: &mut serde_yaml::Mapping,
    ) -> Result<(), AppError> {
        for row in rows {
            if let Some(insert) = row.get("insert").and_then(serde_yaml::Value::as_sequence) {
                apply(insert, configs)?;
            }
            let Some(id) = row
                .get("id")
                .and_then(serde_yaml::Value::as_str)
                .filter(|id| ["llm-deepseek", "llm-pi-ai", "agent-default-model"].contains(id))
            else {
                continue;
            };
            let expected_name = match id {
                "llm-deepseek" => "@deepseek-ai/dsh-llm-deepseek-api-key",
                "llm-pi-ai" => "@deepseek-ai/dsh-llm-pi-ai",
                _ => "@deepseek-ai/dsh-agent-default-model",
            };
            if row
                .get("name")
                .is_some_and(|name| name.as_str() != Some(expected_name))
            {
                return Err(invalid(
                    "DSH managed plugin name does not match its native bundle entry",
                ));
            }
            if row
                .get("disabled")
                .is_some_and(|value| value.as_bool() != Some(false))
                || row.get("remove").is_some()
            {
                return Err(invalid("DSH provider plugin is disabled or removed"));
            }
            if let Some(config) = row.get("config") {
                if has_dynamic(config) || !config.is_mapping() {
                    return Err(invalid(
                        "Dynamic DSH provider configuration requires the native editor",
                    ));
                }
                configs.insert(serde_yaml::Value::String(id.into()), config.clone());
            }
        }
        Ok(())
    }
    apply(parse_patch(source)?.as_sequence().unwrap(), configs)
}

fn inherited_configs(directory: &std::path::Path) -> Result<serde_yaml::Mapping, AppError> {
    let path = directory.join("package.json");
    let manifest: Value = serde_json::from_str(&read_source(&path)?)
        .map_err(|_| invalid("Invalid DSH profile package.json"))?;
    let mut configs = serde_yaml::Mapping::new();
    for bundle in manifest
        .pointer("/dsh/profile/bundles")
        .and_then(Value::as_array)
        .unwrap()
    {
        let bundle = bundle
            .as_str()
            .ok_or_else(|| invalid("Invalid DSH bundle package name"))?;
        if let Some(source) = bundle_source(directory, bundle)? {
            apply_managed_rows(&source, &mut configs)?;
        }
    }
    Ok(configs)
}

fn effective_configs(directory: &std::path::Path) -> Result<serde_yaml::Mapping, AppError> {
    let mut configs = inherited_configs(directory)?;
    apply_managed_rows(
        &read_source(&directory.join("cordis.patch.yml"))?,
        &mut configs,
    )?;
    apply_managed_rows(
        &read_source(&dsh_config::get_dsh_dir().join("cordis.patch.yml"))?,
        &mut configs,
    )?;
    Ok(configs)
}

fn targeted(row: &serde_yaml::Value, ids: &[&str]) -> bool {
    row.get("id")
        .and_then(serde_yaml::Value::as_str)
        .is_some_and(|id| ids.contains(&id))
        || row
            .get("insert")
            .and_then(serde_yaml::Value::as_sequence)
            .is_some_and(|rows| rows.iter().any(|row| targeted(row, ids)))
}

fn check_overrides(ids: &[&str]) -> Result<(), AppError> {
    let source = read_source(&dsh_config::get_dsh_dir().join("cordis.patch.yml"))?;
    if parse_patch(&source)?
        .as_sequence()
        .unwrap()
        .iter()
        .any(|row| targeted(row, ids))
    {
        return Err(invalid(
            "DSH home patch overrides this provider; edit or remove that override first",
        ));
    }
    Ok(())
}

fn yaml_node(value: &Value) -> Result<yaml_edit::Mapping, AppError> {
    let text =
        serde_json::to_string(value).map_err(|_| invalid("Cannot encode DSH configuration"))?;
    let file =
        yaml_edit::YamlFile::from_str(&text).map_err(|_| invalid("Cannot encode DSH YAML"))?;
    file.document()
        .and_then(|doc| doc.as_mapping())
        .ok_or_else(|| invalid("DSH configuration must be an object"))
}

/// Update existing collections in place so comments on managed fields survive.
fn edit_yaml_collection(current: &yaml_edit::YamlNode, desired: &yaml_edit::YamlNode) -> bool {
    if let (Some(current), Some(desired)) = (current.as_mapping(), desired.as_mapping()) {
        edit_yaml_mapping(current, desired);
        return true;
    }
    if let (Some(current), Some(desired)) = (current.as_sequence(), desired.as_sequence()) {
        for index in (desired.len()..current.len()).rev() {
            current.remove(index);
        }
        for index in 0..desired.len() {
            let next = desired.get(index).unwrap();
            if let Some(previous) = current.get(index) {
                if !yaml_edit::yaml_eq(&previous, &next) && !edit_yaml_collection(&previous, &next)
                {
                    current.set(index, next);
                }
            } else {
                current.push(next);
            }
        }
        return true;
    }
    false
}

fn edit_yaml_mapping(current: &yaml_edit::Mapping, desired: &yaml_edit::Mapping) {
    let removed: Vec<_> = current
        .keys()
        .filter(|key| !desired.contains_key(key))
        .collect();
    for key in removed {
        current.remove(key);
    }
    for (key, next) in desired.iter() {
        if let Some(previous) = current.get(&key) {
            if yaml_edit::yaml_eq(&previous, &next) || edit_yaml_collection(&previous, &next) {
                continue;
            }
        }
        current.set(key, next);
    }
}

fn edit_patch(
    source: &str,
    settings: &Value,
    reference: &str,
    inherited: &serde_yaml::Mapping,
) -> Result<String, AppError> {
    let parsed = parse_patch(source)?;
    let direct = settings
        .get("api")
        .and_then(Value::as_str)
        .unwrap_or("deepseek")
        == "deepseek";
    let id = if direct { "llm-deepseek" } else { "llm-pi-ai" };
    let ids = [id, "agent-default-model"];
    let rows = parsed.as_sequence().unwrap();
    let mut checked = inherited.clone();
    apply_managed_rows(source, &mut checked)?;
    for target in ids {
        if rows.iter().filter(|row| targeted(row, &[target])).count() > 1
            || rows
                .iter()
                .any(|row| row.get("insert").is_some() && targeted(row, &[target]))
        {
            return Err(invalid("Ambiguous DSH provider patch entries"));
        }
        if rows
            .iter()
            .find(|row| targeted(row, &[target]))
            .is_some_and(|row| {
                row.get("disabled")
                    .is_some_and(|v| v.as_bool() != Some(false))
                    || row.get("remove").is_some()
            })
        {
            return Err(invalid("DSH provider plugin is disabled or removed"));
        }
    }
    let input = if rows.is_empty() { "[]\n" } else { source };
    let file = yaml_edit::YamlFile::from_str(input)
        .map_err(|_| invalid("Cannot safely edit DSH patch"))?;
    let sequence = file
        .document()
        .and_then(|doc| doc.as_sequence())
        .ok_or_else(|| invalid("Missing DSH patch sequence"))?;
    let mut native = settings
        .get("providerConfig")
        .cloned()
        .unwrap_or_else(|| json!({}));
    native["baseURL"] = settings
        .get("baseUrl")
        .cloned()
        .unwrap_or_else(|| json!(DEFAULT_BASE_URL));
    native["models"] = settings["models"].clone();
    native["apiKeyEnv"] = json!(reference);
    if !direct {
        native["api"] = settings
            .get("api")
            .cloned()
            .unwrap_or_else(|| json!("openai-completions"));
    }
    for target in ids {
        let index = rows
            .iter()
            .position(|row| row.get("id").and_then(serde_yaml::Value::as_str) == Some(target));
        if index.is_none() {
            let initial = inherited
                .get(serde_yaml::Value::String(target.into()))
                .map(serde_json::to_value)
                .transpose()
                .map_err(|_| invalid("Cannot import inherited DSH provider configuration"))?
                .unwrap_or_else(|| json!({}));
            sequence.push(yaml_node(&json!({"id": target, "config": initial}))?);
        }
        let index = index.unwrap_or_else(|| sequence.len() - 1);
        let row_node = sequence
            .get(index)
            .ok_or_else(|| invalid("Missing DSH patch row"))?;
        let row = row_node
            .as_mapping()
            .ok_or_else(|| invalid("Invalid DSH patch row"))?;
        if row.get_mapping("config").is_none() {
            if row.get("config").is_some() {
                return Err(invalid("DSH plugin config must be a mapping"));
            }
            row.set("config", yaml_edit::YamlValue::mapping());
        }
        let config = row.get_mapping("config").unwrap();
        if target == "agent-default-model" {
            config.set(
                "provider",
                if direct {
                    "deepseek-official"
                } else {
                    PI_ROUTE
                },
            );
            let model = settings
                .get("defaultModel")
                .and_then(Value::as_str)
                .unwrap_or_else(|| settings["models"][0]["id"].as_str().unwrap());
            config.set("model", model);
            if let Some(effort) = settings.get("reasoningEffort").and_then(Value::as_str) {
                config.set("reasoningEffort", effort);
            } else {
                config.remove("reasoningEffort");
            }
        } else if direct {
            edit_yaml_mapping(&config, &yaml_node(&native)?);
        } else {
            if config.get_mapping("providers").is_none() {
                if config.get("providers").is_some() {
                    return Err(invalid("DSH providers must be a mapping"));
                }
                config.set("providers", yaml_edit::YamlValue::mapping());
            }
            let providers = config.get_mapping("providers").unwrap();
            if let Some(route) = providers.get_mapping(PI_ROUTE) {
                edit_yaml_mapping(&route, &yaml_node(&native)?);
            } else {
                providers.set(PI_ROUTE, yaml_node(&native)?);
            }
        }
    }
    let rendered = file.to_string();
    let candidate = parse_patch(&rendered)?;
    let candidate_rows = candidate.as_sequence().unwrap();
    for (index, original) in rows.iter().enumerate() {
        if !targeted(original, &ids) && candidate_rows.get(index) != Some(original) {
            return Err(invalid("DSH patch edit would change another plugin"));
        }
    }
    Ok(rendered)
}

pub fn write_provider(settings: &Value) -> Result<(), AppError> {
    dsh_config::validate_dsh_provider_settings(settings)?;
    if is_legacy(settings) {
        if dsh_config::get_dsh_dir()
            .join("profiles/web/package.json")
            .exists()
        {
            let mut current = read_provider("web")?;
            current["apiKey"] = settings["apiKey"].clone();
            return write_provider(&current);
        }
        return dsh_config::write_dsh_api_key(settings["apiKey"].as_str().unwrap());
    }
    let directory = profile_dir(profile_name(settings))?;
    let _lock = dsh_config::CredentialsWriteLock::acquire_at(
        directory.join("package.json.lock"),
        Duration::from_secs(30),
        "Timed out waiting for the DSH profile writer lock",
    )?;
    let direct = settings
        .get("api")
        .and_then(Value::as_str)
        .unwrap_or("deepseek")
        == "deepseek";
    check_overrides(&[
        if direct { "llm-deepseek" } else { "llm-pi-ai" },
        "agent-default-model",
    ])?;
    let path = directory.join("cordis.patch.yml");
    let source = read_source(&path)?;
    let inherited = inherited_configs(&directory)?;
    let reference = format!("CC_SWITCH_{}", uuid::Uuid::new_v4().simple());
    let rendered = edit_patch(&source, settings, &reference, &inherited)?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(&directory).map_err(|e| AppError::io(&path, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|e| AppError::io(&path, e))?;
    }
    temporary
        .write_all(rendered.as_bytes())
        .map_err(|e| AppError::io(&path, e))?;
    temporary.flush().map_err(|e| AppError::io(&path, e))?;
    // A fresh reference cannot change any current route. Publish the profile last.
    dsh_config::write_dsh_credential_ref(&reference, settings["apiKey"].as_str().unwrap())?;
    temporary
        .persist(&path)
        .map_err(|e| AppError::io(&path, e.error))?;
    Ok(())
}

pub fn read_provider(profile: &str) -> Result<Value, AppError> {
    read_provider_route(profile, None)
}

fn read_provider_route(profile: &str, selected_route: Option<&str>) -> Result<Value, AppError> {
    if !valid_profile_name(profile) {
        return Err(invalid("Invalid DSH profile name"));
    }
    let directory = dsh_config::get_dsh_dir().join("profiles").join(profile);
    if !directory.join("package.json").exists() {
        return dsh_config::read_dsh_api_key()?
            .map(|key| dsh_config::dsh_api_key_settings(&key))
            .ok_or_else(|| invalid("DSH API key is missing"));
    }
    let directory = profile_dir(profile)?;
    let configs = effective_configs(&directory)?;
    let find = |id: &str| configs.get(serde_yaml::Value::String(id.into()));
    let default = find("agent-default-model");
    let default_route = default
        .and_then(|c| c.get("provider"))
        .and_then(serde_yaml::Value::as_str)
        .unwrap_or("deepseek-official");
    let route = selected_route.unwrap_or(default_route);
    let direct = route == "deepseek-official";
    let native = if direct {
        find("llm-deepseek")
    } else {
        find("llm-pi-ai")
            .and_then(|c| c.get("providers"))
            .and_then(|c| c.get(route))
    };
    if !direct && !native.is_some_and(|c| c.get("models").is_some() && c.get("baseURL").is_some()) {
        return Err(invalid("DSH catalog-backed routes require explicit Base URL and models before CC Switch import"));
    }
    let reference = native
        .and_then(|c| c.get("apiKeyEnv"))
        .and_then(serde_yaml::Value::as_str)
        .or(direct.then_some("DEEPSEEK_API_KEY"))
        .ok_or_else(|| {
            invalid("DSH route requires an explicit credential reference for CC Switch import")
        })?;
    let key = dsh_config::read_dsh_credential_ref(reference)?
        .ok_or_else(|| invalid("DSH configured credential reference is missing"))?;
    let mut settings = default_settings(&key);
    settings["profile"] = json!(profile);
    if !direct {
        settings["api"] = json!(native
            .and_then(|c| c.get("api"))
            .and_then(serde_yaml::Value::as_str)
            .ok_or_else(|| invalid("DSH route has no explicit protocol"))?);
    }
    let mut advanced = serde_json::Map::new();
    if let Some(native) = native {
        let native: Value = serde_json::to_value(native)
            .map_err(|_| invalid("DSH provider contains dynamic YAML; use the native editor"))?;
        for (key, value) in native
            .as_object()
            .ok_or_else(|| invalid("Invalid DSH provider config"))?
        {
            match key.as_str() {
                "baseURL" => {
                    settings["baseUrl"] = value.clone();
                }
                "models" => {
                    settings["models"] = value.clone();
                }
                "api" | "apiKeyEnv" => {}
                _ => {
                    advanced.insert(key.clone(), value.clone());
                }
            }
        }
    }
    if !advanced.is_empty() {
        settings["providerConfig"] = Value::Object(advanced);
    }
    let model = default
        .filter(|_| route == default_route)
        .and_then(|c| c.get("model"))
        .and_then(serde_yaml::Value::as_str)
        .unwrap_or_else(|| {
            settings["models"][0]["id"]
                .as_str()
                .unwrap_or("deepseek-flash")
        })
        .to_string();
    settings["defaultModel"] = json!(model);
    if let Some(effort) = default
        .filter(|_| route == default_route)
        .and_then(|c| c.get("reasoningEffort"))
        .and_then(serde_yaml::Value::as_str)
    {
        settings["reasoningEffort"] = json!(effort);
    }
    dsh_config::validate_dsh_provider_settings(&settings)?;
    Ok(settings)
}

pub fn read_all_providers() -> Result<Vec<(String, Value)>, AppError> {
    let root = dsh_config::get_dsh_dir().join("profiles");
    if !root.exists() {
        return Ok(vec![("default".into(), read_provider("web")?)]);
    }
    let mut profiles = fs::read_dir(&root)
        .map_err(|e| AppError::io(&root, e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::io(&root, e))?;
    profiles.sort_by_key(|entry| entry.file_name());
    let mut providers = Vec::new();
    for entry in profiles {
        if !entry.path().join("package.json").exists() {
            continue;
        }
        let profile = entry.file_name().to_string_lossy().to_string();
        let directory = profile_dir(&profile)?;
        let configs = effective_configs(&directory)?;
        let routes = configs
            .get(serde_yaml::Value::String("llm-pi-ai".into()))
            .and_then(|config| config.get("providers"))
            .and_then(serde_yaml::Value::as_mapping);
        let direct_ref = configs
            .get(serde_yaml::Value::String("llm-deepseek".into()))
            .and_then(|config| config.get("apiKeyEnv"))
            .and_then(serde_yaml::Value::as_str)
            .unwrap_or("DEEPSEEK_API_KEY");
        if dsh_config::read_dsh_credential_ref(direct_ref)?.is_some() {
            providers.push((
                format!("{profile}-deepseek"),
                read_provider_route(&profile, Some("deepseek-official"))?,
            ));
        }
        if let Some(routes) = routes {
            for route in routes.keys() {
                let route = route
                    .as_str()
                    .ok_or_else(|| invalid("Invalid DSH route name"))?;
                providers.push((
                    format!("{profile}-{route}"),
                    read_provider_route(&profile, Some(route))?,
                ));
            }
        }
    }
    Ok(providers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnvGuard;

    fn profile() -> (tempfile::TempDir, TestEnvGuard) {
        let home = tempfile::TempDir::new().unwrap();
        let guard = TestEnvGuard::isolated(home.path());
        let directory = dsh_config::get_dsh_dir().join("profiles/web");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("package.json"),
            r#"{"dsh":{"profile":{"bundles":["@deepseek-ai/dsh-base"]}}}"#,
        )
        .unwrap();
        (home, guard)
    }

    #[test]
    fn dsh_full_provider_roundtrip_preserves_plugins_and_grants() {
        let (_home, _guard) = profile();
        dsh_config::write_dsh_api_key("original").unwrap();
        let path = profile_dir("web").unwrap().join("cordis.patch.yml");
        let untouched = "# keep this comment\n- id: webserver\n  config:\n    port: 23080\n    trustedHosts: !!js ctx.webRuntime.trustedHosts\n";
        fs::write(&path, untouched).unwrap();
        for api in DSH_API_PROTOCOLS {
            let mut settings = default_settings("synthetic-key");
            settings["api"] = json!(api);
            settings["baseUrl"] = json!("https://gateway.example/v1");
            settings["models"] = json!([{ "id":"model-a", "contextWindow":200000,"maxTokens":8192 }, {"id":"model-b"}]);
            if *api != "deepseek" {
                settings["models"][1]["reasoningEfforts"] = json!({"high":"high"});
            }
            settings["defaultModel"] = json!("model-b");
            settings["reasoningEffort"] = json!("high");
            settings["providerConfig"] = if *api == "deepseek" {
                json!({"retryPolicy":{"mode":"normal", "maxRetries":3}})
            } else {
                json!({"headers":{"X-Gateway":"synthetic"}})
            };
            write_provider(&settings).unwrap();
            let source = fs::read_to_string(&path).unwrap();
            assert!(source.starts_with(untouched), "{source}");
            assert!(!source.contains("synthetic-key"));
            assert_eq!(
                dsh_config::read_dsh_api_key().unwrap().as_deref(),
                Some("original")
            );
            assert_eq!(read_provider("web").unwrap(), settings);
        }
    }

    #[test]
    fn dsh_managed_provider_fields_keep_nested_and_inline_comments_on_switch() {
        let (_home, _guard) = profile();
        let path = profile_dir("web").unwrap().join("cordis.patch.yml");
        for api in ["deepseek", "openai-responses"] {
            let prefix = if api == "deepseek" {
                "- id: llm-deepseek\n  config:\n"
            } else {
                "- id: llm-pi-ai\n  config:\n    providers:\n      cc-switch:\n"
            };
            let indent = if api == "deepseek" {
                "    "
            } else {
                "        "
            };
            let source = format!("{prefix}{indent}# preserve this operator comment\n{indent}baseURL: https://old.example/v1 # endpoint note\n{indent}models:\n{indent}  - id: old # model note\n{indent}    contextWindow: 32000 # capacity note\n{indent}retryPolicy:\n{indent}  # retry note\n{indent}  mode: normal\n{indent}  maxRetries: 1 # limit note\n");
            fs::write(&path, source).unwrap();
            let mut settings = default_settings("synthetic");
            settings["api"] = json!(api);
            settings["baseUrl"] = json!("https://new.example/v1");
            settings["models"] = json!([{"id":"new", "contextWindow":64000}]);
            settings["defaultModel"] = json!("new");
            settings["providerConfig"] = json!({"retryPolicy":{"mode":"normal","maxRetries":2}});
            write_provider(&settings).unwrap();
            let published = fs::read_to_string(&path).unwrap();
            for comment in [
                "# preserve this operator comment",
                "# endpoint note",
                "# model note",
                "# capacity note",
                "# retry note",
                "# limit note",
            ] {
                assert!(published.contains(comment), "Lost {comment}: {published}");
            }
            assert_eq!(read_provider("web").unwrap(), settings);
        }
    }

    #[test]
    fn dsh_image_model_capacities_respect_native_safe_integer_boundaries() {
        for field in ["imagePixelBudget", "imageMaxBytes"] {
            let mut settings = default_settings("synthetic");
            settings["models"] = json!([{"id":"vision", "inputModalities":["text","image"]}]);
            settings["defaultModel"] = json!("vision");
            for capacity in [0, 9_007_199_254_740_992, u64::MAX] {
                settings["models"][0][field] = json!(capacity);
                assert!(validate_settings(&settings).is_err());
            }
            for capacity in [1_u64, 9_007_199_254_740_991] {
                settings["models"][0][field] = json!(capacity);
                validate_settings(&settings).unwrap();
            }
            if field == "imagePixelBudget" {
                settings["models"][0][field] = json!("low");
                validate_settings(&settings).unwrap();
            }
        }
    }

    #[test]
    fn dsh_disabled_thinking_rejects_default_effort_before_native_publication() {
        let (_home, _guard) = profile();
        let path = profile_dir("web").unwrap().join("cordis.patch.yml");
        let source = "# unchanged\n- id: webserver\n  config: {port: 23080}\n";
        fs::write(&path, source).unwrap();
        dsh_config::write_dsh_api_key("original").unwrap();
        let credential_path = dsh_config::get_dsh_credentials_path();
        let credentials = fs::read(&credential_path).unwrap();
        let mut settings = default_settings("synthetic");
        settings["providerConfig"] = json!({"thinking":"disabled"});
        for effort in ["low", "high", "max"] {
            settings["reasoningEffort"] = json!(effort);
            assert!(write_provider(&settings).is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), source);
            assert_eq!(fs::read(&credential_path).unwrap(), credentials);
        }
        settings["reasoningEffort"] = json!("off");
        write_provider(&settings).unwrap();
        assert_eq!(read_provider("web").unwrap(), settings);
        settings.as_object_mut().unwrap().remove("reasoningEffort");
        write_provider(&settings).unwrap();
        assert_eq!(read_provider("web").unwrap(), settings);
    }

    #[test]
    fn dsh_profile_writer_waits_and_preserves_the_latest_native_patch() {
        let (_home, _guard) = profile();
        let directory = profile_dir("web").unwrap();
        let lock = directory.join("package.json.lock");
        fs::write(&lock, format!("{}\n", std::process::id())).unwrap();
        let (sent, received) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            sent.send(write_provider(&default_settings("synthetic")))
                .unwrap();
        });
        assert!(matches!(
            received.recv_timeout(Duration::from_millis(100)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        let latest = "# native refresh\n- id: webserver\n  config: {port: 23080}\n";
        fs::write(directory.join("cordis.patch.yml"), latest).unwrap();
        fs::remove_file(&lock).unwrap();
        received
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        writer.join().unwrap();
        assert!(fs::read_to_string(directory.join("cordis.patch.yml"))
            .unwrap()
            .starts_with(latest));
        assert_eq!(read_provider("web").unwrap(), default_settings("synthetic"));
    }

    #[test]
    fn dsh_concurrent_full_writes_publish_matching_credentials_and_models() {
        let (_home, _guard) = profile();
        let settings: Vec<_> = (0..8).map(|i| json!({"apiKey":format!("synthetic-{i}"), "api":"openai-responses",
            "baseUrl":format!("https://endpoint-{i}.example/v1"), "models":[{"id":format!("model-{i}")}], "defaultModel":format!("model-{i}")})).collect();
        let writers: Vec<_> = settings
            .iter()
            .cloned()
            .map(|candidate| std::thread::spawn(move || write_provider(&candidate)))
            .collect();
        for writer in writers {
            writer.join().unwrap().unwrap();
        }
        let imported = read_provider("web").unwrap();
        assert!(settings.iter().any(|candidate| {
            let mut candidate = candidate.clone();
            candidate["profile"] = json!("web");
            candidate == imported
        }));
    }

    #[test]
    fn dsh_protocol_options_match_native_ranges_and_types() {
        for (api, options) in [
            ("deepseek", json!({"maxImagesPerRequest":1})),
            ("deepseek", json!({"maxRequestFilesBytes":1024})),
            ("deepseek", json!({"maxInlineRequestImageBytes":1024})),
            ("deepseek", json!({"fileExpiresAfterSeconds":3600})),
            ("deepseek", json!({"fileRefreshMarginSeconds":604800})),
            (
                "deepseek",
                json!({"thinking":"disabled", "reasoningEffort":"high"}),
            ),
            ("deepseek", json!({"fileExpiresAfterSeconds":3599})),
            ("deepseek", json!({"fileQuotaCleanupBatch":1001})),
            ("deepseek", json!({"streamIdleTimeoutMs":2147483648_u64})),
            (
                "openai-responses",
                json!({"compat":{"supportsStore":"yes"}}),
            ),
            ("openai-responses", json!({"compat":{"unknown":true}})),
            ("openai-responses", json!({"displayName":""})),
            ("openai-responses", json!({"thinkingBudgets":{"xhigh":100}})),
            (
                "openai-responses",
                json!({"retryPolicy":{"mode":"normal", "backoff":{"initialDelayMs":2000,"maxDelayMs":1000}}}),
            ),
        ] {
            let mut settings = default_settings("synthetic");
            settings["api"] = json!(api);
            settings["providerConfig"] = options;
            assert!(validate_settings(&settings).is_err());
        }
        let parsed =
            parse_patch("- id: other\n  config: {text: '!!js literal'} # !!js comment\n").unwrap();
        assert!(!has_dynamic(&parsed));
        assert!(has_dynamic(
            &parse_patch("- id: other\n  config: {value: !!js expression}\n").unwrap()
        ));
    }

    #[test]
    fn dsh_pi_default_reasoning_matches_the_selected_models_capabilities() {
        for api in &DSH_API_PROTOCOLS[1..] {
            let mut settings = json!({"apiKey":"synthetic", "api":api,
                "models":[{"id":"plain"},{"id":"thinking", "reasoningEfforts":{"off":null,"low":"low","high":"high"}}]});
            for effort in ["low", "high", "max"] {
                settings["reasoningEffort"] = json!(effort);
                assert!(validate_settings(&settings).is_err());
            }
            settings["reasoningEffort"] = json!("off");
            validate_settings(&settings).unwrap();
            settings["models"][0]["reasoningEfforts"] = json!(false);
            settings["reasoningEffort"] = json!("high");
            assert!(validate_settings(&settings).is_err());
            settings["defaultModel"] = json!("thinking");
            validate_settings(&settings).unwrap();
            settings["reasoningEffort"] = json!("max");
            assert!(validate_settings(&settings).is_err());
            settings.as_object_mut().unwrap().remove("reasoningEffort");
            settings["providerConfig"] = json!({"reasoning":"high"});
            validate_settings(&settings).unwrap();
            settings["defaultModel"] = json!("plain");
            assert!(validate_settings(&settings).is_err());
            settings["reasoningEffort"] = json!("off");
            validate_settings(&settings).unwrap();
            settings["defaultModel"] = json!("thinking");
            settings["models"][1]["reasoningEfforts"]
                .as_object_mut()
                .unwrap()
                .remove("off");
            assert!(validate_settings(&settings).is_err());
        }
    }

    #[test]
    fn dsh_managed_plugin_name_mismatch_leaves_profile_and_credentials_unchanged() {
        let (_home, _guard) = profile();
        dsh_config::write_dsh_api_key("original").unwrap();
        let path = profile_dir("web").unwrap().join("cordis.patch.yml");
        for id in ["llm-deepseek", "llm-pi-ai", "agent-default-model"] {
            let source = format!("- id: {id}\n  name: mismatched-package\n  config: {{}}\n");
            fs::write(&path, &source).unwrap();
            let credentials = fs::read(dsh_config::get_dsh_credentials_path()).unwrap();
            assert!(write_provider(&default_settings("synthetic")).is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), source);
            assert_eq!(
                fs::read(dsh_config::get_dsh_credentials_path()).unwrap(),
                credentials
            );
        }
        for model in [
            json!({"id":"deepseek-flash", "name":""}),
            json!({"id":"deepseek-flash", "imageMaxBytes":100}),
            json!({"id":"deepseek-flash", "inputModalities":["text","text"]}),
        ] {
            let mut settings = default_settings("synthetic");
            settings["models"] = json!([model]);
            assert!(validate_settings(&settings).is_err());
        }
    }

    #[test]
    fn dsh_full_provider_initializes_empty_patch_and_retains_other_pi_routes() {
        let (_home, _guard) = profile();
        let mut settings = default_settings("synthetic");
        write_provider(&settings).unwrap();
        assert_eq!(read_provider("web").unwrap(), settings);
        let path = profile_dir("web").unwrap().join("cordis.patch.yml");
        fs::write(&path, "- id: llm-pi-ai\n  config:\n    providers:\n      other:\n        api: openai-completions\n        baseURL: https://other.example/v1\n        models: [{id: other}]\n").unwrap();
        settings["api"] = json!("openai-responses");
        write_provider(&settings).unwrap();
        let value = parse_patch(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            value[0]["config"]["providers"]["other"]["baseURL"].as_str(),
            Some("https://other.example/v1")
        );
        assert_eq!(read_provider("web").unwrap(), settings);
    }

    #[test]
    fn dsh_full_provider_rejects_overrides_and_invalid_profiles_before_credentials() {
        let (_home, _guard) = profile();
        dsh_config::write_dsh_api_key("original").unwrap();
        let credential_path = dsh_config::get_dsh_credentials_path();
        let original = fs::read(&credential_path).unwrap();
        fs::write(dsh_config::get_dsh_dir().join("cordis.patch.yml"), "- id: agent-default-model\n  config: {provider: deepseek-official, model: deepseek-flash}\n").unwrap();
        assert!(write_provider(&default_settings("new"))
            .unwrap_err()
            .to_string()
            .contains("overrides"));
        assert_eq!(fs::read(&credential_path).unwrap(), original);
        for name in ["../web", ".", "..", "/tmp/web", "missing"] {
            let mut settings = default_settings("new");
            settings["profile"] = json!(name);
            assert!(write_provider(&settings).is_err());
            assert_eq!(fs::read(&credential_path).unwrap(), original);
        }
    }

    #[test]
    fn dsh_full_provider_validates_model_catalog_and_managed_fields() {
        for (field, value) in [
            ("baseUrl", json!("https://secret@example.com")),
            ("baseUrl", json!("https://example.com?secret=x")),
            ("api", json!("unsupported")),
            ("profile", json!("../web")),
            ("defaultModel", json!("missing")),
            ("models", json!([])),
            ("models", json!([{ "id":"same"},{"id":"same"}])),
            ("models", json!([{ "id":"deepseek-flash", "maxTokens":0}])),
            ("providerConfig", json!({"apiKeyEnv":"override"})),
            ("reasoningEffort", json!("medium")),
        ] {
            let mut settings = default_settings("key");
            settings[field] = value;
            assert!(validate_settings(&settings).is_err(), "{field}");
        }
    }

    #[test]
    fn dsh_inherits_bundle_routes_and_imports_home_overrides_without_evaluating_js() {
        let (_home, _guard) = profile();
        let directory = profile_dir("web").unwrap();
        let bundle = directory.join("node_modules/test-provider-bundle");
        fs::create_dir_all(&bundle).unwrap();
        fs::write(
            bundle.join("package.json"),
            r#"{"dsh":{"bundle":{"patch":["./cordis.patch.yml"]}}}"#,
        )
        .unwrap();
        fs::write(
            directory.join("package.json"),
            r#"{"dsh":{"profile":{"bundles":["@deepseek-ai/dsh-base","test-provider-bundle"]}}}"#,
        )
        .unwrap();
        fs::write(bundle.join("cordis.patch.yml"), "- id: llm-pi-ai\n  config:\n    providers:\n      inherited:\n        api: openai-completions\n        baseURL: https://inherited.example/v1\n        apiKeyEnv: CUSTOM_REF\n        models: [{id: inherited-model}]\n").unwrap();
        dsh_config::write_dsh_credential_ref("CUSTOM_REF", "inherited-key").unwrap();
        let mut settings = default_settings("managed-key");
        settings["api"] = json!("openai-responses");
        write_provider(&settings).unwrap();
        let imported = read_all_providers().unwrap();
        assert_eq!(imported.len(), 2);
        assert!(imported.iter().any(|(id, value)| id == "web-inherited"
            && value["baseUrl"] == "https://inherited.example/v1"));
        fs::write(
            dsh_config::get_dsh_dir().join("cordis.patch.yml"),
            "- id: agent-default-model\n  config: {provider: inherited, model: inherited-model}\n",
        )
        .unwrap();
        assert_eq!(
            read_provider("web").unwrap()["defaultModel"],
            "inherited-model"
        );
        assert!(write_provider(&settings).is_err());
    }

    #[test]
    fn dsh_dynamic_managed_config_and_invalid_options_leave_native_files_unchanged() {
        let (_home, _guard) = profile();
        dsh_config::write_dsh_api_key("original-key").unwrap();
        let path = profile_dir("web").unwrap().join("cordis.patch.yml");
        let source =
            "- id: llm-deepseek\n  config:\n    baseURL: !!js process.env.CUSTOM_ENDPOINT\n";
        fs::write(&path, source).unwrap();
        let credential_path = dsh_config::get_dsh_credentials_path();
        let credentials = fs::read(&credential_path).unwrap();
        assert!(read_provider("web").is_err());
        assert!(write_provider(&default_settings("candidate-key")).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), source);
        assert_eq!(fs::read(&credential_path).unwrap(), credentials);
        for options in [
            json!({"headers":{"X-Test":"ok"}}),
            json!({"retryPolicy":{"mode":"invalid"}}),
            json!({"maxTokens":0}),
            json!({"filesApiTimeoutMs":"bad"}),
            json!({"unknown":true}),
        ] {
            let mut settings = default_settings("candidate-key");
            settings["providerConfig"] = options;
            assert!(validate_settings(&settings).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn dsh_rejects_linked_profile_patch_without_touching_target_or_credentials() {
        let (home, _guard) = profile();
        let target = home.path().join("outside.yml");
        fs::write(&target, "[]\n").unwrap();
        let path = profile_dir("web").unwrap().join("cordis.patch.yml");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(write_provider(&default_settings("candidate-key")).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "[]\n");
        assert!(!dsh_config::get_dsh_credentials_path().exists());
    }
}
