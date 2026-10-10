use serde_json::json;

use cc_switch_lib::{AppSettings, AppState, AppType, MultiAppConfig, Provider, ProviderService};

#[path = "support.rs"]
mod support;
use support::{ensure_test_home, lock_test_mutex, reset_test_fs, state_from_config};

fn dsh_provider(id: &str, api_key: &str) -> Provider {
    Provider::with_id(
        id.to_string(),
        id.to_string(),
        json!({ "apiKey": api_key }),
        None,
    )
}

fn run_dsh_cli(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    let mut dsh_args = vec!["--app", "dsh"];
    dsh_args.extend_from_slice(args);
    run_cli(home, &dsh_args)
}

fn run_cli(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    cli_command(home, args)
        .output()
        .expect("run isolated DSH command")
}

fn cli_command(home: &std::path::Path, args: &[&str]) -> std::process::Command {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_cc-switch"));
    command
        .args(args)
        .env("HOME", home)
        .env("CC_SWITCH_CONFIG_DIR", home.join(".cc-switch"))
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("CODEX_HOME", home.join(".codex"))
        .env("DSH_HOME", home.join(".dsh"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_RUNTIME_DIR", home.join(".runtime"))
        .env("XDG_STATE_HOME", home.join(".state"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env_remove("CC_SWITCH_DAEMON_SOCKET");
    command
}

fn seed_dsh_credentials(api_key: &str) {
    let path = ensure_test_home().join(".dsh/.credentials.yaml");
    std::fs::create_dir_all(path.parent().expect("credentials parent")).expect("create .dsh");
    std::fs::write(
        path,
        format!(
            "version: 1\nrefs:\n  DEEPSEEK_API_KEY: {api_key}\nrecords:\n  client-connection/browser-session:\n    kind: grant\n    payload:\n      secret: preserve\n"
        ),
    )
    .expect("write DSH credentials");
}

fn dsh_config() -> MultiAppConfig {
    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Dsh);
    let manager = config.get_manager_mut(&AppType::Dsh).expect("DSH manager");
    manager.current = "old".to_string();
    manager
        .providers
        .insert("old".to_string(), dsh_provider("old", "old-key"));
    manager
        .providers
        .insert("new".to_string(), dsh_provider("new", "new-key"));
    config
}

#[test]
fn dsh_current_masks_api_keys_in_command_output() {
    let _lock = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    for (id, api_key) in [
        ("long", "synthetic-current-secret-key"),
        ("short", "q7"),
        ("unicode", "synthetic-密钥-secret"),
    ] {
        for args in [
            vec![
                "provider",
                "add",
                "--name",
                id,
                "--id",
                id,
                "--api-key",
                api_key,
            ],
            vec!["provider", "switch", id],
            vec!["provider", "list"],
            vec!["provider", "current"],
        ] {
            let output = run_dsh_cli(home, &args);
            assert!(output.status.success(), "DSH command should succeed");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(!stdout.contains(api_key), "stdout must not expose the key");
            assert!(!stderr.contains(api_key), "stderr must not expose the key");
            if args.last() == Some(&"current") {
                assert!(stdout.contains("API Key:  ********"));
                assert!(stdout.contains(id));
            }
        }
    }
}

#[test]
fn dsh_concurrent_cli_switches_keep_credentials_and_both_selections_consistent() {
    let _lock = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    seed_dsh_credentials("old-key");
    let state = state_from_config(dsh_config());
    let native_lock = home.join(".dsh/.credentials.yaml.lock");
    std::fs::write(&native_lock, format!("{}\n", std::process::id())).unwrap();
    let spawn = |id: &str| {
        cli_command(home, &["--app", "dsh", "provider", "switch", id])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    };
    let first = spawn("new");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let read_selection = || {
        std::fs::read(home.join(".cc-switch/settings.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|value| {
                value
                    .get("currentProviderDsh")
                    .and_then(|id| id.as_str())
                    .map(str::to_owned)
            })
    };
    while read_selection().as_deref() != Some("new") && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let first_reached_publication = read_selection().as_deref() == Some("new");
    let mut second = spawn("old");
    std::thread::sleep(std::time::Duration::from_millis(250));
    let second_waited = second.try_wait().unwrap().is_none();
    let selection_while_locked = read_selection();
    let db_selection_while_locked = state.db.get_current_provider("dsh").unwrap();
    // Always release the native lock and collect both children before asserting.
    std::fs::remove_file(&native_lock).unwrap();
    let first_output = first.wait_with_output().unwrap();
    let second_output = second.wait_with_output().unwrap();
    assert!(first_reached_publication);
    assert!(second_waited);
    assert_eq!(selection_while_locked.as_deref(), Some("new"));
    assert_eq!(db_selection_while_locked.as_deref(), Some("new"));
    assert!(first_output.status.success());
    assert!(second_output.status.success());
    assert_eq!(
        ProviderService::read_live_settings(AppType::Dsh)
            .unwrap()
            .get("apiKey")
            .and_then(serde_json::Value::as_str),
        Some("old-key")
    );
    assert_eq!(read_selection().as_deref(), Some("old"));
    assert_eq!(
        state.db.get_current_provider("dsh").unwrap().as_deref(),
        Some("old")
    );
    assert!(!home.join(".dsh/.cc-switch-provider.lock").exists());
}

#[test]
fn dsh_provider_operations_refresh_stale_snapshots_before_saving() {
    let _lock = lock_test_mutex();
    reset_test_fs();
    seed_dsh_credentials("old-key");
    let first = state_from_config(dsh_config());
    let stale = AppState::try_new().unwrap();
    ProviderService::add(&first, AppType::Dsh, dsh_provider("third", "third-key")).unwrap();
    ProviderService::update(&first, AppType::Dsh, dsh_provider("old", "refreshed-key")).unwrap();
    ProviderService::switch(&stale, AppType::Dsh, "old").unwrap();
    let providers = stale.db.get_all_providers("dsh").unwrap();
    assert!(providers.contains_key("third"));
    assert_eq!(
        providers["old"].settings_config,
        json!({"apiKey": "refreshed-key"})
    );
    assert_eq!(
        ProviderService::read_live_settings(AppType::Dsh).unwrap(),
        json!({"apiKey": "refreshed-key"})
    );
}

#[test]
fn dsh_switch_updates_only_api_key_and_persists_current_provider() {
    let _lock = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    seed_dsh_credentials("old-key");

    let state: AppState = state_from_config(dsh_config());
    ProviderService::switch(&state, AppType::Dsh, "new").expect("switch DSH provider");

    let source =
        std::fs::read_to_string(home.join(".dsh/.credentials.yaml")).expect("read DSH credentials");
    let value: serde_yaml::Value = serde_yaml::from_str(&source).expect("parse DSH credentials");
    assert_eq!(value["refs"]["DEEPSEEK_API_KEY"].as_str(), Some("new-key"));
    assert_eq!(
        value["records"]["client-connection/browser-session"]["payload"]["secret"].as_str(),
        Some("preserve")
    );
    assert_eq!(
        AppSettings::load().current_provider_dsh.as_deref(),
        Some("new")
    );
    assert_eq!(
        state
            .db
            .get_current_provider(AppType::Dsh.as_str())
            .expect("read current DSH provider")
            .as_deref(),
        Some("new")
    );
}

#[test]
fn dsh_switch_settings_failure_leaves_live_and_selection_unchanged() {
    let _lock = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    seed_dsh_credentials("old-key");
    let state = state_from_config(dsh_config());
    cc_switch_lib::update_settings(AppSettings {
        current_provider_dsh: Some("old".into()),
        ..AppSettings::default()
    })
    .unwrap();
    let credentials = home.join(".dsh/.credentials.yaml");
    let original = std::fs::read(&credentials).unwrap();
    let settings = home.join(".cc-switch/settings.json");
    std::fs::remove_file(&settings).unwrap();
    std::fs::create_dir(&settings).unwrap();

    assert!(ProviderService::switch(&state, AppType::Dsh, "new").is_err());
    assert_eq!(std::fs::read(&credentials).unwrap(), original);
    assert_eq!(
        state.db.get_current_provider("dsh").unwrap().as_deref(),
        Some("old")
    );
    assert_eq!(
        ProviderService::current(&state, AppType::Dsh).unwrap(),
        "old"
    );
    assert_eq!(
        state
            .config
            .read()
            .unwrap()
            .get_manager(&AppType::Dsh)
            .unwrap()
            .current,
        "old"
    );
    assert!(settings.is_dir());

    let output = run_dsh_cli(home, &["provider", "switch", "new"]);
    assert!(!output.status.success());
    assert_eq!(std::fs::read(&credentials).unwrap(), original);
    assert_eq!(
        state.db.get_current_provider("dsh").unwrap().as_deref(),
        Some("old")
    );
    assert!(settings.is_dir());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("new-key"));
}

#[test]
fn dsh_switch_lock_timeout_preserves_native_refresh_and_rolls_back_selection() {
    let _lock = lock_test_mutex();
    reset_test_fs();
    let path = ensure_test_home().join(".dsh/.credentials.yaml");
    seed_dsh_credentials("old-key");
    let state = state_from_config(dsh_config());
    cc_switch_lib::update_settings(AppSettings {
        current_provider_dsh: Some("old".to_string()),
        ..AppSettings::default()
    })
    .unwrap();
    let lock_path = path.with_file_name(".credentials.yaml.lock");
    let holder = format!("{}\n", std::process::id());
    std::fs::write(&lock_path, &holder).unwrap();
    let refreshed = "version: 1\nrefs:\n  DEEPSEEK_API_KEY: old-key\nrecords:\n  browser-session:\n    kind: grant\n    payload:\n      secret: refreshed-during-switch\n";
    let native_path = path.clone();
    let native_writer = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        std::fs::write(native_path, refreshed).unwrap();
    });

    let error = ProviderService::switch(&state, AppType::Dsh, "new").unwrap_err();
    native_writer.join().unwrap();
    assert!(error.to_string().contains("writer lock"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), refreshed);
    assert_eq!(std::fs::read_to_string(&lock_path).unwrap(), holder);
    assert_eq!(
        state.db.get_current_provider("dsh").unwrap().as_deref(),
        Some("old")
    );
    assert_eq!(
        AppSettings::load().current_provider_dsh.as_deref(),
        Some("old")
    );
    assert_eq!(
        state
            .config
            .read()
            .unwrap()
            .get_manager(&AppType::Dsh)
            .unwrap()
            .current,
        "old"
    );
    std::fs::remove_file(lock_path).unwrap();
}

