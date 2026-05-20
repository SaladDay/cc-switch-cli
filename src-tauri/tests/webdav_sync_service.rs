use std::{
    collections::{HashMap, HashSet},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
};

use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
    Router,
};
use cc_switch_lib::{
    get_app_config_dir, set_webdav_sync_settings, AppState as CcAppState, Provider,
    WebDavSyncService, WebDavSyncSettings, WebDavSyncStatus,
};
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;

#[path = "support.rs"]
mod support;
use support::{ensure_test_home, lock_test_mutex, reset_test_fs};

const DAV_ROOT: &str = "/dav";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ServerConfig {
    manifest_head_behavior: ManifestHeadBehavior,
    reject_dotfile_puts: bool,
}

impl ServerConfig {
    fn with_manifest_head(manifest_head_behavior: ManifestHeadBehavior) -> Self {
        Self {
            manifest_head_behavior,
            reject_dotfile_puts: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ManifestHeadBehavior {
    Present,
    Missing,
    ServerError,
}

#[derive(Debug, Default)]
struct ServerState {
    directories: HashSet<String>,
    files: HashMap<String, Vec<u8>>,
    put_paths: Vec<String>,
    get_paths: Vec<String>,
    head_paths: Vec<String>,
    delete_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServerSnapshot {
    put_paths: Vec<String>,
    get_paths: Vec<String>,
    head_paths: Vec<String>,
    delete_paths: Vec<String>,
}

#[derive(Clone)]
struct AppState {
    config: ServerConfig,
    inner: Arc<Mutex<ServerState>>,
}

struct TestWebDavServer {
    base_url: String,
    state: Arc<Mutex<ServerState>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: Option<thread::JoinHandle<()>>,
}

impl TestWebDavServer {
    fn start() -> Self {
        Self::start_with_config(ServerConfig::with_manifest_head(
            ManifestHeadBehavior::Present,
        ))
    }

    fn start_with_config(config: ServerConfig) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind test WebDAV listener");
        let port = listener
            .local_addr()
            .expect("read test WebDAV listener address")
            .port();
        listener
            .set_nonblocking(true)
            .expect("set test WebDAV listener nonblocking");

        let state = Arc::new(Mutex::new(ServerState {
            directories: HashSet::from([DAV_ROOT.to_string()]),
            ..ServerState::default()
        }));
        let app_state = AppState {
            config,
            inner: Arc::clone(&state),
        };
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let join_handle = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build test WebDAV runtime");

            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener)
                    .expect("convert test WebDAV listener");
                let app = Router::new()
                    .route(DAV_ROOT, any(handle_webdav_request))
                    .route("/dav/*path", any(handle_webdav_request))
                    .with_state(app_state);

                axum::serve(listener, app)
                    .with_graceful_shutdown(async move {
                        let _ = shutdown_rx.await;
                    })
                    .await
                    .expect("run test WebDAV server");
            });
        });

        Self {
            base_url: format!("http://127.0.0.1:{port}{DAV_ROOT}"),
            state,
            shutdown_tx: Some(shutdown_tx),
            join_handle: Some(join_handle),
        }
    }

    fn snapshot(&self) -> ServerSnapshot {
        let state = self.state.lock().expect("lock test WebDAV state");
        ServerSnapshot {
            put_paths: state.put_paths.clone(),
            get_paths: state.get_paths.clone(),
            head_paths: state.head_paths.clone(),
            delete_paths: state.delete_paths.clone(),
        }
    }

    fn seed_file(&self, path: &str, bytes: Vec<u8>) {
        self.state
            .lock()
            .expect("lock test WebDAV state for file seed")
            .files
            .insert(path.to_string(), bytes);
    }

    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        self.state
            .lock()
            .expect("lock test WebDAV state for file read")
            .files
            .get(path)
            .cloned()
    }
}

impl Drop for TestWebDavServer {
    fn drop(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        if let Some(join_handle) = self.join_handle.take() {
            join_handle.join().expect("join test WebDAV server thread");
        }
    }
}

