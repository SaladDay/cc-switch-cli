use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use toml_edit::{DocumentMut, Item, TableLike};

use crate::app_config::AppType;
use crate::error::AppError;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigConflict {
    pub app_type: AppType,
    pub target: String,
    pub path: String,
    pub local: String,
    pub incoming: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictChoice {
    KeepLocal,
    UseIncoming,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictPolicy {
    Fail,
    PreferLocal,
    PreferIncoming,
}

impl ConflictPolicy {
    fn choice_for(self, conflicts: &[ConfigConflict]) -> Result<Option<ConflictChoice>, AppError> {
        match self {
            ConflictPolicy::Fail => {
                if conflicts.is_empty() {
                    Ok(None)
                } else {
                    Err(conflict_error(conflicts))
                }
            }
            ConflictPolicy::PreferLocal => Ok(Some(ConflictChoice::KeepLocal)),
            ConflictPolicy::PreferIncoming => Ok(Some(ConflictChoice::UseIncoming)),
        }
    }
}

pub trait ConflictResolver {
    fn resolve_conflict(&mut self, conflict: &ConfigConflict) -> Result<ConflictChoice, AppError>;
}

#[derive(Default)]
pub struct ConflictCollector {
    conflicts: Vec<ConfigConflict>,
}

impl ConflictCollector {
    pub fn into_conflicts(self) -> Vec<ConfigConflict> {
        self.conflicts
    }
}

impl ConflictResolver for ConflictCollector {
    fn resolve_conflict(&mut self, conflict: &ConfigConflict) -> Result<ConflictChoice, AppError> {
        self.conflicts.push(conflict.clone());
        Ok(ConflictChoice::KeepLocal)
    }
}

impl<F> ConflictResolver for F
where
    F: FnMut(&ConfigConflict) -> Result<ConflictChoice, AppError>,
{
    fn resolve_conflict(&mut self, conflict: &ConfigConflict) -> Result<ConflictChoice, AppError> {
        self(conflict)
    }
}

#[derive(Clone, Copy)]
pub enum ConflictResolution<'a> {
    Policy(ConflictPolicy),
    Resolver(&'a std::cell::RefCell<&'a mut dyn ConflictResolver>),
}

impl fmt::Debug for ConflictResolution<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Policy(policy) => f.debug_tuple("Policy").field(policy).finish(),
            Self::Resolver(_) => f.write_str("Resolver(<callback>)"),
        }
    }
}

impl ConflictResolution<'_> {
    fn resolve(self, conflicts: &[ConfigConflict]) -> Result<Option<ConflictChoice>, AppError> {
        match self {
            Self::Policy(policy) => policy.choice_for(conflicts),
            Self::Resolver(resolver) => {
                if conflicts.is_empty() {
                    return Ok(None);
                }

                let mut resolver = resolver.borrow_mut();
                let mut choice = None;
                for conflict in conflicts {
                    choice = Some(resolver.resolve_conflict(conflict)?);
                }
                Ok(choice)
            }
        }
    }

    fn collects_failures(self) -> bool {
        matches!(self, Self::Policy(ConflictPolicy::Fail))
    }
}

impl From<ConflictPolicy> for ConflictResolution<'_> {
    fn from(policy: ConflictPolicy) -> Self {
        Self::Policy(policy)
    }
}

pub fn conflict_error(conflicts: &[ConfigConflict]) -> AppError {
    let mut message = String::from("Live configuration has conflicting local changes:");
    for conflict in conflicts {
        message.push_str("\n- ");
        message.push_str(conflict.app_type.as_str());
        message.push(' ');
        message.push_str(&conflict.target);
        message.push(' ');
        message.push_str(&conflict.path);
        message.push_str(" (local: ");
        message.push_str(&conflict.local);
        message.push_str(", cc-switch: ");
        message.push_str(&conflict.incoming);
        message.push(')');
    }
    AppError::Message(message)
}

pub fn resolve_conflict_choice(
    conflict: ConfigConflict,
    resolution: ConflictResolution<'_>,
) -> Result<ConflictChoice, AppError> {
    Ok(resolution
        .resolve(&[conflict])?
        .unwrap_or(ConflictChoice::KeepLocal))
}

