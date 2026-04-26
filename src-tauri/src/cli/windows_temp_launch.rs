use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::path::Path;

use crate::error::AppError;

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, FALSE, HANDLE, INVALID_HANDLE_VALUE, TRUE,
};
#[cfg(windows)]
use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
#[cfg(windows)]
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    CreateProcessW, GetExitCodeProcess, ResumeThread, WaitForSingleObject,
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, INFINITE, PROCESS_INFORMATION, STARTUPINFOW,
};

// ── cmd shim detection ───────────────────────────────────────────────

#[cfg(windows)]
pub(crate) fn is_cmd_shim(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"))
        .unwrap_or(false)
}

// ── argument quoting ─────────────────────────────────────────────────

/// Returns true when `quote_windows_arg_for_cmd` would wrap `s` in double
/// quotes. We mirror its predicate (sans the `"` case, which is rejected by
/// the caller before this is consulted) so callers can decide whether a
/// trailing `\` is dangerous: only quoted args risk the trailing `\`
/// escaping the closing `"`. Plain Windows paths like `C:\work\` pass
/// through unquoted and are safe.
#[cfg(windows)]
pub(crate) fn arg_requires_cmd_quote(s: &str) -> bool {
    const CMD_SPECIAL: &[char] = &['&', '|', '<', '>', '^', '%', '!', '(', ')'];
    s.is_empty()
        || s.contains(' ')
        || s.contains('\t')
        || s.contains('\n')
        || s.chars().any(|c| CMD_SPECIAL.contains(&c))
}

#[cfg(windows)]
pub(crate) fn quote_windows_arg(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".to_string();
    }
    if !arg.contains(' ') && !arg.contains('\t') && !arg.contains('\n') && !arg.contains('"') {
        return arg.to_string();
    }

    let mut result = String::with_capacity(arg.len() + 2);
    result.push('"');

    let mut chars = arg.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '"' {
            result.push('\\');
            result.push('"');
        } else if ch == '\\' {
            let mut count = 1;
            while chars.peek() == Some(&'\\') {
                count += 1;
                chars.next();
            }
            if chars.peek() == Some(&'"') || chars.peek().is_none() {
                for _ in 0..count * 2 {
                    result.push('\\');
                }
            } else {
                for _ in 0..count {
                    result.push('\\');
                }
            }
        } else {
            result.push(ch);
        }
    }

    result.push('"');
    result
}

#[cfg(windows)]
pub(crate) fn quote_windows_arg_for_cmd(arg: &str) -> String {
    const CMD_SPECIAL: &[char] = &['&', '|', '<', '>', '^', '%', '!', '(', ')'];
    let needs_quote = arg.is_empty()
        || arg.contains(' ')
        || arg.contains('\t')
        || arg.contains('\n')
        || arg.contains('"')
        || arg.chars().any(|c| CMD_SPECIAL.contains(&c));

    if !needs_quote {
        return arg.to_string();
    }

    let mut result = String::with_capacity(arg.len() + 2);
    result.push('"');

    let mut chars = arg.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '"' {
            result.push('\\');
            result.push('"');
        } else if ch == '\\' {
            let mut count = 1;
            while chars.peek() == Some(&'\\') {
                count += 1;
                chars.next();
            }
            if chars.peek() == Some(&'"') || chars.peek().is_none() {
                for _ in 0..count * 2 {
                    result.push('\\');
                }
            } else {
                for _ in 0..count {
                    result.push('\\');
                }
            }
        } else {
            result.push(ch);
        }
    }

    result.push('"');
    result
}

// ── command line construction ────────────────────────────────────────

#[cfg(windows)]
pub(crate) fn build_windows_command_line(program: &OsStr, args: &[OsString]) -> Vec<u16> {
    let program_str = program.to_string_lossy();
    let is_cmd = program_str.eq_ignore_ascii_case("cmd.exe") || program_str.eq_ignore_ascii_case("cmd");

    let mut line = String::new();
    line.push_str(&quote_windows_arg(&program_str));

    let mut after_c = false;
    for arg in args {
        line.push(' ');
        let arg_str = arg.to_string_lossy();
        if is_cmd && after_c {
            line.push_str(&quote_windows_arg_for_cmd(&arg_str));
        } else {
            line.push_str(&quote_windows_arg(&arg_str));
            if is_cmd && arg_str.eq_ignore_ascii_case("/c") {
                after_c = true;
            }
        }
    }
    std::ffi::OsStr::new(&line)
        .encode_wide()
        .chain(Some(0))
        .collect()
}

