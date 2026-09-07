use super::*;
use crate::{
    gemini_config::{
        get_gemini_env_path, get_gemini_settings_path, json_to_env, serialize_env_file,
    },
    test_support::TestEnvGuard,
};
use std::{collections::HashMap, fs};
use tempfile::TempDir;

fn provider(name: &str, settings: Value) -> Provider {
    Provider::with_id("fixture".to_owned(), name.to_owned(), settings, None)
}

fn assert_preparation(provider: &Provider, snippet: Option<&str>, force: bool) {
    let before = fs::read(get_gemini_settings_path()).ok();
    let expected = ProviderService::baseline_prepare(provider, snippet, None, force);
    let actual = ProviderService::prepare_gemini_live_write(provider, snippet, None, force);
    match (expected, actual) {
        (
            Ok(PreparedLiveWrite::Gemini {
                env: expected_env,
                settings: expected_settings,
                auth_type: expected_auth,
            }),
            Ok(PreparedLiveWrite::Gemini {
                env,
                settings,
                auth_type,
            }),
        ) => {
            assert_eq!(env, expected_env);
            assert_eq!(settings, expected_settings);
            assert_eq!(
                serde_json::to_vec(&settings).unwrap(),
                serde_json::to_vec(&expected_settings).unwrap()
            );
            assert_eq!(auth_type, expected_auth);
        }
        (
            Ok(PreparedLiveWrite::GeminiSecurityFlag {
                auth_type: expected,
            }),
            Ok(PreparedLiveWrite::GeminiSecurityFlag { auth_type }),
        ) => assert_eq!(auth_type, expected),
        (Err(expected), Err(actual)) => assert_eq!(actual.to_string(), expected.to_string()),
        _ => panic!("preparation changed its success/error or synchronization result"),
    }
    assert_eq!(fs::read(get_gemini_settings_path()).ok(), before);
}

#[test]
fn gemini_write_preparation_matches_previous_native_shapes_modes_and_errors() {
    let envs = [
        None,
        Some(json!(null)),
        Some(json!([])),
        Some(json!({})),
        Some(json!({"GEMINI_API_KEY":""})),
        Some(json!({"GEMINI_API_KEY":false,"skip":[1],"OTHER":"kept"})),
        Some(
            json!({"Z":"last","GEMINI_API_KEY":"fake","变量":"raw\0value","BAD-KEY":"x\ry","A":"first"}),
        ),
    ];
    let configs = [
        None,
        Some(json!(null)),
        Some(json!(false)),
        Some(json!({})),
        Some(json!({"security":null})),
        Some(json!({"security":{"auth":[]}})),
        Some(json!({"theme":"dark","advanced":{"opaque":[1,2]}})),
    ];
    let existing = [
        None,
        Some("null"),
        Some("[]"),
        Some("{"),
        Some("{}"),
        Some(
            r#"{"before":1,"security":{"extra":"keep","auth":{"token":"fake","selectedType":"old"}},"theme":"light","mcpServers":{"tool":{"command":"fake"}},"after":2}"#,
        ),
    ];
    for name in ["Google", "Custom"] {
        for env in &envs {
            for config in &configs {
                for existing in existing {
                    let temp = TempDir::new().unwrap();
                    let _guard = TestEnvGuard::isolated(temp.path());
                    fs::create_dir_all(get_gemini_settings_path().parent().unwrap()).unwrap();
                    if let Some(existing) = existing {
                        fs::write(get_gemini_settings_path(), existing).unwrap();
                    }
                    let mut input = json!({"future":{"ignored_by_native_projection":true}});
                    if let Some(env) = env {
                        input["env"] = env.clone();
                    }
                    if let Some(config) = config {
                        input["config"] = config.clone();
                    }
                    assert_preparation(&provider(name, input), None, true);
                    assert!(!get_gemini_env_path().exists());
                }
            }
        }
    }
}

