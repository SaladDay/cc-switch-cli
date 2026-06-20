//! Hermes Agent configuration read/write module (ported from upstream cc-switch).
//!
//! Handles read/write operations on `~/.hermes/config.yaml` (YAML format).
//! Hermes uses additive provider management: all provider configurations
//! coexist in the same config file.
//!
//! ## Example config layout
//!
//! ```yaml
//! model:
//!   default: "anthropic/claude-opus-4-7"
//!   provider: "openrouter"
//!   base_url: "https://openrouter.ai/api/v1"
//!
//! agent:
//!   max_turns: 50
//!   reasoning_effort: "high"
//!
//! custom_providers:
//!   - name: openrouter
//!     base_url: https://openrouter.ai/api/v1
//!     api_key: sk-or-...
//!     model: anthropic/claude-opus-4-7
//!     models:
//!       anthropic/claude-opus-4-7:
//!         context_length: 200000
//!
//! mcp_servers:
//!   filesystem:
//!     command: npx
//!     args: ["-y", "@modelcontextprotocol/server-filesystem"]
//! ```

use crate::config::{atomic_write, create_managed_config_dir_all, get_app_config_dir, home_dir};
use crate::error::AppError;
use crate::services::provider::live_merge;
use crate::settings::{effective_backup_retain_count, get_hermes_override_dir};
use chrono::Local;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub const HERMES_DEFAULT_API_MODE: &str = "chat_completions";
pub const HERMES_API_MODES: [&str; 4] = [
    "chat_completions",
    "anthropic_messages",
    "codex_responses",
    "bedrock_converse",
];

// ============================================================================
// Path Functions
// ============================================================================

/// Get the Hermes config directory.
///
/// Default: `~/.hermes/`. Can be overridden via `settings.hermes_config_dir`.
pub fn get_hermes_dir() -> PathBuf {
    if let Some(override_dir) = get_hermes_override_dir() {
        return override_dir;
    }

    home_dir()
        .map(|home| home.join(".hermes"))
        .unwrap_or_else(|| PathBuf::from(".hermes"))
}

/// Get the Hermes config file path (`<hermes_dir>/config.yaml`).
pub fn get_hermes_config_path() -> PathBuf {
    get_hermes_dir().join("config.yaml")
}

fn hermes_write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

// ============================================================================
// Type Definitions
// ============================================================================

/// Hermes write outcome (kept for upstream API compatibility; current CLI
/// callers do not consume this).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HermesWriteOutcome {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_path: Option<String>,
}

/// Hermes top-level `model:` section.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HermesModelConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    /// Preserve unknown fields for forward compatibility.
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

// ============================================================================
// Core YAML / Text Read Functions
// ============================================================================

/// Read the raw Hermes config file (unparsed). Returns `None` if absent.
pub fn read_hermes_config_source() -> Result<Option<String>, AppError> {
    let path = get_hermes_config_path();
    if !path.exists() {
        return Ok(None);
    }

    fs::read_to_string(&path)
        .map(Some)
        .map_err(|e| AppError::io(&path, e))
}

/// Write raw Hermes config (no backup created; intended for snapshot/restore
/// callers).
pub fn write_hermes_config_source(source: &str) -> Result<(), AppError> {
    let path = get_hermes_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }
    atomic_write(&path, source.as_bytes())
}

