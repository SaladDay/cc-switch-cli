#![cfg(unix)]

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

#[path = "support.rs"]
mod support;

struct RunningTui(Child);

impl Drop for RunningTui {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_tui() -> (RunningTui, File, File) {
    support::reset_test_fs();
    let mut master = -1;
    let mut slave = -1;
    let mut size = libc::winsize {
        ws_row: 40,
        ws_col: 120,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: openpty initializes both descriptors and reads the supplied size.
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        },
        0
    );
    // SAFETY: these are fresh, owned descriptors returned by openpty.
    let (mut master, slave) = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
    // Do not let the child inherit the master; otherwise closing it in the
    // parent cannot hang up the PTY. Nonblocking reads drain the TUI's output.
    for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
        // SAFETY: both files are live and fcntl takes integer flags here.
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
            0
        );
    }
    // SAFETY: master remains open for the call.
    assert_eq!(
        unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) },
        0
    );

    let mut command = Command::new(env!("CARGO_BIN_EXE_cc-switch"));
    command
        .arg("interactive")
        .env("TERM", "xterm-256color")
        .env_remove("CC_SWITCH_DAEMON_SOCKET")
        .env_remove("CC_SWITCH_LEGACY_TUI")
        .stdin(slave.try_clone().unwrap())
        .stdout(slave.try_clone().unwrap())
        .stderr(slave.try_clone().unwrap());
    // SAFETY: the pre-exec hook only calls async-signal-safe libc functions.
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            // Reproduce a terminal proxy that leaves its child without SIGHUP.
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
            Ok(())
        });
    }
    let mut child = RunningTui(command.spawn().expect("spawn isolated TUI"));
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut output = Vec::new();
    while !output.windows(6).any(|w| w == b"\x1b[?25l") {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "TUI exited: {output:?}"
        );
        assert!(
            Instant::now() < deadline,
            "TUI did not activate: {output:?}"
        );
        drain_output(&mut master, &mut output);
        std::thread::sleep(Duration::from_millis(10));
    }
    // Let startup finish and the input loop enter poll. Keep draining output so
    // the test exercises input hangup, not a terminal write blocked at startup.
    let settled = Instant::now() + Duration::from_secs(4);
    while Instant::now() < settled {
        drain_output(&mut master, &mut output);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(child.0.try_wait().unwrap().is_none());
    (child, master, slave)
}

fn drain_output(master: &mut File, output: &mut Vec<u8>) {
    let mut buffer = [0; 8192];
    while let Ok(count) = master.read(&mut buffer) {
        if count == 0 {
            break;
        }
        output.extend_from_slice(&buffer[..count]);
    }
}

fn wait_for_exit(child: &mut RunningTui) -> ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "TUI failed to exit after terminal hangup/quit"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn tui_exits_after_pty_hangup_without_sighup() {
    let _lock = support::lock_test_mutex();
    let (mut child, master, _slave) = start_tui();
    drop(master);
    assert!(!wait_for_exit(&mut child).success());
}

#[test]
fn connected_terminal_keeps_keyboard_input_and_normal_restore() {
    let _lock = support::lock_test_mutex();
    let (mut child, mut master, slave) = start_tui();
    master
        .write_all(b"\x03")
        .expect("send Ctrl-C as terminal input");
    assert!(wait_for_exit(&mut child).success());
    let mut output = Vec::new();
    drain_output(&mut master, &mut output);
    assert!(
        output.windows(8).any(|w| w == b"\x1b[?1049l"),
        "leave alternate screen"
    );
    let mut settings = std::mem::MaybeUninit::uninit();
    // SAFETY: slave is live and tcgetattr initializes settings on success.
    assert_eq!(
        unsafe { libc::tcgetattr(slave.as_raw_fd(), settings.as_mut_ptr()) },
        0
    );
    // SAFETY: tcgetattr succeeded.
    let settings = unsafe { settings.assume_init() };
    assert_ne!(
        settings.c_lflag & libc::ICANON,
        0,
        "restore canonical input"
    );
    assert_ne!(settings.c_lflag & libc::ECHO, 0, "restore echo");
}
