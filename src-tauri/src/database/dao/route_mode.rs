//! Route mode DAO
//!
//! Each proxy-capable app has one implicit route group (weighted provider
//! pool); state is stored as a JSON document in the settings KV table under
//! the key `route_mode_{app}`. This fork's schema stays synced with upstream,
//! so adding tables/columns is forbidden — hence the KV storage.

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Upper bound for a single route member's weight
pub const ROUTE_MODE_MAX_WEIGHT: u32 = 1000;

/// Route mode group (one per app)
///
/// The stored order of `members` is the in-request failure skip order
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct RouteModeGroup {
    pub enabled: bool,
    pub members: Vec<RouteModeMember>,
}

/// Route mode group member
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct RouteModeMember {
    pub provider_id: String,
    /// Smooth weighted round-robin weight, 1..=1000
    pub weight: u32,
    pub enabled: bool,
    /// Exact request-model → actual-model mapping
    pub model_mappings: BTreeMap<String, String>,
}

impl RouteModeMember {
    pub fn new(provider_id: impl Into<String>, weight: u32) -> Self {
        Self {
            provider_id: provider_id.into(),
            weight,
            enabled: true,
            model_mappings: BTreeMap::new(),
        }
    }
}

fn route_mode_key(app_type: &str) -> String {
    format!("route_mode_{app_type}")
}

impl RouteModeGroup {
    /// Structural validation; does not check provider existence (providers
    /// may be deleted after the group was saved)
    fn validate_structure(&self) -> Result<(), AppError> {
        let mut seen = std::collections::HashSet::new();
        for member in &self.members {
            if member.provider_id.trim().is_empty() {
                return Err(AppError::InvalidInput(
                    "Route mode member provider id must not be empty.".to_string(),
                ));
            }
            if !seen.insert(member.provider_id.as_str()) {
                return Err(AppError::InvalidInput(format!(
                    "Duplicate route mode member: {}.",
                    member.provider_id
                )));
            }
            if member.weight == 0 || member.weight > ROUTE_MODE_MAX_WEIGHT {
                return Err(AppError::InvalidInput(format!(
                    "Route mode weight for {} must be between 1 and {ROUTE_MODE_MAX_WEIGHT}.",
                    member.provider_id
                )));
            }
            for (request_model, actual_model) in &member.model_mappings {
                if request_model.trim().is_empty() || actual_model.trim().is_empty() {
                    return Err(AppError::InvalidInput(format!(
                        "Route mode model mappings for {} must not contain empty model names.",
                        member.provider_id
                    )));
                }
            }
        }
        Ok(())
    }
}

impl Database {
    /// Read the route mode group; missing or corrupt data returns the default
    /// (disabled, fail closed)
    pub fn get_route_mode_group(&self, app_type: &str) -> Result<RouteModeGroup, AppError> {
        let Some(json) = self.get_setting(&route_mode_key(app_type))? else {
            return Ok(RouteModeGroup::default());
        };

        match serde_json::from_str::<RouteModeGroup>(&json) {
            Ok(group) => {
                // Defend against out-of-band writes (WebDAV restore, older
                // versions): structurally invalid data also means disabled
                if let Err(error) = group.validate_structure() {
                    log::warn!(
                        "invalid route mode config for {app_type}, treating as disabled: {error}"
                    );
                    return Ok(RouteModeGroup::default());
                }
                Ok(group)
            }
            Err(error) => {
                log::warn!("failed to parse route mode config for {app_type}, treating as disabled: {error}");
                Ok(RouteModeGroup::default())
            }
        }
    }

    /// Save the route mode group (validate first, then write)
    pub fn save_route_mode_group(
        &self,
        app_type: &str,
        group: &RouteModeGroup,
    ) -> Result<(), AppError> {
        group.validate_structure()?;

        let conn = lock_conn!(self.conn);
        for member in &group.members {
            let exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM providers WHERE id = ?1 AND app_type = ?2",
                    rusqlite::params![member.provider_id, app_type],
                    |row| row.get::<_, i64>(0).map(|count| count > 0),
                )
                .map_err(|e| AppError::Database(e.to_string()))?;
            if !exists {
                return Err(AppError::InvalidInput(format!(
                    "Route mode member {} does not exist for app {app_type}.",
                    member.provider_id
                )));
            }
        }

        let json = serde_json::to_string(group).map_err(|e| {
            AppError::Database(format!("failed to serialize route mode config: {e}"))
        })?;
        // The connection lock is already held here; calling set_setting would
        // re-lock the same mutex and deadlock, so reuse this connection
        // directly for the KV write
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            rusqlite::params![route_mode_key(app_type), json],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// Whether route mode is enabled
    pub fn route_mode_enabled_for_app(&self, app_type: &str) -> Result<bool, AppError> {
        Ok(self.get_route_mode_group(app_type)?.enabled)
    }

