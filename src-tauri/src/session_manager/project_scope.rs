//! Project-scoped Sessions views over provider metadata.
//!
//! Matching is deliberately lexical and independent of the current filesystem.
//! Historical projects may have moved or been deleted, and resolving them with
//! `canonicalize` would make an otherwise metadata-only operation block on I/O.

use std::collections::HashMap;

use super::paged_manifest::{ManifestError, ManifestReader};

const POSIX_KEY_PREFIX: &str = "posix:";
const WINDOWS_KEY_PREFIX: &str = "windows:";

/// Project range applied before a Sessions metadata or transcript query.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum SessionProjectScope {
    All,
    Unknown,
    Exact {
        /// Original provider-supplied path, retained for display only.
        display_path: String,
        /// Stable lexical identity used for equality checks.
        normalized_path: String,
    },
}

impl SessionProjectScope {
    #[cfg(test)]
    pub(crate) fn exact(display_path: impl Into<String>) -> Option<Self> {
        let display_path = display_path.into();
        let normalized_path = normalize_project_path(&display_path)?;
        Some(Self::Exact {
            display_path,
            normalized_path,
        })
    }

    pub(crate) fn matches(&self, project_dir: Option<&str>) -> bool {
        match self {
            Self::All => true,
            Self::Unknown => project_dir.and_then(normalize_project_path).is_none(),
            Self::Exact {
                normalized_path, ..
            } => project_dir
                .and_then(normalize_project_path)
                .is_some_and(|candidate| candidate == normalized_path.as_str()),
        }
    }

    #[cfg(test)]
    pub(crate) fn display_path(&self) -> Option<&str> {
        match self {
            Self::Exact { display_path, .. } => Some(display_path),
            Self::All | Self::Unknown => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn normalized_path(&self) -> Option<&str> {
        match self {
            Self::Exact {
                normalized_path, ..
            } => Some(normalized_path),
            Self::All | Self::Unknown => None,
        }
    }
}

/// Complete logical Sessions view. Project scope and text query are one atomic
/// condition so an asynchronous result cannot mix an old project with a newer
/// query (or vice versa).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SessionViewSpec {
    pub(crate) project: SessionProjectScope,
    pub(crate) query: String,
}

impl SessionViewSpec {
    pub(crate) fn new(project: SessionProjectScope, query: impl Into<String>) -> Self {
        Self {
            project,
            query: query.into().trim().to_lowercase(),
        }
    }

    #[cfg(test)]
    pub(crate) fn all_projects(query: impl Into<String>) -> Self {
        Self::new(SessionProjectScope::All, query)
    }

    pub(crate) fn is_base_view(&self) -> bool {
        matches!(self.project, SessionProjectScope::All) && self.query.is_empty()
    }
}

/// One exact project option derived exclusively from manifest metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionProjectSummary {
    pub(crate) display_path: String,
    pub(crate) normalized_path: String,
    pub(crate) session_count: usize,
    pub(crate) latest_at: Option<i64>,
}

/// Sessions with no usable project path stay distinct from exact projects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct UnknownProjectSummary {
    pub(crate) session_count: usize,
    pub(crate) latest_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct SessionProjectCatalog {
    pub(crate) projects: Vec<SessionProjectSummary>,
    pub(crate) unknown: UnknownProjectSummary,
    /// Indices into `projects`, sorted by normalized path. This keeps the
    /// user-facing list in recent-first order while active-scope lookup stays
    /// logarithmic and allocation-free on the TUI thread.
    normalized_lookup: Vec<usize>,
}

impl SessionProjectCatalog {
    pub(crate) fn project_position(&self, normalized_path: &str) -> Option<usize> {
        self.normalized_lookup
            .binary_search_by(|index| {
                self.projects[*index]
                    .normalized_path
                    .as_str()
                    .cmp(normalized_path)
            })
            .ok()
            .map(|lookup_index| self.normalized_lookup[lookup_index])
    }
}

