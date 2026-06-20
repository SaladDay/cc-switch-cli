use cc_switch_lib::{AppError, Database, Provider};
use serde_json::json;

fn save_queue_provider(db: &Database, app_type: &str, id: &str) -> Result<(), AppError> {
    let provider = Provider::with_id(
        id.to_string(),
        id.to_string(),
        json!({"env": {"BASE_URL": "https://example.com"}}),
        None,
    );
    db.save_provider(app_type, &provider)?;
    db.add_to_failover_queue(app_type, id)?;
    Ok(())
}

#[tokio::test]
async fn default_cost_multiplier_round_trips() -> Result<(), AppError> {
    let db = Database::memory()?;

    let default = db.get_default_cost_multiplier("claude").await?;
    assert_eq!(default, "1");

    db.set_default_cost_multiplier("claude", "1.5").await?;
    let updated = db.get_default_cost_multiplier("claude").await?;
    assert_eq!(updated, "1.5");

    Ok(())
}

#[tokio::test]
async fn default_cost_multiplier_rejects_non_numeric_values() -> Result<(), AppError> {
    let db = Database::memory()?;

    let err = db
        .set_default_cost_multiplier("claude", "not-a-number")
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        AppError::Localized {
            key: "error.invalidMultiplier",
            ..
        }
    ));

    Ok(())
}

#[tokio::test]
async fn pricing_model_source_round_trips_and_rejects_unknown_values() -> Result<(), AppError> {
    let db = Database::memory()?;

    let default = db.get_pricing_model_source("claude").await?;
    assert_eq!(default, "response");

    db.set_pricing_model_source("claude", "request").await?;
    let updated = db.get_pricing_model_source("claude").await?;
    assert_eq!(updated, "request");

    let err = db
        .set_pricing_model_source("claude", "invalid")
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        AppError::Localized {
            key: "error.invalidPricingMode",
            ..
        }
    ));

    Ok(())
}

