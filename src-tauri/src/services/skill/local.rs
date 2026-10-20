//! Offline, provenance-bound local skill deployment through the service layer.
//! The caller exports a reviewed Git commit. No runtime directories are staging inputs.
use super::*;
use serde_json::{json, Value};

fn invalid(message: impl Into<String>) -> AppError {
    AppError::InvalidInput(message.into())
}

fn validate_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

fn provenance(repo: &str, path: &str, commit: &str, name: &str) -> Result<String, AppError> {
    let repo_parts: Vec<_> = repo.split('/').collect();
    if repo_parts.len() != 2
        || !repo_parts.iter().all(|p| validate_segment(p))
        || !validate_segment(name)
        || name.starts_with('.')
        || !path.split('/').all(validate_segment)
        || path.rsplit('/').next() != Some(name)
        || commit.len() != 40
        || !commit.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(invalid("Invalid local skill source identity/path/commit"));
    }
    Ok(format!(
        "https://github.com/{repo}/blob/{}/{path}/SKILL.md",
        commit.to_ascii_lowercase()
    ))
}

fn same_source(url: &str, repo: &str, path: &str) -> bool {
    let prefix = format!("https://github.com/{repo}/blob/");
    let Some(rest) = url.strip_prefix(&prefix) else {
        return false;
    };
    let Some((commit, tail)) = rest.split_once('/') else {
        return false;
    };
    commit.len() == 40
        && commit.bytes().all(|b| b.is_ascii_hexdigit())
        && tail == format!("{path}/SKILL.md")
}

fn reject_links(path: &Path) -> Result<(), AppError> {
    for ancestor in path.ancestors() {
        if let Ok(meta) = fs::symlink_metadata(ancestor) {
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if meta.file_attributes() & 0x400 != 0 {
                    return Err(invalid("Reparse point in local source path"));
                }
            }
            if meta.file_type().is_symlink() {
                return Err(invalid("Link in local source path"));
            }
        }
    }
    let meta = fs::symlink_metadata(path).map_err(|e| AppError::io(path, e))?;
    if meta.is_dir() {
        for entry in fs::read_dir(path).map_err(|e| AppError::io(path, e))? {
            reject_links(&entry.map_err(|e| AppError::io(path, e))?.path())?;
        }
    } else if !meta.is_file() {
        return Err(invalid("Local source contains a non-regular file"));
    }
    Ok(())
}

// Unlike marketplace hashes, private publication includes hidden files and
// length-frames names/content. All-tree readback must protect user edits too.
pub(super) fn local_tree_hash(root: &Path) -> Result<String, AppError> {
    local_tree_hash_with_runtime(root, &runtime_files::RuntimeFiles::none())
}

pub(super) fn local_tree_hash_with_runtime(
    root: &Path,
    runtime: &runtime_files::RuntimeFiles,
) -> Result<String, AppError> {
    tree_hash(root, runtime, true)
}

pub(super) fn hash_path_name(root: &Path, path: &Path) -> Result<Vec<u8>, AppError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| invalid("Hash path escaped root"))?;
    let mut name = Vec::new();
    // Normalize only actual separators. Preserve literal Unix backslashes
    // and non-Unicode names without changing ordinary UTF-8 baselines.
    for component in relative.components() {
        if !name.is_empty() {
            name.push(b'/');
        }
        name.extend_from_slice(component.as_os_str().as_encoded_bytes());
    }
    Ok(name)
}

fn tree_hash(
    root: &Path,
    runtime: &runtime_files::RuntimeFiles,
    include_directories: bool,
) -> Result<String, AppError> {
    use sha2::{Digest, Sha256};
    let root = in_place::managed_path(root)?;
    let root = root.as_path();
    fn collect(
        root: &Path,
        base: &Path,
        files: &mut Vec<PathBuf>,
        directories: &mut Vec<PathBuf>,
        runtime: &runtime_files::RuntimeFiles,
    ) -> Result<bool, AppError> {
        let mut had_children = false;
        let mut managed_children = false;
        for entry in fs::read_dir(root).map_err(|e| AppError::io(root, e))? {
            had_children = true;
            let path = entry.map_err(|e| AppError::io(root, e))?.path();
            if runtime.contains(path.strip_prefix(base).expect("descendant")) {
                if !fs::symlink_metadata(&path)
                    .map_err(|e| AppError::io(&path, e))?
                    .is_file()
                {
                    return Err(invalid("Runtime path must be a regular file"));
                }
                continue;
            }
            if path.is_dir() {
                if !collect(&path, base, files, directories, runtime)? {
                    continue;
                }
                directories.push(path);
            } else {
                files.push(path);
            }
            managed_children = true;
        }
        Ok(managed_children || !had_children)
    }
    reject_links(root)?;
    let mut files = Vec::new();
    let mut directories = Vec::new();
    collect(root, root, &mut files, &mut directories, runtime)?;
    files.sort();
    let mut digest = Sha256::new();
    for file in files {
        let name = hash_path_name(root, &file)?;
        let bytes = fs::read(&file).map_err(|e| AppError::io(&file, e))?;
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name);
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&file)
                .map_err(|e| AppError::io(&file, e))?
                .permissions()
                .mode()
                & 0o7777;
            digest.update(mode.to_le_bytes());
        }
    }
    let file_hash = format!("local-sha256:{:x}", digest.finalize());
    if !include_directories {
        return Ok(file_hash);
    }
    directories.sort();
    let mut digest = Sha256::new();
    for directory in directories {
        let name = hash_path_name(root, &directory)?;
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name);
    }
    Ok(format!("{file_hash}:{:x}", digest.finalize()))
}

pub(super) fn has_directory_baseline(expected: &str) -> bool {
    expected
        .strip_prefix("local-sha256:")
        .is_some_and(|hash| hash.contains(':'))
}

pub(super) fn publication_hash(
    root: &Path,
    runtime: &runtime_files::RuntimeFiles,
    previous: Option<&str>,
) -> Result<String, AppError> {
    // Legacy records cannot prove ownership of existing empty directories.
    // Keep their file-only baseline and the conservative replacement guard.
    tree_hash(root, runtime, previous.is_none_or(has_directory_baseline))
}

pub(super) fn file_tree_hash_with_runtime(
    root: &Path,
    runtime: &runtime_files::RuntimeFiles,
) -> Result<String, AppError> {
    tree_hash(root, runtime, false)
}

// File-only records and unmanaged trees do not identify owned directories.
// Preserve them rather than guessing that a replacement is safe.
pub(super) fn require_legacy_directory_safety(
    root: &Path,
    expected: &str,
    source: &in_place::SourceSnapshot,
) -> Result<(), AppError> {
    if has_directory_baseline(expected) {
        return Ok(());
    }
    match fs::symlink_metadata(root) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(AppError::io(root, e)),
        Ok(_) => {}
    }
    fn check(
        current: &Path,
        root: &Path,
        source: &in_place::SourceSnapshot,
    ) -> Result<(), AppError> {
        for entry in fs::read_dir(current).map_err(|e| AppError::io(current, e))? {
            let path = entry.map_err(|e| AppError::io(current, e))?.path();
            if fs::symlink_metadata(&path)
                .map_err(|e| AppError::io(&path, e))?
                .is_dir()
            {
                if source
                    .file(path.strip_prefix(root).expect("descendant"))
                    .is_some()
                {
                    return Err(invalid(format!(
                        "Legacy skill directory has no topology baseline; refusing replacement: {}",
                        path.display()
                    )));
                }
                check(&path, root, source)?;
            }
        }
        Ok(())
    }
    check(root, root, source)
}

