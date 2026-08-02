//! Same-volume Skills staging and crash recovery for cloud restores.
//!
//! The database remains the commit oracle. A candidate Skills tree is made
//! durable before it is installed, and every recovery path is derived from a
//! locally generated operation UUID rather than a journal-supplied path.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::AppError;
use crate::restore_protocol::{RestoreOperationId, SKILLS_GENERATION_MARKER};

use super::webdav_sync::archive::extract_skills_zip_into;

#[derive(Debug)]
struct SkillsRestorePaths {
    config_root: PathBuf,
    operation_root: PathBuf,
    staged: PathBuf,
    old: PathBuf,
    live: PathBuf,
}

impl SkillsRestorePaths {
    fn for_operation(operation: RestoreOperationId) -> Result<Self, AppError> {
        crate::config::validate_config_dir()?;
        let config_root = crate::config::resolve_config_dir_without_following_user_symlinks(
            &crate::config::get_app_config_dir(),
        )?;
        validate_existing_directory(&config_root, "CC-Switch configuration")?;
        let operation_root = config_root.join(".restore").join(operation.to_string());
        Ok(Self {
            staged: operation_root.join("skills.new"),
            old: operation_root.join("skills.old"),
            live: config_root.join("skills"),
            config_root,
            operation_root,
        })
    }

    fn marker_in(directory: &Path) -> PathBuf {
        directory.join(SKILLS_GENERATION_MARKER)
    }
}

pub(crate) struct PreparedSkillsRestore {
    operation: RestoreOperationId,
    paths: SkillsRestorePaths,
}

pub(crate) struct InstalledSkillsRestore {
    operation: RestoreOperationId,
}

impl PreparedSkillsRestore {
    pub(crate) fn prepare(
        operation: RestoreOperationId,
        raw_zip: &[u8],
        expected_directories: &BTreeSet<String>,
    ) -> Result<Self, AppError> {
        let paths = SkillsRestorePaths::for_operation(operation)?;
        prepare_operation_directory(&paths)?;

        let result = (|| {
            let payload_directories = extract_skills_zip_into(raw_zip, &paths.staged)?;
            require_exact_skill_payload(expected_directories, &payload_directories)?;
            write_generation_marker(&paths.staged, operation)?;
            sync_tree(&paths.staged)?;
            preflight_same_volume(&paths, operation)?;
            Ok(())
        })();
        if let Err(error) = result {
            cleanup_operation_root(&paths.operation_root);
            return Err(error);
        }

        Ok(Self { operation, paths })
    }

    /// Install the staged tree using rename only. The caller must have already
    /// persisted the old-live database intent before invoking this method.
    pub(crate) fn install(self) -> Result<InstalledSkillsRestore, AppError> {
        validate_live_skills_path(&self.paths.live)?;
        if self.paths.old.exists() {
            return Err(AppError::InvalidInput(format!(
                "Skills restore old-tree path already exists: {}",
                self.paths.old.display()
            )));
        }

        let moved_old = if self.paths.live.exists() {
            fs::rename(&self.paths.live, &self.paths.old)
                .map_err(|error| rename_error("move live Skills aside", &self.paths.live, error))?;
            sync_directory(&self.paths.config_root)?;
            true
        } else {
            false
        };

        if let Err(error) = fs::rename(&self.paths.staged, &self.paths.live) {
            if moved_old {
                if let Err(rollback_error) = fs::rename(&self.paths.old, &self.paths.live) {
                    return Err(AppError::Message(format!(
                        "install staged Skills failed: {error}; restoring old Skills also failed: {rollback_error}"
                    )));
                }
                let _ = sync_directory(&self.paths.config_root);
            }
            return Err(rename_error(
                "install staged Skills",
                &self.paths.staged,
                error,
            ));
        }
        sync_directory(&self.paths.config_root)?;

        Ok(InstalledSkillsRestore {
            operation: self.operation,
        })
    }
}

impl InstalledSkillsRestore {
    pub(crate) fn rollback(self) -> Result<(), AppError> {
        rollback_unpublished_skills(self.operation)
    }

    pub(crate) fn finalize(self) -> Result<(), AppError> {
        finalize_published_skills(self.operation)
    }
}

