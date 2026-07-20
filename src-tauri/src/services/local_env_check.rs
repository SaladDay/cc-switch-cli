use crate::app_config::AppType;
use futures::{stream, StreamExt};
use regex::Regex;
use std::future::Future;
use std::path::Path;
use std::process::{Output, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::Notify;
use tokio::time::{timeout, timeout_at, Instant};

const DEFAULT_TOOL_VERSION_TIMEOUT: Duration = Duration::from_secs(5);
const HERMES_VERSION_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CONCURRENT_VERSION_CHECKS: usize = 2;
const MAX_CAPTURED_OUTPUT_BYTES: usize = 16 * 1024;
const PROCESS_REAP_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocalTool {
    Claude,
    Codex,
    Gemini,
    OpenCode,
    Hermes,
    OpenClaw,
}

impl LocalTool {
    pub const ALL: [LocalTool; 6] = [
        LocalTool::Claude,
        LocalTool::Codex,
        LocalTool::Gemini,
        LocalTool::OpenCode,
        LocalTool::Hermes,
        LocalTool::OpenClaw,
    ];

    pub fn all() -> &'static [LocalTool] {
        &Self::ALL
    }

    pub fn display_name(self) -> &'static str {
        match self {
            LocalTool::Claude => "Claude",
            LocalTool::Codex => "Codex",
            LocalTool::Gemini => "Gemini",
            LocalTool::OpenCode => "OpenCode",
            LocalTool::Hermes => "Hermes",
            LocalTool::OpenClaw => "OpenClaw",
        }
    }

    fn binary_name(self) -> &'static str {
        match self {
            LocalTool::Claude => "claude",
            LocalTool::Codex => "codex",
            LocalTool::Gemini => "gemini",
            LocalTool::OpenCode => "opencode",
            LocalTool::Hermes => "hermes",
            LocalTool::OpenClaw => "openclaw",
        }
    }

    fn version_args(self) -> &'static [&'static str] {
        match self {
            LocalTool::Claude => &["--version", "version"],
            LocalTool::Codex => &["--version"],
            LocalTool::Gemini => &["--version", "-v"],
            LocalTool::OpenCode => &["--version", "version"],
            LocalTool::Hermes => &["--version", "version"],
            LocalTool::OpenClaw => &["--version", "version"],
        }
    }

    fn version_timeout(self) -> Duration {
        if matches!(self, LocalTool::Hermes) {
            HERMES_VERSION_TIMEOUT
        } else {
            DEFAULT_TOOL_VERSION_TIMEOUT
        }
    }

    fn sort_key(self) -> usize {
        Self::ALL
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or(Self::ALL.len())
    }

    pub fn from_app_type(app_type: &AppType) -> Self {
        match app_type {
            AppType::Claude => LocalTool::Claude,
            AppType::Codex => LocalTool::Codex,
            AppType::Gemini => LocalTool::Gemini,
            AppType::OpenCode => LocalTool::OpenCode,
            AppType::Hermes => LocalTool::Hermes,
            AppType::OpenClaw => LocalTool::OpenClaw,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCheckStatus {
    Ok { version: String },
    NotInstalledOrNotExecutable,
    VersionUnavailable { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCheckResult {
    pub tool: LocalTool,
    pub display_name: &'static str,
    pub status: ToolCheckStatus,
}

#[derive(Clone, Default)]
pub(crate) struct LocalEnvCancellation {
    inner: Arc<LocalEnvCancellationInner>,
}

#[derive(Default)]
struct LocalEnvCancellationInner {
    cancelled: AtomicBool,
    notify: Notify,
}

impl LocalEnvCancellation {
    pub(crate) fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::Release);
        self.inner.notify.notify_waiters();
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    async fn cancelled(&self) {
        loop {
            if self.is_cancelled() {
                return;
            }
            let notified = self.inner.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

pub fn check_local_environment() -> Vec<ToolCheckResult> {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            return LocalTool::all()
                .iter()
                .copied()
                .map(|tool| {
                    unavailable_result(tool, format!("version worker unavailable: {error}"))
                })
                .collect();
        }
    };

    let mut results = Vec::with_capacity(LocalTool::all().len());
    runtime.block_on(check_local_environment_progressive(
        LocalEnvCancellation::default(),
        |result| results.push(result),
    ));
    results.sort_by_key(|result| result.tool.sort_key());
    results
}

pub(crate) async fn check_local_environment_progressive<F>(
    cancellation: LocalEnvCancellation,
    on_result: F,
) where
    F: FnMut(ToolCheckResult),
{
    check_local_environment_progressive_with(cancellation, on_result, check_tool).await;
}

async fn check_local_environment_progressive_with<F, P, ProbeFuture>(
    cancellation: LocalEnvCancellation,
    mut on_result: F,
    probe: P,
) where
    F: FnMut(ToolCheckResult),
    P: Fn(LocalTool, LocalEnvCancellation) -> ProbeFuture,
    ProbeFuture: Future<Output = Option<ToolCheckResult>>,
{
    let checks = stream::iter(
        LocalTool::all()
            .iter()
            .copied()
            .map(|tool| probe(tool, cancellation.clone())),
    )
    .buffer_unordered(MAX_CONCURRENT_VERSION_CHECKS);
    futures::pin_mut!(checks);

    while let Some(result) = checks.next().await {
        if let Some(result) = result {
            on_result(result);
        }
    }
}

pub fn check_tool_installed(app_type: &AppType) -> bool {
    let tool = LocalTool::from_app_type(app_type);
    is_tool_installed(tool.binary_name())
}

fn is_tool_installed(bin: &str) -> bool {
    which::which(bin).is_ok()
}

async fn check_tool(
    tool: LocalTool,
    cancellation: LocalEnvCancellation,
) -> Option<ToolCheckResult> {
    if cancellation.is_cancelled() {
        return None;
    }

    let executable = match which::which(tool.binary_name()) {
        Ok(path) => path,
        Err(_) => {
            return Some(ToolCheckResult {
                tool,
                display_name: tool.display_name(),
                status: ToolCheckStatus::NotInstalledOrNotExecutable,
            });
        }
    };

    let status = check_tool_version_at_path(
        &executable,
        tool.version_args(),
        tool.version_timeout(),
        cancellation,
    )
    .await?;

    if let ToolCheckStatus::VersionUnavailable { reason } = &status {
        log::debug!(
            "{} is installed, but its version is unavailable: {}",
            tool.display_name(),
            reason
        );
    }

    Some(ToolCheckResult {
        tool,
        display_name: tool.display_name(),
        status,
    })
}

async fn check_tool_version_at_path(
    executable: &Path,
    version_args: &[&str],
    timeout: Duration,
    cancellation: LocalEnvCancellation,
) -> Option<ToolCheckStatus> {
    let deadline = Instant::now() + timeout;
    let mut last_error = None::<String>;

    for arg in version_args {
        if cancellation.is_cancelled() {
            return None;
        }
        if Instant::now() >= deadline {
            return Some(ToolCheckStatus::VersionUnavailable {
                reason: format!("version check timed out after {}", format_duration(timeout)),
            });
        }

        match run_tool_version_command(executable, arg, deadline, cancellation.clone()).await {
            Ok(output) => {
                let combined = combined_tool_output(&output);
                if !output.status.success() {
                    last_error = Some(command_failure_summary(output.status, &combined));
                    continue;
                }

                if let Some(version) = parse_version(&combined) {
                    return Some(ToolCheckStatus::Ok { version });
                }

                last_error = Some(if combined.trim().is_empty() {
                    "no version output".to_string()
                } else {
                    format!(
                        "unrecognized version output: {}",
                        summarize_tool_output(&combined)
                    )
                });
            }
            Err(VersionCommandError::TimedOut) => {
                return Some(ToolCheckStatus::VersionUnavailable {
                    reason: format!("version check timed out after {}", format_duration(timeout)),
                });
            }
            Err(VersionCommandError::Cancelled) => return None,
            Err(VersionCommandError::Io(error)) => last_error = Some(error),
        }
    }

    Some(ToolCheckStatus::VersionUnavailable {
        reason: last_error.unwrap_or_else(|| "unable to detect version".to_string()),
    })
}

#[derive(Debug, PartialEq, Eq)]
enum VersionCommandError {
    TimedOut,
    Cancelled,
    Io(String),
}

struct VersionProcessGuard {
    attached: bool,
    #[cfg(unix)]
    process_group_id: Option<u32>,
    #[cfg(windows)]
    job: WindowsJob,
}

impl VersionProcessGuard {
    fn prepare() -> std::io::Result<Self> {
        Ok(Self {
            attached: false,
            #[cfg(unix)]
            process_group_id: None,
            #[cfg(windows)]
            job: WindowsJob::new()?,
        })
    }

    fn attach(&mut self, child: &Child) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            self.process_group_id = Some(child.id().ok_or_else(|| {
                std::io::Error::other("spawned version command has no process id")
            })?);
        }

        #[cfg(windows)]
        self.job.assign(child)?;

        self.attached = true;
        Ok(())
    }

    fn start(&self, child: &Child) -> std::io::Result<()> {
        #[cfg(windows)]
        self.job.resume_suspended_process(child)?;

        #[cfg(not(windows))]
        let _ = child;

        Ok(())
    }

    async fn terminate(&mut self, child: &mut Child) {
        let tree_killed = if self.attached {
            match self.kill_tree() {
                Ok(killed) => {
                    if killed {
                        self.disarm();
                    }
                    killed
                }
                Err(error) => {
                    log::warn!("failed to terminate version command process tree: {error}");
                    false
                }
            }
        } else {
            false
        };

        if !tree_killed {
            if let Err(error) = child.start_kill() {
                log::debug!("failed to terminate version command directly: {error}");
            }
        }

        match timeout(PROCESS_REAP_TIMEOUT, child.wait()).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => log::warn!("failed to reap version command: {error}"),
            Err(_) => log::warn!(
                "version command did not exit within {} after termination",
                format_duration(PROCESS_REAP_TIMEOUT)
            ),
        }
    }

    fn cleanup_after_completion(&mut self) -> std::io::Result<()> {
        // Windows descendants remain in the Job and are terminated when its
        // KILL_ON_JOB_CLOSE handle is dropped. Unix process groups have no
        // equivalent kernel-owned handle, so explicitly remove any background
        // processes that outlived a successful version command.
        #[cfg(unix)]
        if self.attached && self.kill_tree()? {
            self.disarm();
        }

        Ok(())
    }

    fn disarm(&mut self) {
        self.attached = false;
        #[cfg(unix)]
        {
            self.process_group_id = None;
        }
    }

    fn kill_tree(&self) -> std::io::Result<bool> {
        #[cfg(unix)]
        {
            let Some(process_group_id) = self.process_group_id else {
                return Ok(false);
            };
            kill_process_group(process_group_id)?;
            Ok(true)
        }

        #[cfg(windows)]
        {
            self.job.terminate()?;
            Ok(true)
        }

        #[cfg(not(any(unix, windows)))]
        {
            Ok(false)
        }
    }
}

impl Drop for VersionProcessGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if self.attached {
            let Some(process_group_id) = self.process_group_id.take() else {
                return;
            };
            if let Err(error) = kill_process_group(process_group_id) {
                log::warn!("failed to clean up dropped version command process group: {error}");
            }
        }
    }
}

