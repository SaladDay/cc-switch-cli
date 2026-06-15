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

// Route priority uses lower numbers as higher priority. Manual provider
// selection outranks normal automatic routes (default 0), while an explicitly
// higher-priority route (< -1) can still override it.
const MANUAL_PROVIDER_PRIORITY: i32 = -1;

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
    /// Returns the matched (route_id, Provider) if found, or None if no route matches.
    pub async fn match_route(
        &self,
        app_type: &str,
        model: &str,
    ) -> Result<Option<(String, Provider)>, ProxyError> {
        self.match_route_internal(app_type, model, None).await
    }

    pub async fn match_route_respecting_manual_provider(
        &self,
        app_type: &str,
        model: &str,
        manual_provider: Option<&Provider>,
    ) -> Result<Option<(String, Provider)>, ProxyError> {
        self.match_route_internal(app_type, model, manual_provider)
            .await
    }

    async fn match_route_internal(
        &self,
        app_type: &str,
        model: &str,
        manual_provider: Option<&Provider>,
    ) -> Result<Option<(String, Provider)>, ProxyError> {
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
            if should_skip_route_for_manual_provider(route.priority, manual_provider) {
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
                let provider_opt = self
                    .db
                    .get_provider_by_id(&route.provider_id, app_type)
                    .map_err(|e| ProxyError::DatabaseError(format!("get_provider_by_id: {e}")))?;
                let Some(provider) = provider_opt else {
                    log::warn!(
                        "model route matched but provider '{}' not found for app '{}' (route={}, pattern={})",
                        route.provider_id, app_type, route.id, route.pattern
                    );
                    continue;
                };
                // 记录命中（异步 + spawn_blocking 避免阻塞）
                let db = self.db.clone();
                let route_id = route.id.clone();
                let model_str = model.to_string();
                let pattern = route.pattern.clone();
                let provider_name = provider.name.clone();
                let provider_id = provider.id.clone();
                let app_type_owned = app_type.to_string();
                tokio::task::spawn_blocking(move || {
                    if let Err(e) = db.record_model_route_hit(&route_id) {
                        log::warn!("failed to record model_route hit: {e}");
                    } else {
                        log::info!(
                            "model route matched: app={app_type_owned}, model={model_str}, pattern={pattern} → provider={provider_name} (id={provider_id})"
                        );
                    }
                });
                return Ok(Some((route.id, provider)));
            }
        }

        Ok(None)
    }
}

