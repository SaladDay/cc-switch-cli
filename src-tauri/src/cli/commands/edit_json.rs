use clap::Subcommand;
use serde_json::Value;

use crate::app_config::AppType;
use crate::error::AppError;
use crate::provider::Provider;
use crate::services::provider::ProviderService;

#[derive(Subcommand)]
pub enum EditJsonCommand {
    /// Edit a provider's settings_config JSON in an external editor
    Provider {
        /// Provider ID
        id: String,

        /// Application type
        #[arg(long, value_enum)]
        app_type: AppType,

        /// Replace settings_config entirely (skip merge with existing keys)
        #[arg(long, default_value_t = false)]
        force: bool,
    },
}

pub fn execute(cmd: EditJsonCommand) -> Result<(), AppError> {
    match cmd {
        EditJsonCommand::Provider {
            id,
            app_type,
            force,
        } => edit_provider(&app_type, &id, force),
    }
}

/// Open the provider's settings_config in an external editor, validate the result,
/// and persist — merging with existing keys by default, or fully replacing when `force` is set.
fn edit_provider(app_type: &AppType, id: &str, force: bool) -> Result<(), AppError> {
    let state = crate::store::AppState::try_new()?;

    let provider = state
        .db
        .get_provider_by_id(id, app_type.as_str())?
        .ok_or_else(|| {
            AppError::InvalidInput(format!(
                "provider '{}' not found for app '{}'",
                id,
                app_type.as_str()
            ))
        })?;

    let initial = serde_json::to_string_pretty(&provider.settings_config)
        .map_err(|e| AppError::Message(format!("failed to serialize settings_config: {e}")))?;

    let edited = crate::cli::editor::open_external_editor(&initial)?;

    if edited.trim() == initial.trim() {
        println!("未修改，已取消");
        return Ok(());
    }

    let new_value = validate_edited_json(&edited, &provider, app_type)?;

    state
        .db
        .update_provider_settings_config(app_type.as_str(), id, &new_value, force)?;

    use crate::cli::ui::success;
    println!(
        "{}",
        success(&format!(
            "✓ 已更新 provider '{}' ({}) 的 settingsConfig",
            id,
            app_type.as_str()
        ))
    );
    Ok(())
}

