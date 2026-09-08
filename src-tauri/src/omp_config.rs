//! Thin adapter for OMP's native files.
//!
//! OMP owns account login and model selection in its native YAML files.
//! CC Switch only manages explicit provider entries in `models.yml`.

use crate::config::{atomic_write_private, get_home_dir};
use crate::error::AppError;
use indexmap::IndexMap;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, LazyLock, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};
use url::Url;

const MAX_OMP_FILE_BYTES: u64 = 1024 * 1024;
const MAX_COMMAND_OUTPUT_BYTES: usize = 64 * 1024;
const MISSING_MODELS_REVISION: &str = "missing";
const MISSING_CONFIG_REVISION: &str = "missing";
pub const OMP_DEFAULT_API_PROTOCOL: &str = "openai-completions";
pub const OMP_API_PROTOCOLS: [&str; 9] = [
    "openai-completions",
    "openai-responses",
    "openai-codex-responses",
    "azure-openai-responses",
    "anthropic-messages",
    "bedrock-converse-stream",
    "google-generative-ai",
    "google-gemini-cli",
    "google-vertex",
];
/// OMP's default timeout for native model discovery requests.
pub const OMP_DEFAULT_DISCOVERY_TIMEOUT_MS: u64 = 10_000;
/// Keep user-configured discovery requests bounded even when a malformed or
/// hostile native file contains an impractically large timeout.
pub const OMP_MAX_DISCOVERY_TIMEOUT_MS: u64 = 120_000;
/// Roles understood by the stock OMP model selector. OMP also accepts custom
/// role names through `modelTags`; those names are intentionally not
/// hard-coded by the adapter and are returned when present in `config.yml`.
pub const OMP_BUILTIN_MODEL_ROLES: [&str; 10] = [
    "default", "smol", "slow", "vision", "plan", "designer", "commit", "tiny", "task", "advisor",
];
static MODELS_FILE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static COMMAND_VALUE_CACHE: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static COMMAND_FAILURE_CACHE: LazyLock<Mutex<HashMap<String, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
#[cfg(test)]
static TEST_AGENT_DIR: LazyLock<Mutex<Option<PathBuf>>> = LazyLock::new(|| Mutex::new(None));

/// Resolve directory-affecting variables the way OMP's dotenv loader does.
/// The process environment wins; otherwise values are read from the current
/// project, active agent/config roots, and home `.env` files. A second pass
/// covers a custom `PI_CODING_AGENT_DIR` or `PI_CONFIG_DIR` introduced by one
/// of those files without recursing through `get_omp_agent_dir`.
fn resolve_omp_path_environment() -> HashMap<String, std::ffi::OsString> {
    let mut resolved = std::env::vars_os()
        .map(|(key, value)| (key.to_string_lossy().into_owned(), value))
        .collect::<HashMap<_, _>>();
    // Only OMP_PROFILE is a documented OMP_* spelling for a PI_* variable.
    // Do not invent aliases for arbitrary OMP_* names (for example
    // OMP_CONFIG_DIR): the native executable ignores those process variables,
    // so treating them as PI_CONFIG_DIR here would make CC-Switch edit a
    // directory that `omp` never reads.
    let mut aliases = resolved
        .iter()
        .filter(|(key, _)| key.as_str() == "OMP_PROFILE")
        .map(|(_, value)| ("PI_PROFILE".to_string(), value.clone()))
        .collect::<Vec<_>>();
    for (key, value) in aliases.drain(..) {
        // OMP_PROFILE is canonical and must override the legacy PI_PROFILE
        // value when both are exported, matching OMP's profile resolver.
        resolved.insert(key, value);
    }

    let home = get_home_dir();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut paths = vec![cwd.join(".env")];
    let config_dir = resolved
        .get("PI_CONFIG_DIR")
        .cloned()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".omp"));
    let config_root = omp_config_root(&home, config_dir);
    let profile = effective_profile_lossy(&resolved);
    let profile_root = profile
        .as_ref()
        .map(|name| config_root.join("profiles").join(name))
        .unwrap_or_else(|| config_root.clone());
    let agent_dir = effective_agent_dir_lossy(&resolved, &profile, &config_root)
        .unwrap_or_else(|| profile_root.join("agent"));
    paths.extend([
        agent_dir.join(".env"),
        profile_root.join(".env"),
        home.join(".env"),
    ]);
    merge_omp_path_dotenv_values(&mut resolved, &paths);

    // Values loaded from the first pass can redirect the roots themselves.
    let config_dir = resolved
        .get("PI_CONFIG_DIR")
        .cloned()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".omp"));
    let config_root = omp_config_root(&home, config_dir);
    let profile = effective_profile_lossy(&resolved);
    let profile_root = profile
        .as_ref()
        .map(|name| config_root.join("profiles").join(name))
        .unwrap_or_else(|| config_root.clone());
    let agent_dir = effective_agent_dir_lossy(&resolved, &profile, &config_root)
        .unwrap_or_else(|| profile_root.join("agent"));
    merge_omp_path_dotenv_values(&mut resolved, &{
        // OMP's named profiles load dotenv from the active profile root
        // only. The base `~/.omp/.env` belongs to the default profile and
        // must not redirect a named profile during this second pass.
        let mut paths = vec![agent_dir.join(".env"), profile_root.join(".env")];
        if profile.is_none() {
            paths.push(config_root.join(".env"));
        }
        paths.push(home.join(".env"));
        paths
    });
    resolved
}

/// Return the profile selected by the process environment, preserving the
/// distinction between an explicitly empty canonical value and a missing
/// value. OMP snapshots these variables before loading dotenv files, so a
/// process-level `PI_PROFILE` must not be overridden by a project `.env`
/// `OMP_PROFILE`.
fn process_profile_override() -> Result<Option<Option<String>>, AppError> {
    if let Some(value) = std::env::var_os("OMP_PROFILE") {
        return Ok(Some(normalize_profile(value.to_string_lossy().as_ref())?));
    }
    if let Some(value) = std::env::var_os("PI_PROFILE") {
        return Ok(Some(normalize_profile(value.to_string_lossy().as_ref())?));
    }
    Ok(None)
}

fn effective_profile(
    _path_env: &HashMap<String, std::ffi::OsString>,
) -> Result<Option<String>, AppError> {
    // OMP resolves the active profile before loading any dotenv files.  A
    // profile mentioned only in ~/.omp/.env (or another dotenv layer) must
    // therefore not redirect CC Switch into a profile that the native `omp`
    // process will never use.  `path_env` still carries dotenv-backed path
    // overrides such as PI_CODING_AGENT_DIR and PI_CONFIG_DIR, but profile
    // selection is intentionally process-environment-only.
    Ok(process_profile_override()?.flatten())
}

fn effective_profile_lossy(path_env: &HashMap<String, std::ffi::OsString>) -> Option<String> {
    effective_profile(path_env).ok().flatten()
}

/// Resolve the agent directory using OMP's source-sensitive precedence. A
/// named process profile derives its own agent directory and suppresses any
/// `PI_CODING_AGENT_DIR`; profiles loaded from dotenv do not, so an explicit
/// process or dotenv agent override remains authoritative.
fn effective_agent_dir_lossy(
    path_env: &HashMap<String, std::ffi::OsString>,
    profile: &Option<String>,
    config_root: &Path,
) -> Option<PathBuf> {
    let process_named_profile = process_profile_override()
        .ok()
        .flatten()
        .flatten()
        .is_some();
    if process_named_profile {
        return None;
    }
    path_env
        .get("PI_CODING_AGENT_DIR")
        .map(resolve_omp_env_agent_path)
        .filter(|value| {
            !is_profile_derived_agent_dir_from_env(config_root, &value.clone().into_os_string())
        })
        .or_else(|| {
            profile
                .as_ref()
                .map(|name| config_root.join("profiles").join(name).join("agent"))
        })
}

fn merge_omp_path_dotenv_values(
    resolved: &mut HashMap<String, std::ffi::OsString>,
    paths: &[PathBuf],
) {
    for path in paths {
        for (key, value) in read_omp_dotenv_file(path) {
            // Preserve an explicitly empty canonical profile in the merged
            // dotenv view for command-backed values, but never use it to
            // select a profile: active profile resolution is process-only.
            // Other empty path settings are ignored so they cannot turn a
            // directory into cwd.
            if value.trim().is_empty() && key != "OMP_PROFILE" {
                continue;
            }
            if resolved.contains_key(&key) {
                continue;
            }
            resolved.insert(key.clone(), std::ffi::OsString::from(&value));
            if key == "OMP_PROFILE" {
                resolved
                    .entry("PI_PROFILE".to_string())
                    .or_insert_with(|| std::ffi::OsString::from(&value));
            }
        }
    }
}

pub(crate) fn get_omp_agent_dir() -> Result<PathBuf, AppError> {
    let home = get_home_dir();
    let path_env = resolve_omp_path_environment();
    let config_dir = path_env
        .get("PI_CONFIG_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".omp"));
    // OMP treats `PI_CONFIG_DIR` as a directory name relative to the user's
    // home (for example `.omp`), including when a caller supplies a leading
    // separator. Keep that upstream join/normalization behavior; the value is
    // an explicit user-controlled path and is not implicitly constrained to
    // remain below `home` when it contains `..` components.
    let config_root = omp_config_root(&home, config_dir);
    let profile = effective_profile(&path_env)?;
    let default_path = profile
        .as_ref()
        .map(|profile| config_root.join("profiles").join(profile).join("agent"))
        .unwrap_or_else(|| config_root.join("agent"));
    #[cfg(test)]
    if let Some(path) = TEST_AGENT_DIR
        .lock()
        .expect("lock OMP test directory")
        .clone()
    {
        return resolve_omp_agent_dir(Some(path), None, default_path);
    }

    let env_override = if process_profile_override()?.flatten().is_none() {
        path_env
            .get("PI_CODING_AGENT_DIR")
            .cloned()
            .filter(|value| {
                // When a named profile was active, OMP may have exported its
                // derived agent path through PI_CODING_AGENT_DIR. If the
                // caller switches back to the default profile while that
                // inherited value remains, ignore it rather than reopening
                // the previous profile's directory. Explicit, unrelated
                // overrides continue to work in default mode.
                !is_profile_derived_agent_dir_from_env(&config_root, value)
            })
    } else {
        None
    };

    // `settings.omp_config_dir` is a CC-Switch-only legacy setting. OMP does
    // not inherit it when launched from the user's shell, so using it here
    // would make a successful write invisible to the real runtime. Only the
    // official OMP selectors are authoritative for native file paths.
    resolve_omp_agent_dir(None, env_override, default_path)
}

/// Resolve the native OMP session directory. OMP stores session data in the
/// XDG data root after `omp config migrate` (for example
/// `$XDG_DATA_HOME/omp/sessions`) while models/config remain under the agent
/// directory. Explicit `PI_CODING_AGENT_DIR` overrides continue to use their
/// own `sessions` child, matching OMP's `agentSubdir` semantics.
pub(crate) fn get_omp_sessions_dir() -> Result<PathBuf, AppError> {
    let agent_dir = get_omp_agent_dir()?;
    let path_env = resolve_omp_path_environment();
    let config_dir = path_env
        .get("PI_CONFIG_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".omp"));
    let config_root = omp_config_root(&get_home_dir(), config_dir);
    let profile = effective_profile(&path_env)?;
    let default_agent = profile
        .as_ref()
        .map(|name| config_root.join("profiles").join(name).join("agent"))
        .unwrap_or_else(|| config_root.join("agent"));
    let explicit_agent_override = path_env
        .get("PI_CODING_AGENT_DIR")
        .filter(|value| !value.is_empty())
        .is_some_and(|value| !is_profile_derived_agent_dir_from_env(&config_root, value));

    // OMP only enables XDG data relocation on Unix platforms. In particular,
    // a Windows process may still inherit XDG_DATA_HOME from a CI environment
    // or shell profile, but native OMP continues using its agent directory.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if !explicit_agent_override && agent_dir == default_agent {
        if let Some(xdg_data) = path_env
            .get("XDG_DATA_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
        {
            let app_root = xdg_data.join("omp");
            let migrated_root = profile
                .as_ref()
                .map(|name| app_root.join("profiles").join(name))
                .unwrap_or(app_root);
            let migrated_sessions = migrated_root.join("sessions");
            // OMP's directory resolver selects the XDG app root based on the
            // migrated root itself, not on whether `sessions/` has already
            // been created. Returning the path even when the directory is
            // currently absent avoids falling back to stale legacy sessions.
            if migrated_root.is_dir() {
                return Ok(migrated_sessions);
            }
        }
    }
    Ok(agent_dir.join("sessions"))
}

/// Resolve OMP's shared user-config directory used by the cross-agent prompt
/// discovery layer. Unlike native models/MCP files, shared prompt lookup uses
/// the configured OMP base/profile and intentionally ignores an arbitrary
/// `PI_CODING_AGENT_DIR` override.
pub(crate) fn get_omp_shared_config_agent_dir() -> Result<PathBuf, AppError> {
    #[cfg(test)]
    if let Some(path) = TEST_AGENT_DIR
        .lock()
        .expect("lock OMP test directory")
        .clone()
    {
        return Ok(path);
    }

    let home = get_home_dir();
    let path_env = resolve_omp_path_environment();
    let config_dir = path_env
        .get("PI_CONFIG_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".omp"));
    let config_root = omp_config_root(&home, config_dir);
    let profile = effective_profile(&path_env)?;
    Ok(profile
        .map(|profile| config_root.join("profiles").join(profile).join("agent"))
        .unwrap_or_else(|| config_root.join("agent")))
}

fn is_profile_derived_agent_dir_from_env(config_root: &Path, value: &std::ffi::OsString) -> bool {
    let candidate = resolve_omp_env_agent_path(value);
    ["OMP_PROFILE", "PI_PROFILE"].iter().any(|key| {
        std::env::var_os(key)
            .and_then(|raw| {
                normalize_profile(raw.to_string_lossy().as_ref())
                    .ok()
                    .flatten()
            })
            .is_some_and(|profile| {
                candidate == config_root.join("profiles").join(profile).join("agent")
            })
    })
}

fn omp_config_root(home: &Path, config_dir: PathBuf) -> PathBuf {
    // OMP treats PI_CONFIG_DIR as a directory *name* under the user's home
    // (its upstream implementation uses `path.join(os.homedir(), value)`).
    // Strip a leading separator so an accidentally absolute-looking value is
    // still interpreted as home-relative, matching OMP's path.join call.
    let config_dir = config_dir.to_string_lossy();
    normalize_omp_path(home.join(config_dir.trim_start_matches(['/', '\\'])))
}

/// Resolve an OMP `PI_CODING_AGENT_DIR` value the same way its Node runtime
/// does: absolute values are kept, while relative values are rooted at the
/// current working directory. Unlike CC-Switch's user-facing settings,
/// OMP does not expand a leading `~` in this variable.
fn resolve_omp_env_agent_path(value: &std::ffi::OsString) -> PathBuf {
    let raw = PathBuf::from(value);
    let path = if raw.is_absolute() {
        raw
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(raw)
    };
    normalize_omp_path(path)
}

/// Lexically normalize a path without requiring the target to exist. OMP's
/// `path.join` removes `.` and `..`; Rust's `PathBuf::join` intentionally does
/// not, so normalizing here keeps the adapter and the official resolver
/// comparable even for not-yet-created profile directories.
fn normalize_omp_path(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(std::path::MAIN_SEPARATOR.to_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Normal(value) => normalized.push(value),
        }
    }
    normalized
}

fn normalize_profile(raw: &str) -> Result<Option<String>, AppError> {
    let profile = raw.trim();
    // OMP reserves only the lowercase `default` profile name. Other casing
    // is rejected by its lowercase profile-name grammar rather than treated
    // as the default profile.
    if profile.is_empty() || profile == "default" {
        return Ok(None);
    }
    let valid = profile.len() <= 64
        && profile.chars().enumerate().all(|(index, ch)| {
            ch.is_ascii_lowercase()
                || ch.is_ascii_digit()
                || (index > 0 && matches!(ch, '.' | '_' | '-'))
        });
    if !valid
        || profile == "."
        || profile == ".."
        || profile.ends_with('.')
        || is_windows_reserved_profile_name(profile)
    {
        return Err(AppError::InvalidInput(format!(
            "Invalid OMP profile '{raw}'. Use 1-64 lowercase letters, digits, '.', '_' or '-'; Windows reserved device names are not allowed."
        )));
    }
    Ok(Some(profile.to_string()))
}

/// Windows reserves device names such as `CON` and `COM1` as path aliases,
/// including when followed by an extension (`con.foo`).  OMP rejects these
/// names up front so profile directories behave consistently across hosts.
fn is_windows_reserved_profile_name(profile: &str) -> bool {
    let basename = profile.split('.').next().unwrap_or(profile);
    matches!(basename, "con" | "prn" | "aux" | "nul")
        || (basename.len() == 4
            && (basename.starts_with("com") || basename.starts_with("lpt"))
            && basename.as_bytes()[3].is_ascii_digit())
}

fn resolve_omp_agent_dir(
    settings_override: Option<PathBuf>,
    env_override: Option<std::ffi::OsString>,
    default_path: PathBuf,
) -> Result<PathBuf, AppError> {
    // Match OMP's own precedence: an explicit PI_CODING_AGENT_DIR controls
    // the process, while the CC-Switch setting is only a fallback for native
    // files when no official environment override is present. This prevents
    // the adapter from silently editing a different directory than `omp`.
    let (path, source) = match env_override {
        Some(value) if !value.is_empty() => {
            (resolve_omp_env_agent_path(&value), "PI_CODING_AGENT_DIR")
        }
        _ => match settings_override {
            Some(path) => (normalize_omp_path(path), "OMP settings override"),
            None => (default_path, "OMP default"),
        },
    };
    if !path.is_absolute() {
        return Err(AppError::InvalidInput(format!(
            "{source} must resolve to an absolute directory: {}",
            path.display()
        )));
    }
    Ok(path)
}

pub(crate) fn get_omp_models_path() -> Result<PathBuf, AppError> {
    let dir = get_omp_agent_dir()?;
    let canonical = dir.join("models.yml");
    if canonical.exists() {
        return Ok(canonical);
    }
    let fallback = dir.join("models.yaml");
    if fallback.exists() {
        return Ok(fallback);
    }
    let legacy = dir.join("models.json");
    if legacy.exists() {
        let bytes = read_file_limited(&legacy, "OMP legacy models")?;
        let source_revision = revision(&bytes);
        // OMP's legacy registry is JSONC (JSON5), not YAML.  Parse it with the
        // same relaxed grammar used by the native migration (comments,
        // trailing commas, and unquoted keys are all accepted), then emit a
        // canonical YAML document at the path OMP reads going forward.
        // Serializing the parsed value also prevents carrying JSONC syntax
        // into `models.yml`, where the YAML parser would reject it later.
        let source = String::from_utf8(bytes).map_err(|error| {
            AppError::Config(format!(
                "OMP legacy models file must be UTF-8 ({}): {error}",
                legacy.display()
            ))
        })?;
        let document: Value = json5::from_str(&source).map_err(|error| {
            AppError::Config(format!(
                "OMP legacy models file is not valid JSON/JSONC ({}): {error}",
                legacy.display()
            ))
        })?;
        let yaml = serde_yaml::to_string(&document).map_err(|error| {
            AppError::Config(format!(
                "failed to serialize migrated OMP models ({}): {error}",
                legacy.display()
            ))
        })?;
        validate_omp_models_root(&document, &canonical)?;
        // Legacy migration writes the first sensitive native file directly
        // from this path resolver, so apply the same directory safety checks
        // used by every later models/config mutation before creating it.
        ensure_private_omp_parent(&canonical)?;
        // The legacy source remains authoritative until OMP (or this adapter)
        // materializes `models.yml`. Re-check it after parsing/serializing so
        // an external edit cannot be silently shadowed by stale YAML.
        ensure_omp_legacy_models_revision(&legacy, &source_revision)?;
        // Another process may have created the canonical file while this
        // migration was in progress. Never overwrite that newer native file.
        if canonical.exists() {
            return Ok(canonical);
        }
        atomic_write_private(&canonical, yaml.as_bytes())?;
        return Ok(canonical);
    }
    Ok(canonical)
}

pub(crate) fn get_omp_settings_path() -> Result<PathBuf, AppError> {
    let dir = get_omp_agent_dir()?;
    let canonical = dir.join("config.yml");
    if canonical.exists() {
        return Ok(canonical);
    }
    let fallback = dir.join("config.yaml");
    if fallback.exists() {
        return Ok(fallback);
    }
    Ok(canonical)
}

/// Legacy global OMP settings were stored as JSONC in `settings.json`.
/// OMP only consults this file when no YAML config exists, then migrates it to
/// `config.yml`; the adapter keeps the same read precedence without mutating
/// the user's files merely by inspecting them.
fn get_omp_legacy_settings_path() -> Result<PathBuf, AppError> {
    Ok(get_omp_agent_dir()?.join("settings.json"))
}

/// Project settings retain the historical JSONC filename as a lower-priority
/// layer. OMP loads `.omp/settings.json` first and then lets `.omp/config.yml`
/// override matching keys.
fn get_omp_project_legacy_settings_path() -> Result<PathBuf, AppError> {
    let cwd = std::env::current_dir().map_err(|error| {
        AppError::Config(format!("failed to resolve current directory: {error}"))
    })?;
    Ok(cwd.join(".omp").join("settings.json"))
}

/// OMP keeps project settings in the working directory rather than beneath
/// the user agent directory.  Unlike the global agent config, the upstream
/// loader only reads `.omp/config.yml`; a project-level `config.yaml` is not a
/// supported fallback and must therefore remain invisible to this adapter.
pub(crate) fn get_omp_project_settings_path() -> Result<PathBuf, AppError> {
    let cwd = std::env::current_dir().map_err(|error| {
        AppError::Config(format!("failed to resolve current directory: {error}"))
    })?;
    let root = cwd.join(".omp");
    Ok(root.join("config.yml"))
}

fn merge_omp_documents(base: &mut Value, overlay: &Value) {
    let Some(base_object) = base.as_object_mut() else {
        *base = overlay.clone();
        return;
    };
    let Some(overlay_object) = overlay.as_object() else {
        *base = overlay.clone();
        return;
    };
    for (key, value) in overlay_object {
        match (base_object.get_mut(key), value) {
            (Some(existing), Value::Object(_)) if existing.is_object() => {
                merge_omp_documents(existing, value)
            }
            _ => {
                base_object.insert(key.clone(), value.clone());
            }
        }
    }
}

fn parse_json5_document(path: &Path, label: &str) -> Result<(Value, String), AppError> {
    if !path.exists() {
        return Ok((
            Value::Object(Map::new()),
            MISSING_CONFIG_REVISION.to_string(),
        ));
    }
    let bytes = read_file_limited(path, label)?;
    let revision = revision(&bytes);
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok((Value::Object(Map::new()), revision));
    }
    let source = String::from_utf8(bytes).map_err(|error| {
        AppError::Config(format!(
            "{label} file must be UTF-8 ({}): {error}",
            path.display()
        ))
    })?;
    let document = json5::from_str::<Value>(&source).map_err(|error| {
        AppError::Config(format!(
            "{label} file is not valid JSON/JSONC ({}): {error}",
            path.display()
        ))
    })?;
    if !document.is_object() {
        return Err(AppError::Config(format!(
            "{label} root must be an object: {}",
            path.display()
        )));
    }
    Ok((document, revision))
}

/// Read the effective global settings layer. A legacy `settings.json` is
/// consulted only when neither `config.yml` nor `config.yaml` exists, matching
/// OMP's one-time migration behavior. The returned path is always the YAML
/// write target, even when the source currently is legacy JSONC.
fn read_omp_global_settings_document() -> Result<(Value, PathBuf), AppError> {
    let target = get_omp_settings_path()?;
    if target.exists() {
        return read_config_document(&target).map(|document| (document, target));
    }
    let legacy = get_omp_legacy_settings_path()?;
    if legacy.exists() {
        return parse_json5_document(&legacy, "OMP legacy settings")
            .map(|(document, _)| (document, target));
    }
    Ok((Value::Object(Map::new()), target))
}

/// Read the project settings JSONC layer followed by the native YAML overlay.
/// `None` means neither project file exists. The second tuple member is the
/// canonical project YAML path used for writes.
fn read_omp_project_settings_layer() -> Result<Option<(Value, PathBuf)>, AppError> {
    let project_yaml = get_omp_project_settings_path()?;
    let project_json = get_omp_project_legacy_settings_path()?;
    let mut merged = Value::Object(Map::new());
    let mut present = false;
    if project_json.exists() {
        let (document, _) = parse_json5_document(&project_json, "OMP project legacy settings")?;
        merge_omp_documents(&mut merged, &document);
        present = true;
    }
    if project_yaml.exists() {
        let document = read_config_document(&project_yaml)?;
        merge_omp_documents(&mut merged, &document);
        present = true;
    }
    Ok(present.then_some((merged, project_yaml)))
}

/// A model entry as it appears in OMP's native `models.yml` registry.
///
/// The complete model object is retained so callers can display or edit
/// forward-compatible OMP fields without reducing them to CC Switch's older
/// provider model shape.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OMPNativeModel {
    pub provider_id: String,
    pub model_id: String,
    pub config: Value,
}