pub(crate) fn rollback_unpublished_skills(operation: RestoreOperationId) -> Result<(), AppError> {
    let paths = SkillsRestorePaths::for_operation(operation)?;
    if !paths.operation_root.exists() {
        return Ok(());
    }
    validate_operation_root(&paths)?;

    let operation_text = operation.to_string();
    let live_marker = read_generation_marker(&paths.live)?;
    let live_is_candidate = live_marker.as_deref() == Some(operation_text.as_str());
    if live_marker.is_some() && !live_is_candidate {
        return Err(AppError::InvalidInput(format!(
            "refusing to roll back Skills owned by another restore generation at {}",
            paths.live.display()
        )));
    }

    if paths.old.exists() {
        validate_existing_directory(&paths.old, "old Skills restore tree")?;
        if paths.live.exists() {
            if !live_is_candidate {
                return Err(AppError::InvalidInput(format!(
                    "refusing to replace unmarked live Skills during rollback: {}",
                    paths.live.display()
                )));
            }
            if paths.staged.exists() {
                return Err(AppError::InvalidInput(format!(
                    "Skills rollback staging collision: {}",
                    paths.staged.display()
                )));
            }
            fs::rename(&paths.live, &paths.staged).map_err(|error| {
                rename_error("move unpublished Skills aside", &paths.live, error)
            })?;
        }
        fs::rename(&paths.old, &paths.live)
            .map_err(|error| rename_error("restore old Skills", &paths.old, error))?;
        sync_directory(&paths.config_root)?;
    } else if paths.live.exists() && live_is_candidate {
        if paths.staged.exists() {
            return Err(AppError::InvalidInput(format!(
                "Skills rollback staging collision: {}",
                paths.staged.display()
            )));
        }
        fs::rename(&paths.live, &paths.staged)
            .map_err(|error| rename_error("remove unpublished Skills", &paths.live, error))?;
        sync_directory(&paths.config_root)?;
    }

    cleanup_operation_root_checked(&paths.operation_root)
}

pub(crate) fn finalize_published_skills(operation: RestoreOperationId) -> Result<(), AppError> {
    let paths = SkillsRestorePaths::for_operation(operation)?;
    let operation_text = operation.to_string();
    let marker = read_generation_marker(&paths.live)?;
    if marker.as_deref() != Some(operation_text.as_str()) {
        return Err(AppError::InvalidInput(format!(
            "published Skills generation marker is missing or stale for operation {operation}"
        )));
    }
    cleanup_operation_root_checked(&paths.operation_root)
}

pub(crate) fn operation_staging_exists(operation: RestoreOperationId) -> Result<bool, AppError> {
    let paths = SkillsRestorePaths::for_operation(operation)?;
    match fs::symlink_metadata(&paths.operation_root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(AppError::InvalidInput(format!(
                "restore operation path is not a plain directory: {}",
                paths.operation_root.display()
            )))
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(AppError::io(&paths.operation_root, error)),
    }
}

fn prepare_operation_directory(paths: &SkillsRestorePaths) -> Result<(), AppError> {
    let restore_root = paths
        .operation_root
        .parent()
        .expect("operation root always has .restore parent");
    ensure_private_directory(restore_root)?;
    if paths.operation_root.exists() {
        return Err(AppError::InvalidInput(format!(
            "restore operation directory already exists: {}",
            paths.operation_root.display()
        )));
    }
    create_private_directory(&paths.operation_root)
}

pub(crate) fn require_exact_skill_payload(
    expected: &BTreeSet<String>,
    payload: &BTreeSet<String>,
) -> Result<(), AppError> {
    if expected == payload {
        return Ok(());
    }
    let missing = expected.difference(payload).cloned().collect::<Vec<_>>();
    let unexpected = payload.difference(expected).cloned().collect::<Vec<_>>();
    Err(AppError::InvalidInput(format!(
        "Skills database/files mismatch (missing payload: {missing:?}; unexpected payload: {unexpected:?})"
    )))
}

fn write_generation_marker(staged: &Path, operation: RestoreOperationId) -> Result<(), AppError> {
    let marker = SkillsRestorePaths::marker_in(staged);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
        .map_err(|error| AppError::io(&marker, error))?;
    writeln!(file, "{operation}").map_err(|error| AppError::io(&marker, error))?;
    file.sync_all()
        .map_err(|error| AppError::io(&marker, error))
}