/// Validate edited JSON: syntax → must be Object → business rules.
fn validate_edited_json(
    edited: &str,
    provider: &Provider,
    app_type: &AppType,
) -> Result<Value, AppError> {
    let value: Value = serde_json::from_str(edited).map_err(|e| {
        AppError::Message(format!("JSON 解析失败: {e}"))
    })?;

    if !value.is_object() {
        return Err(AppError::Message(
            "settingsConfig 必须为 JSON Object".to_string(),
        ));
    }

    if matches!(app_type, AppType::Codex) && !ProviderService::is_codex_official_provider(provider) {
        let config_text = value
            .get("config")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !ProviderService::codex_config_has_base_url(config_text) {
            return Err(AppError::Message(
                "Codex provider 必须配置非空的 base_url".to_string(),
            ));
        }
    }

    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use crate::provider::{Provider, ProviderMeta};
    use serde_json::json;

    fn make_provider(id: &str, settings_config: Value) -> Provider {
        let mut p = Provider::with_id(id.to_string(), "Test Provider".to_string(), settings_config, None);
        p.meta = Some(ProviderMeta::default());
        p
    }

    fn seed_provider(db: &Database, id: &str, app_type: &str, cfg: Value) {
        let p = make_provider(id, cfg);
        db.save_provider(app_type, &p).expect("seed provider");
    }

    #[test]
    fn save_provider_update_merges_custom_keys() {
        let db = Database::memory().expect("memory db");
        // Seed with a custom key
        seed_provider(
            &db,
            "test-id",
            "claude",
            json!({"env": {"ANTHROPIC_BASE_URL": "https://old.example.com"}, "customKey": "my-value"}),
        );

        // Simulate update that only touches canonical keys
        let mut updated = make_provider("test-id", json!({"env": {"ANTHROPIC_BASE_URL": "https://new.example.com"}}));
        updated.meta = None; // so save_provider preserves old meta
        db.save_provider("claude", &updated).expect("save");

        let after = db
            .get_provider_by_id("test-id", "claude")
            .expect("query")
            .expect("exists");
        // canonical key updated
        assert_eq!(after.settings_config["env"]["ANTHROPIC_BASE_URL"], "https://new.example.com");
        // custom key preserved by merge
        assert_eq!(after.settings_config["customKey"], "my-value");
    }

    #[test]
    fn update_provider_settings_config_merges_by_default() {
        let db = Database::memory().expect("memory db");
        seed_provider(
            &db,
            "test-id",
            "claude",
            json!({"env": {"BASE_URL": "old"}, "custom": "keep-me"}),
        );

        db.update_provider_settings_config(
            "claude",
            "test-id",
            &json!({"env": {"BASE_URL": "new"}}),
            false, // merge mode
        )
        .expect("update");

        let after = db
            .get_provider_by_id("test-id", "claude")
            .expect("query")
            .expect("exists");
        assert_eq!(after.settings_config["env"]["BASE_URL"], "new");
        assert_eq!(after.settings_config["custom"], "keep-me");
    }

    #[test]
    fn update_provider_settings_config_force_replaces_entirely() {
        let db = Database::memory().expect("memory db");
        seed_provider(
            &db,
            "test-id",
            "claude",
            json!({"env": {"BASE_URL": "old"}, "custom": "should-be-gone"}),
        );

        db.update_provider_settings_config(
            "claude",
            "test-id",
            &json!({"env": {"BASE_URL": "new"}}),
            true, // force replace
        )
        .expect("update");

        let after = db
            .get_provider_by_id("test-id", "claude")
            .expect("query")
            .expect("exists");
        assert_eq!(after.settings_config["env"]["BASE_URL"], "new");
        assert!(after.settings_config.get("custom").is_none(), "custom key should be removed by force replace");
    }

    #[test]
    fn json_syntax_error() {
        let db = Database::memory().expect("memory db");
        seed_provider(&db, "test-id", "claude", json!({"key": "value"}));

        let provider = db
            .get_provider_by_id("test-id", "claude")
            .expect("query")
            .expect("exists");

        let result = validate_edited_json("{broken", &provider, &AppType::Claude);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("JSON 解析失败"));
    }

    #[test]
    fn non_object_rejected() {
        let db = Database::memory().expect("memory db");
        seed_provider(&db, "test-id", "claude", json!({"key": "value"}));

        let provider = db
            .get_provider_by_id("test-id", "claude")
            .expect("query")
            .expect("exists");

        for invalid in &["[]", "\"string\"", "42", "null"] {
            let result = validate_edited_json(invalid, &provider, &AppType::Claude);
            assert!(
                result.is_err(),
                "expected error for input: {}",
                invalid
            );
            let err = result.unwrap_err().to_string();
            assert!(
                err.contains("JSON Object"),
                "unexpected error for '{}': {err}",
                invalid
            );
        }
    }

    #[test]
    fn codex_official_skips_base_url_check() {
        let db = Database::memory().expect("memory db");
        let mut provider = make_provider("test-id", json!({}));
        provider.meta.as_mut().unwrap().codex_official = Some(true);
        db.save_provider("codex", &provider).expect("seed");

        let provider = db
            .get_provider_by_id("test-id", "codex")
            .expect("query")
            .expect("exists");

        let result = validate_edited_json("{}", &provider, &AppType::Codex);
        assert!(result.is_ok(), "official codex should skip base_url check");
    }

    #[test]
    fn codex_non_official_missing_base_url_fails() {
        let db = Database::memory().expect("memory db");
        let provider = make_provider("test-id", json!({
            "config": "[model_provider]\nprovider = \"custom\"\n"
        }));
        db.save_provider("codex", &provider).expect("seed");

        let provider = db
            .get_provider_by_id("test-id", "codex")
            .expect("query")
            .expect("exists");

        let edited = json!({"config": "[model_provider]\nprovider = \"custom\"\n"}).to_string();
        let result = validate_edited_json(&edited, &provider, &AppType::Codex);
        assert!(result.is_err());

        let err = result.unwrap_err().to_string();
        assert!(err.contains("base_url"));
    }

    #[test]
    fn update_with_identical_content_is_noop() {
        let db = Database::memory().expect("memory db");
        let original = json!({"key": "value", "custom": "preserve-me"});
        seed_provider(&db, "test-id", "claude", original.clone());

        // Merge the same JSON — no actual change should occur
        db.update_provider_settings_config(
            "claude",
            "test-id",
            &json!({"key": "value"}),
            false,
        )
        .expect("update should succeed");

        let after = db
            .get_provider_by_id("test-id", "claude")
            .expect("query")
            .expect("exists");
        assert_eq!(after.settings_config, original, "identical merge must not mutate data");
    }

    #[test]
    fn empty_object_is_valid() {
        let db = Database::memory().expect("memory db");
        seed_provider(&db, "test-id", "claude", json!({"old": true}));

        let provider = db
            .get_provider_by_id("test-id", "claude")
            .expect("query")
            .expect("exists");

        let new_value =
            validate_edited_json("{}", &provider, &AppType::Claude).expect("{} is valid");
        assert!(new_value.is_object());
        assert!(new_value.as_object().unwrap().is_empty());
    }
}
