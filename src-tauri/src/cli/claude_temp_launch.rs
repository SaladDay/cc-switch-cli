use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(not(windows))]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

use crate::error::AppError;
use crate::provider::Provider;
use crate::services::provider::ProviderService;
use serde_json::Value;

#[derive(Debug, Clone)]
pub(crate) struct PreparedClaudeLaunch {
    pub(crate) executable: PathBuf,
    pub(crate) settings_path: PathBuf,
}

impl PreparedClaudeLaunch {
    pub(crate) fn cleanup_settings_file(&self) -> Result<(), AppError> {
        cleanup_temp_settings_file(&self.settings_path)
    }
}

pub(crate) fn prepare_launch(
    provider: &Provider,
    temp_dir: &Path,
) -> Result<PreparedClaudeLaunch, AppError> {
    prepare_launch_with(provider, temp_dir, resolve_claude_binary)
}

pub(crate) fn prepare_launch_from_settings(
    provider_id: &str,
    settings: &Value,
    temp_dir: &Path,
) -> Result<PreparedClaudeLaunch, AppError> {
    prepare_launch_from_settings_with(provider_id, settings, temp_dir, resolve_claude_binary)
}

pub(crate) fn prepare_launch_with<Resolve>(
    provider: &Provider,
    temp_dir: &Path,
    resolve_claude_binary: Resolve,
) -> Result<PreparedClaudeLaunch, AppError>
where
    Resolve: FnOnce() -> Result<PathBuf, AppError>,
{
    prepare_launch_from_settings_with(
        &provider.id,
        &provider.settings_config,
        temp_dir,
        resolve_claude_binary,
    )
}

pub(crate) fn prepare_launch_from_settings_with<Resolve>(
    provider_id: &str,
    settings: &Value,
    temp_dir: &Path,
    resolve_claude_binary: Resolve,
) -> Result<PreparedClaudeLaunch, AppError>
where
    Resolve: FnOnce() -> Result<PathBuf, AppError>,
{
    let executable = resolve_claude_binary()?;

    if settings.get("env").and_then(|v| v.as_object()).is_none() {
        return Err(AppError::localized(
            "claude.temp_launch_missing_env",
            format!("供应商 {} 缺少有效的 env 配置。", provider_id),
            format!("Provider {} is missing a valid env object.", provider_id),
        ));
    }

    let mut normalized_settings = settings.clone();
    let _ = ProviderService::normalize_claude_models_in_value(&mut normalized_settings);
    let settings_path = write_temp_settings_file(temp_dir, provider_id, &normalized_settings)?;

    Ok(PreparedClaudeLaunch {
        executable,
        settings_path,
    })
}

pub(crate) fn resolve_claude_binary() -> Result<PathBuf, AppError> {
    which::which("claude").map_err(|_| {
        AppError::localized(
            "claude.temp_launch_missing_binary",
            "未找到 claude 命令，请先安装 Claude CLI。".to_string(),
            "Could not find `claude` in PATH. Install Claude CLI first.".to_string(),
        )
    })
}

#[cfg(unix)]
pub(crate) fn ensure_temp_launch_supported() -> Result<(), AppError> {
    Ok(())
}

#[cfg(windows)]
pub(crate) fn ensure_temp_launch_supported() -> Result<(), AppError> {
    Ok(())
}

#[cfg(unix)]
pub(crate) fn build_handoff_command(
    prepared: &PreparedClaudeLaunch,
    native_args: &[OsString],
) -> std::process::Command {
    let mut command = std::process::Command::new("/bin/sh");
    command.arg("-c").arg(
        "claude_bin=\"$1\"; settings_path=\"$2\"; shift 2; exit_status=0; cleanup() { rm -f -- \"$settings_path\"; cleanup_status=$?; if [ \"$cleanup_status\" -ne 0 ]; then printf '%s\\n' \"cc-switch: failed to remove temporary Claude settings file: $settings_path\" >&2; if [ \"$exit_status\" -eq 0 ]; then exit_status=$cleanup_status; fi; fi; }; on_signal() { exit_status=\"$1\"; trap - INT TERM HUP; cleanup; exit \"$exit_status\"; }; trap 'on_signal 130' INT; trap 'on_signal 143' TERM; trap 'on_signal 129' HUP; \"$claude_bin\" --settings \"$settings_path\" \"$@\"; exit_status=$?; cleanup; exit \"$exit_status\"",
    );
    command.arg("cc-switch-claude-handoff");
    command.arg(&prepared.executable);
    command.arg(&prepared.settings_path);
    command.args(native_args);
    command
}