/// Read one config document's `modelRoles` map.
fn model_role_entries_from_document(
    document: &Value,
    path: &Path,
) -> Result<IndexMap<String, Option<String>>, AppError> {
    let Some(value) = document.get("modelRoles") else {
        return Ok(IndexMap::new());
    };
    let roles = value.as_object().ok_or_else(|| {
        AppError::Config(format!(
            "OMP config 'modelRoles' must be an object: {}",
            path.display()
        ))
    })?;
    let mut result = IndexMap::new();
    for (role, value) in roles {
        validate_model_role_name(role)?;
        match value {
            Value::Null => {
                result.insert(role.clone(), None);
            }
            Value::String(selector) if !selector.trim().is_empty() => {
                validate_model_selector(selector)?;
                result.insert(role.clone(), Some(selector.clone()));
            }
            Value::String(_) => {}
            _ => {
                return Err(AppError::Config(format!(
                    "OMP config modelRoles.{role} must be a string or null: {}",
                    path.display()
                )))
            }
        }
    }
    Ok(result)
}

fn model_roles_from_document(
    document: &Value,
    path: &Path,
) -> Result<IndexMap<String, String>, AppError> {
    Ok(model_role_entries_from_document(document, path)?
        .into_iter()
        .filter_map(|(role, selector)| selector.map(|selector| (role, selector)))
        .collect())
}

fn omp_model_roles_use_project_storage() -> Result<bool, AppError> {
    let (global, global_path) = read_omp_global_settings_document()?;
    let mut storage = match global.get("modelRoleStorage") {
        None => "global".to_string(),
        Some(value) => value
            .as_str()
            .ok_or_else(|| {
                AppError::Config(format!(
                    "OMP config modelRoleStorage must be 'global' or 'project': {}",
                    global_path.display()
                ))
            })?
            .trim()
            .to_ascii_lowercase(),
    };

    if let Some((project, project_path)) = read_omp_project_settings_layer()? {
        if let Some(value) = project.get("modelRoleStorage") {
            storage = value
                .as_str()
                .ok_or_else(|| {
                    AppError::Config(format!(
                        "OMP config modelRoleStorage must be 'global' or 'project': {}",
                        project_path.display()
                    ))
                })?
                .trim()
                .to_ascii_lowercase();
        }
    }

    match storage.as_str() {
        "global" | "" => Ok(false),
        "project" => Ok(true),
        _ => Err(AppError::Config(format!(
            "OMP config modelRoleStorage must be 'global' or 'project', got '{storage}'"
        ))),
    }
}

fn omp_model_roles_target_path() -> Result<PathBuf, AppError> {
    if omp_model_roles_use_project_storage()? {
        get_omp_project_settings_path()
    } else {
        get_omp_settings_path()
    }
}

/// Read OMP's effective `modelRoles` map and the file that owns role writes.
/// In project storage mode, project assignments overlay global assignments and
/// missing project roles continue to fall back to the global file. When the
/// target YAML has not been created yet, the returned revision is taken from
/// the legacy JSONC source that will be migrated by the next write.
pub(crate) fn read_omp_model_roles_with_metadata(
) -> Result<(IndexMap<String, String>, PathBuf, String), AppError> {
    let _guard = lock_models_file()?;
    read_omp_model_roles_with_metadata_locked()
}

fn read_omp_model_roles_with_metadata_locked(
) -> Result<(IndexMap<String, String>, PathBuf, String), AppError> {
    let (global, global_path) = read_omp_global_settings_document()?;
    let mut roles = model_roles_from_document(&global, &global_path)?;
    let target_path = if omp_model_roles_use_project_storage()? {
        let project_path = get_omp_project_settings_path()?;
        if let Some((project, _)) = read_omp_project_settings_layer()? {
            for (role, selector) in model_role_entries_from_document(&project, &project_path)? {
                if let Some(selector) = selector {
                    roles.insert(role, selector);
                }
            }
        }
        project_path
    } else {
        global_path
    };
    let (_, revision) = read_omp_config_yaml_at(&target_path)?;
    Ok((roles, target_path, revision))
}

/// Read OMP's effective `modelRoles` map from global/project config.
pub(crate) fn read_omp_model_roles() -> Result<IndexMap<String, String>, AppError> {
    read_omp_model_roles_with_metadata().map(|(roles, _, _)| roles)
}

fn effective_omp_settings_document() -> Result<(Value, PathBuf), AppError> {
    let (mut effective, global_path) = read_omp_global_settings_document()?;
    if let Some((project, project_path)) = read_omp_project_settings_layer()? {
        let project_has_disabled = project.get("disabledProviders").is_some();
        merge_omp_documents(&mut effective, &project);
        if project_has_disabled {
            return Ok((effective, project_path));
        }
    }
    Ok((effective, global_path))
}

fn disabled_provider_values(
    value: Option<&Value>,
    path: &Path,
) -> Result<HashSet<String>, AppError> {
    let Some(value) = value else {
        return Ok(HashSet::new());
    };
    let entries = value.as_array().ok_or_else(|| {
        AppError::Config(format!(
            "OMP config disabledProviders must be an array: {}",
            path.display()
        ))
    })?;
    let cwd = std::env::current_dir().map_err(|error| {
        AppError::Config(format!("failed to resolve current directory: {error}"))
    })?;
    let mut result = HashSet::new();
    for entry in entries {
        match entry {
            Value::String(id) if !id.trim().is_empty() => {
                result.insert(id.trim().to_string());
            }
            Value::Object(object) => {
                if !disabled_provider_entry_applies_to_cwd(object, &cwd) {
                    continue;
                }
                let values = object
                    .get("providers")
                    .or_else(|| object.get("values"))
                    .or_else(|| object.get("items"));
                if let Some(values) = values {
                    result.extend(
                        string_or_array_values(values)
                            .into_iter()
                            .map(str::trim)
                            .filter(|id| !id.is_empty())
                            .map(str::to_string),
                    );
                }
            }
            _ => {}
        }
    }
    Ok(result)
}

fn disabled_provider_entry_applies_to_cwd(object: &Map<String, Value>, cwd: &Path) -> bool {
    let paths = ["path", "pathPrefix", "paths", "pathPrefixes"]
        .into_iter()
        .filter_map(|key| object.get(key))
        .flat_map(string_or_array_values)
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    paths.is_empty() || paths.iter().any(|path| path_matches_cwd(path, cwd))
}