#[cfg(windows)]
pub(crate) fn build_env_block_with_override(key: &str, value: &OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    let mut result = Vec::new();
    for (k, v) in std::env::vars_os() {
        // Windows environment variable names are case-insensitive.
        if k.to_string_lossy().eq_ignore_ascii_case(key) {
            continue;
        }
        result.extend(k.encode_wide());
        result.push(b'=' as u16);
        result.extend(v.encode_wide());
        result.push(0);
    }
    // Add our override
    result.extend(key.encode_utf16());
    result.push(b'=' as u16);
    result.extend(value.encode_wide());
    result.push(0);
    // Double-null terminate the block
    result.push(0);
    result
}

// ── Ctrl handler guard ───────────────────────────────────────────────

#[cfg(windows)]
pub(crate) struct ScopedConsoleCtrlHandler;

#[cfg(windows)]
impl ScopedConsoleCtrlHandler {
    pub(crate) fn install() -> Result<Self, AppError> {
        unsafe {
            let result = SetConsoleCtrlHandler(Some(ctrl_handler_swallow), TRUE);
            if result == 0 {
                return Err(AppError::localized(
                    "windows.set_console_ctrl_handler_failed",
                    "设置控制台 Ctrl 处理器失败".to_string(),
                    "Failed to set console Ctrl handler.".to_string(),
                ));
            }
        }
        Ok(ScopedConsoleCtrlHandler)
    }
}

#[cfg(windows)]
impl Drop for ScopedConsoleCtrlHandler {
    fn drop(&mut self) {
        unsafe {
            let _ = SetConsoleCtrlHandler(Some(ctrl_handler_swallow), FALSE);
        }
    }
}

#[cfg(windows)]
unsafe extern "system" fn ctrl_handler_swallow(_ctrl_type: u32) -> i32 {
    TRUE
}

// ── Job Object ───────────────────────────────────────────────────────

#[cfg(windows)]
pub(crate) struct Job {
    handle: HANDLE,
}

#[cfg(windows)]
impl Job {
    pub(crate) fn create_with_kill_on_close() -> Result<Self, AppError> {
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                let code = GetLastError();
                return Err(AppError::windows_create_job_object_failed(code));
            }

            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