#[cfg(unix)]
pub(crate) fn exec_prepared_claude(
    prepared: &PreparedClaudeLaunch,
    native_args: &[OsString],
) -> Result<(), AppError> {
    use std::os::unix::process::CommandExt;

    let exec_err = build_handoff_command(prepared, native_args).exec();
    Err(AppError::localized(
        "claude.temp_launch_exec_failed",
        format!("启动 Claude 失败: {exec_err}"),
        format!("Failed to launch Claude: {exec_err}"),
    ))
}

#[cfg(windows)]
struct ScopedConsoleCtrlHandler;

#[cfg(windows)]
impl ScopedConsoleCtrlHandler {
    fn install() -> Result<Self, AppError> {
        unsafe {
            let result = windows_sys::Win32::System::Console::SetConsoleCtrlHandler(
                Some(ctrl_handler_swallow),
                1,
            );
            if result == 0 {
                return Err(AppError::localized(
                    "windows.console_ctrl_handler_failed",
                    "安装控制台 Ctrl+C 处理器失败".to_string(),
                    "Failed to install console Ctrl+C handler.".to_string(),
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
            let _ = windows_sys::Win32::System::Console::SetConsoleCtrlHandler(
                Some(ctrl_handler_swallow),
                0,
            );
        }
    }
}

#[cfg(windows)]
unsafe extern "system" fn ctrl_handler_swallow(_dw_ctrl_type: u32) -> i32 {
    1
}

#[cfg(windows)]
struct Job {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl Job {
    unsafe fn create_with_kill_on_close() -> Result<Self, AppError> {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };

        let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(AppError::localized(
                "windows.create_job_object_failed",
                "创建 Job Object 失败".to_string(),
                "Failed to create Job Object.".to_string(),
            ));
        }

        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

        let result = SetInformationJobObject(
            handle,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );

        if result == 0 {
            CloseHandle(handle);
            return Err(AppError::localized(
                "windows.set_job_info_failed",
                "设置 Job Object 信息失败".to_string(),
                "Failed to set Job Object information.".to_string(),
            ));
        }

        Ok(Job { handle })
    }

    unsafe fn try_assign(
        &self,
        process: windows_sys::Win32::Foundation::HANDLE,
    ) -> Result<(), std::io::Error> {
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        let result = AssignProcessToJobObject(self.handle, process);
        if result == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    unsafe fn terminate(&self) {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        let _ = TerminateJobObject(self.handle, 1);
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

#[cfg(windows)]
fn is_cmd_shim(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"))
        .unwrap_or(false)
}

/// Returns true when `quote_windows_arg_for_cmd` would wrap `s` in double
/// quotes. We mirror its predicate (sans the `"` case, which is rejected by
/// the caller before this is consulted) so callers can decide whether a
/// trailing `\` is dangerous: only quoted args risk the trailing `\`
/// escaping the closing `"`. Plain Windows paths like `C:\work\` pass
/// through unquoted and are safe.
#[cfg(windows)]
fn arg_requires_cmd_quote(s: &str) -> bool {
    const CMD_SPECIAL: &[char] = &['&', '|', '<', '>', '^', '%', '!', '(', ')'];
    s.is_empty()
        || s.contains(' ')
        || s.contains('\t')
        || s.contains('\n')
        || s.chars().any(|c| CMD_SPECIAL.contains(&c))
}

#[cfg(windows)]
fn build_windows_cmdline(
    prepared: &PreparedClaudeLaunch,
    native_args: &[OsString],
) -> Result<Vec<u16>, AppError> {
    let exe_str = prepared.executable.to_string_lossy();
    let is_cmd = is_cmd_shim(&prepared.executable);

    let mut cmdline = String::new();

    if is_cmd {
        // cmd.exe expands %VAR% and !VAR! (delayed expansion) even inside
        // double quotes. There is no standard escape for these in a /c
        // command line. We quote the argument to prevent command injection
        // from & | < > ^ ( ), but % and ! remain a best-effort limitation
        // of the cmd.exe shell. Without refactoring to bypass cmd.exe /c
        // entirely (e.g. parse the .cmd shim and invoke the underlying
        // binary directly), this expansion cannot be fully avoided. Log a
        // warning so users are aware.
        //
        // cmd.exe does not treat backslash as a quote escape, so a literal
        // double quote inside an arg cannot be safely escaped — reject. A
        // trailing backslash only becomes unsafe when the arg itself would
        // be wrapped in `"..."` by cmd quoting, because then the `\` would
        // escape the closing quote. Plain paths like `C:\work\` need no
        // quoting and pass through verbatim.
        for arg in native_args {
            let s = arg.to_string_lossy();
            if s.contains('%') || s.contains('!') {
                log::warn!(
                    target: "claude_temp_launch",
                    "Native arg contains % or ! which cmd.exe may expand: {}",
                    s
                );
            }
            if s.contains('"') {
                return Err(AppError::localized(
                    "claude.temp_launch_unsafe_cmd_quote",
                    format!(
                        "参数包含双引号，无法安全地通过 cmd.exe /c 传递: {}",
                        s
                    ),
                    format!(
                        "Native arg contains a double quote which cannot be safely passed through cmd.exe /c: {}",
                        s
                    ),
                ));
            }
            if s.ends_with('\\') && arg_requires_cmd_quote(&s) {
                return Err(AppError::localized(
                    "claude.temp_launch_unsafe_cmd_trailing_backslash",
                    format!(
                        "参数同时需要 cmd.exe 加引号且以反斜杠结尾，无法安全传递: {}",
                        s
                    ),
                    format!(
                        "Native arg both requires cmd.exe quoting and ends with a backslash, which cannot be safely passed through cmd.exe /c: {}",
                        s
                    ),
                ));
            }
        }
        cmdline.push_str("cmd.exe /c ");
        cmdline.push_str(&quote_windows_arg_for_cmd(&exe_str));
        cmdline.push_str(" --settings ");
        cmdline.push_str(&quote_windows_arg_for_cmd(
            &prepared.settings_path.to_string_lossy(),
        ));
        for arg in native_args {
            cmdline.push(' ');
            cmdline.push_str(&quote_windows_arg_for_cmd(&arg.to_string_lossy()));
        }
    } else {
        cmdline.push_str(&quote_windows_arg(&exe_str));
        cmdline.push_str(" --settings ");
        cmdline.push_str(&quote_windows_arg(
            &prepared.settings_path.to_string_lossy(),
        ));
        for arg in native_args {
            cmdline.push(' ');
            cmdline.push_str(&quote_windows_arg(&arg.to_string_lossy()));
        }
    }

    Ok(cmdline.encode_utf16().chain(std::iter::once(0)).collect())
}

#[cfg(windows)]
fn quote_windows_arg(arg: &str) -> String {
    if !arg.is_empty()
        && !arg.contains(' ')
        && !arg.contains('\t')
        && !arg.contains('\n')
        && !arg.contains('"')
    {
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
                // Double backslashes when followed by a quote or end of string
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
fn quote_windows_arg_for_cmd(arg: &str) -> String {
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

#[cfg(windows)]
pub(crate) fn exec_prepared_claude(
    prepared: &PreparedClaudeLaunch,
    native_args: &[OsString],
) -> Result<(), AppError> {
    use windows_sys::Win32::Foundation::{CloseHandle, FALSE};
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, GetExitCodeProcess, ResumeThread, WaitForSingleObject, INFINITE,
        PROCESS_INFORMATION, STARTUPINFOW,
    };

    let _ctrl_guard = ScopedConsoleCtrlHandler::install()?;

    let mut cmdline_wide = build_windows_cmdline(prepared, native_args)?;

    let application_name_wide: Option<Vec<u16>> = if is_cmd_shim(&prepared.executable) {
        None
    } else {
        Some(
            std::ffi::OsStr::new(&*prepared.executable)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect(),
        )
    };

    let mut startup_info: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup_info.cb = std::mem::size_of::<STARTUPINFOW>() as u32;

    let mut process_info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

    let app_name_ptr = application_name_wide
        .as_ref()
        .map(|s| s.as_ptr())
        .unwrap_or(std::ptr::null());

    let create_result = unsafe {
        CreateProcessW(
            app_name_ptr,
            cmdline_wide.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            FALSE,
            0x00000004,
            std::ptr::null(),
            std::ptr::null(),
            &startup_info,
            &mut process_info,
        )
    };

    if create_result == 0 {
        return Err(AppError::localized(
            "windows.create_process_failed",
            "创建进程失败".to_string(),
            format!(
                "Failed to create process: {}",
                std::io::Error::last_os_error()
            ),
        ));
    }

    let h_process = process_info.hProcess;
    let h_thread = process_info.hThread;

    let job = match unsafe { Job::create_with_kill_on_close() } {
        Ok(job) => job,
        Err(e) => {
            unsafe {
                let _ = windows_sys::Win32::System::Threading::TerminateProcess(h_process, 1);
                CloseHandle(h_thread);
                CloseHandle(h_process);
            }
            return Err(e);
        }
    };

    if let Err(e) = unsafe { job.try_assign(h_process) } {
        log::warn!(target: "windows.job_assign_failed_fallback", "{}", e);
    }

    let resume_result = unsafe { ResumeThread(h_thread) };
    if resume_result == u32::MAX {
        unsafe {
            job.terminate();
            CloseHandle(h_thread);
            CloseHandle(h_process);
        }
        return Err(AppError::localized(
            "windows.resume_thread_failed",
            "恢复线程失败".to_string(),
            format!(
                "Failed to resume thread: {}",
                std::io::Error::last_os_error()
            ),
        ));
    }

    unsafe {
        WaitForSingleObject(h_process, INFINITE);
    }

    let mut exit_code: u32 = 0;
    let get_exit_result = unsafe { GetExitCodeProcess(h_process, &mut exit_code) };

    unsafe {
        CloseHandle(h_thread);
        CloseHandle(h_process);
    }

    if get_exit_result == 0 {
        return Err(AppError::localized(
            "windows.get_exit_code_failed",
            "获取进程退出码失败".to_string(),
            format!(
                "Failed to get exit code: {}",
                std::io::Error::last_os_error()
            ),
        ));
    }

    if exit_code != 0 {
        return Err(AppError::Message(format!(
            "Claude exited with code {}",
            exit_code
        )));
    }

    Ok(())
}

fn write_temp_settings_file(
    temp_dir: &Path,
    provider_id: &str,
    settings: &serde_json::Value,
) -> Result<PathBuf, AppError> {
    write_temp_settings_file_with(temp_dir, provider_id, settings, finalize_temp_settings_file)
}

fn write_temp_settings_file_with<Finalize>(
    temp_dir: &Path,
    provider_id: &str,
    settings: &serde_json::Value,
    finalize: Finalize,
) -> Result<PathBuf, AppError>
where
    Finalize: FnOnce(&Path) -> Result<(), AppError>,
{
    #[cfg(windows)]
    let timestamp = crate::cli::orphan_scan::current_process_creation_time_nanos();
    #[cfg(not(windows))]
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let filename = format!(
        "cc-switch-claude-{}-{}-{timestamp}.json",
        sanitize_filename_fragment(provider_id),
        std::process::id()
    );
    let path = temp_dir.join(filename);
    let content =
        serde_json::to_vec_pretty(settings).map_err(|source| AppError::JsonSerialize { source })?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| AppError::io(parent, err))?;
    }

    let write_result = (|| {
        let mut file = create_secret_temp_file(&path)?;
        file.write_all(&content)
            .and_then(|()| file.flush())
            .map_err(|err| AppError::io(&path, err))?;
        finalize(&path)?;
        Ok(())
    })();

    match write_result {
        Ok(()) => Ok(path),
        Err(err) => match cleanup_temp_settings_file(&path) {
            Ok(()) => Err(err),
            Err(cleanup_err) => Err(AppError::localized(
                "claude.temp_launch_tempfile_cleanup_failed",
                format!("写入临时设置文件失败: {err}；同时清理失败: {cleanup_err}"),
                format!(
                    "Failed to write the temporary settings file: {err}; also failed to clean it up: {cleanup_err}"
                ),
            )),
        },
    }
}

#[cfg(unix)]
fn finalize_temp_settings_file(path: &Path) -> Result<(), AppError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|err| AppError::io(path, err))
}

#[cfg(not(unix))]
fn finalize_temp_settings_file(path: &Path) -> Result<(), AppError> {
    #[cfg(windows)]
    {
        restrict_to_owner(path, false)?;
    }
    Ok(())
}

#[cfg(unix)]
fn create_secret_temp_file(path: &Path) -> Result<File, AppError> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|err| AppError::io(path, err))
}

#[cfg(not(unix))]
fn create_secret_temp_file(path: &Path) -> Result<File, AppError> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|err| AppError::io(path, err))?;
    #[cfg(windows)]
    {
        restrict_to_owner(path, false)?;
    }
    Ok(file)
}

