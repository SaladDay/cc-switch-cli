//! Windows has no `exec`, so `cc-switch start` runs the CLI as a child in the
//! same console, waits for it, then cleans up and exits with its status.

use std::process::Command;

use windows_sys::core::BOOL;
use windows_sys::Win32::System::Console::{SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_C_EVENT};

use crate::error::AppError;

// Ctrl+C reaches every process on the console. Keep cc-switch alive so it can
// clean up after the child; the child still handles Ctrl+C normally because
// handler routines are not inherited.
unsafe extern "system" fn keep_running_on_ctrl_c(ctrl_type: u32) -> BOOL {
    (ctrl_type == CTRL_C_EVENT || ctrl_type == CTRL_BREAK_EVENT) as BOOL
}

pub(crate) fn run_and_wait(mut command: Command, app_name: &str) -> Result<i32, AppError> {
    unsafe { SetConsoleCtrlHandler(Some(keep_running_on_ctrl_c), 1) };
    let status = command.status();
    unsafe { SetConsoleCtrlHandler(Some(keep_running_on_ctrl_c), 0) };
    let status = status.map_err(|err| {
        AppError::localized(
            "temp_launch_exec_failed",
            format!("启动 {app_name} 失败: {err}"),
            format!("Failed to launch {app_name}: {err}"),
        )
    })?;
    Ok(status.code().unwrap_or(1))
}
