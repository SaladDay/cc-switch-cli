//! File-content transaction. The caller holds the skill write lock through DB commit.
//!
//! Directories are retained except where a file/directory conversion requires
//! removing an empty directory; those operations are journaled and reversible.
//! Unrelated directories are never replaced or removed. This is not an
//! interprocess lock: callers must exclude simultaneous writers for race-free use.
//! Content checks detect observed external edits, but cannot provide filesystem CAS.
//! Staging is deliberately retained after apply until explicit commit; Drop never
//! rolls back (including during unwinding). The immutable journal.json manifest,
//! append-only progress.jsonl, and backup/next are the recovery evidence.
//! Replay complete newline-terminated events over the manifest's prepared state:
//! intent sets pending, mutated adds to touched and clears pending, phase changes
//! the phase. A trailing incomplete record is ignored; a pending intent requires
//! comparing live bytes with backup/next and permission bits with the manifest,
//! not assuming mutation occurred. Rollback remains idempotent using those checks;
//! its terminal phase is recorded only after the whole before-state is verified.
//! Windows case-only renames and case-ambiguous file/directory names are rejected
//! during prepare, even on case-sensitive volumes; directory renames are unsupported.
//! File permissions mean Unix mode bits or Windows readonly, not ACLs/ownership.
//! Conversion journals also contain directory inventories and directory intent/
//! mutated events. Replay these in order with file events; undo in reverse order.

use crate::error::AppError;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, Metadata};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

type Files = BTreeMap<PathBuf, Vec<u8>>;

fn permission_bits(permissions: &fs::Permissions) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.mode() & 0o7777
    }
    #[cfg(not(unix))]
    {
        u32::from(permissions.readonly())
    }
}

fn same_permissions(a: Option<&fs::Permissions>, b: Option<&fs::Permissions>) -> bool {
    a.map(permission_bits) == b.map(permission_bits)
}

// Use Windows ordinal case comparison, rather than Unicode lowercase (which does
// not match Windows path comparison). Sorting keeps preflight O(n log n).
#[cfg(windows)]
fn reject_case_ambiguities(dest: &Path, old: &Inventory, next: &Inventory) -> Result<(), AppError> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn CompareStringOrdinal(
            a: *const u16,
            a_len: i32,
            b: *const u16,
            b_len: i32,
            ignore_case: i32,
        ) -> i32;
    }
    let mut paths: Vec<_> = old
        .files
        .keys()
        .chain(old.dirs.iter())
        .chain(next.files.keys())
        .chain(next.dirs.iter())
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| (p, p.as_os_str().encode_wide().collect::<Vec<_>>()))
        .collect();
    let mut comparison_failed = false;
    paths.sort_by(|a, b| {
        // Filesystem path lengths fit i32; buffers remain alive for this call.
        let result = unsafe {
            CompareStringOrdinal(
                a.1.as_ptr(),
                a.1.len() as i32,
                b.1.as_ptr(),
                b.1.len() as i32,
                1,
            )
        };
        if result == 0 {
            comparison_failed = true;
        }
        result.cmp(&2)
    });
    if comparison_failed {
        return Err(error(dest, "Windows case comparison failed"));
    }
    for pair in paths.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let result = unsafe {
            CompareStringOrdinal(
                a.1.as_ptr(),
                a.1.len() as i32,
                b.1.as_ptr(),
                b.1.len() as i32,
                1,
            )
        };
        if result == 0 {
            return Err(error(dest, "Windows case comparison failed"));
        }
        if result == 2 && a.0 != b.0 {
            return Err(error(
                &dest.join(a.0),
                format!(
                    "case-only rename or name/type ambiguity with {}; rejected before mutation",
                    dest.join(b.0).display()
                ),
            ));
        }
    }
    Ok(())
}

#[derive(Default)]
struct Inventory {
    runtime_files: BTreeSet<PathBuf>,
    files: Files,
    permissions: BTreeMap<PathBuf, fs::Permissions>,
    dirs: BTreeSet<PathBuf>,
    dir_permissions: BTreeMap<PathBuf, fs::Permissions>,
}

#[derive(Clone)]
enum Mutation {
    File(PathBuf),
    RemoveDir(PathBuf),
    CreateDir(PathBuf),
}

impl Mutation {
    fn path(&self) -> &Path {
        match self {
            Self::File(path) | Self::RemoveDir(path) | Self::CreateDir(path) => path,
        }
    }

    fn events(&self) -> (&'static str, &'static str) {
        match self {
            Self::File(_) => ("intent", "mutated"),
            Self::RemoveDir(_) => ("remove_dir_intent", "remove_dir_mutated"),
            Self::CreateDir(_) => ("create_dir_intent", "create_dir_mutated"),
        }
    }
}

pub(super) struct Update {
    runtime: super::runtime_files::RuntimeFiles,
    dest: PathBuf,
    stage: PathBuf,
    before: Files,
    after: Files,
    before_permissions: BTreeMap<PathBuf, fs::Permissions>,
    after_permissions: BTreeMap<PathBuf, fs::Permissions>,
    dirs: BTreeSet<PathBuf>,
    before_dirs: BTreeSet<PathBuf>,
    removed_dirs: BTreeMap<PathBuf, fs::Permissions>,
    conversion_dirs: BTreeSet<PathBuf>,
    operations: Vec<Mutation>,
    touched_operations: Vec<Mutation>,
    changes: Vec<PathBuf>,
    // Only successfully mutated files are eligible for automatic rollback.
    touched: Vec<PathBuf>,
    // Never append after a failed write/sync: its tail may be an incomplete record.
    journal_failed: bool,
    started: bool,
    applied: bool,
    rolled_back: bool,
    #[cfg(test)]
    fail_apply: Option<usize>,
    #[cfg(test)]
    fail_rollback: bool,
    #[cfg(test)]
    journal_bytes_written: usize,
    #[cfg(test)]
    interrupt_journal: Option<(&'static str, bool)>,
}

fn error(path: &Path, message: impl std::fmt::Display) -> AppError {
    AppError::Message(format!("{}: {message}", path.display()))
}

fn metadata(path: &Path) -> Result<Option<Metadata>, AppError> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            #[cfg(windows)]
            let reparse = {
                use std::os::windows::fs::MetadataExt;
                meta.file_attributes() & 0x400 != 0
            };
            #[cfg(not(windows))]
            let reparse = false;
            if meta.file_type().is_symlink() || reparse {
                return Err(error(path, "symlink/reparse node rejected"));
            }
            if !meta.is_file() && !meta.is_dir() {
                return Err(error(path, "non-regular filesystem node rejected"));
            }
            Ok(Some(meta))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(AppError::io(path, e)),
    }
}

// Check ancestors too: rejecting a link only at the leaf would still traverse it.
fn safe_path(path: &Path) -> Result<(), AppError> {
    let mut ancestors: Vec<_> = path.ancestors().collect();
    ancestors.reverse();
    for ancestor in ancestors {
        if let Some(meta) = metadata(ancestor)? {
            if ancestor != path && !meta.is_dir() {
                return Err(error(ancestor, "file/directory collision"));
            }
        }
    }
    Ok(())
}

fn absolute(path: &Path) -> Result<PathBuf, AppError> {
    let path = managed_path(path)?;
    safe_path(&path)?;
    Ok(path)
}

