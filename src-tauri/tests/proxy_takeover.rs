use std::{net::TcpListener, sync::Arc};

use cc_switch_lib::{
    get_claude_settings_path, get_codex_auth_path, get_codex_config_path, read_json_file,
    write_codex_live_atomic, AppState, Database, Provider, ProviderMeta, ProxyService,
};
use serde_json::{json, Value};
use serial_test::serial;

#[path = "support.rs"]
mod support;
use support::{ensure_test_home, lock_test_mutex, reset_test_fs};

fn find_free_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind free local port");
    listener
        .local_addr()
        .expect("read local listener address")
        .port()
}

fn seed_claude_live(value: &Value) {
    let settings_path = get_claude_settings_path();
    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent).expect("create claude live dir");
    }
    std::fs::write(
        &settings_path,
        serde_json::to_string_pretty(value).expect("serialize claude live config"),
    )
    .expect("write claude live config");
}

fn claude_token(value: &Value) -> Option<&str> {
    value.get("env").and_then(|env| {
        [
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_API_KEY",
            "OPENROUTER_API_KEY",
            "OPENAI_API_KEY",
        ]
        .into_iter()
        .find_map(|key| env.get(key).and_then(Value::as_str))
    })
}

fn seed_codex_live(auth: &Value, config_text: &str) {
    if let Some(parent) = get_codex_config_path().parent() {
        std::fs::create_dir_all(parent).expect("create codex live dir");
    }
    write_codex_live_atomic(auth, Some(config_text)).expect("write codex live config");
}

#[cfg(unix)]
fn load_runtime_session_pid(state: &AppState) -> u32 {
    let session: Value = serde_json::from_str(
        &state
            .db
            .get_setting("proxy_runtime_session")
            .expect("load runtime session setting")
            .expect("persisted runtime session should exist"),
    )
    .expect("parse runtime session setting");
    session
        .get("pid")
        .and_then(|value| value.as_u64())
        .expect("runtime session pid") as u32
}

