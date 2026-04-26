use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::codex_config::validate_config_toml;
use crate::error::AppError;
use crate::provider::Provider;
use serde_json::Value;

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::ptr;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, FALSE, HANDLE, TRUE};
#[cfg(windows)]
use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
#[cfg(windows)]
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    CreateProcessW, GetExitCodeProcess, ResumeThread, TerminateProcess, WaitForSingleObject,
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, INFINITE, PROCESS_INFORMATION, STARTUPINFOW,
};

#[derive(Debug, Clone)]
pub(crate) struct PreparedCodexLaunch {
    pub(crate) executable: PathBuf,
    pub(crate) codex_home: PathBuf,
}

impl PreparedCodexLaunch {
    pub(crate) fn cleanup_home_dir(&self) -> Result<(), AppError> {
        cleanup_temp_codex_home(&self.codex_home)
    }
}

pub(crate) fn prepare_launch(
    provider: &Provider,
    temp_dir: &Path,
) -> Result<PreparedCodexLaunch, AppError> {
    prepare_launch_with(provider, temp_dir, resolve_codex_binary)
}

pub(crate) fn prepare_launch_with<Resolve>(
    provider: &Provider,
    temp_dir: &Path,
    resolve_codex_binary: Resolve,
) -> Result<PreparedCodexLaunch, AppError>
where
    Resolve: FnOnce() -> Result<PathBuf, AppError>,
{
    let executable = resolve_codex_binary()?;
    let codex_home = write_temp_codex_home(temp_dir, provider)?;
    Ok(PreparedCodexLaunch {
        executable,
        codex_home,
    })
}

