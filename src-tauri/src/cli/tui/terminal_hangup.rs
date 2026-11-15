use std::fs::File;
use std::io;
use std::os::fd::{AsFd, AsRawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

/// Crossterm 0.29 can spin inside poll/read after a PTY hangs up, so checking
/// from the UI loop cannot stop it. Observe hangup independently without
/// consuming input or changing signal handlers.
pub(super) struct HangupGuard {
    stopped: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl HangupGuard {
    pub(super) fn start() -> io::Result<Self> {
        // Interactive mode already requires stdin/stdout to be terminals.
        let terminal = File::from(io::stdin().as_fd().try_clone_to_owned()?);
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let worker = thread::Builder::new()
            .name("tui-terminal-hangup".into())
            .spawn(move || {
                let mut fd = libc::pollfd {
                    fd: terminal.as_raw_fd(),
                    // HUP is reported regardless of requested events. Normal
                    // input must not wake this observer or be consumed by it.
                    events: 0,
                    revents: 0,
                };
                while !worker_stopped.load(Ordering::Relaxed) {
                    // SAFETY: fd points to a live pollfd and terminal owns its
                    // descriptor for the entire lifetime of this thread.
                    let result = unsafe { libc::poll(&mut fd, 1, 100) };
                    if result < 0 {
                        if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                            continue;
                        }
                        break;
                    }
                    if fd.revents & libc::POLLHUP != 0 {
                        // The terminal is gone and the input thread may never
                        // return. Exit as a hangup would, even when a terminal
                        // proxy did not deliver SIGHUP. This cannot depend on
                        // the stuck reader unwinding; ordinary exits still use
                        // TuiTerminal::drop.
                        std::process::exit(128 + libc::SIGHUP);
                    }
                    if fd.revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
                        break;
                    }
                }
            })?;
        Ok(Self {
            stopped,
            worker: Some(worker),
        })
    }
}

impl Drop for HangupGuard {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
