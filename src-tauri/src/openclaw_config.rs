use crate::config::{atomic_write, get_app_config_dir, home_dir};
use crate::error::AppError;
use crate::provider::OpenClawProviderConfig;
use crate::settings::{effective_backup_retain_count, get_openclaw_override_dir};
use chrono::Local;
use indexmap::IndexMap;
use json_five::parser::{FormatConfiguration, TrailingComma};
use json_five::rt::parser::{
    from_str as rt_from_str, JSONKeyValuePair as RtJSONKeyValuePair,
    JSONObjectContext as RtJSONObjectContext, JSONText as RtJSONText, JSONValue as RtJSONValue,
    KeyValuePairContext as RtKeyValuePairContext,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const OPENCLAW_DEFAULT_SOURCE: &str =
    "{\n  models: {\n    mode: 'merge',\n    providers: {},\n  },\n}\n";
/// Returns the OpenClaw config directory path.
///
/// Priority: cc-switch settings override > `$HOME/.openclaw`
pub fn get_openclaw_dir() -> PathBuf {
    if let Some(override_dir) = get_openclaw_override_dir() {
        return override_dir;
    }

    home_dir()
        .map(|home| home.join(".openclaw"))
        .unwrap_or_else(|| PathBuf::from(".openclaw"))
}

pub fn get_openclaw_config_path() -> PathBuf {
    get_openclaw_dir().join("openclaw.json")
}

fn default_config() -> Value {
    json!({
        "models": {
            "mode": "merge",
            "providers": {}
        }
    })
}

fn openclaw_write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OpenClawHealthWarning {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OpenClawWriteOutcome {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<OpenClawHealthWarning>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenClawDefaultModel {
    pub primary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallbacks: Vec<String>,
    #[serde(flatten, default, skip_serializing_if = "HashMap::is_empty")]
    pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct OpenClawModelCatalogEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(flatten, default, skip_serializing_if = "HashMap::is_empty")]
    pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct OpenClawAgentsDefaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<OpenClawDefaultModel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<HashMap<String, OpenClawModelCatalogEntry>>,
    #[serde(flatten, default, skip_serializing_if = "HashMap::is_empty")]
    pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct OpenClawEnvConfig {
    #[serde(flatten, default, skip_serializing_if = "HashMap::is_empty")]
    pub vars: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct OpenClawToolsConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
    #[serde(flatten, default, skip_serializing_if = "HashMap::is_empty")]
    pub extra: HashMap<String, Value>,
}

pub fn read_openclaw_config() -> Result<Value, AppError> {
    let path = get_openclaw_config_path();
    if !path.exists() {
        return Ok(default_config());
    }

    let content = fs::read_to_string(&path).map_err(|e| AppError::io(&path, e))?;
    json5::from_str(&content)
        .map_err(|e| AppError::Config(format!("Failed to parse OpenClaw config as JSON5: {e}")))
}

pub fn read_openclaw_config_source() -> Result<Option<String>, AppError> {
    let path = get_openclaw_config_path();
    if !path.exists() {
        return Ok(None);
    }

    fs::read_to_string(&path)
        .map(Some)
        .map_err(|e| AppError::io(&path, e))
}

pub fn write_openclaw_config_source(source: &str) -> Result<(), AppError> {
    let path = get_openclaw_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    atomic_write(&path, source.as_bytes())
}

pub fn scan_openclaw_config_health() -> Result<Vec<OpenClawHealthWarning>, AppError> {
    let path = get_openclaw_config_path();
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(&path).map_err(|e| AppError::io(&path, e))?;
    match json5::from_str::<Value>(&content) {
        Ok(config) => Ok(scan_openclaw_health_from_value(&config)),
        Err(err) => Ok(vec![OpenClawHealthWarning {
            code: "config_parse_failed".to_string(),
            message: format!("OpenClaw config could not be parsed as JSON5: {err}"),
            path: Some(path.display().to_string()),
        }]),
    }
}

struct OpenClawConfigDocument {
    path: PathBuf,
    original_source: Option<String>,
    text: RtJSONText,
}

impl OpenClawConfigDocument {
    fn load() -> Result<Self, AppError> {
        let path = get_openclaw_config_path();
        let original_source = if path.exists() {
            Some(fs::read_to_string(&path).map_err(|e| AppError::io(&path, e))?)
        } else {
            None
        };

        let source = original_source
            .clone()
            .unwrap_or_else(|| OPENCLAW_DEFAULT_SOURCE.to_string());
        let text = rt_from_str(&source).map_err(|e| {
            AppError::Config(format!(
                "Failed to parse OpenClaw config as round-trip JSON5 document: {}",
                e.message
            ))
        })?;

        Ok(Self {
            path,
            original_source,
            text,
        })
    }

    fn set_root_section(&mut self, key: &str, value: &Value) -> Result<(), AppError> {
        let RtJSONValue::JSONObject {
            key_value_pairs,
            context,
        } = &mut self.text.value
        else {
            return Err(AppError::Config(
                "OpenClaw config root must be a JSON5 object".to_string(),
            ));
        };

        if key_value_pairs.is_empty()
            && context
                .as_ref()
                .map(|ctx| ctx.wsc.0.is_empty())
                .unwrap_or(true)
        {
            *context = Some(RtJSONObjectContext {
                wsc: ("\n  ".to_string(),),
            });
        }

        let leading_ws = context
            .as_ref()
            .map(|ctx| ctx.wsc.0.clone())
            .unwrap_or_default();
        let entry_separator_ws = derive_entry_separator(&leading_ws);
        let child_indent = extract_trailing_indent(&leading_ws);
        let new_value = value_to_rt_value(key, value, &child_indent)?;

        if let Some(existing) = key_value_pairs
            .iter_mut()
            .find(|pair| json5_key_name(&pair.key).as_deref() == Some(key))
        {
            existing.value = new_value;
            return Ok(());
        }

        let new_pair = if let Some(last_pair) = key_value_pairs.last_mut() {
            let last_ctx = ensure_kvp_context(last_pair);
            let closing_ws = if let Some(after_comma) = last_ctx.wsc.3.clone() {
                last_ctx.wsc.3 = Some(entry_separator_ws.clone());
                after_comma
            } else {
                let closing_ws = std::mem::take(&mut last_ctx.wsc.2);
                last_ctx.wsc.3 = Some(entry_separator_ws.clone());
                closing_ws
            };

            make_root_pair(key, new_value, closing_ws)
        } else {
            make_root_pair(
                key,
                new_value,
                derive_closing_ws_from_separator(&leading_ws),
            )
        };

        key_value_pairs.push(new_pair);
        Ok(())
    }

    fn save(self) -> Result<OpenClawWriteOutcome, AppError> {
        let _guard = openclaw_write_lock().lock()?;

        let current_source = if self.path.exists() {
            Some(fs::read_to_string(&self.path).map_err(|e| AppError::io(&self.path, e))?)
        } else {
            None
        };

        if current_source != self.original_source {
            return Err(AppError::Config(
                "OpenClaw config changed on disk. Please reload and try again.".to_string(),
            ));
        }

        let next_source = self.text.to_string();
        if current_source.as_deref() == Some(next_source.as_str()) {
            let warnings = scan_openclaw_health_from_value(
                &json5::from_str::<Value>(&next_source).map_err(|e| {
                    AppError::Config(format!(
                        "Failed to parse unchanged OpenClaw config as JSON5: {e}"
                    ))
                })?,
            );

            return Ok(OpenClawWriteOutcome {
                backup_path: None,
                warnings,
            });
        }

        let backup_path = current_source
            .as_ref()
            .map(|source| create_openclaw_backup(source))
            .transpose()?
            .map(|path| path.display().to_string());

        atomic_write(&self.path, next_source.as_bytes())?;

        let warnings = scan_openclaw_health_from_value(
            &json5::from_str::<Value>(&next_source).map_err(|e| {
                AppError::Config(format!(
                    "Failed to parse newly written OpenClaw config as JSON5: {e}"
                ))
            })?,
        );

        log::debug!("OpenClaw config written to {:?}", self.path);
        Ok(OpenClawWriteOutcome {
            backup_path,
            warnings,
        })
    }
}

fn write_root_section(section: &str, value: &Value) -> Result<OpenClawWriteOutcome, AppError> {
    let mut document = OpenClawConfigDocument::load()?;
    document.set_root_section(section, value)?;
    document.save()
}

fn create_openclaw_backup(source: &str) -> Result<PathBuf, AppError> {
    let backup_dir = get_app_config_dir().join("backups").join("openclaw");
    fs::create_dir_all(&backup_dir).map_err(|e| AppError::io(&backup_dir, e))?;

    let base_id = format!("openclaw_{}", Local::now().format("%Y%m%d_%H%M%S"));
    let mut filename = format!("{base_id}.json5");
    let mut backup_path = backup_dir.join(&filename);
    let mut counter = 1;

    while backup_path.exists() {
        filename = format!("{base_id}_{counter}.json5");
        backup_path = backup_dir.join(&filename);
        counter += 1;
    }

    atomic_write(&backup_path, source.as_bytes())?;
    cleanup_openclaw_backups(&backup_dir)?;
    Ok(backup_path)
}

fn cleanup_openclaw_backups(dir: &Path) -> Result<(), AppError> {
    let retain = effective_backup_retain_count();
    let mut entries = fs::read_dir(dir)
        .map_err(|e| AppError::io(dir, e))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .map(|ext| ext == "json5" || ext == "json")
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
                "Failed to remove old OpenClaw config backup {}: {err}",
                entry.path().display()
            );
        }
    }

    Ok(())
}

