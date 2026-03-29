use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::error::AppError;
use crate::provider::Provider;
use serde_json::json;

use super::super::app::ToastKind;
use super::super::terminal::TuiTerminal;
use super::RuntimeActionContext;

#[derive(Debug)]
struct PreparedClaudeLaunch {
    executable: PathBuf,
    settings_path: PathBuf,
}

impl PreparedClaudeLaunch {
    fn cleanup_settings_file(&self) -> Result<(), AppError> {
        cleanup_temp_settings_file(&self.settings_path)
    }
}

pub(super) fn launch(ctx: &mut RuntimeActionContext<'_>, id: String) -> Result<(), AppError> {
    launch_with(
        ctx,
        id,
        &std::env::temp_dir(),
        resolve_claude_binary,
        handoff_to_claude,
    )
}

fn launch_with<Resolve, Handoff>(
    ctx: &mut RuntimeActionContext<'_>,
    id: String,
    temp_dir: &Path,
    resolve_claude_binary: Resolve,
    handoff: Handoff,
) -> Result<(), AppError>
where
    Resolve: FnOnce() -> Result<PathBuf, AppError>,
    Handoff: FnOnce(&mut TuiTerminal, &PreparedClaudeLaunch) -> Result<(), AppError>,
{
    if !matches!(ctx.app.app_type, AppType::Claude) {
        return Ok(());
    }

    if let Err(err) = try_launch_with(ctx, &id, temp_dir, resolve_claude_binary, handoff) {
        ctx.app.push_toast(
            texts::tui_temp_launch_failed(&err.to_string()),
            ToastKind::Error,
        );
    }
    Ok(())
}

fn prepare_launch_with<Resolve>(
    provider: &Provider,
    temp_dir: &Path,
    resolve_claude_binary: Resolve,
) -> Result<PreparedClaudeLaunch, AppError>
where
    Resolve: FnOnce() -> Result<PathBuf, AppError>,
{
    let executable = resolve_claude_binary()?;
    let env = provider
        .settings_config
        .get("env")
        .and_then(|value| value.as_object())
        .ok_or_else(|| {
            AppError::localized(
                "claude.temp_launch_missing_env",
                format!("供应商 {} 缺少有效的 env 配置。", provider.id),
                format!("Provider {} is missing a valid env object.", provider.id),
            )
        })?;
    let settings_path = write_temp_settings_file(temp_dir, provider, &json!({ "env": env }))?;

    Ok(PreparedClaudeLaunch {
        executable,
        settings_path,
    })
}

fn resolve_claude_binary() -> Result<PathBuf, AppError> {
    which::which("claude").map_err(|_| {
        AppError::localized(
            "claude.temp_launch_missing_binary",
            "未找到 claude 命令，请先安装 Claude CLI。".to_string(),
            "Could not find `claude` in PATH. Install Claude CLI first.".to_string(),
        )
    })
}

#[cfg(unix)]
fn handoff_to_claude(
    terminal: &mut TuiTerminal,
    prepared: &PreparedClaudeLaunch,
) -> Result<(), AppError> {
    use std::os::unix::process::CommandExt;

    terminal.with_terminal_restored_for_handoff(|| {
        let exec_err = build_handoff_command(prepared).exec();

        Err(AppError::localized(
            "claude.temp_launch_exec_failed",
            format!("启动 Claude 失败: {exec_err}"),
            format!("Failed to launch Claude: {exec_err}"),
        ))
    })
}

#[cfg(unix)]
fn build_handoff_command(prepared: &PreparedClaudeLaunch) -> std::process::Command {
    let mut command = std::process::Command::new(&prepared.executable);
    command.arg("--settings").arg(&prepared.settings_path);
    command
}

#[cfg(not(unix))]
fn handoff_to_claude(
    _terminal: &mut TuiTerminal,
    _prepared: &PreparedClaudeLaunch,
) -> Result<(), AppError> {
    Err(AppError::localized(
        "claude.temp_launch_unsupported_platform",
        "当前平台暂不支持在当前终端临时启动 Claude。".to_string(),
        "Temporary Claude launch in the current terminal is not supported on this platform."
            .to_string(),
    ))
}

fn try_launch_with<Resolve, Handoff>(
    ctx: &mut RuntimeActionContext<'_>,
    id: &str,
    temp_dir: &Path,
    resolve_claude_binary: Resolve,
    handoff: Handoff,
) -> Result<(), AppError>
where
    Resolve: FnOnce() -> Result<PathBuf, AppError>,
    Handoff: FnOnce(&mut TuiTerminal, &PreparedClaudeLaunch) -> Result<(), AppError>,
{
    try_launch_with_platform_check(
        ctx,
        id,
        temp_dir,
        ensure_temp_launch_supported,
        resolve_claude_binary,
        handoff,
    )
}