#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }

    let rc = unsafe { libc::kill(pid as i32, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(unix)]
fn wait_for_process_exit(pid: u32) {
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_secs(5) {
        if !is_process_alive(pid) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    panic!("process {pid} did not exit in time");
}

#[cfg(unix)]
fn ensure_process_stopped(pid: u32) {
    if !is_process_alive(pid) {
        return;
    }

    let _ = unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    wait_for_process_exit(pid);
}

#[cfg(unix)]
struct ManagedSessionCleanup(Option<u32>);

#[cfg(unix)]
impl ManagedSessionCleanup {
    fn new(pid: u32) -> Self {
        Self(Some(pid))
    }
}

#[cfg(unix)]
impl Drop for ManagedSessionCleanup {
    fn drop(&mut self) {
        if let Some(pid) = self.0.take() {
            ensure_process_stopped(pid);
        }
    }
}

#[tokio::test]
#[serial]
async fn manual_takeover_can_rewrite_and_restore_claude_live_config() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let db = Arc::new(Database::init().expect("create database"));
    let provider = Provider::with_id(
        "claude-provider".to_string(),
        "Claude Provider".to_string(),
        json!({
            "env": {
                "ANTHROPIC_API_KEY": "db-key"
            }
        }),
        Some("claude".to_string()),
    );
    db.save_provider("claude", &provider)
        .expect("save claude provider");
    db.set_current_provider("claude", &provider.id)
        .expect("set current claude provider");

    let original_live = json!({
        "env": {
            "ANTHROPIC_API_KEY": "live-key"
        },
        "workspace": {
            "path": "/tmp/workspace"
        }
    });
    seed_claude_live(&original_live);

    let service = ProxyService::new(db.clone());
    let mut config = service.get_config().await.expect("read proxy config");
    config.listen_port = find_free_port();
    service
        .update_config(&config)
        .await
        .expect("update proxy config");

    service
        .set_takeover_for_app("claude", true)
        .await
        .expect("enable claude takeover");

    let expected_proxy_url = format!("http://127.0.0.1:{}", config.listen_port);
    let taken_over: Value =
        read_json_file(&get_claude_settings_path()).expect("read taken over claude live config");
    assert_eq!(
        taken_over
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
            .and_then(|value| value.as_str()),
        Some(expected_proxy_url.as_str()),
        "takeover should rewrite Claude base URL to the local proxy"
    );
    assert_eq!(
        taken_over
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_API_KEY"))
            .and_then(|value| value.as_str()),
        Some("PROXY_MANAGED"),
        "takeover should replace the live Claude token with a managed placeholder"
    );

    let app_proxy = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("read claude app proxy config after takeover");
    assert!(app_proxy.enabled, "takeover should persist enabled=true");
    assert!(
        db.get_live_backup("claude")
            .await
            .expect("read claude live backup after takeover")
            .is_some(),
        "takeover should persist a live backup for restore"
    );

    service
        .set_takeover_for_app("claude", false)
        .await
        .expect("disable claude takeover");

    let restored: Value =
        read_json_file(&get_claude_settings_path()).expect("read restored claude live config");
    assert_eq!(
        restored, original_live,
        "restore should bring back the exact pre-takeover Claude live config"
    );
    assert!(
        !service.is_running().await,
        "service should stop when the last active takeover is restored"
    );
    assert!(
        !db.get_proxy_config_for_app("claude")
            .await
            .expect("read claude app proxy config after restore")
            .enabled,
        "restore should clear enabled=true"
    );
    assert!(
        db.get_live_backup("claude")
            .await
            .expect("read claude live backup after restore")
            .is_none(),
        "restore should delete the live backup once recovery succeeds"
    );
}

#[tokio::test]
#[serial]
async fn reloading_app_state_does_not_recover_an_active_takeover_session() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let state = AppState::try_new().expect("create app state");
    seed_claude_live(&json!({
        "env": {
            "ANTHROPIC_API_KEY": "original-key"
        }
    }));

    state
        .proxy_service
        .set_takeover_for_app("claude", true)
        .await
        .expect("enable claude takeover");
    assert!(
        state.proxy_service.is_running().await,
        "takeover should start the foreground proxy runtime"
    );

    let expected_proxy_url = format!(
        "http://127.0.0.1:{}",
        state.proxy_service.get_status().await.port
    );

    let reloaded = AppState::try_new().expect("reload app state during active takeover");

    let live_after_reload: Value =
        read_json_file(&get_claude_settings_path()).expect("read claude live after reload");
    assert_eq!(
        live_after_reload
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
            .and_then(|value| value.as_str()),
        Some(expected_proxy_url.as_str()),
        "reloading app state during an active foreground proxy session must not trigger startup recovery"
    );
    assert!(
        reloaded
            .db
            .get_proxy_config_for_app("claude")
            .await
            .expect("read claude proxy config after reload")
            .enabled,
        "reloading app state during an active foreground proxy session must keep takeover intent enabled"
    );

    reloaded
        .proxy_service
        .set_takeover_for_app("claude", false)
        .await
        .expect("disable claude takeover after reload");
}

#[tokio::test]
#[serial]
async fn app_state_try_new_recovers_stale_takeover_from_backup() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let original_live = json!({
        "env": {
            "ANTHROPIC_API_KEY": "live-key"
        },
        "workspace": {
            "path": "/tmp/workspace"
        }
    });

    let seeded = AppState::try_new().expect("create seeded app state");
    seeded
        .db
        .save_live_backup(
            "claude",
            &serde_json::to_string(&original_live).expect("serialize claude backup"),
        )
        .await
        .expect("save claude live backup");

    let mut app_proxy = seeded
        .db
        .get_proxy_config_for_app("claude")
        .await
        .expect("read claude proxy config");
    app_proxy.enabled = true;
    seeded
        .db
        .update_proxy_config_for_app(app_proxy)
        .await
        .expect("mark claude takeover enabled");

    seed_claude_live(&json!({
        "env": {
            "ANTHROPIC_BASE_URL": "http://127.0.0.1:15721",
            "ANTHROPIC_API_KEY": "PROXY_MANAGED"
        }
    }));

    drop(seeded);

    let recovered =
        AppState::try_new_with_startup_recovery().expect("recreate app state after crash");

    let restored: Value =
        read_json_file(&get_claude_settings_path()).expect("read recovered claude live config");
    assert_eq!(
        restored, original_live,
        "startup recovery should restore the saved Claude live backup"
    );
    assert!(
        recovered
            .db
            .get_live_backup("claude")
            .await
            .expect("read claude live backup after recovery")
            .is_none(),
        "startup recovery should clear stale backups"
    );
    assert!(
        !recovered
            .db
            .get_proxy_config_for_app("claude")
            .await
            .expect("read claude proxy config after recovery")
            .enabled,
        "startup recovery should clear stale enabled=true takeover state"
    );
}