async fn handle_webdav_request(State(state): State<AppState>, request: Request<Body>) -> Response {
    let method = request.method().as_str().to_string();
    let path = request.uri().path().to_string();

    match method.as_str() {
        "PROPFIND" => {
            let exists = state
                .inner
                .lock()
                .expect("lock PROPFIND state")
                .directories
                .contains(&path);
            if exists {
                multi_status_response()
            } else {
                StatusCode::NOT_FOUND.into_response()
            }
        }
        "MKCOL" => {
            state
                .inner
                .lock()
                .expect("lock MKCOL state")
                .directories
                .insert(path);
            StatusCode::CREATED.into_response()
        }
        "PUT" => {
            if state.config.reject_dotfile_puts
                && path
                    .rsplit('/')
                    .next()
                    .is_some_and(|name| name.starts_with('.'))
            {
                return StatusCode::FORBIDDEN.into_response();
            }
            let parent_exists = path.rsplit_once('/').is_some_and(|(parent, _)| {
                state
                    .inner
                    .lock()
                    .expect("lock PUT parent state")
                    .directories
                    .contains(parent)
            });
            if !parent_exists {
                return StatusCode::CONFLICT.into_response();
            }
            let bytes = to_bytes(request.into_body(), usize::MAX)
                .await
                .expect("read PUT body")
                .to_vec();
            let mut inner = state.inner.lock().expect("lock PUT state");
            inner.put_paths.push(path.clone());
            inner.files.insert(path, bytes);
            StatusCode::CREATED.into_response()
        }
        "GET" => {
            let mut inner = state.inner.lock().expect("lock GET state");
            inner.get_paths.push(path.clone());
            match inner.files.get(&path).cloned() {
                Some(bytes) => (StatusCode::OK, bytes).into_response(),
                None => StatusCode::NOT_FOUND.into_response(),
            }
        }
        "HEAD" => {
            let mut inner = state.inner.lock().expect("lock HEAD state");
            inner.head_paths.push(path.clone());
            if is_manifest_path(&path) {
                match state.config.manifest_head_behavior {
                    ManifestHeadBehavior::Present => StatusCode::OK.into_response(),
                    ManifestHeadBehavior::Missing => StatusCode::NOT_FOUND.into_response(),
                    ManifestHeadBehavior::ServerError => {
                        StatusCode::INTERNAL_SERVER_ERROR.into_response()
                    }
                }
            } else if inner.files.contains_key(&path) {
                StatusCode::OK.into_response()
            } else {
                StatusCode::NOT_FOUND.into_response()
            }
        }
        "DELETE" => {
            let mut inner = state.inner.lock().expect("lock DELETE state");
            inner.delete_paths.push(path.clone());
            inner.files.remove(&path);
            StatusCode::NO_CONTENT.into_response()
        }
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}

fn multi_status_response() -> Response {
    StatusCode::from_u16(207)
        .expect("build 207 Multi-Status")
        .into_response()
}

fn is_manifest_path(path: &str) -> bool {
    path.ends_with("/manifest.json")
}

fn sample_settings(base_url: &str) -> WebDavSyncSettings {
    WebDavSyncSettings {
        enabled: true,
        base_url: base_url.to_string(),
        remote_root: "sync-root".to_string(),
        profile: "default-profile".to_string(),
        username: "demo".to_string(),
        password: "secret".to_string(),
        auto_sync: false,
        status: WebDavSyncStatus::default(),
    }
}

fn find_free_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind free local port");
    listener.local_addr().expect("read free local port").port()
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn empty_zip_bytes() -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let writer = zip::ZipWriter::new(&mut cursor);
        writer.finish().expect("finish empty zip");
    }
    cursor.into_inner()
}

fn seed_claude_remote_provider(state: &CcAppState) {
    let provider = Provider::with_id(
        "claude-provider".to_string(),
        "Claude Provider".to_string(),
        serde_json::json!({
            "env": {
                "ANTHROPIC_API_KEY": "db-key"
            }
        }),
        Some("claude".to_string()),
    );
    state
        .db
        .save_provider("claude", &provider)
        .expect("save claude provider");
    state
        .db
        .set_current_provider("claude", &provider.id)
        .expect("set current claude provider");
}

fn seed_claude_live() {
    let path = cc_switch_lib::get_claude_settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create claude live dir");
    }
    std::fs::write(
        path,
        serde_json::to_string_pretty(&serde_json::json!({
            "env": {
                "ANTHROPIC_API_KEY": "live-key"
            }
        }))
        .expect("serialize claude live"),
    )
    .expect("write claude live");
}

fn assert_upload_artifact_puts(snapshot: &ServerSnapshot) {
    assert_eq!(
        snapshot.put_paths,
        vec![
            "/dav/sync-root/v2/db-v6/default-profile/db.sql".to_string(),
            "/dav/sync-root/v2/db-v6/default-profile/skills.zip".to_string(),
            "/dav/sync-root/v2/db-v6/default-profile/settings.json".to_string(),
            "/dav/sync-root/v2/db-v6/default-profile/manifest.json".to_string()
        ],
        "unexpected upload PUT sequence: {snapshot:?}"
    );
}

