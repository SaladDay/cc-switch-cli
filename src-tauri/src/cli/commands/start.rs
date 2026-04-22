use std::ffi::OsString;

use clap::Subcommand;
use indexmap::IndexMap;

use crate::app_config::AppType;
use crate::cli::claude_temp_launch::{
    ensure_temp_launch_supported, exec_prepared_claude, prepare_launch_from_settings,
    PreparedClaudeLaunch,
};
use crate::cli::codex_temp_launch::{
    ensure_temp_launch_supported as ensure_codex_temp_launch_supported, exec_prepared_codex,
    prepare_launch as prepare_codex_launch, PreparedCodexLaunch,
};
use crate::error::AppError;
use crate::provider::Provider;
use crate::services::ProviderService;
use crate::store::AppState;

const CLAUDE_RESERVED_NATIVE_ARGS: &[&str] = &["--settings"];
const CLAUDE_START_AFTER_LONG_HELP: &str = "\
Examples:
  cc-switch start claude demo
  cc-switch start claude demo -- --dangerously-skip-permissions";

const CODEX_START_AFTER_LONG_HELP: &str = "\
Examples:
  cc-switch start codex demo
  cc-switch start codex demo -- --model gpt-5.4";

#[derive(Subcommand)]
pub enum StartCommand {
    /// Start Claude with a provider selector without switching the global current provider
    #[command(after_long_help = CLAUDE_START_AFTER_LONG_HELP)]
    Claude {
        /// Provider selector: exact ID first, then exact Name
        selector: String,
        /// Native Claude CLI arguments to pass through after `--`
        #[arg(last = true, value_name = "NATIVE_ARGS")]
        native_args: Vec<OsString>,
    },
    /// Start Codex with a provider selector without switching the global current provider
    #[command(after_long_help = CODEX_START_AFTER_LONG_HELP)]
    Codex {
        /// Provider selector: exact ID first, then exact Name
        selector: String,
        /// Native Codex CLI arguments to pass through after `--`
        #[arg(last = true, value_name = "NATIVE_ARGS")]
        native_args: Vec<OsString>,
    },
}

pub fn execute(cmd: StartCommand) -> Result<(), AppError> {
    match cmd {
        StartCommand::Claude {
            selector,
            native_args,
        } => start_claude(&selector, &native_args),
        StartCommand::Codex {
            selector,
            native_args,
        } => start_codex(&selector, &native_args),
    }
}

fn get_state() -> Result<AppState, AppError> {
    AppState::try_new()
}

fn start_claude(selector: &str, native_args: &[OsString]) -> Result<(), AppError> {
    reject_reserved_native_args(native_args, "Claude", CLAUDE_RESERVED_NATIVE_ARGS)?;
    let state = get_state()?;
    let providers = ProviderService::list(&state, AppType::Claude)?;
    let provider = resolve_provider_selector(&providers, selector, "Claude")?;

    ensure_temp_launch_supported()?;
    let prepared = prepare_claude_launch_with(&state, &provider, &std::env::temp_dir())?;
    handoff_claude_and_cleanup(&prepared, native_args)
}

fn start_codex(selector: &str, native_args: &[OsString]) -> Result<(), AppError> {
    start_with(
        selector,
        "Codex",
        || {
            let state = get_state()?;
            ProviderService::list(&state, AppType::Codex)
        },
        |provider| {
            ensure_codex_temp_launch_supported()?;
            let prepared = prepare_codex_launch(provider, &std::env::temp_dir())?;
            handoff_codex_and_cleanup(&prepared, native_args)
        },
    )
}

fn reject_reserved_native_args(
    native_args: &[OsString],
    app_name: &str,
    reserved_args: &[&str],
) -> Result<(), AppError> {
    let Some(conflicting_arg) = native_args.iter().find(|arg| {
        reserved_args
            .iter()
            .any(|reserved| is_reserved_native_arg(arg, reserved))
    }) else {
        return Ok(());
    };

    let conflicting_arg = conflicting_arg.to_string_lossy();
    let app_slug = app_name.to_ascii_lowercase();
    Err(AppError::localized(
        "cli.start.reserved_native_arg",
        format!(
            "`{conflicting_arg}` 不能通过 `cc-switch start {app_slug}` 透传，因为该参数由 cc-switch 管理。"
        ),
        format!(
            "`{conflicting_arg}` cannot be passed through `cc-switch start {app_slug}` because this flag is managed by cc-switch."
        ),
    ))
}

fn is_reserved_native_arg(arg: &OsString, reserved_arg: &str) -> bool {
    let arg = arg.to_string_lossy();
    arg == reserved_arg
        || arg
            .strip_prefix(reserved_arg)
            .is_some_and(|suffix| suffix.starts_with('='))
}