#[tokio::test]
#[serial]
async fn disabling_claude_takeover_without_backup_restores_synced_live_token() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let db = Arc::new(Database::init().expect("create database"));
    let provider = Provider::with_id(
        "claude-provider".to_string(),
        "Claude Provider".to_string(),
        json!({
            "env": {
                "ANTHROPIC_API_KEY": "stale-provider-token"
            }
        }),
        Some("claude".to_string()),
    );
    db.save_provider("claude", &provider)
        .expect("save claude provider");
    db.set_current_provider("claude", &provider.id)
        .expect("set current claude provider");

    seed_claude_live(&json!({
        "env": {
            "ANTHROPIC_AUTH_TOKEN": "fresh-live-token"
        }
    }));

    let service = ProxyService::new(db.clone());
    let mut config = service.get_config().await.expect("read proxy config");
    config.listen_port = find_free_port();
    service
        .update_config(&config)
        .await
        .expect("update proxy config");

    service
        .set_takeover_for_app("claude", true)
        .await
        .expect("enable claude takeover");

    db.delete_live_backup("claude")
        .await
        .expect("delete claude backup to force provider restore fallback");

    service
        .set_takeover_for_app("claude", false)
        .await
        .expect("disable claude takeover");

    let restored: Value =
        read_json_file(&get_claude_settings_path()).expect("read restored claude live config");
    assert_eq!(
        claude_token(&restored),
        Some("fresh-live-token"),
        "fallback restore should use the live Claude token captured when takeover started, not the stale provider token"
    );
    assert_ne!(
        claude_token(&restored),
        Some("stale-provider-token"),
        "fallback restore must not regress to the stale provider credential when the backup is missing"
    );
}

#[tokio::test]
#[serial]
async fn startup_recovery_guard_ignores_stale_external_session_marker() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let original_live = json!({
        "env": {
            "ANTHROPIC_API_KEY": "live-key"
        },
        "workspace": {
            "path": "/tmp/workspace"
        }
    });

    let seeded = AppState::try_new().expect("create seeded app state");
    seeded
        .db
        .save_live_backup(
            "claude",
            &serde_json::to_string(&original_live).expect("serialize claude backup"),
        )
        .await
        .expect("save claude live backup");

    let mut app_proxy = seeded
        .db
        .get_proxy_config_for_app("claude")
        .await
        .expect("read claude proxy config");
    app_proxy.enabled = true;
    seeded
        .db
        .update_proxy_config_for_app(app_proxy)
        .await
        .expect("mark claude takeover enabled");

    seeded
        .db
        .set_setting(
            "proxy_runtime_session",
            &json!({
                "pid": std::process::id(),
                "address": "127.0.0.1",
                "port": find_free_port(),
                "started_at": "2026-03-10T00:00:00Z",
                "kind": "managed_external"
            })
            .to_string(),
        )
        .expect("persist stale external runtime session marker");

    seed_claude_live(&json!({
        "env": {
            "ANTHROPIC_BASE_URL": "http://127.0.0.1:15721",
            "ANTHROPIC_API_KEY": "PROXY_MANAGED"
        }
    }));

    assert!(
        !seeded.proxy_service.is_running().await,
        "stale external session markers must not make startup recovery think the managed proxy is still running"
    );

    seeded
        .proxy_service
        .recover_takeovers_on_startup()
        .await
        .expect("recover stale takeover after stale external session marker");

    let restored: Value =
        read_json_file(&get_claude_settings_path()).expect("read recovered claude live config");
    assert_eq!(
        restored, original_live,
        "startup recovery should ignore stale external session markers and restore the saved Claude live backup"
    );
    assert!(
        seeded
            .db
            .get_setting("proxy_runtime_session")
            .expect("read runtime session marker after recovery")
            .is_none(),
        "startup recovery should clear stale external runtime markers once validation fails"
    );
}

