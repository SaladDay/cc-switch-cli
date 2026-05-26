use std::ffi::OsString;
use std::path::Path;

use crate::app_config::AppType;
use crate::cli::codex_temp_launch::{
    ensure_temp_launch_supported, exec_prepared_codex, prepare_launch, PreparedCodexLaunch,
};
use crate::cli::i18n::texts;
use crate::error::AppError;
use crate::provider::Provider;

use super::super::app::ToastKind;
use super::super::terminal::TuiTerminal;
use super::RuntimeActionContext;

pub(super) fn launch(ctx: &mut RuntimeActionContext<'_>, id: String) -> Result<(), AppError> {
    launch_with(
        ctx,
        id,
        &std::env::temp_dir(),
        ensure_temp_launch_supported,
        prepare_launch,
        handoff_to_codex,
    )
}

fn launch_with<Support, Prepare, Handoff>(
    ctx: &mut RuntimeActionContext<'_>,
    id: String,
    temp_dir: &Path,
    ensure_supported: Support,
    prepare: Prepare,
    handoff: Handoff,
) -> Result<(), AppError>
where
    Support: FnOnce() -> Result<(), AppError>,
    Prepare: FnOnce(&Provider, &Path) -> Result<PreparedCodexLaunch, AppError>,
    Handoff: FnOnce(&mut TuiTerminal, &PreparedCodexLaunch) -> Result<(), AppError>,
{
    if !matches!(ctx.app.app_type, AppType::Codex) {
        return Ok(());
    }

    if let Err(err) = try_launch_with(ctx, &id, temp_dir, ensure_supported, prepare, handoff) {
        ctx.app.push_toast(
            texts::tui_temp_launch_failed(&err.to_string()),
            ToastKind::Error,
        );
    }
    Ok(())
}

fn handoff_to_codex(
    terminal: &mut TuiTerminal,
    prepared: &PreparedCodexLaunch,
) -> Result<(), AppError> {
    let native_args = Vec::<OsString>::new();
    terminal.with_terminal_restored_for_handoff(|| exec_prepared_codex(prepared, &native_args))
}