fn try_launch_with_platform_check<Support, Resolve, Handoff>(
    ctx: &mut RuntimeActionContext<'_>,
    id: &str,
    temp_dir: &Path,
    ensure_supported: Support,
    resolve_claude_binary: Resolve,
    handoff: Handoff,
) -> Result<(), AppError>
where
    Support: FnOnce() -> Result<(), AppError>,
    Resolve: FnOnce() -> Result<PathBuf, AppError>,
    Handoff: FnOnce(&mut TuiTerminal, &PreparedClaudeLaunch) -> Result<(), AppError>,
{
    ensure_supported()?;

    let provider = ctx
        .data
        .providers
        .rows
        .iter()
        .find(|row| row.id == id)
        .map(|row| row.provider.clone())
        .ok_or_else(|| {
            AppError::localized(
                "claude.temp_launch_provider_not_found",
                format!("未找到选中的供应商: {id}"),
                format!("Selected provider not found: {id}"),
            )
        })?;
    let prepared = prepare_launch_with(&provider, temp_dir, resolve_claude_binary)?;
    let handoff_result = handoff(ctx.terminal, &prepared);
    let cleanup_result = prepared.cleanup_settings_file();

    match (handoff_result, cleanup_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(cleanup_err)) => Err(cleanup_err),
        (Err(err), Ok(())) => Err(err),
        (Err(err), Err(cleanup_err)) => Err(AppError::localized(
            "claude.temp_launch_cleanup_failed",
            format!("启动 Claude 失败: {err}；同时清理临时设置文件失败: {cleanup_err}"),
            format!("Failed to launch Claude: {err}; also failed to remove the temporary settings file: {cleanup_err}"),
        )),
    }
}

#[cfg(unix)]
fn ensure_temp_launch_supported() -> Result<(), AppError> {
    Ok(())
}

#[cfg(not(unix))]
fn ensure_temp_launch_supported() -> Result<(), AppError> {
    Err(AppError::localized(
        "claude.temp_launch_unsupported_platform",
        "当前平台暂不支持在当前终端临时启动 Claude。".to_string(),
        "Temporary Claude launch in the current terminal is not supported on this platform."
            .to_string(),
    ))
}

fn write_temp_settings_file(
    temp_dir: &Path,
    provider: &Provider,
    settings: &serde_json::Value,
) -> Result<PathBuf, AppError> {
    write_temp_settings_file_with(temp_dir, provider, settings, finalize_temp_settings_file)
}