#[tokio::test]
async fn failover_live_snapshots_round_trip_and_delete() -> Result<(), AppError> {
    let db = Database::memory()?;
    save_queue_provider(&db, "claude", "provider-a")?;
    save_queue_provider(&db, "claude", "provider-b")?;

    db.save_failover_live_snapshot("claude", "provider-a", r#"{"env":{"TOKEN":"a"}}"#)
        .await?;
    db.save_failover_live_snapshot("claude", "provider-b", r#"{"env":{"TOKEN":"b"}}"#)
        .await?;

    let snapshot = db
        .get_failover_live_snapshot("claude", "provider-a")
        .await?
        .expect("provider-a snapshot");
    assert_eq!(snapshot.app_type, "claude");
    assert_eq!(snapshot.provider_id, "provider-a");
    assert_eq!(snapshot.config_json, r#"{"env":{"TOKEN":"a"}}"#);
    assert!(!snapshot.generated_at.is_empty());

    let listed = db.list_failover_live_snapshots("claude").await?;
    assert_eq!(listed.len(), 2);

    db.delete_failover_live_snapshot("claude", "provider-a")
        .await?;
    assert!(db
        .get_failover_live_snapshot("claude", "provider-a")
        .await?
        .is_none());
    assert_eq!(db.list_failover_live_snapshots("claude").await?.len(), 1);

    db.delete_failover_live_snapshots_for_app("claude").await?;
    assert!(db.list_failover_live_snapshots("claude").await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn delete_all_failover_live_snapshots_clears_every_app() -> Result<(), AppError> {
    let db = Database::memory()?;
    save_queue_provider(&db, "claude", "claude-provider")?;
    save_queue_provider(&db, "codex", "codex-provider")?;
    db.save_failover_live_snapshot("claude", "claude-provider", "{}")
        .await?;
    db.save_failover_live_snapshot("codex", "codex-provider", "{}")
        .await?;

    db.delete_all_failover_live_snapshots().await?;

    assert!(db.list_failover_live_snapshots("claude").await?.is_empty());
    assert!(db.list_failover_live_snapshots("codex").await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn failover_live_snapshots_are_deleted_with_provider() -> Result<(), AppError> {
    let db = Database::memory()?;
    save_queue_provider(&db, "claude", "provider-a")?;
    db.save_failover_live_snapshot("claude", "provider-a", "{}")
        .await?;

    db.delete_provider("claude", "provider-a")?;

    assert!(db
        .get_failover_live_snapshot("claude", "provider-a")
        .await?
        .is_none());
    Ok(())
}

#[tokio::test]
async fn clear_auto_failover_for_supported_apps_disables_failover_flags() -> Result<(), AppError> {
    let db = Database::memory()?;
    save_queue_provider(&db, "claude", "claude-p1")?;
    save_queue_provider(&db, "codex", "codex-p1")?;
    save_queue_provider(&db, "gemini", "gemini-p1")?;
    db.set_proxy_flags_sync("claude", true, true)?;
    db.set_proxy_flags_sync("codex", true, true)?;
    db.set_proxy_flags_sync("gemini", true, true)?;

    let cleared = db.clear_auto_failover_for_supported_apps().await?;

    assert_eq!(cleared, 3);
    assert_eq!(db.get_proxy_flags_sync("claude"), (true, false));
    assert_eq!(db.get_proxy_flags_sync("codex"), (true, false));
    assert_eq!(db.get_proxy_flags_sync("gemini"), (true, false));
    Ok(())
}

#[tokio::test]
async fn disabling_global_proxy_config_clears_supported_failover_rows() -> Result<(), AppError> {
    let db = Database::memory()?;
    save_queue_provider(&db, "claude", "claude-p1")?;
    save_queue_provider(&db, "codex", "codex-p1")?;
    save_queue_provider(&db, "gemini", "gemini-p1")?;
    db.set_proxy_flags_sync("claude", true, true)?;
    db.set_proxy_flags_sync("codex", true, true)?;
    db.set_proxy_flags_sync("gemini", true, true)?;

    let mut config = db.get_global_proxy_config().await?;
    config.proxy_enabled = false;
    db.update_global_proxy_config(config).await?;

    assert_eq!(db.get_proxy_flags_sync("claude"), (true, false));
    assert_eq!(db.get_proxy_flags_sync("codex"), (true, false));
    assert_eq!(db.get_proxy_flags_sync("gemini"), (true, false));
    Ok(())
}

#[test]
fn app_proxy_preferred_ports_round_trip() -> Result<(), AppError> {
    let db = Database::memory()?;

    db.set_app_proxy_preferred_port("codex", 17022)?;
    db.set_app_proxy_preferred_port("gemini", 17023)?;

    assert_eq!(db.get_app_proxy_preferred_port("codex")?, 17022);
    assert_eq!(db.get_app_proxy_preferred_port("gemini")?, 17023);

    Ok(())
}

#[tokio::test]
async fn app_preferred_port_falls_back_to_legacy_proxy_config() -> Result<(), AppError> {
    let db = Database::memory()?;
    let mut config = db.get_proxy_config().await?;
    config.listen_port = 17021;
    db.update_proxy_config(config).await?;

    assert_eq!(db.get_app_proxy_preferred_port("claude")?, 17021);

    db.set_app_proxy_preferred_port("claude", 17022)?;
    assert_eq!(db.get_app_proxy_preferred_port("claude")?, 17022);
    Ok(())
}

#[test]
fn set_proxy_flags_sync_masks_failover_without_takeover() -> Result<(), AppError> {
    let db = Database::memory()?;

    db.set_proxy_flags_sync("claude", false, true)?;

    assert_eq!(db.get_proxy_flags_sync("claude"), (false, false));
    Ok(())
}

#[test]
fn set_proxy_flags_sync_masks_failover_with_empty_queue() -> Result<(), AppError> {
    let db = Database::memory()?;

    db.set_proxy_flags_sync("claude", true, true)?;

    assert_eq!(db.get_proxy_flags_sync("claude"), (true, false));
    Ok(())
}

#[test]
fn set_proxy_flags_sync_preserves_failover_with_non_empty_queue() -> Result<(), AppError> {
    let db = Database::memory()?;
    save_queue_provider(&db, "claude", "claude-p1")?;

    db.set_proxy_flags_sync("claude", true, true)?;

    assert_eq!(db.get_proxy_flags_sync("claude"), (true, true));
    Ok(())
}

#[tokio::test]
async fn update_proxy_config_for_app_masks_failover_without_takeover() -> Result<(), AppError> {
    let db = Database::memory()?;
    let mut config = db.get_proxy_config_for_app("claude").await?;
    config.enabled = false;
    config.auto_failover_enabled = true;

    db.update_proxy_config_for_app(config).await?;

    assert_eq!(db.get_proxy_flags_sync("claude"), (false, false));
    Ok(())
}

#[tokio::test]
async fn update_proxy_config_for_app_masks_failover_with_empty_queue() -> Result<(), AppError> {
    let db = Database::memory()?;
    let mut config = db.get_proxy_config_for_app("claude").await?;
    config.enabled = true;
    config.auto_failover_enabled = true;

    db.update_proxy_config_for_app(config).await?;

    assert_eq!(db.get_proxy_flags_sync("claude"), (true, false));
    Ok(())
}

#[tokio::test]
async fn update_proxy_config_for_app_preserves_failover_with_non_empty_queue(
) -> Result<(), AppError> {
    let db = Database::memory()?;
    save_queue_provider(&db, "claude", "claude-p1")?;
    let mut config = db.get_proxy_config_for_app("claude").await?;
    config.enabled = true;
    config.auto_failover_enabled = true;

    db.update_proxy_config_for_app(config).await?;

    assert_eq!(db.get_proxy_flags_sync("claude"), (true, true));
    Ok(())
}