fn should_skip_route_for_manual_provider(
    route_priority: i32,
    manual_provider: Option<&Provider>,
) -> bool {
    manual_provider.is_some() && route_priority >= MANUAL_PROVIDER_PRIORITY
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

    // Split on *, escape each segment, join with .* and anchor at the start.
    // ^ prevents substring matches (e.g. "claude-*" matching "xclaude-opus").
    // Patterns that do NOT end with '*' are also anchored at the end ($): a
    // suffix rule like "*-4-5" then matches only ids ending in "-4-5" and not
    // "claude-haiku-4-55". Patterns ending in '*' (e.g. "claude-*", "sonnet*")
    // stay open-ended prefix matches; use "*sonnet*" to match a substring.
    let ends_with_wild = pattern.ends_with('*');
    let segments: Vec<&str> = pattern.split('*').collect();
    let mut regex_str = String::from("(?i)^");
    for (i, segment) in segments.iter().enumerate() {
        if i > 0 {
            regex_str.push_str(".*");
        }
        regex_str.push_str(&regex::escape(segment));
    }
    if !ends_with_wild {
        regex_str.push('$');
    }

    Regex::new(&regex_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_route::ModelRoute;

    fn seed_provider(db: &Database, app_type: &str, id: &str) {
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
            id: String::new(),
            app_type: app_type.into(),
            pattern: pattern.into(),
            provider_id: provider_id.into(),
            priority,
            enabled,
            hit_count: 0,
            last_hit_at: None,
            created_at: None,
            updated_at: None,
        }
    }

    fn manual_provider(id: &str) -> Provider {
        Provider {
            id: id.to_string(),
            name: id.to_string(),
            settings_config: serde_json::json!({}),
            website_url: None,
            category: None,
            created_at: None,
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
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
        // 锚定保证：前缀匹配，不可中间包含
        assert!(!re.is_match("xclaude-opus"));
    }

    #[test]
    fn compile_pattern_star_middle_anchored() {
        // *sonnet* 加 ^ 锚定后，必须从开头匹配，但 .* 仍允许中间任意内容
        let re = compile_pattern("*sonnet*").expect("compile *sonnet*");
        assert!(re.is_match("sonnet"));
        assert!(re.is_match("claude-sonnet-4-6"));
        assert!(re.is_match("claude-sonnet"));
        // 包含 "sonnet" 的都匹配（.*sonnet.* 语义）
        assert!(re.is_match("claude- haikuxxsonnetyy"));
        assert!(!re.is_match("claude-haiku-4-6"));
    }

    #[test]
    fn compile_pattern_prefix_anchor_prevents_substring() {
        // claude-* 加 ^ 后，不再匹配 xclaude-opus
        let re = compile_pattern("claude-*").expect("compile claude-*");
        assert!(re.is_match("claude-opus-4-8"));
        assert!(re.is_match("claude-"));
        assert!(!re.is_match("xclaude-opus"));
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
        assert_eq!(result.unwrap().1.id, "prov-sonnet");
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
    async fn test_match_route_star_suffix_rejects_partial() {
        // Regression (Codex P2): "*-4-5" must not match "claude-haiku-4-55".
        // Non-trailing-* suffix rules are anchored at the end, so a longer id
        // that merely contains "-4-5" as a substring is not matched.
        let db = Arc::new(Database::memory().expect("create memory database"));
        seed_provider(&db, "claude", "prov-45");

        let route = test_route("claude", "*-4-5", "prov-45", 1, true);
        db.create_model_route(&route).expect("create route");

        let router = ModelRouter::new(db);
        assert!(router
            .match_route("claude", "claude-haiku-4-55")
            .await
            .expect("match_route")
            .is_none());
    }

    #[tokio::test]
    async fn test_match_route_priority() {
        let db = Arc::new(Database::memory().expect("create memory database"));
        seed_provider(&db, "claude", "prov-high");
        seed_provider(&db, "claude", "prov-low");

        // Higher priority (lower number) should win
        let route_high = test_route("claude", "*sonnet*", "prov-high", 1, true);
        let route_low = test_route("claude", "*sonnet*", "prov-low", 10, true);
        db.create_model_route(&route_high)
            .expect("create high-priority route");
        db.create_model_route(&route_low)
            .expect("create low-priority route");

        let router = ModelRouter::new(db);
        let result = router
            .match_route("claude", "claude-sonnet-4-6")
            .await
            .expect("match_route");
        assert!(result.is_some());
        assert_eq!(result.unwrap().1.id, "prov-high");
    }

    #[tokio::test]
    async fn test_match_route_disabled_skipped() {
        let db = Arc::new(Database::memory().expect("create memory database"));
        seed_provider(&db, "claude", "prov-disabled");

        let route = test_route("claude", "*sonnet*", "prov-disabled", 1, false);
        db.create_model_route(&route)
            .expect("create disabled route");

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
        assert_eq!(result.unwrap().1.id, "prov-case");
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
        assert_eq!(result.unwrap().1.id, "prov-meta");
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
                "INSERT INTO model_routes (id, app_type, pattern, provider_id, priority, enabled)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    uuid::Uuid::new_v4().to_string(),
                    "claude",
                    "*-missing",
                    "prov-missing",
                    1,
                    true
                ],
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

    #[tokio::test]
    async fn normal_route_priority_yields_to_manual_provider() {
        let db = Arc::new(Database::memory().expect("create memory database"));
        seed_provider(&db, "claude", "automatic-provider");

        let route = test_route("claude", "*", "automatic-provider", 0, true);
        db.create_model_route(&route).expect("create route");

        let router = ModelRouter::new(db);
        let manual_provider = manual_provider("manually-selected");
        let result = router
            .match_route_respecting_manual_provider(
                "claude",
                "any-request-model",
                Some(&manual_provider),
            )
            .await
            .expect("match route");

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn explicit_higher_priority_route_can_override_manual_provider() {
        let db = Arc::new(Database::memory().expect("create memory database"));
        seed_provider(&db, "claude", "explicit-route-provider");

        let route = test_route("claude", "*", "explicit-route-provider", -2, true);
        db.create_model_route(&route).expect("create route");

        let router = ModelRouter::new(db);
        let manual_provider = manual_provider("manually-selected");
        let result = router
            .match_route_respecting_manual_provider(
                "claude",
                "any-request-model",
                Some(&manual_provider),
            )
            .await
            .expect("match route")
            .expect("higher-priority route should match");

        assert_eq!(result.1.id, "explicit-route-provider");
    }
}