    /// Flip only the enabled flag (members preserved; precondition guards
    /// live in the service layer)
    pub fn set_route_mode_enabled(&self, app_type: &str, enabled: bool) -> Result<(), AppError> {
        let mut group = self.get_route_mode_group(app_type)?;
        group.enabled = enabled;
        self.save_route_mode_group(app_type, &group)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn provider(id: &str) -> crate::provider::Provider {
        crate::provider::Provider::with_id(
            id.to_string(),
            id.to_string(),
            json!({"env": {"BASE_URL": "https://example.com"}}),
            None,
        )
    }

    fn save_provider(db: &Database, app_type: &str, id: &str) {
        db.save_provider(app_type, &provider(id))
            .expect("save provider");
    }

    #[test]
    fn missing_group_defaults_to_disabled() {
        let db = Database::memory().expect("create memory db");

        let group = db.get_route_mode_group("claude").expect("read group");

        assert!(!group.enabled);
        assert!(group.members.is_empty());
        assert!(!db.route_mode_enabled_for_app("claude").expect("read flag"));
    }

    #[test]
    fn group_roundtrip_preserves_members() {
        let db = Database::memory().expect("create memory db");
        save_provider(&db, "claude", "p1");
        save_provider(&db, "claude", "p2");

        let mut mappings = BTreeMap::new();
        mappings.insert("m-a".to_string(), "m-b".to_string());
        let mut member = RouteModeMember::new("p2", 3);
        member.model_mappings = mappings;
        let group = RouteModeGroup {
            enabled: true,
            members: vec![RouteModeMember::new("p1", 1), member],
        };

        db.save_route_mode_group("claude", &group)
            .expect("save group");

        let loaded = db.get_route_mode_group("claude").expect("read group");
        assert_eq!(loaded, group);
        assert!(db.route_mode_enabled_for_app("claude").expect("flag"));
    }

    #[test]
    fn set_route_mode_enabled_flips_flag_only() {
        let db = Database::memory().expect("create memory db");
        save_provider(&db, "codex", "p1");
        let group = RouteModeGroup {
            enabled: false,
            members: vec![RouteModeMember::new("p1", 2)],
        };
        db.save_route_mode_group("codex", &group).expect("save");

        db.set_route_mode_enabled("codex", true).expect("enable");

        let loaded = db.get_route_mode_group("codex").expect("read");
        assert!(loaded.enabled);
        assert_eq!(loaded.members, group.members);
        assert_eq!(
            db.get_proxy_flags_sync("codex"),
            (false, false),
            "must not touch proxy_config"
        );
    }

    #[test]
    fn save_rejects_duplicate_member_ids() {
        let db = Database::memory().expect("create memory db");
        save_provider(&db, "claude", "p1");

        let group = RouteModeGroup {
            enabled: false,
            members: vec![RouteModeMember::new("p1", 1), RouteModeMember::new("p1", 2)],
        };

        let err = db.save_route_mode_group("claude", &group).unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)));
    }

    #[test]
    fn save_rejects_weight_out_of_range() {
        let db = Database::memory().expect("create memory db");
        save_provider(&db, "claude", "p1");

        for weight in [0, ROUTE_MODE_MAX_WEIGHT + 1] {
            let group = RouteModeGroup {
                enabled: false,
                members: vec![RouteModeMember::new("p1", weight)],
            };
            let err = db.save_route_mode_group("claude", &group).unwrap_err();
            assert!(matches!(err, AppError::InvalidInput(_)), "weight {weight}");
        }
    }

    #[test]
    fn save_rejects_empty_mapping_names() {
        let db = Database::memory().expect("create memory db");
        save_provider(&db, "claude", "p1");

        let mut mappings = BTreeMap::new();
        mappings.insert("  ".to_string(), "m-b".to_string());
        let mut member = RouteModeMember::new("p1", 1);
        member.model_mappings = mappings;
        let group = RouteModeGroup {
            enabled: false,
            members: vec![member],
        };

        let err = db.save_route_mode_group("claude", &group).unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)));
    }

    #[test]
    fn save_rejects_unknown_provider() {
        let db = Database::memory().expect("create memory db");

        let group = RouteModeGroup {
            enabled: false,
            members: vec![RouteModeMember::new("ghost", 1)],
        };

        let err = db.save_route_mode_group("claude", &group).unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)));
    }

    #[test]
    fn corrupt_json_fails_closed_to_disabled_default() {
        let db = Database::memory().expect("create memory db");
        db.set_setting(&route_mode_key("gemini"), "{not json")
            .expect("write corrupt json");

        let group = db.get_route_mode_group("gemini").expect("read group");

        assert!(!group.enabled);
        assert!(group.members.is_empty());
    }

    #[test]
    fn semantically_invalid_json_fails_closed_to_disabled_default() {
        let db = Database::memory().expect("create memory db");
        db.set_setting(
            &route_mode_key("gemini"),
            r#"[{"provider_id":"p1","weight":0,"enabled":true,"modelMappings":{}}]"#,
        )
        .expect("write invalid json");

        let group = db.get_route_mode_group("gemini").expect("read group");

        assert!(!group.enabled);
        assert!(group.members.is_empty());
    }
}
