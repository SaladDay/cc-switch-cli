use std::io::{self, Stdout};
use std::sync::Arc;

use crossterm::{
    cursor, execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
#[cfg(test)]
use ratatui::backend::TestBackend;
use ratatui::prelude::Size;
use ratatui::{backend::CrosstermBackend, Terminal};

use crate::error::AppError;

type LiveTerminal = Terminal<CrosstermBackend<Stdout>>;

type PanicHook = Arc<dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync + 'static>;

#[cfg(test)]
type InMemoryTerminal = Terminal<TestBackend>;

enum TerminalHandle {
    Stdout(LiveTerminal),
    #[cfg(test)]
    Test(InMemoryTerminal),
}

pub struct TuiTerminal {
    terminal: TerminalHandle,
    active: bool,
    #[cfg(test)]
    injected_restore_failure: Option<AppError>,
    #[cfg(test)]
    activation_count: usize,
}

pub struct PanicRestoreHookGuard {
    previous: Option<PanicHook>,
}

impl PanicRestoreHookGuard {
    pub fn install() -> Self {
        let previous = std::panic::take_hook();
        let previous: PanicHook = previous.into();
        let previous_for_hook = previous.clone();

        std::panic::set_hook(Box::new(move |info| {
            let mut stdout = io::stdout();
            let _ = restore_stdout_best_effort(&mut stdout);
            previous_for_hook(info);
        }));

        Self {
            previous: Some(previous),
        }
    }
}

impl Drop for PanicRestoreHookGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            std::panic::set_hook(Box::new(move |info| previous(info)));
        }
    }
}

fn record_err(first_err: &mut Option<AppError>, e: impl ToString) {
    if first_err.is_none() {
        *first_err = Some(AppError::localized(
            "tui_terminal_error",
            format!("终端错误: {}", e.to_string()),
            format!("Terminal error: {}", e.to_string()),
        ));
    }
}

fn restore_stdout_best_effort(stdout: &mut Stdout) -> Result<(), AppError> {
    let mut first_err: Option<AppError> = None;

    if let Err(e) = disable_raw_mode() {
        record_err(&mut first_err, e);
    }

    if let Err(e) = execute!(stdout, cursor::Show, LeaveAlternateScreen) {
        record_err(&mut first_err, e);
    }

    if let Some(err) = first_err {
        Err(err)
    } else {
        Ok(())
    }
}

fn terminal_error(e: impl ToString) -> AppError {
    AppError::localized(
        "tui_terminal_error",
        format!("终端错误: {}", e.to_string()),
        format!("Terminal error: {}", e.to_string()),
    )
}

impl TuiTerminal {
    pub fn new() -> Result<Self, AppError> {
        let mut stdout = io::stdout();
        enable_raw_mode().map_err(terminal_error)?;
        if let Err(e) = execute!(stdout, EnterAlternateScreen, cursor::Hide) {
            let _ = restore_stdout_best_effort(&mut stdout);
            return Err(terminal_error(e));
        }

        let terminal = match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => terminal,
            Err(e) => {
                let mut stdout = io::stdout();
                let _ = restore_stdout_best_effort(&mut stdout);
                return Err(terminal_error(e));
            }
        };