#[cfg(windows)]
fn restrict_to_owner(path: &Path, inherit: bool) -> Result<(), AppError> {
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
    let result = unsafe {
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
    };
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

fn cleanup_temp_settings_file(path: &Path) -> Result<(), AppError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(AppError::io(path, err)),
    }
}

fn sanitize_filename_fragment(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => ch,
            _ => '-',
        })
        .collect();
    if sanitized.is_empty() {
        "provider".to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::AppType;
    use crate::provider::Provider;
    use serde_json::{json, Value};
    #[cfg(unix)]
    use std::ffi::OsString;
    #[cfg(unix)]
    use std::os::unix::{fs::PermissionsExt, process::CommandExt};
    #[cfg(unix)]
    use std::process::Stdio;
    #[cfg(unix)]
    use std::time::Duration;
    use tempfile::TempDir;

    #[cfg(unix)]
    fn write_test_executable(temp_dir: &TempDir, name: &str, body: &str) -> PathBuf {
        let path = temp_dir.path().join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write stub executable");
        let mut permissions = std::fs::metadata(&path)
            .expect("stat stub executable")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).expect("chmod stub executable");
        path
    }

    #[cfg(unix)]
    #[test]
    fn unix_handoff_command_wraps_claude_and_cleans_up_temp_settings() {
        let prepared = PreparedClaudeLaunch {
            executable: PathBuf::from("/usr/local/bin/claude"),
            settings_path: PathBuf::from("/tmp/cc-switch-claude-settings.json"),
        };
        let native_args = vec![
            OsString::from("--dangerously-skip-permissions"),
            OsString::from("--model"),
            OsString::from("sonnet"),
        ];

        let command = build_handoff_command(&prepared, &native_args);
        let args: Vec<OsString> = command.get_args().map(|arg| arg.to_os_string()).collect();

        assert_eq!(command.get_program(), std::path::Path::new("/bin/sh"));
        assert_eq!(
            args,
            vec![
                OsString::from("-c"),
                OsString::from(
                    "claude_bin=\"$1\"; settings_path=\"$2\"; shift 2; exit_status=0; cleanup() { rm -f -- \"$settings_path\"; cleanup_status=$?; if [ \"$cleanup_status\" -ne 0 ]; then printf '%s\\n' \"cc-switch: failed to remove temporary Claude settings file: $settings_path\" >&2; if [ \"$exit_status\" -eq 0 ]; then exit_status=$cleanup_status; fi; fi; }; on_signal() { exit_status=\"$1\"; trap - INT TERM HUP; cleanup; exit \"$exit_status\"; }; trap 'on_signal 130' INT; trap 'on_signal 143' TERM; trap 'on_signal 129' HUP; \"$claude_bin\" --settings \"$settings_path\" \"$@\"; exit_status=$?; cleanup; exit \"$exit_status\""
                ),
                OsString::from("cc-switch-claude-handoff"),
                OsString::from("/usr/local/bin/claude"),
                OsString::from("/tmp/cc-switch-claude-settings.json"),
                OsString::from("--dangerously-skip-permissions"),
                OsString::from("--model"),
                OsString::from("sonnet"),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn interrupting_handoff_still_cleans_up_temp_settings() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let executable = write_test_executable(
            &temp_dir,
            "claude-stub.sh",
            "trap 'exit 130' INT TERM HUP\nwhile :; do sleep 1; done",
        );
        let settings_path = temp_dir.path().join("cc-switch-claude-settings.json");
        std::fs::write(&settings_path, "{}").expect("seed temp settings");

        let prepared = PreparedClaudeLaunch {
            executable,
            settings_path: settings_path.clone(),
        };
        let mut command = build_handoff_command(&prepared, &[]);
        command.stdout(Stdio::null()).stderr(Stdio::null());
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut child = command.spawn().expect("spawn handoff");
        std::thread::sleep(Duration::from_millis(150));
        let kill_result = unsafe { libc::kill(-(child.id() as i32), libc::SIGINT) };
        assert_eq!(kill_result, 0, "send SIGINT to handoff process group");

        let status = child.wait().expect("wait for handoff");
        assert_eq!(status.code(), Some(130));
        assert!(
            !settings_path.exists(),
            "temporary settings file should be removed after interrupt"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_failure_after_successful_handoff_surfaces_nonzero_exit() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let executable = write_test_executable(&temp_dir, "claude-stub.sh", "exit 0");
        let prepared = PreparedClaudeLaunch {
            executable,
            settings_path: PathBuf::from("."),
        };

        let mut command = build_handoff_command(&prepared, &[]);
        command.current_dir(temp_dir.path());
        let output = command.output().expect("run handoff");

        assert!(
            !output.status.success(),
            "cleanup failure should not look like a successful handoff"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("failed to remove temporary Claude settings file"));
    }

    #[test]
    fn temp_settings_file_is_removed_when_finalize_step_fails() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let provider = Provider::with_id(
            "demo".to_string(),
            "Demo".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "sk-demo"
                }
            }),
            None,
        );

        let err = write_temp_settings_file_with(
            temp_dir.path(),
            &provider.id,
            &json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "sk-demo"
                }
            }),
            |_| Err(AppError::Message("simulated finalize failure".to_string())),
        )
        .expect_err("finalize failure should bubble up");

        assert!(
            err.to_string().contains("simulated finalize failure"),
            "unexpected error: {err}"
        );

        let leftover_files: Vec<_> = std::fs::read_dir(temp_dir.path())
            .expect("read temp dir")
            .map(|entry| entry.expect("dir entry").path())
            .collect();
        assert!(
            leftover_files.is_empty(),
            "temporary settings file should be removed on failure, found: {leftover_files:?}"
        );
    }

    #[test]
    fn prepare_launch_writes_claude_env_settings_file() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let provider = Provider::with_id(
            "demo".to_string(),
            "Demo".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "sk-demo",
                    "ANTHROPIC_BASE_URL": "https://api.example.com"
                }
            }),
            None,
        );

        let prepared = prepare_launch_with(&provider, temp_dir.path(), || {
            Ok(PathBuf::from("/usr/bin/claude"))
        })
        .expect("prepare launch");

        assert_eq!(prepared.executable, PathBuf::from("/usr/bin/claude"));
        let written: Value = serde_json::from_str(
            &std::fs::read_to_string(&prepared.settings_path).expect("read temp settings"),
        )
        .expect("parse temp settings");
        assert_eq!(
            written,
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "sk-demo",
                    "ANTHROPIC_BASE_URL": "https://api.example.com"
                }
            })
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = std::fs::metadata(&prepared.settings_path)
                .expect("stat temp settings")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn missing_claude_binary_reports_an_error() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let provider = Provider::with_id(
            "demo".to_string(),
            "Demo".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "sk-demo"
                }
            }),
            None,
        );

        let err = prepare_launch_with(&provider, temp_dir.path(), || {
            Err(AppError::Message("claude binary is missing".to_string()))
        })
        .expect_err("missing binary should fail");

        assert!(err.to_string().contains("claude"));
    }

    #[test]
    fn prepare_launch_writes_model_overrides_to_temp_file() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let provider = Provider::with_id(
            "glm".to_string(),
            "GLM".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "sk-glm",
                    "ANTHROPIC_BASE_URL": "https://open.bigmodel.cn/api/paas/v4",
                    "ANTHROPIC_DEFAULT_SONNET_MODEL": "glm-5.1",
                    "ANTHROPIC_DEFAULT_OPUS_MODEL": "glm-5.1"
                }
            }),
            None,
        );

        let prepared = prepare_launch_with(&provider, temp_dir.path(), || {
            Ok(PathBuf::from("/usr/bin/claude"))
        })
        .expect("prepare launch");

        let written: Value = serde_json::from_str(
            &std::fs::read_to_string(&prepared.settings_path).expect("read temp settings"),
        )
        .expect("parse temp settings");

        let env = written.get("env").expect("env exists");
        assert_eq!(env["ANTHROPIC_DEFAULT_SONNET_MODEL"], "glm-5.1");
        assert_eq!(env["ANTHROPIC_DEFAULT_OPUS_MODEL"], "glm-5.1");
    }

    #[test]
    fn prepare_launch_migrates_legacy_small_fast_model_key() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let provider = Provider::with_id(
            "legacy".to_string(),
            "Legacy".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "sk-legacy",
                    "ANTHROPIC_BASE_URL": "https://api.example.com",
                    "ANTHROPIC_SMALL_FAST_MODEL": "my-fast-model"
                }
            }),
            None,
        );

        let prepared = prepare_launch_with(&provider, temp_dir.path(), || {
            Ok(PathBuf::from("/usr/bin/claude"))
        })
        .expect("prepare launch");

        let written: Value = serde_json::from_str(
            &std::fs::read_to_string(&prepared.settings_path).expect("read temp settings"),
        )
        .expect("parse temp settings");

        let env = written.get("env").expect("env exists");
        assert!(
            env.get("ANTHROPIC_SMALL_FAST_MODEL").is_none(),
            "legacy key should be removed"
        );
        assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "my-fast-model");
    }

    #[test]
    fn prepare_launch_writes_full_settings_config_not_only_env() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let provider = Provider::with_id(
            "full".to_string(),
            "Full".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "sk-full"
                },
                "permissions": {
                    "allow": ["Bash(git*)"]
                }
            }),
            None,
        );

        let prepared = prepare_launch_with(&provider, temp_dir.path(), || {
            Ok(PathBuf::from("/usr/bin/claude"))
        })
        .expect("prepare launch");

        let written: Value = serde_json::from_str(
            &std::fs::read_to_string(&prepared.settings_path).expect("read temp settings"),
        )
        .expect("parse temp settings");

        assert_eq!(written["env"]["ANTHROPIC_AUTH_TOKEN"], "sk-full");
        assert_eq!(written["permissions"]["allow"], json!(["Bash(git*)"]));
    }

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
        assert_eq!(
            quote_windows_arg_for_cmd("foo\"bar"),
            "\"foo\\\"bar\""
        );
    }

    #[cfg(windows)]
    #[test]
    fn quote_windows_arg_for_cmd_quotes_spaces_and_specials() {
        assert_eq!(
            quote_windows_arg_for_cmd("foo & bar"),
            "\"foo & bar\""
        );
    }

    #[cfg(windows)]
    #[test]
    fn quote_windows_arg_for_cmd_leaves_plain_args_unchanged() {
        assert_eq!(quote_windows_arg_for_cmd("normal"), "normal");
        assert_eq!(quote_windows_arg_for_cmd("C:\\path\\file.exe"), "C:\\path\\file.exe");
    }

    #[cfg(windows)]
    #[test]
    fn quote_windows_arg_for_cmd_handles_empty_string() {
        assert_eq!(quote_windows_arg_for_cmd(""), "\"\"");
    }

    #[cfg(windows)]
    #[test]
    fn is_cmd_shim_matches_cmd_and_bat_case_insensitive() {
        assert!(is_cmd_shim(std::path::Path::new("C:\\bin\\claude.cmd")));
        assert!(is_cmd_shim(std::path::Path::new("C:\\bin\\claude.CMD")));
        assert!(is_cmd_shim(std::path::Path::new("C:\\bin\\claude.bat")));
        assert!(is_cmd_shim(std::path::Path::new("C:\\bin\\claude.BAT")));
        assert!(!is_cmd_shim(std::path::Path::new("C:\\bin\\claude.exe")));
        assert!(!is_cmd_shim(std::path::Path::new("C:\\bin\\claude")));
    }

    #[cfg(windows)]
    #[test]
    fn arg_requires_cmd_quote_recognizes_unsafe_inputs() {
        assert!(arg_requires_cmd_quote(""));
        assert!(arg_requires_cmd_quote("foo bar"));
        assert!(arg_requires_cmd_quote("foo\tbar"));
        assert!(arg_requires_cmd_quote("a&b"));
        assert!(arg_requires_cmd_quote("a|b"));
        assert!(arg_requires_cmd_quote("a%b"));
        assert!(arg_requires_cmd_quote("a!b"));
        assert!(arg_requires_cmd_quote("a(b"));
        assert!(arg_requires_cmd_quote("a)b"));
        assert!(!arg_requires_cmd_quote("plain"));
        assert!(!arg_requires_cmd_quote("C:\\work\\"));
        assert!(!arg_requires_cmd_quote("--project-dir=C:\\tmp\\"));
    }

    #[cfg(windows)]
    #[test]
    fn build_windows_cmdline_accepts_plain_trailing_backslash_paths() {
        let prepared = PreparedClaudeLaunch {
            executable: PathBuf::from("C:\\bin\\claude.cmd"),
            settings_path: PathBuf::from("C:\\tmp\\settings.json"),
        };
        let native_args = vec![
            OsString::from("C:\\work\\"),
            OsString::from("--project-dir=C:\\tmp\\"),
        ];

        let cmdline = build_windows_cmdline(&prepared, &native_args)
            .expect("plain trailing backslash should be allowed");

        let cmdline_str = String::from_utf16_lossy(&cmdline);
        assert!(cmdline_str.contains("cmd.exe /c"));
        assert!(cmdline_str.contains("C:\\work\\"));
        assert!(cmdline_str.contains("--project-dir=C:\\tmp\\"));
    }

    #[cfg(windows)]
    #[test]
    fn build_windows_cmdline_rejects_trailing_backslash_when_quoting_required() {
        let prepared = PreparedClaudeLaunch {
            executable: PathBuf::from("C:\\bin\\claude.cmd"),
            settings_path: PathBuf::from("C:\\tmp\\settings.json"),
        };
        let native_args = vec![OsString::from("C:\\Program Files\\dir\\")];

        let err = build_windows_cmdline(&prepared, &native_args)
            .expect_err("space + trailing backslash must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("backslash") || msg.contains("反斜杠"));
    }

    #[cfg(windows)]
    #[test]
    fn build_windows_cmdline_rejects_trailing_backslash_with_special_char() {
        let prepared = PreparedClaudeLaunch {
            executable: PathBuf::from("C:\\bin\\claude.cmd"),
            settings_path: PathBuf::from("C:\\tmp\\settings.json"),
        };
        let native_args = vec![OsString::from("a&b\\")];

        let err = build_windows_cmdline(&prepared, &native_args)
            .expect_err("special char + trailing backslash must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("backslash") || msg.contains("反斜杠"));
    }

    #[cfg(windows)]
    #[test]
    fn build_windows_cmdline_passes_through_for_direct_binary() {
        let prepared = PreparedClaudeLaunch {
            executable: PathBuf::from("C:\\bin\\claude.exe"),
            settings_path: PathBuf::from("C:\\tmp\\settings.json"),
        };
        // Direct .exe path skips cmd quoting; trailing backslash is fine even
        // alongside chars that would normally require quoting.
        let native_args = vec![
            OsString::from("C:\\work\\"),
            OsString::from("--project-dir=C:\\Program Files\\dir\\"),
        ];

        let cmdline = build_windows_cmdline(&prepared, &native_args)
            .expect("direct .exe must accept trailing backslash regardless of quoting");
        let cmdline_str = String::from_utf16_lossy(&cmdline);
        assert!(cmdline_str.contains("C:\\work\\"));
        assert!(cmdline_str.contains("Program Files"));
    }

    #[test]
    fn prepare_launch_from_settings_writes_exact_effective_snapshot() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let provider = Provider::with_id(
            "demo".to_string(),
            "Demo".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "sk-demo",
                    "ANTHROPIC_BASE_URL": "https://provider.example"
                },
                "permissions": {
                    "allow": ["Bash(git status)"]
                },
                "includeCoAuthoredBy": true
            }),
            None,
        );

        let effective = ProviderService::build_effective_live_snapshot(
            &AppType::Claude,
            &provider,
            Some(
                r#"{"env":{"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":1,"ANTHROPIC_BASE_URL":"https://common.example"},"permissions":{"allow":["Bash(ls)"]},"includeCoAuthoredBy":false}"#,
            ),
            true,
        )
        .expect("build effective snapshot");

        let prepared = prepare_launch_from_settings(&provider.id, &effective, temp_dir.path())
            .expect("prepare launch from effective settings");

        let written: Value = serde_json::from_str(
            &std::fs::read_to_string(&prepared.settings_path).expect("read temp settings"),
        )
        .expect("parse temp settings");

        assert_eq!(
            written, effective,
            "temp launch settings should exactly match the canonical effective snapshot"
        );
    }
}