            let result = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &mut info as *mut _ as *mut _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );

            if result == 0 {
                let code = GetLastError();
                CloseHandle(handle);
                return Err(AppError::windows_set_job_information_failed(code));
            }

            Ok(Job { handle })
        }
    }

    pub(crate) fn try_assign(&self, process: HANDLE) -> Result<(), std::io::Error> {
        unsafe {
            let result = AssignProcessToJobObject(self.handle, process);
            if result == 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

// ── process spawning ─────────────────────────────────────────────────

#[cfg(windows)]
pub(crate) fn spawn_suspended_createprocessw(
    program: &std::path::Path,
    args: &[OsString],
    env_block: Option<&[u16]>,
    application_name: Option<&std::path::Path>,
) -> Result<(HANDLE, HANDLE), AppError> {
    use std::ptr;

    let application_name_wide: Option<Vec<u16>> = application_name.map(|p| {
        std::ffi::OsStr::new(p)
            .encode_wide()
            .chain(Some(0))
            .collect()
    });

    let mut command_line = build_windows_command_line(std::ffi::OsStr::new(program), args);

    let mut startup_info: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup_info.cb = std::mem::size_of::<STARTUPINFOW>() as u32;

    let mut process_info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

    let env_ptr = env_block
        .map(|b| b.as_ptr() as *mut _)
        .unwrap_or(ptr::null_mut());

    let app_name_ptr = application_name_wide
        .as_ref()
        .map(|s| s.as_ptr())
        .unwrap_or(ptr::null());

    let result = unsafe {
        CreateProcessW(
            app_name_ptr,
            command_line.as_mut_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
            FALSE,
            CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT,
            env_ptr,
            ptr::null(),
            &startup_info,
            &mut process_info,
        )
    };

    if result == 0 {
        let code = unsafe { GetLastError() };
        return Err(AppError::windows_create_process_failed(code));
    }

    Ok((process_info.hProcess, process_info.hThread))
}

#[cfg(windows)]
pub(crate) fn wait_for_child(process_handle: HANDLE) -> Result<u32, AppError> {
    unsafe {
        let wait_result = WaitForSingleObject(process_handle, INFINITE);
        if wait_result != 0 {
            let code = GetLastError();
            return Err(AppError::localized(
                "windows.wait_for_child_failed",
                format!("等待子进程失败，Win32 错误码: {code}"),
                format!("Failed to wait for child process, Win32 error: {code}"),
            ));
        }

        let mut exit_code: u32 = 0;
        if GetExitCodeProcess(process_handle, &mut exit_code) == 0 {
            let code = GetLastError();
            return Err(AppError::localized(
                "windows.get_exit_code_failed",
                format!("获取子进程退出码失败，Win32 错误码: {code}"),
                format!("Failed to get child exit code, Win32 error: {code}"),
            ));
        }

        Ok(exit_code)
    }
}

// ── ACL / file security ──────────────────────────────────────────────

#[cfg(windows)]
pub(crate) fn restrict_to_owner(path: &Path, inherit: bool) -> Result<(), AppError> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE};
    use windows_sys::Win32::Security::Authorization::{
        SetNamedSecurityInfoW, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        ACL, AddAccessAllowedAceEx, DACL_SECURITY_INFORMATION,
        GetLengthSid, GetTokenInformation, InitializeAcl,
        PROTECTED_DACL_SECURITY_INFORMATION,
        TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    const NO_INHERITANCE: u32 = 0;
    const OBJECT_INHERIT_ACE: u32 = 0x1;
    const CONTAINER_INHERIT_ACE: u32 = 0x2;
    const FILE_ALL_ACCESS: u32 = 0x1F01FF;
    const ACL_REVISION: u32 = 2;

    // Open current process token to get the user SID
    let mut token: HANDLE = std::ptr::null_mut();
    let result = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    if result == 0 {
        return Err(AppError::io(path, std::io::Error::last_os_error()));
    }

    // Get token user info (first call to get size)
    let mut size = 0u32;
    unsafe {
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut size);
    }

    let mut buffer = vec![0u8; size as usize];
    let result = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr() as *mut _,
            size,
            &mut size,
        )
    };
    if result == 0 {
        unsafe { CloseHandle(token) };
        return Err(AppError::io(path, std::io::Error::last_os_error()));
    }

    let token_user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
    let user_sid = token_user.User.Sid;

    unsafe { CloseHandle(token) };

    let sid_len = unsafe { GetLengthSid(user_sid) };

    // ACL size = ACL header + ACCESS_ALLOWED_ACE without SidStart + SID length
    // ACL header = 8 bytes, ACE header+Mask = 8 bytes, SidStart = 4 bytes
    let acl_size = (std::mem::size_of::<ACL>() + 8 + sid_len as usize) as u32;
    let mut acl_buffer = vec![0u8; acl_size as usize];
    let acl = acl_buffer.as_mut_ptr() as *mut ACL;

    let result = unsafe { InitializeAcl(acl, acl_size, ACL_REVISION) };
    if result == 0 {
        return Err(AppError::io(path, std::io::Error::last_os_error()));
    }

    let ace_flags = if inherit {
        OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
    } else {
        NO_INHERITANCE
    };

    let result = unsafe {
        AddAccessAllowedAceEx(acl, ACL_REVISION, ace_flags, FILE_ALL_ACCESS, user_sid)
    };
    if result == 0 {
        return Err(AppError::io(path, std::io::Error::last_os_error()));
    }

    let path_wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let result = unsafe {
        SetNamedSecurityInfoW(
            path_wide.as_ptr() as *mut _,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            acl,
            std::ptr::null_mut(),
        )
    };

    if result != ERROR_SUCCESS {
        return Err(AppError::io(path, std::io::Error::from_raw_os_error(result as i32)));
    }

    Ok(())
}