/// OMP accepts either one string or an array of strings for scoped
/// `disabledProviders` fields. Invalid values are ignored while reading so a
/// malformed extension entry cannot hide otherwise valid provider settings;
/// writes use `validate_string_or_array_field` to reject them explicitly.
fn string_or_array_values(value: &Value) -> Vec<&str> {
    match value {
        Value::String(value) => vec![value.as_str()],
        Value::Array(values) => values.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

fn validate_string_or_array_field(value: &Value, context: &str) -> Result<(), AppError> {
    match value {
        Value::String(_) => Ok(()),
        Value::Array(values) if values.iter().all(Value::is_string) => Ok(()),
        Value::Array(_) => Err(AppError::InvalidInput(format!(
            "{context} values must be strings"
        ))),
        _ => Err(AppError::InvalidInput(format!(
            "{context} must be a string or array"
        ))),
    }
}

fn path_matches_cwd(raw: &Path, cwd: &Path) -> bool {
    let raw = raw.to_string_lossy();
    let expanded = if raw == "~" {
        get_home_dir()
    } else if let Some(rest) = raw.strip_prefix("~/") {
        get_home_dir().join(rest)
    } else if raw.starts_with('/') {
        raw.into_owned().into()
    } else {
        cwd.join(raw.as_ref())
    };
    let expanded = normalize_omp_path(expanded);
    cwd == expanded || cwd.starts_with(&expanded)
}

/// Return the effective OMP provider ids disabled for the current directory.
/// A project `disabledProviders` array replaces the global array, matching
/// OMP's settings merge semantics.
pub(crate) fn read_omp_disabled_providers() -> Result<HashSet<String>, AppError> {
    let _guard = lock_models_file()?;
    read_omp_disabled_providers_locked()
}

/// Read the effective disabled provider set while the caller already owns the
/// native OMP file lock. Keeping this helper lock-free avoids recursively
/// acquiring the non-reentrant process mutex in compound mutations.
fn read_omp_disabled_providers_locked() -> Result<HashSet<String>, AppError> {
    let (document, path) = effective_omp_settings_document()?;
    disabled_provider_values(document.get("disabledProviders"), &path)
}

/// Add or remove a provider id from the active disabledProviders array while
/// preserving path-scoped entries and unrelated settings.
pub(crate) fn set_omp_provider_disabled(provider_id: &str, disabled: bool) -> Result<(), AppError> {
    validate_provider_key(provider_id)?;
    let _guard = lock_models_file()?;
    // Enabling must fail closed when native settings cannot be parsed.  A
    // successful return while `disabledProviders` remains unreadable or
    // unchanged is indistinguishable from a working provider to callers, but
    // OMP would still ignore it (or reject the whole config).
    let (global_document, global_path) = read_omp_global_settings_document()?;
    let project_path = get_omp_project_settings_path()?;
    let project_layer = read_omp_project_settings_layer()?;
    let project_has_disabled = project_layer
        .as_ref()
        .is_some_and(|(document, _)| document.get("disabledProviders").is_some());
    let path = if project_has_disabled {
        project_path.clone()
    } else {
        global_path.clone()
    };

    // OMP's legacy global JSON is a read-only source once YAML exists. When
    // editing for the first time, seed the new YAML document from that source
    // so unknown settings are not discarded. Likewise, a project
    // `settings.json` remains the lower layer until we create an overriding
    // `.omp/config.yml`; carry its disabledProviders array into that override.
    let mut legacy_source_revision = None;
    let (mut document, revision) = if path == global_path {
        if !path.exists() {
            let legacy = get_omp_legacy_settings_path()?;
            if legacy.exists() {
                let (_, source_revision) = parse_json5_document(&legacy, "OMP legacy settings")?;
                legacy_source_revision = Some((legacy, source_revision));
            }
        }
        let (_, revision) = read_config_document_with_revision(&path)?;
        (global_document, revision)
    } else if path.exists() {
        let (mut document, revision) = read_config_document_with_revision(&path)?;
        if document.get("disabledProviders").is_none() && project_has_disabled {
            if let Some((project, _)) = &project_layer {
                if let Some(value) = project.get("disabledProviders") {
                    document
                        .as_object_mut()
                        .expect("validated OMP project config root")
                        .insert("disabledProviders".to_string(), value.clone());
                }
            }
        }
        (document, revision)
    } else if project_has_disabled {
        let (project, _) =
            project_layer.expect("project layer exists when disabledProviders is set");
        let project_legacy = get_omp_project_legacy_settings_path()?;
        if project_legacy.exists() {
            let (_, source_revision) =
                parse_json5_document(&project_legacy, "OMP project legacy settings")?;
            legacy_source_revision = Some((project_legacy, source_revision));
        }
        let (_, revision) = read_config_document_with_revision(&path)?;
        (project, revision)
    } else {
        let (_, revision) = read_config_document_with_revision(&path)?;
        (Value::Object(Map::new()), revision)
    };

    let root = document.as_object_mut().ok_or_else(|| {
        AppError::Config(format!(
            "OMP config root must be an object: {}",
            path.display()
        ))
    })?;
    let entries = match root.get_mut("disabledProviders") {
        Some(value) => value.as_array_mut().ok_or_else(|| {
            AppError::Config(format!(
                "OMP config disabledProviders must be an array: {}",
                path.display()
            ))
        })?,
        None if !disabled => return Ok(()),
        None => root
            .entry("disabledProviders".to_string())
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .expect("inserted disabledProviders array"),
    };
    if disabled {
        if !entries
            .iter()
            .any(|entry| entry.as_str().is_some_and(|id| id.trim() == provider_id))
        {
            entries.push(Value::String(provider_id.to_string()));
        }
    } else {
        let cwd = std::env::current_dir().map_err(|error| {
            AppError::Config(format!("failed to resolve current directory: {error}"))
        })?;
        entries.retain_mut(|entry| {
            if entry.as_str().is_some_and(|id| id.trim() == provider_id) {
                return false;
            }
            let Some(object) = entry.as_object_mut() else {
                return true;
            };
            // A path-scoped object is a complete policy for its own paths.
            // Enabling a provider in the current project must not silently
            // remove the same provider from an unrelated project scope.
            if !disabled_provider_entry_applies_to_cwd(object, &cwd) {
                return true;
            }
            for key in ["providers", "values", "items"] {
                let remove_key = match object.get_mut(key) {
                    Some(Value::String(value)) => value.trim() == provider_id,
                    Some(Value::Array(values)) => {
                        values.retain(|value| {
                            value.as_str().is_none_or(|id| id.trim() != provider_id)
                        });
                        values.is_empty()
                    }
                    _ => false,
                };
                if remove_key {
                    object.remove(key);
                }
            }
            let has_provider_values = ["providers", "values", "items"].iter().any(|key| {
                object
                    .get(*key)
                    .and_then(Value::as_array)
                    .is_some_and(|values| !values.is_empty())
            });
            let is_scope_only = object.keys().all(|key| {
                matches!(
                    key.as_str(),
                    "path" | "pathPrefix" | "paths" | "pathPrefixes"
                )
            });
            if !has_provider_values && is_scope_only {
                return false;
            }
            true
        });
        // If project settings.json supplied the disabledProviders layer,
        // retaining an explicit empty array is necessary to override it.
        // Removing the key would make OMP fall back to the JSON list again.
        let project_json_has_disabled = if project_has_disabled {
            let project_json = get_omp_project_legacy_settings_path()?;
            project_json.exists()
                && parse_json5_document(&project_json, "OMP project legacy settings")?
                    .0
                    .get("disabledProviders")
                    .is_some()
        } else {
            false
        };
        if entries.is_empty() && !project_json_has_disabled {
            root.remove("disabledProviders");
        }
    }
    if let Some((source_path, source_revision)) = legacy_source_revision {
        ensure_omp_legacy_revision(&source_path, &source_revision)?;
    }
    write_config_document(&path, &document, &revision)
}

/// Set or remove one OMP model-role assignment while preserving every other
/// key in `config.yml`, including keys added by newer OMP releases.
pub(crate) fn set_omp_model_role(
    role: &str,
    selector: Option<&str>,
    expected_revision: Option<&str>,
) -> Result<(), AppError> {
    validate_model_role_name(role)?;
    if let Some(selector) = selector {
        validate_model_selector(selector)?;
    }

    let _guard = lock_models_file()?;
    set_omp_model_role_locked(role, selector, expected_revision)
}

fn set_omp_model_role_locked(
    role: &str,
    selector: Option<&str>,
    expected_revision: Option<&str>,
) -> Result<(), AppError> {
    let path = omp_model_roles_target_path()?;
    let mut legacy_source_revision = None;
    let (mut document, revision) = if path == get_omp_settings_path()? {
        if !path.exists() {
            let legacy = get_omp_legacy_settings_path()?;
            if legacy.exists() {
                let (_, source_revision) = parse_json5_document(&legacy, "OMP legacy settings")?;
                legacy_source_revision = Some((legacy, source_revision));
            }
        }
        let (document, _) = read_omp_global_settings_document()?;
        let (_, revision) = read_config_document_with_revision(&path)?;
        (document, revision)
    } else if path.exists() {
        read_config_document_with_revision(&path)?
    } else if let Some((project, _)) = read_omp_project_settings_layer()? {
        let project_legacy = get_omp_project_legacy_settings_path()?;
        if !path.exists() && project_legacy.exists() {
            let (_, source_revision) =
                parse_json5_document(&project_legacy, "OMP project legacy settings")?;
            legacy_source_revision = Some((project_legacy, source_revision));
        }
        let (_, revision) = read_config_document_with_revision(&path)?;
        (project, revision)
    } else {
        read_config_document_with_revision(&path)?
    };
    if let Some(expected) = expected_revision {
        let compare_revision = legacy_source_revision
            .as_ref()
            .map(|(_, source_revision)| source_revision.as_str())
            .unwrap_or(revision.as_str());
        if expected != compare_revision {
            return Err(AppError::Conflict(format!(
                "OMP config.yml changed outside CC Switch: {}",
                path.display()
            )));
        }
    }
    let root = document.as_object_mut().ok_or_else(|| {
        AppError::Config(format!(
            "OMP config root must be an object: {}",
            path.display()
        ))
    })?;
    let roles = root
        .entry("modelRoles".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let roles = roles.as_object_mut().ok_or_else(|| {
        AppError::Config(format!(
            "OMP config 'modelRoles' must be an object: {}",
            path.display()
        ))
    })?;
    match selector {
        Some(selector) => {
            roles.insert(
                role.trim().to_string(),
                Value::String(selector.trim().to_string()),
            );
        }
        None => {
            if path == get_omp_project_settings_path()? {
                // Project role storage is an overlay. OMP treats a project
                // null as "clear this project assignment" and then falls
                // back to the global role. Refuse to report success when the
                // role is inherited from global only, because there is no
                // project value for this operation to remove.
                let project_entries = read_omp_project_settings_layer()?
                    .map(|(project, project_path)| {
                        model_role_entries_from_document(&project, &project_path)
                    })
                    .transpose()?
                    .unwrap_or_default();
                let global_entries = model_role_entries_from_document(
                    &read_omp_global_settings_document()?.0,
                    &get_omp_settings_path()?,
                )?;
                if !project_entries.contains_key(role.trim()) {
                    if global_entries.get(role.trim()).is_some_and(Option::is_some) {
                        return Err(AppError::InvalidInput(format!(
                            "OMP role '{}' is inherited from global config; switch modelRoleStorage to global or edit the global config to remove it",
                            role.trim()
                        )));
                    }
                    return Ok(());
                }
                // Persist null rather than deleting the key so a lower-priority
                // project settings.json value is also cleared by OMP's merge.
                roles.insert(role.trim().to_string(), Value::Null);
            } else {
                roles.remove(role.trim());
                if roles.is_empty() {
                    root.remove("modelRoles");
                }
            }
        }
    }
    if let Some((source_path, source_revision)) = legacy_source_revision {
        ensure_omp_legacy_revision(&source_path, &source_revision)?;
    }
    write_config_document(&path, &document, &revision)
}

/// Atomically validate and assign OMP's default model.  Provider/model
/// membership and the role write share the native-file lock, so another
/// CC-Switch operation cannot remove the selected model between validation and
/// assignment.  The models revision is also rechecked immediately before the
/// config write to detect edits made by external processes.
pub(crate) fn set_omp_default_model(
    provider_id: &str,
    model_id: Option<&str>,
) -> Result<String, AppError> {
    validate_provider_key(provider_id)?;
    let _guard = lock_models_file()?;
    if read_omp_disabled_providers_locked()?.contains(provider_id) {
        return Err(AppError::InvalidInput(format!(
            "OMP provider '{provider_id}' is disabled by config.yml disabledProviders"
        )));
    }

    let models_path = get_omp_models_path()?;
    let (models_document, models_revision) = read_models_document_with_revision(&models_path)?;
    let provider = providers(&models_document, &models_path)?
        .get(provider_id)
        .ok_or_else(|| {
            AppError::InvalidInput(format!(
                "OMP provider '{provider_id}' is not enabled in models.yml"
            ))
        })?;
    // Selecting a default model must obey the same provider semantics as a
    // CC-Switch-created entry. Membership alone is insufficient: an opaque or
    // malformed provider could otherwise be assigned to OMP's default role and
    // make every invocation fail at runtime.
    // Native transports may use non-HTTP base URLs (for example unix://), so
    // apply semantic validation without imposing CC Switch's HTTP-only probe
    // restriction.
    validate_provider_node_for_editor(provider_id, provider)?;
    let models = provider.get("models").and_then(Value::as_array);
    let discovery_only = provider.get("discovery").is_some();
    let selected = match model_id.map(str::trim) {
        Some(model_id) if model_id.is_empty() => {
            return Err(AppError::InvalidInput(
                "OMP model id must be a non-empty string".to_string(),
            ));
        }
        Some(model_id) => {
            // Discovery-backed providers may intentionally omit a static
            // `models` catalog. OMP resolves those model ids at runtime, so
            // an explicit selector must be accepted even when the registry
            // has not been populated yet. Static catalogs remain strict so a
            // typo cannot silently create a dangling role.
            if let Some(models) = models.filter(|models| !models.is_empty()) {
                if !models.iter().any(|model| {
                    model.get("id").and_then(Value::as_str).map(str::trim) == Some(model_id)
                }) {
                    return Err(AppError::InvalidInput(format!(
                        "OMP model '{provider_id}/{model_id}' is not present in models.yml"
                    )));
                }
            } else if !discovery_only {
                return Err(AppError::InvalidInput(format!(
                    "OMP provider '{provider_id}' has no model catalog; specify a discovery provider"
                )));
            }
            model_id.to_string()
        }
        None => {
            let Some(models) = models else {
                return Err(AppError::InvalidInput(format!(
                    "OMP provider '{provider_id}' has no static model catalog; specify --model for a discovery provider"
                )));
            };
            models
                .iter()
                .find_map(|model| model.get("id").and_then(Value::as_str).map(str::trim))
                .filter(|id| !id.is_empty())
                .ok_or_else(|| {
                    AppError::InvalidInput(format!("OMP provider '{provider_id}' has no models"))
                })?
                .to_string()
        }
    };

    // External OMP edits are not coordinated by our mutex.  Refuse to write a
    // role if the registry changed during validation rather than reporting a
    // successful assignment to a model that may no longer exist.
    let (_, actual_revision) = read_models_document_with_revision(&models_path)?;
    if actual_revision != models_revision {
        return Err(AppError::Conflict(format!(
            "OMP models.yml changed outside CC Switch: {}",
            models_path.display()
        )));
    }
    let selector = format!("{provider_id}/{selected}");
    set_omp_model_role_locked("default", Some(&selector), None)?;
    Ok(selector)
}

/// Return the native config file bytes' revision for TUI compare-and-swap
/// editing. The canonical path is returned even when the file does not exist.
/// Read the exact YAML text OMP will consume, together with its content
/// revision (or the legacy JSONC source revision before migration). The TUI
/// uses this for an advanced editor so comments and fields unknown to CC
/// Switch survive a round trip unchanged unless the user explicitly edits
/// them.
pub(crate) fn read_omp_config_yaml() -> Result<(String, String), AppError> {
    let path = get_omp_settings_path()?;
    read_omp_config_yaml_at(&path)
}

pub(crate) fn read_omp_config_yaml_at(path: &Path) -> Result<(String, String), AppError> {
    // Before OMP's YAML migration runs, expose the legacy JSONC document as
    // the exact text shown in the advanced editor. Return the source revision
    // as the CAS token so a concurrent settings.json edit cannot be lost.
    if !path.exists() {
        let legacy = path.with_file_name("settings.json");
        if legacy.exists() {
            let (document, source_revision) = parse_json5_document(&legacy, "OMP legacy settings")?;
            let text = serde_yaml::to_string(&document).map_err(|error| {
                AppError::Config(format!("failed to serialize OMP legacy settings: {error}"))
            })?;
            return Ok((text, source_revision));
        }
    }
    read_yaml_text_with_revision(path, "OMP config")
}

/// Read the exact native model registry text and its revision. When the file
/// does not exist, return an empty but valid OMP document so the first edit can
/// be saved without forcing the user to create the directory by hand.
pub(crate) fn read_omp_models_yaml() -> Result<(String, String), AppError> {
    let path = get_omp_models_path()?;
    let (text, revision) = read_yaml_text_with_revision(&path, "OMP models")?;
    if text.trim().is_empty() {
        return Ok(("providers: {}\n".to_string(), revision));
    }
    Ok((text, revision))
}

/// Replace the complete native config document after validating its YAML
/// shape. This is used by the TUI's advanced editor so unrelated OMP settings
/// remain available without requiring CC Switch to model the entire schema.
pub(crate) fn replace_omp_config_yaml_at(
    path: &Path,
    content: &str,
    expected_revision: &str,
) -> Result<(), AppError> {
    let document: Value = serde_yaml::from_str(content).map_err(|error| {
        AppError::InvalidInput(format!("OMP config is not valid YAML: {error}"))
    })?;
    if !document.is_object() {
        return Err(AppError::InvalidInput(
            "OMP config root must be an object".to_string(),
        ));
    }
    validate_omp_config_document(&document, path)?;
    let _guard = lock_models_file()?;
    let (_, actual_revision) = read_config_document_with_revision(path)?;
    if path.exists() {
        if actual_revision != expected_revision {
            return Err(AppError::Conflict(format!(
                "OMP config.yml changed outside CC Switch: {}",
                path.display()
            )));
        }
        return write_config_document(path, &document, &actual_revision);
    }

    // When the editor was opened against legacy settings.json, compare that
    // source file rather than the still-missing YAML destination. The write
    // itself remains guarded by the canonical missing-file revision.
    let legacy = path.with_file_name("settings.json");
    if legacy.exists() {
        let (_, legacy_revision) = parse_json5_document(&legacy, "OMP legacy settings")?;
        if legacy_revision != expected_revision {
            return Err(AppError::Conflict(format!(
                "OMP legacy settings changed outside CC Switch: {}",
                legacy.display()
            )));
        }
    } else if expected_revision != actual_revision {
        return Err(AppError::Conflict(format!(
            "OMP config.yml changed outside CC Switch: {}",
            path.display()
        )));
    }
    write_config_document(path, &document, &actual_revision)
}

/// Validate the portions of `config.yml` that CC Switch reads or edits while
/// retaining unknown keys for forward compatibility with newer OMP releases.
fn validate_omp_config_document(document: &Value, path: &Path) -> Result<(), AppError> {
    let root = document.as_object().ok_or_else(|| {
        AppError::InvalidInput(format!(
            "OMP config root must be an object: {}",
            path.display()
        ))
    })?;

    // Reuse the same role parser used by normal reads so the advanced editor
    // cannot save a document that the role page (or OMP itself) cannot load.
    model_role_entries_from_document(document, path)?;

    if let Some(storage) = root.get("modelRoleStorage") {
        let storage = storage.as_str().ok_or_else(|| {
            AppError::InvalidInput(format!(
                "OMP config modelRoleStorage must be 'global' or 'project': {}",
                path.display()
            ))
        })?;
        if !matches!(
            storage.trim().to_ascii_lowercase().as_str(),
            "global" | "project"
        ) {
            return Err(AppError::InvalidInput(format!(
                "OMP config modelRoleStorage must be 'global' or 'project', got '{storage}'"
            )));
        }
    }

    if let Some(disabled) = root.get("disabledProviders") {
        let entries = disabled.as_array().ok_or_else(|| {
            AppError::InvalidInput(format!(
                "OMP config disabledProviders must be an array: {}",
                path.display()
            ))
        })?;
        for (index, entry) in entries.iter().enumerate() {
            match entry {
                Value::String(id) if !id.trim().is_empty() => {}
                Value::Object(object) => {
                    // OMP supports path-scoped objects. Validate only the
                    // fields consumed by this adapter and leave extension
                    // fields untouched for forward compatibility.
                    for key in ["path", "pathPrefix", "paths", "pathPrefixes"] {
                        if let Some(value) = object.get(key) {
                            validate_string_or_array_field(
                                value,
                                &format!("OMP config disabledProviders[{index}].{key}"),
                            )?;
                        }
                    }
                    for key in ["providers", "values", "items"] {
                        if let Some(value) = object.get(key) {
                            validate_string_or_array_field(
                                value,
                                &format!("OMP config disabledProviders[{index}].{key}"),
                            )?;
                        }
                    }
                }
                _ => {
                    return Err(AppError::InvalidInput(format!(
                        "OMP config disabledProviders[{index}] must be a provider id string or path-scoped object"
                    )))
                }
            }
        }
    }

    Ok(())
}

/// Replace the complete native models document after validating its root and
/// provider map. Provider values are validated with the import validator so
/// extension-owned forward-compatible entries remain round-trippable.
pub(crate) fn replace_omp_models_yaml(
    content: &str,
    expected_revision: &str,
) -> Result<(), AppError> {
    let path = get_omp_models_path()?;
    let document: Value = serde_yaml::from_str(content).map_err(|error| {
        AppError::InvalidInput(format!("OMP models are not valid YAML: {error}"))
    })?;
    validate_omp_models_root(&document, &path)?;
    let provider_map = providers(&document, &path)?;
    for (provider_id, provider) in provider_map {
        // A full-file edit must preserve every configuration accepted by
        // OMP's native schema, including non-HTTP transports. Keep semantic
        // provider checks (models/api/override requirements), but defer URL
        // restrictions to CC Switch's HTTP-only discovery/test paths.
        validate_provider_node_for_editor(provider_id, provider)?;
    }
    let _guard = lock_models_file()?;
    let (_, actual_revision) = read_models_document_with_revision(&path)?;
    if actual_revision != expected_revision {
        return Err(AppError::Conflict(format!(
            "OMP models.yml changed outside CC Switch: {}",
            path.display()
        )));
    }
    write_models_document(&path, &document, expected_revision)
}

/// Read all valid model entries from every explicit OMP provider. OMP files are
/// forward-compatible and may contain one malformed extension/model node; a
/// single bad entry should not hide every other usable model from the TUI.
pub(crate) fn read_omp_native_models() -> Result<Vec<OMPNativeModel>, AppError> {
    let _guard = lock_models_file()?;
    let path = get_omp_models_path()?;
    let document = read_models_document(&path)?;
    let provider_map = providers(&document, &path)?;
    let mut result = Vec::new();
    for (provider_id, provider) in provider_map {
        let Some(models) = provider.get("models") else {
            continue;
        };
        let Some(models) = models.as_array() else {
            log::warn!(
                "Skipping OMP provider '{provider_id}' because models is not an array: {}",
                path.display()
            );
            continue;
        };
        for model in models {
            let Some(model_id) = model
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
            else {
                log::warn!(
                    "Skipping malformed OMP model without an id under provider '{provider_id}': {}",
                    path.display()
                );
                continue;
            };
            result.push(OMPNativeModel {
                provider_id: provider_id.clone(),
                model_id: model_id.to_string(),
                config: model.clone(),
            });
        }
    }
    Ok(result)
}

/// Add or replace one model in a provider's native model array. The complete
/// model object is supplied by the caller and all unrelated provider/root
/// fields are preserved.
pub(crate) fn upsert_omp_model_checked(
    provider_id: &str,
    model_id: &str,
    model: Value,
    expected_revision: &str,
) -> Result<(), AppError> {
    upsert_omp_model_inner(provider_id, model_id, model, Some(expected_revision))
}

fn upsert_omp_model_inner(
    provider_id: &str,
    model_id: &str,
    model: Value,
    expected_revision: Option<&str>,
) -> Result<(), AppError> {
    validate_provider_key(provider_id)?;
    validate_model_object(model_id, &model)?;
    let _guard = lock_models_file()?;
    let path = get_omp_models_path()?;
    let (mut document, file_revision) = read_models_document_with_revision(&path)?;
    if let Some(expected) = expected_revision {
        if expected != file_revision {
            return Err(AppError::Conflict(format!(
                "OMP models.yml changed outside CC Switch: {}",
                path.display()
            )));
        }
    }
    let provider = providers_mut(&mut document, &path)?
        .get_mut(provider_id)
        .ok_or_else(|| AppError::InvalidInput(format!("OMP provider '{provider_id}' not found")))?;
    let object = provider.as_object_mut().ok_or_else(|| {
        AppError::Config(format!(
            "OMP provider '{provider_id}' must be an object: {}",
            path.display()
        ))
    })?;
    let models = object
        .entry("models".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let models = models.as_array_mut().ok_or_else(|| {
        AppError::Config(format!(
            "OMP provider '{provider_id}' models must be an array: {}",
            path.display()
        ))
    })?;
    let model_id = model_id.trim();
    if let Some(existing) = models.iter_mut().find(|item| {
        item.get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| id == model_id)
    }) {
        *existing = model;
    } else {
        models.push(model);
    }
    // Adding a model changes the provider's semantic requirements. In
    // particular, OMP requires a provider-level `baseUrl` (and either
    // credentials or an explicit `auth` mode) whenever custom models are
    // present. Validate the merged provider before writing so an otherwise
    // valid override-only provider cannot be left in a state that OMP rejects
    // on its next load.
    validate_provider_node_for_editor(provider_id, provider)?;
    write_models_document(&path, &document, &file_revision)
}

pub(crate) fn remove_omp_model(provider_id: &str, model_id: &str) -> Result<bool, AppError> {
    remove_omp_model_inner(provider_id, model_id, None)
}

pub(crate) fn remove_omp_model_checked(
    provider_id: &str,
    model_id: &str,
    expected_revision: &str,
) -> Result<bool, AppError> {
    remove_omp_model_inner(provider_id, model_id, Some(expected_revision))
}

fn remove_omp_model_inner(
    provider_id: &str,
    model_id: &str,
    expected_revision: Option<&str>,
) -> Result<bool, AppError> {
    validate_provider_key(provider_id)?;
    let model_id = model_id.trim();
    if model_id.is_empty() {
        return Err(AppError::InvalidInput(
            "OMP model id cannot be empty".to_string(),
        ));
    }
    let _guard = lock_models_file()?;
    let path = get_omp_models_path()?;
    let (mut document, file_revision) = read_models_document_with_revision(&path)?;
    if let Some(expected) = expected_revision {
        if expected != file_revision {
            return Err(AppError::Conflict(format!(
                "OMP models.yml changed outside CC Switch: {}",
                path.display()
            )));
        }
    }
    // Check existence before role references so a missing target produces a
    // clear not-found result rather than an unrelated dangling-reference
    // error from another entry in modelRoles.
    let provider = providers(&document, &path)?
        .get(provider_id)
        .ok_or_else(|| AppError::InvalidInput(format!("OMP provider '{provider_id}' not found")))?;
    let models = provider
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            AppError::Config(format!(
                "OMP provider '{provider_id}' models must be an array: {}",
                path.display()
            ))
        })?;
    if !models
        .iter()
        .any(|item| item.get("id").and_then(Value::as_str) == Some(model_id))
    {
        return Ok(false);
    }

    // Keep the role-reference check under the same process-wide lock as the
    // models mutation.  Otherwise a concurrent CC Switch role write could
    // land between the check and deletion, leaving a dangling selector.
    ensure_model_role_references_clear_locked(provider_id, Some(model_id), &document)?;
    let provider = providers_mut(&mut document, &path)?
        .get_mut(provider_id)
        .expect("provider existence checked above");
    let models = provider
        .get_mut("models")
        .and_then(Value::as_array_mut)
        .expect("provider models type checked above");
    models.retain(|item| item.get("id").and_then(Value::as_str) != Some(model_id));
    write_models_document(&path, &document, &file_revision)?;
    Ok(true)
}

pub(crate) fn validate_provider_key(provider_id: &str) -> Result<(), AppError> {
    if provider_id.trim().is_empty()
        || provider_id.len() > 128
        || provider_id
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
        || provider_id.contains('/')
    {
        return Err(AppError::InvalidInput(
            "OMP provider key must be 1-128 non-whitespace characters and cannot contain '/'"
                .to_string(),
        ));
    }
    Ok(())
}

fn validate_model_role_name(role: &str) -> Result<(), AppError> {
    let role = role.trim();
    if role.is_empty() || role.len() > 128 || role.chars().any(|ch| ch.is_control()) {
        return Err(AppError::InvalidInput(
            "OMP model role must be 1-128 non-control characters".to_string(),
        ));
    }
    Ok(())
}

fn validate_model_selector(selector: &str) -> Result<(), AppError> {
    let selector = selector.trim();
    if selector.is_empty() || selector.len() > 512 || selector.chars().any(|ch| ch.is_control()) {
        return Err(AppError::InvalidInput(
            "OMP model selector must be 1-512 non-control characters".to_string(),
        ));
    }
    // OMP accepts concrete `provider/model` selectors as well as role aliases
    // (`@smol`, `@slow`) and the wildcard (`*`). Bare model ids are also
    // resolved by the native model resolver, so the adapter must not reject
    // them merely because they lack a slash.
    Ok(())
}

fn validate_model_object(model_id: &str, model: &Value) -> Result<(), AppError> {
    let object = model.as_object().ok_or_else(|| {
        AppError::InvalidInput("OMP model configuration must be an object".to_string())
    })?;
    let actual = object.get("id").and_then(Value::as_str).map(str::trim);
    if actual != Some(model_id.trim()) || model_id.trim().is_empty() {
        return Err(AppError::InvalidInput(
            "OMP model configuration id must match the requested model id".to_string(),
        ));
    }
    validate_model_metadata(object, &format!("OMP model '{model_id}'"), true)?;
    if let Some(base_url) = object.get("baseUrl") {
        let base_url = base_url.as_str().ok_or_else(|| {
            AppError::InvalidInput("OMP model baseUrl must be a string".to_string())
        })?;
        if base_url.trim().is_empty() || !is_valid_request_url(base_url) {
            return Err(AppError::InvalidInput(
                "OMP model baseUrl must be an absolute HTTP(S) URL".to_string(),
            ));
        }
    }
    if let Some(api) = object.get("api") {
        let api = api
            .as_str()
            .ok_or_else(|| AppError::InvalidInput("OMP model api must be a string".to_string()))?;
        validate_api_protocol(api)?;
    }
    Ok(())
}

pub(crate) fn read_omp_native_providers() -> Result<IndexMap<String, Value>, AppError> {
    let _guard = lock_models_file()?;
    read_omp_native_providers_locked(&get_omp_models_path()?)
}

pub(crate) fn read_omp_native_provider(provider_key: &str) -> Result<Option<Value>, AppError> {
    let _guard = lock_models_file()?;
    let path = get_omp_models_path()?;
    let document = read_models_document(&path)?;
    Ok(providers(&document, &path)?.get(provider_key).cloned())
}

pub(crate) fn omp_provider_exists(provider_key: &str) -> Result<bool, AppError> {
    let _guard = lock_models_file()?;
    let path = get_omp_models_path()?;
    let document = read_models_document(&path)?;
    Ok(providers(&document, &path)?.contains_key(provider_key))
}

pub(crate) fn insert_omp_provider(provider_key: &str, config: &Value) -> Result<bool, AppError> {
    validate_provider_node(provider_key, config)?;
    let _guard = lock_models_file()?;
    let path = get_omp_models_path()?;
    let (mut document, expected_revision) = read_models_document_with_revision(&path)?;
    let providers = providers_mut(&mut document, &path)?;

    match providers.get(provider_key) {
        Some(current) if current == config => return Ok(false),
        Some(_) => {
            return Err(AppError::InvalidInput(format!(
                "OMP provider key '{provider_key}' already exists in models.yml"
            )))
        }
        None => {}
    }

    providers.insert(provider_key.to_string(), config.clone());
    write_models_document(&path, &document, &expected_revision)?;
    Ok(true)
}

pub(crate) fn replace_omp_provider(
    provider_key: &str,
    expected: &Value,
    replacement: &Value,
) -> Result<(), AppError> {
    replace_omp_provider_inner(provider_key, expected, replacement, true)
}

pub(crate) fn replace_omp_provider_for_import(
    provider_key: &str,
    expected: &Value,
    replacement: &Value,
) -> Result<(), AppError> {
    replace_omp_provider_inner(provider_key, expected, replacement, false)
}

fn replace_omp_provider_inner(
    provider_key: &str,
    expected: &Value,
    replacement: &Value,
    enforce_semantics: bool,
) -> Result<(), AppError> {
    if enforce_semantics {
        validate_provider_node(provider_key, replacement)?;
    } else {
        validate_provider_node_for_import(provider_key, replacement)?;
    }
    let _guard = lock_models_file()?;
    let path = get_omp_models_path()?;
    let (mut document, expected_revision) = read_models_document_with_revision(&path)?;
    let providers = providers_mut(&mut document, &path)?;
    let current = providers.get(provider_key).ok_or_else(|| {
        AppError::Conflict(format!(
            "OMP provider '{provider_key}' is no longer present in models.yml"
        ))
    })?;
    if !provider_configs_equal_ignoring_name(current, expected) {
        return Err(AppError::Conflict(format!(
            "OMP provider '{provider_key}' changed outside CC Switch"
        )));
    }
    if provider_configs_equal_ignoring_name(current, replacement) {
        return Ok(());
    }
    providers.insert(provider_key.to_string(), replacement.clone());
    write_models_document(&path, &document, &expected_revision)
}

/// Replace a provider when it exists, but only if its current native value
/// still matches the value CC Switch previously loaded. This closes the
/// read/modify/write race where an external edit to `models.yml` could be
/// silently overwritten by a normal provider update.
pub(crate) fn replace_omp_provider_if_present_checked(
    provider_key: &str,
    expected: &Value,
    replacement: &Value,
) -> Result<Option<Value>, AppError> {
    replace_omp_provider_if_present_inner(provider_key, Some(expected), replacement, true)
}

pub(crate) fn replace_omp_provider_if_present_for_import_checked(
    provider_key: &str,
    expected: &Value,
    replacement: &Value,
) -> Result<Option<Value>, AppError> {
    replace_omp_provider_if_present_inner(provider_key, Some(expected), replacement, false)
}

fn replace_omp_provider_if_present_inner(
    provider_key: &str,
    expected: Option<&Value>,
    replacement: &Value,
    enforce_semantics: bool,
) -> Result<Option<Value>, AppError> {
    if enforce_semantics {
        validate_provider_node(provider_key, replacement)?;
    } else {
        validate_provider_node_for_import(provider_key, replacement)?;
    }
    let _guard = lock_models_file()?;
    let path = get_omp_models_path()?;
    let (mut document, expected_revision) = read_models_document_with_revision(&path)?;
    let providers = providers_mut(&mut document, &path)?;
    let Some(current) = providers.get(provider_key).cloned() else {
        return Ok(None);
    };
    if expected.is_some_and(|expected| !provider_configs_equal_ignoring_name(&current, expected)) {
        return Err(AppError::Conflict(format!(
            "OMP provider '{provider_key}' changed outside CC Switch"
        )));
    }
    if provider_configs_equal_ignoring_name(&current, replacement) {
        return Ok(Some(current));
    }
    providers.insert(provider_key.to_string(), replacement.clone());
    write_models_document(&path, &document, &expected_revision)?;
    Ok(Some(current))
}

pub(crate) fn remove_omp_provider_if_matches(
    provider_key: &str,
    expected: &Value,
) -> Result<bool, AppError> {
    remove_omp_provider_inner(provider_key, Some(expected)).map(|removed| removed.is_some())
}

/// Remove the current native provider value after the caller has explicitly
/// requested a live-config removal. The latest native value is returned so the
/// service can preserve it in the CC Switch catalog for a later restore.
pub(crate) fn remove_omp_provider(provider_key: &str) -> Result<Option<Value>, AppError> {
    remove_omp_provider_inner(provider_key, None)
}

/// Remove a provider only when the native value still matches the snapshot
/// imported by CC Switch. This is the destructive counterpart to the update
/// compare-and-swap path and prevents an external `models.yml` edit from being
/// silently discarded by a later delete/remove action.
pub(crate) fn remove_omp_provider_checked(
    provider_key: &str,
    expected: &Value,
) -> Result<Option<Value>, AppError> {
    remove_omp_provider_inner(provider_key, Some(expected))
}

fn remove_omp_provider_inner(
    provider_key: &str,
    expected: Option<&Value>,
) -> Result<Option<Value>, AppError> {
    let _guard = lock_models_file()?;
    let path = get_omp_models_path()?;
    let (mut document, expected_revision) = read_models_document_with_revision(&path)?;
    let current = providers(&document, &path)?.get(provider_key).cloned();
    let Some(current) = current else {
        return Ok(None);
    };
    if expected.is_some_and(|expected| !provider_configs_equal_ignoring_name(&current, expected)) {
        return Err(AppError::Conflict(format!(
            "OMP provider '{provider_key}' changed outside CC Switch"
        )));
    }
    // The role check must be inside the same lock and immediately before the
    // destructive write; see remove_omp_model_inner for the invariant.
    ensure_model_role_references_clear_locked(provider_key, None, &document)?;
    let providers = providers_mut(&mut document, &path)?;
    providers.remove(provider_key);
    write_models_document(&path, &document, &expected_revision)?;
    Ok(Some(current))
}

fn ensure_model_role_references_clear_locked(
    provider_id: &str,
    model_id: Option<&str>,
    models_document: &Value,
) -> Result<(), AppError> {
    // Destructive removal must fail closed when the role configuration cannot
    // be parsed.  Proceeding without knowing whether `default` or another
    // role references the target recreates the very dangling-selector bug this
    // guard is intended to prevent.
    let roles = read_omp_model_roles_with_metadata_locked()?.0;
    let model_providers = model_provider_index(models_document);
    let mut references = Vec::new();
    for (role, selector) in roles.iter() {
        if selector_references_omp_target(
            selector,
            provider_id,
            model_id,
            &roles,
            Some(&model_providers),
            &mut HashSet::new(),
        )? {
            references.push(role.clone());
        }
    }
    if references.is_empty() {
        return Ok(());
    }
    references.sort();
    let subject = model_id
        .map(|model| format!("{provider_id}/{model}"))
        .unwrap_or_else(|| provider_id.to_string());
    Err(AppError::InvalidInput(format!(
        "cannot remove OMP model/provider '{subject}': modelRoles references it ({})",
        references.join(", ")
    )))
}

fn model_provider_index(document: &Value) -> HashMap<String, HashSet<String>> {
    let mut index: HashMap<String, HashSet<String>> = HashMap::new();
    let Some(providers) = document.get("providers").and_then(Value::as_object) else {
        return index;
    };
    for (provider_id, provider) in providers {
        let Some(models) = provider.get("models").and_then(Value::as_array) else {
            continue;
        };
        for model in models {
            if let Some(model_id) = model.get("id").and_then(Value::as_str) {
                index
                    .entry(model_id.trim().to_string())
                    .or_default()
                    .insert(provider_id.clone());
            }
        }
    }
    index
}

fn selector_references_omp_target(
    selector: &str,
    provider_id: &str,
    model_id: Option<&str>,
    roles: &IndexMap<String, String>,
    model_providers: Option<&HashMap<String, HashSet<String>>>,
    visited_roles: &mut HashSet<String>,
) -> Result<bool, AppError> {
    let selector = selector.trim();
    if selector.is_empty() {
        return Ok(false);
    }
    let base = strip_omp_thinking_suffix(selector);

    // `pi/<role>` is OMP's legacy role-alias spelling. Recognize it before
    // treating the slash as a provider/model separator when the suffix names
    // a known or configured role.
    if let Some(role_name) = base.strip_prefix("pi/") {
        if roles.contains_key(role_name) || OMP_BUILTIN_MODEL_ROLES.contains(&role_name) {
            return selector_references_omp_target(
                &format!("@{role_name}"),
                provider_id,
                model_id,
                roles,
                model_providers,
                visited_roles,
            );
        }
    }

    // Explicit provider/model selectors are unambiguous and are the common
    // case. Provider wildcards matter when removing an entire provider, but a
    // model-specific delete does not make `provider/*` dangling.
    if let Some((selector_provider, selector_model)) = base.split_once('/') {
        if selector_provider != provider_id {
            return Ok(false);
        }
        return Ok(match model_id {
            Some(target_model) => selector_model == target_model,
            None => true,
        });
    }

    // OMP's `*`, `@role`, and legacy `pi/role` forms are aliases. Resolve
    // them recursively so deleting a model cannot leave an indirect role
    // reference behind. Cycles are malformed native config; fail closed.
    let role_name = if base == "*" {
        Some("default")
    } else if let Some(name) = base.strip_prefix('@') {
        Some(name)
    } else {
        base.strip_prefix("pi/")
    };
    if let Some(role_name) = role_name {
        if !visited_roles.insert(role_name.to_string()) {
            return Err(AppError::InvalidInput(format!(
                "cannot remove OMP model/provider '{provider_id}': cyclic modelRoles alias involving '{role_name}'"
            )));
        }
        let result = roles
            .get(role_name)
            .map(|value| {
                selector_references_omp_target(
                    value,
                    provider_id,
                    model_id,
                    roles,
                    model_providers,
                    visited_roles,
                )
            })
            .transpose()?
            .unwrap_or(false);
        visited_roles.remove(role_name);
        return Ok(result);
    }

    // Bare model ids are resolved by OMP against its available registry. They
    // are only unambiguous for destructive checks when the id belongs to one
    // provider; in that case block deletion of that provider/model.
    if let Some(target_model) = model_id {
        if base != target_model {
            return Ok(false);
        }
        return Ok(model_providers
            .and_then(|index| index.get(base))
            .is_some_and(|providers| providers.len() == 1 && providers.contains(provider_id)));
    }

    // Removing a provider: a bare selector can point at any of its models.
    Ok(model_providers
        .and_then(|index| index.get(base))
        .is_some_and(|providers| providers.len() == 1 && providers.contains(provider_id)))
}

fn strip_omp_thinking_suffix(selector: &str) -> &str {
    let Some((base, suffix)) = selector.rsplit_once(':') else {
        return selector;
    };
    matches!(
        suffix,
        "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "auto"
    )
    .then_some(base)
    .unwrap_or(selector)
}

/// Resolve the provider portion of an OMP selector for UI/status purposes.
///
/// OMP accepts concrete `provider/model` selectors, wildcard/default aliases,
/// and recursive `@role`/`pi/role` aliases. Bare model ids cannot be mapped
/// safely without the complete native registry and therefore return `None`.
pub(crate) fn omp_selector_provider_id(
    selector: &str,
    roles: &IndexMap<String, String>,
) -> Option<String> {
    fn resolve(
        selector: &str,
        roles: &IndexMap<String, String>,
        visited: &mut HashSet<String>,
    ) -> Option<String> {
        let selector = strip_omp_thinking_suffix(selector.trim());
        if selector.is_empty() {
            return None;
        }
        if let Some((provider, model)) = selector.split_once('/') {
            if provider == "pi"
                && (roles.contains_key(model) || OMP_BUILTIN_MODEL_ROLES.contains(&model))
            {
                return resolve(&format!("@{model}"), roles, visited);
            }
            return (!provider.is_empty() && !model.is_empty()).then(|| provider.to_string());
        }
        let role = if selector == "*" {
            Some("default")
        } else {
            selector.strip_prefix('@')
        };
        let role = role?;
        if !visited.insert(role.to_string()) {
            return None;
        }
        let result = roles
            .get(role)
            .and_then(|selector| resolve(selector, roles, visited));
        visited.remove(role);
        result
    }

    resolve(selector, roles, &mut HashSet::new())
}

pub(crate) fn restore_omp_provider_if_missing(
    provider_key: &str,
    config: &Value,
) -> Result<(), AppError> {
    let _guard = lock_models_file()?;
    let path = get_omp_models_path()?;
    let (mut document, expected_revision) = read_models_document_with_revision(&path)?;
    let providers = providers_mut(&mut document, &path)?;
    match providers.get(provider_key) {
        Some(current) if current == config => Ok(()),
        Some(_) => Err(AppError::Conflict(format!(
            "cannot restore OMP provider '{provider_key}' because another value now owns the key"
        ))),
        None => {
            providers.insert(provider_key.to_string(), config.clone());
            write_models_document(&path, &document, &expected_revision)
        }
    }
}

/// Validate the shape CC Switch can persist as one
/// `models.yml.providers.<provider_key>` node.
///
/// Provider ownership is intentionally source-based: every explicit object in
/// `models.yml.providers` is manageable, including keys also built into OMP.
/// OMP's `/login` credentials live in `auth.json` and are never read here.
pub(crate) fn validate_provider_node(provider_key: &str, config: &Value) -> Result<(), AppError> {
    validate_provider_node_inner(provider_key, config, true, true)
}

/// Validate a complete native provider document for the advanced editor.
/// OMP's schema permits any non-empty `baseUrl` string (for example a native
/// transport URI); CC Switch only requires HTTP(S) when it actively probes a
/// provider. Keep semantic checks while allowing those native URLs to round
/// trip unchanged.
pub(crate) fn validate_provider_node_for_editor(
    provider_key: &str,
    config: &Value,
) -> Result<(), AppError> {
    validate_provider_node_inner(provider_key, config, true, false)
}

/// Validate only the syntactic/typed portions of an OMP provider node.
///
/// Native `models.yml` entries can be owned by OMP itself or by extensions and
/// may intentionally contain only forward-compatible fields. Importing those
/// entries must preserve them even when CC Switch cannot apply its stricter
/// custom-provider semantics. Writes originating from CC Switch continue to
/// use [`validate_provider_node`].
pub(crate) fn validate_provider_node_for_import(
    provider_key: &str,
    config: &Value,
) -> Result<(), AppError> {
    validate_provider_node_inner(provider_key, config, false, false)
}

fn validate_provider_node_inner(
    provider_key: &str,
    config: &Value,
    enforce_semantics: bool,
    enforce_http_urls: bool,
) -> Result<(), AppError> {
    validate_provider_key(provider_key)?;
    let object = config.as_object().ok_or_else(|| {
        AppError::InvalidInput("OMP provider configuration must be an object".to_string())
    })?;
    if object.is_empty() {
        return Err(AppError::InvalidInput(
            "OMP provider configuration cannot be empty".to_string(),
        ));
    }
    if let Some(base_url) = object.get("baseUrl") {
        let base_url = base_url.as_str().ok_or_else(|| {
            AppError::InvalidInput("OMP provider baseUrl must be a string".to_string())
        })?;
        if base_url.trim().is_empty() {
            return Err(AppError::InvalidInput(
                "OMP provider baseUrl must be a non-empty string".to_string(),
            ));
        }
        if enforce_http_urls && !is_valid_request_url(base_url) {
            return Err(AppError::InvalidInput(
                "OMP provider baseUrl must be an absolute HTTP(S) URL".to_string(),
            ));
        }
    }
    if let Some(api_key) = object.get("apiKey") {
        let api_key = api_key.as_str().ok_or_else(|| {
            AppError::InvalidInput("OMP provider apiKey must be a string".to_string())
        })?;
        if api_key.trim().is_empty() {
            return Err(AppError::InvalidInput(
                "OMP provider apiKey must be a non-empty string".to_string(),
            ));
        }
    }
    if let Some(headers) = object.get("headers") {
        let headers = headers.as_object().ok_or_else(|| {
            AppError::InvalidInput("OMP provider headers must be an object".to_string())
        })?;
        if headers.values().any(|value| !value.is_string()) {
            return Err(AppError::InvalidInput(
                "OMP provider headers values must be strings".to_string(),
            ));
        }
    }
    if let Some(api) = object.get("api") {
        let api = api.as_str().ok_or_else(|| {
            AppError::InvalidInput("OMP provider api must be a string".to_string())
        })?;
        validate_api_protocol(api)?;
    }
    if let Some(auth) = object.get("auth") {
        let auth = auth.as_str().ok_or_else(|| {
            AppError::InvalidInput("OMP provider auth must be a string".to_string())
        })?;
        if !matches!(auth, "apiKey" | "none" | "oauth") {
            return Err(AppError::InvalidInput(format!(
                "Unsupported OMP provider auth '{}'. Supported: apiKey, none, oauth",
                auth
            )));
        }
    }
    validate_optional_bool(object, "authHeader", "OMP provider authHeader")?;
    validate_optional_bool(
        object,
        "disableStrictTools",
        "OMP provider disableStrictTools",
    )?;
    validate_optional_string(
        object,
        "guardrailIdentifier",
        "OMP provider guardrailIdentifier",
    )?;
    validate_optional_string(object, "guardrailVersion", "OMP provider guardrailVersion")?;
    validate_optional_enum(
        object,
        "guardrailTrace",
        &["enabled", "disabled", "enabled_full"],
        "OMP provider guardrailTrace",
    )?;
    validate_optional_enum(
        object,
        "transport",
        &["pi-native"],
        "OMP provider transport",
    )?;
    if let Some(request_metadata) = object.get("requestMetadata") {
        let request_metadata = request_metadata.as_object().ok_or_else(|| {
            AppError::InvalidInput("OMP provider requestMetadata must be an object".to_string())
        })?;
        if request_metadata.values().any(|value| !value.is_string()) {
            return Err(AppError::InvalidInput(
                "OMP provider requestMetadata values must be strings".to_string(),
            ));
        }
    }
    if let Some(discovery) = object.get("discovery") {
        let discovery = discovery.as_object().ok_or_else(|| {
            AppError::InvalidInput("OMP provider discovery must be an object".to_string())
        })?;
        let discovery_type = discovery
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AppError::InvalidInput("OMP provider discovery.type must be a string".to_string())
            })?;
        if !matches!(
            discovery_type,
            "ollama" | "llama.cpp" | "lm-studio" | "openai-models-list" | "proxy" | "litellm"
        ) {
            return Err(AppError::InvalidInput(format!(
                "Unsupported OMP discovery type '{}'",
                discovery_type
            )));
        }
        if let Some(timeout) = discovery.get("timeoutMs") {
            let timeout = timeout.as_f64().ok_or_else(|| {
                AppError::InvalidInput(
                    "OMP provider discovery.timeoutMs must be a number".to_string(),
                )
            })?;
            if !timeout.is_finite() || timeout <= 0.0 {
                return Err(AppError::InvalidInput(
                    "OMP provider discovery.timeoutMs must be positive".to_string(),
                ));
            }
        }
        if let Some(inject_v1) = discovery.get("injectV1") {
            if !inject_v1.is_boolean() {
                return Err(AppError::InvalidInput(
                    "OMP provider discovery.injectV1 must be a boolean".to_string(),
                ));
            }
            if discovery_type != "openai-models-list" {
                return Err(AppError::InvalidInput(
                    "OMP provider discovery.injectV1 is only valid for openai-models-list"
                        .to_string(),
                ));
            }
        }
    }
    if let Some(remote) = object.get("remoteCompaction") {
        validate_remote_compaction(remote, "OMP provider remoteCompaction")?;
    }
    if let Some(compat) = object.get("compat") {
        validate_compat(compat, "OMP provider compat")?;
    }
    if let Some(overrides) = object.get("modelOverrides") {
        let overrides = overrides.as_object().ok_or_else(|| {
            AppError::InvalidInput("OMP provider modelOverrides must be an object".to_string())
        })?;
        if overrides.values().any(|value| !value.is_object()) {
            return Err(AppError::InvalidInput(
                "OMP provider modelOverrides values must be objects".to_string(),
            ));
        }
        for (model_id, override_value) in overrides {
            let override_object = override_value
                .as_object()
                .expect("modelOverrides values checked above");
            validate_model_metadata(
                override_object,
                &format!("OMP modelOverrides '{model_id}'"),
                false,
            )?;
        }
    }
    if let Some(models) = object.get("models") {
        let models = models.as_array().ok_or_else(|| {
            AppError::InvalidInput("OMP provider models must be an array".to_string())
        })?;
        for model in models {
            let model = model.as_object().ok_or_else(|| {
                AppError::InvalidInput("OMP provider model must be an object".to_string())
            })?;
            let model_id = model.get("id").ok_or_else(|| {
                AppError::InvalidInput("OMP provider model id is required".to_string())
            })?;
            let model_id = model_id.as_str().ok_or_else(|| {
                AppError::InvalidInput("OMP provider model id must be a string".to_string())
            })?;
            if model_id.trim().is_empty() {
                return Err(AppError::InvalidInput(
                    "OMP provider model id must be a non-empty string".to_string(),
                ));
            }
            validate_model_metadata(model, &format!("OMP model '{model_id}'"), true)?;
            if let Some(base_url) = model.get("baseUrl") {
                let base_url = base_url.as_str().ok_or_else(|| {
                    AppError::InvalidInput("OMP model baseUrl must be a string".to_string())
                })?;
                if base_url.trim().is_empty() {
                    return Err(AppError::InvalidInput(
                        "OMP model baseUrl must be a non-empty string".to_string(),
                    ));
                }
                if enforce_http_urls && !is_valid_request_url(base_url) {
                    return Err(AppError::InvalidInput(
                        "OMP model baseUrl must be an absolute HTTP(S) URL".to_string(),
                    ));
                }
            }
            if let Some(headers) = model.get("headers") {
                let headers = headers.as_object().ok_or_else(|| {
                    AppError::InvalidInput("OMP model headers must be an object".to_string())
                })?;
                if headers.values().any(|value| !value.is_string()) {
                    return Err(AppError::InvalidInput(
                        "OMP model headers values must be strings".to_string(),
                    ));
                }
            }
            if let Some(api) = model.get("api") {
                let api = api.as_str().ok_or_else(|| {
                    AppError::InvalidInput("OMP model api must be a string".to_string())
                })?;
                validate_api_protocol(api)?;
            }
            for field in ["contextWindow", "maxTokens"] {
                if let Some(value) = model.get(field) {
                    let valid = value
                        .as_f64()
                        .is_some_and(|number| number.is_finite() && number > 0.0);
                    if !valid {
                        return Err(AppError::InvalidInput(format!(
                            "OMP model {field} must be a positive number"
                        )));
                    }
                }
            }
        }
    }

    if enforce_semantics {
        // Keep provider-level semantic requirements aligned with OMP's
        // `validateProviderConfiguration()`. Extension-owned native nodes may
        // be opaque (for example `{ extension: {...} }`), so those are retained even
        // without a recognized model/override field.
        let models = object.get("models").and_then(Value::as_array);
        let has_models = models.is_some_and(|models| !models.is_empty());
        let provider_api = object.get("api").and_then(Value::as_str);
        if has_models {
            let has_provider_base_url = object
                .get("baseUrl")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty());
            if !has_provider_base_url {
                return Err(AppError::InvalidInput(
                    "OMP provider baseUrl is required when defining custom models".to_string(),
                ));
            }
            let auth = object
                .get("auth")
                .and_then(Value::as_str)
                .unwrap_or("apiKey");
            // Native OAuth-backed custom providers may omit an inline apiKey;
            // OMP resolves their credentials through its own auth store.
            if object
                .get("apiKey")
                .and_then(Value::as_str)
                .is_none_or(|value| value.trim().is_empty())
                && auth != "none"
                && auth != "oauth"
            {
                return Err(AppError::InvalidInput(format!(
                    "OMP provider '{provider_key}' with models requires apiKey unless auth is none or oauth"
                )));
            }
            if provider_api.is_none()
                && models.is_some_and(|models| {
                    models
                        .iter()
                        .any(|model| model.get("api").and_then(Value::as_str).is_none())
                })
            {
                return Err(AppError::InvalidInput(
                    "OMP provider api is required at provider or every model".to_string(),
                ));
            }
        } else {
            let has_model_overrides = object
                .get("modelOverrides")
                .and_then(Value::as_object)
                .is_some_and(|overrides| !overrides.is_empty());
            let has_nonempty_base_url = object
                .get("baseUrl")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty());
            let has_nonempty_api_key = object
                .get("apiKey")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty());
            // OMP's validator uses JavaScript truthiness for these optional
            // object fields. An explicitly supplied empty object is therefore
            // still a valid override-only provider configuration.
            let has_headers = object.get("headers").is_some();
            let has_compat = object.get("compat").is_some();
            let has_request_metadata = object.get("requestMetadata").is_some();
            let auth_none = object.get("auth").and_then(Value::as_str) == Some("none");
            let has_disable_strict_tools = object
                .get("disableStrictTools")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let has_guardrail = object
                .get("guardrailIdentifier")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty());
            let has_known_override = has_nonempty_base_url
                || has_nonempty_api_key
                || auth_none
                || has_headers
                || has_compat
                || has_request_metadata
                || has_disable_strict_tools
                || has_guardrail
                || object.get("remoteCompaction").is_some()
                || has_model_overrides
                || object.get("discovery").is_some();
            // Unknown fields are preserved verbatim for forward compatibility,
            // but they do not satisfy OMP's semantic requirement on their own.
            // Extension-owned nodes are the sole exception: the explicit
            // `extension` marker tells OMP that the extension supplies the rest of
            // the provider behavior.
            let has_extension_marker = object.contains_key("extension");
            if !has_known_override && !has_extension_marker {
                return Err(AppError::InvalidInput(format!(
                    "OMP provider '{provider_key}' must specify an override or at least one model"
                )));
            }
        }

        if let Some(discovery) = object.get("discovery").and_then(Value::as_object) {
            let discovery_type = discovery
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if discovery_type != "proxy" && provider_api.is_none() {
                return Err(AppError::InvalidInput(
                    "OMP provider api is required when discovery is enabled unless discovery.type is proxy"
                        .to_string(),
                ));
            }
        }
    }
    Ok(())
}

