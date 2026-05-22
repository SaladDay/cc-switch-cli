use std::path::PathBuf;

const APP_DIR_NAME: &str = "cc-switch";

pub fn runtime_dir() -> PathBuf {
    runtime_dir_from(env_dir("XDG_RUNTIME_DIR"), env_dir("TMPDIR"), current_uid())
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

fn runtime_dir_from(xdg: Option<PathBuf>, tmpdir: Option<PathBuf>, uid: u32) -> PathBuf {
    if let Some(dir) = xdg {
        return dir.join(APP_DIR_NAME);
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

    #[test]
    fn runtime_dir_uses_xdg_runtime_dir_when_set() {
        let dir = runtime_dir_from(
            Some(PathBuf::from("/run/user/1000")),
            Some(PathBuf::from("/tmp")),
            1000,
        );
        assert_eq!(dir, PathBuf::from("/run/user/1000/cc-switch"));
    }

    #[test]
    fn runtime_dir_falls_back_to_tmpdir_with_uid_when_xdg_unset() {
        let dir = runtime_dir_from(None, Some(PathBuf::from("/private/tmp")), 501);
        assert_eq!(dir, PathBuf::from("/private/tmp/cc-switch-501"));
    }

    #[test]
    fn runtime_dir_uses_slash_tmp_when_neither_xdg_nor_tmpdir_set() {
        let dir = runtime_dir_from(None, None, 0);
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
        let runtime = runtime_dir_from(Some(PathBuf::from("/run")), None, 0);
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
}