/// Read the Hermes config file as `serde_yaml::Value`. Returns an empty
/// `Mapping` if the file is missing or empty.
pub fn read_hermes_config() -> Result<serde_yaml::Value, AppError> {
    let path = get_hermes_config_path();
    if !path.exists() {
        return Ok(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
    }

    let content = fs::read_to_string(&path).map_err(|e| AppError::io(&path, e))?;
    if content.trim().is_empty() {
        return Ok(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
    }

    serde_yaml::from_str(&content)
        .map_err(|e| AppError::Config(format!("Failed to parse Hermes config as YAML: {e}")))
}

/// Read the Hermes config file as `serde_json::Value` for service-layer
/// callers that expose full live config as JSON.
pub fn read_hermes_config_json() -> Result<Value, AppError> {
    let yaml_value = read_hermes_config()?;
    yaml_to_json(&yaml_value)
}

// ============================================================================
// YAML <-> JSON Conversion Helpers (public so e.g. `mcp::hermes_*` can reuse)
// ============================================================================

/// Convert `serde_yaml::Value` to `serde_json::Value`.
pub fn yaml_to_json(yaml: &serde_yaml::Value) -> Result<Value, AppError> {
    let yaml_str = serde_yaml::to_string(yaml)
        .map_err(|e| AppError::Config(format!("Failed to serialize YAML value: {e}")))?;
    serde_yaml::from_str::<Value>(&yaml_str)
        .map_err(|e| AppError::Config(format!("Failed to convert YAML to JSON: {e}")))
}

/// Convert `serde_json::Value` to `serde_yaml::Value`.
pub fn json_to_yaml(json: &Value) -> Result<serde_yaml::Value, AppError> {
    let json_str = serde_json::to_string(json)
        .map_err(|e| AppError::Config(format!("Failed to serialize JSON value: {e}")))?;
    serde_yaml::from_str(&json_str)
        .map_err(|e| AppError::Config(format!("Failed to convert JSON to YAML: {e}")))
}

// ============================================================================
// YAML Section-Level Replacement
// ============================================================================

/// Returns true if `line` is a YAML top-level key: column 0, not a comment,
/// not a sequence item, and contains a `:` followed by space/EOL.
fn is_top_level_key_line(line: &str) -> bool {
    if line.is_empty() {
        return false;
    }
    let first_char = line.as_bytes()[0];
    if first_char == b' ' || first_char == b'\t' || first_char == b'#' || first_char == b'-' {
        return false;
    }
    if let Some(colon_pos) = line.find(':') {
        let after_colon = &line[colon_pos + 1..];
        after_colon.is_empty() || after_colon.starts_with(' ') || after_colon.starts_with('\t')
    } else {
        false
    }
}

/// Locate the byte range of a top-level YAML section. Returns
/// `(start_inclusive, end_exclusive)`, or `None` if the section is absent.
fn find_yaml_section_range(raw: &str, section_key: &str) -> Option<(usize, usize)> {
    let target = format!("{}:", section_key);
    let mut section_start = None;
    let mut offset = 0;

    for line in raw.split('\n') {
        if section_start.is_none() && is_top_level_key_line(line) && line.starts_with(&target) {
            let after_target = &line[target.len()..];
            if after_target.is_empty()
                || after_target.starts_with(' ')
                || after_target.starts_with('\t')
                || after_target.starts_with('\r')
            {
                section_start = Some(offset);
            }
        } else if section_start.is_some() && is_top_level_key_line(line) {
            return Some((section_start.unwrap(), offset));
        }
        offset += line.len() + 1; // +1 for \n
    }

    section_start.map(|start| (start, raw.len()))
}

/// Serialise `key: value` to a YAML fragment.
fn serialize_yaml_section(key: &str, value: &serde_yaml::Value) -> Result<String, AppError> {
    let mut section = serde_yaml::Mapping::new();
    section.insert(serde_yaml::Value::String(key.to_string()), value.clone());
    serde_yaml::to_string(&serde_yaml::Value::Mapping(section))
        .map_err(|e| AppError::Config(format!("Failed to serialize YAML section '{key}': {e}")))
}

/// Replace the named section in `raw`. If the section is absent, append it
/// to the end of the file.
fn replace_yaml_section(
    raw: &str,
    section_key: &str,
    value: &serde_yaml::Value,
) -> Result<String, AppError> {
    let serialized = serialize_yaml_section(section_key, value)?;

    if let Some((start, end)) = find_yaml_section_range(raw, section_key) {
        let mut result = String::with_capacity(raw.len());
        result.push_str(&raw[..start]);
        result.push_str(&serialized);
        let remainder = &raw[end..];
        if !serialized.ends_with('\n') && !remainder.is_empty() && !remainder.starts_with('\n') {
            result.push('\n');
        }
        result.push_str(remainder);
        Ok(result)
    } else {
        let mut result = raw.to_string();
        if !result.is_empty() && !result.ends_with('\n') {
            result.push('\n');
        }
        result.push_str(&serialized);
        if !result.ends_with('\n') {
            result.push('\n');
        }
        Ok(result)
    }
}

// ============================================================================
// Backup & Cleanup
// ============================================================================

fn create_hermes_backup(source: &str) -> Result<PathBuf, AppError> {
    let backup_dir = get_app_config_dir().join("backups").join("hermes");
    create_managed_config_dir_all(&backup_dir)?;

    let base_id = format!("hermes_{}", Local::now().format("%Y%m%d_%H%M%S"));
    let mut filename = format!("{base_id}.yaml");
    let mut backup_path = backup_dir.join(&filename);
    let mut counter = 1;

    while backup_path.exists() {
        filename = format!("{base_id}_{counter}.yaml");
        backup_path = backup_dir.join(&filename);
        counter += 1;
    }

    atomic_write(&backup_path, source.as_bytes())?;
    cleanup_hermes_backups(&backup_dir)?;
    Ok(backup_path)
}

fn cleanup_hermes_backups(dir: &Path) -> Result<(), AppError> {
    let retain = effective_backup_retain_count();
    let mut entries = fs::read_dir(dir)
        .map_err(|e| AppError::io(dir, e))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .map(|ext| ext == "yaml" || ext == "yml")
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();

    if entries.len() <= retain {
        return Ok(());
    }

    entries.sort_by_key(|entry| entry.metadata().and_then(|m| m.modified()).ok());
    let remove_count = entries.len().saturating_sub(retain);
    for entry in entries.into_iter().take(remove_count) {
        if let Err(err) = fs::remove_file(entry.path()) {
            log::warn!(
                "Failed to remove old Hermes config backup {}: {err}",
                entry.path().display()
            );
        }
    }

    Ok(())
}

// ============================================================================
// High-level Section Write
// ============================================================================

fn write_yaml_section_to_config(
    section_key: &str,
    value: &serde_yaml::Value,
) -> Result<HermesWriteOutcome, AppError> {
    let _guard = hermes_write_lock()
        .lock()
        .map_err(|e| AppError::Config(format!("Failed to acquire Hermes write lock: {e}")))?;
    write_yaml_section_to_config_locked(section_key, value)
}

fn write_yaml_section_to_config_locked(
    section_key: &str,
    value: &serde_yaml::Value,
) -> Result<HermesWriteOutcome, AppError> {
    let config_path = get_hermes_config_path();
    let raw = if config_path.exists() {
        fs::read_to_string(&config_path).map_err(|e| AppError::io(&config_path, e))?
    } else {
        String::new()
    };

    let new_raw = replace_yaml_section(&raw, section_key, value)?;

    if new_raw == raw {
        return Ok(HermesWriteOutcome::default());
    }

    let backup_path = if !raw.is_empty() {
        Some(create_hermes_backup(&raw)?)
    } else {
        None
    };

    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    atomic_write(&config_path, new_raw.as_bytes())?;

    log::debug!(
        "Hermes config section '{}' written to {:?}",
        section_key,
        config_path
    );
    Ok(HermesWriteOutcome {
        backup_path: backup_path.map(|p| p.display().to_string()),
    })
}

// ============================================================================
// Provider Helpers: models array <-> dict, key normalization, source marker
// ============================================================================

/// Convert the UI-friendly array form of `models` to Hermes' YAML dict shape.
///
/// Entries with a missing/empty `id` are dropped. The `id` field is hoisted
/// to be the map key. Insertion order is preserved (requires the
/// `preserve_order` feature on `serde_json`).
fn models_array_to_dict(array: Vec<Value>) -> Value {
    let mut map = Map::new();
    for item in array {
        let Value::Object(mut obj) = item else {
            continue;
        };
        let Some(id) = obj
            .remove("id")
            .and_then(|v| v.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        map.insert(id, Value::Object(obj));
    }
    Value::Object(map)
}

/// Inverse of [`models_array_to_dict`]: YAML dict -> ordered array, with
/// `id` re-injected as an object field.
fn models_dict_to_array(dict: Map<String, Value>) -> Value {
    let mut out = Vec::with_capacity(dict.len());
    for (id, value) in dict {
        let mut obj = match value {
            Value::Object(obj) => obj,
            Value::Null => Map::new(),
            other => {
                log::warn!("Unexpected Hermes model entry for '{id}': {other:?}, skipping");
                continue;
            }
        };
        obj.insert("id".to_string(), Value::String(id));
        out.push(Value::Object(obj));
    }
    Value::Array(out)
}

/// Source marker: entries read from Hermes v12+ `providers:` dict carry
/// this field so the UI can render them read-only.
pub const PROVIDER_SOURCE_FIELD: &str = "_cc_source";
pub const PROVIDER_SOURCE_CUSTOM_LIST: &str = "custom_providers";
pub const PROVIDER_SOURCE_DICT: &str = "providers_dict";

/// Rewrite historical camelCase keys to Hermes' snake_case schema.
fn sanitize_hermes_provider_keys(config: &mut Value) {
    const KEY_ALIASES: &[(&str, &str)] = &[
        ("baseUrl", "base_url"),
        ("apiKey", "api_key"),
        ("apiMode", "api_mode"),
        ("maxTokens", "max_tokens"),
        ("contextLength", "context_length"),
    ];
    // Legacy fields that are neither valid Hermes keys nor mappable to
    // `api_mode`; also strip UI-only source markers before writing YAML.
    const LEGACY_FIELDS_TO_DROP: &[&str] = &["api", PROVIDER_SOURCE_FIELD, "provider_key"];

    let Some(obj) = config.as_object_mut() else {
        return;
    };

    for (from, to) in KEY_ALIASES {
        if let Some(val) = obj.remove(*from) {
            obj.entry((*to).to_string()).or_insert(val);
        }
    }

    for field in LEGACY_FIELDS_TO_DROP {
        obj.remove(*field);
    }
}

/// Pre-write: if `models` is a JSON array, convert it in-place to a dict.
fn normalize_provider_models_for_write(config: &mut Value) {
    let Some(obj) = config.as_object_mut() else {
        return;
    };
    let Some(models_val) = obj.get_mut("models") else {
        return;
    };
    if models_val.is_array() {
        let taken = std::mem::take(models_val);
        if let Value::Array(arr) = taken {
            *models_val = models_array_to_dict(arr);
        }
    }
}

/// Post-read: if `models` is a JSON dict, convert it in-place to an ordered
/// array.
fn denormalize_provider_models_for_read(config: &mut Value) {
    let Some(obj) = config.as_object_mut() else {
        return;
    };
    let Some(models_val) = obj.get_mut("models") else {
        return;
    };
    if models_val.is_object() {
        let taken = std::mem::take(models_val);
        if let Value::Object(map) = taken {
            *models_val = models_dict_to_array(map);
        }
    }
}

/// Normalise a single `providers:` dict entry into the same shape as items
/// in the `custom_providers:` list.
fn normalize_providers_dict_entry(
    key: &str,
    entry: &serde_yaml::Value,
) -> Result<Option<Value>, AppError> {
    if !entry.is_mapping() {
        return Ok(None);
    }
    let mut json_val = yaml_to_json(entry)?;
    let Some(obj) = json_val.as_object_mut() else {
        return Ok(None);
    };
    let resolved_name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| key.trim().to_string());
    if resolved_name.is_empty() {
        return Ok(None);
    }
    obj.insert("name".to_string(), json!(resolved_name));
    obj.insert("provider_key".to_string(), json!(key));
    obj.insert(
        PROVIDER_SOURCE_FIELD.to_string(),
        json!(PROVIDER_SOURCE_DICT),
    );
    Ok(Some(json_val))
}

/// Collect provider entries from the `providers:` dict.
fn read_providers_dict_entries(config: &serde_yaml::Value) -> Vec<(String, Value)> {
    let Some(mapping) = config.get("providers").and_then(|v| v.as_mapping()) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(mapping.len());
    for (k, v) in mapping {
        let Some(key_str) = k.as_str().map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        match normalize_providers_dict_entry(key_str, v) {
            Ok(Some(entry)) => {
                let name = entry
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or(key_str)
                    .to_string();
                out.push((name, entry));
            }
            Ok(None) => {
                log::debug!("Skipping Hermes providers['{key_str}']: not a mapping");
            }
            Err(e) => {
                log::warn!("Failed to normalize Hermes providers['{key_str}']: {e}");
            }
        }
    }
    out
}

/// Returns true when `name` only appears in the `providers:` dict (i.e. it
/// is read-only from CC Switch's perspective).
fn is_dict_only_provider(config: &serde_yaml::Value, name: &str) -> bool {
    let list_has = config
        .get("custom_providers")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .any(|item| item.get("name").and_then(|n| n.as_str()) == Some(name))
        })
        .unwrap_or(false);
    if list_has {
        return false;
    }
    config
        .get("providers")
        .and_then(|v| v.as_mapping())
        .map(|m| {
            m.iter().any(|(k, v)| {
                let key_matches = k.as_str() == Some(name);
                let name_matches = v
                    .get("name")
                    .and_then(|n| n.as_str())
                    .map(|s| s == name)
                    .unwrap_or(false);
                (key_matches || name_matches) && v.is_mapping()
            })
        })
        .unwrap_or(false)
}

