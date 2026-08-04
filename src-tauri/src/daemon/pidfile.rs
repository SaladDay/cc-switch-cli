//! Pidfile management for the supervisor daemon.
//!
//! - The pidfile lives at `daemon::paths::pidfile_path()`.
//! - Acquiring it grabs a non-blocking exclusive flock; if another daemon is
//!   already holding the lock we return `AlreadyHeld` so the caller can exit
//!   gracefully.
//! - The lock is held for the lifetime of the returned `PidFile` value; the
//!   kernel releases the flock when the file descriptor is closed (process
//!   exit, panic, drop), so even an `abort()` cleans up automatically.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum AcquireError {
    /// Another daemon already holds the lock.
    AlreadyHeld { pid: Option<u32> },
    /// Filesystem or syscall error.
    Io(std::io::Error),
}

impl std::fmt::Display for AcquireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyHeld { pid: Some(p) } => {
                write!(f, "another cc-switch daemon is already running (pid {p})")
            }
            Self::AlreadyHeld { pid: None } => {
                write!(f, "another cc-switch daemon is already running")
            }
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for AcquireError {}

#[derive(Debug)]
pub struct PidFile {
    file: File,
    path: PathBuf,
}

impl PidFile {
    pub fn acquire(path: impl AsRef<Path>) -> Result<Self, AcquireError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            crate::daemon::paths::ensure_private_runtime_dir(parent).map_err(AcquireError::Io)?;
        }

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&path)
            .map_err(AcquireError::Io)?;

        let metadata = file.metadata().map_err(AcquireError::Io)?;
        if !metadata.is_file() {
            return Err(AcquireError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("daemon pidfile is not a regular file: {}", path.display()),
            )));
        }
        if metadata.nlink() != 1 {
            return Err(AcquireError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("daemon pidfile must not be hard-linked: {}", path.display()),
            )));
        }
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(AcquireError::Io)?;

        flock_exclusive_nonblock(&file).map_err(|err| {
            if err.kind() == std::io::ErrorKind::WouldBlock {
                let pid = read_pid(&file);
                AcquireError::AlreadyHeld { pid }
            } else {
                AcquireError::Io(err)
            }
        })?;

        // We own the lock — write our pid (truncate first so a stale longer
        // value doesn't bleed through).
        let mut writer = &file;
        writer.set_len(0).map_err(AcquireError::Io)?;
        let pid_text = format!("{}\n", std::process::id());
        writer
            .write_all(pid_text.as_bytes())
            .map_err(AcquireError::Io)?;
        writer.flush().map_err(AcquireError::Io)?;

        Ok(Self { file, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PidFile {
    fn drop(&mut self) {
        // Kernel releases the flock when the fd closes; we only need to remove
        // the on-disk file so a fresh daemon doesn't see a leftover pid number.
        let _ = std::fs::remove_file(&self.path);
        // file is dropped after this, releasing the flock.
        let _ = &self.file;
    }
}

fn flock_exclusive_nonblock(file: &File) -> std::io::Result<()> {
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn read_pid(file: &File) -> Option<u32> {
    let mut reader = file.try_clone().ok()?;
    reader.rewind().ok()?;
    let mut value = String::new();
    reader.read_to_string(&mut value).ok()?;
    value.trim().parse::<u32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_writes_current_pid_and_releases_on_drop() {
        let tmp = tempfile::tempdir().expect("tmp");
        let pidfile_path = tmp.path().join("daemon.pid");

        let lock = PidFile::acquire(&pidfile_path).expect("acquire");
        let contents = std::fs::read_to_string(&pidfile_path).expect("read pidfile");
        assert_eq!(
            contents.trim().parse::<u32>().ok(),
            Some(std::process::id())
        );

        drop(lock);
        assert!(!pidfile_path.exists(), "pidfile should be removed on drop");
    }

    #[test]
    fn second_acquire_returns_already_held_with_pid() {
        let tmp = tempfile::tempdir().expect("tmp");
        let pidfile_path = tmp.path().join("daemon.pid");

        let _first = PidFile::acquire(&pidfile_path).expect("first acquire");
        let err = PidFile::acquire(&pidfile_path).expect_err("second acquire should fail");
        match err {
            AcquireError::AlreadyHeld { pid } => {
                assert_eq!(pid, Some(std::process::id()));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn second_acquire_succeeds_after_first_drops() {
        let tmp = tempfile::tempdir().expect("tmp");
        let pidfile_path = tmp.path().join("daemon.pid");

        let first = PidFile::acquire(&pidfile_path).expect("first");
        drop(first);

        let second = PidFile::acquire(&pidfile_path).expect("second after release");
        drop(second);
    }

    #[test]
    fn acquire_rejects_symlink_and_hardlink_pidfiles() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().expect("tmp");
        let target = tmp.path().join("target");
        std::fs::write(&target, b"sentinel").expect("seed target");

        let symlink_path = tmp.path().join("symlink.pid");
        symlink(&target, &symlink_path).expect("create symlink");
        assert!(matches!(
            PidFile::acquire(&symlink_path),
            Err(AcquireError::Io(_))
        ));

        let hardlink_path = tmp.path().join("hardlink.pid");
        std::fs::hard_link(&target, &hardlink_path).expect("create hardlink");
        assert!(matches!(
            PidFile::acquire(&hardlink_path),
            Err(AcquireError::Io(_))
        ));
        assert_eq!(std::fs::read(&target).expect("read target"), b"sentinel");
    }
}
