use super::*;
use crate::{database::Database, proxy::circuit_breaker::CircuitBreakerConfig};
use serde_json::json;
use serial_test::serial;
use std::{env, sync::Arc};
use tempfile::TempDir;

struct TempHome {
    #[allow(dead_code)]
    dir: TempDir,
    original_home: Option<String>,
    original_userprofile: Option<String>,
    original_config_dir: Option<String>,
}

impl TempHome {
    fn new() -> Self {
        let dir = TempDir::new().expect("failed to create temp home");
        let original_home = env::var("HOME").ok();
        let original_userprofile = env::var("USERPROFILE").ok();
        let original_config_dir = env::var("CC_SWITCH_CONFIG_DIR").ok();

        env::set_var("HOME", dir.path());
        env::set_var("USERPROFILE", dir.path());
        env::set_var("CC_SWITCH_CONFIG_DIR", dir.path().join(".cc-switch"));
        crate::settings::reload_test_settings();

        Self {
            dir,
            original_home,
            original_userprofile,
            original_config_dir,
        }
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        match &self.original_home {
            Some(value) => env::set_var("HOME", value),
            None => env::remove_var("HOME"),
        }

        match &self.original_userprofile {
            Some(value) => env::set_var("USERPROFILE", value),
            None => env::remove_var("USERPROFILE"),
        }

        match &self.original_config_dir {
            Some(value) => env::set_var("CC_SWITCH_CONFIG_DIR", value),
            None => env::remove_var("CC_SWITCH_CONFIG_DIR"),
        }

        crate::settings::reload_test_settings();
    }
}

#[tokio::test]
#[serial(home_settings)]
async fn test_provider_router_creation() {
    let _home = TempHome::new();
    let db = Arc::new(Database::memory().unwrap());
    let router = ProviderRouter::new(db);

    let breaker = router.get_or_create_circuit_breaker("claude:test").await;
    assert!(breaker.allow_request().await.allowed);
}

#[tokio::test]
#[serial(home_settings)]
async fn test_failover_disabled_uses_current_provider() {
    let _home = TempHome::new();
    let db = Arc::new(Database::memory().unwrap());

    let provider_a = Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
    let provider_b = Provider::with_id("b".to_string(), "Provider B".to_string(), json!({}), None);

    db.save_provider("claude", &provider_a).unwrap();
    db.save_provider("claude", &provider_b).unwrap();
    db.set_current_provider("claude", "a").unwrap();
    db.add_to_failover_queue("claude", "b").unwrap();

    let router = ProviderRouter::new(db.clone());
    let providers = router.select_providers("claude").await.unwrap();

    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].id, "a");
}

#[tokio::test]
#[serial(home_settings)]
async fn test_failover_disabled_prefers_effective_current_provider_from_settings() {
    let _home = TempHome::new();
    let db = Arc::new(Database::memory().unwrap());

    let provider_a = Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
    let provider_b = Provider::with_id("b".to_string(), "Provider B".to_string(), json!({}), None);

    db.save_provider("claude", &provider_a).unwrap();
    db.save_provider("claude", &provider_b).unwrap();
    db.set_current_provider("claude", "a").unwrap();
    crate::settings::set_current_provider(&crate::app_config::AppType::Claude, Some("b")).unwrap();

    let router = ProviderRouter::new(db.clone());
    let providers = router.select_providers("claude").await.unwrap();

    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].id, "b");
}

#[tokio::test]
#[serial(home_settings)]
async fn test_failover_disabled_reloads_settings_for_long_lived_router() {
    let _home = TempHome::new();
    let db = Arc::new(Database::memory().unwrap());

    let provider_a = Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
    let provider_b = Provider::with_id("b".to_string(), "Provider B".to_string(), json!({}), None);

    db.save_provider("claude", &provider_a).unwrap();
    db.save_provider("claude", &provider_b).unwrap();
    db.set_current_provider("claude", "a").unwrap();
    crate::settings::set_current_provider(&crate::app_config::AppType::Claude, Some("a")).unwrap();

    let router = ProviderRouter::new(db.clone());
    let first = router.select_providers("claude").await.unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].id, "a");

    crate::settings::set_current_provider(&crate::app_config::AppType::Claude, Some("b")).unwrap();

    let second = router.select_providers("claude").await.unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(
        second[0].id, "b",
        "a long-lived proxy router should observe current provider changes without requiring a process restart"
    );
}

