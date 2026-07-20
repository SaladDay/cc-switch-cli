//! Locate Codex's per-thread state SQLite databases.
//!
//! Codex normally stores `state_5.sqlite` under `CODEX_HOME`, but the
//! `sqlite_home` config key or `CODEX_SQLITE_HOME` can move it elsewhere.
//! History migration and session-title lookup must resolve the same paths.

use std::path::{Path, PathBuf};

use toml_edit::DocumentMut;

pub(crate) const CODEX_STATE_DB_FILENAME: &str = "state_5.sqlite";

const CODEX_SQLITE_HOME_ENV: &str = "CODEX_SQLITE_HOME";

pub(crate) fn codex_state_db_paths(config_dir: &Path, config_text: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    push_unique_path(&mut paths, config_dir.join(CODEX_STATE_DB_FILENAME));

    // Codex config takes precedence over the environment override.
    if let Some(sqlite_home) = sqlite_home_from_codex_config(config_text) {
        push_unique_path(&mut paths, sqlite_home.join(CODEX_STATE_DB_FILENAME));
    } else if let Some(sqlite_home) = sqlite_home_from_env() {
        push_unique_path(&mut paths, sqlite_home.join(CODEX_STATE_DB_FILENAME));
    }
    paths
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

fn sqlite_home_from_codex_config(config_text: &str) -> Option<PathBuf> {
    let doc = config_text.parse::<DocumentMut>().ok()?;
    let raw = doc.get("sqlite_home")?.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    Some(resolve_user_path(raw))
}

fn sqlite_home_from_env() -> Option<PathBuf> {
    let raw = std::env::var(CODEX_SQLITE_HOME_ENV).ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    Some(resolve_user_path(raw))
}

fn resolve_user_path(raw: &str) -> PathBuf {
    if raw == "~" {
        return crate::config::home_dir().unwrap_or_else(|| PathBuf::from(raw));
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = crate::config::home_dir() {
            return home.join(rest);
        }
    }
    if let Some(rest) = raw.strip_prefix("~\\") {
        if let Some(home) = crate::config::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::ffi::OsString;
    use tempfile::tempdir;

    struct SqliteHomeEnvGuard {
        previous: Option<OsString>,
    }

    impl SqliteHomeEnvGuard {
        fn set(path: &Path) -> Self {
            let previous = std::env::var_os(CODEX_SQLITE_HOME_ENV);
            unsafe { std::env::set_var(CODEX_SQLITE_HOME_ENV, path) };
            Self { previous }
        }
    }

    impl Drop for SqliteHomeEnvGuard {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => unsafe { std::env::set_var(CODEX_SQLITE_HOME_ENV, value) },
                None => unsafe { std::env::remove_var(CODEX_SQLITE_HOME_ENV) },
            }
        }
    }

    #[test]
    fn includes_config_sqlite_home_after_default_path() {
        let temp = tempdir().expect("tempdir");
        let sqlite_home = temp.path().join("sqlite-home");
        let config_text = format!("sqlite_home = '{}'\n", sqlite_home.display());

        assert_eq!(
            codex_state_db_paths(temp.path(), &config_text),
            vec![
                temp.path().join(CODEX_STATE_DB_FILENAME),
                sqlite_home.join(CODEX_STATE_DB_FILENAME),
            ]
        );
    }

    #[test]
    fn does_not_duplicate_default_path() {
        let temp = tempdir().expect("tempdir");
        let config_text = format!("sqlite_home = '{}'\n", temp.path().display());

        assert_eq!(
            codex_state_db_paths(temp.path(), &config_text),
            vec![temp.path().join(CODEX_STATE_DB_FILENAME)]
        );
    }

    #[test]
    #[serial]
    fn uses_environment_sqlite_home_when_config_omits_it() {
        let temp = tempdir().expect("tempdir");
        let config_dir = temp.path().join("codex");
        let sqlite_home = temp.path().join("sqlite-home");
        let _env = SqliteHomeEnvGuard::set(&sqlite_home);

        assert_eq!(
            codex_state_db_paths(&config_dir, "model = 'gpt-5'\n"),
            vec![
                config_dir.join(CODEX_STATE_DB_FILENAME),
                sqlite_home.join(CODEX_STATE_DB_FILENAME),
            ]
        );
    }

    #[test]
    #[serial]
    fn config_sqlite_home_takes_precedence_over_environment() {
        let temp = tempdir().expect("tempdir");
        let config_dir = temp.path().join("codex");
        let env_home = temp.path().join("env-home");
        let config_home = temp.path().join("config-home");
        let _env = SqliteHomeEnvGuard::set(&env_home);
        let config_text = format!("sqlite_home = '{}'\n", config_home.display());

        assert_eq!(
            codex_state_db_paths(&config_dir, &config_text),
            vec![
                config_dir.join(CODEX_STATE_DB_FILENAME),
                config_home.join(CODEX_STATE_DB_FILENAME),
            ]
        );
    }
}