#[test]
fn gemini_env_projection_matches_previous_selection_without_credential_validation() {
    for input in [
        json!(null),
        json!(false),
        json!([]),
        json!({}),
        json!({"env":null}),
        json!({"env":[]}),
        json!({"env":{"GEMINI_API_KEY":"","变量":"raw\0value","BAD-KEY":"x\ry","skip":true,"also_skip":[1]}}),
    ] {
        assert_eq!(
            json_to_env(&input).unwrap(),
            baseline_json_to_env(&input).unwrap()
        );
    }
}

#[test]
fn gemini_write_preparation_keeps_common_config_and_sync_gates() {
    let temp = TempDir::new().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    for name in ["Google", "Custom"] {
        let invalid = provider(name, json!({"env":false,"config":false}));
        assert_preparation(&invalid, Some("{"), false);
        assert!(!get_gemini_settings_path().parent().unwrap().exists());
    }
    fs::create_dir_all(get_gemini_settings_path().parent().unwrap()).unwrap();
    fs::write(
        get_gemini_settings_path(),
        r#"{"mcpServers":{"keep":true},"security":{"auth":{"extra":true}}}"#,
    )
    .unwrap();
    for root in [json!(null), json!(false), json!([]), json!("opaque")] {
        assert_preparation(&provider("Google", root.clone()), None, true);
        assert_preparation(&provider("Custom", root), None, true);
    }
    for apply in [None, Some(false), Some(true)] {
        for snippet in [
            None,
            Some(""),
            Some("{"),
            Some(r#"{"env":{"EXTRA":"common","GEMINI_MODEL":"common-model"}}"#),
        ] {
            let mut provider = provider(
                "Custom",
                json!({"env":{"GEMINI_API_KEY":"fake","GEMINI_MODEL":"provider-model"},"config":{"theme":"dark"}}),
            );
            provider.meta = Some(ProviderMeta {
                apply_common_config: apply,
                ..Default::default()
            });
            assert_preparation(&provider, snippet, false);
        }
    }
}

#[test]
fn gemini_write_preparation_preserves_io_diagnostics_large_values_and_native_file_ownership() {
    for directory in [false, true] {
        let temp = TempDir::new().unwrap();
        let _guard = TestEnvGuard::isolated(temp.path());
        fs::create_dir_all(get_gemini_settings_path().parent().unwrap()).unwrap();
        if directory {
            fs::create_dir(get_gemini_settings_path()).unwrap();
        } else {
            fs::write(get_gemini_settings_path(), [0xff]).unwrap();
        }
        assert_preparation(
            &provider(
                "Custom",
                json!({"env":{"GEMINI_API_KEY":"fake"},"config":{"theme":"dark"}}),
            ),
            None,
            true,
        );
    }
    let temp = TempDir::new().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    fs::create_dir_all(get_gemini_settings_path().parent().unwrap()).unwrap();
    // Preparation must not introduce an env-file read requirement.
    fs::create_dir(get_gemini_env_path()).unwrap();
    fs::write(
        get_gemini_settings_path(),
        r#"{"opaque":1.2345678901234567e-123,"security":{"auth":{"extra":"keep"}}}"#,
    )
    .unwrap();
    let large = "x".repeat(cc_switch_core::MAX_OPERATION_CONTENT_BYTES + 1);
    assert_preparation(
        &provider(
            "Google",
            json!({"env":{"LARGE":large},"config":{"future":{"large":large}}}),
        ),
        None,
        true,
    );
    assert!(get_gemini_env_path().is_dir());
}

#[test]
fn gemini_force_write_uses_the_shared_preparation_without_changing_file_results() {
    for name in ["Google", "Custom"] {
        let temp = TempDir::new().unwrap();
        let _guard = TestEnvGuard::isolated(temp.path());
        fs::create_dir_all(get_gemini_settings_path().parent().unwrap()).unwrap();
        fs::write(get_gemini_env_path(), "OLD=remove\n").unwrap();
        fs::write(get_gemini_settings_path(),r#"{"mcpServers":{"keep":true},"security":{"extra":true,"auth":{"opaque":"keep"}},"advanced":{"keep":[1,2]}}"#).unwrap();
        let provider = provider(
            name,
            json!({"env":{"GEMINI_API_KEY":"fake","变量":"literal","GEMINI_MODEL":"model"},"config":{"theme":"dark"}}),
        );
        let PreparedLiveWrite::Gemini { env, settings, .. } =
            ProviderService::baseline_prepare(&provider, None, None, true).unwrap()
        else {
            panic!("expected native write");
        };
        ProviderService::write_gemini_live_force(&provider, None).unwrap();
        assert_eq!(
            fs::read_to_string(get_gemini_env_path()).unwrap(),
            serialize_env_file(&env)
        );
        assert_eq!(
            read_json_file::<Value>(&get_gemini_settings_path()).unwrap(),
            settings
        );
    }
}

// Test-only baseline from CLI f92fed11. Product-policy helpers remain unchanged;
// native env selection and both settings-overlay stages are copied independently.
fn baseline_json_to_env(settings: &Value) -> Result<HashMap<String, String>, AppError> {
    let mut env_map = HashMap::new();
    if let Some(env_obj) = settings.get("env").and_then(|v| v.as_object()) {
        for (key, value) in env_obj {
            if let Some(value) = value.as_str() {
                env_map.insert(key.clone(), value.to_string());
            }
        }
    }
    Ok(env_map)
}

fn baseline_validate(settings: &Value) -> Result<(), AppError> {
    crate::gemini_config::validate_gemini_settings(settings)?;
    let env_map = baseline_json_to_env(settings)?;
    if !env_map.is_empty() && !env_map.contains_key("GEMINI_API_KEY") {
        return Err(AppError::localized(
            "gemini.validation.missing_api_key",
            "Gemini 配置缺少必需字段: GEMINI_API_KEY",
            "Gemini config missing required field: GEMINI_API_KEY",
        ));
    }
    Ok(())
}

impl ProviderService {
    fn baseline_prepare(
        provider: &Provider,
        common_config_snippet: Option<&str>,
        _previous_common_config_snippet: Option<&str>,
        force_sync: bool,
    ) -> Result<PreparedLiveWrite, AppError> {
        use crate::gemini_config::get_gemini_settings_path;

        let auth_type = Self::detect_gemini_auth_type(provider);
        if !force_sync && !crate::sync_policy::should_sync_live(&AppType::Gemini) {
            return Ok(PreparedLiveWrite::GeminiSecurityFlag { auth_type });
        }

        let content_to_write = Self::baseline_build_effective(
            &AppType::Gemini,
            provider,
            common_config_snippet,
            common_config_snippet.is_some(),
        )?;

        // Upstream parity (write_gemini_live): the .env file is a full OVERWRITE
        // with the provider's effective env (`baseline_json_to_env(provider.settings_config)`
        // upstream), for BOTH auth types. Google Official carries OAuth and skips
        // the API-key validation, but still writes the provider's env verbatim
        // (e.g. GEMINI_MODEL / custom vars) — it does not preserve the prior
        // file's unrelated keys.
        let env = match auth_type {
            GeminiAuthType::GoogleOfficial => baseline_json_to_env(&content_to_write)?,
            GeminiAuthType::ApiKey => {
                baseline_validate(&content_to_write)?;
                baseline_json_to_env(&content_to_write)?
            }
        };

        let mut incoming_config = match content_to_write.get("config") {
            Some(Value::Null) | None => json!({}),
            Some(config_value) => {
                if let Some(provider_config) = config_value.as_object() {
                    Value::Object(provider_config.clone())
                } else {
                    return Err(AppError::localized(
                        "gemini.validation.invalid_config",
                        "Gemini 配置格式错误: config 必须是对象或 null",
                        "Gemini config invalid: config must be an object or null",
                    ));
                }
            }
        };

        let config_obj = incoming_config.as_object_mut().ok_or_else(|| {
            AppError::localized(
                "gemini.validation.invalid_config",
                "Gemini 配置格式错误: config 必须是对象或 null",
                "Gemini config invalid: config must be an object or null",
            )
        })?;
        let security = config_obj
            .entry("security".to_string())
            .or_insert_with(|| json!({}));
        let security_obj = security.as_object_mut().ok_or_else(|| {
            AppError::localized(
                "gemini.validation.invalid_security",
                "Gemini 配置格式错误: security 必须是对象",
                "Gemini config invalid: security must be an object",
            )
        })?;
        let auth = security_obj
            .entry("auth".to_string())
            .or_insert_with(|| json!({}));
        let auth_obj = auth.as_object_mut().ok_or_else(|| {
            AppError::localized(
                "gemini.validation.invalid_security_auth",
                "Gemini 配置格式错误: security.auth 必须是对象",
                "Gemini config invalid: security.auth must be an object",
            )
        })?;
        auth_obj.insert(
            "selectedType".to_string(),
            Value::String(Self::gemini_security_selected_type(auth_type).to_string()),
        );

        // Upstream parity (write_gemini_live): settings.json is a SHALLOW merge
        // of the provider's config keys into the existing file, preserving
        // unrelated user fields such as mcpServers. Only the .env file is a full
        // overwrite.
        let settings_path = get_gemini_settings_path();
        let mut settings = if settings_path.exists() {
            read_json_file::<Value>(&settings_path)?
        } else {
            json!({})
        };
        if !settings.is_object() {
            settings = json!({});
        }
        if let (Some(settings_obj), Some(incoming_obj)) =
            (settings.as_object_mut(), incoming_config.as_object())
        {
            for (key, value) in incoming_obj {
                settings_obj.insert(key.clone(), value.clone());
            }
        }

        Ok(PreparedLiveWrite::Gemini {
            env,
            settings,
            auth_type,
        })
    }

    fn baseline_build_effective(
        app_type: &AppType,
        provider: &Provider,
        common_config_snippet: Option<&str>,
        apply_common_config: bool,
    ) -> Result<Value, AppError> {
        let apply_common_config = Self::resolve_live_apply_common_config(
            app_type,
            provider,
            common_config_snippet,
            apply_common_config,
        );

        let content_to_write = common_config::build_effective_settings_with_common_config(
            app_type,
            provider,
            common_config_snippet,
            apply_common_config,
        )?;

        let env_obj = content_to_write
            .get("env")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let settings_path = crate::gemini_config::get_gemini_settings_path();
        let config_value = if let Some(config_value) = content_to_write.get("config") {
            if config_value.is_null() {
                if settings_path.exists() {
                    read_json_file(&settings_path)?
                } else {
                    json!({})
                }
            } else if let Some(provider_config) = config_value.as_object() {
                if provider_config.is_empty() {
                    if settings_path.exists() {
                        read_json_file(&settings_path)?
                    } else {
                        json!({})
                    }
                } else {
                    let mut merged = if settings_path.exists() {
                        read_json_file(&settings_path)?
                    } else {
                        json!({})
                    };

                    if !merged.is_object() {
                        merged = json!({});
                    }

                    let merged_map = merged.as_object_mut().ok_or_else(|| {
                        AppError::localized(
                            "gemini.validation.invalid_settings",
                            "Gemini 现有 settings.json 格式错误: 必须是对象",
                            "Gemini existing settings.json invalid: must be a JSON object",
                        )
                    })?;
                    for (key, value) in provider_config {
                        merged_map.insert(key.clone(), value.clone());
                    }
                    merged
                }
            } else {
                return Err(AppError::localized(
                    "gemini.validation.invalid_config",
                    "Gemini 配置格式错误: config 必须是对象或 null",
                    "Gemini config invalid: config must be an object or null",
                ));
            }
        } else if settings_path.exists() {
            read_json_file(&settings_path)?
        } else {
            json!({})
        };

        Ok(json!({
            "env": env_obj,
            "config": config_value,
        }))
    }
}
