use super::*;
use serde_json::json;

fn assert_normalization_parity(input: Value) {
    let mut expected = input.clone();
    let mut actual = input;
    let expected_changed = baseline_normalize(&mut expected);
    let changed = ProviderService::normalize_claude_models_in_value(&mut actual);
    assert_eq!(changed, expected_changed);
    assert_eq!(actual, expected);
    assert_eq!(
        serde_json::to_string_pretty(&actual).unwrap(),
        serde_json::to_string_pretty(&expected).unwrap()
    );
    let once = actual.clone();
    assert!(!ProviderService::normalize_claude_models_in_value(
        &mut actual
    ));
    assert_eq!(
        serde_json::to_vec(&actual).unwrap(),
        serde_json::to_vec(&once).unwrap()
    );
}

#[test]
fn model_normalization_matches_baseline_for_native_value_combinations() {
    let values = [
        None,
        Some(Value::Null),
        Some(json!(false)),
        Some(json!({"future": 1})),
        Some(json!("")),
        Some(json!(" main ")),
        Some(json!("雪\t")),
    ];
    let keys = [
        "ANTHROPIC_MODEL",
        "ANTHROPIC_SMALL_FAST_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
    ];
    for case in 0..values.len().pow(keys.len() as u32) {
        let mut input = json!({
            "api_format": "host-metadata",
            "env": {"ANTHROPIC_AUTH_TOKEN": "opaque-token", "extra": {"nested": true}},
            "permissions": {"allow": ["Bash(git*)"]},
            "hooks": {"future": [{"apiFormat": "keep"}]},
            "unknown": [null, {"nested": "preserve"}]
        });
        let env = input["env"].as_object_mut().unwrap();
        let mut indices = case;
        for offset in 0..keys.len() {
            let key = keys[if case % 2 == 0 {
                offset
            } else {
                keys.len() - 1 - offset
            }];
            if let Some(value) = &values[indices % values.len()] {
                env.insert(key.to_string(), value.clone());
            }
            indices /= values.len();
        }
        assert_normalization_parity(input);
    }
}

#[test]
fn model_normalization_preserves_malformed_shapes_and_large_native_settings() {
    for input in [
        Value::Null,
        json!(false),
        json!(17),
        json!("text"),
        json!([]),
        json!({}),
        json!({"env": null}),
        json!({"env": false}),
        json!({"env": []}),
        json!({"env": "text"}),
        json!({"env": 17}),
        json!({"env": {"ANTHROPIC_SMALL_FAST_MODEL": [], "ANTHROPIC_MODEL": 17}}),
        json!({"env": {"ANTHROPIC_MODEL": "main", "ANTHROPIC_DEFAULT_OPUS_MODEL": []}}),
        json!({"env": {"ANTHROPIC_SMALL_FAST_MODEL": "fast"}, "unknown": "x".repeat(1024 * 1024 + 1)}),
    ] {
        assert_normalization_parity(input);
    }
}

#[test]
fn live_metadata_cleanup_matches_baseline_without_migrating_models() {
    for mut input in [
        Value::Null,
        json!(17),
        json!("text"),
        json!([]),
        json!({}),
        json!({
            "env": {"ANTHROPIC_SMALL_FAST_MODEL": "legacy", "api_format": "keep"},
            "api_format": null, "unknown": {"openrouterCompatMode": true},
            "apiFormat": [], "permissions": {"allow": ["Read"]},
            "openrouter_compat_mode": {}, "openrouterCompatMode": false,
            "hooks": [{"apiFormat": "keep"}], "last": "preserve order"
        }),
    ] {
        let mut expected = input.clone();
        baseline_sanitize(&mut expected);
        ProviderService::sanitize_claude_settings_for_live(&mut input);
        assert_eq!(input, expected);
        assert_eq!(
            serde_json::to_vec(&input).unwrap(),
            serde_json::to_vec(&expected).unwrap()
        );
    }
}

// Test-only oracle copied from CLI 0593724d before the delegation.
fn baseline_normalize(settings: &mut Value) -> bool {
    let mut changed = false;
    let env = match settings.get_mut("env") {
        Some(v) if v.is_object() => v.as_object_mut().unwrap(),
        _ => return changed,
    };

    let model = env
        .get("ANTHROPIC_MODEL")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let small_fast = env
        .get("ANTHROPIC_SMALL_FAST_MODEL")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let current_haiku = env
        .get("ANTHROPIC_DEFAULT_HAIKU_MODEL")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let current_sonnet = env
        .get("ANTHROPIC_DEFAULT_SONNET_MODEL")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let current_opus = env
        .get("ANTHROPIC_DEFAULT_OPUS_MODEL")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let target_haiku = current_haiku
        .or_else(|| small_fast.clone())
        .or_else(|| model.clone());
    let target_sonnet = current_sonnet
        .or_else(|| model.clone())
        .or_else(|| small_fast.clone());
    let target_opus = current_opus
        .or_else(|| model.clone())
        .or_else(|| small_fast.clone());

    if env.get("ANTHROPIC_DEFAULT_HAIKU_MODEL").is_none() {
        if let Some(v) = target_haiku {
            env.insert(
                "ANTHROPIC_DEFAULT_HAIKU_MODEL".to_string(),
                Value::String(v),
            );
            changed = true;
        }
    }
    if env.get("ANTHROPIC_DEFAULT_SONNET_MODEL").is_none() {
        if let Some(v) = target_sonnet {
            env.insert(
                "ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(),
                Value::String(v),
            );
            changed = true;
        }
    }
    if env.get("ANTHROPIC_DEFAULT_OPUS_MODEL").is_none() {
        if let Some(v) = target_opus {
            env.insert("ANTHROPIC_DEFAULT_OPUS_MODEL".to_string(), Value::String(v));
            changed = true;
        }
    }

    if env.remove("ANTHROPIC_SMALL_FAST_MODEL").is_some() {
        changed = true;
    }

    changed
}

fn baseline_sanitize(settings: &mut Value) {
    if let Some(obj) = settings.as_object_mut() {
        obj.remove("api_format");
        obj.remove("apiFormat");
        obj.remove("openrouter_compat_mode");
        obj.remove("openrouterCompatMode");
    }
}