#[tokio::test]
#[serial]
async fn app_state_try_new_clears_stale_takeover_when_backup_and_provider_are_missing() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let seeded = AppState::try_new().expect("create seeded app state");
    let mut app_proxy = seeded
        .db
        .get_proxy_config_for_app("claude")
        .await
        .expect("read claude proxy config");
    app_proxy.enabled = true;
    seeded
        .db
        .update_proxy_config_for_app(app_proxy)
        .await
        .expect("mark claude takeover enabled");

    seed_claude_live(&json!({
        "env": {
            "ANTHROPIC_BASE_URL": "http://127.0.0.1:15721",
            "ANTHROPIC_API_KEY": "PROXY_MANAGED",
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1"
        },
        "workspace": {
            "path": "/tmp/workspace"
        }
    }));

    drop(seeded);

    let recovered =
        AppState::try_new_with_startup_recovery().expect("recreate app state after stale takeover");

    let cleaned: Value =
        read_json_file(&get_claude_settings_path()).expect("read cleaned claude live config");
    assert_eq!(
        cleaned
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_BASE_URL")),
        None,
        "startup recovery should remove the stale Claude proxy base URL when no restore source exists"
    );
    assert_eq!(
        cleaned
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_API_KEY")),
        None,
        "startup recovery should remove proxy-managed placeholder tokens when no restore source exists"
    );
    assert_eq!(
        cleaned
            .get("env")
            .and_then(|env| env.get("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"))
            .and_then(|value| value.as_str()),
        Some("1"),
        "startup recovery should preserve unrelated Claude live settings"
    );
    assert_eq!(
        cleaned
            .get("workspace")
            .and_then(|workspace| workspace.get("path"))
            .and_then(|value| value.as_str()),
        Some("/tmp/workspace"),
        "startup recovery should preserve non-proxy Claude live content"
    );
    assert!(
        !recovered
            .db
            .get_proxy_config_for_app("claude")
            .await
            .expect("read claude proxy config after stale cleanup")
            .enabled,
        "startup recovery should clear stale enabled=true even when no restore source exists"
    );
}

