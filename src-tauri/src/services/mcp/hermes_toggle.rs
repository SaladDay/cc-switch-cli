//! Hermes keeps its YAML and backup policy around Core's MCP execution.

use std::sync::MutexGuard;

use cc_switch_core::{McpConfigTarget, McpNativeSnapshot};
use serde_json::Value;
use serde_yaml::Value as Yaml;

use super::{native_file::NativeFile, toggle::NativeToggle};
use crate::{
    error::AppError,
    hermes_config::{self, get_hermes_config_path, lock_live_write},
    mcp::{convert_to_hermes_mcp_spec, merge_hermes_spec},
};

pub(super) struct HermesToggle {
    file: NativeFile,
    _guard: MutexGuard<'static, ()>,
}

impl HermesToggle {
    pub(super) fn observe() -> Result<Self, AppError> {
        let guard = lock_live_write()?;
        Ok(Self {
            file: NativeFile::observe(McpConfigTarget::Hermes, &get_hermes_config_path())?,
            _guard: guard,
        })
    }
}

impl NativeToggle for HermesToggle {
    fn apply(
        &mut self,
        id: &str,
        server: &Value,
        enabled: bool,
        previous_snapshot: Option<&McpNativeSnapshot>,
    ) -> Result<Option<McpNativeSnapshot>, AppError> {
        if previous_snapshot.is_some() {
            return Err(AppError::Database(
                "Unsupported Hermes MCP native snapshot".into(),
            ));
        }
        let entry = enabled
            .then(|| convert_to_hermes_mcp_spec(server))
            .transpose()?;
        let raw =
            std::str::from_utf8(self.file.original().unwrap_or_default()).map_err(|error| {
                AppError::io(
                    self.file.path(),
                    std::io::Error::new(std::io::ErrorKind::InvalidData, error),
                )
            })?;
        let contents = prepare_toggle(raw, id, entry)?;
        if contents == raw {
            return Ok(None);
        }
        // Keep the host's existing pre-write backup and retention policy. A
        // later publication/commit failure does not erase recovery backups.
        if !raw.is_empty() {
            hermes_config::create_hermes_backup(raw)?;
        }
        self.file.publish(&contents)?;
        Ok(None)
    }

    fn rollback(&mut self) -> Result<(), AppError> {
        self.file.rollback()
    }
}

fn prepare_toggle(raw: &str, id: &str, entry: Option<Value>) -> Result<String, AppError> {
    let mut root = hermes_config::parse_hermes_config(raw)?;
    let mut untagged = &mut root;
    while let Yaml::Tagged(tagged) = untagged {
        untagged = &mut tagged.value;
    }
    if untagged.is_null() {
        *untagged = Yaml::Mapping(serde_yaml::Mapping::new());
    }
    let mapping = untagged
        .as_mapping_mut()
        .ok_or_else(|| AppError::Config("Hermes config must be a YAML mapping".into()))?;
    let key = Yaml::String(id.into());
    // Preserve the old policy of replacing a non-mapping MCP section, but do
    // not convert sibling entries through JSON: they may contain YAML-only data.
    let section = Yaml::String("mcp_servers".into());
    let mut servers = mapping
        .get(&section)
        .and_then(Yaml::as_mapping)
        .cloned()
        .unwrap_or_default();
    if let Some(entry) = entry {
        let mut merged = match servers.get(&key) {
            Some(existing) => merge_hermes_spec(&hermes_config::yaml_to_json(existing)?, &entry),
            None => entry,
        };
        // Selection owns activation even when the native entry was disabled.
        merged["enabled"] = Value::Bool(true);
        servers.insert(key, hermes_config::json_to_yaml(&merged)?);
    } else {
        servers.shift_remove(&key);
    }
    let servers = Yaml::Mapping(servers);
    let contents = hermes_config::replace_yaml_section(raw, "mcp_servers", &servers)?;
    mapping.insert(section, servers);
    // Quoted/flow sections, document markers and cross-section aliases can
    // defeat text replacement. Never publish invalid or unrelated YAML changes.
    let rendered = hermes_config::parse_hermes_config(&contents)?;
    if rendered != root {
        return Err(AppError::Config(
            "Hermes MCP update would change unrelated YAML".into(),
        ));
    }
    Ok(contents)
}

#[cfg(test)]
mod tests;