/// Aggregate project options a bounded manifest page at a time. This function
/// never opens `source_path` or calls a provider transcript reader.
pub(crate) fn aggregate_project_directories(
    base: &ManifestReader,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<SessionProjectCatalog, ManifestError> {
    let mut projects: HashMap<String, SessionProjectSummary> = HashMap::new();
    let mut unknown = UnknownProjectSummary::default();

    for page_index in 0..base.page_count() {
        ensure_not_cancelled(is_cancelled)?;
        let page = base.load_page(page_index).ok_or_else(|| {
            ManifestError::Corrupt(format!(
                "project catalog source page {page_index} is unreadable in generation {}",
                base.generation()
            ))
        })?;

        for row in page.rows {
            ensure_not_cancelled(is_cancelled)?;
            let latest_at = row.last_active_at.or(row.created_at);
            let Some(display_path) = row.project_dir else {
                update_unknown(&mut unknown, latest_at);
                continue;
            };
            let Some(normalized_path) = normalize_project_path(&display_path) else {
                update_unknown(&mut unknown, latest_at);
                continue;
            };

            let summary =
                projects
                    .entry(normalized_path.clone())
                    .or_insert_with(|| SessionProjectSummary {
                        // Base pages are newest-first, so the first spelling is the
                        // one attached to the most recent session for this project.
                        display_path,
                        normalized_path,
                        session_count: 0,
                        latest_at: None,
                    });
            summary.session_count = summary.session_count.saturating_add(1);
            summary.latest_at = latest_timestamp(summary.latest_at, latest_at);
        }
    }

    ensure_not_cancelled(is_cancelled)?;
    let mut projects = projects.into_values().collect::<Vec<_>>();
    // The normalized path is unique, so this comparator is total and an
    // unstable sort remains deterministic without allocating a second large
    // project vector.
    projects.sort_unstable_by(|a, b| {
        b.latest_at
            .cmp(&a.latest_at)
            .then_with(|| a.normalized_path.cmp(&b.normalized_path))
    });
    let mut normalized_lookup = (0..projects.len()).collect::<Vec<_>>();
    normalized_lookup.sort_unstable_by(|a, b| {
        projects[*a]
            .normalized_path
            .cmp(&projects[*b].normalized_path)
    });
    ensure_not_cancelled(is_cancelled)?;
    Ok(SessionProjectCatalog {
        projects,
        unknown,
        normalized_lookup,
    })
}

/// Case-insensitive substring matching used by the background project picker
/// filter. ASCII paths take a linear-time allocation per candidate; Unicode
/// falls back to full lowercase semantics. Both run off the TUI thread.
pub(crate) fn project_path_contains_query(path: &str, query_lower: &str) -> bool {
    if query_lower.is_empty() {
        return true;
    }
    if path.is_ascii() && query_lower.is_ascii() {
        return path.to_ascii_lowercase().contains(query_lower);
    }
    path.to_lowercase().contains(query_lower)
}

fn update_unknown(summary: &mut UnknownProjectSummary, latest_at: Option<i64>) {
    summary.session_count = summary.session_count.saturating_add(1);
    summary.latest_at = latest_timestamp(summary.latest_at, latest_at);
}

fn latest_timestamp(current: Option<i64>, candidate: Option<i64>) -> Option<i64> {
    match (current, candidate) {
        (Some(current), Some(candidate)) => Some(current.max(candidate)),
        (Some(current), None) => Some(current),
        (None, Some(candidate)) => Some(candidate),
        (None, None) => None,
    }
}

fn ensure_not_cancelled(is_cancelled: &(dyn Fn() -> bool + Sync)) -> Result<(), ManifestError> {
    if is_cancelled() {
        Err(ManifestError::Cancelled)
    } else {
        Ok(())
    }
}

/// Produce a cross-platform lexical path identity without consulting the live
/// filesystem. Windows-looking paths use both slash styles and case folding;
/// other paths use POSIX separators and preserve case. A relative path that
/// contains backslashes is necessarily treated as Windows because its original
/// host is no longer available.
pub(crate) fn normalize_project_path(raw: &str) -> Option<String> {
    let raw = raw.trim_end_matches(['\r', '\n']);
    if raw.trim().is_empty() || raw.contains('\0') {
        return None;
    }

    if looks_like_windows_path(raw) {
        normalize_windows_path(raw).map(|path| format!("{WINDOWS_KEY_PREFIX}{path}"))
    } else {
        normalize_posix_path(raw).map(|path| format!("{POSIX_KEY_PREFIX}{path}"))
    }
}

fn looks_like_windows_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
        || path.starts_with("\\\\")
        || path.starts_with("//")
        || path.contains('\\')
}

fn normalize_posix_path(path: &str) -> Option<String> {
    let rooted = path.starts_with('/');
    let components = normalize_components(path.split('/'), rooted);
    if rooted {
        Some(if components.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", components.join("/"))
        })
    } else if components.is_empty() {
        Some(".".to_string())
    } else {
        Some(components.join("/"))
    }
}