#[tokio::test]
#[serial]
async fn app_state_try_new_restores_claude_from_current_provider_with_common_snippet() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let seeded = AppState::try_new().expect("create seeded app state");
    let mut provider = Provider::with_id(
        "claude-provider".to_string(),
        "Claude Provider".to_string(),
        json!({
            "env": {
                "ANTHROPIC_API_KEY": "fresh-key"
            }
        }),
        Some("claude".to_string()),
    );
    provider.meta = Some(ProviderMeta {
        apply_common_config: Some(true),
        ..Default::default()
    });
    seeded
        .db
        .save_provider("claude", &provider)
        .expect("save claude provider");
    seeded
        .db
        .set_current_provider("claude", &provider.id)
        .expect("set current claude provider");
    seeded
        .db
        .set_config_snippet(
            "claude",
            Some(
                json!({
                    "env": {
                        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1"
                    }
                })
                .to_string(),
            ),
        )
        .expect("set claude common snippet");

    let mut app_proxy = seeded
        .db
        .get_proxy_config_for_app("claude")
        .await
        .expect("read claude proxy config");
    app_proxy.enabled = true;
    seeded
        .db
        .update_proxy_config_for_app(app_proxy)
        .await
        .expect("mark claude takeover enabled");

    seed_claude_live(&json!({
        "env": {
            "ANTHROPIC_BASE_URL": "http://127.0.0.1:15721",
            "ANTHROPIC_API_KEY": "PROXY_MANAGED"
        }
    }));

    drop(seeded);

    let recovered = AppState::try_new_with_startup_recovery()
        .expect("recreate app state after stale claude takeover");

    let restored: Value =
        read_json_file(&get_claude_settings_path()).expect("read restored claude live config");
    assert_eq!(
        restored
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_API_KEY"))
            .and_then(|value| value.as_str()),
        Some("fresh-key"),
        "startup recovery should restore Claude from the current provider when backup is missing"
    );
    assert_eq!(
        restored
            .get("env")
            .and_then(|env| env.get("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"))
            .and_then(|value| value.as_str()),
        Some("1"),
        "startup recovery should preserve Claude common snippet semantics on current-provider restore"
    );
    assert_eq!(
        restored
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_BASE_URL")),
        None,
        "startup recovery should clear the stale Claude proxy base URL after current-provider restore"
    );
    assert!(
        !recovered
            .db
            .get_proxy_config_for_app("claude")
            .await
            .expect("read claude proxy config after current-provider recovery")
            .enabled,
        "startup recovery should clear stale enabled=true after Claude current-provider restore"
    );
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn disabling_managed_proxy_session_restores_current_app_takeover_state() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let original_live = json!({
        "env": {
            "ANTHROPIC_API_KEY": "live-key"
        },
        "workspace": {
            "path": "/tmp/workspace"
        }
    });
    seed_claude_live(&original_live);

    let state = AppState::try_new().expect("create app state");
    let mut config = state
        .proxy_service
        .get_config()
        .await
        .expect("read proxy config");
    config.listen_port = find_free_port();
    state
        .proxy_service
        .update_config(&config)
        .await
        .expect("persist proxy config");

    state
        .proxy_service
        .start_managed_session("claude")
        .await
        .expect("start managed proxy session");

    let taken_over: Value =
        read_json_file(&get_claude_settings_path()).expect("read taken over claude live config");
    assert_eq!(
        taken_over
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_API_KEY"))
            .and_then(|value| value.as_str()),
        Some("PROXY_MANAGED"),
        "managed proxy start should rewrite Claude live config into takeover mode"
    );

    state
        .proxy_service
        .set_managed_session_for_app("claude", false)
        .await
        .expect("disable managed proxy session");

    let restored: Value =
        read_json_file(&get_claude_settings_path()).expect("read restored claude live config");
    assert_eq!(
        restored, original_live,
        "stopping the managed proxy should restore the original Claude live config"
    );
    assert!(
        !state
            .db
            .get_proxy_config_for_app("claude")
            .await
            .expect("read claude proxy config after managed stop")
            .enabled,
        "stopping the managed proxy should clear claude takeover intent"
    );
    assert!(
        state
            .db
            .get_live_backup("claude")
            .await
            .expect("read claude live backup after managed stop")
            .is_none(),
        "stopping the managed proxy should clear the saved live backup"
    );
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn startup_recovery_skips_owned_managed_session_when_probe_is_unreachable() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let original_live = json!({
        "env": {
            "ANTHROPIC_API_KEY": "live-key"
        },
        "workspace": {
            "path": "/tmp/workspace"
        }
    });
    seed_claude_live(&original_live);

    let state = AppState::try_new().expect("create app state");
    let mut config = state
        .proxy_service
        .get_config()
        .await
        .expect("read proxy config");
    config.listen_port = find_free_port();
    state
        .proxy_service
        .update_config(&config)
        .await
        .expect("persist proxy config");

    state
        .proxy_service
        .start_managed_session("claude")
        .await
        .expect("start managed proxy session");
    let _cleanup = ManagedSessionCleanup::new(load_runtime_session_pid(&state));

    let managed_proxy_url = format!("http://127.0.0.1:{}", config.listen_port);
    let taken_over: Value =
        read_json_file(&get_claude_settings_path()).expect("read taken over claude live config");
    assert_eq!(
        taken_over
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
            .and_then(|value| value.as_str()),
        Some(managed_proxy_url.as_str()),
        "managed startup should route Claude through the proxy URL"
    );
    assert_eq!(
        taken_over
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_API_KEY"))
            .and_then(|value| value.as_str()),
        Some("PROXY_MANAGED"),
        "managed startup should place Claude in proxy takeover mode"
    );

    let runtime_session_raw = state
        .db
        .get_setting("proxy_runtime_session")
        .expect("read runtime session marker")
        .expect("runtime session marker should exist");
    let mut runtime_session: Value =
        serde_json::from_str(&runtime_session_raw).expect("parse runtime session marker");
    let original_port = runtime_session
        .get("port")
        .and_then(Value::as_u64)
        .expect("runtime session marker port") as u16;
    let mut unreachable_port = find_free_port();
    while unreachable_port == original_port {
        unreachable_port = find_free_port();
    }
    runtime_session["port"] = json!(unreachable_port);
    state
        .db
        .set_setting("proxy_runtime_session", &runtime_session.to_string())
        .expect("tamper runtime session marker port");

    drop(state);

    let recovered =
        AppState::try_new_with_startup_recovery().expect("recreate app state after restart");

    let live_after_restart: Value =
        read_json_file(&get_claude_settings_path()).expect("read claude live config after restart");
    assert_eq!(
        live_after_restart
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
            .and_then(|value| value.as_str()),
        Some(managed_proxy_url.as_str()),
        "startup recovery must be skipped while owned managed runtime is still alive"
    );
    assert_eq!(
        live_after_restart
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_API_KEY"))
            .and_then(|value| value.as_str()),
        Some("PROXY_MANAGED"),
        "startup recovery must not restore live config while managed runtime still exists"
    );
    assert!(
        recovered
            .db
            .get_proxy_config_for_app("claude")
            .await
            .expect("read claude proxy config after restart")
            .enabled,
        "startup recovery must not clear enabled takeover state for active managed runtime"
    );
    assert!(
        recovered
            .db
            .get_live_backup("claude")
            .await
            .expect("read claude live backup after restart")
            .is_some(),
        "startup recovery must not clear Claude backup while managed runtime still exists"
    );
    let runtime_after_restart = recovered
        .db
        .get_setting("proxy_runtime_session")
        .expect("read runtime session marker after restart");
    assert!(
        runtime_after_restart.is_some(),
        "startup recovery must not clear runtime marker for owned managed runtime"
    );
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn disabling_one_managed_app_restores_only_that_app_while_shared_runtime_keeps_other_takeover(
) {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let original_claude_live = json!({
        "env": {
            "ANTHROPIC_API_KEY": "claude-live-key"
        },
        "workspace": {
            "path": "/tmp/claude-workspace"
        }
    });
    let original_codex_auth = json!({
        "OPENAI_API_KEY": "codex-live-key"
    });
    let original_codex_config =
        "model_provider = \"openai\"\nbase_url = \"https://api.openai.com/v1\"\n";
    seed_claude_live(&original_claude_live);
    seed_codex_live(&original_codex_auth, original_codex_config);

    let state = AppState::try_new().expect("create app state");
    let mut config = state
        .proxy_service
        .get_config()
        .await
        .expect("read proxy config");
    config.listen_port = find_free_port();
    state
        .proxy_service
        .update_config(&config)
        .await
        .expect("persist proxy config");

    state
        .proxy_service
        .set_managed_session_for_app("claude", true)
        .await
        .expect("start managed proxy for claude");
    state
        .proxy_service
        .set_managed_session_for_app("codex", true)
        .await
        .expect("reuse managed proxy for codex");
    let _cleanup = ManagedSessionCleanup::new(load_runtime_session_pid(&state));

    state
        .proxy_service
        .set_managed_session_for_app("claude", false)
        .await
        .expect("disable managed proxy for claude only");

    let restored_claude: Value =
        read_json_file(&get_claude_settings_path()).expect("read restored claude config");
    assert_eq!(
        restored_claude, original_claude_live,
        "disabling Claude should restore only Claude's original live config"
    );
    let codex_auth: Value =
        read_json_file(&get_codex_auth_path()).expect("read active codex auth after claude stop");
    assert_eq!(
        codex_auth
            .get("OPENAI_API_KEY")
            .and_then(|value| value.as_str()),
        Some("PROXY_MANAGED"),
        "Codex should stay in takeover mode while its managed session remains attached"
    );
    let codex_config =
        std::fs::read_to_string(get_codex_config_path()).expect("read active codex config");
    assert!(
        codex_config.contains("127.0.0.1") && codex_config.contains("/v1"),
        "Codex should keep routing through the shared proxy while still attached"
    );
    assert!(
        state.proxy_service.is_running().await,
        "shared managed runtime should stay running while Codex is still attached"
    );

    state
        .proxy_service
        .set_managed_session_for_app("codex", false)
        .await
        .expect("disable managed proxy for codex");

    let restored_codex_auth: Value =
        read_json_file(&get_codex_auth_path()).expect("read restored codex auth");
    assert_eq!(
        restored_codex_auth, original_codex_auth,
        "disabling the last attached app should restore the original Codex auth config"
    );
    let restored_codex_config =
        std::fs::read_to_string(get_codex_config_path()).expect("read restored codex config");
    assert_eq!(
        restored_codex_config, original_codex_config,
        "disabling the last attached app should restore the original Codex config"
    );
}