fn validate_optional_bool(
    object: &Map<String, Value>,
    key: &str,
    label: &str,
) -> Result<(), AppError> {
    if object.get(key).is_some_and(|value| !value.is_boolean()) {
        return Err(AppError::InvalidInput(format!("{label} must be a boolean")));
    }
    Ok(())
}

fn validate_optional_string(
    object: &Map<String, Value>,
    key: &str,
    label: &str,
) -> Result<(), AppError> {
    if object.get(key).is_some_and(|value| !value.is_string()) {
        return Err(AppError::InvalidInput(format!("{label} must be a string")));
    }
    Ok(())
}

fn validate_optional_nonempty_string(
    object: &Map<String, Value>,
    key: &str,
    label: &str,
) -> Result<(), AppError> {
    let Some(value) = object.get(key) else {
        return Ok(());
    };
    let value = value
        .as_str()
        .ok_or_else(|| AppError::InvalidInput(format!("{label} must be a string")))?;
    if value.trim().is_empty() {
        return Err(AppError::InvalidInput(format!("{label} must be non-empty")));
    }
    Ok(())
}

fn validate_optional_enum(
    object: &Map<String, Value>,
    key: &str,
    allowed: &[&str],
    label: &str,
) -> Result<(), AppError> {
    let Some(value) = object.get(key) else {
        return Ok(());
    };
    let value = value
        .as_str()
        .ok_or_else(|| AppError::InvalidInput(format!("{label} must be a string")))?;
    if !allowed.contains(&value) {
        return Err(AppError::InvalidInput(format!(
            "Unsupported {label} '{value}'. Supported: {}",
            allowed.join(", ")
        )));
    }
    Ok(())
}

fn validate_model_metadata(
    model: &Map<String, Value>,
    context: &str,
    require_complete_cost: bool,
) -> Result<(), AppError> {
    validate_optional_nonempty_string(model, "name", &format!("{context} name"))?;
    validate_optional_bool(model, "reasoning", &format!("{context} reasoning"))?;
    validate_optional_bool(model, "supportsTools", &format!("{context} supportsTools"))?;
    validate_optional_bool(
        model,
        "omitMaxOutputTokens",
        &format!("{context} omitMaxOutputTokens"),
    )?;
    validate_optional_bool(
        model,
        "preferWebsockets",
        &format!("{context} preferWebsockets"),
    )?;
    validate_optional_nonempty_string(
        model,
        "contextPromotionTarget",
        &format!("{context} contextPromotionTarget"),
    )?;
    validate_optional_nonempty_string(
        model,
        "compactionModel",
        &format!("{context} compactionModel"),
    )?;
    validate_optional_enum(
        model,
        "imageInputDecoder",
        &["stb"],
        &format!("{context} imageInputDecoder"),
    )?;
    validate_optional_enum(
        model,
        "tokenizer",
        &[
            "claude-v3",
            "claude-v47",
            "claude-v5",
            "claude-v5-sonnet",
            "qwen3",
            "deepseek-v3",
            "kimi-k2",
            "glm5",
        ],
        &format!("{context} tokenizer"),
    )?;
    if let Some(input) = model.get("input") {
        let input = input
            .as_array()
            .ok_or_else(|| AppError::InvalidInput(format!("{context} input must be an array")))?;
        if input.iter().any(|value| {
            !value
                .as_str()
                .is_some_and(|value| matches!(value, "text" | "image"))
        }) {
            return Err(AppError::InvalidInput(format!(
                "{context} input values must be 'text' or 'image'"
            )));
        }
    }
    if let Some(thinking) = model.get("thinking") {
        validate_thinking(thinking, &format!("{context} thinking"))?;
    }
    if let Some(headers) = model.get("headers") {
        let headers = headers.as_object().ok_or_else(|| {
            AppError::InvalidInput(format!("{context} headers must be an object"))
        })?;
        if headers.values().any(|value| !value.is_string()) {
            return Err(AppError::InvalidInput(format!(
                "{context} headers values must be strings"
            )));
        }
    }
    if let Some(compat) = model.get("compat") {
        validate_compat(compat, &format!("{context} compat"))?;
    }
    if let Some(cost) = model.get("cost") {
        let cost = cost
            .as_object()
            .ok_or_else(|| AppError::InvalidInput(format!("{context} cost must be an object")))?;
        if require_complete_cost
            && ["input", "output", "cacheRead", "cacheWrite"]
                .iter()
                .any(|key| !cost.contains_key(*key))
        {
            return Err(AppError::InvalidInput(format!(
                "{context} cost must contain input, output, cacheRead, and cacheWrite"
            )));
        }
        if cost.values().any(|value| !value.is_number()) {
            return Err(AppError::InvalidInput(format!(
                "{context} cost values must be numbers"
            )));
        }
    }
    for field in ["premiumMultiplier", "contextWindow", "maxTokens"] {
        if let Some(value) = model.get(field) {
            let number = value.as_f64().ok_or_else(|| {
                AppError::InvalidInput(format!("{context} {field} must be a number"))
            })?;
            // OMP uses premiumMultiplier as a cost multiplier and permits
            // zero to explicitly disable the premium surcharge. Context and
            // output limits remain strictly positive below.
            let valid = if field == "premiumMultiplier" {
                number.is_finite() && number >= 0.0
            } else {
                number.is_finite() && number > 0.0
            };
            if !valid {
                return Err(AppError::InvalidInput(format!(
                    "{context} {field} must be {}",
                    if field == "premiumMultiplier" {
                        "non-negative"
                    } else {
                        "positive"
                    }
                )));
            }
        }
    }
    if let Some(remote) = model.get("remoteCompaction") {
        validate_remote_compaction(remote, &format!("{context} remoteCompaction"))?;
    }
    Ok(())
}

fn validate_remote_compaction(value: &Value, context: &str) -> Result<(), AppError> {
    let object = value
        .as_object()
        .ok_or_else(|| AppError::InvalidInput(format!("{context} must be an object")))?;
    validate_optional_bool(object, "enabled", &format!("{context} enabled"))?;
    validate_optional_enum(object, "api", &OMP_API_PROTOCOLS, &format!("{context} api"))?;
    for field in ["endpoint", "model", "v2Endpoint", "streamingEndpoint"] {
        validate_optional_nonempty_string(object, field, &format!("{context} {field}"))?;
    }
    validate_optional_bool(
        object,
        "v2StreamingEnabled",
        &format!("{context} v2StreamingEnabled"),
    )?;
    Ok(())
}

fn validate_thinking(value: &Value, context: &str) -> Result<(), AppError> {
    let object = value
        .as_object()
        .ok_or_else(|| AppError::InvalidInput(format!("{context} must be an object")))?;
    validate_optional_enum(
        object,
        "mode",
        &[
            "effort",
            "budget",
            "google-level",
            "anthropic-adaptive",
            "anthropic-budget-effort",
        ],
        &format!("{context} mode"),
    )?;
    if !object.contains_key("mode") {
        return Err(AppError::InvalidInput(format!(
            "{context} mode is required"
        )));
    }
    for key in ["efforts", "levels"] {
        if let Some(values) = object.get(key) {
            let values = values.as_array().ok_or_else(|| {
                AppError::InvalidInput(format!("{context} {key} must be an array"))
            })?;
            if values.iter().any(|value| {
                !value.as_str().is_some_and(|value| {
                    matches!(
                        value,
                        "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
                    )
                })
            }) {
                return Err(AppError::InvalidInput(format!(
                    "{context} {key} values are invalid"
                )));
            }
        }
    }
    for key in ["defaultLevel", "minLevel", "maxLevel"] {
        validate_optional_enum(
            object,
            key,
            &["minimal", "low", "medium", "high", "xhigh", "max"],
            &format!("{context} {key}"),
        )?;
    }
    validate_optional_bool(
        object,
        "supportsDisplay",
        &format!("{context} supportsDisplay"),
    )?;
    validate_optional_bool(
        object,
        "requiresEffort",
        &format!("{context} requiresEffort"),
    )?;
    if let Some(map) = object.get("effortMap") {
        validate_string_map(map, &format!("{context} effortMap"))?;
    }
    let has_efforts = object
        .get("efforts")
        .or_else(|| object.get("levels"))
        .is_some();
    let has_range = object.contains_key("minLevel") && object.contains_key("maxLevel");
    if !has_efforts && !has_range {
        return Err(AppError::InvalidInput(format!(
            "{context} requires efforts, levels, or minLevel/maxLevel"
        )));
    }
    Ok(())
}