#[cfg(windows)]
pub(crate) fn create_secret_temp_file(path: &Path) -> Result<File, AppError> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|err| AppError::io(path, err))?;
    restrict_to_owner(path, false)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn quote_windows_arg_for_cmd_quotes_special_chars() {
        assert_eq!(quote_windows_arg_for_cmd("foo&bar"), "\"foo&bar\"");
        assert_eq!(quote_windows_arg_for_cmd("foo|bar"), "\"foo|bar\"");
        assert_eq!(quote_windows_arg_for_cmd("foo<bar"), "\"foo<bar\"");
        assert_eq!(quote_windows_arg_for_cmd("foo>bar"), "\"foo>bar\"");
        assert_eq!(quote_windows_arg_for_cmd("foo^bar"), "\"foo^bar\"");
        assert_eq!(quote_windows_arg_for_cmd("foo%bar"), "\"foo%bar\"");
        assert_eq!(quote_windows_arg_for_cmd("foo!bar"), "\"foo!bar\"");
        assert_eq!(quote_windows_arg_for_cmd("foo(bar"), "\"foo(bar\"");
        assert_eq!(quote_windows_arg_for_cmd("foo)bar"), "\"foo)bar\"");
    }

    #[cfg(windows)]
    #[test]
    fn quote_windows_arg_for_cmd_escapes_quotes() {
        assert_eq!(quote_windows_arg_for_cmd("foo\"bar"), "\"foo\\\"bar\"");
    }

    #[cfg(windows)]
    #[test]
    fn quote_windows_arg_for_cmd_quotes_spaces_and_specials() {
        assert_eq!(quote_windows_arg_for_cmd("foo & bar"), "\"foo & bar\"");
    }

    #[cfg(windows)]
    #[test]
    fn quote_windows_arg_for_cmd_leaves_plain_args_unchanged() {
        assert_eq!(quote_windows_arg_for_cmd("normal"), "normal");
        assert_eq!(
            quote_windows_arg_for_cmd("C:\\path\\file.exe"),
            "C:\\path\\file.exe"
        );
    }

    #[cfg(windows)]
    #[test]
    fn quote_windows_arg_for_cmd_handles_empty_string() {
        assert_eq!(quote_windows_arg_for_cmd(""), "\"\"");
    }

    #[cfg(windows)]
    #[test]
    fn quote_windows_arg_handles_newlines() {
        assert_eq!(quote_windows_arg("foo\nbar"), "\"foo\nbar\"");
    }

    #[cfg(windows)]
    #[test]
    fn is_cmd_shim_matches_cmd_and_bat_case_insensitive() {
        assert!(is_cmd_shim(std::path::Path::new("C:/tools/app.cmd")));
        assert!(is_cmd_shim(std::path::Path::new("C:/tools/app.CMD")));
        assert!(is_cmd_shim(std::path::Path::new("C:/tools/app.bat")));
        assert!(is_cmd_shim(std::path::Path::new("C:/tools/app.BaT")));
        assert!(!is_cmd_shim(std::path::Path::new("C:/tools/app.exe")));
        assert!(!is_cmd_shim(std::path::Path::new("C:/tools/app")));
    }

    #[cfg(windows)]
    #[test]
    fn arg_requires_cmd_quote_recognizes_unsafe_inputs() {
        assert!(arg_requires_cmd_quote(""));
        assert!(arg_requires_cmd_quote("a b"));
        assert!(arg_requires_cmd_quote("a\tb"));
        assert!(arg_requires_cmd_quote("a&b"));
        assert!(arg_requires_cmd_quote("a%b"));
        assert!(!arg_requires_cmd_quote("plain"));
        assert!(!arg_requires_cmd_quote("C:\\work\\"));
        assert!(!arg_requires_cmd_quote("--project-dir=C:\\tmp\\"));
    }

    #[cfg(windows)]
    #[test]
    fn build_windows_command_line_quotes_after_c_for_cmd() {
        let line = build_windows_command_line(
            OsStr::new("cmd.exe"),
            &[
                OsString::from("/c"),
                OsString::from("foo&bar"),
                OsString::from("normal"),
            ],
        );
        let s = String::from_utf16_lossy(&line);
        // Should contain quoted foo&bar but not normal
        assert!(s.contains("\"foo&bar\""));
        assert!(s.contains("normal"));
    }

    /// Smoke test: spawn a real child process via the shared Windows path,
    /// assign it to a Job Object, resume it, and verify the exit code.
    #[cfg(windows)]
    #[test]
    fn windows_smoke_test_spawn_job_wait_exit_code() {
        let (h_process, h_thread) = spawn_suspended_createprocessw(
            std::path::Path::new("cmd.exe"),
            &[
                OsString::from("/c"),
                OsString::from("exit"),
                OsString::from("42"),
            ],
            None,
            None,
        )
        .expect("spawn_suspended_createprocessw should succeed for cmd.exe");

        let job = Job::create_with_kill_on_close()
            .expect("Job::create_with_kill_on_close should succeed");
        job.try_assign(h_process)
            .expect("try_assign should succeed for a non-nested process");

        let resume_result = unsafe { ResumeThread(h_thread) };
        assert_ne!(
            resume_result, u32::MAX,
            "ResumeThread should not fail"
        );

        unsafe { CloseHandle(h_thread) };

        let exit_code = wait_for_child(h_process).expect("wait_for_child should succeed");
        unsafe { CloseHandle(h_process) };

        assert_eq!(exit_code, 42, "child exit code should be propagated");
    }
}
