use cc_switch_core::{McpConfigTarget, McpEntryDecodePolicy};
use serde_json::{json, Map, Value};

use crate::error::AppError;

#[cfg(test)]
mod tests;

pub(super) fn server_spec(entry: &toml::value::Table) -> Result<Value, AppError> {
    // Retain TOML shape even when the host's shallow conversion drops every
    // member. An empty native header table still takes precedence over legacy
    // headers, and an explicit invalid type must not trigger inference.
    let native: Map<_, _> = [
        "type",
        "command",
        "args",
        "env",
        "cwd",
        "url",
        "headers",
        "http_headers",
    ]
    .into_iter()
    .filter_map(|key| {
        entry.get(key).map(|value| {
            let converted = shallow_value(value).unwrap_or_else(|| match value {
                toml::Value::Array(_) => json!([]),
                toml::Value::Table(_) => json!({}),
                _ => Value::Null,
            });
            (key.to_owned(), converted)
        })
    })
    .collect();
    let decoded = McpConfigTarget::Codex
        .decode_server_with_policy(
            &Value::Object(native),
            McpEntryDecodePolicy::TransportFields,
        )
        .map_err(|error| AppError::McpValidation(error.to_string()))?;
    let mut spec = decoded.as_object().cloned().ok_or_else(|| {
        AppError::McpValidation("Codex MCP entry codec did not return an object".into())
    })?;

    // The CLI historically applies the same shallow conversion to an explicit
    // non-string type as to extensions (including omission of TOML datetimes).
    if let Some(value) = entry.get("type").filter(|value| !value.is_str()) {
        match shallow_value(value) {
            Some(value) => {
                spec.insert("type".into(), value);
            }
            None => {
                spec.remove("type");
            }
        }
    }
    let typ = if entry.contains_key("type") {
        entry.get("type").and_then(toml::Value::as_str)
    } else {
        spec.get("type").and_then(Value::as_str)
    };
    // Cross-transport fields remain extensions under the CLI's catalog policy.
    let core_fields: &[&str] = match typ {
        Some("stdio") => &["type", "command", "args", "env", "cwd"],
        Some("http" | "sse") => &["type", "url", "headers", "http_headers"],
        _ => &["type"],
    };
    for (key, value) in entry {
        if core_fields.contains(&key.as_str()) {
            continue;
        }
        if let Some(value) = shallow_value(value) {
            spec.insert(key.clone(), value);
            log::debug!("导入扩展字段 '{key}'（值已省略）");
        } else {
            log::debug!("跳过复杂字段 '{key}' (TOML → JSON)");
        }
    }
    Ok(Value::Object(spec))
}

fn shallow_value(value: &toml::Value) -> Option<Value> {
    match value {
        toml::Value::String(value) => Some(json!(value)),
        toml::Value::Integer(value) => Some(json!(value)),
        toml::Value::Float(value) => Some(json!(value)),
        toml::Value::Boolean(value) => Some(json!(value)),
        toml::Value::Array(values) => {
            let values: Vec<_> = values
                .iter()
                .filter_map(|value| match value {
                    toml::Value::String(_)
                    | toml::Value::Integer(_)
                    | toml::Value::Float(_)
                    | toml::Value::Boolean(_) => shallow_value(value),
                    _ => None,
                })
                .collect();
            (!values.is_empty()).then_some(Value::Array(values))
        }
        toml::Value::Table(values) => {
            let values: Map<_, _> = values
                .iter()
                .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), json!(value))))
                .collect();
            (!values.is_empty()).then_some(Value::Object(values))
        }
        toml::Value::Datetime(_) => None,
    }
}