fn read_generation_marker(directory: &Path) -> Result<Option<String>, AppError> {
    if !directory.exists() {
        return Ok(None);
    }
    validate_existing_directory(directory, "Skills generation tree")?;
    let marker = SkillsRestorePaths::marker_in(directory);
    match fs::read_to_string(&marker) {
        Ok(value) => {
            let value = value.trim();
            crate::restore_protocol::RestoreOperationId::parse(value)?;
            Ok(Some(value.to_string()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AppError::io(&marker, error)),
    }
}

fn validate_operation_root(paths: &SkillsRestorePaths) -> Result<(), AppError> {
    validate_existing_directory(&paths.operation_root, "restore operation")?;
    for child in [&paths.staged, &paths.old] {
        if child.exists() {
            validate_existing_directory(child, "restore Skills child")?;
        }
    }
    validate_live_skills_path(&paths.live)
}

fn validate_live_skills_path(path: &Path) -> Result<(), AppError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(AppError::InvalidInput(format!(
            "live Skills path cannot be a symbolic link: {}",
            path.display()
        ))),
        Ok(metadata) if !metadata.is_dir() => Err(AppError::InvalidInput(format!(
            "live Skills path is not a directory: {}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppError::io(path, error)),
    }
}

fn validate_existing_directory(path: &Path, label: &str) -> Result<(), AppError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| AppError::io(path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(AppError::InvalidInput(format!(
            "{label} path is not a plain directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn ensure_private_directory(path: &Path) -> Result<(), AppError> {
    match fs::symlink_metadata(path) {
        Ok(_) => validate_existing_directory(path, "managed restore"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_private_directory(path)
        }
        Err(error) => Err(AppError::io(path, error)),
    }
}

fn create_private_directory(path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder
            .create(path)
            .map_err(|error| AppError::io(path, error))
    }
    #[cfg(not(unix))]
    {
        fs::create_dir(path).map_err(|error| AppError::io(path, error))
    }
}

fn preflight_same_volume(
    paths: &SkillsRestorePaths,
    operation: RestoreOperationId,
) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let root_device = fs::metadata(&paths.config_root)
            .map_err(|error| AppError::io(&paths.config_root, error))?
            .dev();
        for path in [&paths.operation_root, &paths.staged] {
            let device = fs::metadata(path)
                .map_err(|error| AppError::io(path, error))?
                .dev();
            if device != root_device {
                return Err(AppError::InvalidInput(format!(
                    "Skills restore staging is on a different filesystem: {}",
                    path.display()
                )));
            }
        }
        if paths.live.exists() {
            let live_device = fs::metadata(&paths.live)
                .map_err(|error| AppError::io(&paths.live, error))?
                .dev();
            if live_device != root_device {
                return Err(AppError::InvalidInput(format!(
                    "live Skills is on a different filesystem: {}",
                    paths.live.display()
                )));
            }
        }
    }

    #[cfg(windows)]
    {
        let root_volume = windows_volume_root(&paths.config_root)?;
        for path in [&paths.operation_root, &paths.staged] {
            if windows_volume_root(path)? != root_volume {
                return Err(AppError::InvalidInput(format!(
                    "Skills restore staging is on a different volume: {}",
                    path.display()
                )));
            }
        }
        if paths.live.exists() && windows_volume_root(&paths.live)? != root_volume {
            return Err(AppError::InvalidInput(format!(
                "live Skills is on a different volume: {}",
                paths.live.display()
            )));
        }
    }

    // Exercise the exact rename primitive before the durable intent exists.
    let probe_source = paths.operation_root.join(".volume-probe");
    let probe_target = paths
        .config_root
        .join(format!(".restore-volume-probe-{operation}"));
    fs::write(&probe_source, b"probe").map_err(|error| AppError::io(&probe_source, error))?;
    fs::rename(&probe_source, &probe_target)
        .map_err(|error| rename_error("preflight Skills volume", &probe_source, error))?;
    if let Err(error) = fs::rename(&probe_target, &probe_source) {
        let _ = fs::remove_file(&probe_target);
        return Err(rename_error(
            "return Skills volume preflight probe",
            &probe_target,
            error,
        ));
    }
    fs::remove_file(&probe_source).map_err(|error| AppError::io(&probe_source, error))?;
    sync_directory(&paths.operation_root)?;
    sync_directory(&paths.config_root)
}

#[cfg(windows)]
fn windows_volume_root(path: &Path) -> Result<PathBuf, AppError> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Storage::FileSystem::GetVolumePathNameW;

    let canonical = path
        .canonicalize()
        .map_err(|error| AppError::io(path, error))?;
    let mut input = canonical.as_os_str().encode_wide().collect::<Vec<_>>();
    input.push(0);
    let mut output = vec![0_u16; 32_768];
    let succeeded =
        unsafe { GetVolumePathNameW(input.as_ptr(), output.as_mut_ptr(), output.len() as u32) };
    if succeeded == 0 {
        return Err(AppError::IoContext {
            context: format!("resolve volume for {}", path.display()),
            source: std::io::Error::last_os_error(),
        });
    }
    let length = output
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(output.len());
    Ok(std::ffi::OsString::from_wide(&output[..length]).into())
}

fn rename_error(action: &str, path: &Path, source: std::io::Error) -> AppError {
    AppError::IoContext {
        context: format!("{action}: {}", path.display()),
        source,
    }
}

fn cleanup_operation_root(path: &Path) {
    if let Err(error) = cleanup_operation_root_checked(path) {
        log::warn!(
            "failed to clean restore operation directory {}: {}",
            path.display(),
            error
        );
    }
}

fn cleanup_operation_root_checked(path: &Path) -> Result<(), AppError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(AppError::InvalidInput(format!(
                "refusing to clean non-directory restore operation path: {}",
                path.display()
            )))
        }
        Ok(_) => fs::remove_dir_all(path).map_err(|error| AppError::io(path, error)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppError::io(path, error)),
    }
}

