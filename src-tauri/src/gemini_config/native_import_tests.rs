use super::*;
use crate::{
    app_config::AppType as CliAppType,
    config::read_json_file,
    gemini_config::{get_gemini_env_path, parse_env_file, parse_env_file_strict},
    test_support::TestEnvGuard,
    AppState, Database, MultiAppConfig, ProviderService, ProxyService,
};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use tempfile::TempDir;

// Independent copy of the previous CLI grammar, not the new shared parser.
fn previous_parse(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim().to_string();
            let value = value.trim().to_string();
            if !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || c == '_') {
                map.insert(key, value);
            }
        }
    }
    map
}

fn previous_read() -> Result<Value, AppError> {
    let path = get_gemini_env_path();
    let env = if path.exists() {
        previous_parse(&fs::read_to_string(&path).map_err(|e| AppError::io(&path, e))?)
    } else {
        HashMap::new()
    };
    let path = get_gemini_settings_path();
    let config: Value = if path.exists() {
        read_json_file(&path)?
    } else {
        json!({})
    };
    Ok(json!({"env":env,"config":config}))
}

fn state() -> AppState {
    let db = Arc::new(Database::init().unwrap());
    AppState {
        db: db.clone(),
        config: RwLock::new(MultiAppConfig::default()),
        proxy_service: ProxyService::new(db),
    }
}

#[test]
fn gemini_assignment_readers_match_previous_grammar_and_localized_errors() {
    for content in [
        "",
        "# comment\r\n\n",
        "invalid\n=empty\nBAD-KEY=skip\nA=valid",
        " 变量 = opaque\0value\nA=x\ry\nÉ2=yes\n9_=yes\ne\u{301}=skip",
        "A = 'literal'\r\nB=a=b # literal\nA=last\nEMPTY=\n",
    ] {
        assert_eq!(parse_env_file(content), previous_parse(content));
    }
    let accepted = "变量=opaque\0value\nA=x\ry\nA=last\nB='literal'";
    assert_eq!(
        parse_env_file_strict(accepted).unwrap(),
        previous_parse(accepted)
    );
    for (line, key, zh, en) in [
        ("invalid", "gemini.env.parse_error.no_equals", "Gemini .env 文件格式错误（第 3 行）：缺少 '=' 分隔符\n行内容: invalid", "Invalid Gemini .env format (line 3): missing '=' separator\nLine: invalid"),
        ("=empty", "gemini.env.parse_error.empty_key", "Gemini .env 文件格式错误（第 3 行）：环境变量名不能为空\n行内容: =empty", "Invalid Gemini .env format (line 3): variable name cannot be empty\nLine: =empty"),
        ("BAD-KEY=skip", "gemini.env.parse_error.invalid_key", "Gemini .env 文件格式错误（第 3 行）：环境变量名只能包含字母、数字和下划线\n变量名: BAD-KEY", "Invalid Gemini .env format (line 3): variable name can only contain letters, numbers, and underscores\nVariable: BAD-KEY"),
    ] {
        let actual = parse_env_file_strict(&format!("# comment\nA=ok\n{line}\nnext-invalid")).unwrap_err();
        assert!(matches!(&actual, AppError::Localized { key: actual, .. } if *actual == key));
        assert_eq!(actual.to_string(), AppError::localized(key, zh, en).to_string());
    }
}

