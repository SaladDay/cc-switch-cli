//! Model Router — per-model provider routing engine
//!
//! When a model route matches, the request uses the route-targeted provider only (single
//! provider, no failover queue). When no model route matches, the request falls back to
//! existing ProviderRouter logic.
//!
//! Wildcard * in pattern matches zero or more characters in model name, case-insensitively.
//! Multiple matching rules resolve to the one with lowest priority number (highest priority).
//! Disabled rules (enabled=false) are never matched.

use std::sync::Arc;

use regex::Regex;

use crate::database::Database;
use crate::provider::Provider;

use super::error::ProxyError;

pub struct ModelRouter {
    db: Arc<Database>,
}

impl ModelRouter {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Match a model name against stored model routes for the given app_type.
    ///
    /// Routes are ordered by priority ASC (lowest number = highest priority).
    /// The first enabled route whose pattern matches `model` wins.
    /// Returns the matched Provider if found, or None if no route matches.
    pub async fn match_route(
        &self,
        app_type: &str,
        model: &str,
    ) -> Result<Option<Provider>, ProxyError> {
        if model.is_empty() {
            return Ok(None);
        }

        let routes = self
            .db
            .list_model_routes(app_type)
            .map_err(|e| ProxyError::DatabaseError(format!("list_model_routes: {e}")))?;

        for route in routes {
            if !route.enabled {
                continue;
            }

            let regex = match compile_pattern(&route.pattern) {
                Ok(re) => re,
                Err(_) => {
                    log::warn!(
                        "model route pattern '{}' is not a valid pattern, skipping",
                        route.pattern
                    );
                    continue;
                }
            };

            if regex.is_match(model) {
                return self
                    .db
                    .get_provider_by_id(&route.provider_id, app_type)
                    .map_err(|e| {
                        ProxyError::DatabaseError(format!("get_provider_by_id: {e}"))
                    });
            }
        }

        Ok(None)
    }
}