/// Reject writes that target providers living in the `providers:` overlay
/// dict.
fn ensure_provider_writable(
    config: &serde_yaml::Value,
    name: &str,
    verb: &str,
) -> Result<(), AppError> {
    if is_dict_only_provider(config, name) {
        return Err(AppError::Config(format!(
            "Provider '{name}' is managed by the Hermes 'providers:' dict \
             — please {verb} it via the Hermes web UI"
        )));
    }
    Ok(())
}

// ============================================================================
// Provider Public API
// ============================================================================

/// Get all providers indexed by name.
///
/// Merges two sources:
/// - `custom_providers:` list (writable from CC Switch)
/// - `providers:` dict (v12+; read-only, tagged with
///   `_cc_source = "providers_dict"`)
///
/// On name collision, the list wins. The `models` field is denormalised
/// from the YAML dict back to an ordered array.
pub fn get_providers() -> Result<IndexMap<String, Value>, AppError> {
    let config = read_hermes_config()?;
    let mut map = IndexMap::new();

    if let Some(seq) = config.get("custom_providers").and_then(|v| v.as_sequence()) {
        for item in seq {
            if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                match yaml_to_json(item) {
                    Ok(mut json_val) => {
                        sanitize_hermes_provider_keys(&mut json_val);
                        denormalize_provider_models_for_read(&mut json_val);
                        if let Some(obj) = json_val.as_object_mut() {
                            obj.insert(
                                PROVIDER_SOURCE_FIELD.to_string(),
                                json!(PROVIDER_SOURCE_CUSTOM_LIST),
                            );
                        }
                        map.insert(name.to_string(), json_val);
                    }
                    Err(e) => {
                        log::warn!("Failed to convert Hermes provider '{name}' to JSON: {e}");
                    }
                }
            }
        }
    }

    for (name, mut entry) in read_providers_dict_entries(&config) {
        if map.contains_key(&name) {
            continue; // List wins on name collision.
        }
        denormalize_provider_models_for_read(&mut entry);
        map.insert(name, entry);
    }

    Ok(map)
}

