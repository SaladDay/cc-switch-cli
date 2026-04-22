use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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

#[cfg(not(unix))]
pub(crate) fn ensure_temp_launch_supported() -> Result<(), AppError> {
    Err(AppError::localized(
        "claude.temp_launch_unsupported_platform",
        "当前平台暂不支持在当前终端临时启动 Claude。".to_string(),
        "Temporary Claude launch in the current terminal is not supported on this platform."
            .to_string(),
    ))
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

#[cfg(not(unix))]
pub(crate) fn exec_prepared_claude(
    _prepared: &PreparedClaudeLaunch,
    _native_args: &[OsString],
) -> Result<(), AppError> {
    Err(AppError::localized(
        "claude.temp_launch_unsupported_platform",
        "当前平台暂不支持在当前终端临时启动 Claude。".to_string(),
        "Temporary Claude launch in the current terminal is not supported on this platform."
            .to_string(),
    ))
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
fn finalize_temp_settings_file(_path: &Path) -> Result<(), AppError> {
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
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|err| AppError::io(path, err))
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
