//! Host-owned MCP path and I/O bridge for cc-switch-core.

use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
};

use cc_switch_core::{
    mcp_config_target, project_mcp_server, project_mcp_servers, replace_mcp_servers,
    McpConfigTarget, McpServerProjection,
};
use serde_json::{Map, Value};

use crate::{
    app_config::{AppType, McpServer},
    error::AppError,
};

pub(super) fn project_server(app: &AppType, id: &str, server: &Value) -> Result<(), AppError> {
    project_live(app, id, McpServerProjection::Enable(server)).map(|_| ())
}

pub(super) fn remove_server(app: &AppType, id: &str) -> Result<(), AppError> {
    project_live(app, id, McpServerProjection::Remove).map(|_| ())
}

pub(super) fn replace_servers(
    app: &AppType,
    servers: &HashMap<String, Value>,
) -> Result<(), AppError> {
    let core_app = app.as_core();
    let servers = servers
        .iter()
        .map(|(id, server)| (id.clone(), server.clone()))
        .collect::<Map<_, _>>();
    update_live(app, |contents| {
        replace_mcp_servers(&core_app, contents, &servers).map_err(core_error)
    })
    .map(|_| ())
}

pub(super) struct LiveReceipt {
    writes: Vec<McpFileReceipt>,
}

struct McpFileReceipt {
    target: McpConfigTarget,
    path: PathBuf,
    before: Option<Vec<u8>>,
    after: Vec<u8>,
}

pub(super) fn apply_server_change(
    previous: Option<&McpServer>,
    next: Option<&McpServer>,
    affected_apps: &[AppType],
) -> Result<LiveReceipt, AppError> {
    let id = next
        .or(previous)
        .expect("an MCP change has a server")
        .id
        .as_str();
    if let Some(server) = next {
        cc_switch_core::validate_mcp_server(id, &server.server).map_err(core_error)?;
    }
    let mut receipt = LiveReceipt { writes: Vec::new() };
    let mut failures = Vec::new();
    for app in affected_apps {
        if let Some(server) = next.filter(|server| server.apps.is_enabled_for(app)) {
            if let Err(error) =
                cc_switch_core::validate_mcp_server_for_app(&app.as_core(), id, &server.server)
            {
                failures.push(format!("{}: {error}", app.as_str()));
                continue;
            }
        }
        let projection = match next {
            Some(server) if server.apps.is_enabled_for(app) => {
                McpServerProjection::Enable(&server.server)
            }
            Some(server) => McpServerProjection::Disable(&server.server),
            None => McpServerProjection::Remove,
        };
        match project_live(app, id, projection) {
            Ok(Some(write)) => receipt.writes.push(write),
            Ok(None) => {}
            Err(error) => failures.push(format!("{}: {error}", app.as_str())),
        }
    }
    if failures.is_empty() {
        Ok(receipt)
    } else {
        let recovery = rollback_writes(receipt).err();
        let error = AppError::Message(format!("MCP live update failed: {}", failures.join("; ")));
        match recovery {
            Some(recovery) => Err(AppError::Message(format!(
                "{error}; MCP live rollback failed: {recovery}"
            ))),
            None => Err(error),
        }
    }
}

pub(super) fn rollback_server_change(receipt: LiveReceipt) -> Result<(), AppError> {
    rollback_writes(receipt)
}

pub(super) fn project_servers(
    app: &AppType,
    base_text: &str,
    servers: &HashMap<String, McpServer>,
) -> Result<String, AppError> {
    let core_app = app.as_core();
    let changes = server_changes(app, servers);
    project_mcp_servers(&core_app, Some(base_text.as_bytes()), &changes)
        .map(|projected| projected.unwrap_or_else(|| base_text.to_owned()))
        .map_err(core_error)
}

pub(super) fn sync_servers(
    app: &AppType,
    servers: &HashMap<String, McpServer>,
) -> Result<(), AppError> {
    let core_app = app.as_core();
    let changes = server_changes(app, servers);
    update_live(app, |contents| {
        project_mcp_servers(&core_app, contents, &changes).map_err(core_error)
    })
    .map(|_| ())
}