/// Get a single provider by name.
pub fn get_provider(name: &str) -> Result<Option<Value>, AppError> {
    Ok(get_providers()?.get(name).cloned())
}

/// Insert or update a provider in the `custom_providers:` list.
///
/// - Matches existing entries by `name`.
/// - Pre-write: camelCase keys are normalised to snake_case, and `models`
///   arrays are converted to dicts.
/// - Mirrors the first `models` key onto the top-level `model:` field
///   (this is what Hermes actually reads on activation).
/// - For existing entries, performs a forward-compat merge: fields present
///   on disk but not submitted by the UI are preserved.
/// - Holds the write lock end-to-end to avoid TOCTOU races.
pub fn set_provider(name: &str, provider_config: Value) -> Result<HermesWriteOutcome, AppError> {
    set_provider_with_resolution(
        name,
        provider_config,
        live_merge::ConflictPolicy::PreferIncoming.into(),
    )
}

pub fn set_provider_with_resolution(
    name: &str,
    provider_config: Value,
    resolution: live_merge::ConflictResolution<'_>,
) -> Result<HermesWriteOutcome, AppError> {
    let providers_value = prepare_provider_with_resolution(name, provider_config, resolution)?;
    write_prepared_providers(&providers_value)
}

pub fn prepare_provider_with_resolution(
    name: &str,
    provider_config: Value,
    resolution: live_merge::ConflictResolution<'_>,
) -> Result<serde_yaml::Value, AppError> {
    let config = read_hermes_config()?;
    ensure_provider_writable(&config, name, "edit")?;

    let mut providers: Vec<serde_yaml::Value> = config
        .get("custom_providers")
        .and_then(|v| v.as_sequence())
        .cloned()
        .unwrap_or_default();

    let mut normalized = provider_config;
    sanitize_hermes_provider_keys(&mut normalized);
    normalize_provider_models_for_write(&mut normalized);

    let first_model_id = normalized
        .get("models")
        .and_then(|v| v.as_object())
        .and_then(|obj| obj.keys().next())
        .cloned();

    let mut submitted = normalized;
    if let Some(obj) = submitted.as_object_mut() {
        obj.insert("name".to_string(), Value::String(name.to_string()));
        if let Some(model_id) = first_model_id {
            obj.insert("model".to_string(), Value::String(model_id));
        } else {
            obj.remove("model");
        }
    }

    if let Some(existing) = providers
        .iter_mut()
        .find(|p| p.get("name").and_then(|n| n.as_str()) == Some(name))
    {
        let existing_json = yaml_to_json(existing)?;
        let merged = live_merge::merge_json_live(
            &crate::app_config::AppType::Hermes,
            format!("config.yaml custom_providers.{name}"),
            existing_json,
            &submitted,
            resolution,
        )?;
        *existing = json_to_yaml(&merged)?;
    } else {
        providers.push(json_to_yaml(&submitted)?);
    }

    Ok(serde_yaml::Value::Sequence(providers))
}

