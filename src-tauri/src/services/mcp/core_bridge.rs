//! Host-owned MCP path and I/O bridge for cc-switch-core.

use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
};

use cc_switch_core::{
    builtin_app_adapter, mcp_config_target, project_mcp_server, project_mcp_servers,
    replace_mcp_servers, McpConfigTarget, McpImport, McpNativeSnapshot, McpServerProjection,
};
use serde_json::{Map, Value};

use crate::{
    app_config::{AppType, McpServer},
    database::{McpLiveChange, McpLiveTarget, McpNativeLinkUpdate, McpOwnedServer},
    error::AppError,
};

pub(super) fn project_server(app: &AppType, id: &str, server: &Value) -> Result<(), AppError> {
    project_live(
        app,
        id,
        LiveProjectionIntent::Enable(server),
        false,
        false,
        None,
    )
    .map(|_| ())
}

pub(super) fn remove_server(app: &AppType, id: &str) -> Result<(), AppError> {
    project_live(app, id, LiveProjectionIntent::Remove, false, false, None).map(|_| ())
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
        Ok(LiveProjection {
            document: replace_mcp_servers(&core_app, contents, &servers).map_err(core_error)?,
            native_snapshots: HashMap::new(),
        })
    })
    .map(|_| ())
}

pub(super) struct LiveReceipt {
    writes: Vec<McpFileReceipt>,
}

pub(super) struct ImportObservation {
    target: McpConfigTarget,
    path: PathBuf,
    contents: Option<Vec<u8>>,
}

impl ImportObservation {
    pub(super) fn is_current(&self) -> Result<bool, AppError> {
        Ok(read_target(self.target, &self.path)? == self.contents)
    }
}

pub(super) fn observe_imports(
    app: &AppType,
) -> Result<(Vec<McpImport>, ImportObservation), AppError> {
    let core_app = app.as_core();
    let target = mcp_config_target(&core_app)
        .ok_or_else(|| AppError::InvalidInput(format!("{} does not support MCP", app.as_str())))?;
    let path = target_path(target)?;
    let contents = read_target(target, &path)?;
    let imports = crate::mcp::import_mcp_servers_compat(app, contents.as_deref())?;
    Ok((
        imports,
        ImportObservation {
            target,
            path,
            contents,
        },
    ))
}

struct McpFileReceipt {
    target: McpConfigTarget,
    path: PathBuf,
    before: Option<Vec<u8>>,
    after: Vec<u8>,
}

enum LiveUpdate {
    Skipped,
    Managed {
        write: Option<McpFileReceipt>,
        native_snapshots: HashMap<String, String>,
    },
}

struct LiveProjection {
    document: Option<String>,
    native_snapshots: HashMap<String, String>,
}

#[derive(Clone, Copy)]
enum LiveProjectionIntent<'a> {
    Enable(&'a Value),
    Disable(&'a Value),
    Remove,
}

pub(super) fn apply_server_change(
    previous: Option<&McpServer>,
    next: Option<&McpServer>,
    targets: &[McpLiveTarget],
) -> Result<McpLiveChange<LiveReceipt>, AppError> {
    let id = next
        .or(previous)
        .expect("an MCP change has a server")
        .id
        .as_str();
    if let Some(server) = next {
        cc_switch_core::validate_mcp_server(id, &server.server).map_err(core_error)?;
    }
    let mut receipt = LiveReceipt { writes: Vec::new() };
    let mut native_links = Vec::new();
    let mut failures = Vec::new();
    for target in targets {
        let app = &target.app;
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
                LiveProjectionIntent::Enable(&server.server)
            }
            Some(server) => LiveProjectionIntent::Disable(&server.server),
            None => LiveProjectionIntent::Remove,
        };
        let reject_existing =
            !target.owned && next.is_some_and(|server| server.apps.is_enabled_for(app));
        match project_live(
            app,
            id,
            projection,
            reject_existing,
            target.owned,
            target.native_snapshot.as_ref(),
        ) {
            Ok(LiveUpdate::Managed {
                write,
                mut native_snapshots,
            }) => {
                if let Some(write) = write {
                    receipt.writes.push(write);
                }
                native_links.push(McpNativeLinkUpdate {
                    server_id: id.to_owned(),
                    app: app.clone(),
                    native_snapshot_json: native_snapshots.remove(id),
                });
            }
            Ok(LiveUpdate::Skipped) => {}
            Err(error) => failures.push(format!("{}: {error}", app.as_str())),
        }
    }
    if failures.is_empty() {
        Ok(McpLiveChange {
            receipt,
            link_updates: native_links,
        })
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
        Ok(LiveProjection {
            document: project_mcp_servers(&core_app, contents, &changes).map_err(core_error)?,
            native_snapshots: HashMap::new(),
        })
    })
    .map(|_| ())
}