pub(super) fn matches_baseline(
    root: &Path,
    expected: &str,
    runtime: &runtime_files::RuntimeFiles,
) -> Result<bool, AppError> {
    let directories = has_directory_baseline(expected);
    if tree_hash(root, runtime, directories)? == expected {
        return Ok(true);
    }
    // Older installations hashed generated Python caches. Accept that exact
    // legacy baseline without ever opening explicitly declared runtime files.
    if runtime.has_automatic() {
        return Ok(tree_hash(root, &runtime.legacy_baseline(), directories)? == expected);
    }
    Ok(false)
}

// Rollback preserves directories. A failed first install may therefore leave
// an unmanaged tree containing only directories, which is safe to reuse as
// long as publication does not replace any of those directories with a file.
fn require_file_empty_target(path: &Path, conflict: &str) -> Result<(), AppError> {
    let path = in_place::managed_path(path)?;
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(AppError::io(&path, e)),
        Ok(_) => {}
    }
    reject_links(&path)?;
    fn check_dirs(path: &Path, conflict: &str) -> Result<(), AppError> {
        for entry in fs::read_dir(path).map_err(|e| AppError::io(path, e))? {
            let entry = entry.map_err(|e| AppError::io(path, e))?;
            let kind = entry
                .file_type()
                .map_err(|e| AppError::io(entry.path(), e))?;
            if !kind.is_dir() {
                return Err(invalid(conflict));
            }
            check_dirs(&entry.path(), conflict)?;
        }
        Ok(())
    }
    check_dirs(&path, conflict)
}

fn abort_updates(mut updates: Vec<in_place::Update>, cause: AppError) -> AppError {
    let mut failures = Vec::new();
    while let Some(mut update) = updates.pop() {
        match update.rollback() {
            Ok(()) => {
                if let Err(e) = update.commit() {
                    failures.push(e.to_string());
                }
            }
            Err(e) => failures.push(e.to_string()),
        }
    }
    AppError::Message(format!(
        "{cause}; {}",
        if failures.is_empty() {
            "local skill rollback verified".to_string()
        } else {
            format!(
                "recovery incomplete; retained journals: {}",
                failures.join("; ")
            )
        }
    ))
}

impl SkillService {
    /// No DB creation/migration, no settings migration, and no runtime writes.
    pub fn local_info(name: &str) -> Result<Value, AppError> {
        if !validate_segment(name) {
            return Err(invalid("Invalid skill name"));
        }
        if !crate::config::get_app_config_dir()
            .join("cc-switch.db")
            .exists()
        {
            return Ok(json!({"found": false, "skill": null}));
        }
        let db = Database::open_readonly_current_schema()?;
        let mut skill = db
            .get_all_installed_skills()?
            .into_values()
            .find(|s| s.directory.eq_ignore_ascii_case(name));
        if let Some(record) = skill.as_mut() {
            record.apps.pi = Self::skill_exists_in_app(&record.directory, &AppType::Pi);
        }
        Ok(json!({"found": skill.is_some(), "skill": skill}))
    }

    /// Install or update one managed local source, preserving every app flag.
    /// DB is written last. File transactions roll back on pre-commit failures.
    /// A post-commit cleanup failure is explicit; journals are kept for recovery.
    pub fn install_local(
        source: &Path,
        name: &str,
        repo: &str,
        path: &str,
        commit: &str,
        app: &AppType,
    ) -> Result<Value, AppError> {
        Self::install_local_checked(source, name, repo, path, commit, app, |_| Ok(()))
    }

    pub fn install_local_with_runtime_files(
        source: &Path,
        name: &str,
        repo: &str,
        path: &str,
        commit: &str,
        app: &AppType,
        runtime_files: &[String],
    ) -> Result<Value, AppError> {
        let runtime = runtime_files::RuntimeFiles::new(runtime_files)?;
        Self::install_local_checked_with_runtime(
            source,
            name,
            repo,
            path,
            commit,
            app,
            &runtime,
            |_| Ok(()),
        )
    }

    fn install_local_checked<F: FnOnce(&Database) -> Result<(), AppError>>(
        source: &Path,
        name: &str,
        repo: &str,
        path: &str,
        commit: &str,
        app: &AppType,
        before_save: F,
    ) -> Result<Value, AppError> {
        Self::install_local_checked_with_runtime(
            source,
            name,
            repo,
            path,
            commit,
            app,
            &runtime_files::RuntimeFiles::default(),
            before_save,
        )
    }