        Ok(Self {
            terminal: TerminalHandle::Stdout(terminal),
            active: true,
            #[cfg(test)]
            injected_restore_failure: None,
            #[cfg(test)]
            activation_count: 0,
        })
    }

    #[cfg(test)]
    pub(crate) fn new_for_test() -> Result<Self, AppError> {
        const TEST_TERMINAL_WIDTH: u16 = 120;
        const TEST_TERMINAL_HEIGHT: u16 = 40;

        let backend = TestBackend::new(TEST_TERMINAL_WIDTH, TEST_TERMINAL_HEIGHT);
        let terminal = Terminal::new(backend).expect("test terminal backend should be infallible");

        Ok(Self {
            terminal: TerminalHandle::Test(terminal),
            active: false,
            injected_restore_failure: None,
            activation_count: 0,
        })
    }

    pub fn draw<F>(&mut self, f: F) -> Result<(), AppError>
    where
        F: FnOnce(&mut ratatui::Frame<'_>),
    {
        match &mut self.terminal {
            TerminalHandle::Stdout(terminal) => {
                terminal.draw(f).map(|_| ()).map_err(terminal_error)
            }
            #[cfg(test)]
            TerminalHandle::Test(terminal) => terminal.draw(f).map(|_| ()).map_err(|e| match e {}),
        }
    }

    pub fn size(&self) -> Result<Size, AppError> {
        match &self.terminal {
            TerminalHandle::Stdout(terminal) => terminal.size().map_err(terminal_error),
            #[cfg(test)]
            TerminalHandle::Test(terminal) => terminal.size().map_err(|e| match e {}),
        }
    }

    pub fn with_terminal_restored<T>(
        &mut self,
        f: impl FnOnce() -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        self.restore_best_effort()?;

        struct ReactivateOnDrop<'a> {
            terminal: &'a mut TuiTerminal,
            reactivated: bool,
        }

        impl Drop for ReactivateOnDrop<'_> {
            fn drop(&mut self) {
                if self.reactivated {
                    return;
                }
                let _ = self.terminal.activate_best_effort();
            }
        }

        let mut guard = ReactivateOnDrop {
            terminal: self,
            reactivated: false,
        };

        let result = f();
        let activate_result = guard.terminal.activate_best_effort();
        if activate_result.is_ok() {
            guard.reactivated = true;
        }
        activate_result?;

        result
    }

    pub fn with_terminal_restored_for_handoff<T>(
        &mut self,
        f: impl FnOnce() -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        if let Err(err) = self.restore_best_effort() {
            self.active = false;
            return recover_terminal_after_handoff_failure(self, err);
        }

        match f() {
            Ok(value) => Ok(value),
            Err(err) => recover_terminal_after_handoff_failure(self, err),
        }
    }

    pub fn restore_best_effort(&mut self) -> Result<(), AppError> {
        if !self.active {
            return Ok(());
        }

        #[cfg(test)]
        if let Some(err) = self.injected_restore_failure.take() {
            return Err(err);
        }

        #[cfg(test)]
        if matches!(self.terminal, TerminalHandle::Test(_)) {
            self.active = false;
            return Ok(());
        }

        let mut first_err: Option<AppError> = None;

        if let Err(e) = disable_raw_mode() {
            record_err(&mut first_err, e);
        }

        match &mut self.terminal {
            TerminalHandle::Stdout(terminal) => {
                if let Err(e) = execute!(terminal.backend_mut(), cursor::Show, LeaveAlternateScreen)
                {
                    record_err(&mut first_err, e);
                }
                let _ = terminal.show_cursor();
            }
            #[cfg(test)]
            TerminalHandle::Test(_) => unreachable!("test terminal should return early"),
        }

        if let Some(err) = first_err {
            Err(err)
        } else {
            self.active = false;
            Ok(())
        }
    }

    pub fn activate_best_effort(&mut self) -> Result<(), AppError> {
        if self.active {
            return Ok(());
        }

        #[cfg(test)]
        if let TerminalHandle::Test(terminal) = &mut self.terminal {
            terminal
                .clear()
                .expect("test terminal backend should clear infallibly");
            self.active = true;
            self.activation_count += 1;
            return Ok(());
        }

        let mut first_err: Option<AppError> = None;

        if let Err(e) = enable_raw_mode() {
            record_err(&mut first_err, e);
        }

        match &mut self.terminal {
            TerminalHandle::Stdout(terminal) => {
                if let Err(e) = execute!(terminal.backend_mut(), EnterAlternateScreen, cursor::Hide)
                {
                    record_err(&mut first_err, e);
                }

                if let Err(e) = terminal.clear() {
                    record_err(&mut first_err, e);
                }
            }
            #[cfg(test)]
            TerminalHandle::Test(_) => unreachable!("test terminal should return early"),
        }

        if let Some(err) = first_err {
            Err(err)
        } else {
            self.active = true;
            Ok(())
        }
    }

    #[cfg(test)]
    pub(crate) fn inject_restore_failure_for_test(&mut self, err: AppError) {
        self.injected_restore_failure = Some(err);
    }

    #[cfg(test)]
    pub(crate) fn is_active_for_test(&self) -> bool {
        self.active
    }

    #[cfg(test)]
    pub(crate) fn activation_count_for_test(&self) -> usize {
        self.activation_count
    }
}