fn try_launch_with<Support, Prepare, Handoff>(
    ctx: &mut RuntimeActionContext<'_>,
    id: &str,
    temp_dir: &Path,
    ensure_supported: Support,
    prepare: Prepare,
    handoff: Handoff,
) -> Result<(), AppError>
where
    Support: FnOnce() -> Result<(), AppError>,
    Prepare: FnOnce(&Provider, &Path) -> Result<PreparedCodexLaunch, AppError>,
    Handoff: FnOnce(&mut TuiTerminal, &PreparedCodexLaunch) -> Result<(), AppError>,
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
                "codex.temp_launch_provider_not_found",
                format!("未找到选中的供应商: {id}"),
                format!("Selected provider not found: {id}"),
            )
        })?;
    let prepared = prepare(&provider, temp_dir)?;
    let handoff_result = handoff(ctx.terminal, &prepared);
    let cleanup_result = prepared.cleanup_home_dir();

    match (handoff_result, cleanup_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(cleanup_err)) => Err(cleanup_err),
        (Err(err), Ok(())) => Err(err),
        (Err(err), Err(cleanup_err)) => Err(AppError::localized(
            "codex.temp_launch_cleanup_failed",
            format!("启动 Codex 失败: {err}；同时清理临时配置目录失败: {cleanup_err}"),
            format!(
                "Failed to launch Codex: {err}; also failed to remove the temporary config directory: {cleanup_err}"
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::AppType;
    use crate::cli::codex_temp_launch::prepare_launch_with;
    use crate::cli::tui::app::{App, ToastKind};
    use crate::cli::tui::data::{ProviderRow, ProvidersSnapshot, UiData};
    use crate::cli::tui::runtime_systems::RequestTracker;
    use crate::provider::Provider;
    use serde_json::{json, Value};
    use std::cell::Cell;
    use std::path::PathBuf;
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
                session_req_tx: None,
                webdav_req_tx: None,
                webdav_loading: &mut self.webdav_loading,
                update_req_tx: None,
                update_check: &mut self.update_check,
                model_fetch_req_tx: None,
            }
        }
    }

    fn provider_row(id: &str, auth: Option<Value>) -> ProviderRow {
        let mut settings = serde_json::Map::new();
        settings.insert(
            "config".to_string(),
            Value::String("model_provider = \"demo\"\nmodel = \"gpt-5.2-codex\"\n".to_string()),
        );
        if let Some(auth) = auth {
            settings.insert("auth".to_string(), auth);
        }

        ProviderRow {
            id: id.to_string(),
            provider: Provider::with_id(
                id.to_string(),
                format!("Provider {id}"),
                Value::Object(settings),
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

    #[test]
    fn launch_failure_does_not_switch_current_provider_and_surfaces_a_toast() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let mut fixture = LaunchFixture::new(
            AppType::Codex,
            "current",
            vec![
                provider_row("current", Some(json!({ "OPENAI_API_KEY": "sk-current" }))),
                provider_row(
                    "candidate",
                    Some(json!({ "OPENAI_API_KEY": "sk-candidate" })),
                ),
            ],
        );
        let captured_home = std::cell::RefCell::new(None::<PathBuf>);

        launch_with(
            &mut fixture.ctx(),
            "candidate".to_string(),
            temp_dir.path(),
            ensure_temp_launch_supported,
            |provider, temp_dir| {
                prepare_launch_with(provider, temp_dir, || Ok(PathBuf::from("/usr/bin/codex")))
            },
            |_, prepared| {
                captured_home.replace(Some(prepared.codex_home.clone()));
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
        let codex_home = captured_home
            .into_inner()
            .expect("handoff should observe the temp codex home");
        assert!(
            !codex_home.exists(),
            "temp Codex home should be removed after a failed launch"
        );
    }

    #[test]
    fn non_codex_runtime_launch_is_ignored_before_handoff() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let mut fixture = LaunchFixture::new(
            AppType::Claude,
            "current",
            vec![provider_row(
                "candidate",
                Some(json!({ "OPENAI_API_KEY": "sk-candidate" })),
            )],
        );
        let handoff_called = Cell::new(false);

        launch_with(
            &mut fixture.ctx(),
            "candidate".to_string(),
            temp_dir.path(),
            ensure_temp_launch_supported,
            |provider, temp_dir| {
                prepare_launch_with(provider, temp_dir, || Ok(PathBuf::from("/usr/bin/codex")))
            },
            |_, _| {
                handoff_called.set(true);
                Ok(())
            },
        )
        .expect("non-Codex runtime dispatch should be ignored");

        assert!(
            !handoff_called.get(),
            "non-Codex apps should not attempt the Codex temporary launch handoff"
        );
        assert!(
            fixture.app.toast.is_none(),
            "ignored non-Codex dispatch should stay silent"
        );
    }

    #[test]
    fn unsupported_platform_fails_before_writing_temp_codex_home() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let mut fixture = LaunchFixture::new(
            AppType::Codex,
            "current",
            vec![provider_row(
                "candidate",
                Some(json!({ "OPENAI_API_KEY": "sk-candidate" })),
            )],
        );
        let prepare_called = Cell::new(false);

        let err = try_launch_with(
            &mut fixture.ctx(),
            "candidate",
            temp_dir.path(),
            || {
                Err(AppError::localized(
                    "codex.temp_launch_unsupported_platform",
                    "当前平台暂不支持在当前终端临时启动 Codex。".to_string(),
                    "Temporary Codex launch in the current terminal is not supported on this platform."
                        .to_string(),
                ))
            },
            |provider, temp_dir| {
                prepare_called.set(true);
                prepare_launch_with(provider, temp_dir, || Ok(PathBuf::from("/usr/bin/codex")))
            },
            |_, _| Ok(()),
        )
        .expect_err("unsupported platforms should fail before preparing temp Codex home");

        assert!(
            err.to_string().contains("not supported"),
            "unexpected error: {err}"
        );
        assert!(
            !prepare_called.get(),
            "unsupported platforms should fail before resolving the Codex binary"
        );
        assert!(
            std::fs::read_dir(temp_dir.path())
                .expect("read temp dir")
                .next()
                .is_none(),
            "unsupported platforms should not create a temp Codex home"
        );
    }
}
