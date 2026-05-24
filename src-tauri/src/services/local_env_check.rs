use crate::app_config::AppType;
use regex::Regex;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const TOOL_VERSION_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone)]
pub enum ToolCheckStatus {
    Ok { version: String },
    NotInstalledOrNotExecutable,
    Error { message: String },
}

#[derive(Debug, Clone)]
pub struct ToolCheckResult {
    pub tool: LocalTool,
    pub display_name: &'static str,
    pub status: ToolCheckStatus,
}

pub fn check_local_environment() -> Vec<ToolCheckResult> {
    LocalTool::all()
        .iter()
        .map(|tool| ToolCheckResult {
            tool: *tool,
            display_name: tool.display_name(),
            status: check_tool_version(tool.binary_name(), tool.version_args()),
        })
        .collect()
}

pub fn check_tool_installed(app_type: &AppType) -> bool {
    let tool = LocalTool::from_app_type(app_type);
    matches!(
        check_tool_version(tool.binary_name(), tool.version_args()),
        ToolCheckStatus::Ok { .. }
    )
}

fn check_tool_version(bin: &str, version_args: &[&str]) -> ToolCheckStatus {
    if which::which(bin).is_err() {
        return ToolCheckStatus::NotInstalledOrNotExecutable;
    }

    let mut last_error = None::<String>;
    for arg in version_args {
        match run_tool_version_command(bin, arg) {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let combined = if stdout.trim().is_empty() {
                    stderr.trim()
                } else {
                    stdout.trim()
                };

                if !output.status.success() {
                    last_error = Some(summarize_tool_output(combined));
                    continue;
                }

                if let Some(version) = parse_version(combined)
                    .or_else(|| nonempty_trimmed(combined).map(|s| truncate_chars(s, 32)))
                {
                    return ToolCheckStatus::Ok { version };
                }

                last_error = Some(summarize_tool_output(combined));
            }
            Err(err) => {
                last_error = Some(err.to_string());
            }
        }
    }

    ToolCheckStatus::Error {
        message: last_error.unwrap_or_else(|| "unable to detect version".to_string()),
    }
}

fn run_tool_version_command(bin: &str, arg: &str) -> Result<std::process::Output, String> {
    let mut child = Command::new(bin)
        .arg(arg)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| err.to_string())?;

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child.wait_with_output().map_err(|err| err.to_string());
            }
            Ok(None) => {
                if started.elapsed() >= TOOL_VERSION_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "version check timed out after {TOOL_VERSION_TIMEOUT:?}"
                    ));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(err.to_string());
            }
        }
    }
}

fn summarize_tool_output(output: &str) -> String {
    let output = output.trim();
    if output.is_empty() {
        return "no output".to_string();
    }
    truncate_chars(output, 48)
}

fn nonempty_trimmed(s: &str) -> Option<&str> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
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
    use super::{parse_version, run_tool_version_command, LocalTool, TOOL_VERSION_TIMEOUT};
    use std::time::Instant;

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

    #[test]
    fn version_command_times_out() {
        let started = Instant::now();
        let err =
            run_tool_version_command("sleep", "5").expect_err("sleeping command should time out");

        assert!(err.contains("timed out"), "{err}");
        assert!(
            started.elapsed() < TOOL_VERSION_TIMEOUT + TOOL_VERSION_TIMEOUT,
            "timeout should bound version detection latency"
        );
    }
}