fn validate_compat(value: &Value, context: &str) -> Result<(), AppError> {
    let object = value
        .as_object()
        .ok_or_else(|| AppError::InvalidInput(format!("{context} must be an object")))?;

    for key in [
        "supportsStore",
        "supportsDeveloperRole",
        "supportsMultipleSystemMessages",
        "supportsReasoningEffort",
        "supportsUsageInStreaming",
        "requiresToolResultName",
        "requiresMistralToolIds",
        "requiresAssistantAfterToolResult",
        "requiresThinkingAsText",
        "requiresReasoningContentForToolCalls",
        "allowsSyntheticReasoningContentForToolCalls",
        "requiresAssistantContentForToolCalls",
        "supportsToolChoice",
        "supportsForcedToolChoice",
        "disableReasoningOnForcedToolChoice",
        "disableReasoningOnToolChoice",
        "qwenTemplateReasoningEffort",
        "supportsStrictMode",
        "supportsLongPromptCacheRetention",
        "supportsReasoningParams",
        "supportsReasoningSummary",
        "alwaysSendMaxTokens",
        "strictResponsesPairing",
        "supportsImageDetailOriginal",
        "supportsContextManagement",
        "supportsEagerToolInputStreaming",
        "allowAnthropicHeaderOverrides",
        "requiresToolResultId",
        "replayUnsignedThinking",
    ] {
        validate_optional_bool(object, key, &format!("{context} {key}"))?;
    }

    validate_optional_enum(
        object,
        "maxTokensField",
        &["max_completion_tokens", "max_tokens"],
        &format!("{context} maxTokensField"),
    )?;
    validate_optional_enum(
        object,
        "reasoningContentField",
        &["reasoning_content", "reasoning", "reasoning_text"],
        &format!("{context} reasoningContentField"),
    )?;
    validate_optional_enum(
        object,
        "thinkingFormat",
        &["openai", "openrouter", "zai", "qwen", "qwen-chat-template"],
        &format!("{context} thinkingFormat"),
    )?;
    validate_optional_enum(
        object,
        "cacheControlFormat",
        &["anthropic"],
        &format!("{context} cacheControlFormat"),
    )?;
    validate_optional_enum(
        object,
        "toolStrictMode",
        &["all_strict", "none"],
        &format!("{context} toolStrictMode"),
    )?;
    validate_optional_enum(
        object,
        "streamMarkupHealingPattern",
        &["kimi", "dsml", "qwen", "thinking"],
        &format!("{context} streamMarkupHealingPattern"),
    )?;
    validate_optional_enum(
        object,
        "promptCacheMode",
        &["none", "automatic", "explicit"],
        &format!("{context} promptCacheMode"),
    )?;

    if let Some(map) = object.get("reasoningEffortMap") {
        validate_string_map(map, &format!("{context} reasoningEffortMap"))?;
    }
    if let Some(map) = object.get("openRouterRouting") {
        validate_routing_map(map, &format!("{context} openRouterRouting"))?;
    }
    if let Some(map) = object.get("vercelGatewayRouting") {
        validate_routing_map(map, &format!("{context} vercelGatewayRouting"))?;
    }
    if let Some(extra_body) = object.get("extraBody") {
        if !extra_body.is_object() {
            return Err(AppError::InvalidInput(format!(
                "{context} extraBody must be an object"
            )));
        }
    }
    for field in [
        "streamIdleTimeoutMs",
        "promptCacheMinimumTokens",
        "promptCacheMaximumCheckpoints",
    ] {
        if let Some(value) = object.get(field) {
            let valid = value
                .as_f64()
                .is_some_and(|number| number.is_finite() && number >= 0.0);
            if !valid {
                return Err(AppError::InvalidInput(format!(
                    "{context} {field} must be a non-negative number"
                )));
            }
        }
    }
    if let Some(when_thinking) = object.get("whenThinking") {
        validate_compat(when_thinking, &format!("{context} whenThinking"))?;
    }
    Ok(())
}

fn validate_string_map(value: &Value, context: &str) -> Result<(), AppError> {
    let object = value
        .as_object()
        .ok_or_else(|| AppError::InvalidInput(format!("{context} must be an object")))?;
    if object.values().any(|value| !value.is_string()) {
        return Err(AppError::InvalidInput(format!(
            "{context} values must be strings"
        )));
    }
    Ok(())
}

fn validate_routing_map(value: &Value, context: &str) -> Result<(), AppError> {
    let object = value
        .as_object()
        .ok_or_else(|| AppError::InvalidInput(format!("{context} must be an object")))?;
    for key in ["only", "order"] {
        if let Some(values) = object.get(key) {
            let values = values.as_array().ok_or_else(|| {
                AppError::InvalidInput(format!("{context} {key} must be an array"))
            })?;
            if values.iter().any(|value| !value.is_string()) {
                return Err(AppError::InvalidInput(format!(
                    "{context} {key} values must be strings"
                )));
            }
        }
    }
    Ok(())
}

/// Validate a provider before CC Switch writes it into OMP's live registry.
/// OMP permits keyless custom model providers only when authentication is
/// explicitly disabled with `none`; a full custom model list otherwise needs a
/// configured API key. Existing native entries are still imported verbatim so
/// built-in/provider-extension credentials remain intact.
pub(crate) fn validate_provider_for_live_write(
    provider_key: &str,
    config: &Value,
) -> Result<(), AppError> {
    validate_provider_node(provider_key, config)
}

pub(crate) fn validate_api_protocol(api: &str) -> Result<(), AppError> {
    if OMP_API_PROTOCOLS.contains(&api.trim()) {
        Ok(())
    } else {
        Err(AppError::InvalidInput(format!(
            "Unsupported OMP API protocol '{}'. Supported: {}",
            api.trim(),
            OMP_API_PROTOCOLS.join(", ")
        )))
    }
}

pub(crate) fn provider_base_url(config: &Value) -> Result<String, AppError> {
    let provider = config.as_object().ok_or_else(|| {
        AppError::InvalidInput("OMP provider configuration must be an object".to_string())
    })?;
    if let Some(url) = nonempty_string(provider.get("baseUrl")) {
        return Ok(url.to_string());
    }
    if let Some(url) = provider
        .get("models")
        .and_then(Value::as_array)
        .and_then(|models| {
            models
                .iter()
                .find_map(|model| nonempty_string(model.get("baseUrl")))
        })
    {
        return Ok(url.to_string());
    }

    // OMP's built-in discovery providers are usable without an explicit
    // provider baseUrl. Mirror the native defaults so CC Switch model fetch,
    // stream checks, and TUI forms can address those local gateways too.
    if let Some(discovery_type) = provider
        .get("discovery")
        .and_then(Value::as_object)
        .and_then(|discovery| discovery.get("type"))
        .and_then(Value::as_str)
    {
        let env_url = |name: &str| resolve_omp_env_value(name);
        let default = match discovery_type {
            "ollama" => env_url("OLLAMA_BASE_URL")
                .and_then(|value| normalize_ollama_base_url(&value))
                .or_else(|| {
                    resolve_omp_env_value("OLLAMA_HOST")
                        .and_then(|value| normalize_ollama_host_env(&value))
                })
                .unwrap_or_else(|| "http://127.0.0.1:11434".to_string()),
            "llama.cpp" => {
                env_url("LLAMA_CPP_BASE_URL").unwrap_or_else(|| "http://127.0.0.1:8080".to_string())
            }
            "lm-studio" => env_url("LM_STUDIO_BASE_URL")
                .unwrap_or_else(|| "http://127.0.0.1:1234/v1".to_string()),
            "openai-models-list" => "http://127.0.0.1:1234/v1".to_string(),
            // OMP's proxy discovery is a local OpenAI-compatible gateway;
            // the native implementation supplies this endpoint when the
            // provider omits an explicit baseUrl.
            "proxy" => "http://127.0.0.1:1234/v1".to_string(),
            "litellm" => env_url("LITELLM_BASE_URL")
                .unwrap_or_else(|| "http://localhost:4000/v1".to_string()),
            _ => {
                return Err(AppError::InvalidInput(
                    "OMP provider has no request URL".to_string(),
                ))
            }
        };
        return Ok(default);
    }

    Err(AppError::InvalidInput(
        "OMP provider has no request URL".to_string(),
    ))
}

/// Normalize Ollama's host-style environment variable into the URL form used
/// by its model discovery client. OLLAMA_HOST accepts values such as
/// `127.0.0.1:11434`, `:11434`, `//ollama.example`, and full HTTP(S) URLs;
/// discovery always addresses the host root and supplies the native HTTP
/// default port when one is omitted.
fn normalize_ollama_host_env(value: &str) -> Option<String> {
    const DEFAULT_PORT: u16 = 11434;

    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else if trimmed.starts_with("//") {
        format!("http:{trimmed}")
    } else if trimmed.starts_with(':') {
        format!("http://127.0.0.1{trimmed}")
    } else {
        format!("http://{trimmed}")
    };

    let mut parsed = Url::parse(&candidate).ok()?;
    if parsed.host_str().is_none() || !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    if parsed.port().is_none() && parsed.scheme() == "http" {
        parsed.set_port(Some(DEFAULT_PORT)).ok()?;
    }

    // OMP's implicit discovery URL consists only of scheme, host, and port;
    // paths, credentials, queries, and fragments in OLLAMA_HOST are ignored.
    let host = parsed.host_str()?;
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    let authority = parsed
        .port()
        .map(|port| format!("{host}:{port}"))
        .unwrap_or(host);
    Some(format!("{}://{authority}", parsed.scheme()))
}

pub(crate) fn normalize_ollama_base_url(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut parsed = Url::parse(trimmed).ok()?;
    if parsed.host_str().is_none() || !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }

    // OMP's OLLAMA_BASE_URL is an origin override, unlike OLLAMA_HOST's
    // host-style syntax. Preserve an explicit port but discard path, query,
    // fragment, and credentials before discovery appends /api/tags.
    parsed.set_username("").ok()?;
    parsed.set_password(None).ok()?;
    parsed.set_path("");
    parsed.set_query(None);
    parsed.set_fragment(None);
    Some(parsed.to_string().trim_end_matches('/').to_string())
}

/// Return the effective timeout for an OMP provider's native discovery.
///
/// OMP defaults discovery probes to ten seconds.  Preserve that default when
/// a discovery block is present but does not specify `timeoutMs`, round
/// fractional values up to the next millisecond, and cap the result so a
/// native file cannot make CC Switch block indefinitely.
pub(crate) fn omp_discovery_timeout_ms(config: &Value) -> Option<u64> {
    let discovery = config.get("discovery")?.as_object()?;
    let timeout = discovery
        .get("timeoutMs")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| value.ceil() as u64)
        .unwrap_or(OMP_DEFAULT_DISCOVERY_TIMEOUT_MS);
    Some(timeout.clamp(1, OMP_MAX_DISCOVERY_TIMEOUT_MS))
}

/// Resolve an OMP discovery environment variable with shell variables taking
/// precedence over the dotenv files OMP loads for the current project/profile.
fn resolve_omp_env_value(name: &str) -> Option<String> {
    env_value_exact(name)
        .filter(|value| !value.trim().is_empty())
        .or_else(|| resolve_omp_dotenv_value(name))
}

/// Resolve an OMP `apiKey` value using OMP's env-name-or-literal semantics.
///
/// OMP treats the configured value as `!command`, an environment variable name,
/// or a literal string (in that order). Command output is bounded and cached so
/// native provider credentials behave consistently without repeated execution.
pub(crate) fn resolve_api_key(value: Option<&Value>) -> Option<String> {
    let configured = value.and_then(Value::as_str)?.trim();
    if configured.is_empty() {
        return None;
    }
    if let Some(command) = configured.strip_prefix('!') {
        return resolve_omp_command_value(command.trim());
    }
    env_value_exact(configured)
        .filter(|resolved| !resolved.trim().is_empty())
        .or_else(|| resolve_omp_dotenv_value(configured))
        .or_else(|| Some(configured.to_string()))
}

/// Resolve OMP's `!command` secret form with the same bounded behavior as the
/// native runtime: shell execution, trimmed stdout, a ten-second timeout, and
/// process-lifetime caching of successful values.
pub(crate) fn resolve_header_value(value: &str) -> Option<String> {
    let value = value.trim();
    if let Some(command) = value.strip_prefix('!') {
        return resolve_omp_command_value(command.trim());
    }
    env_value_exact(value)
        .filter(|resolved| !resolved.trim().is_empty())
        .or_else(|| resolve_omp_dotenv_value(value))
        .or_else(|| Some(value.to_string()))
}

/// Read a user-supplied environment variable name without Windows' implicit
/// case folding. OMP uses an exact-name lookup for configured API-key/header
/// references so a literal such as `public` cannot be hijacked by the system's
/// differently-cased `PUBLIC` variable.
fn env_value_exact(name: &str) -> Option<String> {
    #[cfg(windows)]
    {
        return std::env::vars()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value);
    }

    #[cfg(not(windows))]
    {
        std::env::var(name).ok()
    }
}

fn resolve_omp_command_value(command: &str) -> Option<String> {
    if command.is_empty() {
        return None;
    }
    if let Ok(cache) = COMMAND_VALUE_CACHE.lock() {
        if let Some(value) = cache.get(command) {
            return Some(value.clone());
        }
    }
    if let Ok(mut failures) = COMMAND_FAILURE_CACHE.lock() {
        if let Some(failed_at) = failures.get(command).copied() {
            if failed_at.elapsed() < Duration::from_secs(30) {
                return None;
            }
            failures.remove(command);
        }
    }

    let result = (|| {
        let mut child = spawn_omp_shell(command, Stdio::piped()).ok()?;
        let stdout = child.stdout.take()?;
        let (output_tx, output_rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = stdout
                .take((MAX_COMMAND_OUTPUT_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .map(|_| bytes);
            let _ = output_tx.send(result);
        });

        let mut captured_output = None;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(result) = output_rx.try_recv() {
                match result {
                    Ok(bytes) if bytes.len() <= MAX_COMMAND_OUTPUT_BYTES => {
                        captured_output = Some(bytes);
                    }
                    _ => {
                        cleanup_omp_command(&mut child);
                        return None;
                    }
                }
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    // A shell can leave background descendants behind even
                    // after it exits successfully. Kill the dedicated process
                    // group before collecting output so command-backed secrets
                    // cannot leak long-lived processes.
                    cleanup_omp_command(&mut child);
                    if !status.success() {
                        return None;
                    }
                    if captured_output.is_none() {
                        let bytes = output_rx
                            .recv_timeout(Duration::from_secs(1))
                            .ok()
                            .and_then(Result::ok)?;
                        if bytes.len() > MAX_COMMAND_OUTPUT_BYTES {
                            return None;
                        }
                        captured_output = Some(bytes);
                    }
                    break;
                }
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(25));
                }
                Ok(None) => {
                    cleanup_omp_command(&mut child);
                    return None;
                }
                Err(_) => {
                    cleanup_omp_command(&mut child);
                    return None;
                }
            }
        }

        let bytes = captured_output?;
        let value = String::from_utf8(bytes).ok()?.trim().to_string();
        (!value.is_empty()).then_some(value)
    })();

    match result {
        Some(value) => {
            if let Ok(mut cache) = COMMAND_VALUE_CACHE.lock() {
                cache.insert(command.to_string(), value.clone());
            }
            if let Ok(mut failures) = COMMAND_FAILURE_CACHE.lock() {
                failures.remove(command);
            }
            Some(value)
        }
        None => {
            if let Ok(mut failures) = COMMAND_FAILURE_CACHE.lock() {
                failures.insert(command.to_string(), Instant::now());
            }
            None
        }
    }
}

fn spawn_omp_shell(command: &str, stdout: Stdio) -> std::io::Result<Child> {
    #[cfg(unix)]
    let mut shell = Command::new("/bin/sh");
    #[cfg(windows)]
    let mut shell = Command::new("cmd.exe");

    #[cfg(unix)]
    // Put the shell and every descendant it starts into a private process
    // group. This lets timeout/completion cleanup terminate the whole command
    // tree rather than only the direct shell process.
    unsafe {
        shell.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    // OMP eagerly folds project/agent/config-root/home dotenv files into the
    // Bun process environment before resolving `!command` values. Mirror that
    // behavior here while preserving the parent process environment's
    // precedence for explicitly exported variables.
    let dotenv_env = resolve_omp_dotenv_environment();

    #[cfg(unix)]
    shell.arg("-c").arg(command);
    #[cfg(windows)]
    // Bun's shell-backed command execution uses the native Windows command
    // interpreter. `sh` is not guaranteed to be installed on Windows, while
    // cmd.exe is always available and understands the same `%VAR%` expansion
    // users commonly rely on in OMP `!command` credentials.
    shell.args(["/D", "/S", "/C", command]);

    shell
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::null())
        .envs(dotenv_env)
        .spawn()
}

fn cleanup_omp_command(child: &mut Child) {
    #[cfg(unix)]
    {
        // The shell's PID is also the process-group ID created by setsid.
        // A negative PID targets the complete group; ESRCH is harmless when
        // the shell already exited and no descendants remain.
        let pgid = child.id() as libc::pid_t;
        unsafe {
            let _ = libc::kill(-pgid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Resolve a named OMP environment variable using the same dotenv locations
/// that OMP loads for an agent process. The shell environment wins, followed
/// by the project, agent, OMP config-root, and home dotenv files.
fn resolve_omp_dotenv_value(name: &str) -> Option<String> {
    resolve_omp_dotenv_environment()
        .get(name)
        .filter(|value| !value.trim().is_empty())
        .cloned()
}

/// Load OMP's dotenv files in native precedence order. Values already present
/// in the process environment win; earlier files win over later files. The
/// dotenv parser mirrors OMP's compatibility behavior for the documented
/// `OMP_PROFILE`/`PI_PROFILE` pair without inventing aliases for other names.
fn resolve_omp_dotenv_environment() -> HashMap<String, String> {
    let home = get_home_dir();
    // Resolve path selectors through the same two-pass dotenv loader used by
    // native path discovery. In particular, PI_CONFIG_DIR may be supplied by
    // ~/.env; deriving the config root from process-only variables would then
    // omit the selected custom root's own .env file.
    let path_env = resolve_omp_path_environment();
    let config_dir = path_env
        .get("PI_CONFIG_DIR")
        .filter(|value| !value.is_empty())
        .cloned()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".omp"));
    let config_root = omp_config_root(&home, config_dir);
    let profile = effective_profile(&path_env).ok().flatten();
    let profile_root = profile
        .as_ref()
        .map(|profile| config_root.join("profiles").join(profile))
        .unwrap_or_else(|| config_root.clone());
    // If native path resolution rejects an invalid profile or directory,
    // avoid guessing a fallback agent root for credential commands. The
    // profile/config dotenv layers below remain safe to inspect, while a
    // malformed selector cannot redirect command execution to an unrelated
    // directory.
    let agent_dir = get_omp_agent_dir().ok();
    let mut paths = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        paths.push(cwd.join(".env"));
    }
    if let Some(agent_dir) = agent_dir {
        paths.push(agent_dir.join(".env"));
    }
    paths.push(profile_root.join(".env"));
    paths.push(home.join(".env"));

    let process_env: HashMap<String, String> = std::env::vars().collect();
    // Preserve the process environment (which OMP inherits). Only the
    // documented OMP_PROFILE/PI_PROFILE pair is mirrored; arbitrary
    // OMP_* variables are not path aliases in the native executable.
    let mut resolved = process_env.clone();
    let process_aliases = process_env
        .iter()
        .filter(|(key, _)| *key == "OMP_PROFILE")
        .map(|(_, value)| ("PI_PROFILE".to_string(), value.clone()))
        .collect::<Vec<_>>();
    for (alias, value) in process_aliases {
        // An explicitly exported PI_* value wins over the compatibility
        // alias, matching OMP's credential resolver and avoiding surprising
        // replacement of a user's legacy variable.
        resolved.entry(alias).or_insert(value);
    }
    let mut seen = std::collections::HashSet::new();
    for path in paths {
        if !seen.insert(path.clone()) {
            continue;
        }
        for (key, value) in read_omp_dotenv_file(&path) {
            if value.trim().is_empty() {
                continue;
            }
            if process_env.contains_key(&key) || resolved.contains_key(&key) {
                continue;
            }
            resolved.insert(key.clone(), value.clone());
            // `read_omp_dotenv_file` already materializes the documented
            // profile compatibility alias. No other aliases are synthesized.
        }
    }
    resolved
}

fn read_omp_dotenv_file(path: &Path) -> HashMap<String, String> {
    let Ok(contents) = read_file_limited(path, "OMP dotenv") else {
        return HashMap::new();
    };
    let Ok(contents) = String::from_utf8(contents) else {
        return HashMap::new();
    };
    let mut values = HashMap::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // OMP accepts `export` followed by any horizontal whitespace (spaces
        // or tabs), not just one literal ASCII space.
        let line = if let Some(rest) = line.strip_prefix("export") {
            if rest
                .as_bytes()
                .first()
                .is_some_and(|byte| *byte == b' ' || *byte == b'\t')
            {
                rest.trim_start()
            } else {
                line
            }
        } else {
            line
        };
        let Some((raw_name, raw_value)) = line.split_once('=') else {
            continue;
        };
        let key = raw_name.trim();
        let mut chars = key.chars();
        let valid_name = chars
            .next()
            .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
            && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric());
        if !valid_name {
            continue;
        }
        let value = parse_omp_dotenv_value(raw_value.trim());
        // OMP drops dotenv entries containing NUL bytes; retaining them would
        // later make HeaderValue construction fail (or produce divergent
        // credential resolution).
        if value.contains('\0') {
            continue;
        }
        values.insert(key.to_string(), value);
    }

    // Only the profile selector has a documented OMP_/PI_ compatibility pair.
    // OMP_PROFILE is canonical and overrides PI_PROFILE within the same file.
    if let Some(value) = values.get("OMP_PROFILE").cloned() {
        values.insert("PI_PROFILE".to_string(), value);
    }

    values
}

#[cfg(test)]
fn read_omp_dotenv_value(path: &Path, name: &str) -> Option<String> {
    let values = read_omp_dotenv_file(path);
    values
        .get(name)
        .filter(|value| !value.trim().is_empty())
        .cloned()
}

fn parse_omp_dotenv_value(raw: &str) -> String {
    if let Some(quote @ ('"' | '\'' | '`')) = raw.chars().next() {
        let mut close = raw[1..].find(quote).map(|index| index + 1);
        while let Some(index) = close {
            if index == 0 || raw.as_bytes().get(index - 1) != Some(&b'\\') {
                return raw[1..index].to_string();
            }
            close = raw[index + quote.len_utf8()..]
                .find(quote)
                .map(|next| index + quote.len_utf8() + next);
        }
        // Match OMP's permissive dotenv parser for an unterminated quote:
        // strip the opening quote instead of treating it as part of the
        // secret value.
        return raw[quote.len_utf8()..].to_string();
    }

    raw.split_once(" #")
        .map(|(value, _)| value.trim_end())
        .unwrap_or(raw)
        .to_string()
}

/// Resolve a provider credential for model-list requests. OMP's Gemini CLI
/// integration stores a JSON credential blob; its model endpoint expects the
/// embedded OAuth access token rather than the serialized object.
pub(crate) fn resolve_api_key_for_protocol(
    value: Option<&Value>,
    protocol: Option<&str>,
) -> Option<String> {
    let resolved = resolve_api_key(value)?;
    if protocol == Some("google-gemini-cli") {
        if let Ok(blob) = serde_json::from_str::<Value>(&resolved) {
            if let Some(token) = blob
                .get("access_token")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|token| !token.is_empty())
            {
                return Some(token.to_string());
            }
        }
    }
    Some(resolved)
}

pub(crate) fn is_valid_request_url(raw: &str) -> bool {
    url::Url::parse(raw.trim())
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
}

fn lock_models_file() -> Result<MutexGuard<'static, ()>, AppError> {
    MODELS_FILE_LOCK
        .lock()
        .map_err(|error| AppError::Config(format!("OMP models file lock is poisoned: {error}")))
}

fn read_omp_native_providers_locked(path: &Path) -> Result<IndexMap<String, Value>, AppError> {
    let document = read_models_document(path)?;
    let providers = providers(&document, path)?;
    Ok(providers
        .iter()
        .map(|(provider_key, config)| (provider_key.clone(), config.clone()))
        .collect())
}

fn read_models_document(path: &Path) -> Result<Value, AppError> {
    read_models_document_with_revision(path).map(|(document, _)| document)
}

fn read_config_document(path: &Path) -> Result<Value, AppError> {
    read_config_document_with_revision(path).map(|(document, _)| document)
}

fn read_config_document_with_revision(path: &Path) -> Result<(Value, String), AppError> {
    if !path.exists() {
        return Ok((
            Value::Object(Map::new()),
            MISSING_CONFIG_REVISION.to_string(),
        ));
    }
    let bytes = read_file_limited(path, "OMP config")?;
    let revision = revision(&bytes);
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok((Value::Object(Map::new()), revision));
    }
    let document = parse_yaml_value(path, "OMP config", bytes)?;
    Ok((document, revision))
}

fn read_models_document_with_revision(path: &Path) -> Result<(Value, String), AppError> {
    if !path.exists() {
        return Ok((
            Value::Object(Map::new()),
            MISSING_MODELS_REVISION.to_string(),
        ));
    }
    let bytes = read_file_limited(path, "OMP models")?;
    let revision = revision(&bytes);
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok((Value::Object(Map::new()), revision));
    }
    let document = parse_yaml_value(path, "OMP models", bytes)?;
    Ok((document, revision))
}

fn read_yaml_text_with_revision(path: &Path, label: &str) -> Result<(String, String), AppError> {
    let bytes = match fs::File::open(path) {
        Ok(_) => read_file_limited(path, label)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Checked writers use a stable sentinel for an absent native
            // document. A content hash of the empty byte string made the
            // first `model add`/TUI save fail its compare-and-swap check.
            let missing_revision = if label == "OMP models" {
                MISSING_MODELS_REVISION
            } else {
                MISSING_CONFIG_REVISION
            };
            return Ok((String::new(), missing_revision.to_string()));
        }
        Err(error) => return Err(AppError::io(path, error)),
    };
    let revision = revision(&bytes);
    let text = String::from_utf8(bytes).map_err(|error| {
        AppError::Config(format!(
            "{label} must be UTF-8 ({}): {error}",
            path.display()
        ))
    })?;
    Ok((text, revision))
}