/// Compile a model route pattern into a case-insensitive regex.
///
/// The only special character is `*`, which becomes `.*`.
/// All other characters are treated as literals (regex meta-characters are escaped).
/// Exact patterns (no `*`) are anchored with `^...$`.
fn compile_pattern(pattern: &str) -> Result<Regex, regex::Error> {
    if !pattern.contains('*') {
        // Exact match — anchor and escape
        let escaped = regex::escape(pattern);
        return Regex::new(&format!("(?i)^{escaped}$"));
    }

    // Split on *, escape each segment, join with .*
    let segments: Vec<&str> = pattern.split('*').collect();
    let mut regex_str = String::from("(?i)");
    for (i, segment) in segments.iter().enumerate() {
        if i > 0 {
            regex_str.push_str(".*");
        }
        regex_str.push_str(&regex::escape(segment));
    }

    Regex::new(&regex_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_route::ModelRoute;

    fn seed_provider(db: &Database, app_type: &str, id: &str) {
        use std::sync::Mutex;

        // lock_conn! macro expands to a scope that uses AppError — we call the raw
        // Mutex::lock to avoid requiring an AppError import here.
        let guard = db.conn.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .execute(
                "INSERT INTO providers (id, app_type, name, settings_config, meta)
                 VALUES (?1, ?2, ?3, '{}', '{}')",
                rusqlite::params![id, app_type, id],
            )
            .expect("seed provider");
    }

    fn test_route(
        app_type: &str,
        pattern: &str,
        provider_id: &str,
        priority: i32,
        enabled: bool,
    ) -> ModelRoute {
        ModelRoute {
            id: None,
            app_type: app_type.into(),
            pattern: pattern.into(),
            provider_id: provider_id.into(),
            priority,
            enabled,
            created_at: None,
            updated_at: None,
        }
    }

    // --- Unit tests for compile_pattern ---

    #[test]
    fn compile_pattern_exact_match() {
        let re = compile_pattern("claude-sonnet-4-6").expect("compile exact pattern");
        assert!(re.is_match("claude-sonnet-4-6"));
        assert!(!re.is_match("claude-sonnet-4-55"));
        // Leading/trailing text should not match (anchored)
        assert!(!re.is_match("prefix-claude-sonnet-4-6"));
    }

    #[test]
    fn compile_pattern_star_middle() {
        let re = compile_pattern("*sonnet*").expect("compile *sonnet*");
        assert!(re.is_match("claude-sonnet-4-6"));
        assert!(re.is_match("sonnet"));
        assert!(!re.is_match("opus"));
    }

    #[test]
    fn compile_pattern_star_suffix() {
        let re = compile_pattern("claude-*").expect("compile claude-*");
        assert!(re.is_match("claude-opus-4-8"));
        assert!(!re.is_match("gemini-2.5-pro"));
    }

    #[test]
    fn compile_pattern_star_prefix() {
        let re = compile_pattern("*-4-5").expect("compile *-4-5");
        assert!(re.is_match("claude-haiku-4-5"));
        assert!(re.is_match("deepseek-4-5"));
        assert!(!re.is_match("claude-haiku-4-6"));
    }

    #[test]
    fn compile_pattern_regex_meta_chars_escaped() {
        // + is a regex quantifier — should be treated as literal
        let re = compile_pattern("gpt-4+").expect("compile gpt-4+");
        assert!(re.is_match("gpt-4+"));
        assert!(!re.is_match("gpt-4"));
        assert!(!re.is_match("gpt-4++"));
    }

    // --- Integration tests for match_route (uses in-memory DB) ---

    #[tokio::test]
    async fn test_match_route_exact_pattern() {
        let db = Arc::new(Database::memory().expect("create memory database"));
        seed_provider(&db, "claude", "prov-sonnet");

        let route = test_route("claude", "claude-sonnet-4-6", "prov-sonnet", 1, true);
        db.create_model_route(&route).expect("create route");

        let router = ModelRouter::new(db);
        let result = router
            .match_route("claude", "claude-sonnet-4-6")
            .await
            .expect("match_route");
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, "prov-sonnet");
    }

    #[tokio::test]
    async fn test_match_route_star_sonnet_star() {
        let db = Arc::new(Database::memory().expect("create memory database"));
        seed_provider(&db, "claude", "prov-sonnet");

        let route = test_route("claude", "*sonnet*", "prov-sonnet", 1, true);
        db.create_model_route(&route).expect("create route");

        let router = ModelRouter::new(db);
        assert!(router
            .match_route("claude", "claude-sonnet-4-6")
            .await
            .expect("match_route")
            .is_some());
        assert!(router
            .match_route("claude", "sonnet")
            .await
            .expect("match_route")
            .is_some());
    }

    #[tokio::test]
    async fn test_match_route_claude_star() {
        let db = Arc::new(Database::memory().expect("create memory database"));
        seed_provider(&db, "claude", "prov-claude");

        let route = test_route("claude", "claude-*", "prov-claude", 1, true);
        db.create_model_route(&route).expect("create route");

        let router = ModelRouter::new(db);
        assert!(router
            .match_route("claude", "claude-opus-4-8")
            .await
            .expect("match_route")
            .is_some());
        assert!(router
            .match_route("claude", "gemini-2.5-pro")
            .await
            .expect("match_route")
            .is_none());
    }

    #[tokio::test]
    async fn test_match_route_star_suffix() {
        let db = Arc::new(Database::memory().expect("create memory database"));
        seed_provider(&db, "claude", "prov-45");

        let route = test_route("claude", "*-4-5", "prov-45", 1, true);
        db.create_model_route(&route).expect("create route");

        let router = ModelRouter::new(db);
        assert!(router
            .match_route("claude", "claude-haiku-4-5")
            .await
            .expect("match_route")
            .is_some());
        assert!(router
            .match_route("claude", "deepseek-4-5")
            .await
            .expect("match_route")
            .is_some());
    }

    #[tokio::test]
    async fn test_match_route_priority() {
        let db = Arc::new(Database::memory().expect("create memory database"));
        seed_provider(&db, "claude", "prov-high");
        seed_provider(&db, "claude", "prov-low");

        // Higher priority (lower number) should win
        let route_high = test_route("claude", "*-sonnet", "prov-high", 1, true);
        let route_low = test_route("claude", "*-sonnet", "prov-low", 10, true);
        db.create_model_route(&route_high).expect("create high-priority route");
        db.create_model_route(&route_low).expect("create low-priority route");

        let router = ModelRouter::new(db);
        let result = router
            .match_route("claude", "claude-sonnet-4-6")
            .await
            .expect("match_route");
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, "prov-high");
    }

    #[tokio::test]
    async fn test_match_route_disabled_skipped() {
        let db = Arc::new(Database::memory().expect("create memory database"));
        seed_provider(&db, "claude", "prov-disabled");

        let route = test_route("claude", "*-sonnet", "prov-disabled", 1, false);
        db.create_model_route(&route).expect("create disabled route");

        let router = ModelRouter::new(db);
        assert!(router
            .match_route("claude", "claude-sonnet-4-6")
            .await
            .expect("match_route")
            .is_none());
    }

    #[tokio::test]
    async fn test_match_route_no_match() {
        let db = Arc::new(Database::memory().expect("create memory database"));
        seed_provider(&db, "claude", "prov-specific");

        let route = test_route("claude", "claude-*", "prov-specific", 1, true);
        db.create_model_route(&route).expect("create route");

        let router = ModelRouter::new(db);
        assert!(router
            .match_route("claude", "gemini-2.5-pro")
            .await
            .expect("match_route")
            .is_none());
    }

    #[tokio::test]
    async fn test_match_route_empty_model() {
        let db = Arc::new(Database::memory().expect("create memory database"));
        seed_provider(&db, "claude", "prov-any");

        let route = test_route("claude", "*", "prov-any", 1, true);
        db.create_model_route(&route).expect("create route");

        let router = ModelRouter::new(db);
        assert!(router
            .match_route("claude", "")
            .await
            .expect("match_route")
            .is_none());
    }

    #[tokio::test]
    async fn test_match_route_case_insensitive() {
        let db = Arc::new(Database::memory().expect("create memory database"));
        seed_provider(&db, "claude", "prov-case");

        let route = test_route("claude", "claude-sonnet-*", "prov-case", 1, true);
        db.create_model_route(&route).expect("create route");

        let router = ModelRouter::new(db);
        let result = router
            .match_route("claude", "CLAUDE-SONNET-4-6")
            .await
            .expect("match_route");
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, "prov-case");
    }

    #[tokio::test]
    async fn test_match_route_regex_meta_chars() {
        let db = Arc::new(Database::memory().expect("create memory database"));
        seed_provider(&db, "claude", "prov-meta");

        // gpt-4+ has a literal + — the pattern's + is escaped, not a regex quantifier
        let route = test_route("claude", "gpt-4+", "prov-meta", 1, true);
        db.create_model_route(&route).expect("create route");

        let router = ModelRouter::new(db);
        let result = router
            .match_route("claude", "gpt-4+")
            .await
            .expect("match_route");
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, "prov-meta");
    }

    #[tokio::test]
    async fn test_match_route_missing_provider() {
        let db = Arc::new(Database::memory().expect("create memory database"));

        // FK constraint prevents create_model_route from referencing a non-existent
        // provider. Disable foreign keys to insert a dangling route, then re-enable.
        let guard = db.conn.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .execute_batch("PRAGMA foreign_keys = OFF")
            .expect("disable foreign keys");
        guard
            .execute(
                "INSERT INTO model_routes (app_type, pattern, provider_id, priority, enabled)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params!["claude", "*-missing", "prov-missing", 1, true],
            )
            .expect("insert dangling model route");
        guard
            .execute_batch("PRAGMA foreign_keys = ON")
            .expect("re-enable foreign keys");
        drop(guard);

        let router = ModelRouter::new(db);
        let result = router
            .match_route("claude", "claude-missing")
            .await
            .expect("match_route");
        // Provider doesn't exist — get_provider_by_id returns None
        assert!(result.is_none());
    }
}