// Configuration parents may be aliases (e.g. a mounted home). Resolve only
// above the declared config root; links inside managed skill trees still fail.
pub(super) fn managed_path(path: &Path) -> Result<PathBuf, AppError> {
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(error(path, "parent traversal is not allowed"));
    }
    let path = std::path::absolute(path).map_err(|e| AppError::io(path, e))?;
    let mut roots = vec![crate::config::get_app_config_dir()];
    if let Some(home) = crate::config::home_dir() {
        roots.push(home.join(".agents"));
    }
    for app in super::SkillService::supported_skill_apps() {
        if let Ok(skills) = super::SkillService::get_app_skills_dir(&app) {
            if let Some(root) = skills.parent() {
                roots.push(root.to_path_buf());
            }
        }
    }
    roots.sort_by_key(|root| std::cmp::Reverse(root.components().count()));
    for root in roots {
        let root = std::path::absolute(&root).map_err(|e| AppError::io(&root, e))?;
        if let Ok(relative) = path.strip_prefix(&root) {
            return Ok(crate::config::resolve_config_dir(&root)?.join(relative));
        }
    }
    Ok(path)
}

fn inventory(root: &Path) -> Result<Inventory, AppError> {
    inventory_with_runtime(root, &super::runtime_files::RuntimeFiles::none())
}

fn inventory_with_runtime(
    root: &Path,
    runtime: &super::runtime_files::RuntimeFiles,
) -> Result<Inventory, AppError> {
    safe_path(root)?;
    let mut result = Inventory::default();
    fn walk(
        root: &Path,
        path: &Path,
        result: &mut Inventory,
        runtime: &super::runtime_files::RuntimeFiles,
    ) -> Result<(), AppError> {
        let Some(meta) = metadata(path)? else {
            return Err(error(path, "disappeared during inventory"));
        };
        let relative = path.strip_prefix(root).expect("descendant").to_path_buf();
        if runtime.contains(&relative) {
            if !meta.is_file() {
                return Err(error(path, "runtime path must be a regular file"));
            }
            result.runtime_files.insert(relative);
            return Ok(());
        }
        if meta.is_dir() {
            result
                .dir_permissions
                .insert(relative.clone(), meta.permissions());
            result.dirs.insert(relative);
            for entry in fs::read_dir(path).map_err(|e| AppError::io(path, e))? {
                let entry = entry.map_err(|e| AppError::io(path, e))?;
                walk(root, &entry.path(), result, runtime)?;
            }
        } else {
            result
                .permissions
                .insert(relative.clone(), meta.permissions());
            result
                .files
                .insert(relative, fs::read(path).map_err(|e| AppError::io(path, e))?);
        }
        Ok(())
    }
    if let Some(meta) = metadata(root)? {
        if !meta.is_dir() {
            return Err(error(root, "expected directory"));
        }
        walk(root, root, &mut result, runtime)?;
    }
    Ok(result)
}

// Unregistered first-install targets may only reuse directories left by rollback.
// Inventory rejects files at the root, links/reparse nodes and special nodes too.
pub(super) fn require_file_empty_tree(root: &Path) -> Result<(), AppError> {
    if !inventory(root)?.files.is_empty() {
        return Err(error(root, "unregistered destination contains files"));
    }
    Ok(())
}

fn verify(root: &Path, expected: &Files) -> Result<(), AppError> {
    let actual = inventory(root)?.files;
    for relative in expected.keys().chain(actual.keys()) {
        if expected.get(relative) != actual.get(relative) {
            return Err(error(&root.join(relative), "content inventory conflict"));
        }
    }
    Ok(())
}

fn verify_state(
    root: &Path,
    files: &Files,
    permissions: &BTreeMap<PathBuf, fs::Permissions>,
) -> Result<(), AppError> {
    verify_state_with_runtime(
        root,
        files,
        permissions,
        &super::runtime_files::RuntimeFiles::none(),
    )
}

fn verify_state_with_runtime(
    root: &Path,
    files: &Files,
    permissions: &BTreeMap<PathBuf, fs::Permissions>,
    runtime: &super::runtime_files::RuntimeFiles,
) -> Result<(), AppError> {
    if inventory_with_runtime(root, runtime)?.files != *files {
        return Err(error(root, "content inventory conflict"));
    }
    for (relative, expected) in permissions {
        let path = root.join(relative);
        let actual = metadata(&path)?.map(|m| m.permissions());
        if !same_permissions(actual.as_ref(), Some(expected)) {
            return Err(error(&path, "permission inventory conflict"));
        }
    }
    Ok(())
}

