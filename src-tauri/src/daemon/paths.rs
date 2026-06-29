use std::path::{Path, PathBuf};

const APP_DIR_NAME: &str = "cc-switch";
#[cfg(unix)]
const PRIVATE_RUNTIME_DIR_MODE: u32 = 0o700;
#[cfg(unix)]
const PRIVATE_RUNTIME_SOCKET_MODE: u32 = 0o600;

pub fn runtime_dir() -> PathBuf {
    runtime_dir_from(
        env_dir("XDG_RUNTIME_DIR"),
        state_dir(),
        env_dir("TMPDIR"),
        current_uid(),
    )
}

pub fn state_dir() -> PathBuf {
    state_dir_from(env_dir("XDG_STATE_HOME"), home_dir(), current_uid())
}

pub fn socket_path() -> PathBuf {
    runtime_dir().join("daemon.sock")
}

pub fn pidfile_path() -> PathBuf {
    runtime_dir().join("daemon.pid")
}

pub fn log_path() -> PathBuf {
    state_dir().join("cc-switchd.log")
}

pub(crate) fn ensure_private_runtime_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    set_private_runtime_dir_permissions(path)
}

pub(crate) fn set_private_runtime_socket_permissions(path: &Path) -> std::io::Result<()> {
    set_private_runtime_socket_permissions_impl(path)
}

fn runtime_dir_from(
    xdg: Option<PathBuf>,
    state: PathBuf,
    tmpdir: Option<PathBuf>,
    uid: u32,
) -> PathBuf {
    if let Some(dir) = xdg {
        return dir.join(APP_DIR_NAME);
    }
    if !state.as_os_str().is_empty() {
        return state.join("runtime");
    }
    if let Some(dir) = tmpdir {
        return dir.join(format!("{APP_DIR_NAME}-{uid}"));
    }
    PathBuf::from("/tmp").join(format!("{APP_DIR_NAME}-{uid}"))
}

fn state_dir_from(xdg: Option<PathBuf>, home: Option<PathBuf>, uid: u32) -> PathBuf {
    if let Some(dir) = xdg {
        return dir.join(APP_DIR_NAME);
    }
    if let Some(home) = home {
        return home.join(".local").join("state").join(APP_DIR_NAME);
    }
    PathBuf::from("/tmp").join(format!("{APP_DIR_NAME}-state-{uid}"))
}

#[cfg(unix)]
fn set_private_runtime_dir_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mode = std::fs::metadata(path)?.permissions().mode() & 0o777;
    if mode != PRIVATE_RUNTIME_DIR_MODE {
        std::fs::set_permissions(
            path,
            std::fs::Permissions::from_mode(PRIVATE_RUNTIME_DIR_MODE),
        )?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_private_runtime_dir_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_runtime_socket_permissions_impl(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(
        path,
        std::fs::Permissions::from_mode(PRIVATE_RUNTIME_SOCKET_MODE),
    )
}

#[cfg(not(unix))]
fn set_private_runtime_socket_permissions_impl(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn env_dir(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

fn home_dir() -> Option<PathBuf> {
    crate::config::home_dir()
}

#[cfg(unix)]
fn current_uid() -> u32 {
    unsafe { libc::getuid() }
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn file_mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;

        std::fs::metadata(path)
            .expect("read metadata")
            .permissions()
            .mode()
            & 0o777
    }

    #[test]
    fn runtime_dir_uses_xdg_runtime_dir_when_set() {
        let dir = runtime_dir_from(
            Some(PathBuf::from("/run/user/1000")),
            PathBuf::from("/state/cc-switch"),
            Some(PathBuf::from("/tmp")),
            1000,
        );
        assert_eq!(dir, PathBuf::from("/run/user/1000/cc-switch"));
    }

    #[test]
    fn runtime_dir_falls_back_to_state_runtime_when_xdg_unset() {
        let dir = runtime_dir_from(
            None,
            PathBuf::from("/Users/u/.local/state/cc-switch"),
            Some(PathBuf::from("/private/tmp")),
            501,
        );
        assert_eq!(
            dir,
            PathBuf::from("/Users/u/.local/state/cc-switch/runtime")
        );
    }

    #[test]
    fn runtime_dir_uses_slash_tmp_when_neither_xdg_nor_state_nor_tmpdir_set() {
        let dir = runtime_dir_from(None, PathBuf::new(), None, 0);
        assert_eq!(dir, PathBuf::from("/tmp/cc-switch-0"));
    }

    #[test]
    fn state_dir_uses_xdg_state_home_when_set() {
        let dir = state_dir_from(
            Some(PathBuf::from("/home/u/.local/state")),
            Some(PathBuf::from("/home/u")),
            1000,
        );
        assert_eq!(dir, PathBuf::from("/home/u/.local/state/cc-switch"));
    }

    #[test]
    fn state_dir_falls_back_to_home_dot_local_state_when_xdg_state_unset() {
        let dir = state_dir_from(None, Some(PathBuf::from("/home/u")), 1000);
        assert_eq!(dir, PathBuf::from("/home/u/.local/state/cc-switch"));
    }

    #[test]
    fn state_dir_falls_back_to_tmp_when_no_home() {
        let dir = state_dir_from(None, None, 42);
        assert_eq!(dir, PathBuf::from("/tmp/cc-switch-state-42"));
    }

    #[test]
    fn socket_pidfile_log_paths_compose_from_resolved_dirs() {
        let runtime = runtime_dir_from(
            Some(PathBuf::from("/run")),
            PathBuf::from("/state/cc-switch"),
            None,
            0,
        );
        let state = state_dir_from(Some(PathBuf::from("/state")), None, 0);
        assert_eq!(
            runtime.join("daemon.sock"),
            PathBuf::from("/run/cc-switch/daemon.sock")
        );
        assert_eq!(
            runtime.join("daemon.pid"),
            PathBuf::from("/run/cc-switch/daemon.pid")
        );
        assert_eq!(
            state.join("cc-switchd.log"),
            PathBuf::from("/state/cc-switch/cc-switchd.log")
        );
    }

    #[cfg(unix)]
    #[test]
    fn ensure_private_runtime_dir_tightens_existing_directory_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().expect("create temp dir");
        let runtime = tmp.path().join("runtime");
        std::fs::create_dir_all(&runtime).expect("create runtime dir");
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755))
            .expect("loosen runtime dir permissions");

        ensure_private_runtime_dir(&runtime).expect("secure runtime dir");

        assert_eq!(file_mode(&runtime), 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn set_private_runtime_socket_permissions_tightens_socket_path() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().expect("create temp dir");
        let socket = tmp.path().join("daemon.sock");
        let mut file = std::fs::File::create(&socket).expect("create placeholder socket file");
        file.write_all(b"placeholder")
            .expect("write placeholder socket file");
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o666))
            .expect("loosen socket permissions");

        set_private_runtime_socket_permissions(&socket).expect("secure socket path");

        assert_eq!(file_mode(&socket), 0o600);
    }
}
