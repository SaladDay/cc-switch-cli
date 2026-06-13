//! 模型路由 DAO (Model Route Data Access Object)
//!
//! 管理 model_routes 表的 CRUD 操作，为 per-model provider routing 提供持久化层。
//! 支持按 app_type 列出路由、创建/更新/删除路由、切换启用状态、记录命中统计。
//! id 使用 UUID v4 (TEXT PRIMARY KEY)，与上游 cc-switch 一致。

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::model_route::ModelRoute;

const SELECT_COLS: &str = "id, app_type, pattern, provider_id, priority, enabled, hit_count, last_hit_at, created_at, updated_at";

impl Database {
    /// 列出指定 app_type 的所有模型路由，按 priority ASC, created_at ASC 排序
    pub fn list_model_routes(&self, app_type: &str) -> Result<Vec<ModelRoute>, AppError> {
        let conn = lock_conn!(self.conn);

        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLS} FROM model_routes WHERE app_type = ?1 ORDER BY priority ASC, created_at ASC"
            ))
            .map_err(|e| AppError::Database(e.to_string()))?;

        let items = stmt
            .query_map([app_type], |row| Ok(row_to_route(row)))
            .map_err(|e| AppError::Database(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::Database(e.to_string()))?;

        Ok(items)
    }

    /// 根据 ID 获取单个模型路由
    pub fn get_model_route(&self, id: &str) -> Result<Option<ModelRoute>, AppError> {
        let conn = lock_conn!(self.conn);

        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLS} FROM model_routes WHERE id = ?1"
            ))
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut rows = stmt
            .query_map([id], |row| Ok(row_to_route(row)))
            .map_err(|e| AppError::Database(e.to_string()))?;

        rows.next()
            .transpose()
            .map_err(|e| AppError::Database(e.to_string()))
    }

    /// 创建模型路由（生成 UUID id，验证 provider_id 存在）
    pub fn create_model_route(&self, route: &ModelRoute) -> Result<ModelRoute, AppError> {
        let conn = lock_conn!(self.conn);

        let provider_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM providers WHERE id = ?1 AND app_type = ?2",
                rusqlite::params![&route.provider_id, &route.app_type],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        if !provider_exists {
            return Err(AppError::Database(format!(
                "provider '{}' not found for app '{}'",
                route.provider_id, route.app_type
            )));
        }

        let id = if route.id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            route.id.clone()
        };

        let mut stmt = conn
            .prepare(&format!(
                "INSERT INTO model_routes (id, app_type, pattern, provider_id, priority, enabled)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 RETURNING {SELECT_COLS}"
            ))
            .map_err(|e| AppError::Database(e.to_string()))?;

        stmt.query_row(
            rusqlite::params![
                &id,
                &route.app_type,
                &route.pattern,
                &route.provider_id,
                route.priority,
                route.enabled as i32,
            ],
            |row| Ok(row_to_route(row)),
        )
        .map_err(|e| AppError::Database(e.to_string()))
    }

    /// 更新模型路由
    pub fn update_model_route(&self, id: &str, route: &ModelRoute) -> Result<ModelRoute, AppError> {
        let conn = lock_conn!(self.conn);

        let current_provider: String = conn
            .query_row(
                "SELECT provider_id FROM model_routes WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        if route.provider_id != current_provider {
            let provider_exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM providers WHERE id = ?1 AND app_type = ?2",
                    rusqlite::params![&route.provider_id, &route.app_type],
                    |row| row.get(0),
                )
                .map_err(|e| AppError::Database(e.to_string()))?;

            if !provider_exists {
                return Err(AppError::Database(format!(
                    "provider '{}' not found for app '{}'",
                    route.provider_id, route.app_type
                )));
            }
        }

        let mut stmt = conn
            .prepare(&format!(
                "UPDATE model_routes SET
                     pattern = ?1, provider_id = ?2, priority = ?3, enabled = ?4,
                     updated_at = datetime('now')
                 WHERE id = ?5
                 RETURNING {SELECT_COLS}"
            ))
            .map_err(|e| AppError::Database(e.to_string()))?;

        stmt.query_row(
            rusqlite::params![
                &route.pattern,
                &route.provider_id,
                route.priority,
                route.enabled as i32,
                id,
            ],
            |row| Ok(row_to_route(row)),
        )
        .map_err(|e| AppError::Database(e.to_string()))
    }

    /// 删除模型路由
    pub fn delete_model_route(&self, id: &str) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);

        let changes = conn
            .execute("DELETE FROM model_routes WHERE id = ?1", [id])
            .map_err(|e| AppError::Database(e.to_string()))?;

        if changes == 0 {
            return Err(AppError::Database("model_route not found".to_string()));
        }

        Ok(())
    }

    /// 切换模型路由的启用状态
    pub fn toggle_model_route(&self, id: &str) -> Result<ModelRoute, AppError> {
        let conn = lock_conn!(self.conn);

        let mut stmt = conn
            .prepare(&format!(
                "UPDATE model_routes SET
                     enabled = NOT enabled,
                     updated_at = datetime('now')
                 WHERE id = ?1
                 RETURNING {SELECT_COLS}"
            ))
            .map_err(|e| AppError::Database(e.to_string()))?;

        stmt.query_row([id], |row| Ok(row_to_route(row)))
            .map_err(|e| AppError::Database(e.to_string()))
    }

    /// 记录一次命中（增加 hit_count 并更新 last_hit_at）
    /// 使用 UPDATE 而非事务，性能更好；last_hit_at 只在每次调用时更新（不频繁）
    pub fn record_model_route_hit(&self, id: &str) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);

        let changes = conn
            .execute(
                "UPDATE model_routes SET
                     hit_count = hit_count + 1,
                     last_hit_at = datetime('now')
                 WHERE id = ?1",
                [id],
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        if changes == 0 {
            return Err(AppError::Database("model_route not found".to_string()));
        }

        Ok(())
    }

    /// 获取所有启用的 model_routes（按 app_type + provider_id 聚合用于仪表盘）
    /// 返回 (app_type, provider_id, total_hits) 列表
    pub fn aggregate_route_hits_by_provider(&self) -> Result<Vec<(String, String, i64)>, AppError> {
        let conn = lock_conn!(self.conn);

        let mut stmt = conn
            .prepare(
                "SELECT app_type, provider_id, SUM(hit_count) as total
                 FROM model_routes
                 WHERE enabled = 1
                 GROUP BY app_type, provider_id
                 ORDER BY total DESC",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2)?))
            })
            .map_err(|e| AppError::Database(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::Database(e.to_string()))?;

        Ok(rows)
    }
}