pub(crate) fn resolve_codex_binary() -> Result<PathBuf, AppError> {
    which::which("codex").map_err(|_| {
        AppError::localized(
            "codex.temp_launch_missing_binary",
            "未找到 codex 命令，请先安装 Codex CLI。".to_string(),
            "Could not find `codex` in PATH. Install Codex CLI first.".to_string(),
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

#[cfg(not(any(unix, windows)))]
pub(crate) fn ensure_temp_launch_supported() -> Result<(), AppError> {
    Err(AppError::localized(
        "codex.temp_launch_unsupported_platform",
        "当前平台暂不支持在当前终端临时启动 Codex。".to_string(),
        "Temporary Codex launch in the current terminal is not supported on this platform."
            .to_string(),
    ))
}

#[cfg(unix)]
pub(crate) fn build_handoff_command(
    prepared: &PreparedCodexLaunch,
    native_args: &[OsString],
) -> std::process::Command {
    let mut command = std::process::Command::new("/bin/sh");
    command.arg("-c").arg(
        "codex_home=\"$1\"; codex_bin=\"$2\"; shift 2; exit_status=0; cleanup() { rm -rf -- \"$codex_home\"; cleanup_status=$?; if [ \"$cleanup_status\" -ne 0 ]; then printf '%s\\n' \"cc-switch: failed to remove temporary Codex home: $codex_home\" >&2; if [ \"$exit_status\" -eq 0 ]; then exit_status=$cleanup_status; fi; fi; }; on_signal() { exit_status=\"$1\"; trap - INT TERM HUP; cleanup; exit \"$exit_status\"; }; trap 'on_signal 130' INT; trap 'on_signal 143' TERM; trap 'on_signal 129' HUP; export CODEX_HOME=\"$codex_home\"; \"$codex_bin\" \"$@\"; exit_status=$?; cleanup; exit \"$exit_status\"",
    );
    command.arg("cc-switch-codex-handoff");
    command.arg(&prepared.codex_home);
    command.arg(&prepared.executable);
    command.args(native_args);
    command
}

#[cfg(unix)]
pub(crate) fn exec_prepared_codex(
    prepared: &PreparedCodexLaunch,
    native_args: &[OsString],
) -> Result<(), AppError> {
    use std::os::unix::process::CommandExt;

    let exec_err = build_handoff_command(prepared, native_args).exec();
    Err(AppError::localized(
        "codex.temp_launch_exec_failed",
        format!("启动 Codex 失败: {exec_err}"),
        format!("Failed to launch Codex: {exec_err}"),
    ))
}

#[cfg(windows)]
pub(crate) fn exec_prepared_codex(
    prepared: &PreparedCodexLaunch,
    native_args: &[OsString],
) -> Result<(), AppError> {
    let _ctrl_guard = ScopedConsoleCtrlHandler::install()?;

    let (program, args) = build_command_windows(prepared, native_args)?;

    let env_block = build_env_block_with_override("CODEX_HOME", prepared.codex_home.as_os_str());

    let (process_handle, thread_handle) = spawn_suspended_createprocessw(&program, &args, Some(&env_block))?;

    let job = Job::create_with_kill_on_close()?;

    if let Err(e) = job.try_assign(process_handle) {
        log::warn!("{}", AppError::windows_job_assign_failed_fallback(&e));
    }

    let resume_result = unsafe { ResumeThread(thread_handle) };
    if resume_result == u32::MAX {
        let code = unsafe { GetLastError() };
        unsafe {
            let _ = TerminateProcess(process_handle, 1);
            CloseHandle(thread_handle);
            CloseHandle(process_handle);
        }
        return Err(AppError::windows_resume_thread_failed(code));
    }

    unsafe { CloseHandle(thread_handle) };

    let exit_code = match wait_for_child(process_handle) {
        Ok(code) => code,
        Err(e) => {
            unsafe { CloseHandle(process_handle) };
            return Err(e);
        }
    };
    unsafe { CloseHandle(process_handle) };

    if exit_code != 0 {
        return Err(AppError::localized(
            "codex.temp_launch_exit_nonzero",
            format!("Codex 进程退出码非零: {exit_code}"),
            format!("Codex process exited with non-zero code: {exit_code}"),
        ));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn exec_prepared_codex(
    _prepared: &PreparedCodexLaunch,
    _native_args: &[OsString],
) -> Result<(), AppError> {
    Err(AppError::localized(
        "codex.temp_launch_unsupported_platform",
        "当前平台暂不支持在当前终端临时启动 Codex。".to_string(),
        "Temporary Codex launch in the current terminal is not supported on this platform."
            .to_string(),
    ))
}

#[cfg(windows)]
fn build_command_windows(
    prepared: &PreparedCodexLaunch,
    native_args: &[OsString],
) -> Result<(std::path::PathBuf, Vec<OsString>), AppError> {
    let exec_str = prepared.executable.to_string_lossy();
    if exec_str.ends_with(".cmd") || exec_str.ends_with(".bat") {
        // cmd.exe expands %VAR% and !VAR! (delayed expansion) even inside
        // double quotes. There is no standard escape for these in a /c
        // command line. Without refactoring to bypass cmd.exe /c entirely
        // (e.g. parse the .cmd shim and invoke the underlying binary
        // directly), this expansion cannot be fully avoided. Log a warning
        // so users are aware.
        //
        // Additionally, cmd.exe does not treat backslash as a quote escape,
        // so arguments containing a literal double quote cannot be safely
        // passed through cmd.exe /c. We reject such arguments.
        for arg in native_args {
            let s = arg.to_string_lossy();
            if s.contains('%') || s.contains('!') {
                log::warn!(
                    target: "codex_temp_launch",
                    "Native arg contains % or ! which cmd.exe may expand: {}",
                    s
                );
            }
            if s.contains('"') {
                return Err(AppError::localized(
                    "codex.temp_launch_unsafe_cmd_quote",
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
            if s.ends_with('\\') {
                return Err(AppError::localized(
                    "codex.temp_launch_unsafe_cmd_trailing_backslash",
                    format!(
                        "参数以反斜杠结尾，无法安全地通过 cmd.exe /c 传递: {}",
                        s
                    ),
                    format!(
                        "Native arg ends with a backslash which cannot be safely passed through cmd.exe /c: {}",
                        s
                    ),
                ));
            }
        }
        let mut args = vec![OsString::from("/c"), OsString::from(&prepared.executable)];
        args.extend_from_slice(native_args);
        Ok((std::path::PathBuf::from("cmd.exe"), args))
    } else {
        Ok((prepared.executable.clone(), native_args.to_vec()))
    }
}

#[cfg(windows)]
fn build_windows_command_line(program: &std::ffi::OsStr, args: &[OsString]) -> Vec<u16> {
    let program_str = program.to_string_lossy();
    let is_cmd = program_str.eq_ignore_ascii_case("cmd.exe")
        || program_str.eq_ignore_ascii_case("cmd");

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
fn quote_windows_arg(arg: &str) -> String {
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
fn spawn_suspended_createprocessw(
    program: &std::path::Path,
    args: &[OsString],
    env_block: Option<&[u16]>,
) -> Result<(HANDLE, HANDLE), AppError> {
    let program_wide: Vec<u16> = std::ffi::OsStr::new(program)
        .encode_wide()
        .chain(Some(0))
        .collect();

    let mut command_line = build_windows_command_line(std::ffi::OsStr::new(program), args);

    let mut startup_info: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup_info.cb = std::mem::size_of::<STARTUPINFOW>() as u32;

    let mut process_info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

    let env_ptr = env_block
        .map(|b| b.as_ptr() as *mut _)
        .unwrap_or(ptr::null_mut());

    let result = unsafe {
        CreateProcessW(
            program_wide.as_ptr(),
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
struct Job {
    handle: HANDLE,
}

#[cfg(windows)]
impl Job {
    fn create_with_kill_on_close() -> Result<Self, AppError> {
        unsafe {
            let handle = CreateJobObjectW(ptr::null_mut(), ptr::null());
            if handle.is_null() {
                let code = GetLastError();
                return Err(AppError::localized(
                    "windows.create_job_object_failed",
                    format!("创建 Job Object 失败，Win32 错误码: {code}"),
                    format!("Failed to create Job Object, Win32 error: {code}"),
                ));
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
                return Err(AppError::localized(
                    "windows.set_job_information_failed",
                    format!("设置 Job Object 信息失败，Win32 错误码: {code}"),
                    format!("Failed to set Job Object information, Win32 error: {code}"),
                ));
            }

            Ok(Job { handle })
        }
    }

    fn try_assign(&self, process: HANDLE) -> Result<(), std::io::Error> {
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
            CloseHandle(self.handle);
        }
    }
}

#[cfg(windows)]
struct ScopedConsoleCtrlHandler;

#[cfg(windows)]
impl ScopedConsoleCtrlHandler {
    fn install() -> Result<Self, AppError> {
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
            SetConsoleCtrlHandler(Some(ctrl_handler_swallow), FALSE);
        }
    }
}

#[cfg(windows)]
unsafe extern "system" fn ctrl_handler_swallow(_ctrl_type: u32) -> i32 {
    TRUE
}

#[cfg(windows)]
fn wait_for_child(process_handle: HANDLE) -> Result<u32, AppError> {
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

#[cfg(windows)]
fn build_env_block_with_override(key: &str, value: &std::ffi::OsStr) -> Vec<u16> {
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

fn write_temp_codex_home(temp_dir: &Path, provider: &Provider) -> Result<PathBuf, AppError> {
    write_temp_codex_home_with(temp_dir, provider, finalize_temp_codex_home)
}

fn write_temp_codex_home_with<Finalize>(
    temp_dir: &Path,
    provider: &Provider,
    finalize: Finalize,
) -> Result<PathBuf, AppError>
where
    Finalize: FnOnce(&Path) -> Result<(), AppError>,
{
    let settings = provider.settings_config.as_object().ok_or_else(|| {
        AppError::localized(
            "codex.temp_launch_settings_not_object",
            format!("供应商 {} 的 Codex 配置必须是 JSON 对象。", provider.id),
            format!(
                "Provider {} Codex configuration must be a JSON object.",
                provider.id
            ),
        )
    })?;

    let config_text = match settings.get("config") {
        Some(Value::String(text)) => text.as_str(),
        Some(Value::Null) | None => "",
        Some(_) => {
            return Err(AppError::localized(
                "codex.temp_launch_config_invalid_type",
                format!("供应商 {} 的 config 必须是字符串。", provider.id),
                format!("Provider {} config must be a string.", provider.id),
            ))
        }
    };
    validate_config_toml(config_text)?;

    let auth = match settings.get("auth") {
        Some(Value::Object(auth)) if !auth.is_empty() => Some(Value::Object(auth.clone())),
        Some(Value::Object(_)) | Some(Value::Null) | None => None,
        Some(_) => {
            return Err(AppError::localized(
                "codex.temp_launch_auth_invalid_type",
                format!("供应商 {} 的 auth 必须是 JSON 对象。", provider.id),
                format!("Provider {} auth must be a JSON object.", provider.id),
            ))
        }
    };

    #[cfg(windows)]
    let timestamp = crate::cli::orphan_scan::current_process_creation_time_nanos();
    #[cfg(not(windows))]
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir_name = format!(
        "cc-switch-codex-{}-{}-{timestamp}",
        sanitize_filename_fragment(&provider.id),
        std::process::id()
    );
    let codex_home = temp_dir.join(dir_name);

    let write_result = (|| {
        fs::create_dir_all(&codex_home).map_err(|err| AppError::io(&codex_home, err))?;
        finalize(&codex_home)?;

        let config_path = codex_home.join("config.toml");
        write_secret_file(&config_path, config_text.as_bytes())?;

        if let Some(auth) = auth {
            let auth_path = codex_home.join("auth.json");
            let auth_text = serde_json::to_vec_pretty(&auth)
                .map_err(|source| AppError::JsonSerialize { source })?;
            write_secret_file(&auth_path, &auth_text)?;
        }

        Ok(())
    })();

    match write_result {
        Ok(()) => Ok(codex_home),
        Err(err) => match cleanup_temp_codex_home(&codex_home) {
            Ok(()) => Err(err),
            Err(cleanup_err) => Err(AppError::localized(
                "codex.temp_launch_tempdir_cleanup_failed",
                format!("写入临时 Codex 配置目录失败: {err}；同时清理失败: {cleanup_err}"),
                format!(
                    "Failed to write the temporary Codex home: {err}; also failed to clean it up: {cleanup_err}"
                ),
            )),
        },
    }
}

#[cfg(unix)]
fn finalize_temp_codex_home(path: &Path) -> Result<(), AppError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|err| AppError::io(path, err))
}

#[cfg(not(unix))]
fn finalize_temp_codex_home(path: &Path) -> Result<(), AppError> {
    #[cfg(windows)]
    {
        restrict_to_owner(path, true)?;
    }
    Ok(())
}

fn write_secret_file(path: &Path, content: &[u8]) -> Result<(), AppError> {
    let mut file = create_secret_temp_file(path)?;
    file.write_all(content)
        .and_then(|()| file.flush())
        .map_err(|err| AppError::io(path, err))
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

fn cleanup_temp_codex_home(path: &Path) -> Result<(), AppError> {
    match fs::remove_dir_all(path) {
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
    #[cfg(unix)]
    use std::ffi::OsString;
    #[cfg(unix)]
    use std::os::unix::{fs::PermissionsExt, process::CommandExt};
    #[cfg(unix)]
    use std::process::Stdio;
    #[cfg(unix)]
    use std::time::Duration;
    use tempfile::TempDir;

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
    fn quote_windows_arg_handles_newlines() {
        assert_eq!(quote_windows_arg("foo\nbar"), "\"foo\nbar\"");
    }

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

    fn provider_with(config: &str, auth: Option<Value>) -> Provider {
        let mut settings = serde_json::Map::new();
        settings.insert("config".to_string(), Value::String(config.to_string()));
        if let Some(auth) = auth {
            settings.insert("auth".to_string(), auth);
        }
        Provider::with_id(
            "demo".to_string(),
            "Demo".to_string(),
            Value::Object(settings),
            None,
        )
    }

    fn official_provider_with_auth(config: &str) -> Provider {
        let mut provider = provider_with(
            config,
            Some(serde_json::json!({ "OPENAI_API_KEY": "stale-key" })),
        );
        provider.website_url = Some("https://chatgpt.com/codex".to_string());
        provider
    }

    #[cfg(unix)]
    #[test]
    fn unix_handoff_command_exports_codex_home_and_cleans_up_temp_dir() {
        let prepared = PreparedCodexLaunch {
            executable: PathBuf::from("/usr/local/bin/codex"),
            codex_home: PathBuf::from("/tmp/cc-switch-codex-home"),
        };
        let native_args = vec![OsString::from("--model"), OsString::from("gpt-5.4")];

        let command = build_handoff_command(&prepared, &native_args);
        let args: Vec<OsString> = command.get_args().map(|arg| arg.to_os_string()).collect();

        assert_eq!(command.get_program(), std::path::Path::new("/bin/sh"));
        assert_eq!(
            args,
            vec![
                OsString::from("-c"),
                OsString::from(
                    "codex_home=\"$1\"; codex_bin=\"$2\"; shift 2; exit_status=0; cleanup() { rm -rf -- \"$codex_home\"; cleanup_status=$?; if [ \"$cleanup_status\" -ne 0 ]; then printf '%s\\n' \"cc-switch: failed to remove temporary Codex home: $codex_home\" >&2; if [ \"$exit_status\" -eq 0 ]; then exit_status=$cleanup_status; fi; fi; }; on_signal() { exit_status=\"$1\"; trap - INT TERM HUP; cleanup; exit \"$exit_status\"; }; trap 'on_signal 130' INT; trap 'on_signal 143' TERM; trap 'on_signal 129' HUP; export CODEX_HOME=\"$codex_home\"; \"$codex_bin\" \"$@\"; exit_status=$?; cleanup; exit \"$exit_status\""
                ),
                OsString::from("cc-switch-codex-handoff"),
                OsString::from("/tmp/cc-switch-codex-home"),
                OsString::from("/usr/local/bin/codex"),
                OsString::from("--model"),
                OsString::from("gpt-5.4"),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn interrupting_handoff_still_cleans_up_temp_codex_home() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let executable = write_test_executable(
            &temp_dir,
            "codex-stub.sh",
            "trap 'exit 130' INT TERM HUP\nwhile :; do sleep 1; done",
        );
        let codex_home = temp_dir.path().join("cc-switch-codex-home");
        std::fs::create_dir_all(&codex_home).expect("create temp codex home");
        std::fs::write(codex_home.join("config.toml"), "model = \"gpt-5.4\"\n")
            .expect("seed config");

        let prepared = PreparedCodexLaunch {
            executable,
            codex_home: codex_home.clone(),
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
            !codex_home.exists(),
            "temporary Codex home should be removed after interrupt"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_failure_after_successful_handoff_surfaces_nonzero_exit() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let executable = write_test_executable(&temp_dir, "codex-stub.sh", "exit 0");
        let prepared = PreparedCodexLaunch {
            executable,
            codex_home: PathBuf::from("."),
        };

        let mut command = build_handoff_command(&prepared, &[]);
        command.current_dir(temp_dir.path());
        let output = command.output().expect("run handoff");

        assert!(
            !output.status.success(),
            "cleanup failure should not look like a successful handoff"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("failed to remove temporary Codex home"));
    }

    #[test]
    fn temp_codex_home_is_removed_when_finalize_step_fails() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let provider = provider_with(
            "model_provider = \"demo\"\n",
            Some(serde_json::json!({ "OPENAI_API_KEY": "sk-demo" })),
        );

        let err = write_temp_codex_home_with(temp_dir.path(), &provider, |_| {
            Err(AppError::Message("simulated finalize failure".to_string()))
        })
        .expect_err("finalize failure should bubble up");

        assert!(err.to_string().contains("simulated finalize failure"));
        assert!(
            std::fs::read_dir(temp_dir.path())
                .expect("read temp dir")
                .next()
                .is_none(),
            "temporary Codex home should be removed on failure"
        );
    }

    #[test]
    fn prepare_launch_writes_codex_home_files() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let provider = provider_with(
            "model_provider = \"demo\"\nmodel = \"gpt-5.2-codex\"\n",
            Some(serde_json::json!({ "OPENAI_API_KEY": "sk-demo" })),
        );

        let prepared = prepare_launch_with(&provider, temp_dir.path(), || {
            Ok(PathBuf::from("/usr/bin/codex"))
        })
        .expect("prepare launch");

        assert_eq!(prepared.executable, PathBuf::from("/usr/bin/codex"));
        assert_eq!(
            std::fs::read_to_string(prepared.codex_home.join("config.toml"))
                .expect("read config.toml"),
            "model_provider = \"demo\"\nmodel = \"gpt-5.2-codex\"\n"
        );
        let auth: Value = serde_json::from_str(
            &std::fs::read_to_string(prepared.codex_home.join("auth.json"))
                .expect("read auth.json"),
        )
        .expect("parse auth.json");
        assert_eq!(auth, serde_json::json!({ "OPENAI_API_KEY": "sk-demo" }));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let dir_mode = std::fs::metadata(&prepared.codex_home)
                .expect("stat codex home")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dir_mode, 0o700);

            let auth_mode = std::fs::metadata(prepared.codex_home.join("auth.json"))
                .expect("stat auth.json")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(auth_mode, 0o600);
        }
    }

    #[test]
    fn prepare_launch_allows_missing_auth_for_official_style_providers() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let provider = provider_with("model_provider = \"openai\"\n", None);

        let prepared = prepare_launch_with(&provider, temp_dir.path(), || {
            Ok(PathBuf::from("/usr/bin/codex"))
        })
        .expect("prepare launch");

        assert!(!prepared.codex_home.join("auth.json").exists());
    }

    #[test]
    fn prepare_launch_writes_auth_file_for_official_provider() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let provider = official_provider_with_auth("model_provider = \"openai\"\n");

        let prepared = prepare_launch_with(&provider, temp_dir.path(), || {
            Ok(PathBuf::from("/usr/bin/codex"))
        })
        .expect("prepare launch");

        assert!(prepared.codex_home.join("auth.json").exists());
    }

    #[test]
    fn missing_codex_binary_reports_an_error() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let provider = provider_with("model_provider = \"demo\"\n", None);

        let err = prepare_launch_with(&provider, temp_dir.path(), || {
            Err(AppError::Message("codex binary is missing".to_string()))
        })
        .expect_err("missing binary should fail");

        assert!(err.to_string().contains("codex"));
    }
}