fn normalize_windows_path(path: &str) -> Option<String> {
    let replaced = path.replace('\\', "/");
    let bytes = replaced.as_bytes();
    let (prefix, remainder, rooted) =
        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            let drive = replaced[..2].to_lowercase();
            let remainder = &replaced[2..];
            let rooted = remainder.starts_with('/');
            (drive, remainder.trim_start_matches('/'), rooted)
        } else if replaced.starts_with("//") {
            ("//".to_string(), replaced.trim_start_matches('/'), true)
        } else if replaced.starts_with('/') {
            ("/".to_string(), replaced.trim_start_matches('/'), true)
        } else {
            (String::new(), replaced.as_str(), false)
        };

    let components = normalize_components(remainder.split('/'), rooted)
        .into_iter()
        .map(|component| component.to_lowercase())
        .collect::<Vec<_>>();
    let body = components.join("/");
    let normalized = match (prefix.as_str(), rooted, body.is_empty()) {
        ("//", _, true) => "//".to_string(),
        ("//", _, false) => format!("//{body}"),
        ("/", _, true) => "/".to_string(),
        ("/", _, false) => format!("/{body}"),
        (drive, true, true) if drive.ends_with(':') => format!("{drive}/"),
        (drive, true, false) if drive.ends_with(':') => format!("{drive}/{body}"),
        (drive, false, true) if drive.ends_with(':') => drive.to_string(),
        (drive, false, false) if drive.ends_with(':') => format!("{drive}{body}"),
        ("", false, true) => ".".to_string(),
        ("", false, false) => body,
        _ => return None,
    };
    Some(normalized)
}

