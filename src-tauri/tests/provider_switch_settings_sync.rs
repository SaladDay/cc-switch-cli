use serde_json::json;

use cc_switch_lib::{
    update_settings, AppSettings, AppType, MultiAppConfig, Provider, ProviderService,
};

#[path = "support.rs"]
mod support;
use support::{ensure_test_home, lock_test_mutex, reset_test_fs, state_from_config};

fn codex_provider(id: &str, api_key: &str, model_provider: &str, base_url: &str) -> Provider {
    Provider::with_id(
        id.to_string(),
        id.to_string(),
        json!({
            "auth": { "OPENAI_API_KEY": api_key },
            "config": format!(
                "model_provider = \"{model_provider}\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.{model_provider}]\nbase_url = \"{base_url}\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
            )
        }),
        None,
    )
}

#[test]
fn switch_non_additive_updates_local_settings_current_provider() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let settings = AppSettings {
        current_provider_codex: Some("old-provider".to_string()),
        ..Default::default()
    };
    update_settings(settings).expect("seed local settings current provider");

    let mut config = MultiAppConfig::default();
    let manager = config
        .get_manager_mut(&AppType::Codex)
        .expect("codex manager");
    manager.current = "old-provider".to_string();
    manager.providers.insert(
        "old-provider".to_string(),
        codex_provider(
            "old-provider",
            "old-key",
            "old-provider",
            "https://old.example/v1",
        ),
    );
    manager.providers.insert(
        "new-provider".to_string(),
        codex_provider(
            "new-provider",
            "new-key",
            "new-provider",
            "https://new.example/v1",
        ),
    );

    let state = state_from_config(config);

    ProviderService::switch(&state, AppType::Codex, "new-provider")
        .expect("switch provider should succeed");

    let persisted = AppSettings::load();
    assert_eq!(
        persisted.current_provider_codex.as_deref(),
        Some("new-provider")
    );
    assert_eq!(
        ProviderService::current(&state, AppType::Codex).expect("resolve current provider"),
        "new-provider"
    );
}