#[tokio::test]
#[serial(home_settings)]
async fn test_failover_enabled_uses_queue_order_ignoring_current() {
    let _home = TempHome::new();
    let db = Arc::new(Database::memory().unwrap());

    let mut provider_a =
        Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
    provider_a.sort_index = Some(2);
    let mut provider_b =
        Provider::with_id("b".to_string(), "Provider B".to_string(), json!({}), None);
    provider_b.sort_index = Some(1);

    db.save_provider("claude", &provider_a).unwrap();
    db.save_provider("claude", &provider_b).unwrap();
    db.set_current_provider("claude", "a").unwrap();
    db.add_to_failover_queue("claude", "b").unwrap();
    db.add_to_failover_queue("claude", "a").unwrap();

    let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
    config.enabled = true;
    config.auto_failover_enabled = true;
    db.update_proxy_config_for_app(config).await.unwrap();

    let router = ProviderRouter::new(db.clone());
    let providers = router.select_providers("claude").await.unwrap();

    assert_eq!(providers.len(), 2);
    assert_eq!(providers[0].id, "b");
    assert_eq!(providers[1].id, "a");
}

#[tokio::test]
#[serial(home_settings)]
async fn test_failover_enabled_without_queue_returns_no_providers_configured() {
    let _home = TempHome::new();
    let db = Arc::new(Database::memory().unwrap());

    let provider = Provider::with_id(
        "codex-current".to_string(),
        "Codex Current".to_string(),
        json!({}),
        None,
    );

    db.save_provider("codex", &provider).unwrap();
    db.set_current_provider("codex", "codex-current").unwrap();

    {
        let conn = db.conn.lock().expect("lock db");
        conn.execute(
            "UPDATE proxy_config
             SET enabled = 1, auto_failover_enabled = 1
             WHERE app_type = 'codex'",
            [],
        )
        .expect("seed legacy empty failover state");
    }

    let router = ProviderRouter::new(db.clone());
    let error = router
        .select_providers("codex")
        .await
        .expect_err("empty failover queue should no longer fall back to current provider");

    assert!(matches!(error, ProxyError::NoProvidersConfigured));
}

#[tokio::test]
#[serial(home_settings)]
async fn test_failover_enabled_single_open_queued_provider_does_not_use_non_queued_current() {
    let _home = TempHome::new();
    let db = Arc::new(Database::memory().unwrap());

    db.update_circuit_breaker_config(&CircuitBreakerConfig {
        failure_threshold: 1,
        timeout_seconds: 3600,
        ..Default::default()
    })
    .await
    .unwrap();

    let queued = Provider::with_id("queued".to_string(), "Queued".to_string(), json!({}), None);
    let current = Provider::with_id(
        "current".to_string(),
        "Current".to_string(),
        json!({}),
        None,
    );

    db.save_provider("claude", &queued).unwrap();
    db.save_provider("claude", &current).unwrap();
    db.set_current_provider("claude", "current").unwrap();
    db.add_to_failover_queue("claude", "queued").unwrap();

    let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
    config.enabled = true;
    config.auto_failover_enabled = true;
    db.update_proxy_config_for_app(config).await.unwrap();

    let router = ProviderRouter::new(db.clone());
    router
        .record_result("queued", "claude", false, false, Some("fail".to_string()))
        .await
        .unwrap();

    let error = router
        .select_providers("claude")
        .await
        .expect_err("auto failover should not select a non-queued current provider");

    assert!(matches!(error, ProxyError::AllProvidersCircuitOpen));
}

#[tokio::test]
#[serial(home_settings)]
async fn test_select_providers_does_not_consume_half_open_permit() {
    let _home = TempHome::new();
    let db = Arc::new(Database::memory().unwrap());

    db.update_circuit_breaker_config(&CircuitBreakerConfig {
        failure_threshold: 1,
        timeout_seconds: 0,
        ..Default::default()
    })
    .await
    .unwrap();

    let provider_a = Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
    let provider_b = Provider::with_id("b".to_string(), "Provider B".to_string(), json!({}), None);

    db.save_provider("claude", &provider_a).unwrap();
    db.save_provider("claude", &provider_b).unwrap();
    db.add_to_failover_queue("claude", "a").unwrap();
    db.add_to_failover_queue("claude", "b").unwrap();

    let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
    config.enabled = true;
    config.auto_failover_enabled = true;
    db.update_proxy_config_for_app(config).await.unwrap();

    let router = ProviderRouter::new(db.clone());

    router
        .record_result("b", "claude", false, false, Some("fail".to_string()))
        .await
        .unwrap();

    let providers = router.select_providers("claude").await.unwrap();
    assert_eq!(providers.len(), 2);

    assert!(router.allow_provider_request("b", "claude").await.allowed);
}