fn write_temp_settings_file_with<Finalize>(
    temp_dir: &Path,
    provider: &Provider,
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
        sanitize_filename_fragment(&provider.id),
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
        Err(err) => {
            let cleanup_result = cleanup_temp_settings_file(&path);
            match cleanup_result {
                Ok(()) => Err(err),
                Err(cleanup_err) => Err(AppError::localized(
                    "claude.temp_launch_tempfile_cleanup_failed",
                    format!(
                        "写入临时设置文件失败: {err}；同时清理失败: {cleanup_err}"
                    ),
                    format!(
                        "Failed to write the temporary settings file: {err}; also failed to clean it up: {cleanup_err}"
                    ),
                )),
            }
        }
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
    use crate::cli::tui::app::{App, ToastKind};
    use crate::cli::tui::data::{ProviderRow, ProvidersSnapshot, UiData};
    use crate::cli::tui::runtime_systems::RequestTracker;
    use crate::provider::Provider;
    use serde_json::{json, Value};
    use std::cell::Cell;
    #[cfg(unix)]
    use std::ffi::OsString;
    use tempfile::TempDir;

    struct LaunchFixture {
        terminal: TuiTerminal,
        app: App,
        data: UiData,
        proxy_loading: RequestTracker,
        webdav_loading: RequestTracker,
        update_check: RequestTracker,
    }

    impl LaunchFixture {
        fn new(app_type: AppType, current_id: &str, rows: Vec<ProviderRow>) -> Self {
            Self {
                terminal: TuiTerminal::new_for_test().expect("create test terminal"),
                app: App::new(Some(app_type)),
                data: UiData {
                    providers: ProvidersSnapshot {
                        current_id: current_id.to_string(),
                        rows,
                    },
                    ..UiData::default()
                },
                proxy_loading: RequestTracker::default(),
                webdav_loading: RequestTracker::default(),
                update_check: RequestTracker::default(),
            }
        }

        fn ctx(&mut self) -> RuntimeActionContext<'_> {
            RuntimeActionContext {
                terminal: &mut self.terminal,
                app: &mut self.app,
                data: &mut self.data,
                speedtest_req_tx: None,
                stream_check_req_tx: None,
                skills_req_tx: None,
                proxy_req_tx: None,
                proxy_loading: &mut self.proxy_loading,
                local_env_req_tx: None,
                webdav_req_tx: None,
                webdav_loading: &mut self.webdav_loading,
                update_req_tx: None,
                update_check: &mut self.update_check,
                model_fetch_req_tx: None,
            }
        }
    }

    fn provider_row(id: &str, env: Value) -> ProviderRow {
        ProviderRow {
            id: id.to_string(),
            provider: Provider::with_id(
                id.to_string(),
                format!("Provider {id}"),
                json!({ "env": env }),
                None,
            ),
            api_url: Some("https://api.example.com".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: None,
            default_model_id: None,
        }
    }

    #[cfg(unix)]
    #[test]
    fn unix_handoff_command_execs_claude_directly() {
        let prepared = PreparedClaudeLaunch {
            executable: PathBuf::from("/usr/local/bin/claude"),
            settings_path: PathBuf::from("/tmp/cc-switch-claude-settings.json"),
        };

        let command = build_handoff_command(&prepared);
        let args: Vec<OsString> = command.get_args().map(|arg| arg.to_os_string()).collect();

        assert_eq!(
            command.get_program(),
            std::path::Path::new("/usr/local/bin/claude")
        );
        assert_eq!(
            args,
            vec![
                OsString::from("--settings"),
                OsString::from("/tmp/cc-switch-claude-settings.json"),
            ]
        );
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
            &provider,
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
            assert_eq!(
                mode, 0o600,
                "temp settings file should use owner-only permissions"
            );
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

        assert!(
            err.to_string().contains("claude"),
            "error should mention claude: {err}"
        );
    }

    #[test]
    fn launch_failure_does_not_switch_current_provider_and_surfaces_a_toast() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let mut fixture = LaunchFixture::new(
            AppType::Claude,
            "current",
            vec![
                provider_row(
                    "current",
                    json!({
                        "ANTHROPIC_AUTH_TOKEN": "sk-current"
                    }),
                ),
                provider_row(
                    "candidate",
                    json!({
                        "ANTHROPIC_AUTH_TOKEN": "sk-candidate"
                    }),
                ),
            ],
        );
        let captured_settings_path = std::cell::RefCell::new(None::<PathBuf>);

        launch_with(
            &mut fixture.ctx(),
            "candidate".to_string(),
            temp_dir.path(),
            || Ok(PathBuf::from("/usr/bin/claude")),
            |_, prepared| {
                captured_settings_path.replace(Some(prepared.settings_path.clone()));
                Err(AppError::Message("launch exploded".to_string()))
            },
        )
        .expect("launch failure should stay in the TUI");

        assert_eq!(fixture.data.providers.current_id, "current");
        assert!(
            matches!(
                fixture.app.toast.as_ref(),
                Some(toast)
                    if toast.kind == ToastKind::Error
                        && toast.message == texts::tui_temp_launch_failed("launch exploded")
            ),
            "expected temp launch failure toast, got {:?}",
            fixture.app.toast
        );
        let settings_path = captured_settings_path
            .into_inner()
            .expect("handoff should observe the temp settings path");
        assert!(
            !settings_path.exists(),
            "temp settings file should be removed after a failed launch"
        );
    }

    #[test]
    fn non_claude_runtime_launch_is_ignored_before_handoff() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let mut fixture = LaunchFixture::new(
            AppType::Codex,
            "current",
            vec![provider_row(
                "candidate",
                json!({
                    "ANTHROPIC_AUTH_TOKEN": "sk-candidate"
                }),
            )],
        );
        let handoff_called = Cell::new(false);

        launch_with(
            &mut fixture.ctx(),
            "candidate".to_string(),
            temp_dir.path(),
            || Ok(PathBuf::from("/usr/bin/claude")),
            |_, _| {
                handoff_called.set(true);
                Ok(())
            },
        )
        .expect("non-Claude runtime dispatch should be ignored");

        assert!(
            !handoff_called.get(),
            "non-Claude apps should not attempt the Claude temporary launch handoff"
        );
        assert!(
            fixture.app.toast.is_none(),
            "ignored non-Claude dispatch should stay silent"
        );
    }

    #[test]
    fn unsupported_platform_fails_before_writing_temp_settings_file() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let mut fixture = LaunchFixture::new(
            AppType::Claude,
            "current",
            vec![provider_row(
                "candidate",
                json!({
                    "ANTHROPIC_AUTH_TOKEN": "sk-candidate"
                }),
            )],
        );
        let resolve_called = Cell::new(false);

        let err = try_launch_with_platform_check(
            &mut fixture.ctx(),
            "candidate",
            temp_dir.path(),
            || {
                Err(AppError::localized(
                    "claude.temp_launch_unsupported_platform",
                    "当前平台暂不支持在当前终端临时启动 Claude。".to_string(),
                    "Temporary Claude launch in the current terminal is not supported on this platform."
                        .to_string(),
                ))
            },
            || {
                resolve_called.set(true);
                Ok(PathBuf::from("/usr/bin/claude"))
            },
            |_, _| Ok(()),
        )
        .expect_err("unsupported platforms should fail before preparing temp settings");

        assert!(
            err.to_string().contains("not supported"),
            "unexpected error: {err}"
        );
        assert!(
            !resolve_called.get(),
            "unsupported platforms should fail before resolving the Claude binary"
        );
        assert!(
            std::fs::read_dir(temp_dir.path())
                .expect("read temp dir")
                .next()
                .is_none(),
            "unsupported platforms should not create a temp settings file"
        );
    }
}