#[cfg(unix)]
#[test]
#[serial]
fn startup_recovery_handles_active_managed_proxy_without_existing_tokio_runtime() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fake status listener");
    listener
        .set_nonblocking(true)
        .expect("set fake listener nonblocking");
    let port = listener
        .local_addr()
        .expect("read fake listener address")
        .port();
    let expected_status = json!({
        "running": true,
        "address": "127.0.0.1",
        "port": port,
        "uptime_seconds": 5,
        "managed_session_token": "expected-session-token",
        "active_targets": []
    })
    .to_string();
    let server = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match listener.accept() {
                Ok((mut socket, _)) => {
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        expected_status.len(),
                        expected_status
                    );
                    use std::io::Write;
                    socket
                        .write_all(response.as_bytes())
                        .expect("write fake status response");
                    return;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() >= deadline {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(err) => panic!("accept status request: {err}"),
            }
        }
    });

    let state = AppState::try_new().expect("create app state");
    state
        .db
        .set_setting(
            "proxy_runtime_session",
            &json!({
                "pid": std::process::id(),
                "address": "127.0.0.1",
                "port": port,
                "started_at": "2026-03-10T00:00:00Z",
                "kind": "managed_external",
                "session_token": "expected-session-token"
            })
            .to_string(),
        )
        .expect("persist proxy runtime session marker");

    let recovered = std::panic::catch_unwind(AppState::try_new_with_startup_recovery);
    server.join().expect("join fake status server");

    assert!(
        recovered.is_ok(),
        "startup recovery should not panic when an active managed proxy exists"
    );
    recovered
        .expect("unwrap startup recovery result")
        .expect("startup recovery should succeed");
}