fn server_changes<'a>(
    app: &AppType,
    servers: &'a HashMap<String, McpServer>,
) -> Vec<(&'a str, McpServerProjection<'a>)> {
    let mut servers = servers.values().collect::<Vec<_>>();
    servers.sort_by(|left, right| left.id.cmp(&right.id));
    servers
        .into_iter()
        .map(|server| {
            let projection = if server.apps.is_enabled_for(app) {
                McpServerProjection::Enable(&server.server)
            } else {
                McpServerProjection::Disable(&server.server)
            };
            (server.id.as_str(), projection)
        })
        .collect()
}

fn project_live(
    app: &AppType,
    id: &str,
    projection: McpServerProjection<'_>,
) -> Result<Option<McpFileReceipt>, AppError> {
    let core_app = app.as_core();
    update_live(app, |contents| {
        project_mcp_server(&core_app, contents, id, projection).map_err(core_error)
    })
}

fn update_live(
    app: &AppType,
    project: impl Fn(Option<&[u8]>) -> Result<Option<String>, AppError>,
) -> Result<Option<McpFileReceipt>, AppError> {
    if !crate::sync_policy::should_sync_live(app) {
        return Ok(None);
    }

    let core_app = app.as_core();
    let Some(target) = mcp_config_target(&core_app) else {
        return Ok(None);
    };
    if target == McpConfigTarget::Hermes {
        let path = target_path(target)?;
        return crate::hermes_config::update_hermes_config_source_with_receipt(&project).map(
            |receipt| {
                receipt.map(|(before, after)| McpFileReceipt {
                    target,
                    path,
                    before,
                    after,
                })
            },
        );
    }

    let path = target_path(target)?;
    for _ in 0..4 {
        let contents = read_target(target, &path)?;
        let default_contents = if target == McpConfigTarget::OpenCode && contents.is_none() {
            Some(
                serde_json::to_vec(&crate::opencode_config::default_config())
                    .map_err(|source| AppError::JsonSerialize { source })?,
            )
        } else {
            None
        };
        let Some(projected) = project(contents.as_deref().or(default_contents.as_deref()))? else {
            return Ok(None);
        };
        if crate::config::write_file_if_current(&path, contents.as_deref(), projected.as_bytes())? {
            return Ok(Some(McpFileReceipt {
                target,
                path,
                before: contents,
                after: projected.into_bytes(),
            }));
        }
    }
    Err(AppError::Conflict(format!(
        "{} MCP config kept changing while it was being updated",
        app.as_str()
    )))
}

fn target_path(target: McpConfigTarget) -> Result<PathBuf, AppError> {
    match target {
        McpConfigTarget::Claude => Ok(crate::config::get_claude_mcp_path()),
        McpConfigTarget::Codex => Ok(crate::codex_config::get_codex_config_path()),
        McpConfigTarget::Gemini => Ok(crate::gemini_config::get_gemini_settings_path()),
        McpConfigTarget::OpenCode => Ok(crate::opencode_config::get_opencode_config_path()),
        McpConfigTarget::Hermes => Ok(crate::hermes_config::get_hermes_config_path()),
        McpConfigTarget::GrokBuild => Err(AppError::InvalidInput(
            "CLI does not expose the GrokBuild MCP target".to_owned(),
        )),
    }
}

fn read_target(target: McpConfigTarget, path: &Path) -> Result<Option<Vec<u8>>, AppError> {
    if target == McpConfigTarget::Claude {
        return crate::claude_mcp::read_mcp_json().map(|text| text.map(String::into_bytes));
    }

    match fs::read(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AppError::io(path, error)),
    }
}

fn rollback_writes(receipt: LiveReceipt) -> Result<(), AppError> {
    let mut failures = Vec::new();
    for write in receipt.writes.into_iter().rev() {
        if let Err(error) = restore_write(write) {
            failures.push(error.to_string());
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(AppError::Message(failures.join("; ")))
    }
}

fn restore_write(write: McpFileReceipt) -> Result<(), AppError> {
    let restored = if write.target == McpConfigTarget::Hermes {
        crate::hermes_config::restore_hermes_config_source_if_current(
            &write.after,
            write.before.as_deref(),
        )?
    } else {
        crate::config::replace_file_if_current(
            &write.path,
            Some(write.after.as_slice()),
            write.before.as_deref(),
        )?
    };
    if restored {
        Ok(())
    } else {
        Err(AppError::Conflict(format!(
            "{} changed after the MCP update",
            write.path.display()
        )))
    }
}

fn core_error(error: cc_switch_core::McpConfigError) -> AppError {
    AppError::McpValidation(error.to_string())
}