#[test]
fn gemini_real_read_and_import_keep_previous_snapshots_and_catalog_behavior() {
    for settings in [
        None,
        Some("null"),
        Some("false"),
        Some("42"),
        Some("\"opaque\""),
        Some("[1,2]"),
        Some(
            r#"{"security":{"auth":{"selectedType":"oauth-personal","future":true}},"mcpServers":{"fake":{"command":"echo"}},"advanced":{"keep":[1,2]}}"#,
        ),
        Some(r#"{"opaque":1.2345678901234567e-123}"#),
    ] {
        let temp = TempDir::new().unwrap();
        let _guard = TestEnvGuard::isolated(temp.path());
        fs::create_dir_all(get_gemini_env_path().parent().unwrap()).unwrap();
        let env = "# keep\ninvalid\n变量=opaque\0value\nA=first\nA=last\n";
        fs::write(get_gemini_env_path(), env).unwrap();
        if let Some(settings) = settings {
            fs::write(get_gemini_settings_path(), settings).unwrap();
        }
        let expected = previous_read().unwrap();
        assert_eq!(read().unwrap(), expected);
        assert_eq!(
            ProviderService::read_live_settings(CliAppType::Gemini).unwrap(),
            expected
        );
        let state = state();
        assert!(ProviderService::import_default_config(&state, CliAppType::Gemini).unwrap());
        let providers = state.db.get_all_providers("gemini").unwrap();
        let provider = providers.get("default").unwrap();
        assert_eq!(provider.settings_config, expected);
        assert_eq!(provider.name, "default");
        assert_eq!(provider.category.as_deref(), Some("custom"));
        assert_eq!(
            state.db.get_current_provider("gemini").unwrap().as_deref(),
            Some("default")
        );
        let config = state.config.read().unwrap();
        let manager = config.get_manager(&CliAppType::Gemini).unwrap();
        assert_eq!(manager.current, "default");
        assert_eq!(manager.providers["default"].settings_config, expected);
        drop(config);
        assert_eq!(fs::read_to_string(get_gemini_env_path()).unwrap(), env);
        assert_eq!(
            fs::read_to_string(get_gemini_settings_path())
                .ok()
                .as_deref(),
            settings
        );
        // An existing custom provider must short-circuit before any native read.
        fs::write(get_gemini_settings_path(), "{").unwrap();
        assert!(!ProviderService::import_default_config(&state, CliAppType::Gemini).unwrap());
    }
}

#[test]
fn gemini_missing_env_errors_stay_with_callers_and_empty_snapshot_stays_valid() {
    let temp = TempDir::new().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    let state = state();
    assert!(matches!(
        ProviderService::read_live_settings(CliAppType::Gemini),
        Err(AppError::Localized {
            key: "gemini.env.missing",
            ..
        })
    ));
    assert!(matches!(
        ProviderService::import_default_config(&state, CliAppType::Gemini),
        Err(AppError::Localized {
            key: "gemini.live.missing",
            ..
        })
    ));
    // The inner reader historically returns an empty env if it disappears after
    // a caller's presence check. It must not add a second missing-file error.
    assert_eq!(read().unwrap(), previous_read().unwrap());
    fs::create_dir_all(get_gemini_env_path().parent().unwrap()).unwrap();
    fs::write(get_gemini_env_path(), "").unwrap();
    assert!(ProviderService::import_default_config(&state, CliAppType::Gemini).unwrap());
    assert_eq!(
        ProviderService::read_live_settings(CliAppType::Gemini).unwrap(),
        json!({"env":{},"config":{}})
    );
}

#[test]
fn gemini_read_errors_preserve_path_diagnostics_and_read_order_without_persistence() {
    for (env, settings, env_directory, settings_directory) in [
        (&b"A=ok"[..], &b"{"[..], false, false),
        (&b"\xff"[..], &b"{"[..], false, false),
        (&b"A=ok"[..], &b"\xff"[..], false, false),
        (&b""[..], &b"{"[..], true, false),
        (&b"A=ok"[..], &b""[..], false, true),
    ] {
        let temp = TempDir::new().unwrap();
        let _guard = TestEnvGuard::isolated(temp.path());
        fs::create_dir_all(get_gemini_env_path().parent().unwrap()).unwrap();
        if env_directory {
            fs::create_dir(get_gemini_env_path()).unwrap();
        } else {
            fs::write(get_gemini_env_path(), env).unwrap();
        }
        if settings_directory {
            fs::create_dir(get_gemini_settings_path()).unwrap();
        } else {
            fs::write(get_gemini_settings_path(), settings).unwrap();
        }
        let expected = previous_read().unwrap_err().to_string();
        assert_eq!(read().unwrap_err().to_string(), expected);
        assert_eq!(
            ProviderService::read_live_settings(CliAppType::Gemini)
                .unwrap_err()
                .to_string(),
            expected
        );
        let state = state();
        assert_eq!(
            ProviderService::import_default_config(&state, CliAppType::Gemini)
                .unwrap_err()
                .to_string(),
            expected
        );
        assert!(state.db.get_all_providers("gemini").unwrap().is_empty());
        assert!(state.db.get_current_provider("gemini").unwrap().is_none());
    }
}

#[test]
fn gemini_native_snapshot_keeps_trusted_local_document_sizes() {
    let temp = TempDir::new().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    fs::create_dir_all(get_gemini_env_path().parent().unwrap()).unwrap();
    let large = "x".repeat(cc_switch_core::MAX_OPERATION_CONTENT_BYTES + 1);
    fs::write(get_gemini_env_path(), format!("FUTURE={large}")).unwrap();
    fs::write(
        get_gemini_settings_path(),
        format!("{{\"future\":\"{large}\"}}"),
    )
    .unwrap();
    assert_eq!(read().unwrap(), previous_read().unwrap());
    assert_eq!(
        ProviderService::read_live_settings(CliAppType::Gemini).unwrap(),
        previous_read().unwrap()
    );
}