#[test]
fn check_connection_succeeds_without_round_trip_probe() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let server = TestWebDavServer::start_with_config(ServerConfig {
        manifest_head_behavior: ManifestHeadBehavior::Present,
        reject_dotfile_puts: true,
    });
    set_webdav_sync_settings(Some(sample_settings(&server.base_url)))
        .expect("save test WebDAV settings");

    WebDavSyncService::check_connection().expect("connection check should succeed");

    let snapshot = server.snapshot();
    assert_eq!(
        snapshot.put_paths,
        Vec::<String>::new(),
        "connection check should not write probe files: {snapshot:?}"
    );
    assert_eq!(
        snapshot.get_paths,
        Vec::<String>::new(),
        "connection check should not read back probe files: {snapshot:?}"
    );
    assert_eq!(
        snapshot.delete_paths,
        Vec::<String>::new(),
        "connection check should not clean up probe files: {snapshot:?}"
    );
}

#[test]
fn upload_succeeds_without_manifest_readback() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let server = TestWebDavServer::start_with_config(ServerConfig::with_manifest_head(
        ManifestHeadBehavior::Missing,
    ));
    set_webdav_sync_settings(Some(sample_settings(&server.base_url)))
        .expect("save test WebDAV settings");

    let summary = WebDavSyncService::upload().expect("manifest PUT should decide upload success");

    assert_eq!(summary.decision, cc_switch_lib::SyncDecision::Upload);
    let snapshot = server.snapshot();
    assert_upload_artifact_puts(&snapshot);
    assert_eq!(
        snapshot.get_paths,
        Vec::<String>::new(),
        "upload should not verify manifest bytes via GET"
    );
    assert_eq!(
        snapshot.head_paths,
        vec!["/dav/sync-root/v2/db-v6/default-profile/manifest.json".to_string()],
        "HEAD should remain best-effort metadata only"
    );
    assert_eq!(
        snapshot.delete_paths,
        Vec::<String>::new(),
        "plain upload should not clean up legacy V1 remote data"
    );
}

#[test]
fn upload_succeeds_when_manifest_head_returns_server_error() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let server = TestWebDavServer::start_with_config(ServerConfig::with_manifest_head(
        ManifestHeadBehavior::ServerError,
    ));
    set_webdav_sync_settings(Some(sample_settings(&server.base_url)))
        .expect("save test WebDAV settings");

    let summary =
        WebDavSyncService::upload().expect("manifest HEAD errors should stay best-effort");

    assert_eq!(summary.decision, cc_switch_lib::SyncDecision::Upload);
    let snapshot = server.snapshot();
    assert_upload_artifact_puts(&snapshot);
    assert_eq!(
        snapshot.get_paths,
        Vec::<String>::new(),
        "upload success should not be gated by manifest GET readback"
    );
    assert_eq!(
        snapshot.head_paths,
        vec!["/dav/sync-root/v2/db-v6/default-profile/manifest.json".to_string()],
        "HEAD should still be attempted as best-effort metadata"
    );
    assert_eq!(
        snapshot.delete_paths,
        Vec::<String>::new(),
        "plain upload should not clean up legacy V1 remote data"
    );
}

#[test]
fn server_rejects_put_when_parent_directory_is_missing() {
    let server = TestWebDavServer::start();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build reqwest runtime");

    let status = runtime.block_on(async {
        reqwest::Client::new()
            .put(format!(
                "{}/sync-root/v2/db-v6/default-profile/db.sql",
                server.base_url
            ))
            .body("db")
            .send()
            .await
            .expect("PUT missing parent")
            .status()
    });

    assert_eq!(status, StatusCode::CONFLICT);
}

#[test]
fn webdav_download_rejects_when_proxy_running() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let server = TestWebDavServer::start_with_config(ServerConfig::with_manifest_head(
        ManifestHeadBehavior::Missing,
    ));
    set_webdav_sync_settings(Some(sample_settings(&server.base_url)))
        .expect("save test WebDAV settings");

    let state = CcAppState::try_new().expect("create app state");
    seed_claude_remote_provider(&state);
    WebDavSyncService::upload().expect("seed remote v2 snapshot");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build test runtime");
    runtime.block_on(async {
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
            .expect("update proxy config");
        state
            .proxy_service
            .start()
            .await
            .expect("start proxy runtime");
    });

    let err = WebDavSyncService::download()
        .expect_err("download should reject while proxy runtime is active");

    assert!(
        err.to_string().contains("代理正在运行") || err.to_string().contains("proxy is running"),
        "unexpected error: {err}"
    );

    runtime.block_on(async {
        state
            .proxy_service
            .stop()
            .await
            .expect("stop proxy runtime");
    });
}