fn ensure_object(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_object() {
        *value = Value::Object(Map::new());
    }
    value
        .as_object_mut()
        .expect("value should be object after normalization")
}

fn ensure_kvp_context(pair: &mut RtJSONKeyValuePair) -> &mut RtKeyValuePairContext {
    pair.context.get_or_insert_with(|| RtKeyValuePairContext {
        wsc: (String::new(), " ".to_string(), String::new(), None),
    })
}

fn extract_trailing_indent(separator_ws: &str) -> String {
    separator_ws
        .rsplit_once('\n')
        .map(|(_, tail)| tail.to_string())
        .unwrap_or_default()
}

fn derive_closing_ws_from_separator(separator_ws: &str) -> String {
    let Some((prefix, indent)) = separator_ws.rsplit_once('\n') else {
        return String::new();
    };

    let reduced_indent = if indent.ends_with('\t') {
        &indent[..indent.len().saturating_sub(1)]
    } else if indent.ends_with("  ") {
        &indent[..indent.len().saturating_sub(2)]
    } else if indent.ends_with(' ') {
        &indent[..indent.len().saturating_sub(1)]
    } else {
        indent
    };

    format!("{prefix}\n{reduced_indent}")
}

fn derive_entry_separator(leading_ws: &str) -> String {
    if leading_ws.is_empty() {
        return String::new();
    }

    if leading_ws.contains('\n') {
        return format!("\n{}", extract_trailing_indent(leading_ws));
    }

    String::new()
}

fn is_empty_object(value: &Value) -> bool {
    value
        .as_object()
        .map(|object| object.is_empty())
        .unwrap_or(false)
}

fn should_use_precise_empty_object_fallback(section: &str, value: &Value) -> bool {
    match section {
        "models" => value
            .as_object()
            .and_then(|models| models.get("providers"))
            .map(is_empty_object)
            .unwrap_or(false),
        "tools" => is_empty_object(value),
        _ => false,
    }
}

fn serialize_json5_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '\'' => escaped.push_str("\\'"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0C}' => escaped.push_str("\\f"),
            ch if ch.is_control() => {
                let code = ch as u32;
                escaped.push_str(&format!("\\u{:04X}", code));
            }
            ch => escaped.push(ch),
        }
    }

    format!("'{escaped}'")
}

fn serialize_json5_key(key: &str) -> String {
    if is_identifier_key(key) {
        key.to_string()
    } else {
        serialize_json5_string(key)
    }
}