fn read_file_limited(path: &Path, label: &str) -> Result<Vec<u8>, AppError> {
    let file = fs::File::open(path).map_err(|error| AppError::io(path, error))?;
    let metadata = file.metadata().map_err(|error| AppError::io(path, error))?;
    if metadata.len() > MAX_OMP_FILE_BYTES {
        return Err(AppError::InvalidInput(format!(
            "{label} file exceeds the 1 MiB limit: {}",
            path.display()
        )));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_OMP_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| AppError::io(path, error))?;
    if bytes.len() as u64 > MAX_OMP_FILE_BYTES {
        return Err(AppError::InvalidInput(format!(
            "{label} file exceeds the 1 MiB limit: {}",
            path.display()
        )));
    }
    Ok(bytes)
}

fn parse_yaml_value(path: &Path, label: &str, bytes: Vec<u8>) -> Result<Value, AppError> {
    let source = String::from_utf8(bytes).map_err(|error| {
        AppError::Config(format!(
            "{label} file must be UTF-8 ({}): {error}",
            path.display()
        ))
    })?;
    serde_yaml::from_str(&source).map_err(|error| {
        AppError::Config(format!(
            "{label} file is not valid YAML ({}): {error}",
            path.display()
        ))
    })
}

fn providers<'a>(document: &'a Value, path: &Path) -> Result<&'a Map<String, Value>, AppError> {
    let root = document.as_object().ok_or_else(|| {
        AppError::Config(format!(
            "OMP models root must be an object: {}",
            path.display()
        ))
    })?;
    match root.get("providers") {
        None => Ok(empty_json_object()),
        Some(Value::Object(providers)) => Ok(providers),
        Some(_) => Err(AppError::Config(format!(
            "OMP models 'providers' must be an object: {}",
            path.display()
        ))),
    }
}

/// Validate the managed shape of the `models.yml` root.
///
/// OMP's `ModelsConfigSchema` keeps unknown root keys by default. Preserve
/// those fields so a provider edit cannot discard metadata introduced by a
/// newer native release; only the provider map's type is constrained because
/// it is the portion CC Switch mutates.
fn validate_omp_models_root(document: &Value, path: &Path) -> Result<(), AppError> {
    let root = document.as_object().ok_or_else(|| {
        AppError::InvalidInput(format!(
            "OMP models root must be an object: {}",
            path.display()
        ))
    })?;
    if let Some(value) = root.get("providers") {
        if !value.is_object() {
            return Err(AppError::InvalidInput(format!(
                "OMP models 'providers' must be an object: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn providers_mut<'a>(
    document: &'a mut Value,
    path: &Path,
) -> Result<&'a mut Map<String, Value>, AppError> {
    let root = document.as_object_mut().ok_or_else(|| {
        AppError::Config(format!(
            "OMP models root must be an object: {}",
            path.display()
        ))
    })?;
    let value = root
        .entry("providers".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    value.as_object_mut().ok_or_else(|| {
        AppError::Config(format!(
            "OMP models 'providers' must be an object: {}",
            path.display()
        ))
    })
}

fn empty_json_object() -> &'static Map<String, Value> {
    static EMPTY: LazyLock<Map<String, Value>> = LazyLock::new(Map::new);
    &EMPTY
}

fn write_models_document(
    path: &Path,
    document: &Value,
    expected_revision: &str,
) -> Result<(), AppError> {
    validate_omp_models_root(document, path)?;
    let bytes = serde_yaml::to_string(document)
        .map_err(|error| AppError::Config(format!("failed to serialize OMP models: {error}")))?
        .into_bytes();
    ensure_private_omp_parent(path)?;
    ensure_models_revision(path, expected_revision)?;
    atomic_write_private(path, &bytes)
}

fn write_config_document(
    path: &Path,
    document: &Value,
    expected_revision: &str,
) -> Result<(), AppError> {
    let bytes = serde_yaml::to_string(document)
        .map_err(|error| AppError::Config(format!("failed to serialize OMP config: {error}")))?
        .into_bytes();
    ensure_private_omp_parent(path)?;
    ensure_config_revision(path, expected_revision)?;
    atomic_write_private(path, &bytes)
}

fn provider_configs_equal_ignoring_name(left: &Value, right: &Value) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    strip_native_name(&mut left);
    strip_native_name(&mut right);
    left == right
}

fn strip_native_name(config: &mut Value) {
    if let Some(object) = config.as_object_mut() {
        object.remove("name");
    }
}

fn ensure_models_revision(path: &Path, expected_revision: &str) -> Result<(), AppError> {
    let actual_revision = match fs::File::open(path) {
        Ok(_) => revision(&read_file_limited(path, "OMP models")?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            MISSING_MODELS_REVISION.to_string()
        }
        Err(error) => return Err(AppError::io(path, error)),
    };
    if actual_revision == expected_revision {
        Ok(())
    } else {
        Err(AppError::Conflict(format!(
            "OMP models.yml changed outside CC Switch: {}",
            path.display()
        )))
    }
}

fn ensure_config_revision(path: &Path, expected_revision: &str) -> Result<(), AppError> {
    let actual_revision = match fs::File::open(path) {
        Ok(_) => revision(&read_file_limited(path, "OMP config")?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            MISSING_CONFIG_REVISION.to_string()
        }
        Err(error) => return Err(AppError::io(path, error)),
    };
    if actual_revision == expected_revision {
        Ok(())
    } else {
        Err(AppError::Conflict(format!(
            "OMP config.yml changed outside CC Switch: {}",
            path.display()
        )))
    }
}

/// Guard the legacy JSONC source while materializing its first native YAML
/// overlay. The target YAML is intentionally absent at that point, so its
/// `missing` revision alone cannot detect a concurrent edit to the source
/// document that was copied into the new file.
fn ensure_omp_legacy_revision(path: &Path, expected_revision: &str) -> Result<(), AppError> {
    let actual_revision = match fs::File::open(path) {
        Ok(_) => revision(&read_file_limited(path, "OMP legacy settings")?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(AppError::Conflict(format!(
                "OMP legacy settings changed outside CC Switch: {}",
                path.display()
            )));
        }
        Err(error) => return Err(AppError::io(path, error)),
    };
    if actual_revision == expected_revision {
        Ok(())
    } else {
        Err(AppError::Conflict(format!(
            "OMP legacy settings changed outside CC Switch: {}",
            path.display()
        )))
    }
}

/// Guard the legacy `models.json` source while materializing its first native
/// YAML overlay. A missing `models.yml` revision alone cannot detect a
/// concurrent edit to the JSONC document that was parsed into the new file.
fn ensure_omp_legacy_models_revision(path: &Path, expected_revision: &str) -> Result<(), AppError> {
    let actual_revision = match fs::File::open(path) {
        Ok(_) => revision(&read_file_limited(path, "OMP legacy models")?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(AppError::Conflict(format!(
                "OMP legacy models changed outside CC Switch: {}",
                path.display()
            )));
        }
        Err(error) => return Err(AppError::io(path, error)),
    };
    if actual_revision == expected_revision {
        Ok(())
    } else {
        Err(AppError::Conflict(format!(
            "OMP legacy models changed outside CC Switch: {}",
            path.display()
        )))
    }
}

fn revision(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Ensure a user-level OMP managed file is created below a private directory.
///
/// Models/config files and user-level prompt overrides share the same native
/// agent directory. Keep the permission check in one place so a prompt write
/// cannot accidentally weaken the privacy guarantees applied to credentials.
pub(crate) fn ensure_private_omp_parent(path: &Path) -> Result<(), AppError> {
    let parent = path.parent().ok_or_else(|| {
        AppError::Config(format!(
            "OMP managed path has no parent directory: {}",
            path.display()
        ))
    })?;

    // Capture directories that do not exist before creation so only those
    // newly materialized below an existing root receive private permissions.
    // Walking with symlink_metadata (rather than exists/metadata) lets us
    // reject dangling or redirected ancestors before create_dir_all follows
    // them.
    let mut missing = Vec::new();
    let mut cursor = parent;
    loop {
        match fs::symlink_metadata(cursor) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(cursor.to_path_buf());
                let Some(next) = cursor.parent() else {
                    break;
                };
                if next == cursor {
                    break;
                }
                cursor = next;
            }
            Err(error) => return Err(AppError::io(cursor, error)),
        }
    }
    fs::create_dir_all(parent).map_err(|source| AppError::io(parent, source))?;

    // Validate every existing ancestor after creation as well. This closes
    // the common symlink/permission gap where an intermediate component was
    // absent during the initial check but redirected before the write.
    let mut ancestors = Vec::new();
    let mut cursor = parent;
    loop {
        ancestors.push(cursor.to_path_buf());
        let Some(next) = cursor.parent() else {
            break;
        };
        if next == cursor {
            break;
        }
        cursor = next;
    }

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    for ancestor in ancestors {
        let metadata =
            fs::symlink_metadata(&ancestor).map_err(|source| AppError::io(&ancestor, source))?;
        if metadata.file_type().is_symlink() {
            return Err(AppError::InvalidInput(format!(
                "OMP managed path cannot contain a symlinked directory: {}",
                ancestor.display()
            )));
        }
        if !metadata.is_dir() {
            return Err(AppError::InvalidInput(format!(
                "OMP managed path component is not a directory: {}",
                ancestor.display()
            )));
        }

        #[cfg(unix)]
        {
            let mut mode = metadata.permissions().mode() & 0o7777;
            if missing.iter().any(|path| path == &ancestor) {
                fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o700))
                    .map_err(|source| AppError::io(&ancestor, source))?;
                mode = 0o700;
            }
            let writable = mode & 0o022 != 0;
            // A sticky world-writable ancestor such as /tmp is safe as a
            // shared container root; the managed directory itself must still
            // remain private. Any other group/other-writable ancestor can let
            // another user redirect a missing managed component before the
            // write, so reject it even when only the group write bit is set.
            let shared_sticky_root = mode & 0o1000 != 0;
            if (ancestor == parent && writable)
                || (ancestor != parent && writable && !shared_sticky_root)
            {
                return Err(AppError::InvalidInput(format!(
                    "OMP managed directory cannot be group/other writable: {} ({mode:04o}); run chmod 700 {}",
                    ancestor.display(),
                    ancestor.display()
                )));
            }
        }
    }

    Ok(())
}

