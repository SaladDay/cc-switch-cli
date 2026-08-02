mod hermes;
mod projection;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::session_manager::paged_manifest::PAGE_SIZE;
use crate::session_manager::{SessionMeta, SessionUsageSummary};

pub(crate) use projection::project_main_connection;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SessionCostIdentity {
    pub(crate) provider_id: String,
    pub(crate) session_id: String,
    pub(crate) source_path: Option<String>,
}

impl From<&SessionMeta> for SessionCostIdentity {
    fn from(row: &SessionMeta) -> Self {
        Self {
            provider_id: row.provider_id.clone(),
            session_id: row.session_id.clone(),
            source_path: row.source_path.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct QueryControl {
    pub(crate) active_cost_seq: Arc<AtomicU64>,
    pub(crate) cost_seq: u64,
    pub(crate) deadline: Instant,
}

impl QueryControl {
    pub(crate) fn is_superseded(&self) -> bool {
        self.active_cost_seq.load(Ordering::Acquire) != self.cost_seq
    }

    pub(crate) fn deadline_exceeded(&self) -> bool {
        Instant::now() >= self.deadline
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.is_superseded() || self.deadline_exceeded()
    }

    fn install_progress_handler(&self, conn: &rusqlite::Connection) {
        let control = self.clone();
        conn.progress_handler(1_000, Some(move || control.is_cancelled()));
    }

    fn clear_progress_handler(conn: &rusqlite::Connection) {
        conn.progress_handler(0, None::<fn() -> bool>);
    }
}

/// Remove identities that cannot be mapped unambiguously on the visible page.
///
/// Exact duplicate rows collapse harmlessly. A logical provider/session pair
/// with different source paths is withheld entirely so an overlay can never be
/// attached to the wrong visible row.
fn unambiguous_identities(identities: &[SessionCostIdentity]) -> Vec<SessionCostIdentity> {
    let mut paths = HashMap::<(&str, &str), HashSet<Option<&str>>>::new();
    for identity in identities {
        paths
            .entry((identity.provider_id.as_str(), identity.session_id.as_str()))
            .or_default()
            .insert(identity.source_path.as_deref());
    }

    let mut seen = HashSet::new();
    identities
        .iter()
        .filter(|identity| {
            paths
                .get(&(identity.provider_id.as_str(), identity.session_id.as_str()))
                .is_some_and(|sources| sources.len() == 1)
        })
        .filter(|identity| seen.insert((*identity).clone()))
        .cloned()
        .collect()
}

/// Project one immutable manifest page. Every failure is deliberately local:
/// the Sessions page keeps metadata and renders `-` for unavailable usage.
pub(crate) fn project_page(
    identities: &[SessionCostIdentity],
    control: &QueryControl,
) -> HashMap<SessionCostIdentity, SessionUsageSummary> {
    if identities.is_empty() || identities.len() > PAGE_SIZE || control.is_cancelled() {
        return HashMap::new();
    }
    let identities = unambiguous_identities(identities);
    let mut overlays = HashMap::new();

    let main_identities = identities
        .iter()
        .filter(|identity| {
            matches!(
                identity.provider_id.as_str(),
                "claude" | "codex" | "gemini" | "opencode"
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    if !main_identities.is_empty() {
        match Database::open_readonly_current_schema_with_busy_timeout(
            std::time::Duration::from_millis(250),
        ) {
            Ok(db) => {
                let result = (|| {
                    let conn = lock_conn!(db.conn);
                    conn.busy_timeout(std::time::Duration::from_millis(250))?;
                    conn.pragma_update(None, "query_only", true)?;
                    control.install_progress_handler(&conn);
                    let result = project_main_connection(&conn, &main_identities, control);
                    QueryControl::clear_progress_handler(&conn);
                    result
                })();
                match result {
                    Ok(values) => overlays.extend(values),
                    Err(error) => {
                        log::debug!("[SESSION-COST] main database projection unavailable: {error}")
                    }
                }
            }
            Err(error) => {
                log::debug!("[SESSION-COST] main database snapshot unavailable: {error}")
            }
        }
    }

    let hermes_identities = identities
        .iter()
        .filter(|identity| identity.provider_id == "hermes")
        .cloned()
        .collect::<Vec<_>>();
    if !hermes_identities.is_empty() && !control.is_cancelled() {
        match hermes::project(&hermes_identities, control) {
            Ok(values) => overlays.extend(values),
            Err(error) => log::debug!("[SESSION-COST] Hermes projection unavailable: {error}"),
        }
    }

    overlays
}

#[cfg(test)]
mod benchmark_tests;
#[cfg(test)]
mod tests;
