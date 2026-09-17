//! Explicit activation of a managed account for the next standalone Codex process.
use crate::services::{CodexOAuthService, ProviderService};
use crate::{app_config::AppType, database::Database};
use serde_json::Value;

pub fn active_account_id() -> Option<String> {
    let auth: Value =
        serde_json::from_slice(&std::fs::read(crate::codex_config::get_codex_auth_path()).ok()?)
            .ok()?;
    if auth
        .get("auth_mode")
        .and_then(Value::as_str)
        .is_some_and(|mode| mode != "chatgpt")
    {
        return None;
    }
    auth.pointer("/tokens/account_id")?
        .as_str()
        .map(str::to_owned)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct NativeAuthCopies {
    live: Option<(std::path::PathBuf, String)>,
    providers: Vec<(String, String)>,
}

impl NativeAuthCopies {
    pub(crate) fn read(account_id: &str, refresh_token: &str) -> Result<Self, String> {
        use sha2::{Digest, Sha256};
        let matches = |auth: &Value| {
            auth.pointer("/tokens/account_id").and_then(Value::as_str) == Some(account_id)
                && auth
                    .pointer("/tokens/refresh_token")
                    .and_then(Value::as_str)
                    == Some(refresh_token)
        };
        let path = crate::codex_config::get_codex_auth_path();
        let live = match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.to_string()),
        }
        .filter(|bytes| {
            serde_json::from_slice::<Value>(bytes)
                .ok()
                .is_some_and(|auth| matches(&auth))
        })
        .map(|bytes| (path, format!("{:x}", Sha256::digest(&bytes))));
        let db = crate::config::get_app_config_dir()
            .join("cc-switch.db")
            .exists()
            .then(Database::init)
            .transpose()
            .map_err(|e| e.to_string())?;
        let mut providers = Vec::new();
        if let Some(db) = &db {
            for (id, provider) in db.get_all_providers("codex").map_err(|e| e.to_string())? {
                if ProviderService::codex_live_write_category(&provider) == Some("official") {
                    if let Some(auth) = provider
                        .settings_config
                        .get("auth")
                        .filter(|auth| matches(auth))
                    {
                        providers.push((
                            id,
                            format!("{:x}", Sha256::digest(auth.to_string().as_bytes())),
                        ));
                    }
                }
            }
        }
        Ok(Self { live, providers })
    }

    pub(crate) fn publish(&self, auth: &Value) -> Result<(), String> {
        use sha2::{Digest, Sha256};
        if let Some((path, expected)) = &self.live {
            let current = match std::fs::read(path) {
                Ok(bytes) => Some(bytes),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e.to_string()),
            };
            if let Some(bytes) = current {
                if &format!("{:x}", Sha256::digest(&bytes)) == expected
                    && serde_json::from_slice::<Value>(&bytes).ok().as_ref() != Some(auth)
                {
                    crate::config::atomic_write_private(
                        path,
                        &serde_json::to_vec_pretty(auth).map_err(|e| e.to_string())?,
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
        }
        if !self.providers.is_empty() {
            let db = Database::init().map_err(|e| e.to_string())?;
            let target = format!("{:x}", Sha256::digest(auth.to_string().as_bytes()));
            let providers = db.get_all_providers("codex").map_err(|e| e.to_string())?;
            for (id, source) in &self.providers {
                if source == &target || !providers.contains_key(id) {
                    continue;
                }
                db.update_codex_provider_auth(id, auth, source)
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
}

/// Import accepted launch changes while the caller holds the state mutation guard.
#[cfg(feature = "cli")]
pub(crate) async fn capture_native_auth_locked(auth: &Value) -> Result<(), String> {
    let manager = CodexOAuthService::manager();
    let _lock = manager.lock_store().await.map_err(|e| e.to_string())?;
    manager
        .reload_from_disk()
        .await
        .map_err(|e| e.to_string())?;
    manager
        .capture_codex_auth(auth)
        .await
        .map_err(|e| e.to_string())
}

/// Only auth and the current official provider snapshot change. config.toml is never rewritten.
pub async fn use_account(account_id: &str) -> Result<(), String> {
    let _guard = super::state_coordination::acquire_restore_mutation_guard().await?;
    // The legacy path helper falls back to ~/.codex when CODEX_HOME is absent on disk.
    // Create an explicit home first so activation never writes to that fallback.
    if crate::settings::get_codex_override_dir().is_none() {
        if let Some(home) = std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()) {
            std::fs::create_dir_all(home).map_err(|e| e.to_string())?;
        }
    }
    let db = Database::init().map_err(|e| e.to_string())?;
    let config_path = crate::codex_config::get_codex_config_path();
    let config = match std::fs::read_to_string(&config_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.to_string()),
    };
    let mut parsed: toml::Value = config
        .parse()
        .map_err(|_| "Invalid Codex config.toml".to_string())?;
    // Codex resolves the selected profile before root settings. Normalize only our
    // exact official history bucket in this validation copy, never the live file.
    if let Some(profile) = parsed.get("profile").and_then(|v| v.as_str()) {
        let overrides = parsed
            .get("profiles")
            .and_then(|v| v.get(profile))
            .cloned()
            .ok_or_else(|| "The selected Codex profile does not exist.".to_string())?;
        let overrides = overrides
            .as_table()
            .ok_or_else(|| "Invalid Codex profile".to_string())?;
        // Of the settings validated here, only model_provider is a Codex
        // profile field. Credential storage and forced-login constraints are
        // root settings; unknown profile keys must not override them.
        if let Some(model_provider) = overrides.get("model_provider") {
            parsed
                .as_table_mut()
                .ok_or_else(|| "Invalid Codex config table".to_string())?
                .insert("model_provider".into(), model_provider.clone());
        }
    }
    let normalized = crate::codex_config::strip_codex_unified_session_bucket(
        &toml::to_string(&parsed).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let parsed: toml::Value = normalized
        .parse()
        .map_err(|e: toml::de::Error| e.to_string())?;
    if let Some(workspace) = parsed.get("forced_chatgpt_workspace_id") {
        let allowed = match workspace {
            toml::Value::String(id) => id == account_id,
            toml::Value::Array(ids) if ids.iter().all(|id| id.as_str().is_some()) => {
                ids.iter().any(|id| id.as_str() == Some(account_id))
            }
            _ => return Err("Invalid forced_chatgpt_workspace_id.".into()),
        };
        if !allowed {
            return Err("The selected account does not match forced_chatgpt_workspace_id.".into());
        }
    }
    if parsed
        .get("cli_auth_credentials_store")
        .and_then(|v| v.as_str())
        .is_some_and(|v| v != "file")
    {
        return Err(
            "Codex account activation requires cli_auth_credentials_store = \"file\".".into(),
        );
    }
    if parsed.get("forced_login_method").and_then(|v| v.as_str()) == Some("api") {
        return Err("Codex is configured for API-key login.".into());
    }
    if parsed
        .get("model_provider")
        .and_then(|v| v.as_str())
        .is_some_and(|v| v != "openai")
    {
        return Err(
            "Select the official Codex provider before activating a ChatGPT account.".into(),
        );
    }
    if db
        .get_proxy_config_for_app_or_default("codex")
        .await
        .map_err(|e| e.to_string())?
        .enabled
    {
        return Err(
            "Disable Codex proxy takeover before activating a standalone Codex account.".into(),
        );
    }
    let current = crate::settings::get_effective_current_provider(&db, &AppType::Codex)
        .map_err(|e| e.to_string())?;
    let providers = db.get_all_providers("codex").map_err(|e| e.to_string())?;
    let provider = current.as_ref().and_then(|id| providers.get(id));
    if provider.is_some_and(|p| ProviderService::codex_live_write_category(p) != Some("official")) {
        return Err(
            "Select the official Codex provider before activating a ChatGPT account.".into(),
        );
    }
    let auth_path = crate::codex_config::get_codex_auth_path();
    let previous = match std::fs::read(&auth_path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    let previous_auth = previous
        .as_ref()
        .map(|bytes| {
            serde_json::from_slice::<Value>(bytes)
                .map_err(|_| "Invalid Codex auth.json".to_string())
        })
        .transpose()?;
    let outgoing_id = previous_auth
        .as_ref()
        .and_then(|a| a.pointer("/tokens/account_id"))
        .and_then(Value::as_str);
    let manager = CodexOAuthService::manager();
    let _account_lock = manager.lock_store().await.map_err(|e| e.to_string())?;
    manager
        .reload_from_disk()
        .await
        .map_err(|e| e.to_string())?;
    if !manager.contains_account(account_id).await {
        return Err(format!("Managed account not found: {account_id}"));
    }
    for provider in providers
        .values()
        .filter(|p| ProviderService::codex_live_write_category(p) == Some("official"))
    {
        if let Some(auth) = provider.settings_config.get("auth").filter(|auth| {
            let id = auth.pointer("/tokens/account_id").and_then(Value::as_str);
            id == Some(account_id) || (outgoing_id.is_some() && id == outgoing_id)
        }) {
            manager
                .capture_codex_auth(auth)
                .await
                .map_err(|e| e.to_string())?;
        }
    }
    if let Some(live) = &previous_auth {
        manager
            .capture_codex_auth(live)
            .await
            .map_err(|e| e.to_string())?;
    }
    let auth = manager
        .export_codex_auth(account_id)
        .await
        .map_err(|e| e.to_string())?;
    let previous = match std::fs::read(&auth_path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    let provider_before = provider
        .map(|p| {
            db.get_all_providers("codex")
                .map(|all| all.get(&p.id).cloned())
        })
        .transpose()
        .map_err(|e| e.to_string())?
        .flatten();
    let bytes = serde_json::to_vec_pretty(&auth).map_err(|e| e.to_string())?;
    crate::config::atomic_write_private(&auth_path, &bytes).map_err(|e| e.to_string())?;
    let result = async {
        if let Some(provider) = provider {
            let mut settings = provider.settings_config.clone();
            settings["auth"] = auth;
            db.update_provider_settings_config("codex", &provider.id, &settings)
                .map_err(|e| e.to_string())?;
        }
        manager
            .set_default_account_locked(account_id)
            .await
            .map_err(|e| e.to_string())
    }
    .await;
    if let Err(error) = result {
        let restore = match previous {
            Some(bytes) => {
                crate::config::atomic_write_private(&auth_path, &bytes).map_err(|e| e.to_string())
            }
            None => std::fs::remove_file(&auth_path).map_err(|e| e.to_string()),
        };
        let provider_restore = provider_before
            .as_ref()
            .map(|p| db.update_provider_settings_config("codex", &p.id, &p.settings_config))
            .transpose();
        if restore.is_err() || provider_restore.is_err() {
            return Err(format!(
                "{error}; rollback failed; inspect Codex account status before starting Codex."
            ));
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use serde_json::json;

    fn auth(id: &str, refresh: &str) -> Value {
        let claims = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({"exp": chrono::Utc::now().timestamp() + 3600})).unwrap(),
        );
        json!({"auth_mode":"chatgpt", "OPENAI_API_KEY":null,
            "tokens":{"account_id":id, "access_token":format!("e30.{claims}.sig"), "refresh_token":refresh, "id_token":"test-id-token"},
            "last_refresh":"2026-01-01T00:00:00Z"})
    }

    fn seed() -> (tempfile::TempDir, crate::test_support::TestEnvGuard) {
        let temp = tempfile::tempdir().unwrap();
        let env = crate::test_support::TestEnvGuard::isolated(temp.path());
        let dir = crate::config::get_app_config_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let mut accounts = serde_json::Map::new();
        for id in ["a", "b"] {
            accounts.insert(
                id.into(),
                json!({"account_id":id,"refresh_token":format!("refresh-{id}"),
                "authenticated_at":1,"codex_auth":auth(id,&format!("refresh-{id}"))}),
            );
        }
        crate::config::write_json_file(
            &dir.join("codex_oauth_auth.json"),
            &json!({"version":1,"default_account_id":"a","accounts":accounts}),
        )
        .unwrap();
        crate::config::write_json_file(
            &crate::codex_config::get_codex_auth_path(),
            &auth("a", "refresh-a"),
        )
        .unwrap();
        (temp, env)
    }

    #[tokio::test]
    async fn use_account_round_trip_preserves_config_and_rotated_credentials() {
        let (_temp, _env) = seed();
        let config = "# Keep comments\nmodel = \"gpt-5\"\n[projects.\"/work\"]\ntrust_level = \"trusted\"\n[mcp_servers.demo]\ncommand = \"demo\"\n";
        crate::config::write_text_file(&crate::codex_config::get_codex_config_path(), config)
            .unwrap();
        use_account("b").await.unwrap();
        assert_eq!(active_account_id().as_deref(), Some("b"));
        let mut refreshed = auth("b", "rotated-b");
        refreshed["last_refresh"] = json!("2026-02-01T00:00:00Z");
        crate::config::write_json_file(&crate::codex_config::get_codex_auth_path(), &refreshed)
            .unwrap();
        use_account("a").await.unwrap();
        use_account("b").await.unwrap();
        let live: Value = serde_json::from_slice(
            &std::fs::read(crate::codex_config::get_codex_auth_path()).unwrap(),
        )
        .unwrap();
        assert_eq!(live["tokens"]["refresh_token"], "rotated-b");
        assert_eq!(
            std::fs::read_to_string(crate::codex_config::get_codex_config_path()).unwrap(),
            config
        );
        let fresh = crate::proxy::providers::codex_oauth_auth::CodexOAuthManager::new(
            crate::config::get_app_config_dir(),
        );
        assert_eq!(fresh.default_account_id().await.as_deref(), Some("b"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(crate::codex_config::get_codex_auth_path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[tokio::test]
    async fn use_account_updates_only_current_official_snapshot() {
        let (_temp, _env) = seed();
        let db = Database::init().unwrap();
        for id in ["official", "other"] {
            let mut provider = crate::provider::Provider::with_id(
                id.into(),
                id.into(),
                json!({"auth":auth("a","refresh-a"),"config":"model = \"gpt-5\""}),
                None,
            );
            provider.category = Some("official".into());
            db.save_provider("codex", &provider).unwrap();
        }
        db.set_current_provider("codex", "official").unwrap();
        use_account("b").await.unwrap();
        let providers = db.get_all_providers("codex").unwrap();
        assert_eq!(
            providers["official"].settings_config["auth"]["tokens"]["account_id"],
            "b"
        );
        assert_eq!(
            providers["other"].settings_config["auth"]["tokens"]["account_id"],
            "a"
        );
    }

    #[tokio::test]
    async fn use_account_rejects_incompatible_config_without_changing_live_auth() {
        let (_temp, _env) = seed();
        let path = crate::codex_config::get_codex_auth_path();
        let original = std::fs::read(&path).unwrap();
        for config in [
            "model_provider = \"third-party\"",
            "cli_auth_credentials_store = \"keyring\"",
            "forced_login_method = \"api\"",
            "forced_chatgpt_workspace_id = \"different\"",
            r#"forced_chatgpt_workspace_id = ["a"]"#,
            r#"forced_chatgpt_workspace_id = []"#,
            "cli_auth_credentials_store = \"keyring\"\nprofile = \"p\"\n[profiles.p]\ncli_auth_credentials_store = \"file\"",
            "forced_login_method = \"api\"\nprofile = \"p\"\n[profiles.p]\nforced_login_method = \"chatgpt\"",
            "forced_chatgpt_workspace_id = \"a\"\nprofile = \"p\"\n[profiles.p]\nforced_chatgpt_workspace_id = \"b\"",
        ] {
            crate::config::write_text_file(&crate::codex_config::get_codex_config_path(), config)
                .unwrap();
            assert!(use_account("b").await.is_err(), "{config}");
            assert_eq!(std::fs::read(&path).unwrap(), original);
        }
    }

    #[tokio::test]
    async fn use_account_accepts_allowed_workspace_list() {
        let (_temp, _env) = seed();
        let config = "forced_chatgpt_workspace_id = [\"a\", \"b\"]";
        crate::config::write_text_file(&crate::codex_config::get_codex_config_path(), config)
            .unwrap();
        use_account("b").await.unwrap();
        assert_eq!(active_account_id().as_deref(), Some("b"));
        assert_eq!(
            std::fs::read_to_string(crate::codex_config::get_codex_config_path()).unwrap(),
            config
        );
    }

    #[tokio::test]
    async fn use_account_missing_account_keeps_current_login() {
        let (_temp, _env) = seed();
        assert!(use_account("missing").await.is_err());
        assert_eq!(active_account_id().as_deref(), Some("a"));
        assert_eq!(
            CodexOAuthService::manager()
                .default_account_id()
                .await
                .as_deref(),
            Some("a")
        );
    }

    #[tokio::test]
    async fn auth_default_does_not_activate_codex() {
        let (_temp, _env) = seed();
        CodexOAuthService::set_default_account("b").await.unwrap();
        assert_eq!(active_account_id().as_deref(), Some("a"));
        let status = crate::services::AuthService::get_status("codex_oauth")
            .await
            .unwrap();
        assert_eq!(status.default_account_id.as_deref(), Some("b"));
        assert_eq!(status.active_codex_account_id.as_deref(), Some("a"));
    }
    #[tokio::test]
    async fn use_account_restores_live_login_on_snapshot_failure() {
        let (_temp, _env) = seed();
        let db = Database::init().unwrap();
        let existing: Value =
            crate::config::read_json_file(&crate::codex_config::get_codex_auth_path()).unwrap();
        let mut provider = crate::provider::Provider::with_id(
            "official".into(),
            "Official".into(),
            json!({"auth":existing,"config":""}),
            None,
        );
        provider.category = Some("official".into());
        db.save_provider("codex", &provider).unwrap();
        db.set_current_provider("codex", "official").unwrap();
        let conn =
            rusqlite::Connection::open(crate::config::get_app_config_dir().join("cc-switch.db"))
                .unwrap();
        conn.execute_batch("CREATE TRIGGER reject_auth BEFORE UPDATE OF settings_config ON providers BEGIN SELECT RAISE(ABORT, 'test failure'); END;").unwrap();
        let before = std::fs::read(crate::codex_config::get_codex_auth_path()).unwrap();
        assert!(use_account("b").await.is_err());
        assert_eq!(
            std::fs::read(crate::codex_config::get_codex_auth_path()).unwrap(),
            before
        );
        assert_eq!(
            CodexOAuthService::manager()
                .default_account_id()
                .await
                .as_deref(),
            Some("a")
        );
    }

    #[tokio::test]
    async fn use_account_is_seen_by_an_already_running_manager() {
        let (_temp, _env) = seed();
        let cached = crate::proxy::providers::codex_oauth_auth::CodexOAuthManager::new(
            crate::config::get_app_config_dir(),
        );
        assert_eq!(cached.default_account_id().await.as_deref(), Some("a"));
        use_account("b").await.unwrap();
        assert_eq!(cached.default_account_id().await.as_deref(), Some("b"));
    }
    #[tokio::test]
    async fn use_account_supports_official_unified_history_without_rewriting_config() {
        let (_temp, _env) = seed();
        let config =
            crate::codex_config::inject_codex_unified_session_bucket("model = \"gpt-5\"\n")
                .unwrap();
        crate::config::write_text_file(&crate::codex_config::get_codex_config_path(), &config)
            .unwrap();
        use_account("b").await.unwrap();
        assert_eq!(active_account_id().as_deref(), Some("b"));
        assert_eq!(
            std::fs::read_to_string(crate::codex_config::get_codex_config_path()).unwrap(),
            config
        );
    }

    #[tokio::test]
    async fn use_account_honors_selected_profile_and_ignores_unused_profiles() {
        let (_temp, _env) = seed();
        let config = "profile = \"third\"\n[profiles.third]\nmodel_provider = \"third-party\"\n";
        crate::config::write_text_file(&crate::codex_config::get_codex_config_path(), config)
            .unwrap();
        assert!(use_account("b").await.is_err());
        assert_eq!(active_account_id().as_deref(), Some("a"));
        let config = "profile = \"official\"\nmodel_provider = \"third-party\"\n[profiles.official]\nmodel_provider = \"openai\"\n[profiles.unused]\nmodel_provider = \"third-party\"\n";
        crate::config::write_text_file(&crate::codex_config::get_codex_config_path(), config)
            .unwrap();
        use_account("b").await.unwrap();
        assert_eq!(active_account_id().as_deref(), Some("b"));
        assert_eq!(
            std::fs::read_to_string(crate::codex_config::get_codex_config_path()).unwrap(),
            config
        );
    }
}