#[test]
fn dsh_provider_rejects_base_url_or_model_settings() {
    let _lock = lock_test_mutex();
    reset_test_fs();
    ensure_test_home();
    seed_dsh_credentials("old-key");

    let state = state_from_config(MultiAppConfig::default());
    let invalid = Provider::with_id(
        "invalid".to_string(),
        "Invalid DSH".to_string(),
        json!({ "apiKey": "key", "baseUrl": "https://example.test" }),
        None,
    );
    let error = ProviderService::add(&state, AppType::Dsh, invalid)
        .expect_err("DSH must reject provider-managed base URL");
    assert!(error.to_string().contains("only apiKey"));
}

#[test]
fn dsh_imported_provider_with_unsupported_fields_cannot_switch() {
    let _lock = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    seed_dsh_credentials("old-key");
    let credentials = home.join(".dsh/.credentials.yaml");
    let original = std::fs::read(&credentials).unwrap();
    let mut config = dsh_config();
    config
        .get_manager_mut(&AppType::Dsh)
        .unwrap()
        .providers
        .get_mut("new")
        .unwrap()
        .settings_config =
        json!({"apiKey": "new-key", "baseUrl": "https://example.invalid", "model": "custom"});
    let storage = home.join(".cc-switch");
    std::fs::create_dir_all(&storage).unwrap();
    std::fs::write(
        storage.join("config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();

    let output = run_dsh_cli(home, &["use", "new"]);
    assert!(!output.status.success());
    let message = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(message.contains("only apiKey"));
    assert!(!message.contains("new-key"));
    assert_eq!(std::fs::read(&credentials).unwrap(), original);
    let state = AppState::try_new().unwrap();
    assert_eq!(
        state.db.get_current_provider("dsh").unwrap().as_deref(),
        Some("old")
    );
    assert_eq!(
        ProviderService::current(&state, AppType::Dsh).unwrap(),
        "old"
    );
    assert_eq!(AppSettings::load().current_provider_dsh, None);
    assert!(ProviderService::switch(&state, AppType::Dsh, "new").is_err());
    assert_eq!(std::fs::read(&credentials).unwrap(), original);
    assert_eq!(
        state.db.get_current_provider("dsh").unwrap().as_deref(),
        Some("old")
    );
}

#[test]
fn dsh_switch_rejects_invalid_credentials_without_changing_current_or_live_file() {
    let _lock = lock_test_mutex();
    reset_test_fs();
    let path = ensure_test_home().join(".dsh/.credentials.yaml");
    seed_dsh_credentials("old-key");
    let state = state_from_config(dsh_config());
    cc_switch_lib::update_settings(AppSettings {
        current_provider_dsh: Some("old".to_string()),
        ..AppSettings::default()
    })
    .expect("select old DSH provider");
    let source = "version: 2\nrefs:\n  DEEPSEEK_API_KEY: old-key\n";
    std::fs::write(&path, source).expect("write unsupported credentials version");

    ProviderService::switch(&state, AppType::Dsh, "new")
        .expect_err("invalid credentials must block switching");
    assert_eq!(std::fs::read_to_string(path).unwrap(), source);
    assert_eq!(
        AppSettings::load().current_provider_dsh.as_deref(),
        Some("old")
    );
    assert_eq!(
        state.db.get_current_provider("dsh").unwrap().as_deref(),
        Some("old")
    );
}

#[test]
fn dsh_common_config_service_rejects_mutations_without_side_effects() {
    let _lock = lock_test_mutex();
    reset_test_fs();
    let path = ensure_test_home().join(".dsh/.credentials.yaml");
    seed_dsh_credentials("old-key");
    let state = state_from_config(dsh_config());
    let source = std::fs::read(&path).unwrap();
    let providers = serde_json::to_value(state.db.get_all_providers("dsh").unwrap()).unwrap();
    for result in [
        ProviderService::set_common_config_snippet(&state, AppType::Dsh, Some("{}".to_string())),
        ProviderService::clear_common_config_snippet(&state, AppType::Dsh),
        ProviderService::extract_common_config_snippet_from_settings(
            AppType::Dsh,
            &json!({ "apiKey": "old-key" }),
        )
        .map(|_| ()),
    ] {
        assert!(result.unwrap_err().to_string().contains("does not support"));
    }
    assert_eq!(std::fs::read(path).unwrap(), source);
    assert_eq!(state.db.get_config_snippet("dsh").unwrap(), None);
    assert_eq!(
        serde_json::to_value(state.db.get_all_providers("dsh").unwrap()).unwrap(),
        providers
    );
    assert_eq!(
        state.db.get_current_provider("dsh").unwrap().as_deref(),
        Some("old")
    );
}

#[test]
fn dsh_usage_queries_reject_saved_scripts_without_network_requests() {
    let _lock = lock_test_mutex();
    reset_test_fs();
    ensure_test_home();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut config = dsh_config();
    let provider = config
        .get_manager_mut(&AppType::Dsh)
        .unwrap()
        .providers
        .get_mut("old")
        .unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    provider.meta = Some(cc_switch_lib::ProviderMeta {
        usage_script: Some(serde_json::from_value(json!({
            "enabled": true,
            "language": "javascript",
            "code": format!("({{ request: {{ url: '{url}', method: 'GET' }}, extractor: function() {{ return {{ remaining: 1 }}; }} }})"),
            "timeout": 1,
            "apiKey": "override-key",
            "baseUrl": url,
            "templateType": "custom"
        })).unwrap()),
        ..Default::default()
    });
    let state = state_from_config(config);
    let result =
        futures::executor::block_on(ProviderService::query_usage(&state, AppType::Dsh, "old"));
    assert!(result.unwrap_err().to_string().contains("does not support"));
    let result = futures::executor::block_on(ProviderService::query_provider_usage(
        &state,
        AppType::Dsh,
        "old",
    ));
    assert!(result.unwrap_err().contains("does not support"));
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn dsh_unsupported_cli_commands_leave_provider_mcp_and_credentials_unchanged() {
    let _lock = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    seed_dsh_credentials("old-key");
    let state = state_from_config(dsh_config());
    let server: cc_switch_lib::McpServer = serde_json::from_value(json!({
        "id": "shared", "name": "Shared", "server": { "command": "unused" },
        "apps": { "claude": true }
    }))
    .unwrap();
    state.db.save_mcp_server(&server).unwrap();
    let credentials = std::fs::read(home.join(".dsh/.credentials.yaml")).unwrap();
    let providers = serde_json::to_value(state.db.get_all_providers("dsh").unwrap()).unwrap();
    let servers = serde_json::to_value(state.db.get_all_mcp_servers().unwrap()).unwrap();
    for args in [
        vec!["mcp", "list"],
        vec!["mcp", "sync"],
        vec!["mcp", "import"],
        vec!["mcp", "delete", "shared"],
        vec!["prompts", "list"],
        vec!["prompts", "create", "--id", "unsupported"],
        vec!["skills", "list"],
        vec!["sessions", "list", "--json"],
        vec!["sessions", "sync-usage", "--json"],
        vec!["skills", "uninstall", "unsupported"],
        vec!["config", "common", "show"],
        vec!["config", "common", "clear"],
        vec!["config", "common", "set", "--snippet", "{}"],
        vec![
            "config",
            "common",
            "extract",
            "--settings-config",
            "{\"apiKey\":\"old-key\"}",
            "--save",
        ],
        vec!["provider", "usage-query", "show", "old"],
        vec!["provider", "usage-query", "clear", "old"],
        vec![
            "provider",
            "usage-query",
            "set",
            "old",
            "--enabled",
            "--template",
            "custom",
        ],
    ] {
        let output = run_dsh_cli(home, &args);
        assert!(!output.status.success(), "unexpected success: {args:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("does not support"),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(
        std::fs::read(home.join(".dsh/.credentials.yaml")).unwrap(),
        credentials
    );
    assert_eq!(
        serde_json::to_value(state.db.get_all_providers("dsh").unwrap()).unwrap(),
        providers
    );
    assert_eq!(
        serde_json::to_value(state.db.get_all_mcp_servers().unwrap()).unwrap(),
        servers
    );
    assert_eq!(state.db.get_config_snippet("dsh").unwrap(), None);
    assert_eq!(
        state.db.get_current_provider("dsh").unwrap().as_deref(),
        Some("old")
    );
}

#[test]
fn dsh_unsupported_cli_commands_reject_before_initial_database_import() {
    let _lock = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    seed_dsh_credentials("old-key");
    let credentials = std::fs::read(home.join(".dsh/.credentials.yaml")).unwrap();
    let database = home.join(".cc-switch/cc-switch.db");
    assert!(!database.exists());
    for args in [
        vec!["mcp", "list"],
        vec!["prompts", "list"],
        vec!["skills", "list"],
        vec!["sessions", "list", "--json"],
        vec!["sessions", "sync-usage", "--json"],
        vec!["config", "common", "clear"],
        vec!["provider", "usage-query", "show", "old"],
        vec!["provider", "quota", "old", "--json"],
        vec!["provider", "speedtest", "old"],
        vec!["provider", "stream-check", "old"],
        vec!["provider", "fetch-models", "old"],
        vec!["provider", "export", "old"],
        vec!["provider", "set-default", "old"],
        vec!["provider", "remove-from-config", "old"],
        vec!["proxy", "enable"],
        vec!["proxy", "config", "--listen-port", "18090"],
    ] {
        let output = run_dsh_cli(home, &args);
        assert!(!output.status.success(), "unexpected success: {args:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("does not support"),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!database.exists(), "startup created database for {args:?}");
        assert_eq!(
            std::fs::read(home.join(".dsh/.credentials.yaml")).unwrap(),
            credentials
        );
    }
}

#[test]
fn dsh_session_provider_targets_reject_before_database_access() {
    let _lock = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    seed_dsh_credentials("old-key");
    let credentials = std::fs::read(home.join(".dsh/.credentials.yaml")).unwrap();
    for alias in ["dsh", "deepseek", "deepseek-harness"] {
        for command in ["list", "sync-usage"] {
            let output = run_cli(home, &["sessions", command, "--provider", alias, "--json"]);
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("unsupported provider"));
            assert!(!home.join(".cc-switch/cc-switch.db").exists());
            assert_eq!(
                std::fs::read(home.join(".dsh/.credentials.yaml")).unwrap(),
                credentials
            );
        }
    }
}

#[test]
fn dsh_add_preflight_rejects_unsupported_options_without_import() {
    let _lock = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    seed_dsh_credentials("old-key");
    let path = home.join(".dsh/.credentials.yaml");
    let credentials = std::fs::read(&path).unwrap();
    let settings = std::fs::read(home.join(".cc-switch/settings.json")).unwrap();
    let raw_file = home.join("unsupported-dsh.json");
    std::fs::write(
        &raw_file,
        r#"{"apiKey":"synthetic-new","baseUrl":"https://example.invalid"}"#,
    )
    .unwrap();
    for alias in ["dsh", "deepseek", "deepseek-harness"] {
        let mut cases = vec![
            vec!["--api-key", "synthetic-new", "--template", "deepseek"],
            vec!["--api-key", "synthetic-new", "--api-key-field", "api-key"],
            vec!["--api-key", "synthetic-new", "--max-output-tokens", "100"],
            vec!["--api-key", "synthetic-new", "--impersonate-claude-code"],
            vec!["--api-key", "synthetic-new", "--common-config"],
            vec!["--api-key", "synthetic-new", "--fast-mode"],
            vec![
                "--config",
                r#"{"apiKey":"synthetic-new","baseUrl":"https://example.invalid"}"#,
            ],
            vec![
                "--config",
                r#"{"apiKey":"synthetic-new","model":"unmanaged"}"#,
            ],
            vec!["--config", r#"{"apiKey":""}"#],
            vec!["--config-file", raw_file.to_str().unwrap()],
        ];
        for flag in [
            "--base-url",
            "--model",
            "--haiku-model",
            "--sonnet-model",
            "--opus-model",
            "--fable-model",
            "--subagent-model",
            "--api-format",
            "--account-id",
        ] {
            cases.push(vec!["--api-key", "synthetic-new", flag, "unsupported"]);
        }
        for options in cases {
            let mut args = vec!["--app", alias, "provider", "add", "--name", "Reject"];
            args.extend(options);
            let output = run_cli(home, &args);
            assert!(!output.status.success(), "unexpected success: {args:?}");
            assert!(!output.stderr.is_empty());
            assert!(!home.join(".cc-switch/cc-switch.db").exists());
            assert_eq!(std::fs::read(&path).unwrap(), credentials);
            assert_eq!(
                std::fs::read(home.join(".cc-switch/settings.json")).unwrap(),
                settings
            );
        }
    }
}

#[test]
fn dsh_add_accepts_custom_template_and_raw_api_key_config() {
    let _lock = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let raw_file = home.join("supported-dsh.json");
    std::fs::write(&raw_file, r#"{"apiKey":"synthetic-file"}"#).unwrap();
    for (id, options) in [
        (
            "field",
            vec!["--template", "custom", "--api-key", "synthetic-field"],
        ),
        (
            "inline",
            vec!["--config", r#"{"apiKey":"synthetic-inline"}"#],
        ),
        ("file", vec!["--config-file", raw_file.to_str().unwrap()]),
    ] {
        let mut args = vec!["provider", "add", "--id", id, "--name", id];
        args.extend(options);
        let output = run_dsh_cli(home, &args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let state = AppState::try_new().unwrap();
    let providers = state.db.get_all_providers("dsh").unwrap();
    for id in ["field", "inline", "file"] {
        assert_eq!(
            providers[id].settings_config,
            json!({"apiKey": format!("synthetic-{id}")})
        );
    }
}

#[test]
fn dsh_deeplink_targets_reject_before_startup() {
    let _lock = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    seed_dsh_credentials("old-key");
    let path = home.join(".dsh/.credentials.yaml");
    let credentials = std::fs::read(&path).unwrap();
    let settings = std::fs::read(home.join(".cc-switch/settings.json")).unwrap();
    for alias in ["dsh", "deepseek", "deepseek-harness"] {
        for url in [
            format!("ccswitch://v1/import?resource=provider&app={alias}&name=Demo&apiKey=url-synthetic&endpoint=https%3A%2F%2Fapi.deepseek.com&homepage=https%3A%2F%2Fdeepseek.com"),
            format!("ccswitch://v1/import?resource=prompt&app={alias}&name=Demo&content=dGVzdA=="),
            format!("ccswitch://v1/import?resource=mcp&apps=claude,{alias}&config=e30="),
        ] {
            for args in [
                vec!["deeplink", &url],
                vec!["--app", alias, "deeplink", &url],
            ] {
                let output = run_cli(home, &args);
                assert!(!output.status.success(), "unexpected success: {args:?}");
                let error = String::from_utf8_lossy(&output.stderr);
                assert!(error.contains("Invalid app") || error.contains("cannot be used"), "{error}");
                assert!(!home.join(".cc-switch/cc-switch.db").exists());
                assert_eq!(std::fs::read(&path).unwrap(), credentials);
                assert_eq!(std::fs::read(home.join(".cc-switch/settings.json")).unwrap(), settings);
            }
        }
    }
}

#[test]
fn dsh_explicit_mcp_and_skill_targets_reject_before_startup() {
    let _lock = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    seed_dsh_credentials("old-key");
    let path = home.join(".dsh/.credentials.yaml");
    let credentials = std::fs::read(&path).unwrap();
    let settings = std::fs::read(home.join(".cc-switch/settings.json")).unwrap();
    for alias in ["dsh", "deepseek", "deepseek-harness"] {
        for prefix in [
            vec!["mcp", "enable", "missing"],
            vec!["mcp", "disable", "missing"],
            vec!["mcp", "set-apps", "missing"],
            vec!["skills", "enable", "missing"],
            vec!["skills", "disable", "missing"],
            vec!["skills", "set-apps", "missing"],
            vec!["skills", "import-from-apps", "missing"],
        ] {
            let combined = format!("codex,{alias}");
            for targets in [
                vec!["--apps", alias],
                vec!["--apps", &combined],
                vec!["--apps", "claude", "--apps", alias],
            ] {
                let mut args = prefix.clone();
                args.extend(targets);
                let output = run_cli(home, &args);
                assert!(!output.status.success(), "unexpected success: {args:?}");
                assert!(
                    String::from_utf8_lossy(&output.stderr).contains("does not support dsh"),
                    "{args:?}: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(!home.join(".cc-switch/cc-switch.db").exists());
                assert_eq!(
                    std::fs::read(home.join(".cc-switch/settings.json")).unwrap(),
                    settings
                );
                assert_eq!(std::fs::read(&path).unwrap(), credentials);
            }
        }
    }
}

#[test]
fn dsh_explicit_proxy_takeover_rejects_before_startup_without_global_app() {
    let _lock = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    seed_dsh_credentials("old-key");
    let path = home.join(".dsh/.credentials.yaml");
    let credentials = std::fs::read(&path).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let port = address.port().to_string();
    drop(listener);
    for target in ["dsh", "deepseek", "deepseek-harness"] {
        let output = run_cli(
            home,
            &[
                "proxy",
                "serve",
                "--listen-address",
                "127.0.0.1",
                "--listen-port",
                &port,
                "--takeover",
                "claude",
                "--takeover",
                target,
            ],
        );
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("does not support"));
        assert!(!home.join(".cc-switch/cc-switch.db").exists());
        assert_eq!(std::fs::read(&path).unwrap(), credentials);
        assert!(std::net::TcpStream::connect(address).is_err());
    }
}

#[test]
fn dsh_one_off_model_fetch_rejects_before_network_access() {
    let _lock = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let output = run_dsh_cli(
        home,
        &[
            "provider",
            "fetch-models",
            "--base-url",
            &url,
            "--api-key",
            "test-key",
        ],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not support"));
    assert!(!home.join(".cc-switch/cc-switch.db").exists());
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