pub fn write_prepared_providers(
    providers_value: &serde_yaml::Value,
) -> Result<HermesWriteOutcome, AppError> {
    write_yaml_section_to_config("custom_providers", providers_value)
}

/// Remove a provider from the `custom_providers:` list.
pub fn remove_provider(name: &str) -> Result<HermesWriteOutcome, AppError> {
    let _guard = hermes_write_lock().lock()?;
    let config = read_hermes_config()?;

    ensure_provider_writable(&config, name, "remove")?;

    let mut providers: Vec<serde_yaml::Value> = config
        .get("custom_providers")
        .and_then(|v| v.as_sequence())
        .cloned()
        .unwrap_or_default();

    let original_len = providers.len();
    providers.retain(|p| p.get("name").and_then(|n| n.as_str()) != Some(name));
    if providers.len() == original_len {
        return Ok(HermesWriteOutcome::default());
    }

    let providers_value = serde_yaml::Value::Sequence(providers);
    write_yaml_section_to_config_locked("custom_providers", &providers_value)
}

// ============================================================================
// Current Provider Helpers
// ============================================================================

fn primary_model_id_from_value(value: &Value) -> Option<String> {
    value
        .get("models")
        .and_then(Value::as_array)
        .and_then(|models| models.first())
        .and_then(|model| model.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn provider_matches_model(provider: &Value, model_id: &str) -> bool {
    let model_id = model_id.trim();
    if model_id.is_empty() {
        return false;
    }

    provider
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|value| value == model_id)
        || provider
            .get("models")
            .and_then(Value::as_object)
            .is_some_and(|models| models.contains_key(model_id))
        || provider
            .get("models")
            .and_then(Value::as_array)
            .is_some_and(|models| {
                models.iter().any(|model| {
                    model
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .is_some_and(|value| value == model_id)
                })
            })
}

/// Get the currently active provider id (driven by top-level
/// `model.provider`).
pub fn get_current_provider_id() -> Result<Option<String>, AppError> {
    let config = read_hermes_config_json()?;
    let Some(model) = config.get("model").and_then(Value::as_object) else {
        return Ok(None);
    };

    let provider_ref = model
        .get("provider")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();

    if !provider_ref.is_empty() && get_providers()?.contains_key(provider_ref) {
        return Ok(Some(provider_ref.to_string()));
    }

    if provider_ref == "custom" {
        let default_model = model
            .get("default")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(default_model) = default_model {
            for (id, provider) in get_providers()? {
                if provider_matches_model(&provider, default_model) {
                    return Ok(Some(id));
                }
            }
        }
    }

    Ok(None)
}

/// Switch to a given provider by writing the top-level `model:` section.
///
/// `model.provider` is always updated; `model.default` is only overwritten
/// when the new provider declares at least one model — otherwise the old
/// value is preserved to avoid leaving Hermes without an available model.
pub fn set_current_provider(id: &str, provider: &Value) -> Result<HermesWriteOutcome, AppError> {
    apply_switch_defaults(id, provider)
}

// ============================================================================
// Model Section
// ============================================================================

/// Read the top-level `model:` section.
pub fn get_model_config() -> Result<Option<HermesModelConfig>, AppError> {
    let config = read_hermes_config()?;
    let Some(model_value) = config.get("model") else {
        return Ok(None);
    };
    let json_val = yaml_to_json(model_value)?;
    let model = serde_json::from_value(json_val)
        .map_err(|e| AppError::Config(format!("Failed to parse Hermes model config: {e}")))?;
    Ok(Some(model))
}

/// Write the top-level `model:` section.
pub fn set_model_config(model: &HermesModelConfig) -> Result<HermesWriteOutcome, AppError> {
    let json_val =
        serde_json::to_value(model).map_err(|e| AppError::JsonSerialize { source: e })?;
    let yaml_val = json_to_yaml(&json_val)?;
    write_yaml_section_to_config("model", &yaml_val)
}

/// Refresh the top-level `model:` defaults when switching providers.
pub fn apply_switch_defaults(
    provider_id: &str,
    settings_config: &Value,
) -> Result<HermesWriteOutcome, AppError> {
    let first_model_id = primary_model_id_from_value(settings_config);

    let current = get_model_config()?.unwrap_or_default();
    let merged = HermesModelConfig {
        default: first_model_id.or(current.default.clone()),
        provider: Some(provider_id.to_string()),
        ..current
    };
    set_model_config(&merged)
}

// ============================================================================
// MCP Section Access (consumed by `mcp::hermes_*` helpers)
// ============================================================================

/// Get the `mcp_servers:` section.
pub fn get_mcp_servers_yaml() -> Result<serde_yaml::Mapping, AppError> {
    let config = read_hermes_config()?;
    Ok(config
        .get("mcp_servers")
        .and_then(|v| v.as_mapping())
        .cloned()
        .unwrap_or_default())
}

/// Read-modify-write the `mcp_servers:` section under the write lock.
pub fn update_mcp_servers_yaml<F>(updater: F) -> Result<(), AppError>
where
    F: FnOnce(&mut serde_yaml::Mapping) -> Result<(), AppError>,
{
    let _guard = hermes_write_lock()
        .lock()
        .map_err(|e| AppError::Config(format!("Failed to acquire Hermes write lock: {e}")))?;
    let config = read_hermes_config()?;
    let mut servers = config
        .get("mcp_servers")
        .and_then(|v| v.as_mapping())
        .cloned()
        .unwrap_or_default();
    updater(&mut servers)?;
    let value = serde_yaml::Value::Mapping(servers);
    write_yaml_section_to_config_locked("mcp_servers", &value)?;
    Ok(())
}

// ============================================================================
// Memory Files (~/.hermes/memories/{MEMORY,USER}.md)
// ============================================================================

/// The two Hermes memory blob kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryKind {
    Memory,
    User,
}