fn row_to_route(row: &rusqlite::Row) -> ModelRoute {
    ModelRoute {
        id: row.get(0).expect("id"),
        app_type: row.get(1).expect("app_type"),
        pattern: row.get(2).expect("pattern"),
        provider_id: row.get(3).expect("provider_id"),
        priority: row.get(4).expect("priority"),
        enabled: row.get::<_, i32>(5).expect("enabled") != 0,
        hit_count: row.get(6).expect("hit_count"),
        last_hit_at: row.get(7).expect("last_hit_at"),
        created_at: row.get(8).expect("created_at"),
        updated_at: row.get(9).expect("updated_at"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn seed_provider(db: &Database, app_type: &str, id: &str) -> Result<(), AppError> {
        let conn = lock_conn!(db.conn);
        conn.execute(
            "INSERT INTO providers (id, app_type, name, settings_config, meta)
             VALUES (?1, ?2, ?3, '{}', '{}')",
            rusqlite::params![id, app_type, id],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    fn test_route(pattern: &str, provider_id: &str, priority: i32) -> ModelRoute {
        ModelRoute {
            id: String::new(),
            app_type: "claude".into(),
            pattern: pattern.into(),
            provider_id: provider_id.into(),
            priority,
            enabled: true,
            hit_count: 0,
            last_hit_at: None,
            created_at: None,
            updated_at: None,
        }
    }

    #[test]
    fn create_and_get_model_route_roundtrip() -> Result<(), AppError> {
        let db = Database::memory()?;
        seed_provider(&db, "claude", "test-prov")?;

        let created = db.create_model_route(&test_route("*-sonnet", "test-prov", 10))?;

        assert_eq!(created.id.len(), 36);
        assert_eq!(created.pattern, "*-sonnet");
        assert_eq!(created.provider_id, "test-prov");
        assert_eq!(created.priority, 10);
        assert!(created.enabled);
        assert_eq!(created.hit_count, 0);
        assert!(created.created_at.is_some());

        let got = db.get_model_route(&created.id)?;
        assert!(got.is_some());
        assert_eq!(got.unwrap().pattern, "*-sonnet");

        Ok(())
    }

    #[test]
    fn create_model_route_rejects_invalid_provider() -> Result<(), AppError> {
        let db = Database::memory()?;

        let result = db.create_model_route(&test_route("*-sonnet", "nonexistent", 10));
        assert!(result.is_err());

        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("provider") && msg.contains("not found"),
            "expected provider not found error, got: {msg}"
        );

        Ok(())
    }

    #[test]
    fn list_model_routes_ordered_by_priority() -> Result<(), AppError> {
        let db = Database::memory()?;
        seed_provider(&db, "claude", "p1")?;

        let r1 = db.create_model_route(&test_route("mid", "p1", 5))?;
        let r2 = db.create_model_route(&test_route("low", "p1", 1))?;
        let r3 = db.create_model_route(&test_route("high", "p1", 3))?;

        let routes = db.list_model_routes("claude")?;
        assert_eq!(routes.len(), 3);
        assert_eq!(routes[0].id, r2.id);
        assert_eq!(routes[0].priority, 1);
        assert_eq!(routes[1].id, r3.id);
        assert_eq!(routes[1].priority, 3);
        assert_eq!(routes[2].id, r1.id);
        assert_eq!(routes[2].priority, 5);

        Ok(())
    }

    #[test]
    fn update_model_route_modifies_fields() -> Result<(), AppError> {
        let db = Database::memory()?;
        seed_provider(&db, "claude", "p1")?;
        seed_provider(&db, "claude", "p2")?;

        let created = db.create_model_route(&test_route("*-sonnet", "p1", 10))?;

        let updated = db.update_model_route(
            &created.id,
            &ModelRoute {
                id: created.id.clone(),
                app_type: "claude".into(),
                pattern: "claude-*".into(),
                provider_id: "p2".into(),
                priority: 5,
                enabled: false,
                hit_count: 0,
                last_hit_at: None,
                created_at: None,
                updated_at: None,
            },
        )?;

        assert_eq!(updated.pattern, "claude-*");
        assert_eq!(updated.provider_id, "p2");
        assert_eq!(updated.priority, 5);
        assert!(!updated.enabled);

        let got = db.get_model_route(&created.id)?;
        assert!(got.is_some());
        let got = got.unwrap();
        assert_eq!(got.pattern, "claude-*");
        assert!(!got.enabled);

        Ok(())
    }

    #[test]
    fn toggle_model_route_flips_enabled() -> Result<(), AppError> {
        let db = Database::memory()?;
        seed_provider(&db, "claude", "p1")?;

        let created = db.create_model_route(&test_route("*-sonnet", "p1", 10))?;
        assert!(created.enabled);

        let toggled_off = db.toggle_model_route(&created.id)?;
        assert!(!toggled_off.enabled);

        let toggled_on = db.toggle_model_route(&created.id)?;
        assert!(toggled_on.enabled);

        Ok(())
    }

    #[test]
    fn delete_model_route_removes_row() -> Result<(), AppError> {
        let db = Database::memory()?;
        seed_provider(&db, "claude", "p1")?;

        let created = db.create_model_route(&test_route("*-sonnet", "p1", 10))?;

        db.delete_model_route(&created.id)?;

        let got = db.get_model_route(&created.id)?;
        assert!(got.is_none());

        let result = db.delete_model_route("nonexistent-id");
        assert!(result.is_err());

        Ok(())
    }

    #[test]
    fn record_model_route_hit_increments_count() -> Result<(), AppError> {
        let db = Database::memory()?;
        seed_provider(&db, "claude", "p1")?;

        let created = db.create_model_route(&test_route("*-sonnet", "p1", 10))?;
        assert_eq!(created.hit_count, 0);

        db.record_model_route_hit(&created.id)?;
        db.record_model_route_hit(&created.id)?;
        db.record_model_route_hit(&created.id)?;

        let got = db.get_model_route(&created.id)?.unwrap();
        assert_eq!(got.hit_count, 3);
        assert!(got.last_hit_at.is_some());

        Ok(())
    }

    #[test]
    fn aggregate_route_hits_by_provider_groups_correctly() -> Result<(), AppError> {
        let db = Database::memory()?;
        seed_provider(&db, "claude", "p1")?;
        seed_provider(&db, "claude", "p2")?;
        seed_provider(&db, "codex", "cx1")?;

        let r1 = db.create_model_route(&test_route("*sonnet*", "p1", 1))?;
        let r2 = db.create_model_route(&test_route("*opus*", "p2", 2))?;
        let mut codex_route = test_route("*codex*", "cx1", 1);
        codex_route.app_type = "codex".to_string();
        let r3 = db.create_model_route(&codex_route)?;
        let _r4 = db.create_model_route(&test_route("disabled", "p1", 5))?;

        // r4 is disabled
        db.toggle_model_route(
            &db.list_model_routes("claude")?
                .iter()
                .find(|r| r.pattern == "disabled")
                .unwrap()
                .id,
        )?;

        // 5 hits to claude/p1, 3 to claude/p2, 2 to codex/cx1
        for _ in 0..5 {
            db.record_model_route_hit(&r1.id)?;
        }
        for _ in 0..3 {
            db.record_model_route_hit(&r2.id)?;
        }
        for _ in 0..2 {
            db.record_model_route_hit(&r3.id)?;
        }

        let agg = db.aggregate_route_hits_by_provider()?;
        // r4 was disabled but got 0 hits, so it should be filtered out
        assert_eq!(agg.len(), 3);
        assert_eq!(agg[0], ("claude".to_string(), "p1".to_string(), 5));
        assert_eq!(agg[1], ("claude".to_string(), "p2".to_string(), 3));
        assert_eq!(agg[2], ("codex".to_string(), "cx1".to_string(), 2));

        Ok(())
    }
}