    fn install_local_checked_with_runtime<F: FnOnce(&Database) -> Result<(), AppError>>(
        source: &Path,
        name: &str,
        repo: &str,
        path: &str,
        commit: &str,
        app: &AppType,
        runtime: &runtime_files::RuntimeFiles,
        before_save: F,
    ) -> Result<Value, AppError> {
        if !matches!(app, AppType::Codex) {
            return Err(invalid("Local installation defaults must target codex"));
        }
        Self::require_valid_directory(name)?;
        let readme_url = provenance(repo, path, commit, name)?;
        if !source.is_absolute() || !source.is_dir() {
            return Err(invalid("Source must be an absolute directory"));
        }
        reject_links(source)?;
        let mut snapshot = in_place::SourceSnapshot::capture(source, runtime)?;
        let skill_bytes = snapshot
            .file(Path::new("SKILL.md"))
            .ok_or_else(|| invalid("Source requires SKILL.md"))?;
        let skill_content =
            std::str::from_utf8(skill_bytes).map_err(|_| invalid("SKILL.md must be UTF-8"))?;
        let metadata = Self::parse_skill_metadata_content(skill_content);
        if metadata.name.as_deref() != Some(name)
            || metadata
                .description
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
        {
            return Err(invalid(
                "SKILL.md requires matching name and a nonempty description",
            ));
        }
        let _guard = skill_state_write_guard();
        let _publication_guard = super::publication_lock::acquire()?;
        let db = Database::open_for_skill_publication()?;
        let baseline = db.capture_local_skill_baseline(name)?;
        let mut existing = baseline.skill.clone();
        if let Some(record) = existing.as_mut() {
            // Pi tracks deployment in its native directory, not in the skill row.
            record.apps.pi = Self::skill_exists_in_app(&record.directory, &AppType::Pi);
        }
        if let Some(old) = &existing {
            if old.id != format!("local:{name}")
                || old.directory != name
                || old.repo_owner.is_some()
                || old.repo_name.is_some()
                || old.repo_branch.is_some()
                || !same_source(old.readme_url.as_deref().unwrap_or(""), repo, path)
            {
                return Err(invalid(
                    "Foreign skill name/source conflict; refusing replacement",
                ));
            }
        }
        let ssot = Self::get_ssot_dir()?.join(name);
        let source_canonical = snapshot.source();
        // Runtime content is never an acceptable authoritative source, even for a new name.
        for root in std::iter::once(Self::get_ssot_dir()?)
            .chain(Self::supported_skill_apps().filter_map(|a| Self::get_app_skills_dir(&a).ok()))
        {
            if root.exists()
                && source_canonical
                    .starts_with(fs::canonicalize(&root).map_err(|e| AppError::io(&root, e))?)
            {
                return Err(invalid("Runtime skill directories cannot be local sources"));
            }
        }
        if existing.is_none() {
            require_file_empty_target(&ssot, "Unmanaged SSOT directory conflict")?;
            require_legacy_directory_safety(&ssot, "", &snapshot)?;
        }
        if let Some(old) = &existing {
            if !matches_baseline(
                &ssot,
                old.content_hash.as_deref().unwrap_or_default(),
                runtime,
            )? {
                return Err(invalid("Installed SSOT has local edits; refusing update"));
            }
            require_legacy_directory_safety(
                &ssot,
                old.content_hash.as_deref().unwrap_or_default(),
                &snapshot,
            )?;
        }
        let apps = existing
            .as_ref()
            .map(|s| s.apps.clone())
            .unwrap_or_else(|| SkillApps::only(app));
        let mut destinations = vec![in_place::managed_path(&ssot)?];
        for target in Self::supported_skill_apps().filter(|a| apps.is_enabled_for(a)) {
            let dest =
                Self::get_distinct_app_skills_dir(&Self::get_ssot_dir()?, &target)?.join(name);
            if Self::is_symlink(&dest) {
                if existing.is_some()
                    && fs::canonicalize(&dest).ok() == fs::canonicalize(&ssot).ok()
                {
                    continue;
                }
                return Err(invalid("Foreign application link conflict"));
            }
            let dest = in_place::managed_path(&dest)?;
            if destinations
                .iter()
                .any(|other| !Self::paths_equal(other, &dest) && Self::paths_overlap(other, &dest))
            {
                return Err(invalid("Local skill deployment targets cannot overlap"));
            }
            if existing.is_none() {
                require_file_empty_target(&dest, "Unmanaged application directory conflict")?;
                require_legacy_directory_safety(&dest, "", &snapshot)?;
            }
            // Compare physical paths, while retaining managed-path/link checks
            // for every app. Shared app directories need only one transaction.
            if !destinations
                .iter()
                .any(|other| Self::paths_equal(other, &dest))
            {
                destinations.push(dest);
            }
        }
        if existing.is_some() {
            // Check the complete topology first, then compare each projection
            // to the verified SSOT, including ordinary empty directories.
            let ssot_hash = Self::compute_pi_deployment_hash_with_runtime(&ssot, runtime)?;
            for dest in destinations.iter().skip(1).filter(|dest| dest.exists()) {
                if Self::compute_pi_deployment_hash_with_runtime(dest, runtime)? != ssot_hash {
                    return Err(invalid(
                        "Application skill has local edits; refusing update",
                    ));
                }
            }
        }
        if existing.is_none() {
            for dest in destinations.iter().skip(1) {
                snapshot.retain_directories_from(dest)?;
            }
        }
        snapshot.retain_directories_from(&ssot)?;
        // Prepare every file tree before any deployment mutation. Copies for new
        // projections avoid needing symlink privileges; existing SSOT links survive.
        let mut prepared = Vec::new();
        for dest in &destinations {
            match snapshot.prepare(dest) {
                Ok(update) => prepared.push(update),
                Err(error) => {
                    for update in prepared {
                        let _ = update.commit();
                    }
                    return Err(error);
                }
            }
        }
        let mut applied = Vec::new();
        for mut update in prepared {
            if let Err(error) = update.apply() {
                return Err(abort_updates(applied, error));
            }
            applied.push(update);
        }
        if let Err(error) = Self::verify_publication_projections(&destinations, runtime) {
            return Err(abort_updates(applied, error));
        }
        let content_hash = match publication_hash(
            &ssot,
            runtime,
            existing
                .as_ref()
                .and_then(|old| old.content_hash.as_deref()),
        ) {
            Ok(hash) => hash,
            Err(error) => return Err(abort_updates(applied, error)),
        };
        let now = Utc::now().timestamp();
        let skill = InstalledSkill {
            id: format!("local:{name}"),
            name: name.to_string(),
            description: metadata.description,
            directory: name.to_string(),
            repo_owner: None,
            repo_name: None,
            repo_branch: None,
            readme_url: Some(readme_url.clone()),
            apps,
            installed_at: existing.as_ref().map(|s| s.installed_at).unwrap_or(now),
            content_hash: Some(content_hash.clone()),
            updated_at: now,
        };
        if let Err(error) = before_save(&db) {
            return Err(abort_updates(applied, error));
        }
        if let Err(error) = db.save_local_skill_if_unchanged(&baseline, &skill) {
            return Err(abort_updates(applied, error));
        }
        let action = match &existing {
            None => "installed",
            Some(old)
                if old.content_hash.as_deref() == Some(content_hash.as_str())
                    && old.readme_url.as_deref() == Some(readme_url.as_str()) =>
            {
                "unchanged"
            }
            Some(_) => "updated",
        };
        let mut cleanup = Vec::new();
        for update in applied {
            if let Err(error) = update.commit() {
                cleanup.push(error.to_string());
            }
        }
        if !cleanup.is_empty() {
            return Err(AppError::Message(format!(
                "Local deployment committed; cleanup incomplete: {}",
                cleanup.join("; ")
            )));
        }
        Ok(json!({"action": action, "skill": skill, "sourceRepo": repo,
            "sourcePath": path, "sourceCommit": commit.to_ascii_lowercase()}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_updates_keep_retained_and_runtime_only_topology_consistent() {
        for missing in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let _env = crate::test_support::TestEnvGuard::isolated(home.path());
            let input = source(home.path(), "first");
            fs::create_dir_all(input.join("retained/empty")).unwrap();
            for directory in ["private-dir", "generated/deep"] {
                fs::create_dir_all(input.join(directory)).unwrap();
                fs::write(input.join(directory).join("code.py"), "old code").unwrap();
            }
            install(&input, 'a').unwrap();
            let ssot = SkillService::get_ssot_dir().unwrap().join("demo");
            let app = SkillService::get_app_skills_dir(&AppType::Codex)
                .unwrap()
                .join("demo");
            fs::write(ssot.join("private-dir/local.json"), "private").unwrap();
            fs::create_dir_all(ssot.join("generated/deep/__pycache__")).unwrap();
            fs::write(ssot.join("generated/deep/__pycache__/code.pyc"), "cache").unwrap();
            if missing {
                fs::rename(&app, home.path().join("preserved-app")).unwrap();
            }
            let next = source(&home.path().join("next"), "second");
            for commit in ['b', 'c'] {
                let outcome = SkillService::install_local_with_runtime_files(
                    &next,
                    "demo",
                    "owner/private",
                    "private-skills/demo",
                    &commit.to_string().repeat(40),
                    &AppType::Codex,
                    &["private-dir/local.json".into()],
                )
                .unwrap();
                let runtime =
                    runtime_files::RuntimeFiles::new(&["private-dir/local.json".into()]).unwrap();
                for dest in [&ssot, &app] {
                    assert!(dest.join("retained/empty").is_dir());
                    assert_eq!(
                        local_tree_hash_with_runtime(dest, &runtime).unwrap(),
                        outcome["skill"]["contentHash"].as_str().unwrap()
                    );
                    assert_eq!(
                        fs::read(dest.join("SKILL.md")).unwrap(),
                        fs::read(next.join("SKILL.md")).unwrap()
                    );
                }
                assert!(!app.join("private-dir").exists());
                assert!(!app.join("generated").exists());
                assert_eq!(
                    fs::read(ssot.join("private-dir/local.json")).unwrap(),
                    b"private"
                );
                assert_eq!(
                    fs::read(ssot.join("generated/deep/__pycache__/code.pyc")).unwrap(),
                    b"cache"
                );
            }
        }
    }

    #[test]
    fn local_first_install_preserves_distinct_empty_trees_in_all_copies() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let input = source(home.path(), "first");
        let ssot = SkillService::get_ssot_dir().unwrap().join("demo");
        let app = SkillService::get_app_skills_dir(&AppType::Codex)
            .unwrap()
            .join("demo");
        fs::create_dir_all(ssot.join("ssot-only/empty")).unwrap();
        fs::create_dir_all(app.join("app-only/empty")).unwrap();
        let outcome = install(&input, 'a').unwrap();
        for dest in [&ssot, &app] {
            assert!(dest.join("ssot-only/empty").is_dir());
            assert!(dest.join("app-only/empty").is_dir());
            assert_eq!(
                local_tree_hash(dest).unwrap(),
                outcome["skill"]["contentHash"].as_str().unwrap()
            );
        }
        assert!(install(&input, 'b').is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn local_republish_rejects_backslash_file_renames() {
        for (from, to, app_only) in [
            (r"a\b", "a/b", false),
            ("a/b", r"a\b", false),
            (r"a\b", "a/b", true),
            ("a/b", r"a\b", true),
        ] {
            let home = tempfile::tempdir().unwrap();
            let _env = crate::test_support::TestEnvGuard::isolated(home.path());
            let input = source(home.path(), "first");
            fs::create_dir_all(input.join("a")).unwrap();
            // Keep the directory topology and file ordering unchanged by rename.
            fs::write(input.join("a/0"), "anchor").unwrap();
            fs::write(input.join(from), "original").unwrap();
            install(&input, 'a').unwrap();
            let targets = [
                SkillService::get_ssot_dir().unwrap().join("demo"),
                SkillService::get_app_skills_dir(&AppType::Codex)
                    .unwrap()
                    .join("demo"),
            ];
            let info = SkillService::local_info("demo").unwrap();
            let ssot_before = local_tree_hash(&targets[0]).unwrap();
            let changed = usize::from(app_only);
            let before = local_tree_hash(&targets[changed]).unwrap();
            for target in targets.iter().skip(changed) {
                fs::rename(target.join(from), target.join(to)).unwrap();
            }
            let after = local_tree_hash(&targets[changed]).unwrap();
            assert_eq!(before.rsplit(':').next(), after.rsplit(':').next());
            assert_ne!(before, after);
            let error = install(&input, 'b').unwrap_err();
            assert!(error.to_string().contains("local edits"), "{error}");
            assert_eq!(SkillService::local_info("demo").unwrap(), info);
            if app_only {
                assert_eq!(local_tree_hash(&targets[0]).unwrap(), ssot_before);
            }
            for (i, target) in targets.iter().enumerate() {
                let (present, absent) = if app_only && i == 0 {
                    (from, to)
                } else {
                    (to, from)
                };
                assert!(!target.join(absent).exists());
                assert_eq!(fs::read(target.join(present)).unwrap(), b"original");
                assert_eq!(fs::read(target.join("a/0")).unwrap(), b"anchor");
                assert_eq!(
                    fs::read(target.join("SKILL.md")).unwrap(),
                    fs::read(input.join("SKILL.md")).unwrap()
                );
            }
        }
    }

    #[test]
    #[cfg(unix)]
    fn local_tree_hash_distinguishes_non_utf8_file_and_directory_names() {
        use std::os::unix::ffi::OsStrExt;
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let from = std::ffi::OsStr::from_bytes(b"name-\x80");
        let to = std::ffi::OsStr::from_bytes(b"name-\x81");
        assert_eq!(from.to_string_lossy(), to.to_string_lossy());
        for directory in [false, true] {
            let root = tempfile::tempdir_in(home.path()).unwrap();
            if directory {
                fs::create_dir(root.path().join(from)).unwrap();
            } else {
                fs::write(root.path().join(from), "original").unwrap();
            }
            let before = local_tree_hash(root.path()).unwrap();
            fs::rename(root.path().join(from), root.path().join(to)).unwrap();
            assert_ne!(local_tree_hash(root.path()).unwrap(), before);
            assert!(
                !matches_baseline(root.path(), &before, &runtime_files::RuntimeFiles::none())
                    .unwrap()
            );
        }
    }

    #[test]
    fn local_update_protects_added_ssot_directories_even_when_all_copies_match() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let input = source(home.path(), "first");
        install(&input, 'a').unwrap();
        let targets = [
            SkillService::get_ssot_dir().unwrap().join("demo"),
            SkillService::get_app_skills_dir(&AppType::Codex)
                .unwrap()
                .join("demo"),
        ];
        let before = SkillService::local_info("demo").unwrap()["skill"].clone();
        for target in &targets {
            fs::create_dir_all(target.join("notes/nested")).unwrap();
        }
        source(home.path(), "second");
        fs::write(input.join("notes"), "must not replace user directory").unwrap();
        let error = install(&input, 'b').unwrap_err();
        assert!(error.to_string().contains("local edits"), "{error}");
        assert_eq!(SkillService::local_info("demo").unwrap()["skill"], before);
        for target in targets {
            assert!(target.join("notes/nested").is_dir());
            assert!(fs::read_to_string(target.join("SKILL.md"))
                .unwrap()
                .contains("first"));
        }
    }

    #[test]
    fn local_legacy_directory_protection_survives_an_intermediate_update() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let input = source(home.path(), "first");
        install(&input, 'a').unwrap();
        let targets = [
            SkillService::get_ssot_dir().unwrap().join("demo"),
            SkillService::get_app_skills_dir(&AppType::Codex)
                .unwrap()
                .join("demo"),
        ];
        let db = Database::init().unwrap();
        let mut row = db.get_installed_skill("local:demo").unwrap().unwrap();
        row.content_hash = Some(
            file_tree_hash_with_runtime(&targets[0], &runtime_files::RuntimeFiles::none()).unwrap(),
        );
        db.save_skill(&row).unwrap();
        for target in &targets {
            fs::create_dir_all(target.join("notes")).unwrap();
        }
        source(home.path(), "second");
        assert_eq!(install(&input, 'b').unwrap()["action"], "updated");
        let before = db.get_installed_skill("local:demo").unwrap().unwrap();
        assert!(!has_directory_baseline(
            before.content_hash.as_deref().unwrap()
        ));
        source(home.path(), "third");
        fs::write(input.join("notes"), "must not adopt user directory").unwrap();
        let error = install(&input, 'c').unwrap_err();
        assert!(
            error.to_string().contains("no topology baseline"),
            "{error}"
        );
        assert_eq!(
            db.get_installed_skill("local:demo")
                .unwrap()
                .unwrap()
                .content_hash,
            before.content_hash
        );
        for target in targets {
            assert!(target.join("notes").is_dir());
            assert!(fs::read_to_string(target.join("SKILL.md"))
                .unwrap()
                .contains("second"));
        }
    }

    #[test]
    fn local_update_still_protects_nonruntime_empty_app_directories() {
        for explicit in [false, true] {
            for relative in ["notes", "notes/nested", "__pycache__"] {
                let home = tempfile::tempdir().unwrap();
                let _env = crate::test_support::TestEnvGuard::isolated(home.path());
                let input = source(home.path(), "first");
                install(&input, 'a').unwrap();
                let targets = [
                    SkillService::get_ssot_dir().unwrap().join("demo"),
                    SkillService::get_app_skills_dir(&AppType::Codex)
                        .unwrap()
                        .join("demo"),
                ];
                let before = SkillService::local_info("demo").unwrap()["skill"].clone();
                fs::create_dir_all(targets[1].join(relative)).unwrap();
                if explicit {
                    for (i, target) in targets.iter().enumerate() {
                        fs::write(target.join("local.json"), format!("private-{i}")).unwrap();
                    }
                }
                source(home.path(), "second");
                fs::write(
                    input.join(relative.split('/').next().unwrap()),
                    "cannot replace user directory",
                )
                .unwrap();
                let paths = if explicit {
                    vec!["local.json".into()]
                } else {
                    Vec::new()
                };
                let error = SkillService::install_local_with_runtime_files(
                    &input,
                    "demo",
                    "owner/private",
                    "private-skills/demo",
                    &"b".repeat(40),
                    &AppType::Codex,
                    &paths,
                )
                .unwrap_err();
                assert!(error.to_string().contains("local edits"), "{error}");
                assert!(targets[1].join(relative).is_dir());
                assert_eq!(SkillService::local_info("demo").unwrap()["skill"], before);
                for (i, target) in targets.iter().enumerate() {
                    assert!(fs::read_to_string(target.join("SKILL.md"))
                        .unwrap()
                        .contains("first"));
                    if explicit {
                        assert_eq!(
                            fs::read_to_string(target.join("local.json")).unwrap(),
                            format!("private-{i}")
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn local_update_allows_baselined_empty_directory_conversion() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let input = source(home.path(), "first");
        fs::create_dir_all(input.join("notes/nested")).unwrap();
        install(&input, 'a').unwrap();
        let targets = [
            SkillService::get_ssot_dir().unwrap().join("demo"),
            SkillService::get_app_skills_dir(&AppType::Codex)
                .unwrap()
                .join("demo"),
        ];
        let cache = targets[1].join("scripts/__pycache__/module.pyc");
        fs::create_dir_all(cache.parent().unwrap()).unwrap();
        fs::write(&cache, "private cache").unwrap();
        let next = source(&home.path().join("next"), "second");
        fs::write(next.join("notes"), "published file").unwrap();
        assert_eq!(install(&next, 'b').unwrap()["action"], "updated");
        for target in targets {
            assert_eq!(fs::read(target.join("notes")).unwrap(), b"published file");
            assert!(fs::read_to_string(target.join("SKILL.md"))
                .unwrap()
                .contains("second"));
        }
        assert_eq!(fs::read(cache).unwrap(), b"private cache");
    }

    #[test]
    fn local_update_keeps_runtime_files_per_copy_and_rejects_source_conflicts() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let input = source(home.path(), "first");
        install(&input, 'a').unwrap();
        let targets = [
            SkillService::get_ssot_dir().unwrap().join("demo"),
            SkillService::get_app_skills_dir(&AppType::Codex)
                .unwrap()
                .join("demo"),
        ];
        for (i, target) in targets.iter().enumerate() {
            fs::create_dir_all(target.join("scripts/__pycache__")).unwrap();
            fs::write(target.join("scripts/__pycache__/module.pyc"), [i as u8]).unwrap();
            fs::write(target.join("local.json"), format!("secret-{i}")).unwrap();
        }
        let publish = || {
            SkillService::install_local_with_runtime_files(
                &input,
                "demo",
                "owner/private",
                "private-skills/demo",
                &"b".repeat(40),
                &AppType::Codex,
                &["local.json".into()],
            )
        };
        source(home.path(), "second");
        assert_eq!(publish().unwrap()["action"], "updated");
        for (i, target) in targets.iter().enumerate() {
            assert_eq!(
                fs::read(target.join("scripts/__pycache__/module.pyc")).unwrap(),
                [i as u8]
            );
            assert_eq!(
                fs::read_to_string(target.join("local.json")).unwrap(),
                format!("secret-{i}")
            );
            assert!(fs::read_to_string(target.join("SKILL.md"))
                .unwrap()
                .contains("second"));
        }
        fs::write(input.join("local.json"), "must not publish").unwrap();
        assert!(publish()
            .unwrap_err()
            .to_string()
            .contains("source conflict"));
        for target in &targets {
            assert!(fs::read_to_string(target.join("SKILL.md"))
                .unwrap()
                .contains("second"));
        }
    }

    #[cfg(windows)]
    #[test]
    fn local_update_does_not_open_runtime_files() {
        use std::os::windows::fs::OpenOptionsExt;
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let input = source(home.path(), "first");
        install(&input, 'a').unwrap();
        let targets = [
            SkillService::get_ssot_dir().unwrap().join("demo"),
            SkillService::get_app_skills_dir(&AppType::Codex)
                .unwrap()
                .join("demo"),
        ];
        let mut handles = Vec::new();
        for target in &targets {
            for relative in ["local.json", "__pycache__/module.pyc"] {
                let file = target.join(relative);
                fs::create_dir_all(file.parent().unwrap()).unwrap();
                fs::write(&file, "private").unwrap();
                handles.push(
                    fs::OpenOptions::new()
                        .read(true)
                        .share_mode(0)
                        .open(file)
                        .unwrap(),
                );
            }
        }
        source(home.path(), "second");
        SkillService::install_local_with_runtime_files(
            &input,
            "demo",
            "owner/private",
            "private-skills/demo",
            &"b".repeat(40),
            &AppType::Codex,
            &["local.json".into()],
        )
        .unwrap();
        drop(handles);
    }

    #[test]
    fn local_update_automatically_excludes_only_python_cache_files() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let input = source(home.path(), "first");
        install(&input, 'a').unwrap();
        let app = SkillService::get_app_skills_dir(&AppType::Codex)
            .unwrap()
            .join("demo");
        fs::create_dir_all(app.join("scripts/__pycache__")).unwrap();
        fs::write(app.join("scripts/__pycache__/m.pyc"), "cache").unwrap();
        source(home.path(), "second");
        fs::create_dir_all(input.join("scripts/__pycache__")).unwrap();
        fs::write(
            input.join("scripts/__pycache__/source.pyc"),
            "generated source cache",
        )
        .unwrap();
        install(&input, 'b').unwrap();
        assert_eq!(
            fs::read(app.join("scripts/__pycache__/m.pyc")).unwrap(),
            b"cache"
        );
        assert!(!SkillService::get_ssot_dir()
            .unwrap()
            .join("demo/scripts/__pycache__/source.pyc")
            .exists());
        fs::write(app.join("scripts/__pycache__/keep.txt"), "user content").unwrap();
        source(home.path(), "third");
        assert!(install(&input, 'c')
            .unwrap_err()
            .to_string()
            .contains("local edits"));
    }

    #[test]
    fn local_update_accepts_legacy_hash_that_included_python_cache() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let input = source(home.path(), "first");
        install(&input, 'a').unwrap();
        let targets = [
            SkillService::get_ssot_dir().unwrap().join("demo"),
            SkillService::get_app_skills_dir(&AppType::Codex)
                .unwrap()
                .join("demo"),
        ];
        for target in &targets {
            fs::create_dir_all(target.join("__pycache__")).unwrap();
            fs::write(target.join("__pycache__/module.pyc"), "legacy cache").unwrap();
        }
        let db = Database::init().unwrap();
        let mut row = db.get_installed_skill("local:demo").unwrap().unwrap();
        row.content_hash = Some(
            file_tree_hash_with_runtime(&targets[0], &runtime_files::RuntimeFiles::none()).unwrap(),
        );
        db.save_skill(&row).unwrap();
        source(home.path(), "second");
        assert_eq!(
            SkillService::install_local_with_runtime_files(
                &input,
                "demo",
                "owner/private",
                "private-skills/demo",
                &"b".repeat(40),
                &AppType::Codex,
                &[],
            )
            .unwrap()["action"],
            "updated"
        );
        for target in targets {
            assert_eq!(
                fs::read(target.join("__pycache__/module.pyc")).unwrap(),
                b"legacy cache"
            );
            assert!(fs::read_to_string(target.join("SKILL.md"))
                .unwrap()
                .contains("second"));
        }
    }
    fn source(root: &Path, text: &str) -> PathBuf {
        let dir = root.join("export").join("demo");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: demo\ndescription: Test local source\n---\n{text}\n"),
        )
        .unwrap();
        dir
    }
    fn install(dir: &Path, commit: char) -> Result<Value, AppError> {
        SkillService::install_local(
            dir,
            "demo",
            "owner/private",
            "private-skills/demo",
            &commit.to_string().repeat(40),
            &AppType::Codex,
        )
    }
    #[test]
    fn local_schema20_publication_preserves_proxy_flags_on_success_and_rejection() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let input = source(home.path(), "first");
        let db = Database::init().unwrap();
        {
            let conn = db.conn.lock().unwrap();
            assert_eq!(Database::get_user_version(&conn).unwrap(), 20);
            assert!(
                conn.execute(
                    "UPDATE proxy_config SET enabled=0, auto_failover_enabled=1",
                    [],
                )
                .unwrap()
                    > 0
            );
        }
        let proxy_flags = || {
            let conn = db.conn.lock().unwrap();
            let mut stmt = conn
                .prepare("SELECT app_type,enabled,auto_failover_enabled FROM proxy_config ORDER BY app_type")
                .unwrap();
            let flags = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            flags
        };
        let before = proxy_flags();
        assert_eq!(install(&input, 'a').unwrap()["action"], "installed");
        assert_eq!(proxy_flags(), before);
        source(home.path(), "second");
        // Rejection after file deployment must also avoid startup normalization.
        let error = SkillService::install_local_checked(
            &input,
            "demo",
            "owner/private",
            "private-skills/demo",
            &"b".repeat(40),
            &AppType::Codex,
            |_| Err(invalid("injected publication rejection")),
        )
        .unwrap_err();
        assert!(error.to_string().contains("rollback verified"));
        assert_eq!(proxy_flags(), before);
        // A preflight source conflict must preserve the same flags too.
        assert!(SkillService::install_local(
            &input,
            "demo",
            "other/repo",
            "private-skills/demo",
            &"b".repeat(40),
            &AppType::Codex,
        )
        .unwrap_err()
        .to_string()
        .contains("Foreign"));
        assert_eq!(proxy_flags(), before);
    }

    #[test]
    fn local_install_rejects_app_directory_inside_another_skill() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let input = source(home.path(), "first");
        let ssot = SkillService::get_ssot_dir().unwrap();
        let other = ssot.join("other");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("SKILL.md"), "other skill must remain intact").unwrap();
        let before = local_tree_hash(&other).unwrap();
        let mut settings = crate::settings::get_settings();
        settings.codex_config_dir = Some(other.to_string_lossy().into_owned());
        crate::settings::update_settings(settings).unwrap();