#[test]
fn webdav_migrate_v1_to_v2_rejects_when_takeover_is_active() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let server = TestWebDavServer::start();
    set_webdav_sync_settings(Some(sample_settings(&server.base_url)))
        .expect("save test WebDAV settings");

    let state = CcAppState::try_new().expect("create app state");
    seed_claude_remote_provider(&state);
    seed_claude_live();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build test runtime");

    let db_sql = state
        .db
        .export_sql_string()
        .expect("export local db for v1 snapshot")
        .into_bytes();
    let skills_zip = empty_zip_bytes();
    let settings_sync = b"{}".to_vec();
    let manifest = serde_json::json!({
        "format": "cc-switch-webdav-sync",
        "version": 1,
        "updatedAt": "2026-04-15T00:00:00Z",
        "updatedBy": "test",
        "artifacts": {
            "dbSql": {
                "path": "db.sql",
                "sha256": sha256_hex(&db_sql),
                "size": db_sql.len()
            },
            "skillsZip": {
                "path": "skills.zip",
                "sha256": sha256_hex(&skills_zip),
                "size": skills_zip.len()
            },
            "settingsSync": {
                "path": "settings-sync.json",
                "sha256": sha256_hex(&settings_sync),
                "size": settings_sync.len()
            }
        }
    });

    server.seed_file(
        "/dav/sync-root/v1/default-profile/manifest.json",
        serde_json::to_vec(&manifest).expect("serialize v1 manifest"),
    );
    server.seed_file("/dav/sync-root/v1/default-profile/db.sql", db_sql);
    server.seed_file("/dav/sync-root/v1/default-profile/skills.zip", skills_zip);

    runtime.block_on(async {
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
            .expect("update proxy config");

        state
            .proxy_service
            .set_takeover_for_app("claude", true)
            .await
            .expect("enable takeover");
        state
            .proxy_service
            .stop()
            .await
            .expect("stop runtime but keep takeover state");

        let takeover = state
            .proxy_service
            .get_takeover_status()
            .await
            .expect("read takeover status");
        assert!(takeover.claude, "takeover should remain active after stop");
        assert!(
            !state.proxy_service.is_running().await,
            "runtime should be stopped so migrate hits takeover preflight"
        );
    });

    let err = WebDavSyncService::migrate_v1_to_v2()
        .expect_err("migration should reject while takeover remains active");

    assert!(
        err.to_string().contains("接管") || err.to_string().contains("takeover"),
        "unexpected error: {err}"
    );

    runtime.block_on(async {
        state
            .proxy_service
            .set_takeover_for_app("claude", false)
            .await
            .expect("clear takeover state");
    });
}