impl MemoryKind {
    pub fn filename(self) -> &'static str {
        match self {
            Self::Memory => "MEMORY.md",
            Self::User => "USER.md",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::User => "user",
        }
    }
}

fn memories_dir() -> PathBuf {
    get_hermes_dir().join("memories")
}

/// Read a Hermes memory file. Returns an empty string when the file does
/// not exist.
pub fn read_memory(kind: MemoryKind) -> Result<String, AppError> {
    let path = memories_dir().join(kind.filename());
    match fs::read_to_string(&path) {
        Ok(content) => Ok(content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(AppError::io(&path, e)),
    }
}

/// Atomically write a Hermes memory file.
pub fn write_memory(kind: MemoryKind, content: &str) -> Result<(), AppError> {
    let path = memories_dir().join(kind.filename());
    atomic_write(&path, content.as_bytes())
}

/// Per-blob character budget plus enable flags. Missing fields fall back
/// to defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesMemoryLimits {
    pub memory: usize,
    pub user: usize,
    pub memory_enabled: bool,
    pub user_enabled: bool,
}

impl Default for HermesMemoryLimits {
    fn default() -> Self {
        Self {
            memory: 2200,
            user: 1375,
            memory_enabled: true,
            user_enabled: true,
        }
    }
}

/// Toggle a memory blob on/off while preserving the rest of the `memory:`
/// section.
pub fn set_memory_enabled(kind: MemoryKind, enabled: bool) -> Result<HermesWriteOutcome, AppError> {
    let _guard = hermes_write_lock().lock()?;
    let config = read_hermes_config()?;

    let mut memory = match config.get("memory") {
        Some(serde_yaml::Value::Mapping(m)) => m.clone(),
        _ => serde_yaml::Mapping::new(),
    };

    let key = match kind {
        MemoryKind::Memory => "memory_enabled",
        MemoryKind::User => "user_profile_enabled",
    };
    memory.insert(
        serde_yaml::Value::String(key.to_string()),
        serde_yaml::Value::Bool(enabled),
    );

    write_yaml_section_to_config_locked("memory", &serde_yaml::Value::Mapping(memory))
}

