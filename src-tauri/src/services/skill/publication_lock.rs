//! Serializes complete Skill publications across cc-switch processes.

use std::fs::{self, File, Metadata, OpenOptions};
use std::path::Path;

use crate::config::{create_managed_config_dir_all, get_app_config_dir};
use crate::error::AppError;

const LOCK_FILE_NAME: &str = "skills-publication.lock";

pub(crate) struct PublicationLock {
    file: File,
}

impl Drop for PublicationLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
        // Keep the file: unlinking it lets a new caller lock a different inode
        // while another process is still waiting on the original one.
    }
}

/// Caller holds the process Skill state write guard, before taking any DB lock.
/// Keep this guard through baseline capture, apply, commit, rollback and cleanup.
/// This lock is not reentrant; helpers reuse their entry point's guard.
pub(crate) fn acquire() -> Result<PublicationLock, AppError> {
    let config_dir = get_app_config_dir();
    create_managed_config_dir_all(&config_dir)?;
    acquire_at(&config_dir.join(LOCK_FILE_NAME))
}

// Explicit paths keep lock tests independent of configuration, settings and DBs.
fn acquire_at(path: &Path) -> Result<PublicationLock, AppError> {
    let file = open_lock_file(path)?;
    file.lock().map_err(|error| AppError::io(path, error))?;
    Ok(PublicationLock { file })
}