fn serialize_json5_value(value: &Value, indent_level: usize) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => serialize_json5_string(text),
        Value::Array(items) => {
            if items.is_empty() {
                return "[]".to_string();
            }

            let current_indent = "  ".repeat(indent_level);
            let child_indent = "  ".repeat(indent_level + 1);
            let mut output = String::from("[\n");
            for (index, item) in items.iter().enumerate() {
                output.push_str(&child_indent);
                output.push_str(&serialize_json5_value(item, indent_level + 1));
                if index + 1 != items.len() {
                    output.push(',');
                }
                output.push('\n');
            }
            output.push_str(&current_indent);
            output.push(']');
            output
        }
        Value::Object(map) => {
            if map.is_empty() {
                return "{}".to_string();
            }

            let current_indent = "  ".repeat(indent_level);
            let child_indent = "  ".repeat(indent_level + 1);
            let mut output = String::from("{\n");
            for (index, (key, item)) in map.iter().enumerate() {
                output.push_str(&child_indent);
                output.push_str(&serialize_json5_key(key));
                output.push_str(": ");
                output.push_str(&serialize_json5_value(item, indent_level + 1));
                if index + 1 != map.len() {
                    output.push(',');
                }
                output.push('\n');
            }
            output.push_str(&current_indent);
            output.push('}');
            output
        }
    }
}

fn serialize_section_value(section: &str, value: &Value) -> Result<String, AppError> {
    if should_use_precise_empty_object_fallback(section, value) {
        return Ok(serialize_json5_value(value, 0));
    }

    json_five::to_string_formatted(
        value,
        FormatConfiguration::with_indent(2, TrailingComma::NONE),
    )
    .map_err(|e| AppError::Config(format!("Failed to serialize JSON5 section: {e}")))
}

fn value_to_rt_value(
    section: &str,
    value: &Value,
    parent_indent: &str,
) -> Result<RtJSONValue, AppError> {
    let source = serialize_section_value(section, value)?;

    let adjusted = reindent_json5_block(&source, parent_indent);
    let text = rt_from_str(&adjusted).map_err(|e| {
        AppError::Config(format!(
            "Failed to parse generated JSON5 section: {}",
            e.message
        ))
    })?;
    Ok(text.value)
}

fn reindent_json5_block(source: &str, parent_indent: &str) -> String {
    let normalized = normalize_json_five_output(source);
    if parent_indent.is_empty() || !normalized.contains('\n') {
        return normalized;
    }

    let mut lines = normalized.lines();
    let Some(first_line) = lines.next() else {
        return String::new();
    };

    let mut result = String::from(first_line);
    for line in lines {
        result.push('\n');
        result.push_str(parent_indent);
        result.push_str(line);
    }
    result
}

fn normalize_json_five_output(source: &str) -> String {
    source.replace("\\/", "/")
}

fn make_root_pair(key: &str, value: RtJSONValue, closing_ws: String) -> RtJSONKeyValuePair {
    RtJSONKeyValuePair {
        key: make_json5_key(key),
        value,
        context: Some(RtKeyValuePairContext {
            wsc: (String::new(), " ".to_string(), closing_ws, None),
        }),
    }
}

fn make_json5_key(key: &str) -> RtJSONValue {
    if is_identifier_key(key) {
        RtJSONValue::Identifier(key.to_string())
    } else {
        RtJSONValue::DoubleQuotedString(key.to_string())
    }
}

