use std::ffi::OsString;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const SYSTEM_CLIPBOARD_TIMEOUT: Duration = Duration::from_millis(750);
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);
const REMOTE_SESSION_ENV_VARS: &[&str] =
    &["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY", "MOSH_CONNECTION"];

#[derive(Debug, Clone, Copy)]
struct ClipboardCommand {
    program: &'static str,
    args: &'static [&'static str],
}

#[cfg(target_os = "macos")]
const CLIPBOARD_COMMANDS: &[ClipboardCommand] = &[ClipboardCommand {
    program: "pbcopy",
    args: &[],
}];

#[cfg(target_os = "linux")]
const CLIPBOARD_COMMANDS: &[ClipboardCommand] = &[
    ClipboardCommand {
        program: "wl-copy",
        args: &[],
    },
    ClipboardCommand {
        program: "xclip",
        args: &["-selection", "clipboard", "-in"],
    },
    ClipboardCommand {
        program: "xsel",
        args: &["--clipboard", "--input"],
    },
];

#[cfg(target_os = "windows")]
const CLIPBOARD_COMMANDS: &[ClipboardCommand] = &[ClipboardCommand {
    program: "clip.exe",
    args: &[],
}];

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
const CLIPBOARD_COMMANDS: &[ClipboardCommand] = &[];

pub(super) fn copy_with_system_tool(text: &str) -> bool {
    if is_remote_session_with(|name| std::env::var_os(name)) {
        log::debug!("skipping host clipboard tools in a remote terminal session");
        return false;
    }

    let deadline = Instant::now() + SYSTEM_CLIPBOARD_TIMEOUT;
    for candidate in CLIPBOARD_COMMANDS {
        let Ok(program) = which::which(candidate.program) else {
            continue;
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match run_clipboard_command(&program, candidate.args, text, remaining) {
            Ok(true) => return true,
            Ok(false) => {
                log::debug!(
                    "system clipboard command `{}` exited unsuccessfully",
                    candidate.program
                );
            }
            Err(err) => {
                log::debug!(
                    "system clipboard command `{}` failed: {err}",
                    candidate.program
                );
            }
        }
    }
    false
}

fn is_remote_session_with(mut get_var: impl FnMut(&str) -> Option<OsString>) -> bool {
    REMOTE_SESSION_ENV_VARS
        .iter()
        .any(|name| get_var(name).is_some_and(|value| !value.is_empty()))
}

fn run_clipboard_command(
    program: &Path,
    args: &[&str],
    text: &str,
    timeout: Duration,
) -> io::Result<bool> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    let write_result = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("clipboard command stdin was unavailable"))
        .and_then(|mut stdin| stdin.write_all(text.as_bytes()));
    if let Err(err) = write_result {
        let _ = child.kill();
        let _ = child.wait();
        return Err(err);
    }

    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status.success());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(false);
        }
        thread::sleep(PROCESS_POLL_INTERVAL.min(remaining));
    }
}

#[cfg(test)]
mod tests {
    use super::{is_remote_session_with, run_clipboard_command, CLIPBOARD_COMMANDS};
    use std::ffi::OsString;
    use std::path::Path;
    use std::time::Duration;

    #[test]
    fn clipboard_helpers_are_invoked_directly_without_a_shell() {
        for command in CLIPBOARD_COMMANDS {
            assert!(!command.program.trim().is_empty());
            assert!(!command.program.chars().any(char::is_whitespace));
            assert!(command.args.iter().all(|arg| !arg.contains("{text}")));
        }
    }

    #[test]
    fn remote_sessions_do_not_target_the_host_clipboard() {
        assert!(is_remote_session_with(|name| {
            (name == "SSH_CONNECTION").then(|| OsString::from("client server"))
        }));
        assert!(is_remote_session_with(|name| {
            (name == "MOSH_CONNECTION").then(|| OsString::from("client server"))
        }));
        assert!(!is_remote_session_with(|_| None));
        assert!(!is_remote_session_with(|name| {
            (name == "SSH_TTY").then(OsString::new)
        }));
    }

    #[cfg(unix)]
    #[test]
    fn clipboard_command_writes_text_to_stdin_and_waits_for_success() {
        assert!(run_clipboard_command(
            Path::new("/bin/cat"),
            &[],
            "codex resume session-1",
            Duration::from_secs(1),
        )
        .expect("run clipboard command"));
    }
}