pub fn merge_json_live(
    app_type: &AppType,
    target: impl Into<String>,
    local: Value,
    incoming: &Value,
    resolution: ConflictResolution<'_>,
) -> Result<Value, AppError> {
    let target = target.into();
    let mut merged = local;
    let mut conflicts = Vec::new();
    merge_json_value(
        app_type,
        &target,
        String::new(),
        &mut merged,
        incoming,
        resolution,
        &mut conflicts,
    )?;
    if resolution.collects_failures() && !conflicts.is_empty() {
        return Err(conflict_error(&conflicts));
    }
    Ok(merged)
}

pub fn merge_json_with_base_live(
    app_type: &AppType,
    target: impl Into<String>,
    local: Value,
    base: &Value,
    incoming: &Value,
    resolution: ConflictResolution<'_>,
) -> Result<Value, AppError> {
    let target = target.into();
    let mut merged = local;
    let mut conflicts = Vec::new();
    merge_json_value_with_base(
        app_type,
        &target,
        String::new(),
        &mut merged,
        Some(base),
        incoming,
        resolution,
        &mut conflicts,
    )?;
    if resolution.collects_failures() && !conflicts.is_empty() {
        return Err(conflict_error(&conflicts));
    }
    Ok(merged)
}

fn merge_json_value(
    app_type: &AppType,
    target: &str,
    path: String,
    local: &mut Value,
    incoming: &Value,
    resolution: ConflictResolution<'_>,
    conflicts: &mut Vec<ConfigConflict>,
) -> Result<(), AppError> {
    match (local, incoming) {
        (Value::Object(local_map), Value::Object(incoming_map)) => {
            for (key, incoming_value) in incoming_map {
                let next_path = json_child_path(&path, key);
                match local_map.get_mut(key) {
                    Some(local_value) => merge_json_value(
                        app_type,
                        target,
                        next_path,
                        local_value,
                        incoming_value,
                        resolution,
                        conflicts,
                    )?,
                    None => {
                        local_map.insert(key.clone(), incoming_value.clone());
                    }
                }
            }
            Ok(())
        }
        (local_value, incoming_value) => {
            // Prefer-incoming: cc-switch's value always wins on a scalar mismatch.
            if local_value != incoming_value {
                *local_value = incoming_value.clone();
            }
            Ok(())
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "recursive merge carries immutable context plus per-node state"
)]
fn merge_json_value_with_base(
    app_type: &AppType,
    target: &str,
    path: String,
    local: &mut Value,
    base: Option<&Value>,
    incoming: &Value,
    resolution: ConflictResolution<'_>,
    conflicts: &mut Vec<ConfigConflict>,
) -> Result<(), AppError> {
    if base.is_some_and(|base| base == incoming) {
        return Ok(());
    }

    match (local, base, incoming) {
        (Value::Object(local_map), base, Value::Object(incoming_map)) => {
            let base_map = base.and_then(Value::as_object);
            for (key, incoming_value) in incoming_map {
                let next_path = json_child_path(&path, key);
                match local_map.get_mut(key) {
                    Some(local_value) => merge_json_value_with_base(
                        app_type,
                        target,
                        next_path,
                        local_value,
                        base_map.and_then(|map| map.get(key)),
                        incoming_value,
                        resolution,
                        conflicts,
                    )?,
                    None => {
                        let base_value = base_map.and_then(|map| map.get(key));
                        if let Some(base_value) = base_value {
                            if incoming_value == base_value {
                                // The user removed a key cc-switch did not change;
                                // keep the user's removal.
                                continue;
                            }
                            // cc-switch changed this key (prefer-incoming wins),
                            // so re-introduce it.
                            local_map.insert(key.clone(), incoming_value.clone());
                        } else {
                            local_map.insert(key.clone(), incoming_value.clone());
                        }
                    }
                }
            }
            if let Some(base_map) = base_map {
                for (key, base_value) in base_map {
                    if incoming_map.contains_key(key) {
                        continue;
                    }
                    let Some(local_value) = local_map.get(key) else {
                        continue;
                    };
                    let _ = local_value;
                    // cc-switch removed this key (it existed in base but not in
                    // incoming); prefer-incoming applies the removal.
                    local_map.remove(key);
                }
            }
            Ok(())
        }
        (local_value, base, incoming_value) => {
            // Prefer-incoming: cc-switch's value always wins on a scalar mismatch,
            // regardless of whether the user diverged from the base.
            let _ = base;
            if local_value != incoming_value {
                *local_value = incoming_value.clone();
            }
            Ok(())
        }
    }
}

pub fn merge_env_live(
    app_type: &AppType,
    target: impl Into<String>,
    mut local: HashMap<String, String>,
    incoming: &HashMap<String, String>,
    resolution: ConflictResolution<'_>,
) -> Result<HashMap<String, String>, AppError> {
    let target = target.into();
    let mut conflicts = Vec::new();
    for (key, incoming_value) in incoming {
        match local.get_mut(key) {
            Some(local_value) if local_value != incoming_value => {
                // Prefer-incoming: cc-switch's value wins on a key mismatch.
                *local_value = incoming_value.clone();
            }
            Some(_) => {}
            None => {
                local.insert(key.clone(), incoming_value.clone());
            }
        }
    }
    if resolution.collects_failures() && !conflicts.is_empty() {
        return Err(conflict_error(&conflicts));
    }
    Ok(local)
}

pub fn merge_toml_live(
    app_type: &AppType,
    target: impl Into<String>,
    local_text: &str,
    incoming_text: &str,
    resolution: ConflictResolution<'_>,
) -> Result<String, AppError> {
    let target = target.into();
    let mut local_doc = parse_toml_live(local_text, &target)?;
    let incoming_doc = parse_toml_live(incoming_text, &target)?;
    let mut conflicts = Vec::new();
    merge_toml_table_like(
        app_type,
        &target,
        String::new(),
        local_doc.as_table_mut(),
        incoming_doc.as_table(),
        resolution,
        &mut conflicts,
    )?;
    if resolution.collects_failures() && !conflicts.is_empty() {
        return Err(conflict_error(&conflicts));
    }
    Ok(local_doc.to_string())
}

pub fn merge_toml_with_base_live(
    app_type: &AppType,
    target: impl Into<String>,
    local_text: &str,
    base_text: &str,
    incoming_text: &str,
    resolution: ConflictResolution<'_>,
) -> Result<String, AppError> {
    let target = target.into();
    let mut local_doc = parse_toml_live(local_text, &target)?;
    let base_doc = parse_toml_live(base_text, &target)?;
    let incoming_doc = parse_toml_live(incoming_text, &target)?;
    let mut conflicts = Vec::new();
    merge_toml_table_like_with_base(
        app_type,
        &target,
        String::new(),
        local_doc.as_table_mut(),
        Some(base_doc.as_table()),
        incoming_doc.as_table(),
        resolution,
        &mut conflicts,
    )?;
    if resolution.collects_failures() && !conflicts.is_empty() {
        return Err(conflict_error(&conflicts));
    }
    Ok(local_doc.to_string())
}

fn parse_toml_live(text: &str, target: &str) -> Result<DocumentMut, AppError> {
    text.trim()
        .parse::<DocumentMut>()
        .map_err(|e| AppError::Config(format!("TOML parse error in {target}: {e}")))
}

fn merge_toml_item(
    app_type: &AppType,
    target: &str,
    path: String,
    local: &mut Item,
    incoming: &Item,
    resolution: ConflictResolution<'_>,
    conflicts: &mut Vec<ConfigConflict>,
) -> Result<(), AppError> {
    if let Some(incoming_table) = incoming.as_table_like() {
        if let Some(local_table) = local.as_table_like_mut() {
            return merge_toml_table_like(
                app_type,
                target,
                path,
                local_table,
                incoming_table,
                resolution,
                conflicts,
            );
        }
    }

    // Prefer-incoming: cc-switch's value wins on a scalar mismatch.
    if !toml_items_equal(local, incoming) {
        *local = incoming.clone();
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "recursive TOML item merge carries immutable context plus per-node state"
)]
fn merge_toml_item_with_base(
    app_type: &AppType,
    target: &str,
    path: String,
    local: &mut Item,
    base: Option<&Item>,
    incoming: &Item,
    resolution: ConflictResolution<'_>,
    conflicts: &mut Vec<ConfigConflict>,
) -> Result<(), AppError> {
    if base.is_some_and(|base| toml_items_equal(base, incoming)) {
        return Ok(());
    }

    if let Some(incoming_table) = incoming.as_table_like() {
        if let Some(local_table) = local.as_table_like_mut() {
            let base_table = base.and_then(Item::as_table_like);
            return merge_toml_table_like_with_base(
                app_type,
                target,
                path,
                local_table,
                base_table,
                incoming_table,
                resolution,
                conflicts,
            );
        }
    }

    // Prefer-incoming: cc-switch's value wins on a scalar mismatch, regardless
    // of whether the user diverged from the base.
    let _ = base;
    if !toml_items_equal(local, incoming) {
        *local = incoming.clone();
    }
    Ok(())
}

fn merge_toml_table_like(
    app_type: &AppType,
    target: &str,
    path: String,
    local: &mut dyn TableLike,
    incoming: &dyn TableLike,
    resolution: ConflictResolution<'_>,
    conflicts: &mut Vec<ConfigConflict>,
) -> Result<(), AppError> {
    for (key, incoming_item) in incoming.iter() {
        let next_path = toml_child_path(&path, key);
        match local.get_mut(key) {
            Some(local_item) => merge_toml_item(
                app_type,
                target,
                next_path,
                local_item,
                incoming_item,
                resolution,
                conflicts,
            )?,
            None => {
                local.insert(key, incoming_item.clone());
            }
        }
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "recursive TOML table merge carries immutable context plus per-node state"
)]
fn merge_toml_table_like_with_base(
    app_type: &AppType,
    target: &str,
    path: String,
    local: &mut dyn TableLike,
    base: Option<&dyn TableLike>,
    incoming: &dyn TableLike,
    resolution: ConflictResolution<'_>,
    conflicts: &mut Vec<ConfigConflict>,
) -> Result<(), AppError> {
    for (key, incoming_item) in incoming.iter() {
        let next_path = toml_child_path(&path, key);
        let base_item = base.and_then(|table| table.get(key));
        match local.get_mut(key) {
            Some(local_item) => merge_toml_item_with_base(
                app_type,
                target,
                next_path,
                local_item,
                base_item,
                incoming_item,
                resolution,
                conflicts,
            )?,
            None => {
                if let Some(base_item) = base_item {
                    if toml_items_equal(incoming_item, base_item) {
                        // The user removed a key cc-switch did not change; keep
                        // the user's removal.
                        continue;
                    }
                    // cc-switch changed this key (prefer-incoming); re-introduce it.
                    local.insert(key, incoming_item.clone());
                } else {
                    local.insert(key, incoming_item.clone());
                }
            }
        }
    }

    if let Some(base) = base {
        let removed_keys = base
            .iter()
            .filter(|(key, _)| !incoming.contains_key(key) && local.contains_key(key))
            .map(|(key, base_item)| (key.to_string(), base_item.clone()))
            .collect::<Vec<_>>();
        for (key, base_item) in removed_keys {
            let Some(local_item) = local.get(&key) else {
                continue;
            };
            let _ = (local_item, base_item);
            // cc-switch removed this key (it existed in base but not in
            // incoming); prefer-incoming applies the removal.
            local.remove(&key);
        }
    }

    Ok(())
}

fn toml_items_equal(left: &Item, right: &Item) -> bool {
    match (
        left.as_value(),
        right.as_value(),
        left.as_table_like(),
        right.as_table_like(),
    ) {
        (Some(left_value), Some(right_value), _, _) => {
            left_value.to_string().trim() == right_value.to_string().trim()
        }
        (_, _, Some(left_table), Some(right_table)) => toml_tables_equal(left_table, right_table),
        _ => left.to_string().trim() == right.to_string().trim(),
    }
}

fn toml_tables_equal(left: &dyn TableLike, right: &dyn TableLike) -> bool {
    left.iter().count() == right.iter().count()
        && left.iter().all(|(key, left_item)| {
            right
                .get(key)
                .is_some_and(|right_item| toml_items_equal(left_item, right_item))
        })
}

fn json_child_path(parent: &str, key: &str) -> String {
    if parent.is_empty() {
        key.to_string()
    } else {
        format!("{parent}.{key}")
    }
}

fn toml_child_path(parent: &str, key: &str) -> String {
    if parent.is_empty() {
        key.to_string()
    } else {
        format!("{parent}.{key}")
    }
}

fn display_path(path: &str) -> String {
    if path.is_empty() {
        "<root>".to_string()
    } else {
        path.to_string()
    }
}

fn json_display(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        _ => serde_json::to_string(value).unwrap_or_else(|_| value.to_string()),
    }
}