fn matches_state(
    path: &Path,
    bytes: Option<&Vec<u8>>,
    permissions: Option<&fs::Permissions>,
) -> Result<bool, AppError> {
    let actual = read_optional(path)?;
    let actual_permissions = reachable_metadata(path)?
        .filter(|m| m.is_file())
        .map(|m| m.permissions());
    Ok(actual.as_ref() == bytes && same_permissions(actual_permissions.as_ref(), permissions))
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, AppError> {
    match reachable_metadata(path)? {
        None => Ok(None),
        Some(meta) if meta.is_file() => fs::read(path).map(Some).map_err(|e| AppError::io(path, e)),
        Some(_) => Ok(None),
    }
}

// A file ancestor means this descendant has no file/dir node. This matters when
// retrying rollback after its conversion root has already been restored to a file.
// Inspect every reachable ancestor without following links or reparse points.
fn reachable_metadata(path: &Path) -> Result<Option<Metadata>, AppError> {
    let mut ancestors: Vec<_> = path.ancestors().collect();
    ancestors.reverse();
    for ancestor in ancestors {
        match metadata(ancestor)? {
            Some(meta) if ancestor == path => return Ok(Some(meta)),
            Some(meta) if meta.is_dir() => {}
            _ => return Ok(None),
        }
    }
    Ok(None)
}

fn ensure_dir(path: &Path) -> Result<(), AppError> {
    safe_path(path)?;
    fs::create_dir_all(path).map_err(|e| AppError::io(path, e))?;
    safe_path(path)
}

// The tempfile lives in the target parent (same volume). persist overwrites files
// atomically on Windows as well as Unix; no unlink-before-rename gap is introduced.
fn replace(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    replace_with_permissions(path, bytes, None)
}

fn replace_with_permissions(
    path: &Path,
    bytes: &[u8],
    permissions: Option<&fs::Permissions>,
) -> Result<(), AppError> {
    safe_path(path)?;
    let parent = path.parent().ok_or_else(|| error(path, "missing parent"))?;
    ensure_dir(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| AppError::io(parent, e))?;
    temp.write_all(bytes)
        .map_err(|e| AppError::io(temp.path(), e))?;
    if let Some(permissions) = permissions {
        temp.as_file()
            .set_permissions(permissions.clone())
            .map_err(|e| AppError::io(temp.path(), e))?;
    }
    temp.as_file()
        .sync_all()
        .map_err(|e| AppError::io(temp.path(), e))?;
    temp.persist(path)
        .map_err(|e| AppError::io(path, e.error))?;
    Ok(())
}

impl Update {
    pub(super) fn prepare(source: &Path, dest: &Path) -> Result<Self, AppError> {
        Self::prepare_with_runtime(source, dest, &super::runtime_files::RuntimeFiles::none())
    }

    pub(super) fn prepare_with_runtime(
        source: &Path,
        dest: &Path,
        runtime: &super::runtime_files::RuntimeFiles,
    ) -> Result<Self, AppError> {
        let source = absolute(source)?;
        let dest = absolute(dest)?;
        if metadata(&source)?.is_none() {
            return Err(error(&source, "source does not exist"));
        }
        // Canonicalize existing ancestors to catch Windows case aliases and overlap.
        fn resolved(path: &Path) -> Result<PathBuf, AppError> {
            if metadata(path)?.is_some() {
                return fs::canonicalize(path).map_err(|e| AppError::io(path, e));
            }
            let parent = path.parent().ok_or_else(|| error(path, "missing parent"))?;
            Ok(resolved(parent)?.join(
                path.file_name()
                    .ok_or_else(|| error(path, "missing name"))?,
            ))
        }
        let source = resolved(&source)?;
        let dest = resolved(&dest)?;
        if source.starts_with(&dest) || dest.starts_with(&source) {
            return Err(error(
                &dest,
                format!("overlaps source {}", source.display()),
            ));
        }
        runtime.validate_source(&source)?;
        let next = inventory_with_runtime(&source, runtime)?;
        let old = inventory_with_runtime(&dest, runtime)?;
        for path in &old.runtime_files {
            if path.ancestors().any(|p| next.files.contains_key(p)) {
                return Err(error(
                    &dest.join(path),
                    "runtime file/source conversion conflict",
                ));
            }
        }
        #[cfg(windows)]
        reject_case_ambiguities(&dest, &old, &next)?;
        #[cfg(windows)]
        for runtime_path in &old.runtime_files {
            if let Some(source_path) = next.files.keys().find(|source_path| {
                runtime_path.components().count() == source_path.components().count()
                    && super::runtime_files::RuntimeFiles::paths_overlap(runtime_path, source_path)
            }) {
                return Err(error(
                    &dest.join(runtime_path),
                    format!(
                        "runtime file/source case conflict with {}",
                        source_path.display()
                    ),
                ));
            }
        }
        let removed_dirs: BTreeMap<_, _> = old
            .dir_permissions
            .iter()
            .filter(|(dir, _)| dir.ancestors().any(|p| next.files.contains_key(p)))
            .map(|(dir, permissions)| (dir.clone(), permissions.clone()))
            .collect();
        let conversion_dirs: BTreeSet<_> = next
            .dirs
            .iter()
            .filter(|dir| dir.ancestors().any(|p| old.files.contains_key(p)))
            .cloned()
            .collect();
        let changes: Vec<PathBuf> = old
            .files
            .keys()
            .chain(next.files.keys())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|p| {
                old.files.get(p) != next.files.get(p)
                    || !same_permissions(old.permissions.get(p), next.permissions.get(p))
            })
            .collect();
        let mut operations: Vec<_> = changes.iter().cloned().map(Mutation::File).collect();
        if !removed_dirs.is_empty() || !conversion_dirs.is_empty() {
            // Delete old files first, then empty conversion directories deepest
            // first. Only then can new directories and new files occupy them.
            operations = changes
                .iter()
                .filter(|p| !next.files.contains_key(*p))
                .cloned()
                .map(Mutation::File)
                .collect();
            let mut removals: Vec<_> = removed_dirs.keys().cloned().collect();
            removals.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
            operations.extend(removals.into_iter().map(Mutation::RemoveDir));
            operations.extend(conversion_dirs.iter().cloned().map(Mutation::CreateDir));
            operations.extend(
                changes
                    .iter()
                    .filter(|p| next.files.contains_key(*p))
                    .cloned()
                    .map(Mutation::File),
            );
        }
        // Windows cannot atomically overwrite/remove a readonly file. Refuse
        // such mutations before starting, rather than create an update that
        // cannot be undone after a later file or database failure.
        #[cfg(windows)]
        for relative in &changes {
            if old.permissions.get(relative).is_some_and(|p| p.readonly())
                || next.permissions.get(relative).is_some_and(|p| p.readonly())
            {
                return Err(error(
                    &dest.join(relative),
                    "readonly file mutation rejected before update",
                ));
            }
        }
        let parent = dest
            .parent()
            .ok_or_else(|| error(&dest, "missing parent"))?;
        let staging_parent = parent
            .ancestors()
            .find(|p| p.is_dir())
            .ok_or_else(|| error(parent, "no existing staging parent"))?;
        let stage = tempfile::Builder::new()
            .prefix(".skill-in-place-")
            .tempdir_in(staging_parent)
            .map_err(|e| AppError::io(staging_parent, e))?;
        // Disable automatic deletion immediately. Even preparation errors report evidence.
        let stage = stage.keep();
        let mut transaction = Self {
            runtime: runtime.clone(),
            dest,
            stage,
            before: old.files,
            after: next.files,
            dirs: next.dirs,
            before_dirs: old.dirs,
            removed_dirs,
            conversion_dirs,
            operations,
            touched_operations: Vec::new(),
            before_permissions: old.permissions,
            after_permissions: next.permissions,
            changes,
            touched: Vec::new(),
            journal_failed: false,
            started: false,
            applied: false,
            rolled_back: false,
            #[cfg(test)]
            fail_apply: None,
            #[cfg(test)]
            fail_rollback: false,
            #[cfg(test)]
            journal_bytes_written: 0,
            #[cfg(test)]
            interrupt_journal: None,
        };
        let prepared = (|| {
            for (name, files) in [
                ("backup", &transaction.before),
                ("next", &transaction.after),
            ] {
                let root = transaction.stage.join(name);
                ensure_dir(&root)?;
                for (relative, bytes) in files {
                    replace(&root.join(relative), bytes)?;
                }
                verify(&root, files)?;
            }
            transaction.write_manifest()
        })();
        if let Err(e) = prepared {
            // A preparation failure must not erase the path advertised in the error.
            let mut transaction = transaction;
            transaction.started = true;
            return Err(transaction.context(e));
        }
        Ok(transaction)
    }

    fn context(&self, e: AppError) -> AppError {
        error(
            &self.dest,
            format!("{e}; preserved staging: {}", self.stage.display()),
        )
    }

    fn write_manifest(&mut self) -> Result<(), AppError> {
        let entries: Vec<_> = self
            .changes
            .iter()
            .map(|p| {
                serde_json::json!({
                    "path": p,
                    "operation": match (self.before.contains_key(p), self.after.contains_key(p)) {
                        (true, true) => "changed", (true, false) => "deleted", _ => "new"
                    },
                    "before_permissions": self.before_permissions.get(p).map(permission_bits),
                    "after_permissions": self.after_permissions.get(p).map(permission_bits)
                })
            })
            .collect();
        let data = serde_json::to_vec_pretty(&serde_json::json!({
            "version": 2, "destination": self.dest, "phase": "prepared",
            "runtime_files": self.runtime,
            "automatic_runtime_files": "__pycache__/*.pyc",
            "progress_log": "progress.jsonl", "files": entries,
            "before_directories": self.before_dirs,
            "created_conversion_directories": self.conversion_dirs,
            "removed_directories": self.removed_dirs.iter().map(|(path, permissions)|
                serde_json::json!({"path": path, "permissions": permission_bits(permissions)})
            ).collect::<Vec<_>>()
        }))
        .map_err(|e| error(&self.stage.join("journal.json"), e))?;
        replace(&self.stage.join("journal.json"), &data)?;
        #[cfg(test)]
        {
            self.journal_bytes_written += data.len();
        }
        replace(&self.stage.join("progress.jsonl"), b"")
    }

    fn journal(
        &mut self,
        phase: &str,
        event: &str,
        relative: Option<&Path>,
    ) -> Result<(), AppError> {
        let path = self.stage.join("progress.jsonl");
        if self.journal_failed {
            return Err(error(
                &path,
                "previous progress write failed; refusing to append to uncertain tail",
            ));
        }
        #[cfg(test)]
        if self
            .interrupt_journal
            .is_some_and(|(name, after)| name == event && !after)
        {
            panic!("interrupted before {event} record");
        }
        // Each event is bounded by one path, never by the total change/touched set.
        // Sync the intent before touching its live file. A missing completion means
        // the manifest and actual before/after state must resolve the pending intent.
        let mut data = serde_json::to_vec(&serde_json::json!({
            "phase": phase, "event": event, "path": relative
        }))
        .map_err(|e| error(&path, e))?;
        data.push(b'\n');
        let result = (|| {
            safe_path(&path)?;
            // Preparation creates the file; do not silently recreate lost evidence.
            let mut file = fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .map_err(|e| AppError::io(&path, e))?;
            file.write_all(&data).map_err(|e| AppError::io(&path, e))?;
            file.sync_all().map_err(|e| AppError::io(&path, e))
        })();
        if result.is_err() {
            self.journal_failed = true;
        }
        #[cfg(test)]
        if result.is_ok() {
            self.journal_bytes_written += data.len();
            if self
                .interrupt_journal
                .is_some_and(|(name, after)| name == event && after)
            {
                panic!("interrupted after {event} record");
            }
        }
        result
    }

    pub(super) fn apply(&mut self) -> Result<(), AppError> {
        if self.started {
            return Err(self.context(error(&self.dest, "transaction already started")));
        }
        self.started = true;
        let result = self.apply_inner();
        if let Err(cause) = result {
            let rollback = self.rollback();
            return Err(self.context(error(
                &self.dest,
                match rollback {
                    Ok(()) => format!("apply failed: {cause}; rollback verified"),
                    Err(e) => format!("apply failed: {cause}; rollback failed: {e}"),
                },
            )));
        }
        self.applied = true;
        Ok(())
    }

    fn apply_inner(&mut self) -> Result<(), AppError> {
        verify_state_with_runtime(
            &self.dest,
            &self.before,
            &self.before_permissions,
            &self.runtime,
        )?;
        // Empty directory edits can also invalidate a planned conversion.
        if inventory_with_runtime(&self.dest, &self.runtime)?.dirs != self.before_dirs {
            return Err(error(&self.dest, "directory inventory conflict"));
        }
        self.journal("applying", "phase", None)?;
        for relative in &self.dirs {
            if !self.conversion_dirs.contains(relative) {
                ensure_dir(&self.dest.join(relative))?;
            }
        }
        for operation in self.operations.clone() {
            let relative = operation.path().to_path_buf();
            let path = self.dest.join(&relative);
            let (intent, mutated) = operation.events();
            self.journal("applying", intent, Some(&relative))?;
            #[cfg(test)]
            if self.fail_apply == Some(self.touched_operations.len() + 1) {
                return Err(error(&path, "injected apply failure"));
            }
            match &operation {
                Mutation::File(_) => {
                    if !matches_state(
                        &path,
                        self.before.get(&relative),
                        self.before_permissions.get(&relative),
                    )? {
                        return Err(error(&path, "external change before apply"));
                    }
                    if let Some(bytes) = self.after.get(&relative) {
                        replace_with_permissions(
                            &path,
                            bytes,
                            self.after_permissions.get(&relative),
                        )?;
                    } else {
                        fs::remove_file(&path).map_err(|e| AppError::io(&path, e))?;
                    }
                    self.touched.push(relative.clone());
                }
                Mutation::RemoveDir(_) => {
                    let meta = reachable_metadata(&path)?;
                    if !meta.as_ref().is_some_and(|m| m.is_dir())
                        || !same_permissions(
                            meta.as_ref().map(|m| m.permissions()).as_ref(),
                            self.removed_dirs.get(&relative),
                        )
                    {
                        return Err(error(&path, "directory changed before conversion"));
                    }
                    // Never recursively remove: a concurrent addition must survive.
                    fs::remove_dir(&path).map_err(|e| AppError::io(&path, e))?;
                }
                Mutation::CreateDir(_) => {
                    if reachable_metadata(&path)?.is_some() {
                        return Err(error(
                            &path,
                            "external directory collision before conversion",
                        ));
                    }
                    safe_path(&path)?;
                    fs::create_dir(&path).map_err(|e| AppError::io(&path, e))?;
                }
            }
            self.touched_operations.push(operation);
            self.journal("applying", mutated, Some(&relative))?;
        }
        verify_state_with_runtime(
            &self.dest,
            &self.after,
            &self.after_permissions,
            &self.runtime,
        )?;
        self.journal("applied", "phase", None)
    }

    pub(super) fn rollback(&mut self) -> Result<(), AppError> {
        let result = self.rollback_inner();
        result.map_err(|e| self.context(e))
    }

    fn rollback_inner(&mut self) -> Result<(), AppError> {
        let mut failures = Vec::new();
        for operation in self.touched_operations.iter().rev() {
            let relative = operation.path();
            let path = self.dest.join(relative);
            let restore = (|| {
                match operation {
                    Mutation::CreateDir(_) => {
                        let Some(meta) = reachable_metadata(&path)? else {
                            return Ok(());
                        };
                        if meta.is_file()
                            && matches_state(
                                &path,
                                self.before.get(relative),
                                self.before_permissions.get(relative),
                            )?
                        {
                            return Ok(());
                        }
                        #[cfg(test)]
                        if self.fail_rollback {
                            return Err(error(&path, "injected rollback failure"));
                        }
                        if !meta.is_dir() {
                            return Err(error(&path, "external node blocks directory rollback"));
                        }
                        // Nonempty means an external file, or an earlier failed
                        // file rollback. Preserve it and retain recovery evidence.
                        return fs::remove_dir(&path).map_err(|e| AppError::io(&path, e));
                    }
                    Mutation::RemoveDir(_) => {
                        if let Some(meta) = reachable_metadata(&path)? {
                            if meta.is_dir()
                                && same_permissions(
                                    Some(&meta.permissions()),
                                    self.removed_dirs.get(relative),
                                )
                            {
                                return Ok(());
                            }
                            return Err(error(&path, "external node blocks directory restore"));
                        }
                        #[cfg(test)]
                        if self.fail_rollback {
                            return Err(error(&path, "injected rollback failure"));
                        }
                        safe_path(&path)?;
                        fs::create_dir(&path).map_err(|e| AppError::io(&path, e))?;
                        return fs::set_permissions(&path, self.removed_dirs[relative].clone())
                            .map_err(|e| AppError::io(&path, e));
                    }
                    Mutation::File(_) => {}
                }
                if matches_state(
                    &path,
                    self.before.get(relative),
                    self.before_permissions.get(relative),
                )? {
                    return Ok(());
                }
                if !matches_state(
                    &path,
                    self.after.get(relative),
                    self.after_permissions.get(relative),
                )? {
                    return Err(error(
                        &path,
                        "external change; refusing to overwrite during rollback",
                    ));
                }
                #[cfg(test)]
                if self.fail_rollback {
                    return Err(error(&path, "injected rollback failure"));
                }
                if let Some(bytes) = self.before.get(relative) {
                    replace_with_permissions(&path, bytes, self.before_permissions.get(relative))
                } else {
                    fs::remove_file(&path).map_err(|e| AppError::io(&path, e))
                }
            })();
            if let Err(e) = restore {
                failures.push(e.to_string());
            }
        }
        if let Err(e) = verify_state_with_runtime(
            &self.dest,
            &self.before,
            &self.before_permissions,
            &self.runtime,
        ) {
            failures.push(e.to_string());
        }
        for (relative, permissions) in &self.removed_dirs {
            let path = self.dest.join(relative);
            match reachable_metadata(&path) {
                Ok(Some(meta))
                    if meta.is_dir()
                        && same_permissions(Some(&meta.permissions()), Some(permissions)) => {}
                _ => failures.push(format!("{}: directory rollback conflict", path.display())),
            }
        }
        if failures.is_empty() {
            self.journal("rolled_back", "phase", None)?;
            self.rolled_back = true;
            self.applied = false;
            Ok(())
        } else {
            Err(error(&self.dest, failures.join("; ")))
        }
    }

    /// Call only after the database commits, or after a verified rollback to discard
    /// its recovery evidence. A cleanup error is not a request to roll back the DB.
    pub(super) fn commit(mut self) -> Result<(), AppError> {
        if self.started && !self.applied && !self.rolled_back {
            return Err(self.context(error(&self.dest, "unresolved transaction; cleanup refused")));
        }
        #[cfg(test)]
        if FAIL_CLEANUP_FOR.with(|target| {
            target.borrow().as_ref().is_some_and(|target| {
                fs::canonicalize(target).ok().is_some_and(|target| {
                    fs::canonicalize(&self.dest).ok().as_ref() == Some(&target)
                })
            })
        }) {
            return Err(self.context(error(&self.stage, "injected staging cleanup failure")));
        }
        fs::remove_dir_all(&self.stage).map_err(|e| self.context(AppError::io(&self.stage, e)))?;
        self.started = true; // Drop has nothing left to clean.
        Ok(())
    }
}