#[test]
fn webdav_migrate_v1_to_v2_applies_settings_sync() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let server = TestWebDavServer::start();
    set_webdav_sync_settings(Some(sample_settings(&server.base_url)))
        .expect("save test WebDAV settings");

    let state = CcAppState::try_new().expect("create app state");
    seed_claude_remote_provider(&state);

    let db_sql = state
        .db
        .export_sql_string()
        .expect("export local db for v1 snapshot")
        .into_bytes();
    let skills_zip = empty_zip_bytes();
    let settings_sync = serde_json::to_vec_pretty(&serde_json::json!({
        "language": "zh",
        "skillSyncMethod": "copy",
        "security": {
            "auth": {
                "selectedType": "gemini-api-key"
            }
        },
        "customEndpointsClaude": {
            "endpoint-a": {
                "url": "https://claude.example.com",
                "addedAt": 123,
                "lastUsed": 456
            }
        }
    }))
    .expect("serialize v1 settings sync");
    let manifest = serde_json::json!({
        "format": "cc-switch-webdav-sync",
        "version": 1,
        "updatedAt": "2026-04-15T00:00:00Z",
        "updatedBy": "test",
        "artifacts": {
            "dbSql": {
                "path": "db.sql",
                "sha256": sha256_hex(&db_sql),
                "size": db_sql.len()
            },
            "skillsZip": {
                "path": "skills.zip",
                "sha256": sha256_hex(&skills_zip),
                "size": skills_zip.len()
            },
            "settingsSync": {
                "path": "settings-sync.json",
                "sha256": sha256_hex(&settings_sync),
                "size": settings_sync.len()
            }
        }
    });

    server.seed_file(
        "/dav/sync-root/v1/default-profile/manifest.json",
        serde_json::to_vec(&manifest).expect("serialize v1 manifest"),
    );
    server.seed_file("/dav/sync-root/v1/default-profile/db.sql", db_sql);
    server.seed_file("/dav/sync-root/v1/default-profile/skills.zip", skills_zip);
    server.seed_file(
        "/dav/sync-root/v1/default-profile/settings-sync.json",
        settings_sync,
    );

    let summary = WebDavSyncService::migrate_v1_to_v2().expect("migrate v1 to v2");
    assert_eq!(summary.decision, cc_switch_lib::SyncDecision::Download);

    let settings_path = get_app_config_dir().join("settings.json");
    let raw = std::fs::read_to_string(&settings_path).expect("read settings.json");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("parse settings.json");

    assert_eq!(value["language"], "zh");
    assert_eq!(value["skillSyncMethod"], "copy");
    assert_eq!(
        value
            .pointer("/security/auth/selectedType")
            .and_then(|value| value.as_str()),
        Some("gemini-api-key")
    );
    assert_eq!(
        value
            .pointer("/customEndpointsClaude/endpoint-a/url")
            .and_then(|value| value.as_str()),
        Some("https://claude.example.com")
    );
    assert_eq!(
        value
            .pointer("/webdavSync/baseUrl")
            .and_then(|value| value.as_str()),
        Some(server.base_url.as_str()),
        "local WebDAV connection settings must survive applying v1 settings sync"
    );

    let remote_settings_path = "/dav/sync-root/v2/db-v6/default-profile/settings.json";
    let remote_settings = server
        .read_file(remote_settings_path)
        .unwrap_or_else(|| panic!("migrated V2 remote should include {remote_settings_path}"));
    let remote_value: serde_json::Value =
        serde_json::from_slice(&remote_settings).expect("parse remote settings.json");
    assert_eq!(remote_value["language"], "zh");
    assert_eq!(remote_value["skillSyncMethod"], "copy");

    let remote_manifest = server
        .read_file("/dav/sync-root/v2/db-v6/default-profile/manifest.json")
        .expect("read migrated V2 manifest");
    let remote_manifest: serde_json::Value =
        serde_json::from_slice(&remote_manifest).expect("parse migrated V2 manifest");
    assert!(
        remote_manifest
            .pointer("/artifacts/settings.json")
            .is_some(),
        "migrated V2 manifest should track settings.json"
    );
}

#[test]
fn webdav_download_rejects_when_takeover_artifacts_exist_even_if_enabled_flag_is_cleared() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let server = TestWebDavServer::start_with_config(ServerConfig::with_manifest_head(
        ManifestHeadBehavior::Missing,
    ));
    set_webdav_sync_settings(Some(sample_settings(&server.base_url)))
        .expect("save test WebDAV settings");

    let state = CcAppState::try_new().expect("create app state");
    seed_claude_remote_provider(&state);
    seed_claude_live();
    WebDavSyncService::upload().expect("seed remote v2 snapshot");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build test runtime");
    runtime.block_on(async {
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
            .expect("update proxy config");

        state
            .proxy_service
            .set_takeover_for_app("claude", true)
            .await
            .expect("enable takeover");
        state
            .proxy_service
            .stop()
            .await
            .expect("stop runtime but keep takeover artifacts");

        let mut app_proxy = state
            .db
            .get_proxy_config_for_app("claude")
            .await
            .expect("read claude proxy config");
        app_proxy.enabled = false;
        state
            .db
            .update_proxy_config_for_app(app_proxy)
            .await
            .expect("clear enabled flag while keeping takeover artifacts");

        let takeover = state
            .proxy_service
            .get_takeover_status()
            .await
            .expect("read takeover status");
        assert!(
            !takeover.claude,
            "enabled flag should appear cleared for the weak takeover view"
        );
        assert!(
            state
                .db
                .get_live_backup("claude")
                .await
                .expect("read live backup")
                .is_some(),
            "live backup should still exist so strong takeover predicate stays true"
        );
    });

    let err = WebDavSyncService::download()
        .expect_err("download should reject when takeover artifacts remain active");

    assert!(
        err.to_string().contains("接管") || err.to_string().contains("takeover"),
        "unexpected error: {err}"
    );

    runtime.block_on(async {
        state
            .proxy_service
            .set_takeover_for_app("claude", false)
            .await
            .expect("clear takeover artifacts for cleanup");
    });
}