fn start_with<Load, Launch>(
    selector: &str,
    app_name: &str,
    load_providers: Load,
    launch_provider: Launch,
) -> Result<(), AppError>
where
    Load: FnOnce() -> Result<IndexMap<String, Provider>, AppError>,
    Launch: FnOnce(&Provider) -> Result<(), AppError>,
{
    let providers = load_providers()?;
    let provider = resolve_provider_selector(&providers, selector, app_name)?;
    launch_provider(&provider)
}

fn prepare_claude_launch_with(
    state: &AppState,
    provider: &Provider,
    temp_dir: &std::path::Path,
) -> Result<PreparedClaudeLaunch, AppError> {
    let settings = ProviderService::build_effective_live_snapshot_from_state(
        state,
        AppType::Claude,
        provider,
    )?;
    prepare_launch_from_settings(&provider.id, &settings, temp_dir)
}

fn handoff_claude_and_cleanup(
    prepared: &PreparedClaudeLaunch,
    native_args: &[OsString],
) -> Result<(), AppError> {
    finish_launch(
        exec_prepared_claude(prepared, native_args),
        prepared.cleanup_settings_file(),
        "Claude",
        "临时设置文件",
        "temporary settings file",
        "claude.temp_launch_cleanup_failed",
    )
}

fn handoff_codex_and_cleanup(
    prepared: &PreparedCodexLaunch,
    native_args: &[OsString],
) -> Result<(), AppError> {
    finish_launch(
        exec_prepared_codex(prepared, native_args),
        prepared.cleanup_home_dir(),
        "Codex",
        "临时配置目录",
        "temporary config directory",
        "codex.temp_launch_cleanup_failed",
    )
}

fn finish_launch(
    handoff_result: Result<(), AppError>,
    cleanup_result: Result<(), AppError>,
    app_name: &str,
    cleanup_target_zh: &str,
    cleanup_target_en: &str,
    cleanup_failed_key: &'static str,
) -> Result<(), AppError> {
    match (handoff_result, cleanup_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(cleanup_err)) => Err(cleanup_err),
        (Err(err), Ok(())) => Err(err),
        (Err(err), Err(cleanup_err)) => Err(AppError::localized(
            cleanup_failed_key,
            format!(
                "启动 {app_name} 失败: {err}；同时清理{cleanup_target_zh}失败: {cleanup_err}"
            ),
            format!(
                "Failed to launch {app_name}: {err}; also failed to remove the {cleanup_target_en}: {cleanup_err}"
            ),
        )),
    }
}