#[cfg(test)]
thread_local! {
    static FAIL_CLEANUP_FOR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(super) fn with_cleanup_failure<T>(dest: &Path, run: impl FnOnce() -> T) -> T {
    struct Reset(Option<PathBuf>);
    impl Drop for Reset {
        fn drop(&mut self) {
            FAIL_CLEANUP_FOR.with(|target| *target.borrow_mut() = self.0.take());
        }
    }
    let _reset = Reset(FAIL_CLEANUP_FOR.with(|target| target.replace(Some(dest.to_path_buf()))));
    run()
}

impl Drop for Update {
    fn drop(&mut self) {
        if !self.started {
            let _ = fs::remove_dir_all(&self.stage);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn runtime_files_are_not_staged_and_survive_apply_and_rollback() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let source = home.path().join("input");
        let dest = home.path().join("output");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(dest.join("scripts/__pycache__")).unwrap();
        fs::write(source.join("SKILL.md"), "new").unwrap();
        fs::write(dest.join("SKILL.md"), "old").unwrap();
        fs::write(dest.join("local.json"), "private").unwrap();
        fs::write(dest.join("scripts/__pycache__/m.pyc"), "cache").unwrap();
        let runtime =
            super::super::runtime_files::RuntimeFiles::new(&["local.json".into()]).unwrap();
        let mut update = Update::prepare_with_runtime(&source, &dest, &runtime).unwrap();
        for staging in ["backup", "next"] {
            assert!(!update.stage.join(staging).join("local.json").exists());
            assert!(!update
                .stage
                .join(staging)
                .join("scripts/__pycache__/m.pyc")
                .exists());
        }
        update.apply().unwrap();
        update.rollback().unwrap();
        update.commit().unwrap();
        assert_eq!(fs::read(dest.join("SKILL.md")).unwrap(), b"old");
        assert_eq!(fs::read(dest.join("local.json")).unwrap(), b"private");
        assert_eq!(
            fs::read(dest.join("scripts/__pycache__/m.pyc")).unwrap(),
            b"cache"
        );
        fs::write(source.join("scripts"), "cannot replace cache ancestor").unwrap();
        assert!(Update::prepare_with_runtime(&source, &dest, &runtime).is_err());
    }
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let dest = temp.path().join("dest");
        fs::create_dir_all(source.join("sub")).unwrap();
        fs::create_dir_all(dest.join("sub")).unwrap();
        for (root, path, data) in [
            (&source, "a", "new"),
            (&source, "sub/add", "add"),
            (&source, "same", "same"),
            (&dest, "a", "old"),
            (&dest, "sub/delete", "delete"),
            (&dest, "same", "same"),
        ] {
            fs::write(root.join(path), data).unwrap();
        }
        (temp, source, dest)
    }

    // Read only persisted evidence, not the Update's in-memory touched vector.
    // An incomplete final line cannot authorize a mutation or clear an intent.
    fn recovery_state(stage: &Path) -> (String, Option<PathBuf>, BTreeSet<PathBuf>) {
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(stage.join("journal.json")).unwrap()).unwrap();
        assert_eq!(manifest["version"], 2);
        assert_eq!(manifest["progress_log"], "progress.jsonl");
        let mut phase = manifest["phase"].as_str().unwrap().to_string();
        let mut pending = None;
        let mut touched = BTreeSet::new();
        let progress = fs::read(stage.join("progress.jsonl")).unwrap();
        for line in progress.split_inclusive(|byte| *byte == b'\n') {
            if !line.ends_with(b"\n") {
                break;
            }
            let record: serde_json::Value = serde_json::from_slice(line).unwrap();
            phase = record["phase"].as_str().unwrap().to_string();
            match record["event"].as_str().unwrap() {
                "intent" => {
                    pending = Some(serde_json::from_value(record["path"].clone()).unwrap());
                }
                "mutated" => {
                    let path: PathBuf = serde_json::from_value(record["path"].clone()).unwrap();
                    assert_eq!(pending.as_ref(), Some(&path));
                    touched.insert(path);
                    pending = None;
                }
                "phase" => assert!(record["path"].is_null()),
                other => panic!("unknown progress event: {other}"),
            }
        }
        (phase, pending, touched)
    }

    #[test]
    fn in_place_journal_total_writes_grow_linearly() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        fn exercise(count: usize) -> usize {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("source");
            let dest = temp.path().join("dest");
            fs::create_dir(&source).unwrap();
            fs::create_dir(&dest).unwrap();
            for index in 0..count {
                let name = format!("file-{index:05}");
                fs::write(source.join(&name), "new").unwrap();
                fs::write(dest.join(&name), "old").unwrap();
            }
            let mut update = Update::prepare(&source, &dest).unwrap();
            let manifest = fs::read(update.stage.join("journal.json")).unwrap();
            assert_eq!(update.journal_bytes_written, manifest.len());
            assert!(fs::read(update.stage.join("progress.jsonl"))
                .unwrap()
                .is_empty());
            assert_eq!(
                recovery_state(&update.stage),
                ("prepared".into(), None, BTreeSet::new())
            );
            update.apply().unwrap();
            let applied_log = fs::read(update.stage.join("progress.jsonl")).unwrap();
            assert_eq!(
                applied_log
                    .split(|b| *b == b'\n')
                    .filter(|line| !line.is_empty())
                    .count(),
                2 * count + 2
            );
            let (phase, pending, touched) = recovery_state(&update.stage);
            assert_eq!(phase, "applied");
            assert!(pending.is_none());
            assert_eq!(
                touched,
                update.changes.iter().cloned().collect::<BTreeSet<_>>()
            );
            assert_eq!(
                update.journal_bytes_written,
                manifest.len() + applied_log.len()
            );
            update.rollback().unwrap();
            let progress = fs::read(update.stage.join("progress.jsonl")).unwrap();
            assert!(
                progress.starts_with(&applied_log),
                "rollback must append, not rewrite"
            );
            assert_eq!(
                fs::read(update.stage.join("journal.json")).unwrap(),
                manifest
            );
            assert_eq!(
                recovery_state(&update.stage),
                ("rolled_back".into(), None, touched)
            );
            verify_state(&dest, &update.before, &update.before_permissions).unwrap();
            // Account for every successful log write, not just final on-disk size:
            // repeatedly rewriting a growing snapshot violates this equality.
            let total = manifest.len() + progress.len();
            assert_eq!(update.journal_bytes_written, total);
            assert!(total <= 1024 + 1024 * count);
            update.commit().unwrap();
            total
        }
        let small = exercise(32);
        let large = exercise(128);
        assert!(large > small);
        assert!(
            large <= 4 * small + 1024,
            "log volume must grow with file count"
        );
    }

    #[test]
    fn in_place_interrupted_progress_retains_recovery_evidence() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        for (event, after, partial_tail) in [
            ("intent", true, false),
            ("mutated", false, false),
            ("mutated", false, true),
            ("mutated", true, false),
        ] {
            let (_temp, source, dest) = fixture();
            let mut update = Update::prepare(&source, &dest).unwrap();
            let stage = update.stage.clone();
            let relative = update.changes[0].clone();
            let before_mode = permission_bits(update.before_permissions.get(&relative).unwrap());
            let after_mode = permission_bits(update.after_permissions.get(&relative).unwrap());
            let manifest = fs::read(stage.join("journal.json")).unwrap();
            update.interrupt_journal = Some((event, after));
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                update.apply().unwrap();
            }));
            assert!(result.is_err());
            if partial_tail {
                let mut log = fs::OpenOptions::new()
                    .append(true)
                    .open(stage.join("progress.jsonl"))
                    .unwrap();
                log.write_all(b"{\"event\":\"mutated\"").unwrap();
                log.sync_all().unwrap();
            }
            assert_eq!(fs::read(stage.join("journal.json")).unwrap(), manifest);
            let (phase, pending, touched) = recovery_state(&stage);
            assert_eq!(phase, "applying");
            if event == "mutated" && after {
                assert!(pending.is_none());
                assert_eq!(touched, BTreeSet::from([relative.clone()]));
            } else {
                assert_eq!(pending, Some(relative.clone()));
                assert!(touched.is_empty());
            }
            // Intent alone does not say whether the live mutation happened.
            let live = fs::read(dest.join(&relative)).unwrap();
            assert_eq!(live, if event == "intent" { b"old" } else { b"new" });
            assert_eq!(
                fs::read(stage.join("backup").join(&relative)).unwrap(),
                b"old"
            );
            assert_eq!(
                fs::read(stage.join("next").join(&relative)).unwrap(),
                b"new"
            );
            let manifest: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
            let entry = manifest["files"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| {
                    serde_json::from_value::<PathBuf>(entry["path"].clone()).unwrap() == relative
                })
                .unwrap();
            assert_eq!(entry["operation"], "changed");
            assert_eq!(entry["before_permissions"], before_mode);
            assert_eq!(entry["after_permissions"], after_mode);
        }
    }

    #[test]
    fn in_place_progress_failure_keeps_evidence_and_refuses_further_appends() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        let (_temp, source, dest) = fixture();
        let mut update = Update::prepare(&source, &dest).unwrap();
        let stage = update.stage.clone();
        let progress = stage.join("progress.jsonl");
        let saved = stage.join("saved-progress.jsonl");
        fs::rename(&progress, &saved).unwrap();
        fs::create_dir(&progress).unwrap();
        assert!(update.apply().is_err());
        assert!(update.touched.is_empty());
        verify_state(&dest, &update.before, &update.before_permissions).unwrap();
        fs::remove_dir(&progress).unwrap();
        fs::rename(&saved, &progress).unwrap();
        // Even if the I/O problem disappears, a possibly torn log cannot be resumed.
        let message = update.rollback().unwrap_err().to_string();
        assert!(message.contains("previous progress write failed"));
        assert!(fs::read(&progress).unwrap().is_empty());
        assert_eq!(
            recovery_state(&stage),
            ("prepared".into(), None, BTreeSet::new())
        );
        assert!(update.commit().is_err());
        assert!(stage.join("journal.json").is_file());
        assert_eq!(fs::read(stage.join("backup/a")).unwrap(), b"old");
        assert_eq!(fs::read(stage.join("next/a")).unwrap(), b"new");
    }

    #[test]
    fn in_place_changes_delete_add_and_rollback() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        let (_temp, source, dest) = fixture();
        let before = inventory(&dest).unwrap().files;
        let same = dest.join("same");
        let timestamp = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        fs::OpenOptions::new()
            .write(true)
            .open(&same)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(timestamp))
            .unwrap();
        let unchanged_mtime = fs::metadata(&same).unwrap().modified().unwrap();
        let mut update = Update::prepare(&source, &dest).unwrap();
        assert_eq!(update.changes.len(), 3);
        update.apply().unwrap();
        assert_eq!(
            fs::metadata(&same).unwrap().modified().unwrap(),
            unchanged_mtime
        );
        verify(&dest, &inventory(&source).unwrap().files).unwrap();
        update.rollback().unwrap();
        assert_eq!(
            fs::metadata(&same).unwrap().modified().unwrap(),
            unchanged_mtime
        );
        verify(&dest, &before).unwrap();
        assert!(dest.join("sub").is_dir());
        let stage = update.stage.clone();
        update.commit().unwrap();
        assert!(!stage.exists());
    }

    #[test]
    fn in_place_noop_and_new_destination() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        let (temp, source, _) = fixture();
        let dest = temp.path().join("new/nested/dest");
        let mut update = Update::prepare(&source, &dest).unwrap();
        assert!(!dest.exists());
        update.apply().unwrap();
        update.commit().unwrap();
        let mut update = Update::prepare(&source, &dest).unwrap();
        assert!(update.changes.is_empty());
        update.apply().unwrap();
        update.commit().unwrap();
    }

    #[test]
    fn in_place_same_path_and_overlap_rejected() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        let (_temp, source, dest) = fixture();
        assert!(Update::prepare(&source, &source).is_err());
        assert!(Update::prepare(&source, &source.join("child")).is_err());
        assert!(Update::prepare(&dest.join("sub"), &dest).is_err());
    }

    #[test]
    fn in_place_external_change_before_apply_is_not_touched() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        let (_temp, source, dest) = fixture();
        let mut update = Update::prepare(&source, &dest).unwrap();
        fs::write(dest.join("same"), "edit").unwrap();
        assert!(update.apply().is_err());
        assert!(update.touched.is_empty());
        assert_eq!(fs::read(dest.join("same")).unwrap(), b"edit");
        assert_eq!(fs::read(dest.join("a")).unwrap(), b"old");
    }

    #[test]
    fn in_place_new_destination_rollback_keeps_directories() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        let (temp, source, _) = fixture();
        let dest = temp.path().join("new");
        let mut update = Update::prepare(&source, &dest).unwrap();
        update.apply().unwrap();
        update.rollback().unwrap();
        assert!(dest.join("sub").is_dir());
        assert!(inventory(&dest).unwrap().files.is_empty());
        update.commit().unwrap();
    }

    #[test]
    fn in_place_config_parent_alias_keeps_internal_link_protection() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        let (temp, source, _) = fixture();
        let real = temp.path().join("real");
        let alias = temp.path().join("alias");
        fs::create_dir_all(real.join(".cc-switch/skills")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        #[cfg(windows)]
        match std::os::windows::fs::symlink_dir(&real, &alias) {
            Ok(()) => {}
            Err(e) if e.raw_os_error() == Some(1314) => return,
            Err(e) => panic!("create parent alias: {e}"),
        }
        let config = alias.join(".cc-switch");
        std::env::set_var("CC_SWITCH_CONFIG_DIR", &config);
        let dest = config.join("skills/example");
        let mut update = Update::prepare(&source, &dest).unwrap();
        update.apply().unwrap();
        update.commit().unwrap();
        assert_eq!(
            fs::read(real.join(".cc-switch/skills/example/a")).unwrap(),
            b"new"
        );
        let internal = config.join("skills/internal");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&source, &internal).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&source, &internal).unwrap();
        assert!(Update::prepare(&source, &internal).is_err());
        assert!(Update::prepare(&source, &internal.join("child")).is_err());
        // A configured root itself cannot be a link either.
        std::env::set_var("CC_SWITCH_CONFIG_DIR", &alias);
        assert!(Update::prepare(&source, &alias.join("skills/example")).is_err());
        // Unrelated ancestor aliases do not gain trust from configuration.
        assert!(Update::prepare(&source, &alias.join("unmanaged/example")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn in_place_symlink_rejected_in_both_trees_and_ancestors() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        use std::os::unix::fs::symlink;
        let (temp, source, dest) = fixture();
        symlink(&dest, source.join("link")).unwrap();
        assert!(Update::prepare(&source, &dest).is_err());
        fs::remove_file(source.join("link")).unwrap();
        symlink(&source, dest.join("link")).unwrap();
        assert!(Update::prepare(&source, &dest).is_err());
        symlink(&source, temp.path().join("alias")).unwrap();
        assert!(Update::prepare(&temp.path().join("alias/sub"), &dest).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn in_place_unix_executable_permissions_and_rollback() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        use std::os::unix::fs::PermissionsExt;
        let (_temp, source, dest) = fixture();
        // Identical bytes must still produce a permission-only transaction entry.
        fs::write(source.join("a"), "old").unwrap();
        fs::set_permissions(source.join("a"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(dest.join("a"), fs::Permissions::from_mode(0o640)).unwrap();
        let mut update = Update::prepare(&source, &dest).unwrap();
        assert!(update.changes.contains(&PathBuf::from("a")));
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(update.stage.join("journal.json")).unwrap()).unwrap();
        let entry = manifest["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["path"] == "a")
            .unwrap();
        assert_eq!(entry["before_permissions"], 0o640);
        assert_eq!(entry["after_permissions"], 0o755);
        update.apply().unwrap();
        assert_eq!(
            fs::metadata(dest.join("a")).unwrap().permissions().mode() & 0o777,
            0o755
        );
        update.rollback().unwrap();
        assert_eq!(
            fs::metadata(dest.join("a")).unwrap().permissions().mode() & 0o777,
            0o640
        );
        update.commit().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn in_place_external_permission_edit_is_preserved() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        use std::os::unix::fs::PermissionsExt;
        let (_temp, source, dest) = fixture();
        fs::set_permissions(source.join("a"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(dest.join("a"), fs::Permissions::from_mode(0o640)).unwrap();
        let mut update = Update::prepare(&source, &dest).unwrap();
        update.apply().unwrap();
        fs::set_permissions(dest.join("a"), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(update.rollback().is_err());
        assert_eq!(
            fs::metadata(dest.join("a")).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[cfg(windows)]
    #[test]
    fn in_place_windows_case_only_names_rejected_before_mutation() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        let (_temp, source, dest) = fixture();
        let before = inventory(&dest).unwrap();
        fs::remove_file(source.join("a")).unwrap();
        fs::write(source.join("A"), "new").unwrap();
        let message = Update::prepare(&source, &dest).err().unwrap().to_string();
        assert!(message.contains("case-only"));
        verify_state(&dest, &before.files, &before.permissions).unwrap();
        // The same rejection must cover file-to-directory case aliases.
        fs::remove_file(source.join("A")).unwrap();
        fs::create_dir(source.join("A")).unwrap();
        assert!(Update::prepare(&source, &dest).is_err());
        verify_state(&dest, &before.files, &before.permissions).unwrap();
        fs::remove_dir(source.join("A")).unwrap();
        fs::rename(source.join("sub"), source.join("temporary-sub")).unwrap();
        fs::rename(source.join("temporary-sub"), source.join("SUB")).unwrap();
        assert!(Update::prepare(&source, &dest).is_err());
        verify_state(&dest, &before.files, &before.permissions).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn in_place_windows_readonly_mutation_rejected_before_update() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        let (_temp, source, dest) = fixture();
        let path = source.join("a");
        let mut readonly = fs::metadata(&path).unwrap().permissions();
        readonly.set_readonly(true);
        fs::set_permissions(&path, readonly).unwrap();
        let before = inventory(&dest).unwrap();
        let result = Update::prepare(&source, &dest);
        let mut writable = fs::metadata(&path).unwrap().permissions();
        writable.set_readonly(false);
        fs::set_permissions(&path, writable).unwrap();
        assert!(result.err().unwrap().to_string().contains("readonly"));
        verify_state(&dest, &before.files, &before.permissions).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn in_place_windows_reparse_rejected() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        use std::os::windows::fs::symlink_dir;
        let (_temp, source, dest) = fixture();
        match symlink_dir(&dest, source.join("link")) {
            Ok(()) => {}
            Err(e) if e.raw_os_error() == Some(1314) => {
                eprintln!("symlink test unavailable: Windows symlink privilege not granted");
                return;
            }
            Err(e) => panic!("create symlink: {e}"),
        }
        assert!(Update::prepare(&source, &dest).is_err());
        fs::remove_dir(source.join("link")).unwrap();
        symlink_dir(&source, dest.join("link")).unwrap();
        assert!(Update::prepare(&source, &dest).is_err());
        fs::remove_dir(dest.join("link")).unwrap();
    }

    #[test]
    fn in_place_injected_failure_rolls_back_only_touched_files() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        let (_temp, source, dest) = fixture();
        let before = inventory(&dest).unwrap().files;
        let mut update = Update::prepare(&source, &dest).unwrap();
        update.fail_apply = Some(2);
        assert!(update
            .apply()
            .unwrap_err()
            .to_string()
            .contains("rollback verified"));
        verify(&dest, &before).unwrap();
        update.commit().unwrap();
    }

    #[test]
    fn in_place_rollback_failure_preserves_evidence_and_can_retry() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        let (_temp, source, dest) = fixture();
        let mut update = Update::prepare(&source, &dest).unwrap();
        update.fail_apply = Some(2);
        update.fail_rollback = true;
        let stage = update.stage.clone();
        let touched = BTreeSet::from([update.changes[0].clone()]);
        let pending = Some(update.changes[1].clone());
        let message = update.apply().unwrap_err().to_string();
        assert!(message.contains("rollback failed"));
        assert!(message.contains(&stage.display().to_string()));
        assert!(stage.join("backup/a").is_file());
        assert_eq!(
            recovery_state(&stage),
            ("applying".into(), pending.clone(), touched.clone())
        );
        update.fail_rollback = false;
        update.rollback().unwrap();
        // The verified terminal phase supersedes the pending failed operation,
        // while retaining its evidence, just as the old snapshot journal did.
        assert_eq!(
            recovery_state(&stage),
            ("rolled_back".into(), pending, touched)
        );
        update.commit().unwrap();
    }

    #[test]
    fn in_place_external_edits_survive_rollback() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        let (_temp, source, dest) = fixture();
        let mut update = Update::prepare(&source, &dest).unwrap();
        update.apply().unwrap();
        fs::write(dest.join("a"), "external").unwrap();
        fs::write(dest.join("unrelated"), "external").unwrap();
        assert!(update.rollback().is_err());
        assert_eq!(fs::read(dest.join("a")).unwrap(), b"external");
        assert_eq!(fs::read(dest.join("unrelated")).unwrap(), b"external");
        assert!(!dest.join("sub/add").exists());
        assert!(dest.join("sub/delete").exists());
        let stage = update.stage.clone();
        let touched = update.changes.iter().cloned().collect::<BTreeSet<_>>();
        drop(update);
        assert!(stage.join("journal.json").exists());
        // A failed rollback must not claim a verified before-state in the log.
        assert_eq!(recovery_state(&stage), ("applied".into(), None, touched));
    }

    #[test]
    fn in_place_drop_and_panic_preserve_evidence() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        let (_temp, source, dest) = fixture();
        let mut update = Update::prepare(&source, &dest).unwrap();
        let stage = update.stage.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            update.apply().unwrap();
            panic!("simulated caller panic before DB commit");
        }));
        assert!(result.is_err());
        assert!(stage.join("backup/a").exists());
        assert!(stage.join("next/a").exists());
        assert!(stage.join("journal.json").exists());
    }

    #[test]
    fn in_place_new_directory_collision_preflight_does_not_mutate() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        let (_temp, source, dest) = fixture();
        let mut update = Update::prepare(&source, &dest).unwrap();
        fs::create_dir(dest.join("sub/add")).unwrap();
        let before = inventory(&dest).unwrap().files;
        assert!(update.apply().is_err());
        verify(&dest, &before).unwrap();
        assert!(dest.join("sub/add").is_dir());
        update.commit().unwrap();
    }

    fn conversion_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let dest = temp.path().join("dest");
        fs::create_dir_all(source.join("to_dir/nested/empty")).unwrap();
        fs::create_dir_all(dest.join("to_file/nested/empty")).unwrap();
        fs::write(source.join("to_dir/nested/new"), b"new child").unwrap();
        fs::write(source.join("to_file"), b"new file").unwrap();
        fs::write(dest.join("to_file/nested/old"), b"old child").unwrap();
        fs::write(dest.join("to_dir"), b"old file").unwrap();
        for root in [&source, &dest] {
            fs::create_dir_all(root.join("unchanged/empty")).unwrap();
            fs::write(root.join("unchanged/stable"), b"stable").unwrap();
        }
        (temp, source, dest)
    }

    fn assert_directory_state(root: &Path, expected: &Inventory) {
        let actual = inventory(root).unwrap();
        assert_eq!(actual.dirs, expected.dirs);
        for (path, permissions) in &expected.dir_permissions {
            assert!(same_permissions(
                actual.dir_permissions.get(path),
                Some(permissions)
            ));
        }
    }

    #[test]
    fn in_place_file_directory_conversions_commit_and_rollback() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        let (_temp, source, dest) = conversion_fixture();
        let before = inventory(&dest).unwrap();
        let unchanged = dest.join("unchanged");
        let file_mtime = fs::metadata(unchanged.join("stable"))
            .unwrap()
            .modified()
            .unwrap();
        let dir_mtime = fs::metadata(&unchanged).unwrap().modified().unwrap();
        let mut update = Update::prepare(&source, &dest).unwrap();
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(update.stage.join("journal.json")).unwrap()).unwrap();
        assert_eq!(manifest["removed_directories"].as_array().unwrap().len(), 3);
        assert_eq!(
            manifest["created_conversion_directories"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        update.apply().unwrap();
        let next = inventory(&source).unwrap();
        verify_state(&dest, &next.files, &next.permissions).unwrap();
        assert_eq!(inventory(&dest).unwrap().dirs, next.dirs);
        update.rollback().unwrap();
        update.rollback().unwrap();
        verify_state(&dest, &before.files, &before.permissions).unwrap();
        assert_directory_state(&dest, &before);
        update.commit().unwrap();
        let mut update = Update::prepare(&source, &dest).unwrap();
        update.apply().unwrap();
        let stage = update.stage.clone();
        update.commit().unwrap();
        assert!(!stage.exists());
        verify_state(&dest, &next.files, &next.permissions).unwrap();
        assert_eq!(
            fs::metadata(unchanged.join("stable"))
                .unwrap()
                .modified()
                .unwrap(),
            file_mtime
        );
        assert_eq!(
            fs::metadata(&unchanged).unwrap().modified().unwrap(),
            dir_mtime
        );
    }

    #[test]
    fn in_place_conversion_failure_at_each_operation_restores_complete_tree() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        let (_temp, source, dest) = conversion_fixture();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                dest.join("to_file/nested/empty"),
                fs::Permissions::from_mode(0o751),
            )
            .unwrap();
            fs::set_permissions(dest.join("to_dir"), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let before = inventory(&dest).unwrap();
        let count = Update::prepare(&source, &dest).unwrap().operations.len();
        for fail in 1..=count {
            let mut update = Update::prepare(&source, &dest).unwrap();
            update.fail_apply = Some(fail);
            let message = update.apply().unwrap_err().to_string();
            assert!(
                message.contains("rollback verified"),
                "operation {fail}: {message}"
            );
            verify_state(&dest, &before.files, &before.permissions).unwrap();
            assert_directory_state(&dest, &before);
            update.commit().unwrap();
        }
    }

    #[test]
    fn in_place_conversion_failed_rollback_retains_evidence_and_retries() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        let (_temp, source, dest) = conversion_fixture();
        let before = inventory(&dest).unwrap();
        let mut update = Update::prepare(&source, &dest).unwrap();
        let stage = update.stage.clone();
        update.apply().unwrap();
        update.fail_rollback = true;
        assert!(update.rollback().is_err());
        assert!(stage.join("backup/to_file/nested/old").is_file());
        let progress = fs::read_to_string(stage.join("progress.jsonl")).unwrap();
        assert!(progress.contains("remove_dir_mutated"));
        assert!(progress.contains("create_dir_mutated"));
        assert!(!progress.contains("rolled_back"));
        update.fail_rollback = false;
        update.rollback().unwrap();
        update.rollback().unwrap();
        verify_state(&dest, &before.files, &before.permissions).unwrap();
        assert_directory_state(&dest, &before);
        update.commit().unwrap();
    }

    #[test]
    fn in_place_conversion_rollback_preserves_external_files() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        let (_temp, source, dest) = conversion_fixture();
        let mut update = Update::prepare(&source, &dest).unwrap();
        let stage = update.stage.clone();
        update.apply().unwrap();
        fs::write(dest.join("to_dir/user-file"), b"external child").unwrap();
        fs::write(dest.join("to_file"), b"external replacement").unwrap();
        assert!(update.rollback().is_err());
        assert_eq!(
            fs::read(dest.join("to_dir/user-file")).unwrap(),
            b"external child"
        );
        assert_eq!(
            fs::read(dest.join("to_file")).unwrap(),
            b"external replacement"
        );
        assert_eq!(fs::read(dest.join("unchanged/stable")).unwrap(), b"stable");
        assert!(stage.join("backup/to_dir").is_file());
        assert!(stage.join("backup/to_file/nested/old").is_file());
        drop(update);
        assert!(stage.join("journal.json").is_file());
    }

    #[test]
    fn in_place_cleanup_failure_preserves_applied_tree_and_evidence() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        let (_temp, source, dest) = fixture();
        let mut update = Update::prepare(&source, &dest).unwrap();
        let stage = update.stage.clone();
        update.apply().unwrap();
        let error = with_cleanup_failure(&dest, || update.commit()).unwrap_err();
        assert!(error
            .to_string()
            .contains("injected staging cleanup failure"));
        verify(&dest, &inventory(&source).unwrap().files).unwrap();
        assert!(stage.join("backup/a").is_file());
        assert_eq!(recovery_state(&stage).0, "applied");
    }

    #[cfg(windows)]
    #[test]
    fn in_place_windows_held_directories_without_share_delete() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        use std::os::windows::fs::OpenOptionsExt;
        let (_temp, source, dest) = fixture();
        let _root = fs::OpenOptions::new()
            .read(true)
            .custom_flags(0x02000000)
            .share_mode(3)
            .open(&dest)
            .unwrap();
        let _sub = fs::OpenOptions::new()
            .read(true)
            .custom_flags(0x02000000)
            .share_mode(3)
            .open(dest.join("sub"))
            .unwrap();
        let mut update = Update::prepare(&source, &dest).unwrap();
        update.apply().unwrap();
        update.rollback().unwrap();
        update.commit().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn in_place_windows_locked_individual_file() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        use std::os::windows::fs::OpenOptionsExt;
        let (_temp, source, dest) = fixture();
        let before = inventory(&dest).unwrap().files;
        let mut update = Update::prepare(&source, &dest).unwrap();
        let _locked = fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(dest.join("sub/delete"))
            .unwrap();
        assert!(update.apply().is_err());
        verify(&dest, &before).unwrap();
        update.commit().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn in_place_windows_locked_rollback_retains_backup() {
        let isolation_root = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(isolation_root.path());
        use std::os::windows::fs::OpenOptionsExt;
        let (_temp, source, dest) = fixture();
        let mut update = Update::prepare(&source, &dest).unwrap();
        update.apply().unwrap();
        let locked = fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(dest.join("a"))
            .unwrap();
        let stage = update.stage.clone();
        assert!(update.rollback().is_err());
        assert!(stage.join("backup/a").exists());
        drop(locked);
        update.rollback().unwrap();
        update.commit().unwrap();
    }
}