fn recover_terminal_after_handoff_failure<T>(
    terminal: &mut TuiTerminal,
    err: AppError,
) -> Result<T, AppError> {
    match terminal.activate_best_effort() {
        Ok(()) => Err(err),
        Err(activate_err) => Err(AppError::localized(
            "tui_terminal_restore_for_handoff_failed",
            format!("恢复终端失败: {err}；重新激活 TUI 失败: {activate_err}"),
            format!("Failed to restore terminal: {err}; also failed to reactivate the TUI: {activate_err}"),
        )),
    }
}

impl Drop for TuiTerminal {
    fn drop(&mut self) {
        let _ = self.restore_best_effort();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};
    use std::thread;

    use super::TuiTerminal;

    #[test]
    fn new_for_test_supports_parallel_construction_without_touching_real_tty() {
        let barrier = Arc::new(Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..32 {
                        let mut terminal = TuiTerminal::new_for_test().expect("create terminal");
                        let size = terminal.size().expect("read terminal size");
                        assert!(size.width > 0);
                        assert!(size.height > 0);
                        terminal.draw(|_| {}).expect("draw frame");
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("thread should complete without panic");
        }
    }

    #[test]
    fn with_terminal_restored_keeps_test_terminal_usable() {
        let mut terminal = TuiTerminal::new_for_test().expect("create terminal");

        let value = terminal
            .with_terminal_restored(|| Ok::<_, crate::error::AppError>(42))
            .expect("run callback with terminal restored");

        assert_eq!(value, 42);
        terminal
            .draw(|_| {})
            .expect("draw after with_terminal_restored");
    }

    #[test]
    fn test_terminal_can_activate_and_restore_without_real_tty() {
        let mut terminal = TuiTerminal::new_for_test().expect("create test terminal");

        terminal
            .activate_best_effort()
            .expect("test terminal should activate without a real tty");
        terminal
            .restore_best_effort()
            .expect("test terminal should restore without a real tty");
    }

    #[test]
    fn with_terminal_restored_for_handoff_reactivates_after_restore_failure() {
        let mut terminal = TuiTerminal::new_for_test().expect("create test terminal");
        terminal
            .activate_best_effort()
            .expect("activate test terminal first");
        terminal
            .draw(|frame| {
                let area = frame.area();
                frame.render_widget(ratatui::widgets::Paragraph::new("stale"), area);
            })
            .expect("draw stale frame");
        terminal.inject_restore_failure_for_test(crate::error::AppError::localized(
            "test.restore_failure",
            "模拟 restore 失败".to_string(),
            "simulated restore failure".to_string(),
        ));

        let err = terminal
            .with_terminal_restored_for_handoff(|| Ok::<_, crate::error::AppError>(()))
            .expect_err("restore failure should be returned");

        assert!(
            err.to_string().contains("restore failure")
                || err.to_string().contains("simulated restore failure"),
            "unexpected error: {err}"
        );
        assert!(
            terminal.is_active_for_test(),
            "pre-handoff restore failure should reactivate the TUI"
        );
        assert_eq!(
            terminal.activation_count_for_test(),
            2,
            "handoff recovery should perform a second activation after the restore failure"
        );
    }

    #[test]
    fn with_terminal_restored_for_handoff_reactivates_after_callback_error() {
        let mut terminal = TuiTerminal::new_for_test().expect("create test terminal");
        terminal
            .activate_best_effort()
            .expect("activate test terminal first");

        let err = terminal
            .with_terminal_restored_for_handoff(|| {
                Err::<(), _>(crate::error::AppError::localized(
                    "test.handoff_failure",
                    "模拟 handoff 失败".to_string(),
                    "simulated handoff failure".to_string(),
                ))
            })
            .expect_err("callback failure should be returned");

        assert!(
            err.to_string().contains("handoff failure")
                || err.to_string().contains("simulated handoff failure"),
            "unexpected error: {err}"
        );
        assert!(
            terminal.is_active_for_test(),
            "handoff callback errors after restore should reactivate the TUI"
        );
        assert_eq!(
            terminal.activation_count_for_test(),
            2,
            "handoff recovery should perform a second activation after callback failure"
        );
    }
}
