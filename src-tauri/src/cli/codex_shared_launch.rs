//! Opt-in shared history for `start codex`; the ordinary temporary launch is unchanged.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{symlink, DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};
use toml_edit::{value, DocumentMut};

use super::codex_temp_launch::{preview_launch_with, resolve_codex_binary, PreparedCodexLaunch};
use crate::codex_config::{get_codex_config_dir, inject_codex_unified_session_bucket};
use crate::config::atomic_write_private;
use crate::error::AppError;
use crate::provider::Provider;

pub(super) fn launch(
    provider: &Provider,
    native_args: &[OsString],
    dry_run: bool,
) -> Result<(), AppError> {
    // These overrides can select a different provider or storage directory.
    // Other native arguments (including resume/fork and --model) pass through.
    if native_args.iter().any(|arg| {
        let arg = arg.to_string_lossy();
        arg == "--oss"
            || arg == "--config"
            || arg.starts_with("--config=")
            || arg.starts_with("-c") && !arg.starts_with("--")
            || arg == "--profile"
            || arg.starts_with("--profile=")
            || arg.starts_with("-p") && !arg.starts_with("--")
    }) {
        return Err(AppError::Config(crate::t!(
            "--shared-sessions manages provider and storage settings; native --config, --profile and --oss overrides are not supported.",
            "--shared-sessions 管理供应商与存储设置，不支持透传 --config、--profile 或 --oss 覆盖。"
        ).to_string()));
    }
    let source = std::path::absolute(get_codex_config_dir())
        .map_err(|err| AppError::Config(err.to_string()))?;
    let mut prepared = preview_launch_with(provider, &std::env::temp_dir(), resolve_codex_binary)?;
    let source_config = crate::codex_config::read_codex_config_text()?;
    let sqlite_paths = crate::codex_state_db::codex_state_db_paths(&source, &source_config);
    let sqlite_home = sqlite_paths.last().unwrap().parent().unwrap();
    // Config paths are relative to config.toml; environment paths use the cwd.
    let config_has_sqlite_home = source_config
        .parse::<DocumentMut>()
        .ok()
        .is_some_and(|doc| {
            doc.get("sqlite_home")
                .and_then(|item| item.as_str())
                .is_some_and(|path| !path.trim().is_empty())
        });
    let sqlite_home = std::path::absolute(if config_has_sqlite_home {
        source.join(sqlite_home)
    } else {
        sqlite_home.to_path_buf()
    })
    .map_err(|err| AppError::Config(err.to_string()))?;
    let config = shared_config(provider, &sqlite_home)?;
    prepared.codex_home = provider_home(&source, &provider.id);
    if dry_run {
        println!("CODEX_HOME={}", prepared.codex_home.display());
        println!(
            "{} {}",
            crate::t!("Shared history:", "共享历史："),
            source.display()
        );
        println!(
            "{}",
            crate::t!(
                "Dry run; no launch files were written.",
                "仅预览，未写入启动文件。"
            )
        );
        return Ok(());
    }

    let _lock = prepare_home(&prepared.codex_home, &source, provider, &config)?;
    // Codex inherits the lock so killing only the supervisor cannot expose an
    // active home. After native exit/capture, explicitly unlock the shared file
    // description so surviving background children cannot retain a stale lock.
    let fd = _lock.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
        return Err(AppError::io(
            &prepared.codex_home,
            std::io::Error::last_os_error(),
        ));
    }
    let err = handoff_command(&prepared, &sqlite_home, fd, native_args).exec();
    Err(AppError::Config(format!("Failed to launch Codex: {err}")))
}

fn provider_home(source: &Path, provider_id: &str) -> PathBuf {
    source
        .join(".cc-switch-launches")
        .join(format!("{:x}", Sha256::digest(provider_id.as_bytes())))
}