pub(super) fn apply_owned_server_sync(
    app: &AppType,
    servers: &[McpOwnedServer],
) -> Result<McpLiveChange<LiveReceipt>, AppError> {
    if servers.is_empty() {
        return Ok(McpLiveChange {
            receipt: LiveReceipt { writes: Vec::new() },
            link_updates: Vec::new(),
        });
    }
    let core_app = app.as_core();
    let adapter = builtin_app_adapter(&core_app);
    let update = update_live(app, |contents| {
        let observed_snapshots = servers
            .iter()
            .map(|target| {
                adapter
                    .capture_mcp_native_snapshot(contents, &target.server.id)
                    .map_err(core_error)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let native_snapshots = servers
            .iter()
            .zip(&observed_snapshots)
            .filter_map(|(target, observed)| {
                observed
                    .as_ref()
                    .or(target.native_snapshot.as_ref())
                    .map(|snapshot| {
                        serialize_snapshot(&target.server.id, snapshot)
                            .map(|snapshot| (target.server.id.clone(), snapshot))
                    })
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        let changes = owned_server_changes(app, servers, &observed_snapshots);
        Ok(LiveProjection {
            document: project_mcp_servers(&core_app, contents, &changes).map_err(core_error)?,
            native_snapshots,
        })
    })?;
    let (write, native_snapshots) = match update {
        LiveUpdate::Managed {
            write,
            native_snapshots,
        } => (write, native_snapshots),
        LiveUpdate::Skipped => {
            return Ok(McpLiveChange {
                receipt: LiveReceipt { writes: Vec::new() },
                link_updates: Vec::new(),
            });
        }
    };
    let link_updates = servers
        .iter()
        .map(|target| McpNativeLinkUpdate {
            server_id: target.server.id.clone(),
            app: app.clone(),
            native_snapshot_json: native_snapshots.get(&target.server.id).cloned(),
        })
        .collect();
    Ok(McpLiveChange {
        receipt: LiveReceipt {
            writes: write.into_iter().collect(),
        },
        link_updates,
    })
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

fn owned_server_changes<'a>(
    app: &AppType,
    servers: &'a [McpOwnedServer],
    observed_snapshots: &'a [Option<McpNativeSnapshot>],
) -> Vec<(&'a str, McpServerProjection<'a>)> {
    let mut servers = servers.iter().zip(observed_snapshots).collect::<Vec<_>>();
    servers.sort_by(|(left, _), (right, _)| left.server.id.cmp(&right.server.id));
    servers
        .into_iter()
        .map(|(target, observed_snapshot)| {
            let server = &target.server;
            let projection = if server.apps.is_enabled_for(app) {
                match observed_snapshot
                    .as_ref()
                    .or(target.native_snapshot.as_ref())
                {
                    Some(snapshot) => McpServerProjection::Restore {
                        server: &server.server,
                        snapshot,
                    },
                    None => McpServerProjection::Enable(&server.server),
                }
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
    intent: LiveProjectionIntent<'_>,
    reject_existing: bool,
    owned: bool,
    stored_snapshot: Option<&McpNativeSnapshot>,
) -> Result<LiveUpdate, AppError> {
    let core_app = app.as_core();
    let adapter = builtin_app_adapter(&core_app);
    update_live(app, |contents| {
        if reject_existing
            && adapter
                .contains_mcp_server(contents, id)
                .map_err(core_error)?
        {
            return Err(AppError::Conflict(format!(
                "{} already contains an unmanaged MCP server '{id}'",
                app.as_str()
            )));
        }
        let observed_snapshot = adapter
            .capture_mcp_native_snapshot(contents, id)
            .map_err(core_error)?;
        let snapshot = observed_snapshot.as_ref().or(stored_snapshot);
        let projection = match (intent, owned, snapshot) {
            (LiveProjectionIntent::Enable(server), true, Some(snapshot)) => {
                McpServerProjection::Restore { server, snapshot }
            }
            (LiveProjectionIntent::Enable(server), _, _) => McpServerProjection::Enable(server),
            (LiveProjectionIntent::Disable(server), _, _) => McpServerProjection::Disable(server),
            (LiveProjectionIntent::Remove, _, _) => McpServerProjection::Remove,
        };
        let mut native_snapshots = HashMap::new();
        if let Some(snapshot) = snapshot {
            native_snapshots.insert(id.to_owned(), serialize_snapshot(id, snapshot)?);
        }
        Ok(LiveProjection {
            document: project_mcp_server(&core_app, contents, id, projection)
                .map_err(core_error)?,
            native_snapshots,
        })
    })
}

fn update_live(
    app: &AppType,
    project: impl Fn(Option<&[u8]>) -> Result<LiveProjection, AppError>,
) -> Result<LiveUpdate, AppError> {
    if !crate::sync_policy::should_sync_live(app) {
        return Ok(LiveUpdate::Skipped);
    }

    let core_app = app.as_core();
    let Some(target) = mcp_config_target(&core_app) else {
        return Ok(LiveUpdate::Skipped);
    };
    if target == McpConfigTarget::Hermes {
        let path = target_path(target)?;
        return crate::hermes_config::update_hermes_config_source_with_receipt(|contents| {
            project(contents).map(|projection| projection.document)
        })
        .map(|receipt| LiveUpdate::Managed {
            write: receipt.map(|(before, after)| McpFileReceipt {
                target,
                path,
                before,
                after,
            }),
            native_snapshots: HashMap::new(),
        });
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
        let projection = project(contents.as_deref().or(default_contents.as_deref()))?;
        let Some(projected) = projection.document else {
            return Ok(LiveUpdate::Managed {
                write: None,
                native_snapshots: projection.native_snapshots,
            });
        };
        if crate::config::write_file_if_current(&path, contents.as_deref(), projected.as_bytes())? {
            return Ok(LiveUpdate::Managed {
                write: Some(McpFileReceipt {
                    target,
                    path,
                    before: contents,
                    after: projected.into_bytes(),
                }),
                native_snapshots: projection.native_snapshots,
            });
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

fn serialize_snapshot(id: &str, snapshot: &McpNativeSnapshot) -> Result<String, AppError> {
    serde_json::to_string(snapshot).map_err(|error| {
        AppError::McpValidation(format!(
            "failed to store native snapshot for '{id}': {error}"
        ))
    })
}