        assert!(install(&input, 'a')
            .unwrap_err()
            .to_string()
            .contains("cannot overlap"));
        assert_eq!(local_tree_hash(&other).unwrap(), before);
        assert!(!other.join("skills").exists());
        assert!(!ssot.join("demo").exists());
        assert_eq!(SkillService::local_info("demo").unwrap()["found"], false);
    }

    #[test]
    #[cfg(unix)]
    fn local_republish_preserves_existing_user_permission_edits() {
        use std::os::unix::fs::PermissionsExt;
        for app_copy in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let _env = crate::test_support::TestEnvGuard::isolated(home.path());
            let input = source(home.path(), "first");
            fs::write(input.join("run.sh"), "#!/bin/sh\n").unwrap();
            fs::set_permissions(input.join("run.sh"), fs::Permissions::from_mode(0o644)).unwrap();
            install(&input, 'a').unwrap();
            let info = SkillService::local_info("demo").unwrap();
            let target = if app_copy {
                SkillService::get_app_skills_dir(&AppType::Codex).unwrap()
            } else {
                SkillService::get_ssot_dir().unwrap()
            }
            .join("demo/run.sh");
            fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
            assert!(install(&input, 'a').is_err());
            assert_eq!(
                fs::metadata(&target).unwrap().permissions().mode() & 0o7777,
                0o755
            );
            assert_eq!(SkillService::local_info("demo").unwrap(), info);
        }
    }

    #[test]
    fn local_install_retains_upstream_old_schema_migration() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let input = source(home.path(), "first");
        {
            let db = Database::init().unwrap();
            let conn = db.conn.lock().unwrap();
            Database::set_user_version(&conn, 19).unwrap();
        }
        assert_eq!(install(&input, 'a').unwrap()["action"], "installed");
        let db = Database::open_existing_current_schema().unwrap();
        assert_eq!(
            Database::get_user_version(&db.conn.lock().unwrap()).unwrap(),
            20
        );
    }

    #[test]
    fn local_failed_first_install_retries_with_file_empty_trees() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let input = source(home.path(), "first");
        fs::create_dir_all(input.join("nested")).unwrap();
        fs::write(input.join("nested/data.bin"), [0, 255]).unwrap();
        let error = SkillService::install_local_checked(
            &input,
            "demo",
            "owner/private",
            "private-skills/demo",
            &"a".repeat(40),
            &AppType::Codex,
            |_| Err(invalid("injected first publication failure")),
        )
        .unwrap_err();
        assert!(error.to_string().contains("rollback verified"));
        assert_eq!(SkillService::local_info("demo").unwrap()["found"], false);
        let targets = [
            SkillService::get_ssot_dir().unwrap().join("demo"),
            SkillService::get_app_skills_dir(&AppType::Codex)
                .unwrap()
                .join("demo"),
        ];
        for target in &targets {
            assert!(target.join("nested").is_dir());
            assert!(!target.join("SKILL.md").exists());
            assert!(!target.join("nested/data.bin").exists());
        }
        assert_eq!(install(&input, 'a').unwrap()["action"], "installed");
        for target in &targets {
            assert_eq!(
                fs::read(target.join("SKILL.md")).unwrap(),
                fs::read(input.join("SKILL.md")).unwrap()
            );
            assert_eq!(fs::read(target.join("nested/data.bin")).unwrap(), [0, 255]);
        }
    }

    fn first_install_rejects_unbaselined_directory_replacement(app_copy: bool) {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let input = source(home.path(), "first");
        fs::write(input.join("notes"), "must not replace user directory").unwrap();
        fs::create_dir_all(input.join("published/nested")).unwrap();
        let targets = [
            SkillService::get_ssot_dir().unwrap().join("demo"),
            SkillService::get_app_skills_dir(&AppType::Codex)
                .unwrap()
                .join("demo"),
        ];
        for target in &targets {
            fs::create_dir_all(target.join("retained/nested")).unwrap();
        }
        let protected = &targets[usize::from(app_copy)];
        fs::create_dir_all(protected.join("notes/nested")).unwrap();
        let before: Vec<_> = targets
            .iter()
            .map(|target| local_tree_hash(target).unwrap())
            .collect();
        assert_eq!(SkillService::local_info("demo").unwrap()["found"], false);

        let error = install(&input, 'a').unwrap_err();
        assert!(
            error.to_string().contains("no topology baseline"),
            "{error}"
        );
        assert!(protected.join("notes/nested").is_dir());
        for (target, hash) in targets.iter().zip(before) {
            assert_eq!(local_tree_hash(target).unwrap(), hash);
            assert!(!target.join("SKILL.md").exists());
            assert!(!target.join("published").exists());
        }
        assert_eq!(SkillService::local_info("demo").unwrap()["found"], false);
    }

    #[test]
    fn local_first_install_rejects_unbaselined_ssot_directory_replacement() {
        first_install_rejects_unbaselined_directory_replacement(false);
    }

    #[test]
    fn local_first_install_rejects_unbaselined_app_directory_replacement() {
        first_install_rejects_unbaselined_directory_replacement(true);
    }

    #[test]
    fn local_install_rejects_unmanaged_trees_with_even_empty_files() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let input = source(home.path(), "first");
        for target in [
            SkillService::get_ssot_dir().unwrap().join("demo"),
            SkillService::get_app_skills_dir(&AppType::Codex)
                .unwrap()
                .join("demo"),
        ] {
            fs::create_dir_all(target.join("nested")).unwrap();
            let file = target.join("nested/.keep");
            fs::write(&file, b"").unwrap();
            assert!(install(&input, 'a')
                .unwrap_err()
                .to_string()
                .contains("Unmanaged"));
            assert_eq!(fs::read(&file).unwrap(), Vec::<u8>::new());
            assert_eq!(SkillService::local_info("demo").unwrap()["found"], false);
            fs::remove_file(file).unwrap();
        }
    }

    #[test]
    #[cfg(unix)]
    fn local_empty_target_rejects_links_and_nonregular_nodes() {
        use std::os::unix::{fs::symlink, net::UnixListener};
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let target = SkillService::get_ssot_dir().unwrap().join("demo");
        let empty = home.path().join("empty");
        fs::create_dir_all(&empty).unwrap();
        fs::create_dir_all(&target).unwrap();
        symlink(&empty, target.join("link")).unwrap();
        assert!(require_file_empty_target(&target, "conflict").is_err());
        fs::remove_file(target.join("link")).unwrap();
        let _socket = UnixListener::bind(target.join("socket")).unwrap();
        assert!(require_file_empty_target(&target, "conflict").is_err());
    }

    #[test]
    #[cfg(windows)]
    fn local_empty_target_rejects_junctions() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let target = SkillService::get_ssot_dir().unwrap().join("demo");
        let empty = home.path().join("empty");
        fs::create_dir_all(&empty).unwrap();
        fs::create_dir_all(&target).unwrap();
        let linked = target.join("link");
        let status = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&linked)
            .arg(&empty)
            .output()
            .unwrap();
        assert!(status.status.success(), "junction fixture failed");
        assert!(require_file_empty_target(&target, "conflict").is_err());
        fs::remove_dir(&linked).unwrap();
    }

    #[test]
    fn local_install_updates_pi_copy_and_preserves_user_edits() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let _pi = crate::pi_config::test_support::TestAgentDir::new();
        let input = source(home.path(), "first");
        install(&input, 'a').unwrap();
        let pi = SkillService::get_app_skills_dir(&AppType::Pi)
            .unwrap()
            .join("demo");
        fs::create_dir_all(&pi).unwrap();
        fs::copy(input.join("SKILL.md"), pi.join("SKILL.md")).unwrap();
        assert_eq!(
            SkillService::local_info("demo").unwrap()["skill"]["apps"]["pi"],
            true
        );
        source(home.path(), "second");
        assert_eq!(install(&input, 'b').unwrap()["skill"]["apps"]["pi"], true);
        assert_eq!(
            fs::read(pi.join("SKILL.md")).unwrap(),
            fs::read(input.join("SKILL.md")).unwrap()
        );
        fs::write(pi.join("SKILL.md"), "user edit").unwrap();
        source(home.path(), "third");
        let ssot = SkillService::get_ssot_dir().unwrap().join("demo/SKILL.md");
        let before = fs::read(&ssot).unwrap();
        assert!(install(&input, 'c')
            .unwrap_err()
            .to_string()
            .contains("local edits"));
        assert_eq!(fs::read(pi.join("SKILL.md")).unwrap(), b"user edit");
        assert_eq!(fs::read(ssot).unwrap(), before);
        assert!(
            SkillService::local_info("demo").unwrap()["skill"]["readmeUrl"]
                .as_str()
                .unwrap()
                .contains(&"b".repeat(40))
        );
    }

    #[test]
    fn local_install_update_preserves_apps_other_skills_and_bytes() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let input = source(home.path(), "first");
        assert_eq!(install(&input, 'a').unwrap()["action"], "installed");
        SkillService::toggle_app("demo", &AppType::Claude, true).unwrap();
        let untouched = SkillService::get_ssot_dir().unwrap().join("unrelated");
        fs::create_dir_all(&untouched).unwrap();
        fs::write(untouched.join("keep.bin"), [0, 255, 13, 10]).unwrap();
        source(home.path(), "second");
        fs::write(input.join("raw.bin"), [0, 255, 13, 10]).unwrap();
        let result = install(&input, 'b').unwrap();
        assert_eq!(result["action"], "updated");
        assert_eq!(result["skill"]["apps"]["claude"], true);
        assert_eq!(result["skill"]["apps"]["codex"], true);
        assert_eq!(
            fs::read(untouched.join("keep.bin")).unwrap(),
            [0, 255, 13, 10]
        );
        for app in [AppType::Codex, AppType::Claude] {
            assert_eq!(
                fs::read(
                    SkillService::get_app_skills_dir(&app)
                        .unwrap()
                        .join("demo/raw.bin")
                )
                .unwrap(),
                [0, 255, 13, 10]
            );
        }
        assert_eq!(install(&input, 'b').unwrap()["action"], "unchanged");
    }

    fn shared_app_directory_update(aliased_parent: bool) {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let shared = home.path().join("shared/config");
        fs::create_dir_all(&shared).unwrap();
        let claude = if aliased_parent {
            #[cfg(unix)]
            {
                let alias = home.path().join("alias");
                std::os::unix::fs::symlink(home.path().join("shared"), &alias).unwrap();
                alias.join("config")
            }
            #[cfg(not(unix))]
            {
                unreachable!("parent alias fixture is Unix-only")
            }
        } else {
            shared.clone()
        };
        std::env::set_var("CLAUDE_CONFIG_DIR", &claude);
        let mut settings = crate::settings::get_settings();
        settings.codex_config_dir = Some(shared.to_string_lossy().into_owned());
        crate::settings::update_settings(settings).unwrap();
        let input = source(home.path(), "first");
        fs::write(input.join("obsolete.bin"), [1, 2, 3]).unwrap();
        install(&input, 'a').unwrap();
        let db = Database::init().unwrap();
        let mut row = db.get_installed_skill("local:demo").unwrap().unwrap();
        row.apps.claude = true;
        db.save_skill(&row).unwrap();

        source(home.path(), "second");
        fs::remove_file(input.join("obsolete.bin")).unwrap();
        fs::write(input.join(".private"), [0, 255, 13, 10]).unwrap();
        let result = install(&input, 'b').unwrap();
        assert_eq!(result["action"], "updated");
        assert_eq!(result["skill"]["apps"]["claude"], true);
        assert_eq!(result["skill"]["apps"]["codex"], true);
        for target in [
            SkillService::get_ssot_dir().unwrap().join("demo"),
            shared.join("skills/demo"),
        ] {
            assert_eq!(
                local_tree_hash(&target).unwrap(),
                local_tree_hash(&input).unwrap()
            );
            assert!(!target.join("obsolete.bin").exists());
        }
        assert_eq!(install(&input, 'b').unwrap()["action"], "unchanged");
        let before = SkillService::local_info("demo").unwrap();
        fs::write(shared.join("skills/demo/.private"), "user edit").unwrap();
        source(home.path(), "third");
        assert!(install(&input, 'c')
            .unwrap_err()
            .to_string()
            .contains("local edits"));
        assert_eq!(SkillService::local_info("demo").unwrap(), before);
        assert_eq!(
            fs::read(shared.join("skills/demo/.private")).unwrap(),
            b"user edit"
        );
    }

    #[test]
    fn local_install_updates_shared_claude_codex_directory_once() {
        shared_app_directory_update(false);
    }

    #[test]
    #[cfg(unix)]
    fn local_install_deduplicates_shared_directory_through_parent_alias() {
        shared_app_directory_update(true);
    }

    #[test]
    fn local_install_rejects_nested_app_targets_before_mutation() {
        // Exercise both ancestor orders in supported_skill_apps().
        for claude_is_outer in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let _env = crate::test_support::TestEnvGuard::isolated(home.path());
            let input = source(home.path(), "first");
            install(&input, 'a').unwrap();
            let outer_config = crate::codex_config::get_codex_config_dir();
            let outer = outer_config.join("skills/demo");
            let inner_config = outer.join("nested");
            fs::create_dir_all(&inner_config).unwrap();
            let inner = inner_config.join("skills/demo");
            let (claude, codex) = if claude_is_outer {
                (&outer_config, &inner_config)
            } else {
                (&inner_config, &outer_config)
            };
            std::env::set_var("CLAUDE_CONFIG_DIR", claude);
            let mut settings = crate::settings::get_settings();
            settings.codex_config_dir = Some(codex.to_string_lossy().into_owned());
            crate::settings::update_settings(settings).unwrap();
            let db = Database::init().unwrap();
            let mut row = db.get_installed_skill("local:demo").unwrap().unwrap();
            row.apps.claude = true;
            db.save_skill(&row).unwrap();
            let before = SkillService::local_info("demo").unwrap();
            let ssot = SkillService::get_ssot_dir().unwrap().join("demo");
            let ssot_before = local_tree_hash(&ssot).unwrap();
            let outer_before = local_tree_hash(&outer).unwrap();
            source(home.path(), "second");

            let error = install(&input, 'b').unwrap_err().to_string();
            assert!(error.contains("targets cannot overlap"), "{error}");
            assert_eq!(local_tree_hash(&ssot).unwrap(), ssot_before);
            assert_eq!(local_tree_hash(&outer).unwrap(), outer_before);
            assert!(!inner.exists());
            assert_eq!(SkillService::local_info("demo").unwrap(), before);
        }
    }

    #[test]
    fn local_schema20_update_preserves_pi_mcp_unknown_skill_fields_and_settings() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let input = source(home.path(), "first");
        install(&input, 'a').unwrap();
        {
            let db = Database::init().unwrap();
            let conn = db.conn.lock().unwrap();
            assert_eq!(Database::get_user_version(&conn).unwrap(), 20);
            conn.execute_batch("INSERT INTO mcp_servers(id,name,server_config,enabled_codex,enabled_mcode,enabled_pi) VALUES('pi-sentinel','Pi sentinel','{}',1,1,1); ALTER TABLE skills ADD COLUMN schema20_extra TEXT; UPDATE skills SET enabled_mcode=1,schema20_extra='keep'; INSERT INTO settings(key,value) VALUES('schema20_sentinel','keep');").unwrap();
        }
        assert_eq!(SkillService::local_info("demo").unwrap()["found"], true);
        source(home.path(), "second");
        install(&input, 'b').unwrap();
        let db = Database::init().unwrap();
        let conn = db.conn.lock().unwrap();
        let mcp: (i64,i64,i64) = conn.query_row("SELECT enabled_codex,enabled_mcode,enabled_pi FROM mcp_servers WHERE id='pi-sentinel'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(mcp, (1, 1, 1));
        let skill: (i64, String) = conn
            .query_row(
                "SELECT enabled_mcode,schema20_extra FROM skills WHERE directory='demo'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(skill, (1, "keep".into()));
        assert_eq!(
            conn.query_row(
                "SELECT value FROM settings WHERE key='schema20_sentinel'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "keep"
        );
    }

    #[test]
    fn local_install_rejects_foreign_source_and_user_edits() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let input = source(home.path(), "first");
        install(&input, 'a').unwrap();
        let db = Database::init().unwrap();
        let mut record = db.get_installed_skill("local:demo").unwrap().unwrap();
        record.readme_url = Some(format!(
            "https://github.com/other/repo/blob/{}/private-skills/demo/SKILL.md",
            "a".repeat(40)
        ));
        db.save_skill(&record).unwrap();
        assert!(install(&input, 'b')
            .unwrap_err()
            .to_string()
            .contains("Foreign"));
        record.readme_url = Some(
            provenance(
                "owner/private",
                "private-skills/demo",
                &"a".repeat(40),
                "demo",
            )
            .unwrap(),
        );
        db.save_skill(&record).unwrap();
        fs::write(
            SkillService::get_app_skills_dir(&AppType::Codex)
                .unwrap()
                .join("demo/SKILL.md"),
            "user edit",
        )
        .unwrap();
        assert!(install(&input, 'b')
            .unwrap_err()
            .to_string()
            .contains("local edits"));
    }
    #[test]
    fn local_info_missing_does_not_initialize_database() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        assert_eq!(SkillService::local_info("demo").unwrap()["found"], false);
        assert!(!home.path().join(".cc-switch/cc-switch.db").exists());
    }
    #[test]
    fn local_install_rejects_bad_structure_and_identity() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let input = source(home.path(), "first");
        assert!(SkillService::install_local(
            &input,
            "../demo",
            "owner/private",
            "private-skills/demo",
            &"a".repeat(40),
            &AppType::Codex
        )
        .is_err());
        fs::write(input.join("SKILL.md"), "no metadata").unwrap();
        assert!(install(&input, 'a').is_err());
    }
    #[test]
    fn local_install_protects_hidden_user_edits() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let input = source(home.path(), "first");
        fs::write(input.join(".private"), "original").unwrap();
        install(&input, 'a').unwrap();
        let ssot = SkillService::get_ssot_dir().unwrap().join("demo");
        fs::write(ssot.join(".private"), "user edit").unwrap();
        assert!(install(&input, 'b')
            .unwrap_err()
            .to_string()
            .contains("local edits"));
        assert_eq!(
            fs::read_to_string(ssot.join(".private")).unwrap(),
            "user edit"
        );
    }
    fn concurrent_edit(change_source: bool) {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let input = source(home.path(), "first");
        install(&input, 'a').unwrap();
        source(home.path(), "second");
        let result = SkillService::install_local_checked(
            &input,
            "demo",
            "owner/private",
            "private-skills/demo",
            &"b".repeat(40),
            &AppType::Codex,
            |db| {
                let mut row = db.get_installed_skill("local:demo")?.unwrap();
                if change_source {
                    row.readme_url = Some("external source edit".to_string());
                } else {
                    row.apps.claude = true;
                }
                db.save_skill(&row)
            },
        );
        assert!(result.unwrap_err().to_string().contains("Concurrent"));
        let row = Database::init()
            .unwrap()
            .get_installed_skill("local:demo")
            .unwrap()
            .unwrap();
        if change_source {
            assert_eq!(row.readme_url.as_deref(), Some("external source edit"));
        } else {
            assert!(row.apps.claude);
        }
        assert!(
            fs::read_to_string(SkillService::get_ssot_dir().unwrap().join("demo/SKILL.md"))
                .unwrap()
                .contains("first")
        );
        assert!(fs::read_to_string(
            SkillService::get_app_skills_dir(&AppType::Codex)
                .unwrap()
                .join("demo/SKILL.md")
        )
        .unwrap()
        .contains("first"));
    }
    #[test]
    fn local_install_preserves_concurrent_app_edit_and_rolls_back_files() {
        concurrent_edit(false);
    }
    #[test]
    fn local_install_preserves_concurrent_source_edit_and_rolls_back_files() {
        concurrent_edit(true);
    }
    #[test]
    #[cfg(windows)]
    fn local_install_rejects_junction_source() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let input = source(home.path(), "first");
        let linked = home.path().join("linked-source");
        // Windows junctions do not require developer mode or symlink privileges.
        let status = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&linked)
            .arg(&input)
            .output()
            .unwrap();
        assert!(status.status.success(), "junction fixture failed");
        assert!(install(&linked, 'a')
            .unwrap_err()
            .to_string()
            .contains("Reparse"));
        fs::remove_dir(&linked).unwrap();
    }
    #[test]
    #[cfg(windows)]
    fn local_install_locked_app_rolls_back_ssot_and_database() {
        use std::os::windows::fs::OpenOptionsExt;
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let input = source(home.path(), "first");
        install(&input, 'a').unwrap();
        let dest = SkillService::get_app_skills_dir(&AppType::Codex)
            .unwrap()
            .join("demo/SKILL.md");
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&dest)
            .unwrap();
        source(home.path(), "second");
        assert!(install(&input, 'b').is_err());
        drop(held);
        assert!(fs::read_to_string(&dest).unwrap().contains("first"));
        assert!(
            fs::read_to_string(SkillService::get_ssot_dir().unwrap().join("demo/SKILL.md"))
                .unwrap()
                .contains("first")
        );
        let record = Database::init()
            .unwrap()
            .get_installed_skill("local:demo")
            .unwrap()
            .unwrap();
        assert!(record.readme_url.unwrap().contains(&"a".repeat(40)));
    }
}