fn shared_config(provider: &Provider, sqlite_home: &Path) -> Result<String, AppError> {
    let text = provider
        .settings_config
        .get("config")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let mut doc = text
        .parse::<DocumentMut>()
        .map_err(|err| AppError::Config(err.to_string()))?;
    let profile = doc
        .get("profile")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let selected = profile
        .as_deref()
        .and_then(|name| doc.get("profiles")?.get(name)?.get("model_provider"))
        .or_else(|| doc.get("model_provider"))
        .and_then(|v| v.as_str())
        .unwrap_or("openai")
        .to_string();
    let existing = doc
        .get("model_providers")
        .and_then(|v| v.get(&selected))
        .cloned();
    let route = if selected == "openai" {
        // Reuse the same official-auth route as the existing unified-history setting.
        let official = inject_codex_unified_session_bucket("")?
            .parse::<DocumentMut>()
            .map_err(|err| AppError::Config(err.to_string()))?;
        let mut route = official["model_providers"]["custom"].clone();
        // Codex's built-in openai route ignores model_providers.openai and uses
        // the top-level endpoint override instead. Keep that endpoint on custom.
        if let Some(url) = doc
            .get("openai_base_url")
            .and_then(|item| item.as_str())
            .filter(|url| !url.is_empty())
        {
            route["base_url"] = value(url);
        }
        route
    } else {
        existing.filter(|v| v.is_table_like()).ok_or_else(|| {
            AppError::Config(format!(
                "--shared-sessions requires a configured model_providers.{selected} table"
            ))
        })?
    };
    // Values work with both regular and inline model_providers tables.
    doc["model_providers"]["custom"] = toml_edit::Item::Value(
        route
            .into_value()
            .map_err(|_| AppError::Config("Invalid Codex provider table".into()))?,
    );
    doc["model_provider"] = value("custom");
    if let Some(profile) = profile {
        if doc.get("profiles").and_then(|v| v.get(&profile)).is_some() {
            doc["profiles"][&profile]["model_provider"] = value("custom");
        }
    }
    doc["sqlite_home"] = value(sqlite_home.to_string_lossy().as_ref());
    doc["cli_auth_credentials_store"] = value("file");
    Ok(doc.to_string())
}

fn private_dir(path: &Path) -> Result<(), AppError> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(path).map_err(|err| AppError::io(path, err))?;
            if metadata.is_dir() {
                Ok(())
            } else {
                Err(AppError::Config(format!(
                    "Expected a directory, not a symlink or file: {}",
                    path.display()
                )))
            }
        }
        Err(err) => Err(AppError::io(path, err)),
    }
}

fn prepare_home(
    home: &Path,
    source: &Path,
    provider: &Provider,
    config: &str,
) -> Result<File, AppError> {
    fs::create_dir_all(source).map_err(|err| AppError::io(source, err))?;
    private_dir(home.parent().unwrap())?;
    private_dir(home)?;
    let lock_path = home.join(".lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&lock_path)
        .map_err(|err| AppError::io(&lock_path, err))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::WouldBlock {
            return Err(AppError::Config(
                crate::t!(
                    "This provider already has an active --shared-sessions launch.",
                    "此供应商已有一个正在运行的 --shared-sessions 实例。"
                )
                .to_string(),
            ));
        }
        return Err(AppError::io(&lock_path, err));
    }
    let source = source
        .canonicalize()
        .map_err(|err| AppError::io(source, err))?;
    // SQLite supplies the shared resume/title index. Codex atomically replaces
    // session_index.jsonl on deletion, so that local cache must not be a symlink.
    for (name, directory) in [
        ("sessions", true),
        ("archived_sessions", true),
        ("thread-writer-locks", true),
        ("rollout-migrations", true),
        (".tmp/rollout-maintenance.lock", false),
    ] {
        let target = source.join(name);
        if directory {
            fs::create_dir_all(&target).map_err(|err| AppError::io(&target, err))?;
        } else {
            private_dir(&home.join(".tmp"))?;
            fs::create_dir_all(target.parent().unwrap())
                .map_err(|err| AppError::io(&target, err))?;
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(&target)
                .map_err(|err| AppError::io(&target, err))?;
        }
        let link = home.join(name);
        match symlink(&target, &link) {
            Ok(()) => (),
            Err(err)
                if err.kind() == std::io::ErrorKind::AlreadyExists
                    && fs::read_link(&link).ok().as_ref() == Some(&target) => {}
            Err(err) => return Err(AppError::io(&link, err)),
        }
    }
    atomic_write_private(&home.join("config.toml"), config.as_bytes())?;
    let auth_path = home.join("auth.json");
    let auth = provider
        .settings_config
        .get("auth")
        .filter(|auth| auth.as_object().is_some_and(|v| !v.is_empty()))
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let auth_source_path = home.join(".auth-source");
    let auth_source = format!("{:x}", Sha256::digest(auth.to_string().as_bytes()));
    // The DB is the source for explicit credential edits. If it is unchanged,
    // keep Codex's local refresh/logout, including after an untrappable exit.
    if fs::read_to_string(&auth_source_path).ok().as_deref() == Some(&auth_source) {
        return Ok(lock);
    }
    if auth.as_object().is_some_and(|auth| !auth.is_empty()) {
        let bytes = serde_json::to_vec_pretty(&auth)
            .map_err(|source| AppError::JsonSerialize { source })?;
        atomic_write_private(&auth_path, &bytes)?;
    } else {
        match fs::remove_file(&auth_path) {
            Ok(()) => (),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => (),
            Err(err) => return Err(AppError::io(&auth_path, err)),
        }
    }
    atomic_write_private(&auth_source_path, auth_source.as_bytes())?;
    Ok(lock)
}