fn resolve_provider_selector(
    providers: &IndexMap<String, Provider>,
    selector: &str,
    app_name: &str,
) -> Result<Provider, AppError> {
    if let Some(provider) = providers.get(selector) {
        return Ok(provider.clone());
    }

    let exact_name_matches: Vec<_> = providers
        .values()
        .filter(|provider| provider.name == selector)
        .cloned()
        .collect();

    match exact_name_matches.as_slice() {
        [] => Err(AppError::localized(
            "cli.start.provider_selector_not_found",
            format!(
                "供应商选择器 '{}' 未匹配到任何 {} 供应商的 ID 或名称",
                selector, app_name
            ),
            format!(
                "Provider selector '{}' did not match any {} provider by ID or name",
                selector, app_name
            ),
        )),
        [provider] => Ok(provider.clone()),
        matches => {
            let ids = matches
                .iter()
                .map(|provider| provider.id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Err(AppError::localized(
                "cli.start.provider_selector_ambiguous",
                format!(
                    "供应商选择器 '{}' 存在歧义，匹配到这些 ID: {}",
                    selector, ids
                ),
                format!(
                    "Provider selector '{}' is ambiguous. Matching IDs: {}",
                    selector, ids
                ),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::ffi::OsString;
    use tempfile::TempDir;

    fn provider(id: &str, name: &str) -> Provider {
        Provider::with_id(
            id.to_string(),
            name.to_string(),
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": format!("sk-{id}")
                }
            }),
            None,
        )
    }

    fn state_from_config(config: crate::app_config::MultiAppConfig) -> AppState {
        let db = std::sync::Arc::new(crate::Database::memory().expect("create memory database"));
        db.migrate_from_json(&config)
            .expect("seed memory database from config");
        AppState {
            db: db.clone(),
            config: std::sync::RwLock::new(config),
            proxy_service: crate::ProxyService::new(db),
        }
    }

    #[test]
    fn claude_rejects_reserved_settings_native_arg() {
        let err = reject_reserved_native_args(
            &[OsString::from("--settings")],
            "Claude",
            CLAUDE_RESERVED_NATIVE_ARGS,
        )
        .expect_err("reserved Claude args should be rejected");

        assert!(err.to_string().contains("--settings"));
        assert!(err.to_string().contains("cc-switch start claude"));
    }

    #[test]
    fn claude_rejects_reserved_settings_equals_native_arg() {
        let err = reject_reserved_native_args(
            &[OsString::from("--settings=/tmp/other.json")],
            "Claude",
            CLAUDE_RESERVED_NATIVE_ARGS,
        )
        .expect_err("reserved Claude args should be rejected");

        assert!(err.to_string().contains("--settings=/tmp/other.json"));
    }

    #[test]
    fn codex_allows_non_reserved_native_args() {
        reject_reserved_native_args(&[OsString::from("--model")], "Codex", &[])
            .expect("Codex passthrough args should remain allowed");
    }

    #[test]
    fn selector_prefers_exact_id_over_name() {
        let providers = IndexMap::from([
            ("demo".to_string(), provider("demo", "Shared Name")),
            ("other".to_string(), provider("other", "demo")),
        ]);

        let resolved =
            resolve_provider_selector(&providers, "demo", "Claude").expect("resolve provider");

        assert_eq!(resolved.id, "demo");
    }

    #[test]
    fn selector_accepts_exact_name_when_id_is_missing() {
        let providers = IndexMap::from([("demo".to_string(), provider("demo", "Claude Demo"))]);

        let resolved = resolve_provider_selector(&providers, "Claude Demo", "Claude")
            .expect("resolve by name");

        assert_eq!(resolved.id, "demo");
    }

    #[test]
    fn selector_rejects_ambiguous_names() {
        let providers = IndexMap::from([
            ("demo-a".to_string(), provider("demo-a", "Shared Name")),
            ("demo-b".to_string(), provider("demo-b", "Shared Name")),
        ]);

        let err = resolve_provider_selector(&providers, "Shared Name", "Claude")
            .expect_err("ambiguous name should fail");

        assert!(err.to_string().contains("Shared Name"));
        assert!(err.to_string().contains("demo-a"));
        assert!(err.to_string().contains("demo-b"));
    }

    #[test]
    fn start_claude_launches_resolved_provider() {
        let providers = IndexMap::from([("demo".to_string(), provider("demo", "Claude Demo"))]);
        let launched = std::cell::RefCell::new(None::<String>);

        start_with(
            "Claude Demo",
            "Claude",
            || Ok(providers),
            |provider| {
                launched.replace(Some(provider.id.clone()));
                Ok(())
            },
        )
        .expect("start command should launch matching provider");

        assert_eq!(launched.into_inner().as_deref(), Some("demo"));
    }

    #[test]
    fn start_codex_launches_resolved_provider() {
        let providers = IndexMap::from([("demo".to_string(), provider("demo", "Codex Demo"))]);
        let launched = std::cell::RefCell::new(None::<String>);

        start_with(
            "Codex Demo",
            "Codex",
            || Ok(providers),
            |provider| {
                launched.replace(Some(provider.id.clone()));
                Ok(())
            },
        )
        .expect("start command should launch matching provider");

        assert_eq!(launched.into_inner().as_deref(), Some("demo"));
    }

    #[test]
    fn prepare_claude_launch_with_writes_effective_snapshot_from_state() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let mut config = crate::app_config::MultiAppConfig::default();
        config.ensure_app(&AppType::Claude);
        config.common_config_snippets.claude = Some(
            r#"{"env":{"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":1,"ANTHROPIC_BASE_URL":"https://common.example"},"includeCoAuthoredBy":false,"permissions":{"allow":["Bash(ls)"]}}"#
                .to_string(),
        );
        let state = state_from_config(config);
        let provider = Provider::with_id(
            "demo".to_string(),
            "Claude Demo".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "sk-demo",
                    "ANTHROPIC_BASE_URL": "https://provider.example"
                },
                "includeCoAuthoredBy": true,
                "permissions": {
                    "allow": ["Bash(git status)"]
                }
            }),
            None,
        );

        let prepared =
            prepare_claude_launch_with(&state, &provider, temp_dir.path()).expect("prepare launch");
        let written: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&prepared.settings_path).expect("read temp settings"),
        )
        .expect("parse temp settings");
        let expected = ProviderService::build_effective_live_snapshot_from_state(
            &state,
            AppType::Claude,
            &provider,
        )
        .expect("build effective snapshot");

        assert_eq!(
            written, expected,
            "CLI temp launch should write the canonical effective snapshot"
        );
    }

    #[test]
    fn finish_launch_returns_cleanup_error_when_handoff_succeeds() {
        let err = finish_launch(
            Ok(()),
            Err(AppError::Message("cleanup failed".to_string())),
            "Codex",
            "临时配置目录",
            "temporary config directory",
            "codex.temp_launch_cleanup_failed",
        )
        .expect_err("cleanup failure should bubble up");

        assert!(err.to_string().contains("cleanup failed"));
    }

    #[test]
    fn finish_launch_combines_handoff_and_cleanup_errors() {
        let err = finish_launch(
            Err(AppError::Message("handoff failed".to_string())),
            Err(AppError::Message("cleanup failed".to_string())),
            "Codex",
            "临时配置目录",
            "temporary config directory",
            "codex.temp_launch_cleanup_failed",
        )
        .expect_err("dual failure should be combined");

        assert!(err.to_string().contains("handoff failed"));
        assert!(err.to_string().contains("cleanup failed"));
    }
}