fn nonempty_string(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard, OnceLock};

    static CURRENT_DIR_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    pub(crate) struct TestAgentDir {
        _dir: Option<tempfile::TempDir>,
        previous: Option<PathBuf>,
    }

    impl TestAgentDir {
        pub(crate) fn new() -> Self {
            let dir = tempfile::tempdir().expect("create OMP test directory");
            let agent_dir = dir.path().join("agent");
            std::fs::create_dir_all(&agent_dir).expect("create OMP test agent directory");
            // The production writer rejects group/other-writable ancestors;
            // tempfile directories inherit a permissive mode on some hosts,
            // so make the isolated test root match a private user config root.
            restrict_test_directory(dir.path());
            restrict_test_directory(&agent_dir);
            Self::set(agent_dir, Some(dir))
        }

        pub(crate) fn at(agent_dir: &Path) -> Self {
            if let Err(error) = std::fs::create_dir_all(agent_dir) {
                panic!("create OMP test agent directory: {error}");
            }
            if let Some(parent) = agent_dir.parent() {
                restrict_test_directory(parent);
            }
            restrict_test_directory(agent_dir);
            Self::set(agent_dir.to_path_buf(), None)
        }

        fn set(agent_dir: PathBuf, dir: Option<tempfile::TempDir>) -> Self {
            let previous = super::TEST_AGENT_DIR
                .lock()
                .expect("lock OMP test directory")
                .replace(agent_dir);
            Self {
                _dir: dir,
                previous,
            }
        }
    }

    fn restrict_test_directory(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
                .expect("restrict OMP test agent directory");
        }
    }

    impl Drop for TestAgentDir {
        fn drop(&mut self) {
            *super::TEST_AGENT_DIR
                .lock()
                .expect("lock OMP test directory") = self.previous.take();
        }
    }

    /// Process-wide current-directory guard for tests that exercise OMP's
    /// project-layer resolution. `std::env::set_current_dir` is process-global,
    /// so tests must serialize it and restore the directory even when an
    /// assertion panics; otherwise a dropped `TempDir` leaves later tests in a
    /// non-existent working directory.
    pub(crate) struct CurrentDirGuard {
        _lock: MutexGuard<'static, ()>,
        previous: PathBuf,
    }

    impl CurrentDirGuard {
        pub(crate) fn change_to(path: &Path) -> Self {
            let lock = CURRENT_DIR_LOCK
                .get_or_init(|| Mutex::new(()))
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let previous = std::env::current_dir().expect("read current directory");
            // Project-layer tests often create managed `.omp` files below a
            // `tempfile` root. Match a real private project root before the
            // production permission checks walk that ancestor.
            restrict_test_directory(path);
            std::env::set_current_dir(path).expect("switch current directory");
            Self {
                _lock: lock,
                previous,
            }
        }
    }

    impl Drop for CurrentDirGuard {
        fn drop(&mut self) {
            std::env::set_current_dir(&self.previous).expect("restore current directory");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use serial_test::serial;

    fn provider() -> Value {
        json!({
            "name": "Example",
            "baseUrl": "https://aomp.example.com/v1",
            "api": "openai-completions",
            "apiKey": "secret",
            "models": [{"id": "example-model"}]
        })
    }

    #[test]
    fn provider_node_accepts_unknown_native_fields() {
        let mut value = provider();
        value["sdkOption"] = json!({"timeout": 30});
        value["models"][0]["compat"] = json!({"supportsDeveloperRole": true});
        validate_provider_node("cc-switch-example", &value).expect("valid provider");
        assert!(validate_provider_node("unknown-only", &json!({"sdkOption": {}})).is_err());

        let custom_scheme = json!({
            "baseUrl": "unix:///var/run/omp.sock",
            "api": "openai-completions",
            "apiKey": "secret",
            "models": [{"id": "socket-model"}]
        });
        validate_provider_node_for_import("socket-provider", &custom_scheme)
            .expect("native import accepts non-HTTP provider schemes");
        assert!(validate_provider_node("socket-provider", &custom_scheme).is_err());
    }

    #[test]
    fn provider_request_metadata_is_a_valid_semantic_override() {
        validate_provider_node(
            "metadata-only",
            &json!({"requestMetadata": {"client": "cc-switch"}}),
        )
        .expect("OMP accepts requestMetadata-only providers");
        assert!(validate_provider_node(
            "invalid-metadata",
            &json!({
                "requestMetadata": ["not-an-object"]
            })
        )
        .is_err());
        assert!(validate_provider_node(
            "invalid-metadata-value",
            &json!({
                "requestMetadata": {"attempt": 1}
            })
        )
        .is_err());

        for (field, value) in [
            ("headers", json!({})),
            ("compat", json!({})),
            ("requestMetadata", json!({})),
        ] {
            let mut config = Map::new();
            config.insert(field.to_string(), value);
            validate_provider_node(&format!("empty-{field}"), &Value::Object(config))
                .expect("OMP accepts explicitly supplied empty override objects");
        }
    }

    #[test]
    fn models_root_preserves_unknown_fields_like_omp_schema() {
        let path = PathBuf::from("/tmp/omp/models.yml");
        assert!(validate_omp_models_root(&json!({"providers": {}}), &path).is_ok());
        assert!(
            validate_omp_models_root(&json!({"providers": {}, "futureSetting": true}), &path)
                .is_ok()
        );
        assert!(validate_omp_models_root(&json!({"providers": []}), &path).is_err());
    }

    #[test]
    fn provider_node_ownership_depends_on_models_json_membership() {
        let mut oauth = provider();
        oauth["oauth"] = json!("anthroompc");
        validate_provider_node("cc-switch-example", &oauth)
            .expect("an explicit models.yml node stays manageable");
        validate_provider_node("anthroompc", &json!({"baseUrl": "https://example.com/v1"}))
            .expect("a built-in provider key may be explicitly configured");
        assert!(validate_provider_node("", &json!({})).is_err());
        assert!(validate_provider_for_live_write("anthroompc", &json!({})).is_err());
        assert!(validate_provider_node("anthroompc", &json!("invalid")).is_err());
    }

    #[test]
    fn extension_owned_provider_without_models_is_importable() {
        validate_provider_node(
            "extension-provider",
            &json!({"extension": {"type": "custom"}}),
        )
        .expect("opaque extension providers do not need model metadata");
    }

    #[test]
    fn relative_agent_directory_matches_omp_cwd_resolution() {
        let resolved = resolve_omp_agent_dir(
            None,
            Some("relative/omp-agent".into()),
            PathBuf::from("default"),
        )
        .expect("relative OMP directory should resolve from cwd");
        assert!(resolved.is_absolute());
        assert!(resolved.ends_with("relative/omp-agent"));
    }

    #[test]
    fn config_dir_absolute_path_is_rebased_under_home() {
        let home = PathBuf::from("/home/example");
        let absolute = PathBuf::from("/tmp/omp-config");
        assert_eq!(
            omp_config_root(&home, absolute),
            home.join("tmp/omp-config")
        );
        assert_eq!(
            omp_config_root(&home, PathBuf::from(".omp")),
            home.join(".omp")
        );
        assert_eq!(
            omp_config_root(&home, PathBuf::from("../shared-omp")),
            PathBuf::from("/home/shared-omp")
        );
    }

    #[test]
    fn profile_names_match_omp_validation_and_windows_reserved_names() {
        assert_eq!(normalize_profile("default").unwrap(), None);
        assert_eq!(
            normalize_profile("work-2.0_a").unwrap().as_deref(),
            Some("work-2.0_a")
        );
        for name in [
            "WORK", "Work", "con", "con.json", "prn", "aux.txt", "nul", "com0", "com9.foo", "lpt0",
            "lpt9.bar",
        ] {
            assert!(
                normalize_profile(name).is_err(),
                "profile should be rejected: {name}"
            );
        }
        assert!(normalize_profile("com10").is_ok());
    }

    #[test]
    #[serial]
    fn default_profile_ignores_inherited_named_profile_agent_override() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(".omp");
        let derived = root.join("profiles").join("work").join("agent");
        let previous_omp_profile = std::env::var_os("OMP_PROFILE");
        let previous_pi_profile = std::env::var_os("PI_PROFILE");
        let previous_agent_dir = std::env::var_os("PI_CODING_AGENT_DIR");
        std::env::set_var("OMP_PROFILE", "default");
        std::env::set_var("PI_PROFILE", "work");
        std::env::set_var("PI_CODING_AGENT_DIR", &derived);
        assert!(is_profile_derived_agent_dir_from_env(
            &root,
            &derived.into_os_string()
        ));
        match previous_omp_profile {
            Some(value) => std::env::set_var("OMP_PROFILE", value),
            None => std::env::remove_var("OMP_PROFILE"),
        }
        match previous_pi_profile {
            Some(value) => std::env::set_var("PI_PROFILE", value),
            None => std::env::remove_var("PI_PROFILE"),
        }
        match previous_agent_dir {
            Some(value) => std::env::set_var("PI_CODING_AGENT_DIR", value),
            None => std::env::remove_var("PI_CODING_AGENT_DIR"),
        }
    }

    #[test]
    fn official_environment_directory_precedes_settings_override() {
        let temp = tempfile::tempdir().expect("tempdir");
        let settings_dir = temp.path().join("settings-agent");
        let env_dir = temp.path().join("env-agent");

        assert_eq!(
            resolve_omp_agent_dir(
                Some(settings_dir),
                Some(env_dir.clone().into_os_string()),
                temp.path().join("default-agent"),
            )
            .expect("resolve OMP directory"),
            env_dir
        );
    }

    #[test]
    #[serial]
    fn dotenv_profile_keeps_explicit_agent_directory_override() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let _cwd = test_support::CurrentDirGuard::change_to(home.path());
        let previous = [
            ("OMP_PROFILE", std::env::var_os("OMP_PROFILE")),
            ("PI_PROFILE", std::env::var_os("PI_PROFILE")),
            (
                "PI_CODING_AGENT_DIR",
                std::env::var_os("PI_CODING_AGENT_DIR"),
            ),
            ("PI_CONFIG_DIR", std::env::var_os("PI_CONFIG_DIR")),
        ];
        std::env::remove_var("OMP_PROFILE");
        std::env::remove_var("PI_PROFILE");
        std::env::remove_var("PI_CODING_AGENT_DIR");
        std::env::remove_var("PI_CONFIG_DIR");
        fs::write(
            home.path().join(".env"),
            "OMP_PROFILE=omp-dotenv\nPI_CODING_AGENT_DIR=dotenv-agent\nPI_CONFIG_DIR=.dotenvcfg\n",
        )
        .expect("write project dotenv");

        assert_eq!(
            get_omp_agent_dir().expect("resolve dotenv agent directory"),
            home.path().join("dotenv-agent")
        );

        std::env::set_var("PI_CODING_AGENT_DIR", "proc-agent");
        assert_eq!(
            get_omp_agent_dir().expect("resolve process agent directory"),
            home.path().join("proc-agent")
        );

        for (key, value) in previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }

    #[test]
    #[serial]
    fn dotenv_profile_does_not_select_an_omp_profile() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let _cwd = test_support::CurrentDirGuard::change_to(home.path());
        let previous = [
            ("OMP_PROFILE", std::env::var_os("OMP_PROFILE")),
            ("PI_PROFILE", std::env::var_os("PI_PROFILE")),
            (
                "PI_CODING_AGENT_DIR",
                std::env::var_os("PI_CODING_AGENT_DIR"),
            ),
            ("PI_CONFIG_DIR", std::env::var_os("PI_CONFIG_DIR")),
        ];
        for (key, _) in &previous {
            std::env::remove_var(key);
        }
        fs::write(home.path().join(".env"), "OMP_PROFILE=dotenv-only\n")
            .expect("write dotenv profile");

        assert_eq!(
            get_omp_shared_config_agent_dir().expect("resolve default OMP profile"),
            home.path().join(".omp/agent")
        );

        for (key, value) in previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }

    #[test]
    #[serial]
    fn named_profile_does_not_read_base_config_dotenv_on_second_pass() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let _cwd = test_support::CurrentDirGuard::change_to(home.path());
        let previous = [
            ("OMP_PROFILE", std::env::var_os("OMP_PROFILE")),
            ("PI_PROFILE", std::env::var_os("PI_PROFILE")),
            ("PI_CONFIG_DIR", std::env::var_os("PI_CONFIG_DIR")),
            (
                "PI_CODING_AGENT_DIR",
                std::env::var_os("PI_CODING_AGENT_DIR"),
            ),
        ];
        std::env::set_var("OMP_PROFILE", "work");
        std::env::remove_var("PI_PROFILE");
        std::env::remove_var("PI_CONFIG_DIR");
        std::env::remove_var("PI_CODING_AGENT_DIR");

        let base_env = home.path().join(".omp/.env");
        fs::create_dir_all(base_env.parent().expect("base config root"))
            .expect("create base config root");
        fs::write(&base_env, "PI_CONFIG_DIR=.wrong-from-base\n")
            .expect("write base profile dotenv");

        assert_eq!(
            get_omp_shared_config_agent_dir().expect("resolve named profile"),
            home.path().join(".omp/profiles/work/agent")
        );

        for (key, value) in previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }

    #[test]
    #[serial]
    fn legacy_config_editor_uses_source_revision_and_preserves_unknown_fields() {
        let _agent = test_support::TestAgentDir::new();
        let path = get_omp_settings_path().expect("OMP config path");
        let legacy = path.with_file_name("settings.json");
        ensure_private_omp_parent(&legacy).expect("create OMP config directory");
        fs::write(
            &legacy,
            r#"{ modelRoles: { default: "legacy/provider" }, futureSetting: { enabled: true } }"#,
        )
        .expect("write legacy settings");

        let (yaml, revision) = read_omp_config_yaml().expect("read migrated editor text");
        assert!(yaml.contains("futureSetting"));
        let edited = format!("{yaml}modelRoleStorage: global\n");
        fs::write(
            &legacy,
            "{ modelRoles: { default: \"changed/provider\" }, futureSetting: { enabled: true } }",
        )
        .expect("edit legacy source externally");
        let error = replace_omp_config_yaml_at(&path, &edited, &revision)
            .expect_err("stale legacy source must be rejected");
        assert!(matches!(error, AppError::Conflict(_)));

        let (yaml, revision) = read_omp_config_yaml().expect("reread migrated editor text");
        let edited = format!("{yaml}modelRoleStorage: global\n");
        replace_omp_config_yaml_at(&path, &edited, &revision).expect("migrate config safely");
        assert!(path.exists());
        assert!(fs::read_to_string(&path)
            .expect("read migrated config")
            .contains("futureSetting"));
    }

    #[test]
    #[serial]
    fn unsupported_omp_directory_env_does_not_override_pi_config_dir() {
        let previous = [
            ("OMP_CONFIG_DIR", std::env::var_os("OMP_CONFIG_DIR")),
            ("PI_CONFIG_DIR", std::env::var_os("PI_CONFIG_DIR")),
            ("OMP_PROFILE", std::env::var_os("OMP_PROFILE")),
            ("PI_PROFILE", std::env::var_os("PI_PROFILE")),
        ];
        std::env::set_var("PI_CONFIG_DIR", ".pi-config");
        std::env::set_var("OMP_CONFIG_DIR", ".omp-config");
        std::env::remove_var("OMP_PROFILE");
        std::env::remove_var("PI_PROFILE");

        let env = resolve_omp_path_environment();
        assert_eq!(
            env.get("PI_CONFIG_DIR")
                .map(|value| value.to_string_lossy()),
            Some(".pi-config".into())
        );

        for (key, value) in previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }

    #[test]
    #[serial]
    fn unsupported_omp_directory_dotenv_does_not_select_a_config_root() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let _cwd = test_support::CurrentDirGuard::change_to(home.path());
        let previous = [
            ("OMP_CONFIG_DIR", std::env::var_os("OMP_CONFIG_DIR")),
            ("PI_CONFIG_DIR", std::env::var_os("PI_CONFIG_DIR")),
            ("OMP_PROFILE", std::env::var_os("OMP_PROFILE")),
            ("PI_PROFILE", std::env::var_os("PI_PROFILE")),
            (
                "PI_CODING_AGENT_DIR",
                std::env::var_os("PI_CODING_AGENT_DIR"),
            ),
        ];
        for (key, _) in &previous {
            std::env::remove_var(key);
        }
        fs::write(home.path().join(".env"), "OMP_CONFIG_DIR=.ignored\n")
            .expect("write project dotenv");

        let env = resolve_omp_path_environment();
        assert!(!env.contains_key("PI_CONFIG_DIR"));
        assert_eq!(
            get_omp_shared_config_agent_dir().expect("resolve default config root"),
            home.path().join(".omp/agent")
        );

        for (key, value) in previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }

    #[test]
    #[serial]
    fn explicitly_empty_omp_profile_selects_default_over_legacy_profile() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());
        let previous = [
            ("OMP_PROFILE", std::env::var_os("OMP_PROFILE")),
            ("PI_PROFILE", std::env::var_os("PI_PROFILE")),
        ];
        std::env::set_var("OMP_PROFILE", "");
        std::env::set_var("PI_PROFILE", "work");

        assert_eq!(
            get_omp_shared_config_agent_dir().expect("resolve shared config directory"),
            temp.path().join(".omp/agent")
        );

        for (key, value) in previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }

    #[test]
    #[serial]
    fn process_pi_profile_wins_over_empty_omp_profile_from_dotenv() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let _cwd = test_support::CurrentDirGuard::change_to(home.path());
        let previous_omp_profile = std::env::var_os("OMP_PROFILE");
        let previous_pi_profile = std::env::var_os("PI_PROFILE");
        std::env::remove_var("OMP_PROFILE");
        std::env::set_var("PI_PROFILE", "work");
        fs::write(home.path().join(".env"), "OMP_PROFILE=\n")
            .expect("write empty canonical profile");

        let resolved = resolve_omp_path_environment();
        assert_eq!(
            resolved
                .get("OMP_PROFILE")
                .map(|value| value.to_string_lossy()),
            Some("".into())
        );
        assert_eq!(
            get_omp_shared_config_agent_dir().expect("resolve shared config directory"),
            home.path().join(".omp/profiles/work/agent")
        );

        match previous_omp_profile {
            Some(value) => std::env::set_var("OMP_PROFILE", value),
            None => std::env::remove_var("OMP_PROFILE"),
        }
        match previous_pi_profile {
            Some(value) => std::env::set_var("PI_PROFILE", value),
            None => std::env::remove_var("PI_PROFILE"),
        }
    }

    #[test]
    #[serial]
    fn models_yaml_is_used_when_models_yml_is_absent() {
        let _agent = test_support::TestAgentDir::new();
        let yaml_path = get_omp_agent_dir().unwrap().join("models.yaml");
        ensure_private_omp_parent(&yaml_path).expect("create private agent directory");
        fs::write(&yaml_path, "providers: {}\n").unwrap();
        assert_eq!(get_omp_models_path().unwrap(), yaml_path);
        insert_omp_provider("fallback", &provider()).expect("write fallback models file");
        assert!(yaml_path.exists());
        assert!(!yaml_path.with_file_name("models.yml").exists());
    }

    #[test]
    #[serial]
    fn migrated_xdg_data_home_is_used_for_omp_sessions() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let xdg_data_home = home.path().join("xdg-data");
        std::fs::create_dir_all(xdg_data_home.join("omp/sessions"))
            .expect("create migrated OMP sessions directory");
        std::fs::create_dir_all(home.path().join(".omp/agent"))
            .expect("create native OMP agent directory");
        std::env::set_var("XDG_DATA_HOME", &xdg_data_home);

        assert_eq!(
            get_omp_sessions_dir().expect("resolve migrated OMP sessions directory"),
            xdg_data_home.join("omp/sessions")
        );
    }

    #[test]
    #[serial]
    fn migrated_xdg_data_root_wins_before_sessions_directory_exists() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let xdg_data_home = home.path().join("xdg-data");
        std::fs::create_dir_all(xdg_data_home.join("omp")).expect("create migrated OMP data root");
        std::fs::create_dir_all(home.path().join(".omp/agent/sessions"))
            .expect("create legacy sessions directory");
        std::env::set_var("XDG_DATA_HOME", &xdg_data_home);

        assert_eq!(
            get_omp_sessions_dir().expect("resolve migrated OMP sessions directory"),
            xdg_data_home.join("omp/sessions")
        );
    }

    #[test]
    #[serial]
    fn missing_models_yaml_uses_checked_writer_revision_sentinel() {
        let _agent = test_support::TestAgentDir::new();
        let models_path = get_omp_models_path().expect("models path");
        assert!(!models_path.exists());

        let (text, revision) = read_omp_models_yaml().expect("read missing models document");
        assert_eq!(text, "providers: {}\n");
        assert_eq!(revision, MISSING_MODELS_REVISION);

        let (_, config_revision) = read_omp_config_yaml().expect("read missing config document");
        assert_eq!(config_revision, MISSING_CONFIG_REVISION);
    }

    #[test]
    #[serial]
    fn legacy_models_json_is_migrated_to_models_yml() {
        let _agent = test_support::TestAgentDir::new();
        let agent_dir = get_omp_agent_dir().unwrap();
        fs::create_dir_all(&agent_dir).unwrap();
        let legacy_path = agent_dir.join("models.json");
        fs::write(
            &legacy_path,
            r#"// OMP legacy JSONC supports comments and trailing commas.
            {
              // Older OMP releases used a top-level models array. The native
              // schema keeps this unknown field during JSON -> YAML migration.
              models: [],
              providers: {
                legacy: {
                  baseUrl: 'https://legacy.example/v1',
                  api: 'openai-completions',
                  apiKey: 'KEY',
                  models: [{ id: 'model', }],
                },
              },
            }"#,
        )
        .unwrap();

        let path = get_omp_models_path().expect("migrate legacy models file");
        assert_eq!(path, agent_dir.join("models.yml"));
        assert!(path.exists());
        let migrated = fs::read_to_string(&path).expect("read migrated YAML");
        assert!(!migrated.contains("// OMP legacy"));
        assert!(migrated.contains("models: []"));
        assert!(migrated.contains("providers:"));
        assert!(read_omp_native_providers()
            .expect("read migrated providers")
            .contains_key("legacy"));
    }

    #[test]
    #[serial]
    fn legacy_models_revision_guard_rejects_external_changes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("models.json");
        fs::write(&path, "{ providers: {} }\n").expect("write legacy models");
        let original = read_file_limited(&path, "OMP legacy models").expect("read legacy models");
        let original_revision = revision(&original);
        fs::write(&path, "{ providers: { newer: {} } }\n").expect("edit legacy models");

        let error = ensure_omp_legacy_models_revision(&path, &original_revision)
            .expect_err("stale legacy models must be rejected");
        assert!(matches!(error, AppError::Conflict(_)));
    }

    #[test]
    fn all_upstream_api_protocols_are_accepted() {
        for api in OMP_API_PROTOCOLS {
            validate_api_protocol(api).expect("upstream OMP protocol");
        }
        assert!(validate_api_protocol("openai-chat").is_err());
    }

    #[test]
    #[serial]
    fn api_key_resolves_environment_name_before_literal_fallback() {
        let key_name = "CC_SWITCH_OMP_TEST_API_KEY";
        std::env::set_var(key_name, "resolved-secret");
        assert_eq!(
            resolve_api_key(Some(&json!(key_name))).as_deref(),
            Some("resolved-secret")
        );
        std::env::remove_var(key_name);
        assert_eq!(
            resolve_api_key(Some(&json!("literal-secret"))).as_deref(),
            Some("literal-secret")
        );
        assert_eq!(
            resolve_api_key_for_protocol(
                Some(&json!(r#"{"access_token":"oauth-secret"}"#)),
                Some("google-gemini-cli"),
            )
            .as_deref(),
            Some("oauth-secret")
        );
    }

    #[test]
    #[cfg(windows)]
    #[serial]
    fn environment_resolution_requires_exact_case_on_windows() {
        let name = "CcSwitchOmpExactCase_9F3A";
        let lowercase_name = name.to_ascii_lowercase();
        std::env::set_var(name, "resolved-secret");
        assert_eq!(
            resolve_api_key(Some(&json!(name))).as_deref(),
            Some("resolved-secret")
        );
        assert_eq!(
            resolve_api_key(Some(&json!(lowercase_name))).as_deref(),
            Some(lowercase_name.as_str())
        );
        std::env::remove_var(name);
    }

    #[test]
    #[cfg(unix)]
    fn command_backed_values_are_resolved_and_cached() {
        let command = "printf command-secret";
        assert_eq!(
            resolve_api_key(Some(&json!(format!("!{command}")))).as_deref(),
            Some("command-secret")
        );
        assert_eq!(
            resolve_header_value(&format!("!{command}")).as_deref(),
            Some("command-secret")
        );
    }

    #[test]
    #[cfg(windows)]
    fn command_backed_values_use_the_native_windows_shell() {
        let command = "echo windows-command-secret";
        assert_eq!(
            resolve_api_key(Some(&json!(format!("!{command}")))).as_deref(),
            Some("windows-command-secret")
        );
    }

    #[test]
    #[cfg(unix)]
    #[serial]
    fn command_backed_values_inherit_omp_agent_dotenv() {
        let _agent = test_support::TestAgentDir::new();
        let agent_dir = get_omp_agent_dir().expect("agent directory");
        fs::create_dir_all(&agent_dir).expect("create agent directory");
        let env_name = "OMP_CC_SWITCH_COMMAND_DOTENV_9F3A";
        std::env::remove_var(env_name);
        fs::write(agent_dir.join(".env"), format!("{env_name}=from-agent\n"))
            .expect("write OMP agent dotenv");

        let command = format!("printf %s \"${env_name}\"");
        assert_eq!(
            resolve_api_key(Some(&json!(format!("!{command}")))).as_deref(),
            Some("from-agent")
        );
    }

    #[test]
    #[serial]
    fn command_backed_values_follow_dotenv_config_dir_override() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let _cwd = test_support::CurrentDirGuard::change_to(home.path());
        let previous = [
            ("PI_CONFIG_DIR", std::env::var_os("PI_CONFIG_DIR")),
            (
                "PI_CODING_AGENT_DIR",
                std::env::var_os("PI_CODING_AGENT_DIR"),
            ),
            ("OMP_PROFILE", std::env::var_os("OMP_PROFILE")),
            ("PI_PROFILE", std::env::var_os("PI_PROFILE")),
        ];
        for (key, _) in &previous {
            std::env::remove_var(key);
        }

        fs::write(home.path().join(".env"), "PI_CONFIG_DIR=.custom-omp\n")
            .expect("write home dotenv");
        let custom_agent = home.path().join(".custom-omp/agent");
        fs::create_dir_all(&custom_agent).expect("create custom agent directory");
        fs::write(
            custom_agent.join(".env"),
            "OMP_CC_SWITCH_CUSTOM_CONFIG_SECRET=from-custom-config\n",
        )
        .expect("write custom agent dotenv");

        assert_eq!(
            resolve_api_key(Some(&json!("OMP_CC_SWITCH_CUSTOM_CONFIG_SECRET"))).as_deref(),
            Some("from-custom-config")
        );

        for (key, value) in previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }

    #[test]
    #[cfg(unix)]
    fn command_backed_values_reject_oversized_output_without_unbounded_buffering() {
        let command = "yes x | head -c 70000";
        assert!(resolve_api_key(Some(&json!(format!("!{command}")))).is_none());
    }

    #[test]
    fn dotenv_api_key_values_support_export_and_quotes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(".env");
        fs::write(
            &path,
            "# comment\nexport OMP_TEST_QUOTED=\"secret-value\" # trailing\nexport\tOMP_TEST_TAB=tab-value\nOMP_TEST_PLAIN=plain-value\n",
        )
        .expect("write dotenv file");

        assert_eq!(
            read_omp_dotenv_value(&path, "OMP_TEST_QUOTED").as_deref(),
            Some("secret-value")
        );
        assert_eq!(
            read_omp_dotenv_value(&path, "OMP_TEST_PLAIN").as_deref(),
            Some("plain-value")
        );
        assert_eq!(
            read_omp_dotenv_value(&path, "OMP_TEST_TAB").as_deref(),
            Some("tab-value")
        );
        fs::write(&path, "1INVALID=bad\n_VALID=good\n").expect("rewrite dotenv file");
        assert!(read_omp_dotenv_value(&path, "1INVALID").is_none());
        assert_eq!(
            read_omp_dotenv_value(&path, "_VALID").as_deref(),
            Some("good")
        );

        fs::write(&path, "PI_TEST_ALIAS=legacy\nOMP_TEST_ALIAS=canonical\n")
            .expect("rewrite non-profile dotenv names");
        assert_eq!(
            read_omp_dotenv_value(&path, "PI_TEST_ALIAS").as_deref(),
            Some("legacy")
        );
        fs::write(&path, "OMP_TEST_ALIAS=canonical\n").expect("remove legacy variable");
        assert!(read_omp_dotenv_value(&path, "PI_TEST_ALIAS").is_none());

        fs::write(&path, "PI_PROFILE=legacy\nOMP_PROFILE=canonical\n")
            .expect("rewrite profile aliases");
        assert_eq!(
            read_omp_dotenv_value(&path, "PI_PROFILE").as_deref(),
            Some("canonical")
        );
    }

    #[test]
    #[serial]
    fn api_key_resolves_exact_omp_agent_dotenv_names_without_invented_aliases() {
        let _agent = test_support::TestAgentDir::new();
        let agent_dir = get_omp_agent_dir().expect("agent directory");
        fs::create_dir_all(&agent_dir).expect("create agent directory");
        let path = agent_dir.join(".env");
        fs::write(&path, "OMP_CC_SWITCH_DOTENV_SECRET=from-agent\n")
            .expect("write OMP agent dotenv");

        assert_eq!(
            resolve_api_key(Some(&json!("OMP_CC_SWITCH_DOTENV_SECRET"))).as_deref(),
            Some("from-agent")
        );
        assert_eq!(
            resolve_api_key(Some(&json!("PI_CC_SWITCH_DOTENV_SECRET"))).as_deref(),
            Some("PI_CC_SWITCH_DOTENV_SECRET")
        );
    }

    #[test]
    #[serial]
    fn api_key_does_not_invent_process_omp_aliases() {
        let omp_name = "OMP_CC_SWITCH_PROCESS_ALIAS_9F3A";
        let pi_name = "PI_CC_SWITCH_PROCESS_ALIAS_9F3A";
        std::env::set_var(omp_name, "from-omp");
        std::env::remove_var(pi_name);
        assert_eq!(
            resolve_api_key(Some(&json!(pi_name))).as_deref(),
            Some(pi_name)
        );

        std::env::set_var(pi_name, "from-pi");
        assert_eq!(
            resolve_api_key(Some(&json!(pi_name))).as_deref(),
            Some("from-pi")
        );
        std::env::remove_var(omp_name);
        std::env::remove_var(pi_name);
    }

    #[test]
    fn known_provider_and_model_fields_are_type_checked() {
        let mut invalid_provider = provider();
        invalid_provider["authHeader"] = json!("yes");
        assert!(validate_provider_node("invalid-auth-header", &invalid_provider).is_err());

        let mut invalid_transport = provider();
        invalid_transport["transport"] = json!("http");
        assert!(validate_provider_node("invalid-transport", &invalid_transport).is_err());

        let mut invalid_override = provider();
        invalid_override["modelOverrides"] = json!({
            "example-model": { "contextWindow": "large" }
        });
        assert!(validate_provider_node("invalid-override", &invalid_override).is_err());

        let mut invalid_model = provider();
        invalid_model["models"][0]["name"] = json!(" ");
        assert!(validate_provider_node("invalid-model-name", &invalid_model).is_err());

        let mut invalid_compaction = provider();
        invalid_compaction["remoteCompaction"] = json!({"endpoint": ""});
        assert!(validate_provider_node("invalid-compaction", &invalid_compaction).is_err());

        let mut invalid_compat = provider();
        invalid_compat["compat"] = json!({ "supportsStore": "yes" });
        assert!(validate_provider_node("invalid-compat", &invalid_compat).is_err());

        let mut invalid_thinking = provider();
        invalid_thinking["models"][0]["thinking"] = json!({ "mode": "effort" });
        assert!(validate_provider_node("invalid-thinking", &invalid_thinking).is_err());
    }

    #[test]
    fn custom_model_provider_requirements_match_omp_schema() {
        let mut missing_key = provider();
        missing_key
            .as_object_mut()
            .expect("provider object")
            .remove("apiKey");
        assert!(validate_provider_for_live_write("missing-key", &missing_key).is_err());

        let mut missing_api = provider();
        missing_api
            .as_object_mut()
            .expect("provider object")
            .remove("api");
        assert!(validate_provider_for_live_write("missing-api", &missing_api).is_err());

        let mut missing_base_url = provider();
        missing_base_url
            .as_object_mut()
            .expect("provider object")
            .remove("baseUrl");
        assert!(validate_provider_for_live_write("missing-url", &missing_base_url).is_err());

        let mut invalid_model = provider();
        invalid_model["models"] = json!([{"id": ""}]);
        assert!(validate_provider_for_live_write("invalid-model", &invalid_model).is_err());

        let mut partial_cost = provider();
        partial_cost["models"][0]["cost"] = json!({"input": 1.25});
        assert!(validate_provider_for_live_write("partial-cost", &partial_cost).is_err());

        let mut partial_override = provider();
        partial_override["modelOverrides"] = json!({
            "example-model": {"cost": {"input": 1.25}}
        });
        validate_provider_for_live_write("partial-override", &partial_override)
            .expect("OMP permits partial model override cost metadata");

        validate_provider_for_live_write(
            "keyless-local",
            &json!({
                "baseUrl": "http://127.0.0.1:4000/v1",
                "api": "openai-completions",
                "auth": "none",
                "models": [{"id": "local"}]
            }),
        )
        .expect("auth none permits keyless custom models");

        validate_provider_for_live_write(
            "keyless-oauth",
            &json!({
                "baseUrl": "https://api.example.com/v1",
                "api": "openai-completions",
                "auth": "oauth",
                "models": [{"id": "model-a"}]
            }),
        )
        .expect("auth oauth permits custom models without an inline apiKey");

        assert!(validate_provider_for_live_write(
            "per-model-endpoints",
            &json!({
                "api": "openai-completions",
                "apiKey": "secret",
                "models": [{
                    "id": "model-a",
                    "baseUrl": "https://model-a.example.com/v1"
                }]
            }),
        )
        .is_err());

        validate_provider_for_live_write(
            "proxy-discovery",
            &json!({
                "baseUrl": "https://example.com/v1",
                "discovery": {"type": "proxy"}
            }),
        )
        .expect("proxy discovery may omit provider api");
        assert!(validate_provider_for_live_write(
            "bad-discovery",
            &json!({
                "baseUrl": "https://example.com/v1",
                "discovery": {"type": "ollama"}
            })
        )
        .is_err());

        let mut zero_premium = provider();
        zero_premium["models"][0]["premiumMultiplier"] = json!(0);
        validate_provider_for_live_write("zero-premium", &zero_premium)
            .expect("zero premium multiplier is valid OMP metadata");
    }

    #[test]
    #[serial]
    fn adding_model_to_override_only_provider_is_rejected_without_mutation() {
        let _agent = test_support::TestAgentDir::new();
        let override_only = json!({"headers": {}});
        insert_omp_provider("override-only", &override_only).expect("seed override provider");
        let (_, revision) = read_omp_models_yaml().expect("read models revision");

        let error = upsert_omp_model_checked(
            "override-only",
            "model-a",
            json!({"id": "model-a"}),
            &revision,
        )
        .expect_err("custom models require provider-level connection settings");
        assert!(error.to_string().contains("baseUrl is required"));
        assert_eq!(
            read_omp_native_provider("override-only").expect("read provider"),
            Some(override_only)
        );
    }

    #[test]
    #[serial]
    fn discovery_provider_base_url_uses_native_defaults_and_env_overrides() {
        for (discovery_type, expected) in [
            ("ollama", "http://127.0.0.1:11434"),
            ("llama.cpp", "http://127.0.0.1:8080"),
            ("lm-studio", "http://127.0.0.1:1234/v1"),
            ("openai-models-list", "http://127.0.0.1:1234/v1"),
            ("proxy", "http://127.0.0.1:1234/v1"),
            ("litellm", "http://localhost:4000/v1"),
        ] {
            let config = json!({"discovery": {"type": discovery_type}});
            assert_eq!(provider_base_url(&config).unwrap(), expected);
        }

        let previous_ollama_base_url = std::env::var_os("OLLAMA_BASE_URL");
        std::env::set_var(
            "OLLAMA_BASE_URL",
            "https://ollama.example/custom/path?token=secret#fragment",
        );
        assert_eq!(
            provider_base_url(&json!({"discovery": {"type": "ollama"}})).unwrap(),
            "https://ollama.example"
        );
        if let Some(value) = previous_ollama_base_url {
            std::env::set_var("OLLAMA_BASE_URL", value);
        } else {
            std::env::remove_var("OLLAMA_BASE_URL");
        }
    }

    #[test]
    fn ollama_host_values_are_normalized_like_omp() {
        for (input, expected) in [
            ("127.0.0.1:11434", "http://127.0.0.1:11434"),
            ("127.0.0.1", "http://127.0.0.1:11434"),
            (":11434", "http://127.0.0.1:11434"),
            ("//ollama.example", "http://ollama.example:11434"),
            ("https://ollama.example/path", "https://ollama.example"),
            ("[::1]:1234", "http://[::1]:1234"),
        ] {
            assert_eq!(normalize_ollama_host_env(input).as_deref(), Some(expected));
        }
        assert!(normalize_ollama_host_env("ftp://ollama.example").is_none());
        assert!(normalize_ollama_host_env(" ").is_none());

        assert_eq!(
            normalize_ollama_base_url("http://ollama.example/custom/path?x=1#frag").as_deref(),
            Some("http://ollama.example")
        );
        assert_eq!(
            normalize_ollama_base_url("https://ollama.example:9443/v1/").as_deref(),
            Some("https://ollama.example:9443")
        );
    }

    #[test]
    #[serial]
    fn discovery_provider_base_url_reads_omp_agent_dotenv() {
        let _agent = test_support::TestAgentDir::new();
        let agent_dir = get_omp_agent_dir().expect("OMP agent directory");
        fs::create_dir_all(&agent_dir).expect("create OMP agent directory");
        fs::write(
            agent_dir.join(".env"),
            "OLLAMA_BASE_URL=https://dotenv-ollama.example/custom/path\n",
        )
        .expect("write OMP dotenv");

        let previous = std::env::var_os("OLLAMA_BASE_URL");
        std::env::remove_var("OLLAMA_BASE_URL");
        assert_eq!(
            provider_base_url(&json!({"discovery": {"type": "ollama"}})).unwrap(),
            "https://dotenv-ollama.example"
        );
        if let Some(value) = previous {
            std::env::set_var("OLLAMA_BASE_URL", value);
        }
    }

    #[test]
    #[serial]
    fn discovery_only_provider_accepts_explicit_default_model() {
        let _agent = test_support::TestAgentDir::new();
        let path = get_omp_models_path().expect("OMP models path");
        ensure_private_omp_parent(&path).expect("create OMP agent directory");
        fs::write(
            &path,
            "providers:\n  local:\n    baseUrl: http://127.0.0.1:11434\n    api: openai-completions\n    auth: none\n    discovery:\n      type: ollama\n",
        )
        .expect("write discovery-only provider");

        let selector = set_omp_default_model("local", Some("llama3"))
            .expect("discovery-only providers accept explicit model ids");
        assert_eq!(selector, "local/llama3");
        assert_eq!(
            read_omp_model_roles()
                .expect("read model roles")
                .get("default")
                .map(String::as_str),
            Some("local/llama3")
        );

        let error = set_omp_default_model("local", None)
            .expect_err("discovery-only providers require an explicit model id");
        assert!(error
            .to_string()
            .contains("specify --model for a discovery provider"));
    }

    #[test]
    #[serial]
    fn setting_default_model_checks_disabled_provider_without_reentrant_lock() {
        let _agent = test_support::TestAgentDir::new();
        let models_path = get_omp_models_path().expect("OMP models path");
        ensure_private_omp_parent(&models_path).expect("create OMP agent directory");
        fs::write(
            &models_path,
            "providers:\n  local:\n    baseUrl: http://127.0.0.1:11434\n    api: openai-completions\n    auth: none\n    models:\n      - id: llama3\n",
        )
        .expect("write provider");
        let config_path = get_omp_settings_path().expect("OMP config path");
        ensure_private_omp_parent(&config_path).expect("create OMP config directory");
        fs::write(&config_path, "disabledProviders: [local]\n").expect("disable provider");

        let error = set_omp_default_model("local", Some("llama3"))
            .expect_err("disabled providers cannot become the default");
        assert!(error.to_string().contains("is disabled"));
    }

    #[test]
    #[serial]
    fn empty_models_file_is_treated_as_an_empty_registry() {
        let _agent = test_support::TestAgentDir::new();
        let path = get_omp_models_path().expect("OMP models path");
        fs::create_dir_all(path.parent().expect("models parent")).expect("create parent");
        fs::write(&path, "\n  \n").expect("write empty models file");
        let providers = read_omp_native_providers().expect("read empty models file");
        assert!(providers.is_empty());
    }

    #[test]
    #[serial]
    fn duplicate_provider_key_is_validation_not_a_write_conflict() {
        let _agent = test_support::TestAgentDir::new();
        insert_omp_provider("duplicate", &provider()).expect("insert provider");
        let mut replacement = provider();
        replacement["name"] = json!("Other");

        let error = insert_omp_provider("duplicate", &replacement)
            .expect_err("duplicate provider key must be rejected");
        assert!(matches!(error, AppError::InvalidInput(_)));
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn newly_created_models_file_and_agent_directory_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let _agent = test_support::TestAgentDir::new();
        insert_omp_provider("cc-switch-private", &provider()).expect("write private models file");

        let path = get_omp_models_path().expect("models path");
        let file_mode = fs::metadata(&path)
            .expect("models metadata")
            .permissions()
            .mode()
            & 0o777;
        let directory_mode = fs::metadata(path.parent().expect("agent directory"))
            .expect("agent directory metadata")
            .permissions()
            .mode()
            & 0o777;

        assert_eq!(file_mode, 0o600);
        assert_eq!(directory_mode, 0o700);
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn group_writable_omp_ancestor_is_rejected() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("create temporary root");
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o770))
            .expect("make temporary root group-writable");
        let path = temp.path().join("agent").join("models.yml");

        let error = ensure_private_omp_parent(&path)
            .expect_err("group-writable ancestors must not protect OMP credentials");
        assert!(error.to_string().contains("group/other writable"));
    }

    #[test]
    #[serial]
    fn stale_models_revision_does_not_overwrite_an_external_edit() {
        let _agent = test_support::TestAgentDir::new();
        let path = get_omp_models_path().expect("models path");
        ensure_private_omp_parent(&path).expect("create agent directory");
        fs::write(&path, r#"{"providers":{"external":{"models":[]}}}"#)
            .expect("write initial models");
        let (_, stale_revision) =
            read_models_document_with_revision(&path).expect("read models revision");

        let external = r#"{"providers":{"external":{"models":[]},"omp-added":{"models":[]}}}"#;
        fs::write(&path, external).expect("edit models externally");

        let replacement = json!({"providers": {"cc-switch": provider()}});
        let error = write_models_document(&path, &replacement, &stale_revision)
            .expect_err("stale write must fail");
        assert!(matches!(error, AppError::Conflict(_)));
        assert_eq!(
            fs::read_to_string(path).expect("read external models"),
            external
        );
    }

    #[test]
    #[serial]
    fn full_models_editor_preserves_native_transports_and_rejects_malformed_nodes() {
        let _agent = test_support::TestAgentDir::new();
        let path = get_omp_models_path().expect("models path");
        ensure_private_omp_parent(&path).expect("create agent directory");
        fs::write(&path, "providers: {}\n").expect("seed models file");
        let (_, revision) = read_omp_models_yaml().expect("read models");

        // OMP's schema accepts non-HTTP baseUrl strings used by native
        // transports. The full-file editor must round-trip those entries even
        // though CC Switch's own discovery/test operations are HTTP-only.
        replace_omp_models_yaml(
            "providers:\n  socket:\n    baseUrl: unix:///var/run/omp.sock\n    api: openai-completions\n    apiKey: secret\n    models:\n      - id: socket-model\n",
            &revision,
        )
        .expect("native transport should be accepted by full-file editor");
        let (_, revision) = read_omp_models_yaml().expect("read native transport models");

        let error =
            replace_omp_models_yaml("providers:\n  malformed:\n    sdkOption: {}\n", &revision)
                .expect_err("ordinary malformed providers must not bypass write validation");
        assert!(error.to_string().contains("must specify an override"));
    }

    #[test]
    #[serial]
    fn project_model_roles_overlay_and_receive_writes_when_selected() {
        let _agent = test_support::TestAgentDir::new();
        let temp = tempfile::tempdir().expect("create project directory");
        let _cwd = test_support::CurrentDirGuard::change_to(temp.path());

        let global_path = get_omp_settings_path().expect("global config path");
        ensure_private_omp_parent(&global_path).expect("create global config directory");
        fs::write(
            &global_path,
            "modelRoleStorage: project\nmodelRoles:\n  default: global/provider\n  slow: global/slow\n",
        )
        .expect("write global config");
        let project_path = get_omp_project_settings_path().expect("project config path");
        ensure_private_omp_parent(&project_path).expect("create project config directory");
        fs::write(&project_path, "modelRoles:\n  default: project/provider\n")
            .expect("write project config");

        let (roles, target, revision) =
            read_omp_model_roles_with_metadata().expect("read effective roles");
        assert_eq!(target, project_path);
        assert_eq!(
            roles.get("default").map(String::as_str),
            Some("project/provider")
        );
        assert_eq!(roles.get("slow").map(String::as_str), Some("global/slow"));

        set_omp_model_role("default", Some("project/updated"), Some(&revision))
            .expect("write project role");
        let written = fs::read_to_string(&target).expect("read project config");
        assert!(written.contains("project/updated"));
        assert!(written.contains("modelRoles:"));
    }

    #[test]
    #[serial]
    fn project_role_deletion_clears_project_override_without_hiding_global_role() {
        let _agent = test_support::TestAgentDir::new();
        let temp = tempfile::tempdir().expect("create project directory");
        let _cwd = test_support::CurrentDirGuard::change_to(temp.path());

        let global_path = get_omp_settings_path().expect("global config path");
        ensure_private_omp_parent(&global_path).expect("create global config directory");
        fs::write(
            &global_path,
            "modelRoleStorage: project\nmodelRoles:\n  default: global/provider\n",
        )
        .expect("write global config");

        let project_path = get_omp_project_settings_path().expect("project config path");
        let (_, _, revision) = read_omp_model_roles_with_metadata().expect("read project roles");
        let error = set_omp_model_role("default", None, Some(&revision))
            .expect_err("inherited global role must not report a no-op as deletion");
        assert!(error.to_string().contains("inherited from global"));

        ensure_private_omp_parent(&project_path).expect("create project config directory");
        fs::write(&project_path, "modelRoles:\n  default: project/provider\n")
            .expect("write project override");
        let (_, _, revision) = read_omp_model_roles_with_metadata().expect("reread project roles");
        set_omp_model_role("default", None, Some(&revision)).expect("clear project override");

        let written: Value =
            serde_yaml::from_str(&fs::read_to_string(&project_path).expect("read project config"))
                .expect("parse project config");
        assert_eq!(
            written
                .get("modelRoles")
                .and_then(Value::as_object)
                .and_then(|roles| roles.get("default")),
            Some(&Value::Null)
        );
        assert_eq!(
            read_omp_model_roles()
                .expect("read effective roles")
                .get("default")
                .map(String::as_str),
            Some("global/provider")
        );
    }

    #[test]
    #[serial]
    fn full_config_editor_rejects_invalid_managed_fields() {
        let _agent = test_support::TestAgentDir::new();
        let path = get_omp_settings_path().expect("OMP config path");
        ensure_private_omp_parent(&path).expect("create config directory");
        fs::write(&path, "futureSetting: true\n").expect("seed config");
        let (_, revision) = read_omp_config_yaml().expect("read config");

        for invalid in [
            "modelRoles: 123\n",
            "modelRoles:\n  default: [bad]\n",
            "modelRoleStorage: workspace\n",
            "disabledProviders: invalid\n",
        ] {
            assert!(
                replace_omp_config_yaml_at(&path, invalid, &revision).is_err(),
                "invalid config should be rejected: {invalid}"
            );
            assert_eq!(
                fs::read_to_string(&path).expect("read unchanged config"),
                "futureSetting: true\n"
            );
        }

        replace_omp_config_yaml_at(
            &path,
            "futureSetting: true\nmodelRoleStorage: project\nmodelRoles:\n  default: provider/model\ndisabledProviders: []\n",
            &revision,
        )
        .expect("valid config should be accepted");
    }

    #[test]
    fn selector_provider_resolution_supports_aliases_and_cycles() {
        let roles = IndexMap::from([
            ("default".to_string(), "@slow:high".to_string()),
            ("slow".to_string(), "provider/model".to_string()),
        ]);
        assert_eq!(
            omp_selector_provider_id("*", &roles).as_deref(),
            Some("provider")
        );
        assert_eq!(
            omp_selector_provider_id("pi/default", &roles).as_deref(),
            Some("provider")
        );
        assert_eq!(omp_selector_provider_id("bare-model", &roles), None);

        let cycle = IndexMap::from([
            ("default".to_string(), "@slow".to_string()),
            ("slow".to_string(), "@default".to_string()),
        ]);
        assert_eq!(omp_selector_provider_id("*", &cycle), None);
    }

    #[test]
    #[serial]
    fn project_disabled_providers_replace_global_and_can_be_cleared() {
        let _agent = test_support::TestAgentDir::new();
        let temp = tempfile::tempdir().expect("create project directory");
        let _cwd = test_support::CurrentDirGuard::change_to(temp.path());

        let global_path = get_omp_settings_path().expect("global config path");
        ensure_private_omp_parent(&global_path).expect("create global config directory");
        fs::write(&global_path, "disabledProviders: [global]\n").expect("write global config");
        let project_path = get_omp_project_settings_path().expect("project config path");
        ensure_private_omp_parent(&project_path).expect("create project config directory");
        fs::write(&project_path, "disabledProviders: [project]\n").expect("write project config");

        let disabled = read_omp_disabled_providers().expect("read disabled providers");
        assert!(disabled.contains("project"));
        assert!(!disabled.contains("global"));
        set_omp_provider_disabled("project", false).expect("clear disabled provider");
        assert!(!read_omp_disabled_providers()
            .expect("reread disabled providers")
            .contains("project"));
    }

    #[test]
    #[serial]
    fn enabling_provider_preserves_unrelated_path_scoped_disabled_entries() {
        let _agent = test_support::TestAgentDir::new();
        let temp = tempfile::tempdir().expect("create project directory");
        let _cwd = test_support::CurrentDirGuard::change_to(temp.path());
        let unrelated = temp.path().join("unrelated");

        let global_path = get_omp_settings_path().expect("global config path");
        ensure_private_omp_parent(&global_path).expect("create global config directory");
        fs::write(
            &global_path,
            format!(
                "disabledProviders:\n  - path: {}\n    providers: [victim]\n  - path: {}\n    providers: [victim, keep]\n  - global\n",
                unrelated.display(),
                temp.path().display()
            ),
        )
        .expect("write global config");

        set_omp_provider_disabled("victim", false).expect("enable provider in current project");

        let written = fs::read_to_string(&global_path).expect("read updated global config");
        let document: Value = serde_yaml::from_str(&written).expect("parse updated config");
        let entries = document
            .get("disabledProviders")
            .and_then(Value::as_array)
            .expect("disabledProviders array");
        assert!(entries.iter().any(|entry| {
            entry.get("path").and_then(Value::as_str) == Some(unrelated.to_string_lossy().as_ref())
                && entry
                    .get("providers")
                    .and_then(Value::as_array)
                    .is_some_and(|providers| {
                        providers.iter().any(|id| id.as_str() == Some("victim"))
                    })
        }));
        assert!(entries.iter().any(|entry| {
            entry.get("path").and_then(Value::as_str)
                == Some(temp.path().to_string_lossy().as_ref())
                && entry
                    .get("providers")
                    .and_then(Value::as_array)
                    .is_some_and(|providers| {
                        providers.len() == 1 && providers[0].as_str() == Some("keep")
                    })
        }));
        assert!(entries.iter().any(|entry| entry.as_str() == Some("global")));
    }

    #[test]
    #[serial]
    fn disabled_provider_scopes_accept_string_forms_and_enable_removes_them() {
        let _agent = test_support::TestAgentDir::new();
        let temp = tempfile::tempdir().expect("create project directory");
        let _cwd = test_support::CurrentDirGuard::change_to(temp.path());
        let path = get_omp_settings_path().expect("global config path");
        ensure_private_omp_parent(&path).expect("create global config directory");
        fs::write(
            &path,
            format!(
                "disabledProviders:\n  - path: {}\n    providers: victim\n  - pathPrefix: {}\n    values: [keep, victim]\n",
                temp.path().display(),
                temp.path().display()
            ),
        )
        .expect("write string-form scoped providers");

        let disabled = read_omp_disabled_providers().expect("read disabled providers");
        assert!(disabled.contains("victim"));
        assert!(disabled.contains("keep"));

        set_omp_provider_disabled("victim", false).expect("enable provider");
        let written: Value =
            serde_yaml::from_str(&fs::read_to_string(&path).expect("read updated config"))
                .expect("parse updated config");
        let entries = written
            .get("disabledProviders")
            .and_then(Value::as_array)
            .expect("disabledProviders array");
        assert!(entries
            .iter()
            .all(|entry| { entry.get("providers").and_then(Value::as_str) != Some("victim") }));
        assert!(entries.iter().any(|entry| {
            entry
                .get("values")
                .and_then(Value::as_array)
                .is_some_and(|values| values.iter().any(|value| value.as_str() == Some("keep")))
        }));
    }

    #[test]
    #[serial]
    fn project_settings_json_is_loaded_before_yaml_and_yaml_wins() {
        let _agent = test_support::TestAgentDir::new();
        let temp = tempfile::tempdir().expect("create project directory");
        let _cwd = test_support::CurrentDirGuard::change_to(temp.path());

        let project_root = temp.path().join(".omp");
        fs::create_dir_all(&project_root).expect("create project config directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&project_root, fs::Permissions::from_mode(0o700))
                .expect("make project config directory private");
        }
        fs::write(
            project_root.join("settings.json"),
            r#"{
                modelRoleStorage: "project",
                modelRoles: { default: "json/provider", slow: "json/slow" },
                disabledProviders: ["json-disabled"]
            }"#,
        )
        .expect("write project JSON settings");
        fs::write(
            project_root.join("config.yml"),
            "modelRoles:\n  default: yaml/provider\n",
        )
        .expect("write project YAML settings");

        let (roles, target, _) = read_omp_model_roles_with_metadata().expect("read project roles");
        assert_eq!(target, project_root.join("config.yml"));
        assert_eq!(
            roles.get("default").map(String::as_str),
            Some("yaml/provider")
        );
        assert_eq!(roles.get("slow").map(String::as_str), Some("json/slow"));
        assert!(read_omp_disabled_providers()
            .expect("read JSON disabled providers")
            .contains("json-disabled"));
    }

    #[test]
    #[serial]
    fn project_yaml_disabled_providers_override_json_layer() {
        let _agent = test_support::TestAgentDir::new();
        let temp = tempfile::tempdir().expect("create project directory");
        let _cwd = test_support::CurrentDirGuard::change_to(temp.path());

        let project_root = temp.path().join(".omp");
        fs::create_dir_all(&project_root).expect("create project config directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&project_root, fs::Permissions::from_mode(0o700))
                .expect("make project config directory private");
        }
        fs::write(
            project_root.join("settings.json"),
            r#"{ disabledProviders: ["json-disabled"] }"#,
        )
        .expect("write project JSON settings");
        fs::write(
            project_root.join("config.yml"),
            "disabledProviders: [yaml-disabled]\n",
        )
        .expect("write project YAML settings");

        let disabled = read_omp_disabled_providers().expect("read effective disabled providers");
        assert!(disabled.contains("yaml-disabled"));
        assert!(!disabled.contains("json-disabled"));
    }

    #[test]
    #[serial]
    fn project_config_yaml_is_not_treated_as_an_omp_settings_source() {
        let _agent = test_support::TestAgentDir::new();
        let temp = tempfile::tempdir().expect("create project directory");
        let _cwd = test_support::CurrentDirGuard::change_to(temp.path());

        let project_root = temp.path().join(".omp");
        fs::create_dir_all(&project_root).expect("create project config directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&project_root, fs::Permissions::from_mode(0o700))
                .expect("make project config directory private");
        }
        fs::write(
            project_root.join("config.yaml"),
            "modelRoleStorage: project\nmodelRoles:\n  default: ignored/provider\n",
        )
        .expect("write unsupported project YAML spelling");

        assert_eq!(
            get_omp_project_settings_path().expect("project settings path"),
            project_root.join("config.yml")
        );
        let layer = read_omp_project_settings_layer().expect("read project settings layer");
        assert!(layer.is_none(), "OMP does not load project config.yaml");
    }

    #[test]
    #[serial]
    fn editing_json_disabled_provider_creates_yaml_override_instead_of_mutating_json() {
        let _agent = test_support::TestAgentDir::new();
        let temp = tempfile::tempdir().expect("create project directory");
        let _cwd = test_support::CurrentDirGuard::change_to(temp.path());

        let project_root = temp.path().join(".omp");
        fs::create_dir_all(&project_root).expect("create project config directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&project_root, fs::Permissions::from_mode(0o700))
                .expect("make project config directory private");
        }
        let json_path = project_root.join("settings.json");
        fs::write(&json_path, r#"{ disabledProviders: ["json-disabled"] }"#)
            .expect("write project JSON settings");

        set_omp_provider_disabled("json-disabled", false).expect("enable JSON provider");
        let written = fs::read_to_string(project_root.join("config.yml"))
            .expect("read project YAML override");
        assert!(written.contains("disabledProviders"));
        assert!(!read_omp_disabled_providers()
            .expect("read effective disabled providers")
            .contains("json-disabled"));
        assert!(fs::read_to_string(json_path)
            .expect("read original project JSON")
            .contains("json-disabled"));
    }

    #[test]
    #[serial]
    fn legacy_global_settings_json_is_read_until_yaml_is_created() {
        let _agent = test_support::TestAgentDir::new();
        let temp = tempfile::tempdir().expect("create agent directory");
        let agent_dir = temp.path().join("agent");
        let _agent_override = test_support::TestAgentDir::at(&agent_dir);
        fs::create_dir_all(&agent_dir).expect("create agent directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&agent_dir, fs::Permissions::from_mode(0o700))
                .expect("make agent directory private");
        }
        fs::write(
            agent_dir.join("settings.json"),
            r#"{ modelRoles: { default: "legacy/provider" }, futureSetting: { enabled: true } }"#,
        )
        .expect("write legacy settings");

        let (roles, target, revision) =
            read_omp_model_roles_with_metadata().expect("read legacy global roles");
        assert_eq!(
            roles.get("default").map(String::as_str),
            Some("legacy/provider")
        );
        assert_eq!(target, agent_dir.join("config.yml"));
        let (_, legacy_revision) =
            parse_json5_document(&agent_dir.join("settings.json"), "OMP legacy settings")
                .expect("read legacy revision");
        assert_eq!(revision, legacy_revision);

        set_omp_model_role("default", Some("new/provider"), Some(&revision))
            .expect("write migrated YAML role");
        let written = fs::read_to_string(agent_dir.join("config.yml")).expect("read YAML config");
        assert!(written.contains("new/provider"));
        assert!(written.contains("futureSetting"));
        assert!(agent_dir.join("settings.json").exists());
    }

    #[test]
    fn provider_selector_identifiers_reject_ambiguous_keys() {
        assert!(validate_provider_key("bad/provider").is_err());
        assert!(validate_provider_key("bad provider").is_err());
        assert!(validate_model_selector("provider/model").is_ok());
        assert!(validate_model_selector("@smol").is_ok());
        assert!(validate_model_selector("*").is_ok());
        assert!(validate_model_selector("provider").is_ok());
        assert!(validate_model_selector("\u{0000}").is_err());
    }

    #[test]
    fn model_role_reference_checks_strip_every_omp_reasoning_suffix() {
        assert_eq!(
            strip_omp_thinking_suffix("provider/model:minimal"),
            "provider/model"
        );
        assert_eq!(
            strip_omp_thinking_suffix("provider/model:off"),
            "provider/model"
        );
        assert_eq!(
            strip_omp_thinking_suffix("provider/model:xhigh"),
            "provider/model"
        );
        assert_eq!(
            strip_omp_thinking_suffix("provider/model:unknown"),
            "provider/model:unknown"
        );
    }

    #[test]
    #[serial]
    fn directory_selectors_are_loaded_from_project_dotenv() {
        let temp = tempfile::tempdir().expect("create project directory");
        let previous = [
            ("PI_CONFIG_DIR", std::env::var_os("PI_CONFIG_DIR")),
            ("OMP_PROFILE", std::env::var_os("OMP_PROFILE")),
            ("PI_PROFILE", std::env::var_os("PI_PROFILE")),
            (
                "PI_CODING_AGENT_DIR",
                std::env::var_os("PI_CODING_AGENT_DIR"),
            ),
        ];
        for (key, _) in &previous {
            std::env::remove_var(key);
        }
        let _cwd = test_support::CurrentDirGuard::change_to(temp.path());
        fs::write(temp.path().join(".env"), "PI_CONFIG_DIR=.omp-dotenv\n")
            .expect("write project dotenv");
        let env = resolve_omp_path_environment();
        assert_eq!(
            env.get("PI_CONFIG_DIR")
                .map(|value| value.to_string_lossy()),
            Some(".omp-dotenv".into())
        );

        drop(_cwd);
        for (key, value) in previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }

    #[test]
    #[serial]
    fn deleting_referenced_models_is_rejected_before_native_mutation() {
        let _agent = test_support::TestAgentDir::new();
        insert_omp_provider("provider", &provider()).expect("seed provider");
        set_omp_model_role("default", Some("provider/example-model"), None).expect("seed role");

        let error = remove_omp_model("provider", "example-model")
            .expect_err("referenced model must not be deleted");
        assert!(error.to_string().contains("modelRoles references it"));
        assert!(read_omp_native_provider("provider")
            .expect("read provider")
            .is_some());
    }

    #[test]
    #[serial]
    fn deleting_bare_or_aliased_model_roles_is_rejected_when_unambiguous() {
        let _agent = test_support::TestAgentDir::new();
        insert_omp_provider("provider", &provider()).expect("seed provider");
        set_omp_model_role("smol", Some("example-model"), None).expect("seed bare role");
        set_omp_model_role("default", Some("pi/smol"), None).expect("seed legacy role alias");

        let error = remove_omp_model("provider", "example-model")
            .expect_err("bare/aliased role must prevent model deletion");
        assert!(error.to_string().contains("modelRoles references it"));
    }

    #[test]
    #[serial]
    fn deleting_provider_with_ambiguous_bare_model_is_allowed() {
        let _agent = test_support::TestAgentDir::new();
        insert_omp_provider("provider-a", &provider()).expect("seed first provider");
        let mut second = provider();
        second["models"][0]["id"] = json!("example-model");
        insert_omp_provider("provider-b", &second).expect("seed second provider");
        set_omp_model_role("default", Some("example-model"), None).expect("seed bare role");

        remove_omp_provider("provider-a").expect("ambiguous bare selector is not dangling");
    }

    #[test]
    #[serial]
    fn unknown_root_fields_survive_provider_crud_write() {
        let _agent = test_support::TestAgentDir::new();
        let path = get_omp_models_path().expect("models path");
        ensure_private_omp_parent(&path).expect("create agent directory");
        fs::write(
            &path,
            "version: 2\nproviders:\n  external:\n    baseUrl: https://external.example/v1\n",
        )
        .expect("write models with root metadata");

        insert_omp_provider("cc-switch-root-metadata", &provider())
            .expect("unknown root fields should be preserved");
        let written = fs::read_to_string(&path).expect("read updated models");
        assert!(written.contains("version: 2"));
        assert!(written.contains("cc-switch-root-metadata:"));
    }
}