fn validate_lock_metadata(path: &Path, metadata: &Metadata) -> Result<(), AppError> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(AppError::InvalidInput(format!(
            "Skill publication lock must be a regular file, not a link: {}",
            path.display()
        )));
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(AppError::InvalidInput(format!(
                "Skill publication lock must not be a reparse point: {}",
                path.display()
            )));
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if metadata.nlink() != 1 {
            return Err(AppError::InvalidInput(format!(
                "Skill publication lock must not have hard links: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn open_lock_file(path: &Path) -> Result<File, AppError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => validate_lock_metadata(path, &metadata)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(AppError::io(path, error)),
    }

    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        // NONBLOCK prevents a swapped-in FIFO from hanging before validation.
        options
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };

        // Inspect the link itself, even if it replaced a checked regular file.
        // Deny delete sharing so the lock file cannot be replaced while open.
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    }
    let file = options
        .open(path)
        .map_err(|error| AppError::io(path, error))?;
    let metadata = file.metadata().map_err(|error| AppError::io(path, error))?;
    validate_lock_metadata(path, &metadata)?;

    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };

        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        // SAFETY: file owns a live handle; the out pointer is valid and only
        // read after the API reports success.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } == 0 {
            return Err(AppError::io(path, std::io::Error::last_os_error()));
        }
        // SAFETY: the successful call initialized the complete structure.
        if unsafe { info.assume_init() }.nNumberOfLinks != 1 {
            return Err(AppError::InvalidInput(format!(
                "Skill publication lock must not have hard links: {}",
                path.display()
            )));
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| AppError::io(path, error))?;
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    const CHILD_TEST: &str = "services::skill::publication_lock::tests::publication_lock_child";
    const CHILD_DIR: &str = "CC_SWITCH_TEST_SKILL_PUBLICATION_LOCK_DIR";
    const CHILD_MODE: &str = "CC_SWITCH_TEST_SKILL_PUBLICATION_LOCK_MODE";

    // Kill and reap on assertion failure as well as success. A lock regression
    // must fail with a timeout instead of leaving a blocked test process behind.
    struct LockChild(Child);

    impl LockChild {
        fn spawn(dir: &Path, mode: &str) -> Self {
            Self(
                Command::new(std::env::current_exe().expect("current test executable"))
                    .args(["--exact", CHILD_TEST, "--test-threads=1", "--nocapture"])
                    .env(CHILD_DIR, dir)
                    .env(CHILD_MODE, mode)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .spawn()
                    .expect("spawn publication lock child"),
            )
        }

        fn wait_for_marker(&mut self, marker: &Path) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !marker.exists() {
                assert!(
                    self.0.try_wait().expect("poll child").is_none(),
                    "lock child exited before creating {}",
                    marker.display()
                );
                assert!(Instant::now() < deadline, "lock child marker timed out");
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        fn wait_success(&mut self) {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(status) = self.0.try_wait().expect("poll child") {
                    assert!(status.success(), "lock child failed: {status}");
                    return;
                }
                assert!(Instant::now() < deadline, "lock child exit timed out");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    impl Drop for LockChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn publication_lock_child() -> Result<(), AppError> {
        let Some(dir) = std::env::var_os(CHILD_DIR) else {
            return Ok(());
        };
        let dir = std::path::PathBuf::from(dir);
        assert!(
            dir.is_absolute(),
            "child requires an explicit temporary path"
        );
        let path = dir.join(LOCK_FILE_NAME);
        let mode = std::env::var(CHILD_MODE).expect("child lock mode");
        if mode == "contend" {
            let file = open_lock_file(&path)?;
            assert!(
                matches!(file.try_lock(), Err(fs::TryLockError::WouldBlock)),
                "child bypassed the parent's OS lock"
            );
            drop(file);
            fs::write(dir.join("blocked"), b"1").map_err(|error| AppError::io(&dir, error))?;
        } else {
            assert!(matches!(mode.as_str(), "acquire" | "exit"));
        }
        let _guard = acquire_at(&path)?;
        fs::write(dir.join(&mode), b"1").map_err(|error| AppError::io(&dir, error))?;
        if mode == "exit" {
            // Deliberately skip Rust destructors to exercise OS release on exit.
            std::process::exit(0);
        }
        Ok(())
    }

    #[test]
    fn publication_lock_blocks_another_process_until_guard_drops() {
        let temp = tempfile::tempdir().expect("temporary lock directory");
        let path = temp.path().join(LOCK_FILE_NAME);
        fs::write(&path, b"stable lock file").unwrap();
        let guard = acquire_at(&path).unwrap();
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            let metadata = fs::metadata(&path).unwrap();
            (metadata.dev(), metadata.ino())
        };
        let mut child = LockChild::spawn(temp.path(), "contend");
        child.wait_for_marker(&temp.path().join("blocked"));
        assert!(!temp.path().join("contend").exists());
        drop(guard);
        child.wait_success();
        assert!(temp.path().join("contend").exists());
        assert_eq!(fs::read(&path).unwrap(), b"stable lock file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = fs::metadata(&path).unwrap();
            assert_eq!((metadata.dev(), metadata.ino()), identity);
        }
    }

    #[test]
    fn publication_lock_is_released_on_error() {
        fn fail_after_lock(path: &Path) -> Result<(), AppError> {
            let _guard = acquire_at(path)?;
            Err(AppError::Message("publication rejected".into()))
        }

        let temp = tempfile::tempdir().expect("temporary lock directory");
        assert!(fail_after_lock(&temp.path().join(LOCK_FILE_NAME)).is_err());
        let mut child = LockChild::spawn(temp.path(), "acquire");
        child.wait_success();
        assert!(temp.path().join("acquire").exists());
    }

    #[test]
    fn publication_lock_is_released_when_holder_process_exits() {
        let temp = tempfile::tempdir().expect("temporary lock directory");
        let mut holder = LockChild::spawn(temp.path(), "exit");
        holder.wait_success();
        assert!(temp.path().join("exit").exists());
        assert!(temp.path().join(LOCK_FILE_NAME).is_file());
        let mut next = LockChild::spawn(temp.path(), "acquire");
        next.wait_success();
        assert!(temp.path().join("acquire").exists());
    }

    #[test]
    fn publication_lock_rejects_directories_and_hard_links() {
        let temp = tempfile::tempdir().expect("temporary lock directory");
        let directory = temp.path().join("directory.lock");
        fs::create_dir(&directory).unwrap();
        assert!(acquire_at(&directory).is_err());
        let target = temp.path().join("target");
        fs::write(&target, b"untouched").unwrap();
        let link = temp.path().join("hardlink.lock");
        fs::hard_link(&target, &link).unwrap();
        assert!(acquire_at(&link).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"untouched");
    }

    #[cfg(unix)]
    #[test]
    fn publication_lock_rejects_symlinks_and_special_files() {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temporary lock directory");
        let target = temp.path().join("target");
        fs::write(&target, b"untouched").unwrap();
        let link = temp.path().join("symlink.lock");
        symlink(&target, &link).unwrap();
        assert!(acquire_at(&link).is_err());
        let dangling = temp.path().join("dangling.lock");
        symlink(temp.path().join("missing"), &dangling).unwrap();
        assert!(acquire_at(&dangling).is_err());
        let fifo = temp.path().join("fifo.lock");
        let fifo_c = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: fifo_c is a valid, NUL-terminated temporary path.
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
        assert!(acquire_at(&fifo).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"untouched");
        assert!(!temp.path().join("missing").exists());
    }

    #[cfg(windows)]
    #[test]
    fn publication_lock_rejects_symlinks() {
        let temp = tempfile::tempdir().expect("temporary lock directory");
        let target = temp.path().join("target");
        fs::write(&target, b"untouched").unwrap();
        let link = temp.path().join("symlink.lock");
        match std::os::windows::fs::symlink_file(&target, &link) {
            Ok(()) => assert!(acquire_at(&link).is_err()),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return,
            Err(error) => panic!("create temporary symlink: {error}"),
        }
        assert_eq!(fs::read(&target).unwrap(), b"untouched");
    }
}