#[cfg(windows)]
struct WindowsJob(std::os::windows::io::OwnedHandle);

#[cfg(windows)]
impl WindowsJob {
    fn new() -> std::io::Result<Self> {
        use std::os::windows::io::FromRawHandle;
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };

        let raw_handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw_handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let handle = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(raw_handle) };

        let mut limits = unsafe { std::mem::zeroed::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                raw_handle,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            return Err(std::io::Error::last_os_error());
        }

        Ok(Self(handle))
    }

    fn assign(&self, child: &Child) -> std::io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;

        let process_handle = child.raw_handle().ok_or_else(|| {
            std::io::Error::other("spawned version command has no process handle")
        })?;
        let assigned = unsafe {
            AssignProcessToJobObject(self.0.as_raw_handle().cast(), process_handle.cast())
        };
        if assigned == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    fn resume_suspended_process(&self, child: &Child) -> std::io::Result<()> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
        };
        use windows_sys::Win32::System::Threading::{
            GetProcessIdOfThread, OpenThread, ResumeThread, THREAD_QUERY_LIMITED_INFORMATION,
            THREAD_SUSPEND_RESUME,
        };

        let process_id = child
            .id()
            .ok_or_else(|| std::io::Error::other("spawned version command has no process id"))?;
        // Tokio keeps only the process handle. The process is still suspended here,
        // so reopen its primary thread before allowing any CLI code to execute.
        let raw_snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if raw_snapshot == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        let snapshot = unsafe { OwnedHandle::from_raw_handle(raw_snapshot) };

        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..THREADENTRY32::default()
        };
        let mut has_entry = unsafe {
            Thread32First(
                snapshot.as_raw_handle().cast(),
                std::ptr::from_mut(&mut entry),
            )
        } != 0;
        if !has_entry {
            return Err(std::io::Error::last_os_error());
        }

        let mut primary_thread_id = None;
        while has_entry {
            if entry.th32OwnerProcessID == process_id
                && primary_thread_id.replace(entry.th32ThreadID).is_some()
            {
                return Err(std::io::Error::other(format!(
                    "suspended version command {process_id} has multiple threads"
                )));
            }

            has_entry = unsafe {
                Thread32Next(
                    snapshot.as_raw_handle().cast(),
                    std::ptr::from_mut(&mut entry),
                )
            } != 0;
        }

        let thread_id = primary_thread_id.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "failed to find suspended version command thread",
            )
        })?;
        let raw_thread = unsafe {
            OpenThread(
                THREAD_SUSPEND_RESUME | THREAD_QUERY_LIMITED_INFORMATION,
                0,
                thread_id,
            )
        };
        if raw_thread.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let thread = unsafe { OwnedHandle::from_raw_handle(raw_thread) };
        let owner_process_id = unsafe { GetProcessIdOfThread(thread.as_raw_handle().cast()) };
        if owner_process_id == 0 {
            return Err(std::io::Error::last_os_error());
        }
        if owner_process_id != process_id {
            return Err(std::io::Error::other(
                "suspended version command thread id was reused",
            ));
        }

        let previous_suspend_count = unsafe { ResumeThread(thread.as_raw_handle().cast()) };
        if previous_suspend_count == u32::MAX {
            return Err(std::io::Error::last_os_error());
        }
        if previous_suspend_count != 1 {
            return Err(std::io::Error::other(format!(
                "unexpected version command suspend count: {previous_suspend_count}"
            )));
        }
        Ok(())
    }

    fn terminate(&self) -> std::io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;

        let terminated = unsafe { TerminateJobObject(self.0.as_raw_handle().cast(), 1) };
        if terminated == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