fn toml_display(item: &Item) -> String {
    item.to_string().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_merge_preserves_local_and_adds_incoming_nested_keys() {
        let local = json!({
            "env": {
                "LOCAL": "keep",
                "SAME": "value"
            }
        });
        let incoming = json!({
            "env": {
                "REMOTE": "add",
                "SAME": "value"
            }
        });

        let merged = merge_json_live(
            &AppType::Claude,
            "settings.json",
            local,
            &incoming,
            ConflictPolicy::Fail.into(),
        )
        .unwrap();

        assert_eq!(
            merged,
            json!({
                "env": {
                    "LOCAL": "keep",
                    "REMOTE": "add",
                    "SAME": "value"
                }
            })
        );
    }

    #[test]
    fn json_merge_prefers_incoming_on_scalar_difference() {
        let local = json!({ "env": { "MODEL": "local" } });
        let incoming = json!({ "env": { "MODEL": "incoming" } });

        let merged = merge_json_live(
            &AppType::Claude,
            "settings.json",
            local,
            &incoming,
            ConflictPolicy::PreferIncoming.into(),
        )
        .unwrap();

        assert_eq!(merged, json!({ "env": { "MODEL": "incoming" } }));
    }

    #[test]
    fn json_merge_prefers_incoming_for_multiple_differences() {
        let local = json!({
            "env": {
                "MODEL": "local",
                "TOKEN": "local-token"
            }
        });
        let incoming = json!({
            "env": {
                "MODEL": "incoming",
                "TOKEN": "incoming-token"
            }
        });

        let merged = merge_json_live(
            &AppType::Claude,
            "settings.json",
            local,
            &incoming,
            ConflictPolicy::PreferIncoming.into(),
        )
        .unwrap();

        assert_eq!(
            merged,
            json!({
                "env": {
                    "MODEL": "incoming",
                    "TOKEN": "incoming-token"
                }
            })
        );
    }

    #[test]
    fn json_merge_can_prefer_incoming_conflict() {
        let local = json!({ "array": ["local"] });
        let incoming = json!({ "array": ["incoming"] });

        let merged = merge_json_live(
            &AppType::Claude,
            "settings.json",
            local,
            &incoming,
            ConflictPolicy::PreferIncoming.into(),
        )
        .unwrap();

        assert_eq!(merged, json!({ "array": ["incoming"] }));
    }

    #[test]
    fn json_merge_with_base_updates_when_local_matches_base() {
        let base = json!({
            "options": {
                "baseURL": "https://old.example.com/v1",
                "apiKey": "sk-old"
            },
            "models": {
                "main": { "name": "Main" }
            }
        });
        let local = base.clone();
        let incoming = json!({
            "options": {
                "baseURL": "https://new.example.com/v1",
                "apiKey": "sk-new"
            },
            "models": {
                "main": { "name": "Main Updated" }
            }
        });

        let merged = merge_json_with_base_live(
            &AppType::OpenCode,
            "opencode.json provider.local",
            local,
            &base,
            &incoming,
            ConflictPolicy::Fail.into(),
        )
        .unwrap();

        assert_eq!(merged, incoming);
    }

    #[test]
    fn json_merge_with_base_prefers_incoming_when_local_and_incoming_changed() {
        let base = json!({
            "options": {
                "baseURL": "https://old.example.com/v1",
                "apiKey": "sk-old"
            }
        });
        let local = json!({
            "options": {
                "baseURL": "https://local.example.com/v1",
                "apiKey": "sk-old"
            }
        });
        let incoming = json!({
            "options": {
                "baseURL": "https://incoming.example.com/v1",
                "apiKey": "sk-new"
            }
        });

        let merged = merge_json_with_base_live(
            &AppType::OpenCode,
            "opencode.json provider.local",
            local,
            &base,
            &incoming,
            ConflictPolicy::PreferIncoming.into(),
        )
        .unwrap();

        // Prefer-incoming: cc-switch's values win even where the user diverged.
        assert_eq!(
            merged.pointer("/options/baseURL").and_then(Value::as_str),
            Some("https://incoming.example.com/v1")
        );
        assert_eq!(
            merged.pointer("/options/apiKey").and_then(Value::as_str),
            Some("sk-new")
        );
    }

    #[test]
    fn json_merge_with_base_removes_deleted_incoming_keys_when_local_matches_base() {
        let base = json!({
            "npm": "@ai-sdk/openai-compatible",
            "options": {
                "baseURL": "https://old.example.com/v1"
            },
            "modalities": { "input": ["text", "image"] },
            "localOnly": true
        });
        let local = base.clone();
        let incoming = json!({
            "npm": "@ai-sdk/openai-compatible",
            "options": {
                "baseURL": "https://new.example.com/v1"
            },
            "localOnly": true
        });

        let merged = merge_json_with_base_live(
            &AppType::OpenCode,
            "opencode.json provider.vision",
            local,
            &base,
            &incoming,
            ConflictPolicy::Fail.into(),
        )
        .unwrap();

        assert!(merged.get("modalities").is_none());
        assert_eq!(
            merged.pointer("/options/baseURL").and_then(Value::as_str),
            Some("https://new.example.com/v1")
        );
        assert_eq!(merged.get("localOnly"), Some(&json!(true)));
    }

    #[test]
    fn json_merge_with_base_prefers_incoming_removal_when_changed_locally() {
        let base = json!({
            "modalities": { "input": ["text", "image"] }
        });
        let local = json!({
            "modalities": { "input": ["text"] }
        });
        let incoming = json!({});

        let merged = merge_json_with_base_live(
            &AppType::OpenCode,
            "opencode.json provider.vision",
            local,
            &base,
            &incoming,
            ConflictPolicy::PreferIncoming.into(),
        )
        .unwrap();

        // Prefer-incoming: cc-switch removed the key, so the removal wins even
        // though the user had locally edited it.
        assert!(merged.get("modalities").is_none());
    }

    #[test]
    fn json_merge_with_base_preserves_live_deleted_key_when_incoming_matches_base() {
        let base = json!({
            "options": {
                "baseURL": "https://old.example.com/v1",
                "apiKey": "sk-old"
            }
        });
        let local = json!({
            "options": {
                "baseURL": "https://old.example.com/v1"
            }
        });
        let incoming = json!({
            "options": {
                "baseURL": "https://new.example.com/v1",
                "apiKey": "sk-old"
            }
        });

        let merged = merge_json_with_base_live(
            &AppType::OpenCode,
            "opencode.json provider.vision",
            local,
            &base,
            &incoming,
            ConflictPolicy::Fail.into(),
        )
        .unwrap();

        assert_eq!(
            merged.pointer("/options/baseURL").and_then(Value::as_str),
            Some("https://new.example.com/v1")
        );
        assert!(merged.pointer("/options/apiKey").is_none());
    }

    #[test]
    fn json_merge_with_base_prefers_incoming_when_live_deleted_key_and_incoming_changed() {
        let base = json!({
            "options": {
                "baseURL": "https://old.example.com/v1",
                "apiKey": "sk-old"
            }
        });
        let local = json!({
            "options": {
                "baseURL": "https://old.example.com/v1"
            }
        });
        let incoming = json!({
            "options": {
                "baseURL": "https://new.example.com/v1",
                "apiKey": "sk-new"
            }
        });

        let merged = merge_json_with_base_live(
            &AppType::OpenCode,
            "opencode.json provider.vision",
            local,
            &base,
            &incoming,
            ConflictPolicy::PreferIncoming.into(),
        )
        .unwrap();

        // Prefer-incoming: cc-switch re-introduces apiKey (it changed it from
        // base) and updates baseURL.
        assert_eq!(
            merged.pointer("/options/baseURL").and_then(Value::as_str),
            Some("https://new.example.com/v1")
        );
        assert_eq!(
            merged.pointer("/options/apiKey").and_then(Value::as_str),
            Some("sk-new")
        );
    }

    #[test]
    fn toml_merge_preserves_local_and_adds_nested_incoming_keys() {
        let local = r#"
model = "sonnet"
[model_providers.local]
base_url = "https://local.example"
"#;
        let incoming = r#"
model = "sonnet"
[model_providers.local]
api_key_env_var = "KEY"
"#;

        let merged = merge_toml_live(
            &AppType::Codex,
            "config.toml",
            local,
            incoming,
            ConflictPolicy::Fail.into(),
        )
        .unwrap();

        assert!(merged.contains("base_url = \"https://local.example\""));
        assert!(merged.contains("api_key_env_var = \"KEY\""));
    }

    #[test]
    fn toml_merge_prefers_incoming_on_scalar_difference() {
        let merged = merge_toml_live(
            &AppType::Codex,
            "config.toml",
            "model = \"local\"",
            "model = \"incoming\"",
            ConflictPolicy::PreferIncoming.into(),
        )
        .unwrap();

        assert!(merged.contains("model = \"incoming\""));
        assert!(!merged.contains("model = \"local\""));
    }

    #[test]
    fn toml_merge_with_base_removes_deleted_incoming_sections_when_local_matches_base() {
        let base = r#"
model_provider = "rightcode"

[model_providers.rightcode]
name = "RightCode"
base_url = "https://rightcode.example/v1"
wire_api = "responses"
"#;
        let local = r#"
model_provider = "rightcode"
local_only = "kept"

[model_providers.rightcode]
name = "RightCode"
base_url = "https://rightcode.example/v1"
wire_api = "responses"
"#;
        let incoming = r#"
model_provider = "aihubmix"

[model_providers.aihubmix]
name = "AiHubMix"
base_url = "https://aihubmix.example/v1"
wire_api = "responses"
"#;

        let merged = merge_toml_with_base_live(
            &AppType::Codex,
            "config.toml",
            local,
            base,
            incoming,
            ConflictPolicy::PreferIncoming.into(),
        )
        .unwrap();
        let parsed: toml::Value = toml::from_str(&merged).expect("parse merged TOML");
        let providers = parsed
            .get("model_providers")
            .and_then(|value| value.as_table())
            .expect("model providers table");

        assert!(providers.get("rightcode").is_none());
        assert_eq!(
            providers
                .get("aihubmix")
                .and_then(|provider| provider.get("base_url"))
                .and_then(|value| value.as_str()),
            Some("https://aihubmix.example/v1")
        );
        assert_eq!(
            parsed.get("local_only").and_then(|value| value.as_str()),
            Some("kept")
        );
    }

    #[test]
    fn toml_merge_prefers_incoming_for_multiple_differences() {
        let merged = merge_toml_live(
            &AppType::Codex,
            "config.toml",
            r#"
model = "local"
model_provider = "local-provider"
"#,
            r#"
model = "incoming"
model_provider = "incoming-provider"
"#,
            ConflictPolicy::PreferIncoming.into(),
        )
        .unwrap();

        assert!(merged.contains("model = \"incoming\""));
        assert!(merged.contains("model_provider = \"incoming-provider\""));
        assert!(!merged.contains("local"));
    }

    #[test]
    fn env_merge_prefers_incoming_for_multiple_differences() {
        let local = HashMap::from([
            ("API_KEY".to_string(), "local".to_string()),
            ("BASE_URL".to_string(), "https://local.example".to_string()),
        ]);
        let incoming = HashMap::from([
            ("API_KEY".to_string(), "incoming".to_string()),
            (
                "BASE_URL".to_string(),
                "https://incoming.example".to_string(),
            ),
        ]);

        let merged = merge_env_live(
            &AppType::Gemini,
            ".env",
            local,
            &incoming,
            ConflictPolicy::PreferIncoming.into(),
        )
        .unwrap();

        assert_eq!(merged.get("API_KEY").map(String::as_str), Some("incoming"));
        assert_eq!(
            merged.get("BASE_URL").map(String::as_str),
            Some("https://incoming.example")
        );
    }
}