fn handoff_command(
    prepared: &PreparedCodexLaunch,
    sqlite_home: &Path,
    lock_fd: i32,
    native_args: &[OsString],
) -> Command {
    let mut command = Command::new("/bin/sh");
    command.arg("-c").arg(
        "provider_id=\"$1\"; export CODEX_HOME=\"$2\"; codex_bin=\"$3\"; cc_switch_bin=\"$4\"; lock_fd=\"$5\"; shift 5; persist() { \"$cc_switch_bin\" internal capture-codex-temp \"$provider_id\" \"$CODEX_HOME\" --auth-only; persist_status=$?; if [ \"$persist_status\" -ne 0 ]; then printf '%s\\n' 'cc-switch: failed to persist Codex login state' >&2; if [ \"$exit_status\" -eq 0 ]; then exit_status=$persist_status; fi; fi; \"$cc_switch_bin\" internal release-codex-lock \"$lock_fd\"; }; on_signal() { exit_status=\"$1\"; trap - INT TERM HUP; persist; exit \"$exit_status\"; }; trap 'on_signal 130' INT; trap 'on_signal 143' TERM; trap 'on_signal 129' HUP; \"$codex_bin\" \"$@\"; exit_status=$?; persist; exit \"$exit_status\""
    );
    command
        .arg("cc-switch-codex-shared-handoff")
        .arg(&prepared.provider_id)
        .arg(&prepared.codex_home)
        .arg(&prepared.executable)
        .arg(&prepared.cc_switch_executable)
        .arg(lock_fd.to_string())
        // Native resume otherwise restores the provider saved in the old thread.
        // An explicit CLI override keeps the provider selected for this launch.
        .args(["--config", "model_provider=\"custom\""])
        .args(["--config", "cli_auth_credentials_store=\"file\""])
        .arg("--config")
        .arg(format!(
            "sqlite_home={}",
            toml_edit::Value::from(sqlite_home.to_string_lossy().as_ref())
        ))
        .args(native_args);
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnvGuard;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    fn provider(id: &str, config: &str) -> Provider {
        Provider::with_id(
            id.into(),
            id.into(),
            json!({
                "config": config, "auth": {"OPENAI_API_KEY": format!("key-{id}")}
            }),
            None,
        )
    }

    #[test]
    fn shared_config_preserves_selected_endpoint_and_original_settings() {
        let original = "model_provider = 'relay'\nmodel = 'demo'\n[model_providers.relay]\nname = 'Relay'\nbase_url = 'https://relay.invalid/v1'\nexperimental_bearer_token = 'private-token'\nwire_api = 'responses'\n";
        let provider = provider("relay", original);
        let config = shared_config(&provider, Path::new("/shared/sqlite")).unwrap();
        let doc = config.parse::<DocumentMut>().unwrap();
        assert_eq!(doc["model_provider"].as_str(), Some("custom"));
        assert_eq!(
            doc["model_providers"]["custom"]["base_url"].as_str(),
            Some("https://relay.invalid/v1")
        );
        assert_eq!(
            doc["model_providers"]["custom"]["experimental_bearer_token"].as_str(),
            Some("private-token")
        );
        assert_eq!(doc["sqlite_home"].as_str(), Some("/shared/sqlite"));
        assert_eq!(doc["cli_auth_credentials_store"].as_str(), Some("file"));
        assert_eq!(provider.settings_config["config"], original);
    }

    #[test]
    fn shared_config_reuses_official_route_and_selected_config_profile() {
        for config in ["", "model_provider = 'openai'\n"] {
            let doc = shared_config(&provider("official", config), Path::new("/shared"))
                .unwrap()
                .parse::<DocumentMut>()
                .unwrap();
            assert_eq!(doc["model_provider"].as_str(), Some("custom"));
            assert_eq!(
                doc["model_providers"]["custom"]["requires_openai_auth"].as_bool(),
                Some(true)
            );
        }
        let config = "openai_base_url = 'http://localhost:4321/v1'\n[model_providers.openai]\nname = 'Ignored by native Codex'\nbase_url = 'https://unused.invalid'\n";
        let doc = shared_config(&provider("official", config), Path::new("/shared"))
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        assert_eq!(
            doc["model_providers"]["custom"]["base_url"].as_str(),
            Some("http://localhost:4321/v1")
        );
        let config = "profile = 'work'\n[profiles.work]\nmodel_provider = 'relay'\nmodel = 'work-model'\n[model_providers.relay]\nname = 'Relay'\nbase_url = 'https://relay.invalid'\n";
        let doc = shared_config(&provider("profile", config), Path::new("/shared"))
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        assert_eq!(
            doc["profiles"]["work"]["model_provider"].as_str(),
            Some("custom")
        );
        assert_eq!(
            doc["profiles"]["work"]["model"].as_str(),
            Some("work-model")
        );
        assert_eq!(
            doc["model_providers"]["custom"]["base_url"].as_str(),
            Some("https://relay.invalid")
        );
    }

    #[test]
    fn shared_homes_keep_history_paths_and_isolate_credentials() {
        let temp = TempDir::new().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let source = temp.path().join(".codex");
        let a = provider("../team", "");
        let b = provider("plus", "");
        let a_home = provider_home(&source, &a.id);
        let b_home = provider_home(&source, &b.id);
        let config = shared_config(&a, &source).unwrap();
        let a_lock = prepare_home(&a_home, &source, &a, &config).unwrap();
        let b_lock = prepare_home(&b_home, &source, &b, &config).unwrap();
        assert!(a_home.starts_with(source.join(".cc-switch-launches")));
        fs::write(a_home.join("sessions/from-team.jsonl"), "history").unwrap();
        fs::write(a_home.join("archived_sessions/from-team.jsonl"), "archived").unwrap();
        assert_eq!(
            fs::read_to_string(b_home.join("sessions/from-team.jsonl")).unwrap(),
            "history"
        );
        assert_eq!(
            fs::read_to_string(b_home.join("archived_sessions/from-team.jsonl")).unwrap(),
            "archived"
        );
        assert_ne!(
            fs::read(a_home.join("auth.json")).unwrap(),
            fs::read(b_home.join("auth.json")).unwrap()
        );
        assert!(!source.join("auth.json").exists());
        assert!(!source.join("config.toml").exists());
        for name in [
            "thread-writer-locks/thread.lock",
            ".tmp/rollout-maintenance.lock",
        ] {
            let a_lock = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(a_home.join(name))
                .unwrap();
            a_lock.try_lock().unwrap();
            let b_lock = OpenOptions::new()
                .write(true)
                .open(b_home.join(name))
                .unwrap();
            assert!(matches!(
                b_lock.try_lock(),
                Err(fs::TryLockError::WouldBlock)
            ));
            drop(a_lock);
            b_lock.try_lock().unwrap();
        }
        assert_eq!(
            fs::metadata(a_home.join("auth.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(prepare_home(&a_home, &source, &b, &config).is_err());
        assert!(fs::read_to_string(a_home.join("auth.json"))
            .unwrap()
            .contains("key-../team"));
        drop(a_lock);
        drop(b_lock);
        assert!(a_home.join("sessions/from-team.jsonl").exists());
        let _again = prepare_home(&a_home, &source, &a, &config).unwrap();
        assert_eq!(
            fs::read_to_string(a_home.join("sessions/from-team.jsonl")).unwrap(),
            "history"
        );
        fs::remove_file(a_home.join("auth.json")).unwrap();
        drop(_again);
        let _after_logout = prepare_home(&a_home, &source, &a, &config).unwrap();
        assert!(!a_home.join("auth.json").exists());
    }

    #[test]
    fn shared_home_rejects_redirected_private_directory() {
        let temp = TempDir::new().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let source = temp.path().join(".codex");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, source.join(".cc-switch-launches")).unwrap();
        let provider = provider("team", "");
        assert!(prepare_home(
            &provider_home(&source, &provider.id),
            &source,
            &provider,
            ""
        )
        .is_err());
        assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
    }
}
