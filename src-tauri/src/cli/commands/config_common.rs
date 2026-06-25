use clap::Subcommand;
use std::fs;
use std::path::Path;

use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::cli::ui::{highlight, info, success};
use crate::error::AppError;
use crate::services::ProviderService;
use crate::store::AppState;

#[derive(Subcommand, Debug, Clone)]
pub enum CommonConfigCommand {
    /// Show current common config snippet
    Show,
    /// Format a common config snippet and print the normalized result
    Format {
        /// Inline snippet text (Claude/Gemini/OpenCode/OpenClaw: JSON object; Codex: TOML)
        #[arg(long = "snippet", value_name = "SNIPPET", conflicts_with = "file")]
        snippet: Option<String>,

        /// Read snippet text from file using the selected app's format rules
        #[arg(long, conflicts_with = "snippet")]
        file: Option<std::path::PathBuf>,
    },
    /// Extract a common config snippet from a provider or settings payload
    Extract {
        /// Provider ID to extract from; defaults to the current provider
        #[arg(long)]
        provider: Option<String>,

        /// Inline settingsConfig JSON payload to extract from
        #[arg(long = "settings-config", value_name = "JSON", conflicts_with_all = ["provider", "file"])]
        settings_config: Option<String>,

        /// Read settingsConfig JSON payload from file
        #[arg(long, conflicts_with_all = ["provider", "settings_config"])]
        file: Option<std::path::PathBuf>,

        /// Save the extracted snippet as the app's common config
        #[arg(long)]
        save: bool,
    },
    /// Set common config snippet for the selected app
    #[command(
        after_long_help = "Compatibility:\n  --json <SNIPPET>  Legacy alias for --snippet <SNIPPET>."
    )]
    Set {
        /// Inline snippet text (Claude/Gemini/OpenCode: JSON object; Codex: TOML)
        #[arg(
            long = "snippet",
            alias = "json",
            value_name = "SNIPPET",
            conflicts_with = "file"
        )]
        snippet: Option<String>,

        /// Read snippet text from file using the selected app's format rules
        #[arg(long, conflicts_with = "snippet")]
        file: Option<std::path::PathBuf>,

        /// Compatibility flag; changes already try to refresh the current live config when applicable
        #[arg(long)]
        apply: bool,
    },
    /// Clear common config snippet for the selected app
    Clear {
        /// Compatibility flag; clearing already tries to refresh the current live config when applicable
        #[arg(long)]
        apply: bool,
    },
}