#[tokio::test]
#[serial]
async fn app_state_try_new_restores_codex_from_current_provider_and_auth_json() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let seeded = AppState::try_new().expect("create seeded app state");
    let mut provider = Provider::with_id(
        "openai-official".to_string(),
        "OpenAI Official".to_string(),
        json!({
            "auth": { "OPENAI_API_KEY": "sk-restored" },
            "config": "model_provider = \"openai-official\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.openai-official]\nbase_url = \"https://api.openai.com/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
        }),
        None,
    );
    provider.meta = Some(ProviderMeta {
        apply_common_config: Some(true),
        ..Default::default()
    });
    seeded
        .db
        .save_provider("codex", &provider)
        .expect("save codex provider");
    seeded
        .db
        .set_current_provider("codex", &provider.id)
        .expect("set current codex provider");
    seeded
        .db
        .set_config_snippet("codex", Some("disable_response_storage = true".to_string()))
        .expect("set codex common snippet");

    let mut app_proxy = seeded
        .db
        .get_proxy_config_for_app("codex")
        .await
        .expect("read codex proxy config");
    app_proxy.enabled = true;
    seeded
        .db
        .update_proxy_config_for_app(app_proxy)
        .await
        .expect("mark codex takeover enabled");

    seed_codex_live(
        &json!({
            "OPENAI_API_KEY": "PROXY_MANAGED"
        }),
        "model_provider = \"openai-official\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.openai-official]\nbase_url = \"http://127.0.0.1:15721/v1\"\n",
    );
    assert!(
        get_codex_auth_path().exists(),
        "stale Codex takeover should seed auth.json before recovery"
    );

    drop(seeded);

    let recovered = AppState::try_new_with_startup_recovery()
        .expect("recreate app state after stale codex takeover");

    assert!(
        get_codex_auth_path().exists(),
        "startup recovery should restore auth.json from the current provider snapshot"
    );
    let restored_auth: serde_json::Value =
        read_json_file(&get_codex_auth_path()).expect("read restored auth.json");
    assert_eq!(
        restored_auth.get("OPENAI_API_KEY").and_then(|v| v.as_str()),
        Some("sk-restored")
    );
    let restored_config =
        std::fs::read_to_string(get_codex_config_path()).expect("read restored codex config.toml");
    assert!(
        restored_config.contains("https://api.openai.com/v1"),
        "startup recovery should restore the current Codex provider config"
    );
    assert!(
        restored_config.contains("disable_response_storage = true"),
        "startup recovery should reapply the Codex common snippet when restoring from the current provider"
    );
    assert!(
        !restored_config.contains("127.0.0.1"),
        "startup recovery should clear stale Codex localhost proxy routing"
    );
    assert!(
        !recovered
            .db
            .get_proxy_config_for_app("codex")
            .await
            .expect("read codex proxy config after current-provider recovery")
            .enabled,
        "startup recovery should clear stale enabled=true after Codex current-provider restore"
    );
}