#[tokio::test]
#[serial(home_settings)]
async fn test_release_permit_neutral_frees_half_open_slot() {
    let _home = TempHome::new();
    let db = Arc::new(Database::memory().unwrap());

    db.update_circuit_breaker_config(&CircuitBreakerConfig {
        failure_threshold: 1,
        timeout_seconds: 0,
        ..Default::default()
    })
    .await
    .unwrap();

    let provider_a = Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
    db.save_provider("claude", &provider_a).unwrap();
    db.add_to_failover_queue("claude", "a").unwrap();

    let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
    config.enabled = true;
    config.auto_failover_enabled = true;
    db.update_proxy_config_for_app(config).await.unwrap();

    let router = ProviderRouter::new(db.clone());

    router
        .record_result("a", "claude", false, false, Some("fail".to_string()))
        .await
        .unwrap();

    let first = router.allow_provider_request("a", "claude").await;
    assert!(first.allowed);
    assert!(first.used_half_open_permit);

    let second = router.allow_provider_request("a", "claude").await;
    assert!(!second.allowed);

    router
        .release_permit_neutral("a", "claude", first.used_half_open_permit)
        .await;

    let third = router.allow_provider_request("a", "claude").await;
    assert!(third.allowed);
    assert!(third.used_half_open_permit);
}

#[tokio::test]
#[serial(home_settings)]
async fn test_record_result_uses_app_failure_threshold_for_health_updates() {
    let _home = TempHome::new();
    let db = Arc::new(Database::memory().unwrap());

    let provider = Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
    db.save_provider("claude", &provider).unwrap();

    let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
    config.circuit_failure_threshold = 2;
    db.update_proxy_config_for_app(config).await.unwrap();

    let router = ProviderRouter::new(db.clone());

    router
        .record_result("a", "claude", false, false, Some("fail-1".to_string()))
        .await
        .unwrap();
    let first_health = db.get_provider_health("a", "claude").await.unwrap();
    assert!(first_health.is_healthy);
    assert_eq!(first_health.consecutive_failures, 1);

    router
        .record_result("a", "claude", false, false, Some("fail-2".to_string()))
        .await
        .unwrap();
    let second_health = db.get_provider_health("a", "claude").await.unwrap();
    assert!(!second_health.is_healthy);
    assert_eq!(second_health.consecutive_failures, 2);
    assert_eq!(second_health.last_error.as_deref(), Some("fail-2"));
}

fn seed_route_members(db: &Database, members: &[(&str, u32, bool)]) {
    for (id, _, _) in members {
        let provider = Provider::with_id((*id).to_string(), (*id).to_string(), json!({}), None);
        db.save_provider("claude", &provider).unwrap();
    }
    let group = crate::database::RouteModeGroup {
        enabled: false,
        members: members
            .iter()
            .map(|(id, weight, enabled)| {
                let mut member = crate::database::RouteModeMember::new(*id, *weight);
                member.enabled = *enabled;
                member
            })
            .collect(),
    };
    db.save_route_mode_group("claude", &group).unwrap();
    db.set_route_mode_enabled("claude", true).unwrap();
}

#[tokio::test]
#[serial(home_settings)]
async fn test_route_mode_uses_members_and_ignores_failover_queue() {
    let _home = TempHome::new();
    let db = Arc::new(Database::memory().unwrap());

    let queued = Provider::with_id("c".to_string(), "Queued".to_string(), json!({}), None);
    db.save_provider("claude", &queued).unwrap();
    db.set_current_provider("claude", "c").unwrap();
    db.add_to_failover_queue("claude", "c").unwrap();

    seed_route_members(&db, &[("a", 3, true), ("b", 1, true)]);

    let router = ProviderRouter::new(db.clone());
    let mut a_count = 0;
    let mut b_count = 0;
    for _ in 0..8 {
        let providers = router.select_providers("claude").await.unwrap();
        assert_eq!(providers.len(), 2, "only route members are selected");
        assert!(
            providers.iter().all(|provider| provider.id != "c"),
            "failover-queued non-member must never be selected"
        );
        if providers[0].id == "a" {
            a_count += 1;
        } else {
            b_count += 1;
        }
    }

    assert_eq!((a_count, b_count), (6, 2), "3:1 weights over 8 selects");
}