fn normalize_components<'a>(
    components: impl IntoIterator<Item = &'a str>,
    rooted: bool,
) -> Vec<&'a str> {
    let mut normalized = Vec::new();
    for component in components {
        match component {
            "" | "." => {}
            ".." => {
                if normalized.last().is_some_and(|last| *last != "..") {
                    normalized.pop();
                } else if !rooted {
                    normalized.push(component);
                }
            }
            _ => normalized.push(component),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tempfile::TempDir;

    use super::*;
    use crate::session_manager::paged_manifest::PagedManifestStore;
    use crate::session_manager::SessionMeta;

    fn publish_catalog_rows(
        rows: impl IntoIterator<Item = SessionMeta>,
    ) -> (TempDir, ManifestReader) {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = PagedManifestStore::open_at(temp.path()).expect("manifest store");
        let mut builder = store.begin_build("codex").expect("manifest builder");
        for row in rows {
            builder.push(row).expect("push row");
        }
        builder.publish().expect("publish manifest");
        let reader = store.open_reader("codex").expect("manifest reader");
        (temp, reader)
    }

    fn meta(id: usize, project_dir: Option<&str>, latest_at: i64) -> SessionMeta {
        SessionMeta {
            provider_id: "codex".to_string(),
            session_id: format!("session-{id}"),
            project_dir: project_dir.map(str::to_string),
            last_active_at: Some(latest_at),
            source_path: Some(format!("/missing/session-{id}.jsonl")),
            ..SessionMeta::default()
        }
    }

    #[test]
    fn lexical_normalization_handles_posix_components_without_prefix_matching() {
        let scope = SessionProjectScope::exact("/repo/foo/./src/..").expect("exact scope");

        assert!(scope.matches(Some("/repo/foo/")));
        assert!(scope.matches(Some("/repo/other/../foo")));
        assert!(!scope.matches(Some("/repo/foo-old")));
        assert!(!scope.matches(Some("/repo/foo/subdir")));
    }

    #[test]
    fn windows_paths_normalize_separators_drive_case_and_components() {
        let scope = SessionProjectScope::exact(r"C:\Work\Repo\src\..").expect("exact scope");

        assert!(scope.matches(Some("c:/work/repo/")));
        assert!(scope.matches(Some(r"C:\WORK\.\REPO")));
        assert!(!scope.matches(Some(r"C:\Work\Repo-Old")));

        let unc = SessionProjectScope::exact(r"\\Server\Share\Repo").expect("UNC scope");
        assert!(unc.matches(Some("//server/share/repo/")));
    }

    #[test]
    fn posix_paths_remain_case_sensitive_and_relative_paths_stay_relative() {
        assert_ne!(
            normalize_project_path("/repo/Foo"),
            normalize_project_path("/repo/foo")
        );
        assert_ne!(
            normalize_project_path("repo/foo"),
            normalize_project_path("/repo/foo")
        );
        assert_eq!(
            normalize_project_path("repo/other/../foo"),
            normalize_project_path("repo/foo")
        );
    }

    #[test]
    fn unknown_scope_matches_only_missing_or_unusable_paths() {
        assert!(SessionProjectScope::Unknown.matches(None));
        assert!(SessionProjectScope::Unknown.matches(Some("")));
        assert!(SessionProjectScope::Unknown.matches(Some("\r\n")));
        assert!(!SessionProjectScope::Unknown.matches(Some("/repo/foo")));
        assert!(SessionProjectScope::All.matches(None));
        assert!(SessionProjectScope::All.matches(Some("/repo/foo")));
    }

    #[test]
    fn exact_scope_retains_the_original_display_path() {
        let display = "/repo/foo/./";
        let scope = SessionProjectScope::exact(display).expect("exact scope");

        assert_eq!(scope.display_path(), Some(display));
        assert_eq!(
            scope.normalized_path(),
            normalize_project_path(display).as_deref()
        );
    }

    #[test]
    fn catalog_aggregates_exact_projects_and_unknown_separately_across_pages() {
        let mut rows = (0..205)
            .map(|index| meta(index, Some("/repo/other"), index as i64))
            .collect::<Vec<_>>();
        rows.push(meta(205, Some("/repo/foo/./"), 400));
        rows.push(meta(206, Some("/repo/foo"), 300));
        rows.push(meta(207, None, 500));
        rows.push(meta(208, Some(""), 250));
        let (_temp, reader) = publish_catalog_rows(rows);

        let catalog = aggregate_project_directories(&reader, &|| false).expect("catalog");

        assert_eq!(catalog.projects.len(), 2);
        assert_eq!(catalog.projects[0].display_path, "/repo/foo/./");
        assert_eq!(catalog.projects[0].session_count, 2);
        assert_eq!(catalog.projects[0].latest_at, Some(400));
        assert_eq!(catalog.projects[1].session_count, 205);
        assert_eq!(catalog.unknown.session_count, 2);
        assert_eq!(catalog.unknown.latest_at, Some(500));
    }

    #[test]
    fn catalog_cancellation_stops_between_metadata_rows() {
        let rows = (0..205).map(|index| meta(index, Some("/repo/foo"), index as i64));
        let (_temp, reader) = publish_catalog_rows(rows);
        let checks = AtomicUsize::new(0);
        let cancel = || checks.fetch_add(1, Ordering::AcqRel) >= 25;

        assert!(matches!(
            aggregate_project_directories(&reader, &cancel),
            Err(ManifestError::Cancelled)
        ));
    }

    #[test]
    fn catalog_lookup_finds_exact_projects_without_changing_recent_first_order() {
        let rows = [
            meta(0, Some("/repo/alpha"), 10),
            meta(1, Some("/repo/beta"), 30),
            meta(2, Some("/repo/gamma"), 20),
        ];
        let (_temp, reader) = publish_catalog_rows(rows);

        let catalog = aggregate_project_directories(&reader, &|| false).expect("catalog");

        assert_eq!(
            catalog
                .projects
                .iter()
                .map(|project| project.display_path.as_str())
                .collect::<Vec<_>>(),
            vec!["/repo/beta", "/repo/gamma", "/repo/alpha"]
        );
        let alpha = normalize_project_path("/repo/alpha").expect("normalized alpha");
        let beta = normalize_project_path("/repo/beta").expect("normalized beta");
        assert_eq!(catalog.project_position(&alpha), Some(2));
        assert_eq!(catalog.project_position(&beta), Some(0));
        assert_eq!(catalog.project_position("posix:/repo/missing"), None);
    }

    #[test]
    fn project_picker_path_matching_is_case_insensitive() {
        assert!(project_path_contains_query(
            "/Users/Alice/MyProject",
            "project"
        ));
        assert!(project_path_contains_query(
            "/Users/Alice/MyProject",
            "alice/my"
        ));
        assert!(project_path_contains_query("/工作区/项目甲", "项目"));
        assert!(!project_path_contains_query(
            "/Users/Alice/MyProject",
            "other"
        ));
    }

    #[test]
    fn view_spec_normalizes_query_and_identifies_only_the_raw_base_view() {
        let base = SessionViewSpec::all_projects("   ");
        let filtered = SessionViewSpec::new(
            SessionProjectScope::exact("/repo/foo").expect("scope"),
            "  NeEdLe  ",
        );

        assert!(base.is_base_view());
        assert_eq!(filtered.query, "needle");
        assert!(!filtered.is_base_view());
    }
}