async fn run_tool_version_command(
    executable: &Path,
    arg: &str,
    deadline: Instant,
    cancellation: LocalEnvCancellation,
) -> Result<Output, VersionCommandError> {
    if cancellation.is_cancelled() {
        return Err(VersionCommandError::Cancelled);
    }

    let mut command = Command::new(executable);
    command
        .arg(arg)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    // Assign the process to its Job before the shim can create descendants.
    command.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);

    let mut process_guard = VersionProcessGuard::prepare()
        .map_err(|error| VersionCommandError::Io(error.to_string()))?;
    let mut child = command
        .spawn()
        .map_err(|error| VersionCommandError::Io(error.to_string()))?;
    if let Err(error) = process_guard.attach(&child) {
        process_guard.terminate(&mut child).await;
        return Err(VersionCommandError::Io(format!(
            "failed to supervise version command process tree: {error}"
        )));
    }
    if cancellation.is_cancelled() {
        process_guard.terminate(&mut child).await;
        return Err(VersionCommandError::Cancelled);
    }
    if let Err(error) = process_guard.start(&child) {
        process_guard.terminate(&mut child).await;
        return Err(VersionCommandError::Io(format!(
            "failed to start supervised version command: {error}"
        )));
    }
    let Some(stdout) = child.stdout.take() else {
        process_guard.terminate(&mut child).await;
        return Err(VersionCommandError::Io(
            "failed to capture version command stdout".to_string(),
        ));
    };
    let Some(stderr) = child.stderr.take() else {
        process_guard.terminate(&mut child).await;
        return Err(VersionCommandError::Io(
            "failed to capture version command stderr".to_string(),
        ));
    };

    enum CommandOutcome {
        Completed(std::io::Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>)>),
        TimedOut,
        Cancelled,
    }

    let outcome = {
        let collect = async {
            tokio::try_join!(
                child.wait(),
                drain_output_limited(stdout),
                drain_output_limited(stderr)
            )
        };

        tokio::select! {
            biased;
            _ = cancellation.cancelled() => CommandOutcome::Cancelled,
            result = timeout_at(deadline, collect) => match result {
                Ok(result) => CommandOutcome::Completed(result),
                Err(_) => CommandOutcome::TimedOut,
            },
        }
    };

    match outcome {
        CommandOutcome::Completed(Ok((status, stdout, stderr))) => {
            process_guard.cleanup_after_completion().map_err(|error| {
                VersionCommandError::Io(format!(
                    "failed to clean up completed version command process tree: {error}"
                ))
            })?;
            Ok(Output {
                status,
                stdout,
                stderr,
            })
        }
        CommandOutcome::Completed(Err(error)) => {
            process_guard.terminate(&mut child).await;
            Err(VersionCommandError::Io(error.to_string()))
        }
        CommandOutcome::TimedOut => {
            process_guard.terminate(&mut child).await;
            Err(VersionCommandError::TimedOut)
        }
        CommandOutcome::Cancelled => {
            process_guard.terminate(&mut child).await;
            Err(VersionCommandError::Cancelled)
        }
    }
}