#[tokio::test]
#[serial(home_settings)]
async fn test_route_mode_drops_disabled_and_dangling_members() {
    let _home = TempHome::new();
    let db = Arc::new(Database::memory().unwrap());

    seed_route_members(&db, &[("a", 1, true), ("b", 1, false)]);
    // Out-of-band write simulating a provider deleted after the group was
    // saved: the dangling "ghost" member must be dropped at selection time
    db.set_setting(
        "route_mode_claude",
        &json!({
            "enabled": true,
            "members": [
                {"providerId": "a", "weight": 1, "enabled": true, "modelMappings": {}},
                {"providerId": "ghost", "weight": 1, "enabled": true, "modelMappings": {}}
            ]
        })
        .to_string(),
    )
    .unwrap();

    let router = ProviderRouter::new(db.clone());
    let providers = router.select_providers("claude").await.unwrap();

    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].id, "a");
}

#[tokio::test]
#[serial(home_settings)]
async fn test_route_mode_all_breakers_open_returns_circuit_open_error() {
    let _home = TempHome::new();
    let db = Arc::new(Database::memory().unwrap());

    db.update_circuit_breaker_config(&CircuitBreakerConfig {
        failure_threshold: 1,
        timeout_seconds: 3600,
        ..Default::default()
    })
    .await
    .unwrap();

    seed_route_members(&db, &[("a", 1, true), ("b", 1, true)]);

    let router = ProviderRouter::new(db.clone());
    router
        .record_result("a", "claude", false, false, Some("fail".to_string()))
        .await
        .unwrap();
    router
        .record_result("b", "claude", false, false, Some("fail".to_string()))
        .await
        .unwrap();

    let error = router
        .select_providers("claude")
        .await
        .expect_err("all breakers open must surface the circuit-open error");

    assert!(matches!(error, ProxyError::AllProvidersCircuitOpen));
}

#[tokio::test]
#[serial(home_settings)]
async fn test_route_mode_without_enabled_members_returns_no_providers() {
    let _home = TempHome::new();
    let db = Arc::new(Database::memory().unwrap());

    seed_route_members(&db, &[]);

    let router = ProviderRouter::new(db.clone());
    let error = router
        .select_providers("claude")
        .await
        .expect_err("empty route group must surface no-providers");

    assert!(matches!(error, ProxyError::NoProvidersConfigured));
}

#[tokio::test]
#[serial(home_settings)]
async fn test_route_mode_winner_leads_and_rest_follow_in_stored_order() {
    let _home = TempHome::new();
    let db = Arc::new(Database::memory().unwrap());

    seed_route_members(&db, &[("a", 1, true), ("b", 1, true)]);

    let router = ProviderRouter::new(db.clone());
    let first = router.select_providers("claude").await.unwrap();
    assert_eq!([first[0].id.as_str(), first[1].id.as_str()], ["a", "b"]);

    let second = router.select_providers("claude").await.unwrap();
    assert_eq!([second[0].id.as_str(), second[1].id.as_str()], ["b", "a"]);
}

#[tokio::test]
#[serial(home_settings)]
async fn test_route_mode_takes_priority_when_both_modes_enabled() {
    let _home = TempHome::new();
    let db = Arc::new(Database::memory().unwrap());

    let provider_a = Provider::with_id("a".to_string(), "A".to_string(), json!({}), None);
    let provider_b = Provider::with_id("b".to_string(), "B".to_string(), json!({}), None);
    db.save_provider("claude", &provider_a).unwrap();
    db.save_provider("claude", &provider_b).unwrap();
    db.set_current_provider("claude", "a").unwrap();
    db.add_to_failover_queue("claude", "a").unwrap();

    seed_route_members(&db, &[("a", 1, true), ("b", 1, true)]);

    let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
    config.enabled = true;
    config.auto_failover_enabled = true;
    db.update_proxy_config_for_app(config).await.unwrap();

    let router = ProviderRouter::new(db.clone());
    let providers = router.select_providers("claude").await.unwrap();

    // Legacy failover with this queue would return only "a"; the route
    // branch must win and return both members with the winner first.
    assert_eq!(providers.len(), 2);
    assert_eq!(providers[0].id, "a");
    assert_eq!(providers[1].id, "b");
}
