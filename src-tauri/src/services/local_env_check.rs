use regex::Regex;
use std::process::Command;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalTool {
    Claude,
    Codex,
    Gemini,
    OpenCode,
    OpenClaw,
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
    const SPECS: &[(LocalTool, &str, &str, &[&str])] = &[
        (
            LocalTool::Claude,
            "claude",
            "Claude",
            &["--version", "version"],
        ),
        (LocalTool::Codex, "codex", "Codex", &["--version"]),
        (LocalTool::Gemini, "gemini", "Gemini", &["--version", "-v"]),
        (
            LocalTool::OpenCode,
            "opencode",
            "OpenCode",
            &["--version", "version"],
        ),
        (
            LocalTool::OpenClaw,
            "openclaw",
            "OpenClaw",
            &["--version", "version"],
        ),
    ];

    SPECS
        .iter()
        .map(|(tool, bin, display_name, args)| ToolCheckResult {
            tool: *tool,
            display_name,
            status: check_tool_version(bin, args),
        })
        .collect()
}

fn check_tool_version(bin: &str, version_args: &[&str]) -> ToolCheckStatus {
    if which::which(bin).is_err() {
        return ToolCheckStatus::NotInstalledOrNotExecutable;
    }

    let mut last_error = None::<String>;
    for arg in version_args {
        match Command::new(bin).arg(arg).output() {
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
    use super::parse_version;

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
}