async fn drain_output_limited<R>(mut reader: R) -> std::io::Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut captured = Vec::with_capacity(MAX_CAPTURED_OUTPUT_BYTES);
    let mut chunk = [0_u8; 4096];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            return Ok(captured);
        }

        let remaining = MAX_CAPTURED_OUTPUT_BYTES.saturating_sub(captured.len());
        if remaining > 0 {
            captured.extend_from_slice(&chunk[..read.min(remaining)]);
        }
    }
}

#[cfg(unix)]
fn kill_process_group(process_group_id: u32) -> std::io::Result<()> {
    let Ok(process_group_id) = i32::try_from(process_group_id) else {
        return Err(std::io::Error::other("version command pid exceeds i32"));
    };
    let result = unsafe { libc::kill(-process_group_id, libc::SIGKILL) };
    if result == 0 {
        return Ok(());
    }

    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

fn unavailable_result(tool: LocalTool, reason: String) -> ToolCheckResult {
    ToolCheckResult {
        tool,
        display_name: tool.display_name(),
        status: ToolCheckStatus::VersionUnavailable { reason },
    }
}

fn combined_tool_output(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    match (stdout.trim(), stderr.trim()) {
        ("", "") => String::new(),
        ("", stderr) => stderr.to_string(),
        (stdout, "") => stdout.to_string(),
        (stdout, stderr) => format!("{stdout}\n{stderr}"),
    }
}

fn command_failure_summary(status: std::process::ExitStatus, output: &str) -> String {
    let detail = summarize_tool_output(output);
    if detail == "no output" {
        format!("version command exited with {status}")
    } else {
        format!("version command exited with {status}: {detail}")
    }
}

fn format_duration(duration: Duration) -> String {
    if duration.subsec_nanos() == 0 {
        format!("{}s", duration.as_secs())
    } else {
        format!("{}ms", duration.as_millis())
    }
}

fn summarize_tool_output(output: &str) -> String {
    let output = output.trim();
    if output.is_empty() {
        return "no output".to_string();
    }
    truncate_chars(output, 48)
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, c) in s.chars().enumerate() {
        if idx >= max_chars {
            out.push('…');
            break;
        }
        out.push(c);
    }
    out
}