#[cfg(unix)]
fn sync_tree(path: &Path) -> Result<(), AppError> {
    for entry in fs::read_dir(path).map_err(|error| AppError::io(path, error))? {
        let entry = entry.map_err(|error| AppError::io(path, error))?;
        if entry
            .file_type()
            .map_err(|error| AppError::io(&entry.path(), error))?
            .is_dir()
        {
            sync_tree(&entry.path())?;
        }
    }
    sync_directory(path)
}

#[cfg(not(unix))]
fn sync_tree(_path: &Path) -> Result<(), AppError> {
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), AppError> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| AppError::io(path, error))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), AppError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        finalize_published_skills, rollback_unpublished_skills, PreparedSkillsRestore,
        SkillsRestorePaths,
    };
    use crate::restore_protocol::RestoreOperationId;
    use std::collections::BTreeSet;
    use std::io::Write;

    const OPERATION: &str = "00112233-4455-4677-8899-aabbccddeeff";

    fn one_skill_zip(name: &str, content: &[u8]) -> Vec<u8> {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        writer
            .start_file(
                format!("{name}/SKILL.md"),
                crate::services::webdav_sync::archive::zip_file_options(),
            )
            .expect("start Skill file");
        writer.write_all(content).expect("write Skill file");
        writer.finish().expect("finish Skills ZIP").into_inner()
    }

    #[test]
    #[serial_test::serial(home_settings)]
    fn rollback_uses_old_database_intent_state_to_restore_old_tree() {
        let home = tempfile::tempdir().expect("isolated restore home");
        let _environment = crate::test_support::TestEnvGuard::isolated(home.path());
        let config_root = crate::config::get_app_config_dir();
        std::fs::create_dir_all(&config_root).expect("create config root");
        let live_file = config_root.join("skills/old/SKILL.md");
        std::fs::create_dir_all(live_file.parent().expect("old parent")).expect("create old Skill");
        std::fs::write(&live_file, b"old").expect("write old Skill");

        let operation = RestoreOperationId::for_test(OPERATION);
        let expected = BTreeSet::from(["new".to_string()]);
        let installed =
            PreparedSkillsRestore::prepare(operation, &one_skill_zip("new", b"new"), &expected)
                .expect("prepare Skills")
                .install()
                .expect("install Skills");
        std::mem::forget(installed);

        rollback_unpublished_skills(operation).expect("roll back unpublished Skills");
        assert_eq!(std::fs::read(&live_file).expect("read old Skill"), b"old");
        assert!(!config_root.join("skills/new").exists());
    }

    #[test]
    #[serial_test::serial(home_settings)]
    fn published_generation_keeps_new_tree_and_discards_old_tree() {
        let home = tempfile::tempdir().expect("isolated restore home");
        let _environment = crate::test_support::TestEnvGuard::isolated(home.path());
        let config_root = crate::config::get_app_config_dir();
        std::fs::create_dir_all(config_root.join("skills/old")).expect("create old Skill");
        std::fs::write(config_root.join("skills/old/SKILL.md"), b"old").expect("write old Skill");

        let operation = RestoreOperationId::for_test(OPERATION);
        let expected = BTreeSet::from(["new".to_string()]);
        let installed =
            PreparedSkillsRestore::prepare(operation, &one_skill_zip("new", b"new"), &expected)
                .expect("prepare Skills")
                .install()
                .expect("install Skills");
        std::mem::forget(installed);

        finalize_published_skills(operation).expect("finalize published Skills");
        assert_eq!(
            std::fs::read(config_root.join("skills/new/SKILL.md")).expect("read new Skill"),
            b"new"
        );
        assert!(!SkillsRestorePaths::for_operation(operation)
            .expect("paths")
            .operation_root
            .exists());
    }
}
