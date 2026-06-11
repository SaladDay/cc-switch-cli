//! 模型路由 DAO (Model Route Data Access Object)
//!
//! 管理 model_routes 表的 CRUD 操作，为 per-model provider routing 提供持久化层。
//! 支持按 app_type 列出路由、创建/更新/删除路由、切换启用状态。

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::model_route::ModelRoute;

impl Database {
    /// 列出指定 app_type 的所有模型路由，按 priority ASC, created_at ASC 排序
    pub fn list_model_routes(&self, app_type: &str) -> Result<Vec<ModelRoute>, AppError> {
        let conn = lock_conn!(self.conn);

        let mut stmt = conn
            .prepare(
                "SELECT id, app_type, pattern, provider_id, priority, enabled, created_at, updated_at
                 FROM model_routes
                 WHERE app_type = ?1
                 ORDER BY priority ASC, created_at ASC",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let items = stmt
            .query_map([app_type], |row| {
                Ok(ModelRoute {
                    id: Some(row.get(0)?),
                    app_type: row.get(1)?,
                    pattern: row.get(2)?,
                    provider_id: row.get(3)?,
                    priority: row.get(4)?,
                    enabled: row.get::<_, i32>(5)? != 0,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            })
            .map_err(|e| AppError::Database(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::Database(e.to_string()))?;

        Ok(items)
    }

    /// 根据 ID 获取单个模型路由
    pub fn get_model_route(&self, id: i64) -> Result<Option<ModelRoute>, AppError> {
        let conn = lock_conn!(self.conn);

        let mut stmt = conn
            .prepare(
                "SELECT id, app_type, pattern, provider_id, priority, enabled, created_at, updated_at
                 FROM model_routes
                 WHERE id = ?1",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut rows = stmt
            .query_map([id], |row| {
                Ok(ModelRoute {
                    id: Some(row.get(0)?),
                    app_type: row.get(1)?,
                    pattern: row.get(2)?,
                    provider_id: row.get(3)?,
                    priority: row.get(4)?,
                    enabled: row.get::<_, i32>(5)? != 0,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            })
            .map_err(|e| AppError::Database(e.to_string()))?;

        rows.next()
            .transpose()
            .map_err(|e| AppError::Database(e.to_string()))
    }

    /// 创建模型路由（验证 provider_id 存在）
    pub fn create_model_route(&self, route: &ModelRoute) -> Result<ModelRoute, AppError> {
        let conn = lock_conn!(self.conn);

        // 验证 provider 存在
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

        let mut stmt = conn
            .prepare(
                "INSERT INTO model_routes (app_type, pattern, provider_id, priority, enabled)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 RETURNING id, app_type, pattern, provider_id, priority, enabled, created_at, updated_at",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        stmt.query_row(
            rusqlite::params![
                &route.app_type,
                &route.pattern,
                &route.provider_id,
                route.priority,
                route.enabled as i32,
            ],
            |row| {
                Ok(ModelRoute {
                    id: Some(row.get(0)?),
                    app_type: row.get(1)?,
                    pattern: row.get(2)?,
                    provider_id: row.get(3)?,
                    priority: row.get(4)?,
                    enabled: row.get::<_, i32>(5)? != 0,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            },
        )
        .map_err(|e| AppError::Database(e.to_string()))
    }

    /// 更新模型路由
    pub fn update_model_route(&self, id: i64, route: &ModelRoute) -> Result<ModelRoute, AppError> {
        let conn = lock_conn!(self.conn);

        // 如果 provider_id 变更，验证新 provider 存在
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
            .prepare(
                "UPDATE model_routes SET
                     pattern = ?1, provider_id = ?2, priority = ?3, enabled = ?4,
                     updated_at = datetime('now')
                 WHERE id = ?5
                 RETURNING id, app_type, pattern, provider_id, priority, enabled, created_at, updated_at",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        stmt.query_row(
            rusqlite::params![
                &route.pattern,
                &route.provider_id,
                route.priority,
                route.enabled as i32,
                id,
            ],
            |row| {
                Ok(ModelRoute {
                    id: Some(row.get(0)?),
                    app_type: row.get(1)?,
                    pattern: row.get(2)?,
                    provider_id: row.get(3)?,
                    priority: row.get(4)?,
                    enabled: row.get::<_, i32>(5)? != 0,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            },
        )
        .map_err(|e| AppError::Database(e.to_string()))
    }

    /// 删除模型路由
    pub fn delete_model_route(&self, id: i64) -> Result<(), AppError> {
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
    pub fn toggle_model_route(&self, id: i64) -> Result<ModelRoute, AppError> {
        let conn = lock_conn!(self.conn);

        let mut stmt = conn
            .prepare(
                "UPDATE model_routes SET
                     enabled = NOT enabled,
                     updated_at = datetime('now')
                 WHERE id = ?1
                 RETURNING id, app_type, pattern, provider_id, priority, enabled, created_at, updated_at",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        stmt.query_row([id], |row| {
            Ok(ModelRoute {
                id: Some(row.get(0)?),
                app_type: row.get(1)?,
                pattern: row.get(2)?,
                provider_id: row.get(3)?,
                priority: row.get(4)?,
                enabled: row.get::<_, i32>(5)? != 0,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })
        .map_err(|e| AppError::Database(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 在内存数据库中准备一个 provider 供测试使用
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
            id: None,
            app_type: "claude".into(),
            pattern: pattern.into(),
            provider_id: provider_id.into(),
            priority,
            enabled: true,
            created_at: None,
            updated_at: None,
        }
    }

    #[test]
    fn create_and_get_model_route_roundtrip() -> Result<(), AppError> {
        let db = Database::memory()?;
        seed_provider(&db, "claude", "test-prov")?;

        let created = db.create_model_route(&test_route("*-sonnet", "test-prov", 10))?;

        assert_eq!(created.id, Some(1));
        assert_eq!(created.pattern, "*-sonnet");
        assert_eq!(created.provider_id, "test-prov");
        assert_eq!(created.priority, 10);
        assert!(created.enabled);
        assert!(created.created_at.is_some());

        let got = db.get_model_route(1)?;
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

        db.create_model_route(&test_route("mid", "p1", 5))?;
        db.create_model_route(&test_route("low", "p1", 1))?;
        db.create_model_route(&test_route("high", "p1", 3))?;

        let routes = db.list_model_routes("claude")?;
        assert_eq!(routes.len(), 3);
        assert_eq!(routes[0].priority, 1);
        assert_eq!(routes[1].priority, 3);
        assert_eq!(routes[2].priority, 5);

        Ok(())
    }

    #[test]
    fn update_model_route_modifies_fields() -> Result<(), AppError> {
        let db = Database::memory()?;
        seed_provider(&db, "claude", "p1")?;
        seed_provider(&db, "claude", "p2")?;

        db.create_model_route(&test_route("*-sonnet", "p1", 10))?;

        let updated = db.update_model_route(
            1,
            &ModelRoute {
                id: None,
                app_type: "claude".into(),
                pattern: "claude-*".into(),
                provider_id: "p2".into(),
                priority: 5,
                enabled: false,
                created_at: None,
                updated_at: None,
            },
        )?;

        assert_eq!(updated.pattern, "claude-*");
        assert_eq!(updated.provider_id, "p2");
        assert_eq!(updated.priority, 5);
        assert!(!updated.enabled);

        // Verify persistence
        let got = db.get_model_route(1)?;
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

        let toggled_off = db.toggle_model_route(1)?;
        assert!(!toggled_off.enabled);

        let toggled_on = db.toggle_model_route(1)?;
        assert!(toggled_on.enabled);

        Ok(())
    }

    #[test]
    fn delete_model_route_removes_row() -> Result<(), AppError> {
        let db = Database::memory()?;
        seed_provider(&db, "claude", "p1")?;

        db.create_model_route(&test_route("*-sonnet", "p1", 10))?;

        db.delete_model_route(1)?;

        let got = db.get_model_route(1)?;
        assert!(got.is_none());

        // delete non-existent should error
        let result = db.delete_model_route(999);
        assert!(result.is_err());

        Ok(())
    }
}