pub(crate) fn parse_version(output: &str) -> Option<String> {
    let output = output.trim();
    if output.is_empty() {
        return None;
    }

    static VERSION_RE: OnceLock<Regex> = OnceLock::new();
    let re = VERSION_RE.get_or_init(|| {
        Regex::new(r"(?i)\bv?(\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?)")
            .expect("VERSION_RE must compile")
    });

    let caps = re.captures(output)?;
    Some(caps.get(1)?.as_str().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::time::Instant as StdInstant;

    #[test]
    fn local_tool_specs_include_all_supported_clis() {
        let display_names = LocalTool::all()
            .iter()
            .map(|tool| tool.display_name())
            .collect::<Vec<_>>();

        assert_eq!(
            display_names,
            vec!["Claude", "Codex", "Gemini", "OpenCode", "Hermes", "OpenClaw"]
        );
        assert_eq!(LocalTool::Hermes.binary_name(), "hermes");
        assert_eq!(LocalTool::OpenClaw.binary_name(), "openclaw");
        assert_eq!(LocalTool::Hermes.version_timeout(), Duration::from_secs(10));
        assert_eq!(LocalTool::Claude.version_timeout(), Duration::from_secs(5));
    }

    #[test]
    fn parse_version_extracts_semver() {
        assert_eq!(parse_version("claude 2.1.12\n").as_deref(), Some("2.1.12"));
        assert_eq!(parse_version("0.95.0").as_deref(), Some("0.95.0"));
    }

    #[test]
    fn parse_version_supports_prerelease() {
        assert_eq!(
            parse_version("gemini version: 1.2.3-beta.1").as_deref(),
            Some("1.2.3-beta.1")
        );
    }

    #[test]
    fn parse_version_returns_none_for_garbage() {
        assert_eq!(parse_version("nonsense").as_deref(), None);
    }

    #[tokio::test]
    async fn version_checks_are_bounded_and_publish_fast_results_first() {
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let mut completed = Vec::new();

        check_local_environment_progressive_with(
            LocalEnvCancellation::default(),
            |result| completed.push(result.tool),
            |tool, _| {
                let active = active.clone();
                let maximum = maximum.clone();
                async move {
                    let running = active.fetch_add(1, AtomicOrdering::SeqCst) + 1;
                    maximum.fetch_max(running, AtomicOrdering::SeqCst);
                    let delay = if matches!(tool, LocalTool::Claude) {
                        Duration::from_millis(80)
                    } else {
                        Duration::from_millis(5)
                    };
                    tokio::time::sleep(delay).await;
                    active.fetch_sub(1, AtomicOrdering::SeqCst);
                    Some(ToolCheckResult {
                        tool,
                        display_name: tool.display_name(),
                        status: ToolCheckStatus::Ok {
                            version: "1.2.3".to_string(),
                        },
                    })
                }
            },
        )
        .await;

        assert_eq!(maximum.load(AtomicOrdering::SeqCst), 2);
        assert_eq!(completed.len(), LocalTool::all().len());
        assert_eq!(completed.first(), Some(&LocalTool::Codex));
        assert!(
            completed.iter().position(|tool| *tool == LocalTool::Codex)
                < completed.iter().position(|tool| *tool == LocalTool::Claude)
        );
    }

    #[cfg(unix)]
    fn fake_tool(script: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let tool_path = temp_dir.path().join("fake-tool");
        let mut file = std::fs::File::create(&tool_path).expect("create fake tool");
        file.write_all(script.as_bytes()).expect("write fake tool");
        let mut permissions = file
            .metadata()
            .expect("read fake tool metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&tool_path, permissions).expect("make fake tool executable");
        (temp_dir, tool_path)
    }

    #[cfg(unix)]
    #[test]
    fn installed_check_does_not_run_tool() {
        let (temp_dir, tool_path) = fake_tool("#!/bin/sh\nprintf ran > \"$0.executed\"\n");
        let marker_path = temp_dir.path().join("fake-tool.executed");

        assert!(is_tool_installed(
            tool_path.to_str().expect("fake tool path should be utf-8")
        ));
        assert!(
            !marker_path.exists(),
            "visibility detection must not execute version commands"
        );
    }

    const VERSION_SLEEP_HELPER_FILTER: &str = "spawned_version_probe_sleep_helper";

    #[test]
    fn cc_switch_spawned_version_probe_sleep_helper_test() {
        if std::env::args().any(|arg| arg == VERSION_SLEEP_HELPER_FILTER) {
            std::thread::sleep(Duration::from_secs(30));
        }
    }

    #[tokio::test]
    async fn version_command_times_out() {
        let started = StdInstant::now();
        let timeout = Duration::from_millis(100);
        let test_executable = std::env::current_exe().expect("resolve current test executable");
        let err = run_tool_version_command(
            &test_executable,
            VERSION_SLEEP_HELPER_FILTER,
            Instant::now() + timeout,
            LocalEnvCancellation::default(),
        )
        .await
        .expect_err("sleeping command should time out");

        assert_eq!(err, VersionCommandError::TimedOut);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "timeout should bound version detection latency"
        );
    }

    #[cfg(windows)]
    const VERSION_DESCENDANT_HELPER_FILTER: &str = "spawned_version_probe_descendant_helper";

    #[cfg(windows)]
    const VERSION_DESCENDANT_PID_ENV: &str = "CC_SWITCH_TEST_VERSION_DESCENDANT_PID";

    #[cfg(windows)]
    #[test]
    fn cc_switch_spawned_version_probe_descendant_helper_test() {
        if !std::env::args().any(|arg| arg == VERSION_DESCENDANT_HELPER_FILTER) {
            return;
        }

        let pid_path = std::env::var_os(VERSION_DESCENDANT_PID_ENV)
            .expect("descendant helper requires a pid path");
        std::fs::write(pid_path, std::process::id().to_string())
            .expect("record descendant helper pid");
        std::thread::sleep(Duration::from_secs(30));
    }

    #[cfg(windows)]
    struct WindowsProcessWaitHandle(std::os::windows::io::OwnedHandle);

    #[cfg(windows)]
    impl WindowsProcessWaitHandle {
        fn open(pid: u32) -> Self {
            use std::os::windows::io::FromRawHandle;
            use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};

            let raw_handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
            assert!(!raw_handle.is_null(), "open descendant process {pid}");
            Self(unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(raw_handle) })
        }

        fn exited(&self) -> bool {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
            use windows_sys::Win32::System::Threading::WaitForSingleObject;

            match unsafe { WaitForSingleObject(self.0.as_raw_handle().cast(), 0) } {
                WAIT_OBJECT_0 => true,
                WAIT_TIMEOUT => false,
                result => panic!("failed to query descendant process state: {result}"),
            }
        }
    }

    #[cfg(windows)]
    fn create_windows_descendant_shim(
    ) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let pid_path = temp_dir.path().join("descendant.pid");
        let shim_path = temp_dir.path().join("version-shim.cmd");
        let test_executable = std::env::current_exe().expect("resolve current test executable");
        let escape_batch_value = |value: &Path| value.display().to_string().replace('%', "%%");
        let script = format!(
            "@echo off\r\nset \"{VERSION_DESCENDANT_PID_ENV}={}\"\r\n\"{}\" {VERSION_DESCENDANT_HELPER_FILTER}\r\n",
            escape_batch_value(&pid_path),
            escape_batch_value(&test_executable),
        );
        std::fs::write(&shim_path, script).expect("write Windows version shim");
        (temp_dir, pid_path, shim_path)
    }

    #[cfg(windows)]
    async fn wait_for_windows_descendant(
        pid_path: &Path,
        task: &tokio::task::JoinHandle<Result<Output, VersionCommandError>>,
    ) -> (u32, WindowsProcessWaitHandle) {
        let start_deadline = StdInstant::now() + Duration::from_secs(5);
        while !pid_path.exists() && StdInstant::now() < start_deadline {
            assert!(
                !task.is_finished(),
                "version command stopped before descendant started"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let pid = std::fs::read_to_string(pid_path)
            .expect("Windows shim descendant should record its pid")
            .parse::<u32>()
            .expect("descendant pid should be numeric");
        let wait_handle = WindowsProcessWaitHandle::open(pid);
        (pid, wait_handle)
    }

    #[cfg(windows)]
    async fn assert_windows_process_exits(pid: u32, process: &WindowsProcessWaitHandle) {
        let exit_deadline = StdInstant::now() + Duration::from_secs(1);
        while !process.exited() && StdInstant::now() < exit_deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            process.exited(),
            "Windows shim descendant {pid} survived process-tree cleanup"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_version_shim_is_suspended_until_job_attach() {
        use std::process::Stdio;
        use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;

        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let marker_path = temp_dir.path().join("version-shim-ran");
        let shim_path = temp_dir.path().join("suspended-version-shim.cmd");
        let script = format!(
            "@echo off\r\n> \"{}\" echo ran\r\n",
            marker_path.display().to_string().replace('%', "%%")
        );
        std::fs::write(&shim_path, script).expect("write suspended Windows shim");

        let mut command = Command::new(&shim_path);
        command
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .creation_flags(CREATE_SUSPENDED);
        let mut process_guard = VersionProcessGuard::prepare().expect("create Windows job");
        let mut child = command.spawn().expect("spawn suspended Windows shim");

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !marker_path.exists(),
            "suspended shim ran before Job attach"
        );
        process_guard
            .attach(&child)
            .expect("attach shim to Windows job");
        assert!(!marker_path.exists(), "attaching the Job resumed the shim");
        process_guard
            .start(&child)
            .expect("resume attached Windows shim");

        let status = timeout(Duration::from_secs(2), child.wait())
            .await
            .expect("resumed Windows shim should exit")
            .expect("wait for resumed Windows shim");
        assert!(status.success());
        assert!(marker_path.exists(), "resumed Windows shim did not run");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn cancellation_terminates_windows_shim_descendant_tree() {
        let (_temp_dir, pid_path, shim_path) = create_windows_descendant_shim();

        let cancellation = LocalEnvCancellation::default();
        let task_cancellation = cancellation.clone();
        let task = tokio::spawn(async move {
            run_tool_version_command(
                &shim_path,
                "--version",
                Instant::now() + Duration::from_secs(10),
                task_cancellation,
            )
            .await
        });
        let (pid, process) = wait_for_windows_descendant(&pid_path, &task).await;

        cancellation.cancel();
        let result = timeout(Duration::from_secs(2), task)
            .await
            .expect("cancelled command should stop promptly")
            .expect("version command task should not panic");
        assert_eq!(
            result.expect_err("command should report cancellation"),
            VersionCommandError::Cancelled
        );
        assert_windows_process_exits(pid, &process).await;
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn aborting_probe_terminates_windows_shim_descendant_tree() {
        let (_temp_dir, pid_path, shim_path) = create_windows_descendant_shim();
        let task = tokio::spawn(async move {
            run_tool_version_command(
                &shim_path,
                "--version",
                Instant::now() + Duration::from_secs(10),
                LocalEnvCancellation::default(),
            )
            .await
        });
        let (pid, process) = wait_for_windows_descendant(&pid_path, &task).await;

        task.abort();
        let error = task
            .await
            .expect_err("aborted probe task should be cancelled");
        assert!(error.is_cancelled());
        assert_windows_process_exits(pid, &process).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_terminates_an_active_version_command() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let pid_path = temp_dir.path().join("command.pid");
        let script = format!(
            "#!/bin/sh\nprintf '%s' \"$$\" > '{}'\nsleep 30\n",
            pid_path.display()
        );
        let (_tool_dir, tool_path) = fake_tool(&script);
        let cancellation = LocalEnvCancellation::default();
        let task_cancellation = cancellation.clone();
        let task = tokio::spawn(async move {
            run_tool_version_command(
                &tool_path,
                "--version",
                Instant::now() + Duration::from_secs(5),
                task_cancellation,
            )
            .await
        });

        let start_deadline = StdInstant::now() + Duration::from_secs(1);
        while !pid_path.exists() && StdInstant::now() < start_deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let pid = std::fs::read_to_string(&pid_path)
            .expect("command pid should be recorded before cancellation")
            .parse::<i32>()
            .expect("command pid should be numeric");

        cancellation.cancel();
        let result = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("cancelled command should stop promptly")
            .expect("version command task should not panic");
        assert_eq!(
            result.expect_err("command should report cancellation"),
            VersionCommandError::Cancelled
        );

        let alive = unsafe { libc::kill(pid, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
        assert!(!alive, "cancelled version command {pid} is still running");
    }

    #[cfg(unix)]
    async fn wait_for_unix_descendant(pid_path: &Path) -> i32 {
        let start_deadline = StdInstant::now() + Duration::from_secs(3);
        while !pid_path.exists() && StdInstant::now() < start_deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        std::fs::read_to_string(pid_path)
            .expect("descendant pid should be recorded")
            .parse::<i32>()
            .expect("descendant pid should be numeric")
    }

    #[cfg(unix)]
    async fn assert_unix_process_exits(pid: i32) {
        let exit_deadline = StdInstant::now() + Duration::from_secs(1);
        while StdInstant::now() < exit_deadline {
            let alive = unsafe { libc::kill(pid, 0) } == 0
                || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
            if !alive {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("descendant process {pid} survived process-group cleanup");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn aborting_probe_terminates_unix_descendant_process_group() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let pid_path = temp_dir.path().join("descendant.pid");
        let script = format!(
            "#!/bin/sh\nsleep 30 &\nchild=$!\nprintf '%s' \"$child\" > '{}'\nwait \"$child\"\n",
            pid_path.display()
        );
        let (_tool_dir, tool_path) = fake_tool(&script);
        let task = tokio::spawn(async move {
            run_tool_version_command(
                &tool_path,
                "--version",
                Instant::now() + Duration::from_secs(30),
                LocalEnvCancellation::default(),
            )
            .await
        });
        let pid = wait_for_unix_descendant(&pid_path).await;

        task.abort();
        let error = task
            .await
            .expect_err("aborted probe task should be cancelled");
        assert!(error.is_cancelled());
        assert_unix_process_exits(pid).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn successful_probe_terminates_detached_unix_descendant() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let pid_path = temp_dir.path().join("descendant.pid");
        let script = format!(
            "#!/bin/sh\ntrap '' HUP\nsleep 30 </dev/null >/dev/null 2>&1 &\nchild=$!\nprintf '%s' \"$child\" > '{}'\nprintf '1.2.3\\n'\n",
            pid_path.display()
        );
        let (_tool_dir, tool_path) = fake_tool(&script);

        let output = run_tool_version_command(
            &tool_path,
            "--version",
            Instant::now() + Duration::from_secs(2),
            LocalEnvCancellation::default(),
        )
        .await
        .expect("version command should complete successfully");
        assert!(output.status.success());
        assert_eq!(
            parse_version(&combined_tool_output(&output)).as_deref(),
            Some("1.2.3")
        );

        let pid = wait_for_unix_descendant(&pid_path).await;
        assert_unix_process_exits(pid).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_does_not_try_fallback_command() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let marker_path = temp_dir.path().join("fallback-ran");
        let script = format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then sleep 5; fi\nprintf ran > '{}'\nprintf '1.2.3\\n'\n",
            marker_path.display()
        );
        let (_tool_dir, tool_path) = fake_tool(&script);

        let status = check_tool_version_at_path(
            &tool_path,
            &["--version", "version"],
            Duration::from_millis(120),
            LocalEnvCancellation::default(),
        )
        .await
        .expect("probe should not be cancelled");

        assert!(matches!(status, ToolCheckStatus::VersionUnavailable { .. }));
        assert!(
            !marker_path.exists(),
            "fallback must not run after a timeout"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fallback_attempts_share_one_deadline() {
        let script = "#!/bin/sh\nsleep 0.08\nif [ \"$1\" = \"--version\" ]; then exit 2; fi\nprintf '1.2.3\\n'\n";
        let (_tool_dir, tool_path) = fake_tool(script);
        let started = StdInstant::now();

        let status = check_tool_version_at_path(
            &tool_path,
            &["--version", "version"],
            Duration::from_millis(120),
            LocalEnvCancellation::default(),
        )
        .await
        .expect("probe should not be cancelled");

        assert!(matches!(status, ToolCheckStatus::VersionUnavailable { .. }));
        assert!(started.elapsed() < Duration::from_millis(220));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn large_stdout_and_stderr_are_drained_without_unbounded_capture() {
        let script = "#!/bin/sh\nprintf '1.2.3\\n'\ni=0\nwhile [ $i -lt 5000 ]; do printf 'stdout-padding-%08d\\n' \"$i\"; printf 'stderr-padding-%08d\\n' \"$i\" >&2; i=$((i + 1)); done\n";
        let (_tool_dir, tool_path) = fake_tool(script);

        let output = run_tool_version_command(
            &tool_path,
            "--version",
            Instant::now() + Duration::from_secs(5),
            LocalEnvCancellation::default(),
        )
        .await
        .expect("large output command should complete");

        assert!(output.stdout.len() <= MAX_CAPTURED_OUTPUT_BYTES);
        assert!(output.stderr.len() <= MAX_CAPTURED_OUTPUT_BYTES);
        assert_eq!(
            parse_version(&combined_tool_output(&output)).as_deref(),
            Some("1.2.3")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_terminates_descendant_process_group() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let pid_path = temp_dir.path().join("descendant.pid");
        let script = format!(
            "#!/bin/sh\nsleep 30 &\nchild=$!\nprintf '%s' \"$child\" > '{}'\nwait \"$child\"\n",
            pid_path.display()
        );
        let (_tool_dir, tool_path) = fake_tool(&script);

        let task = tokio::spawn(async move {
            run_tool_version_command(
                &tool_path,
                "--version",
                Instant::now() + Duration::from_secs(4),
                LocalEnvCancellation::default(),
            )
            .await
        });
        let pid = wait_for_unix_descendant(&pid_path).await;
        let result = task.await.expect("version command task should not panic");
        assert_eq!(
            result.expect_err("command should time out"),
            VersionCommandError::TimedOut
        );

        assert_unix_process_exits(pid).await;
    }
}