pub fn execute(cmd: CommonConfigCommand, app_type: AppType) -> Result<(), AppError> {
    match cmd {
        CommonConfigCommand::Show => show(app_type),
        CommonConfigCommand::Format { snippet, file } => {
            format(app_type, snippet.as_deref(), file.as_deref())
        }
        CommonConfigCommand::Extract {
            provider,
            settings_config,
            file,
            save,
        } => extract(
            app_type,
            provider.as_deref(),
            settings_config.as_deref(),
            file.as_deref(),
            save,
        ),
        CommonConfigCommand::Set {
            snippet,
            file,
            apply,
        } => set(app_type, snippet.as_deref(), file.as_deref(), apply),
        CommonConfigCommand::Clear { apply } => clear(app_type, apply),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommonConfigSnippetAction {
    Set,
    Clear,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FollowUpMessage {
    Info(&'static str),
    Success(&'static str),
}

fn get_state() -> Result<AppState, AppError> {
    AppState::try_new()
}

fn no_current_provider_message(action: CommonConfigSnippetAction) -> &'static str {
    match action {
        CommonConfigSnippetAction::Set => texts::common_config_snippet_no_current_provider(),
        CommonConfigSnippetAction::Clear => {
            texts::common_config_snippet_no_current_provider_after_clear()
        }
    }
}

fn follow_up_message(
    app_type: AppType,
    action: CommonConfigSnippetAction,
    current_id: &str,
) -> Option<FollowUpMessage> {
    if app_type.is_additive_mode() {
        return None;
    }

    if current_id.trim().is_empty() {
        Some(FollowUpMessage::Info(no_current_provider_message(action)))
    } else {
        Some(FollowUpMessage::Success(
            texts::common_config_snippet_applied(),
        ))
    }
}

fn show(app_type: AppType) -> Result<(), AppError> {
    let state = get_state()?;
    let config = state.config.read()?;
    let snippet = config.common_config_snippets.get(&app_type).cloned();

    println!("{}", highlight(texts::config_common_snippet_title()));
    println!("{}", "=".repeat(50));
    println!("App: {}", app_type.as_str());
    println!();

    match snippet {
        Some(snippet) if !snippet.trim().is_empty() => println!("{}", snippet),
        _ => println!("{}", info(texts::config_common_snippet_none_set())),
    }

    Ok(())
}

fn read_required_text(
    inline_text: Option<&str>,
    file: Option<&Path>,
    missing_message: &'static str,
) -> Result<String, AppError> {
    if let Some(text) = inline_text {
        Ok(text.to_string())
    } else if let Some(path) = file {
        fs::read_to_string(path).map_err(|e| AppError::io(path, e))
    } else {
        Err(AppError::InvalidInput(missing_message.to_string()))
    }
}

fn canonical_common_snippet(app_type: AppType, raw: &str) -> Result<Option<String>, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    match app_type {
        AppType::Claude
        | AppType::Gemini
        | AppType::OpenCode
        | AppType::Hermes
        | AppType::OpenClaw => {
            let value: serde_json::Value = serde_json::from_str(raw).map_err(|e| {
                AppError::InvalidInput(texts::tui_toast_invalid_json(&e.to_string()))
            })?;
            if !value.is_object() {
                return Err(AppError::InvalidInput(
                    texts::common_config_snippet_not_object().to_string(),
                ));
            }

            serde_json::to_string_pretty(&value)
                .map(Some)
                .map_err(|e| AppError::Message(texts::failed_to_serialize_json(&e.to_string())))
        }
        AppType::Codex => {
            let doc = trimmed.parse::<toml_edit::DocumentMut>().map_err(|e| {
                AppError::InvalidInput(texts::common_config_snippet_invalid_toml(&e.to_string()))
            })?;
            Ok(Some(doc.to_string().trim().to_string()))
        }
    }
}

fn format(
    app_type: AppType,
    snippet_text: Option<&str>,
    file: Option<&Path>,
) -> Result<(), AppError> {
    let raw = read_required_text(
        snippet_text,
        file,
        texts::config_common_snippet_require_json_or_file(),
    )?;
    if let Some(snippet) = canonical_common_snippet(app_type, &raw)? {
        println!("{}", snippet);
    }
    Ok(())
}

fn set(
    app_type: AppType,
    snippet_text: Option<&str>,
    file: Option<&Path>,
    _apply: bool,
) -> Result<(), AppError> {
    let raw = read_required_text(
        snippet_text,
        file,
        texts::config_common_snippet_require_json_or_file(),
    )?;
    let snippet = canonical_common_snippet(app_type.clone(), &raw)?.unwrap_or_default();

    let state = get_state()?;
    ProviderService::set_common_config_snippet(&state, app_type.clone(), Some(snippet))?;

    println!(
        "{}",
        success(&texts::config_common_snippet_set_for_app(app_type.as_str()))
    );

    let current_id = if app_type.is_additive_mode() {
        String::new()
    } else {
        ProviderService::current(&state, app_type.clone())?
    };
    if let Some(message) = follow_up_message(app_type, CommonConfigSnippetAction::Set, &current_id)
    {
        match message {
            FollowUpMessage::Info(text) => println!("{}", info(text)),
            FollowUpMessage::Success(text) => println!("{}", success(text)),
        }
    }

    Ok(())
}

fn extract(
    app_type: AppType,
    provider_id: Option<&str>,
    settings_config_text: Option<&str>,
    file: Option<&Path>,
    save: bool,
) -> Result<(), AppError> {
    let state = get_state()?;
    let extracted = if settings_config_text.is_some() || file.is_some() {
        let raw = read_required_text(
            settings_config_text,
            file,
            texts::config_common_snippet_require_json_or_file(),
        )?;
        let settings_config: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| AppError::InvalidInput(texts::tui_toast_invalid_json(&e.to_string())))?;
        ProviderService::extract_common_config_snippet_from_settings(
            app_type.clone(),
            &settings_config,
        )?
    } else if let Some(provider_id) = provider_id {
        let providers = ProviderService::list(&state, app_type.clone())?;
        let provider = providers.get(provider_id).ok_or_else(|| {
            let msg = texts::provider_not_found(provider_id);
            AppError::localized("provider.not_found", msg.clone(), msg)
        })?;
        ProviderService::extract_common_config_snippet_from_settings(
            app_type.clone(),
            &provider.settings_config,
        )?
    } else {
        ProviderService::extract_common_config_snippet(&state, app_type.clone())?
    };

    if save {
        let snippet = canonical_common_snippet(app_type.clone(), &extracted)?.unwrap_or_default();
        ProviderService::set_common_config_snippet(
            &state,
            app_type.clone(),
            Some(snippet.clone()),
        )?;
        println!("{}", success(texts::common_config_snippet_extracted()));
        if !snippet.trim().is_empty() {
            println!();
            println!("{}", snippet);
        }
        return Ok(());
    }

    if let Some(snippet) = canonical_common_snippet(app_type, &extracted)? {
        println!("{}", snippet);
    }
    Ok(())
}

fn clear(app_type: AppType, _apply: bool) -> Result<(), AppError> {
    let state = get_state()?;
    ProviderService::set_common_config_snippet(&state, app_type.clone(), None)?;

    println!(
        "{}",
        success(&format!(
            "✓ Common config snippet cleared for app '{}'",
            app_type.as_str()
        ))
    );

    let current_id = if app_type.is_additive_mode() {
        String::new()
    } else {
        ProviderService::current(&state, app_type.clone())?
    };
    if let Some(message) =
        follow_up_message(app_type, CommonConfigSnippetAction::Clear, &current_id)
    {
        match message {
            FollowUpMessage::Info(text) => println!("{}", info(text)),
            FollowUpMessage::Success(text) => println!("{}", success(text)),
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;
    use serial_test::serial;
    use tempfile::TempDir;

    use crate::codex_config::{get_codex_config_dir, get_codex_config_path};
    use crate::config::{get_claude_settings_path, read_json_file, write_json_file};
    use crate::provider::{Provider, ProviderMeta};
    use crate::services::ProviderService;
    use crate::test_support::TestEnvGuard;

    type EnvGuard = TestEnvGuard;

    fn common_config_meta(enabled: bool) -> ProviderMeta {
        ProviderMeta {
            apply_common_config: Some(enabled),
            ..Default::default()
        }
    }

    fn seed_current_claude_provider_with_meta(meta: Option<ProviderMeta>) -> (TempDir, EnvGuard) {
        let temp_home = TempDir::new().expect("create temp home");
        let env = TestEnvGuard::isolated(temp_home.path());
        let state = AppState::try_new().expect("create state");
        let mut provider = Provider::with_id(
            "p1".to_string(),
            "Provider One".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://provider.example"
                }
            }),
            None,
        );
        provider.meta = meta;

        ProviderService::add(&state, AppType::Claude, provider).expect("seed provider");
        ProviderService::switch(&state, AppType::Claude, "p1").expect("switch provider");
        write_json_file(
            &get_claude_settings_path(),
            &json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://provider.example",
                    "LOCAL_ONLY": "preserve-me"
                }
            }),
        )
        .expect("seed live settings");

        (temp_home, env)
    }

    fn seed_current_claude_provider() -> (TempDir, EnvGuard) {
        seed_current_claude_provider_with_meta(None)
    }

    fn seed_current_codex_provider_with_meta(meta: Option<ProviderMeta>) -> (TempDir, EnvGuard) {
        let temp_home = TempDir::new().expect("create temp home");
        let env = TestEnvGuard::isolated(temp_home.path());
        std::fs::create_dir_all(get_codex_config_dir()).expect("create codex config dir");
        let state = AppState::try_new().expect("create state");
        let mut provider = Provider::with_id(
            "p1".to_string(),
            "Provider One".to_string(),
            json!({
                "auth": {
                    "OPENAI_API_KEY": "sk-provider"
                },
                "config": "model_provider = \"provider-one\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.provider-one]\nbase_url = \"https://provider.example/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
            }),
            None,
        );
        provider.meta = meta;

        ProviderService::add(&state, AppType::Codex, provider).expect("seed codex provider");
        ProviderService::switch(&state, AppType::Codex, "p1").expect("switch provider");

        (temp_home, env)
    }

    fn seed_current_codex_provider() -> (TempDir, EnvGuard) {
        seed_current_codex_provider_with_meta(None)
    }

    #[test]
    #[serial]
    fn set_stores_claude_snippet_without_enabling_provider_live_config() {
        let (_temp_home, _env) = seed_current_claude_provider();

        set(
            AppType::Claude,
            Some(r#"{"alwaysThinkingEnabled":false}"#),
            None,
            false,
        )
        .expect("set should succeed");

        let state = AppState::try_new().expect("reload state");
        let stored = state
            .config
            .read()
            .expect("read config")
            .common_config_snippets
            .claude
            .clone()
            .expect("stored claude snippet");
        let stored_json: serde_json::Value =
            serde_json::from_str(&stored).expect("stored snippet should be valid JSON");
        assert_eq!(stored_json["alwaysThinkingEnabled"], false);

        let live: serde_json::Value =
            read_json_file(&get_claude_settings_path()).expect("read live settings");
        assert!(
            live.get("alwaysThinkingEnabled").is_none(),
            "setting a new common snippet must not opt the current provider into common config"
        );
    }

    #[test]
    #[serial]
    fn set_updates_live_config_for_common_config_enabled_claude_provider() {
        let (_temp_home, _env) =
            seed_current_claude_provider_with_meta(Some(common_config_meta(true)));

        set(
            AppType::Claude,
            Some(r#"{"alwaysThinkingEnabled":false}"#),
            None,
            false,
        )
        .expect("set should succeed");

        let live: serde_json::Value =
            read_json_file(&get_claude_settings_path()).expect("read live settings");
        assert_eq!(live["alwaysThinkingEnabled"], false);
    }

    #[test]
    #[serial]
    fn clear_updates_live_config_even_without_apply_flag() {
        let (_temp_home, _env) = seed_current_claude_provider();
        let state = AppState::try_new().expect("reload state");

        ProviderService::set_common_config_snippet(
            &state,
            AppType::Claude,
            Some(r#"{"alwaysThinkingEnabled":false}"#.to_string()),
        )
        .expect("seed common snippet");

        clear(AppType::Claude, false).expect("clear should succeed");

        let live: serde_json::Value =
            read_json_file(&get_claude_settings_path()).expect("read live settings");
        assert!(
            live.get("alwaysThinkingEnabled").is_none(),
            "clearing the common snippet should also clear it from live settings"
        );
    }

    #[test]
    #[serial]
    fn set_accepts_codex_toml_common_snippet_without_enabling_provider_live_config() {
        let (_temp_home, _env) = seed_current_codex_provider();

        set(
            AppType::Codex,
            Some("disable_response_storage = true"),
            None,
            false,
        )
        .expect("set should accept codex toml snippet");

        let state = AppState::try_new().expect("reload state");
        let stored = state
            .config
            .read()
            .expect("read config")
            .common_config_snippets
            .codex
            .clone();
        assert_eq!(stored.as_deref(), Some("disable_response_storage = true"));

        let live =
            std::fs::read_to_string(get_codex_config_path()).expect("read live codex config");
        assert!(
            !live.contains("disable_response_storage = true"),
            "setting a new common snippet must not opt the current provider into common config"
        );
    }

    #[test]
    #[serial]
    fn set_accepts_codex_toml_common_snippet_and_updates_enabled_provider_live_config() {
        let (_temp_home, _env) =
            seed_current_codex_provider_with_meta(Some(common_config_meta(true)));

        set(
            AppType::Codex,
            Some("disable_response_storage = true"),
            None,
            false,
        )
        .expect("set should accept codex toml snippet");

        let live =
            std::fs::read_to_string(get_codex_config_path()).expect("read live codex config");
        assert!(
            live.contains("disable_response_storage = true"),
            "service path should merge the codex common snippet into the live config"
        );
    }

    #[test]
    fn format_pretty_prints_json_common_snippet() {
        let formatted =
            canonical_common_snippet(AppType::Claude, r#"{"env":{"CC_SWITCH_TEST":"1"}}"#)
                .expect("format snippet")
                .expect("non-empty snippet");

        assert_eq!(
            formatted,
            "{\n  \"env\": {\n    \"CC_SWITCH_TEST\": \"1\"\n  }\n}"
        );
    }

    #[test]
    fn format_normalizes_codex_toml_common_snippet() {
        let formatted = canonical_common_snippet(AppType::Codex, "model_reasoning_effort=\"high\"")
            .expect("format snippet")
            .expect("non-empty snippet");

        let parsed = formatted
            .parse::<toml_edit::DocumentMut>()
            .expect("formatted snippet should remain valid TOML");
        assert_eq!(
            parsed
                .get("model_reasoning_effort")
                .and_then(|item| item.as_str()),
            Some("high")
        );
    }

    #[test]
    #[serial]
    fn extract_from_settings_config_saves_common_snippet() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());

        extract(
            AppType::Claude,
            None,
            Some(
                r#"{
                    "env": {
                        "ANTHROPIC_BASE_URL": "https://provider.example",
                        "ANTHROPIC_AUTH_TOKEN": "sk-test",
                        "CC_SWITCH_SHARED": "1"
                    }
                }"#,
            ),
            None,
            true,
        )
        .expect("extract should succeed");

        let state = AppState::try_new().expect("reload state");
        let stored = state
            .config
            .read()
            .expect("read config")
            .common_config_snippets
            .claude
            .clone()
            .expect("stored claude snippet");
        let stored_json: serde_json::Value =
            serde_json::from_str(&stored).expect("stored snippet should be valid JSON");
        assert_eq!(stored_json["env"]["CC_SWITCH_SHARED"], "1");
        assert!(stored_json["env"].get("ANTHROPIC_AUTH_TOKEN").is_none());
    }

    #[test]
    #[serial]
    fn extract_from_named_provider_saves_common_snippet() {
        let (_temp_home, _env) = seed_current_claude_provider();

        extract(AppType::Claude, Some("p1"), None, None, true).expect("extract should succeed");

        let state = AppState::try_new().expect("reload state");
        let stored = state
            .config
            .read()
            .expect("read config")
            .common_config_snippets
            .claude
            .clone()
            .expect("stored claude snippet");
        let stored_json: serde_json::Value =
            serde_json::from_str(&stored).expect("stored snippet should be valid JSON");
        assert_eq!(stored_json, json!({}));
    }

    #[test]
    fn set_rejects_non_object_opencode_common_snippet() {
        let err = set(AppType::OpenCode, Some("[]"), None, false)
            .expect_err("OpenCode common snippet should require a JSON object");

        assert!(
            err.to_string()
                .contains(texts::common_config_snippet_not_object()),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn no_current_provider_message_preserves_saved_copy_for_set() {
        assert_eq!(
            no_current_provider_message(CommonConfigSnippetAction::Set),
            texts::common_config_snippet_no_current_provider()
        );
    }

    #[test]
    fn no_current_provider_message_uses_clear_copy_for_clear() {
        assert_eq!(
            no_current_provider_message(CommonConfigSnippetAction::Clear),
            texts::common_config_snippet_no_current_provider_after_clear()
        );
    }

    #[test]
    fn follow_up_message_is_omitted_for_additive_apps() {
        assert!(follow_up_message(AppType::OpenCode, CommonConfigSnippetAction::Set, "").is_none());
    }
}