fn is_identifier_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    matches!(first, 'a'..='z' | 'A'..='Z' | '_' | '$')
        && chars.all(|ch| matches!(ch, 'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '$'))
}

fn json5_key_name(key: &RtJSONValue) -> Option<&str> {
    match key {
        RtJSONValue::Identifier(name)
        | RtJSONValue::DoubleQuotedString(name)
        | RtJSONValue::SingleQuotedString(name) => Some(name),
        _ => None,
    }
}

fn warning(code: &str, message: impl Into<String>, path: Option<&str>) -> OpenClawHealthWarning {
    OpenClawHealthWarning {
        code: code.to_string(),
        message: message.into(),
        path: path.map(|value| value.to_string()),
    }
}

fn scan_openclaw_health_from_value(config: &Value) -> Vec<OpenClawHealthWarning> {
    let mut warnings = Vec::new();

    if let Some(profile) = config
        .get("tools")
        .and_then(|tools| tools.get("profile"))
        .and_then(Value::as_str)
    {
        const OPENCLAW_TOOLS_PROFILES: &[&str] = &["minimal", "coding", "messaging", "full"];
        if !OPENCLAW_TOOLS_PROFILES.contains(&profile) {
            warnings.push(warning(
                "invalid_tools_profile",
                format!("tools.profile uses unsupported value '{profile}'."),
                Some("tools.profile"),
            ));
        }
    }

    if config
        .get("agents")
        .and_then(|agents| agents.get("defaults"))
        .and_then(|defaults| defaults.get("timeout"))
        .is_some()
    {
        warnings.push(warning(
            "legacy_agents_timeout",
            "agents.defaults.timeout is deprecated; use agents.defaults.timeoutSeconds.",
            Some("agents.defaults.timeout"),
        ));
    }

    if let Some(value) = config.get("env").and_then(|env| env.get("vars")) {
        if !value.is_object() {
            warnings.push(warning(
                "stringified_env_vars",
                "env.vars should be an object. The current value looks stringified or malformed.",
                Some("env.vars"),
            ));
        }
    }

    if let Some(value) = config.get("env").and_then(|env| env.get("shellEnv")) {
        if !value.is_object() {
            warnings.push(warning(
                "stringified_env_shell_env",
                "env.shellEnv should be an object. The current value looks stringified or malformed.",
                Some("env.shellEnv"),
            ));
        }
    }

    warnings
}

fn remove_legacy_timeout(defaults_value: &mut Value) {
    if let Some(defaults_obj) = defaults_value.as_object_mut() {
        defaults_obj.remove("timeout");
    }
}

fn default_model_from_config(config: &Value) -> Result<Option<OpenClawDefaultModel>, AppError> {
    let Some(model_value) = config
        .get("agents")
        .and_then(|agents| agents.get("defaults"))
        .and_then(|defaults| defaults.get("model"))
    else {
        return Ok(None);
    };

    let model = serde_json::from_value(model_value.clone())
        .map_err(|e| AppError::Config(format!("Failed to parse agents.defaults.model: {e}")))?;
    Ok(Some(model))
}

pub fn get_providers() -> Result<Map<String, Value>, AppError> {
    let config = read_openclaw_config()?;
    Ok(config
        .get("models")
        .and_then(|models| models.get("providers"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default())
}

pub fn get_provider(id: &str) -> Result<Option<Value>, AppError> {
    Ok(get_providers()?.get(id).cloned())
}

pub fn set_provider(id: &str, provider_config: Value) -> Result<OpenClawWriteOutcome, AppError> {
    let mut full_config = read_openclaw_config()?;
    {
        let root = ensure_object(&mut full_config);
        let models = root.entry("models".to_string()).or_insert_with(|| {
            json!({
                "mode": "merge",
                "providers": {}
            })
        });
        let providers = ensure_object(models)
            .entry("providers".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        ensure_object(providers).insert(id.to_string(), provider_config);
    }

    let models_value = full_config.get("models").cloned().unwrap_or_else(|| {
        json!({
            "mode": "merge",
            "providers": {}
        })
    });
    write_root_section("models", &models_value)
}

pub fn remove_provider(id: &str) -> Result<OpenClawWriteOutcome, AppError> {
    let mut config = read_openclaw_config()?;
    let mut removed = false;

    if let Some(providers) = config
        .get_mut("models")
        .and_then(|models| models.get_mut("providers"))
        .and_then(Value::as_object_mut)
    {
        removed = providers.remove(id).is_some();
    }

    if !removed {
        return Ok(OpenClawWriteOutcome::default());
    }

    let models_value = config.get("models").cloned().unwrap_or_else(|| {
        json!({
            "mode": "merge",
            "providers": {}
        })
    });
    write_root_section("models", &models_value)
}

pub fn get_default_model() -> Result<Option<OpenClawDefaultModel>, AppError> {
    let config = read_openclaw_config()?;
    default_model_from_config(&config)
}

pub fn set_default_model(model: &OpenClawDefaultModel) -> Result<OpenClawWriteOutcome, AppError> {
    let mut config = read_openclaw_config()?;
    let model_value =
        serde_json::to_value(model).map_err(|source| AppError::JsonSerialize { source })?;
    {
        let root = ensure_object(&mut config);
        let agents = root
            .entry("agents".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let defaults = ensure_object(agents)
            .entry("defaults".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        ensure_object(defaults).insert("model".to_string(), model_value);
    }

    let agents_value = config
        .get("agents")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    write_root_section("agents", &agents_value)
}

pub fn get_typed_providers() -> Result<IndexMap<String, OpenClawProviderConfig>, AppError> {
    let providers = get_providers()?;
    let mut result = IndexMap::new();

    for (id, value) in providers {
        match serde_json::from_value::<OpenClawProviderConfig>(value.clone()) {
            Ok(config) => {
                result.insert(id, config);
            }
            Err(err) => {
                log::warn!("Failed to parse OpenClaw provider '{id}': {err}");
            }
        }
    }

    Ok(result)
}

pub fn set_typed_provider(
    id: &str,
    config: &OpenClawProviderConfig,
) -> Result<OpenClawWriteOutcome, AppError> {
    let value =
        serde_json::to_value(config).map_err(|source| AppError::JsonSerialize { source })?;
    set_provider(id, value)
}

pub fn get_model_catalog() -> Result<Option<HashMap<String, OpenClawModelCatalogEntry>>, AppError> {
    let config = read_openclaw_config()?;

    let Some(models_value) = config
        .get("agents")
        .and_then(|agents| agents.get("defaults"))
        .and_then(|defaults| defaults.get("models"))
    else {
        return Ok(None);
    };

    let catalog = serde_json::from_value(models_value.clone())
        .map_err(|e| AppError::Config(format!("Failed to parse agents.defaults.models: {e}")))?;
    Ok(Some(catalog))
}

pub fn set_model_catalog(
    catalog: &HashMap<String, OpenClawModelCatalogEntry>,
) -> Result<OpenClawWriteOutcome, AppError> {
    let mut config = read_openclaw_config()?;
    let catalog_value =
        serde_json::to_value(catalog).map_err(|source| AppError::JsonSerialize { source })?;
    {
        let root = ensure_object(&mut config);
        let agents = root
            .entry("agents".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let defaults = ensure_object(agents)
            .entry("defaults".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        ensure_object(defaults).insert("models".to_string(), catalog_value);
    }

    let agents_value = config
        .get("agents")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    write_root_section("agents", &agents_value)
}

pub fn get_agents_defaults() -> Result<Option<OpenClawAgentsDefaults>, AppError> {
    let config = read_openclaw_config()?;

    let Some(defaults_value) = config
        .get("agents")
        .and_then(|agents| agents.get("defaults"))
    else {
        return Ok(None);
    };

    let defaults = serde_json::from_value(defaults_value.clone())
        .map_err(|e| AppError::Config(format!("Failed to parse agents.defaults: {e}")))?;
    Ok(Some(defaults))
}

pub fn set_agents_defaults(
    defaults: &OpenClawAgentsDefaults,
) -> Result<OpenClawWriteOutcome, AppError> {
    let mut config = read_openclaw_config()?;
    let mut defaults_value =
        serde_json::to_value(defaults).map_err(|source| AppError::JsonSerialize { source })?;
    remove_legacy_timeout(&mut defaults_value);
    {
        let root = ensure_object(&mut config);
        let agents = root
            .entry("agents".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        ensure_object(agents).insert("defaults".to_string(), defaults_value);
    }

    let agents_value = config
        .get("agents")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    write_root_section("agents", &agents_value)
}

pub fn get_env_config() -> Result<OpenClawEnvConfig, AppError> {
    let config = read_openclaw_config()?;

    let Some(env_value) = config.get("env") else {
        return Ok(OpenClawEnvConfig {
            vars: HashMap::new(),
        });
    };

    serde_json::from_value(env_value.clone())
        .map_err(|e| AppError::Config(format!("Failed to parse env config: {e}")))
}

pub fn set_env_config(env: &OpenClawEnvConfig) -> Result<OpenClawWriteOutcome, AppError> {
    let value = serde_json::to_value(env).map_err(|source| AppError::JsonSerialize { source })?;
    write_root_section("env", &value)
}

pub fn get_tools_config() -> Result<OpenClawToolsConfig, AppError> {
    let config = read_openclaw_config()?;

    let Some(tools_value) = config.get("tools") else {
        return Ok(OpenClawToolsConfig {
            profile: None,
            allow: Vec::new(),
            deny: Vec::new(),
            extra: HashMap::new(),
        });
    };

    serde_json::from_value(tools_value.clone())
        .map_err(|e| AppError::Config(format!("Failed to parse tools config: {e}")))
}

pub fn set_tools_config(tools: &OpenClawToolsConfig) -> Result<OpenClawWriteOutcome, AppError> {
    let value = serde_json::to_value(tools).map_err(|source| AppError::JsonSerialize { source })?;
    write_root_section("tools", &value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::get_app_config_dir;
    use crate::settings::{get_settings, update_settings, AppSettings};
    use crate::test_support::{lock_test_home_and_settings, set_test_home_override};
    use serde_json::json;
    use serial_test::serial;
    use std::collections::HashMap;
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    struct SettingsGuard {
        previous: AppSettings,
    }

    impl SettingsGuard {
        fn with_openclaw_dir(path: &std::path::Path) -> Self {
            let previous = get_settings();
            let mut settings = AppSettings::default();
            settings.openclaw_config_dir = Some(path.display().to_string());
            update_settings(settings).expect("set openclaw override dir");
            Self { previous }
        }
    }

    impl Drop for SettingsGuard {
        fn drop(&mut self) {
            update_settings(self.previous.clone()).expect("restore previous settings");
        }
    }

    struct HomeGuard {
        old_home: Option<std::ffi::OsString>,
        old_test_home: Option<std::ffi::OsString>,
    }

    impl HomeGuard {
        fn set(home: &Path) -> Self {
            let old_home = std::env::var_os("HOME");
            let old_test_home = std::env::var_os("CC_SWITCH_TEST_HOME");
            std::env::set_var("HOME", home);
            std::env::set_var("CC_SWITCH_TEST_HOME", home);
            set_test_home_override(Some(home));
            crate::settings::reload_test_settings();
            Self {
                old_home,
                old_test_home,
            }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.old_home.take() {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match self.old_test_home.take() {
                Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
                None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
            }
            set_test_home_override(self.old_home.as_deref().map(Path::new));
            crate::settings::reload_test_settings();
        }
    }

    fn with_test_paths(source: &str, test: impl FnOnce(&Path)) {
        let _guard = lock_test_home_and_settings();
        let temp = tempdir().expect("create tempdir");
        let openclaw_dir = temp.path().join(".openclaw");
        fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
        let config_path = openclaw_dir.join("openclaw.json");
        fs::write(&config_path, source).expect("seed openclaw config");
        let _home = HomeGuard::set(temp.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);
        test(&config_path);
    }

    #[test]
    #[serial]
    fn missing_config_returns_default_models_object() {
        let _guard = lock_test_home_and_settings();
        let dir = tempdir().expect("create tempdir");
        let _settings = SettingsGuard::with_openclaw_dir(dir.path());

        let config = read_openclaw_config().expect("read default config");
        assert_eq!(config["models"]["mode"], json!("merge"));
        assert_eq!(config["models"]["providers"], json!({}));
    }

    #[test]
    #[serial]
    fn read_openclaw_config_accepts_json5_syntax() {
        let _guard = lock_test_home_and_settings();
        let dir = tempdir().expect("create tempdir");
        let _settings = SettingsGuard::with_openclaw_dir(dir.path());

        let source = r#"{
  // json5 comments should be accepted
  models: {
    mode: 'merge',
    providers: {
      demo: {
        baseUrl: 'https://example.test/v1',
        apiKey: 'sk-demo',
      },
    },
  },
}
"#;
        fs::write(get_openclaw_config_path(), source).expect("write json5 config");

        let config = read_openclaw_config().expect("parse json5 config");
        assert_eq!(
            config["models"]["providers"]["demo"]["baseUrl"],
            json!("https://example.test/v1")
        );
        assert_eq!(
            config["models"]["providers"]["demo"]["apiKey"],
            json!("sk-demo")
        );
    }

    #[test]
    #[serial]
    fn read_openclaw_config_does_not_rewrite_string_contents() {
        let _guard = lock_test_home_and_settings();
        let dir = tempdir().expect("create tempdir");
        let _settings = SettingsGuard::with_openclaw_dir(dir.path());

        let source = r#"{
  models: {
    mode: 'merge',
    providers: {
      demo: {
        note: '{foo:1},}',
        prompt: 'keep ,} and {bar:2} literally',
      },
    },
  },
}
"#;
        fs::write(get_openclaw_config_path(), source).expect("write json5 config");

        let config = read_openclaw_config().expect("parse json5 config without rewriting strings");
        assert_eq!(
            config["models"]["providers"]["demo"]["note"],
            json!("{foo:1},}")
        );
        assert_eq!(
            config["models"]["providers"]["demo"]["prompt"],
            json!("keep ,} and {bar:2} literally")
        );
    }

    #[test]
    #[serial]
    fn set_and_remove_provider_only_touch_target_entry() {
        let _guard = lock_test_home_and_settings();
        let dir = tempdir().expect("create tempdir");
        let _settings = SettingsGuard::with_openclaw_dir(dir.path());

        fs::write(
            get_openclaw_config_path(),
            r#"{
  models: {
    mode: 'merge',
    providers: {
      keep: { baseUrl: 'https://keep.test' },
      remove: { baseUrl: 'https://remove.test' },
    },
  },
}
"#,
        )
        .expect("seed json5 config");

        set_provider("added", json!({ "baseUrl": "https://added.test" })).expect("set provider");
        remove_provider("remove").expect("remove provider");

        let providers = get_providers().expect("read providers");
        assert!(providers.contains_key("keep"));
        assert!(providers.contains_key("added"));
        assert!(!providers.contains_key("remove"));
    }

    #[test]
    #[serial]
    fn remove_missing_provider_is_noop_and_does_not_create_file() {
        let _guard = lock_test_home_and_settings();
        let dir = tempdir().expect("create tempdir");
        let _settings = SettingsGuard::with_openclaw_dir(dir.path());

        let path = get_openclaw_config_path();
        assert!(!path.exists(), "precondition: config file should be absent");

        remove_provider("missing").expect("removing a missing provider should be a no-op");

        assert!(
            !path.exists(),
            "no-op remove should not create a new openclaw.json"
        );
    }

    #[test]
    #[serial]
    fn remove_last_provider_keeps_empty_providers_map() {
        let _guard = lock_test_home_and_settings();
        let dir = tempdir().expect("create tempdir");
        let _settings = SettingsGuard::with_openclaw_dir(dir.path());

        fs::write(
            get_openclaw_config_path(),
            r#"{
  models: {
    mode: 'merge',
    providers: {
      only: { baseUrl: 'https://only.test' },
    },
  },
}
"#,
        )
        .expect("seed json5 config with single provider");

        remove_provider("only").expect("remove last provider should succeed");

        let config = read_openclaw_config().expect("read config after removing last provider");
        assert_eq!(config["models"]["mode"], json!("merge"));
        assert_eq!(config["models"]["providers"], json!({}));
    }

    #[test]
    #[serial]
    fn remove_last_provider_rewrites_models_section_like_upstream() {
        let _guard = lock_test_home_and_settings();
        let dir = tempdir().expect("create tempdir");
        let _settings = SettingsGuard::with_openclaw_dir(dir.path());

        fs::write(
            get_openclaw_config_path(),
            r#"{
  // preserve top-level comment
  models: {
    mode: 'merge',
    // preserve providers comment
    providers: {
      only: { baseUrl: 'https://only.test' },
    },
  },
  tools: {
    profile: 'coding',
  },
}
"#,
        )
        .expect("seed json5 config with comments and single quotes");

        remove_provider("only").expect("remove last provider should succeed");

        let written = fs::read_to_string(get_openclaw_config_path())
            .expect("read config after removing last provider");

        assert!(
            written.contains("// preserve top-level comment"),
            "top-level comment should survive targeted remove: {written}"
        );
        assert!(
            written.contains("mode: 'merge'"),
            "models.mode formatting should stay JSON5-style: {written}"
        );
        assert!(
            !written.contains("// preserve providers comment"),
            "rewriting the models subtree should drop providers-level comments like upstream: {written}"
        );
        assert!(
            written.contains("providers: {}"),
            "providers map should become an empty object after rewrite: {written}"
        );
        assert!(
            written.contains("profile: 'coding'"),
            "unrelated sections should preserve existing source text: {written}"
        );
    }

    #[test]
    fn empty_object_fallback_targets_models_with_empty_providers_and_empty_tools() {
        let models_value = json!({
            "mode": "merge",
            "providers": {}
        });
        let empty_tools_value = json!({});
        let env_value = json!({
            "vars": {}
        });
        let models_with_other_empty_object = json!({
            "mode": "merge",
            "providers": {
                "demo": {
                    "headers": {}
                }
            }
        });

        assert!(should_use_precise_empty_object_fallback(
            "models",
            &models_value
        ));
        assert!(should_use_precise_empty_object_fallback(
            "tools",
            &empty_tools_value
        ));
        assert!(!should_use_precise_empty_object_fallback("env", &env_value));
        assert!(!should_use_precise_empty_object_fallback(
            "models",
            &models_with_other_empty_object
        ));
    }

    #[test]
    fn serialize_section_value_uses_standard_formatter_outside_precise_fallback_shape() {
        let env_value = json!({
            "vars": {
                "TOKEN": "value"
            }
        });
        let tools_value = json!({
            "profile": "coding",
            "allow": ["Read"]
        });

        let expected_env = json_five::to_string_formatted(
            &env_value,
            FormatConfiguration::with_indent(2, TrailingComma::NONE),
        )
        .expect("standard formatter should handle non-fallback shape");
        let expected_tools = json_five::to_string_formatted(
            &tools_value,
            FormatConfiguration::with_indent(2, TrailingComma::NONE),
        )
        .expect("standard formatter should handle non-empty tools shape");

        let actual_env = serialize_section_value("env", &env_value)
            .expect("serialize non-fallback shape should succeed");
        let actual_tools = serialize_section_value("tools", &tools_value)
            .expect("serialize non-empty tools shape should succeed");

        assert_eq!(actual_env, expected_env);
        assert_eq!(actual_tools, expected_tools);
    }

    #[test]
    #[serial]
    fn default_model_round_trip_preserves_existing_providers() {
        let _guard = lock_test_home_and_settings();
        let dir = tempdir().expect("create tempdir");
        let _settings = SettingsGuard::with_openclaw_dir(dir.path());

        fs::write(
            get_openclaw_config_path(),
            r#"{
  models: {
    mode: 'merge',
    providers: {
      demo: {
        baseUrl: 'https://demo.test/v1',
        apiKey: 'sk-demo',
        models: [
          { id: 'gpt-4.1' },
          { id: 'gpt-4.1-mini' },
        ],
      },
    },
  },
}
"#,
        )
        .expect("seed openclaw config");

        let model = OpenClawDefaultModel {
            primary: "demo/gpt-4.1".to_string(),
            fallbacks: vec!["demo/gpt-4.1-mini".to_string()],
            extra: std::collections::HashMap::from([(
                "reasoningEffort".to_string(),
                json!("high"),
            )]),
        };

        set_default_model(&model).expect("write default model");

        assert_eq!(
            get_default_model().expect("read default model"),
            Some(model.clone())
        );

        let providers = get_providers().expect("read providers after default-model write");
        assert!(
            providers.contains_key("demo"),
            "writing default model should not drop provider entries"
        );
    }

    #[test]
    #[serial]
    fn typed_provider_round_trip_preserves_known_and_unknown_fields() {
        let _guard = lock_test_home_and_settings();
        let dir = tempdir().expect("create tempdir");
        let _settings = SettingsGuard::with_openclaw_dir(dir.path());

        let mut model_extra = std::collections::HashMap::new();
        model_extra.insert("providerHint".to_string(), json!("reasoning"));

        let mut provider_extra = std::collections::HashMap::new();
        provider_extra.insert("region".to_string(), json!("us-east-1"));

        let config = OpenClawProviderConfig {
            base_url: Some("https://example.test/v1".to_string()),
            api_key: Some("sk-test".to_string()),
            api: Some("openai-responses".to_string()),
            models: vec![crate::provider::OpenClawModelEntry {
                id: "gpt-4.1".to_string(),
                name: Some("GPT-4.1".to_string()),
                alias: Some("fast".to_string()),
                cost: Some(crate::provider::OpenClawModelCost {
                    input: 0.1,
                    output: 0.2,
                    extra: std::collections::HashMap::new(),
                }),
                context_window: Some(200000),
                extra: model_extra,
            }],
            headers: std::collections::HashMap::from([("x-org".to_string(), "demo".to_string())]),
            extra: provider_extra,
        };

        set_typed_provider("demo", &config).expect("write typed provider");

        let providers = get_typed_providers().expect("read typed providers");
        let provider = providers.get("demo").expect("typed provider should exist");
        assert_eq!(
            provider.base_url.as_deref(),
            Some("https://example.test/v1")
        );
        assert_eq!(provider.api_key.as_deref(), Some("sk-test"));
        assert_eq!(provider.api.as_deref(), Some("openai-responses"));
        assert_eq!(provider.models.len(), 1);
        assert_eq!(provider.models[0].id, "gpt-4.1");
        assert_eq!(provider.models[0].context_window, Some(200000));
        assert_eq!(
            provider.models[0].extra.get("providerHint"),
            Some(&json!("reasoning"))
        );
        assert_eq!(provider.extra.get("region"), Some(&json!("us-east-1")));
    }

    #[test]
    #[serial]
    fn get_providers_reads_multiple_json5_entries() {
        let _guard = lock_test_home_and_settings();
        let dir = tempdir().expect("create tempdir");
        let _settings = SettingsGuard::with_openclaw_dir(dir.path());

        let source = r#"{
  models: {
    mode: 'merge',
    providers: {
      openai: {
        name: 'OpenAI Compatible',
        apiKey: 'sk-openai',
      },
      anthropic: {
        name: 'Anthropic',
        api_key: 'sk-anthropic',
      },
    },
  },
}
"#;
        fs::write(get_openclaw_config_path(), source).expect("write json5 config");

        let providers = get_providers().expect("read providers from json5 config");
        assert_eq!(providers.len(), 2);
        assert_eq!(providers["openai"]["apiKey"], json!("sk-openai"));
        assert_eq!(providers["anthropic"]["api_key"], json!("sk-anthropic"));
    }

    #[test]
    #[serial]
    fn scan_openclaw_config_health_returns_parse_warning_for_invalid_json5() {
        let _guard = lock_test_home_and_settings();
        let dir = tempdir().expect("create tempdir");
        let _home = HomeGuard::set(dir.path());
        let _settings = SettingsGuard::with_openclaw_dir(&dir.path().join(".openclaw"));
        fs::create_dir_all(get_openclaw_dir()).expect("create openclaw dir");
        fs::write(get_openclaw_config_path(), "{ broken: [ }").expect("write invalid json5");

        let warnings = scan_openclaw_config_health().expect("scan health should not fail");
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "config_parse_failed");
        assert!(warnings[0].path.is_some());
    }

    #[test]
    #[serial]
    fn default_model_write_preserves_top_level_comments() {
        let source = r#"{
  // top-level comment
  models: {
    mode: 'merge',
    providers: {
      provider: {
        models: [{ id: 'model' }],
      },
    },
  },
}
"#;

        with_test_paths(source, |_| {
            let outcome = set_default_model(&OpenClawDefaultModel {
                primary: "provider/model".to_string(),
                fallbacks: Vec::new(),
                extra: HashMap::new(),
            })
            .expect("write default model");

            assert!(outcome.backup_path.is_some());

            let written =
                fs::read_to_string(get_openclaw_config_path()).expect("read written config");
            assert!(written.contains("// top-level comment"));
            assert!(written.contains("agents: {"));
            assert!(written.contains("provider/model"));
        });
    }

    #[test]
    #[serial]
    fn default_model_noop_write_skips_backup() {
        let source = r#"{
  models: {
    mode: 'merge',
    providers: {
      provider: {
        models: [
          { id: 'model' },
          { id: 'fallback' },
        ],
      },
    },
  },
}
"#;

        with_test_paths(source, |_| {
            let model = OpenClawDefaultModel {
                primary: "provider/model".to_string(),
                fallbacks: vec!["provider/fallback".to_string()],
                extra: HashMap::new(),
            };

            let first_outcome = set_default_model(&model).expect("first default-model write");
            assert!(first_outcome.backup_path.is_some());

            let first_written =
                fs::read_to_string(get_openclaw_config_path()).expect("read first written config");
            let backup_dir = get_app_config_dir().join("backups").join("openclaw");
            let backup_count = fs::read_dir(&backup_dir).expect("read backup dir").count();
            assert_eq!(backup_count, 1);

            let second_outcome = set_default_model(&model).expect("second default-model write");
            assert!(second_outcome.backup_path.is_none());

            let second_written =
                fs::read_to_string(get_openclaw_config_path()).expect("read second written config");
            assert_eq!(second_written, first_written);
            assert_eq!(
                fs::read_dir(&backup_dir)
                    .expect("re-read backup dir")
                    .count(),
                backup_count
            );
        });
    }

    #[test]
    #[serial]
    fn backup_cleanup_uses_settings_retain_count() {
        let _guard = lock_test_home_and_settings();
        let dir = tempdir().expect("create tempdir");
        let openclaw_dir = dir.path().join(".openclaw");
        fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
        let _home = HomeGuard::set(dir.path());

        let previous = get_settings();
        let mut settings = AppSettings::default();
        settings.openclaw_config_dir = Some(openclaw_dir.display().to_string());
        settings.backup_retain_count = Some(2);
        update_settings(settings).expect("set settings with backup retain count");

        fs::write(
            get_openclaw_config_path(),
            r#"{
  models: {
    mode: 'merge',
    providers: {
      provider: {
        models: [
          { id: 'model-0' },
          { id: 'model-1' },
          { id: 'model-2' },
          { id: 'model-3' },
        ],
      },
    },
  },
}
"#,
        )
        .expect("seed openclaw config");

        for i in 0..4 {
            set_default_model(&OpenClawDefaultModel {
                primary: format!("provider/model-{i}"),
                fallbacks: Vec::new(),
                extra: HashMap::new(),
            })
            .expect("write default model for backup pruning");
        }

        let backup_dir = get_app_config_dir().join("backups").join("openclaw");
        let backup_count = fs::read_dir(&backup_dir).expect("read backup dir").count();
        assert_eq!(backup_count, 2, "backup pruning should follow settings");

        update_settings(previous).expect("restore previous settings");
    }

    #[test]
    #[serial]
    fn save_detects_external_conflict() {
        let source = r#"{
  models: {
    mode: 'merge',
    providers: {},
  },
}
"#;

        with_test_paths(source, |config_path| {
            let mut document = OpenClawConfigDocument::load().expect("load document");
            document
                .set_root_section("env", &json!({ "TOKEN": "value" }))
                .expect("set env root section");

            fs::write(config_path, "{ changedExternally: true }\n")
                .expect("overwrite config externally");
            let err = document
                .save()
                .expect_err("save should detect external change");
            assert!(err.to_string().contains("OpenClaw config changed on disk"));
        });
    }

    #[test]
    #[serial]
    fn model_catalog_round_trip_preserves_existing_default_model() {
        let source = r#"{
  models: {
    mode: 'merge',
    providers: {
      demo: {
        models: [
          { id: 'model-primary' },
          { id: 'model-fallback' },
        ],
      },
    },
  },
  agents: {
    defaults: {
      model: {
        primary: 'demo/model-primary',
      },
    },
  },
}
"#;

        with_test_paths(source, |_| {
            let catalog = HashMap::from([
                (
                    "demo/model-primary".to_string(),
                    OpenClawModelCatalogEntry {
                        alias: Some("Primary".to_string()),
                        extra: HashMap::from([("tier".to_string(), json!("gold"))]),
                    },
                ),
                (
                    "demo/model-fallback".to_string(),
                    OpenClawModelCatalogEntry {
                        alias: Some("Fallback".to_string()),
                        extra: HashMap::new(),
                    },
                ),
            ]);

            set_model_catalog(&catalog).expect("write model catalog");

            assert_eq!(
                get_model_catalog().expect("read model catalog"),
                Some(catalog)
            );
            assert_eq!(
                get_default_model().expect("read preserved default model"),
                Some(OpenClawDefaultModel {
                    primary: "demo/model-primary".to_string(),
                    fallbacks: Vec::new(),
                    extra: HashMap::new(),
                })
            );
        });
    }

    #[test]
    #[serial]
    fn agents_defaults_round_trip_strips_legacy_timeout_and_preserves_extra_fields() {
        let source = r#"{
  models: {
    mode: 'merge',
    providers: {
      demo: {
        models: [
          { id: 'model-primary' },
          { id: 'model-fallback' },
        ],
      },
    },
  },
}
"#;

        with_test_paths(source, |_| {
            let defaults = OpenClawAgentsDefaults {
                model: Some(OpenClawDefaultModel {
                    primary: "demo/model-primary".to_string(),
                    fallbacks: vec!["demo/model-fallback".to_string()],
                    extra: HashMap::new(),
                }),
                models: Some(HashMap::from([(
                    "demo/model-primary".to_string(),
                    OpenClawModelCatalogEntry {
                        alias: Some("Primary".to_string()),
                        extra: HashMap::new(),
                    },
                )])),
                extra: HashMap::from([
                    ("timeout".to_string(), json!(30)),
                    ("timeoutSeconds".to_string(), json!(45)),
                    ("reasoningEffort".to_string(), json!("high")),
                ]),
            };

            set_agents_defaults(&defaults).expect("write full agents.defaults");

            let round_trip = get_agents_defaults()
                .expect("read full agents.defaults")
                .expect("agents.defaults should exist");
            assert_eq!(round_trip.model, defaults.model);
            assert_eq!(round_trip.models, defaults.models);
            assert_eq!(round_trip.extra.get("timeoutSeconds"), Some(&json!(45)));
            assert_eq!(
                round_trip.extra.get("reasoningEffort"),
                Some(&json!("high"))
            );
            assert!(
                !round_trip.extra.contains_key("timeout"),
                "legacy timeout should be stripped on write"
            );
        });
    }

    #[test]
    #[serial]
    fn env_and_tools_section_helpers_round_trip() {
        let source = r#"{
  // top-level comment
  models: {
    mode: 'merge',
    providers: {},
  },
}
"#;

        with_test_paths(source, |_| {
            let env = OpenClawEnvConfig {
                vars: HashMap::from([
                    ("vars".to_string(), json!({ "TOKEN": "value" })),
                    ("shellEnv".to_string(), json!({ "PATH": "/usr/bin" })),
                ]),
            };
            let tools = OpenClawToolsConfig {
                profile: Some("coding".to_string()),
                allow: vec!["Read".to_string()],
                deny: vec!["Bash(rm:*)".to_string()],
                extra: HashMap::from([("telemetry".to_string(), json!(true))]),
            };

            set_env_config(&env).expect("write env section");
            set_tools_config(&tools).expect("write tools section");

            assert_eq!(get_env_config().expect("read env section"), env);
            assert_eq!(get_tools_config().expect("read tools section"), tools);
        });
    }

    #[test]
    #[serial]
    fn scan_openclaw_health_detects_invalid_tools_and_env_values() {
        let source = r#"{
  models: {
    mode: 'merge',
    providers: {},
  },
  tools: {
    profile: 'default',
  },
  agents: {
    defaults: {
      timeout: 30,
    },
  },
  env: {
    vars: '[object Object]',
    shellEnv: 'oops',
  },
}
"#;

        with_test_paths(source, |_| {
            let warnings = scan_openclaw_config_health().expect("scan config health");
            let codes = warnings
                .into_iter()
                .map(|warning| warning.code)
                .collect::<Vec<_>>();

            assert!(codes.contains(&"invalid_tools_profile".to_string()));
            assert!(codes.contains(&"legacy_agents_timeout".to_string()));
            assert!(codes.contains(&"stringified_env_vars".to_string()));
            assert!(codes.contains(&"stringified_env_shell_env".to_string()));
        });
    }
}