/// Read memory budgets + enable flags. Falls back to defaults for any
/// field that fails to parse.
pub fn read_memory_limits() -> Result<HermesMemoryLimits, AppError> {
    let mut out = HermesMemoryLimits::default();
    let config = read_hermes_config()?;
    let Some(memory) = config.get("memory") else {
        return Ok(out);
    };

    if let Some(v) = memory.get("memory_char_limit").and_then(|v| v.as_u64()) {
        out.memory = v as usize;
    }
    if let Some(v) = memory.get("user_char_limit").and_then(|v| v.as_u64()) {
        out.user = v as usize;
    }
    if let Some(v) = memory.get("memory_enabled").and_then(|v| v.as_bool()) {
        out.memory_enabled = v;
    }
    if let Some(v) = memory.get("user_profile_enabled").and_then(|v| v.as_bool()) {
        out.user_enabled = v;
    }

    Ok(out)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnvGuard;
    use serde_json::json;
    use serial_test::serial;

    fn with_test_home<T>(test_fn: impl FnOnce() -> T) -> T {
        let tmp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(tmp.path());
        test_fn()
    }

    #[test]
    fn sanitize_rewrites_camel_case_aliases() {
        let mut v = json!({
            "baseUrl": "https://x",
            "apiKey": "k",
            "maxTokens": 4096,
            "contextLength": 200000,
        });
        sanitize_hermes_provider_keys(&mut v);
        let obj = v.as_object().unwrap();
        assert!(obj.contains_key("base_url"));
        assert!(obj.contains_key("api_key"));
        assert!(obj.contains_key("max_tokens"));
        assert!(obj.contains_key("context_length"));
        assert!(!obj.contains_key("baseUrl"));
    }

    #[test]
    fn sanitize_drops_legacy_fields() {
        let mut v = json!({
            "api": "openai-completions",
            "_cc_source": "custom_providers",
            "provider_key": "x",
            "base_url": "https://x",
        });
        sanitize_hermes_provider_keys(&mut v);
        let obj = v.as_object().unwrap();
        assert!(!obj.contains_key("api"));
        assert!(!obj.contains_key("_cc_source"));
        assert!(!obj.contains_key("provider_key"));
        assert!(obj.contains_key("base_url"));
    }

    #[test]
    fn models_array_to_dict_roundtrip() {
        let arr = vec![
            json!({"id": "foo", "context_length": 200000}),
            json!({"id": "bar"}),
            json!({"id": "  "}),
            json!({"context_length": 1}),
        ];
        let dict = models_array_to_dict(arr);
        let map = dict.as_object().unwrap();
        assert_eq!(map.len(), 2);
        assert!(map.contains_key("foo"));
        assert!(map.contains_key("bar"));

        let back = models_dict_to_array(map.clone());
        let arr = back.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        let ids: Vec<&str> = arr.iter().filter_map(|v| v["id"].as_str()).collect();
        assert!(ids.contains(&"foo"));
        assert!(ids.contains(&"bar"));
    }

    #[test]
    #[serial(home_settings)]
    fn set_and_get_provider_roundtrip() {
        with_test_home(|| {
            let provider = json!({
                "base_url": "https://example.com/v1",
                "api_key": "sk-test",
                "models": [
                    {"id": "gpt-4o", "context_length": 128000},
                    {"id": "gpt-3.5"}
                ]
            });
            set_provider("acme", provider).unwrap();

            let got = get_provider("acme").unwrap().expect("provider exists");
            assert_eq!(got["base_url"], "https://example.com/v1");
            // models should be in array form
            let arr = got["models"].as_array().unwrap();
            assert_eq!(arr.len(), 2);
            // first model id reflected to top-level
            // (read goes through get_providers which strips dict form back to array)
            let yaml = read_hermes_config().unwrap();
            let seq = yaml["custom_providers"].as_sequence().unwrap();
            let entry = seq
                .iter()
                .find(|p| p["name"].as_str() == Some("acme"))
                .unwrap();
            assert_eq!(entry["model"].as_str(), Some("gpt-4o"));
            // models in YAML must be a mapping
            assert!(entry["models"].is_mapping());
        });
    }

    #[test]
    #[serial(home_settings)]
    fn remove_provider_works() {
        with_test_home(|| {
            set_provider("foo", json!({"base_url": "u1"})).unwrap();
            set_provider("bar", json!({"base_url": "u2"})).unwrap();
            remove_provider("foo").unwrap();
            let providers = get_providers().unwrap();
            assert!(!providers.contains_key("foo"));
            assert!(providers.contains_key("bar"));
        });
    }

    #[test]
    #[serial(home_settings)]
    fn dict_only_provider_is_read_only() {
        with_test_home(|| {
            let raw = "providers:\n  remote:\n    name: remote\n    base_url: u\n";
            write_hermes_config_source(raw).unwrap();
            let providers = get_providers().unwrap();
            let entry = providers.get("remote").unwrap();
            assert_eq!(entry["_cc_source"], "providers_dict");
            // Edits / removes are rejected
            let err = set_provider("remote", json!({"base_url": "x"})).unwrap_err();
            assert!(err.to_string().contains("providers"));
            let err = remove_provider("remote").unwrap_err();
            assert!(err.to_string().contains("providers"));
        });
    }

    #[test]
    #[serial(home_settings)]
    fn section_writes_preserve_other_sections() {
        with_test_home(|| {
            let raw = "# leading comment\n\
agent:\n  max_turns: 5\n\
custom_providers: []\n";
            write_hermes_config_source(raw).unwrap();
            set_provider("foo", json!({"base_url": "u"})).unwrap();
            let text = read_hermes_config_source().unwrap().unwrap();
            assert!(text.contains("# leading comment"));
            assert!(text.contains("agent:"));
            assert!(text.contains("max_turns: 5"));
            assert!(text.contains("foo"));
        });
    }

    #[test]
    #[serial(home_settings)]
    fn memory_round_trip() {
        with_test_home(|| {
            write_memory(MemoryKind::Memory, "hello").unwrap();
            assert_eq!(read_memory(MemoryKind::Memory).unwrap(), "hello");
            assert_eq!(read_memory(MemoryKind::User).unwrap(), "");
        });
    }

    #[test]
    #[serial(home_settings)]
    fn memory_limits_defaults_when_absent() {
        with_test_home(|| {
            let limits = read_memory_limits().unwrap();
            assert_eq!(limits.memory, 2200);
            assert_eq!(limits.user, 1375);
            assert!(limits.memory_enabled);
            assert!(limits.user_enabled);
        });
    }

    #[test]
    #[serial(home_settings)]
    fn memory_set_enabled_preserves_other_fields() {
        with_test_home(|| {
            let raw = "memory:\n  memory_char_limit: 4000\n  user_char_limit: 2000\n";
            write_hermes_config_source(raw).unwrap();
            set_memory_enabled(MemoryKind::Memory, false).unwrap();
            let limits = read_memory_limits().unwrap();
            assert_eq!(limits.memory, 4000);
            assert!(!limits.memory_enabled);
            assert!(limits.user_enabled);
        });
    }

    #[test]
    #[serial(home_settings)]
    fn set_current_provider_writes_model_section() {
        with_test_home(|| {
            set_provider(
                "acme",
                json!({
                    "base_url": "https://x",
                    "models": [{"id": "model-a"}]
                }),
            )
            .unwrap();
            set_current_provider(
                "acme",
                &json!({
                    "models": [{"id": "model-a"}]
                }),
            )
            .unwrap();
            let id = get_current_provider_id().unwrap().unwrap();
            assert_eq!(id, "acme");

            let model = get_model_config().unwrap().unwrap();
            assert_eq!(model.provider.as_deref(), Some("acme"));
        });
    }
}
