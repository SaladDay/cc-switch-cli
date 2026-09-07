//! 供应商数据访问对象
//!
//! 提供供应商（Provider）的 CRUD 操作。

use crate::database::dao::providers_seed::{is_official_seed_id, OFFICIAL_SEEDS};
use crate::database::{lock_conn, shared_store_error, sqlite_write_error, Database};
use crate::error::AppError;
use crate::provider::Provider;
use cc_switch_store::{
    ProviderInsert as SharedProviderInsert, ProviderRow as SharedProviderRow, ProviderWriteOutcome,
};
use indexmap::IndexMap;
use rusqlite::params;
use std::collections::{HashMap, HashSet};

fn require_applied(outcome: ProviderWriteOutcome, action: &str) -> Result<(), AppError> {
    match outcome {
        ProviderWriteOutcome::Applied => Ok(()),
        ProviderWriteOutcome::NotApplied => Err(AppError::Conflict(format!(
            "provider {action} was not applied"
        ))),
    }
}

fn provider_insert_from_model<'a>(
    app_type: &'a str,
    provider: &'a Provider,
    settings_config: &'a str,
    meta: &'a str,
    is_current: bool,
    in_failover_queue: bool,
) -> Result<SharedProviderInsert<'a>, AppError> {
    let sort_index = provider
        .sort_index
        .map(i64::try_from)
        .transpose()
        .map_err(|_| AppError::Database("provider sort_index is too large".to_owned()))?;

    Ok(SharedProviderInsert {
        id: &provider.id,
        app_type,
        name: &provider.name,
        settings_config,
        website_url: provider.website_url.as_deref(),
        category: provider.category.as_deref(),
        created_at: provider.created_at,
        sort_index,
        notes: provider.notes.as_deref(),
        icon: provider.icon.as_deref(),
        icon_color: provider.icon_color.as_deref(),
        meta,
        is_current: i64::from(is_current),
        in_failover_queue: i64::from(in_failover_queue),
    })
}

fn provider_from_shared_row(row: SharedProviderRow) -> Result<Provider, AppError> {
    let sort_index = match row.sort_index {
        Some(value) => Some(usize::try_from(value).map_err(|_| {
            AppError::Database(format!(
                "provider sort_index is outside the supported range: {value}"
            ))
        })?),
        None => None,
    };

    Ok(Provider {
        id: row.id,
        name: row.name,
        settings_config: serde_json::from_str(&row.settings_config)
            .unwrap_or(serde_json::Value::Null),
        website_url: row.website_url,
        category: row.category,
        created_at: row.created_at,
        sort_index,
        notes: row.notes,
        meta: Some(serde_json::from_str(&row.meta).unwrap_or_default()),
        icon: row.icon,
        icon_color: row.icon_color,
        in_failover_queue: row.in_failover_queue != 0,
    })
}

impl Database {
    /// 获取指定应用类型的所有供应商
    pub fn get_all_providers(
        &self,
        app_type: &str,
    ) -> Result<IndexMap<String, Provider>, AppError> {
        let mut conn = lock_conn!(self.conn);
        let transaction = conn.transaction().map_err(sqlite_write_error)?;
        let providers = Self::read_providers_on(&transaction, app_type)?;
        transaction.commit().map_err(sqlite_write_error)?;
        Ok(providers)
    }

    pub(crate) fn read_providers_on(
        conn: &rusqlite::Connection,
        app_type: &str,
    ) -> Result<IndexMap<String, Provider>, AppError> {
        let provider_rows = cc_switch_store::read_provider_rows(conn, Some(app_type))
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut providers = IndexMap::new();
        for row in provider_rows {
            let mut provider = provider_from_shared_row(row)?;
            let id = provider.id.clone();

            // 加载 endpoints
            let mut stmt_endpoints = conn.prepare(
                "SELECT url, added_at FROM provider_endpoints WHERE provider_id = ?1 AND app_type = ?2 ORDER BY added_at ASC, url ASC"
            ).map_err(|e| AppError::Database(e.to_string()))?;

            let endpoints_iter = stmt_endpoints
                .query_map(params![id, app_type], |row| {
                    let url: String = row.get(0)?;
                    let added_at: Option<i64> = row.get(1)?;
                    Ok((
                        url,
                        crate::settings::CustomEndpoint {
                            url: "".to_string(),
                            added_at: added_at.unwrap_or(0),
                            last_used: None,
                        },
                    ))
                })
                .map_err(|e| AppError::Database(e.to_string()))?;

            let mut custom_endpoints = HashMap::new();
            for ep_res in endpoints_iter {
                let (url, mut ep) = ep_res.map_err(|e| AppError::Database(e.to_string()))?;
                ep.url = url.clone();
                custom_endpoints.insert(url, ep);
            }

            if let Some(meta) = &mut provider.meta {
                meta.custom_endpoints = custom_endpoints;
            }

            providers.insert(id, provider);
        }

        Ok(providers)
    }

    /// 获取当前激活的供应商 ID
    pub fn get_current_provider(&self, app_type: &str) -> Result<Option<String>, AppError> {
        let conn = lock_conn!(self.conn);
        Self::read_current_provider_on(&conn, app_type)
    }

    pub(crate) fn read_current_provider_on(
        conn: &rusqlite::Connection,
        app_type: &str,
    ) -> Result<Option<String>, AppError> {
        let mut stmt = conn
            .prepare("SELECT id FROM providers WHERE app_type = ?1 AND is_current = 1 LIMIT 1")
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut rows = stmt
            .query(params![app_type])
            .map_err(|e| AppError::Database(e.to_string()))?;

        if let Some(row) = rows.next().map_err(|e| AppError::Database(e.to_string()))? {
            Ok(Some(
                row.get(0).map_err(|e| AppError::Database(e.to_string()))?,
            ))
        } else {
            Ok(None)
        }
    }

    /// 根据 ID 获取单个供应商
    pub fn get_provider_by_id(
        &self,
        id: &str,
        app_type: &str,
    ) -> Result<Option<Provider>, AppError> {
        let conn = lock_conn!(self.conn);
        cc_switch_store::read_provider_row(&conn, id, app_type)
            .map_err(|e| AppError::Database(e.to_string()))?
            .map(provider_from_shared_row)
            .transpose()
    }

    /// 仅获取指定 app 下所有 provider 的 id 集合。
    pub fn get_provider_ids(&self, app_type: &str) -> Result<HashSet<String>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare("SELECT id FROM providers WHERE app_type = ?1")
            .map_err(|e| AppError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![app_type], |row| row.get::<_, String>(0))
            .map_err(|e| AppError::Database(e.to_string()))?;
        let mut ids = HashSet::new();
        for row in rows {
            ids.insert(row.map_err(|e| AppError::Database(e.to_string()))?);
        }
        Ok(ids)
    }

    /// 判断指定 app 下是否存在非官方种子的供应商。
    pub fn has_non_official_seed_provider(&self, app_type: &str) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare("SELECT id FROM providers WHERE app_type = ?1")
            .map_err(|e| AppError::Database(e.to_string()))?;
        let mut rows = stmt
            .query(params![app_type])
            .map_err(|e| AppError::Database(e.to_string()))?;
        while let Some(row) = rows.next().map_err(|e| AppError::Database(e.to_string()))? {
            let id: String = row.get(0).map_err(|e| AppError::Database(e.to_string()))?;
            if !is_official_seed_id(&id) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn next_sort_index_for_app(&self, app_type: &str) -> Result<usize, AppError> {
        let conn = lock_conn!(self.conn);
        let max: Option<i64> = conn
            .query_row(
                "SELECT MAX(sort_index) FROM providers WHERE app_type = ?1",
                params![app_type],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(max.map(|value| (value + 1) as usize).unwrap_or(0))
    }

    /// 启动时补齐上游官方预设供应商（Claude / Codex / Gemini）。
    pub fn init_default_official_providers(&self) -> Result<usize, AppError> {
        if self
            .get_bool_flag("official_providers_seeded")
            .unwrap_or(false)
        {
            return Ok(0);
        }

        let mut inserted = 0usize;
        let now_ms = chrono::Utc::now().timestamp_millis();

        for seed in OFFICIAL_SEEDS {
            let app_type = seed.app_type.as_str();
            if self.get_provider_by_id(seed.id, app_type)?.is_some() {
                continue;
            }

            let settings_config: serde_json::Value =
                serde_json::from_str(seed.settings_config_json).map_err(|err| {
                    AppError::Database(format!("Seed JSON parse failed for {}: {err}", seed.id))
                })?;

            let mut provider = Provider::with_id(
                seed.id.to_string(),
                seed.name.to_string(),
                settings_config,
                Some(seed.website_url.to_string()),
            );
            provider.category = Some("official".to_string());
            provider.icon = Some(seed.icon.to_string());
            provider.icon_color = Some(seed.icon_color.to_string());
            provider.sort_index = Some(self.next_sort_index_for_app(app_type)?);
            provider.created_at = Some(now_ms);

            self.save_provider(app_type, &provider)?;
            inserted += 1;
            log::info!("✓ Seeded official provider: {} ({})", seed.name, app_type);
        }

        self.set_setting("official_providers_seeded", "true")?;
        Ok(inserted)
    }

    /// 保存供应商（新增或更新）
    ///
    /// 注意：更新模式下不同步 endpoints，因为编辑模式下端点通过单独的 API 管理
    /// （add_custom_endpoint / remove_custom_endpoint），避免覆盖用户的修改。
    pub fn save_provider(&self, app_type: &str, provider: &Provider) -> Result<(), AppError> {
        // 处理 meta：取出 endpoints 以便单独处理
        let mut meta_clone = provider.meta.clone().unwrap_or_default();
        let endpoints = std::mem::take(&mut meta_clone.custom_endpoints);
        let settings_config = serde_json::to_string(&provider.settings_config)
            .map_err(|e| AppError::Database(format!("Failed to serialize settings_config: {e}")))?;
        let meta = serde_json::to_string(&meta_clone)
            .map_err(|e| AppError::Database(format!("Failed to serialize meta: {e}")))?;

        let mut conn = lock_conn!(self.conn);
        let mut tx =
            cc_switch_store::begin_immediate_transaction(&mut conn).map_err(shared_store_error)?;

        // 检查是否存在（用于判断新增/更新，以及保留 is_current 和 in_failover_queue）
        let existing = cc_switch_store::read_provider_row(&tx, &provider.id, app_type)
            .map_err(shared_store_error)?;
        let is_current = existing.as_ref().is_some_and(|row| row.is_current != 0);
        let in_failover_queue = existing
            .as_ref()
            .map_or(provider.in_failover_queue, |row| row.in_failover_queue != 0);
        let shared_provider = provider_insert_from_model(
            app_type,
            provider,
            &settings_config,
            &meta,
            is_current,
            in_failover_queue,
        )?;

        if let Some(existing) = existing.as_ref() {
            let outcome = cc_switch_store::update_provider_row_if_unchanged(
                &mut tx,
                existing.source_fingerprint(),
                &shared_provider,
            )
            .map_err(shared_store_error)?;
            require_applied(outcome, "update")?;
        } else {
            let outcome = cc_switch_store::insert_provider_if_absent(&mut tx, &shared_provider)
                .map_err(shared_store_error)?;
            require_applied(outcome, "insert")?;

            // 只有新增时才同步 endpoints
            for (url, endpoint) in endpoints {
                tx.execute(
                    "INSERT INTO provider_endpoints (provider_id, app_type, url, added_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![provider.id, app_type, url, endpoint.added_at],
                )
                .map_err(|e| AppError::Database(e.to_string()))?;
            }
        }

        tx.commit().map_err(sqlite_write_error)?;
        Ok(())
    }

    /// 删除供应商
    pub fn delete_provider(&self, app_type: &str, id: &str) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let mut tx =
            cc_switch_store::begin_immediate_transaction(&mut conn).map_err(shared_store_error)?;
        let target =
            cc_switch_store::read_provider_row(&tx, id, app_type).map_err(shared_store_error)?;
        let (takeover_enabled, auto_failover_enabled, queued_count, deleting_queued, target_exists): (
            bool,
            bool,
            i64,
            bool,
            bool,
        ) = tx
            .query_row(
                "SELECT
                     COALESCE((SELECT enabled FROM proxy_config
                               WHERE app_type COLLATE BINARY = ?1), 0),
                     COALESCE((SELECT auto_failover_enabled FROM proxy_config
                               WHERE app_type COLLATE BINARY = ?1), 0),
                     (SELECT COUNT(*) FROM providers
                      WHERE app_type COLLATE BINARY = ?1 AND in_failover_queue = 1),
                     COALESCE((SELECT in_failover_queue FROM providers
                               WHERE app_type COLLATE BINARY = ?1
                                 AND id COLLATE BINARY = ?2), 0),
                     EXISTS(SELECT 1 FROM providers
                            WHERE app_type COLLATE BINARY = ?1
                              AND id COLLATE BINARY = ?2)",
                params![app_type, id],
                |row| {
                    Ok((
                        row.get::<_, i32>(0)? != 0,
                        row.get::<_, i32>(1)? != 0,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i32>(3)? != 0,
                        row.get::<_, i32>(4)? != 0,
                    ))
                },
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        if takeover_enabled && auto_failover_enabled && queued_count == 1 && deleting_queued {
            return Err(AppError::InvalidInput(
                "At least one provider must remain in the failover queue while proxy failover is active.".to_string(),
            ));
        }

        let outcome = match target.as_ref() {
            Some(target) => cc_switch_store::delete_provider_with_host_cleanup_if_unchanged(
                &mut tx,
                id,
                app_type,
                target.source_fingerprint(),
            )
            .map_err(shared_store_error)?,
            None => ProviderWriteOutcome::NotApplied,
        };
        if target_exists {
            require_applied(outcome, "delete")?;
        }
        tx.commit().map_err(sqlite_write_error)?;
        Ok(())
    }

    /// 设置当前供应商
    pub fn set_current_provider(&self, app_type: &str, id: &str) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let mut tx =
            cc_switch_store::begin_immediate_transaction(&mut conn).map_err(shared_store_error)?;
        let providers =
            cc_switch_store::read_provider_rows(&tx, Some(app_type)).map_err(shared_store_error)?;
        let target = providers.iter().find(|provider| provider.id == id);

        for provider in providers
            .iter()
            .filter(|provider| provider.is_current != 0 && provider.id != id)
        {
            let outcome = cc_switch_store::set_provider_current_if_unchanged(
                &mut tx,
                &provider.id,
                app_type,
                provider.source_fingerprint(),
                false,
            )
            .map_err(shared_store_error)?;
            require_applied(outcome, "current-provider reset")?;
        }

        let outcome = match target {
            Some(target) => cc_switch_store::set_provider_current_if_unchanged(
                &mut tx,
                id,
                app_type,
                target.source_fingerprint(),
                true,
            )
            .map_err(shared_store_error)?,
            None => ProviderWriteOutcome::NotApplied,
        };
        if target.is_some() {
            require_applied(outcome, "current-provider selection")?;
        }

        tx.commit().map_err(sqlite_write_error)?;
        Ok(())
    }

    /// 更新供应商的 settings_config（仅更新配置，不改变其他字段）
    pub fn update_provider_settings_config(
        &self,
        app_type: &str,
        provider_id: &str,
        settings_config: &serde_json::Value,
    ) -> Result<(), AppError> {
        let settings_config = serde_json::to_string(settings_config)
            .map_err(|e| AppError::Database(format!("Failed to serialize settings_config: {e}")))?;
        let mut conn = lock_conn!(self.conn);
        let mut tx =
            cc_switch_store::begin_immediate_transaction(&mut conn).map_err(shared_store_error)?;
        let target = cc_switch_store::read_provider_row(&tx, provider_id, app_type)
            .map_err(shared_store_error)?;
        let outcome = match target.as_ref() {
            Some(target) => cc_switch_store::update_provider_settings_config_if_unchanged(
                &mut tx,
                provider_id,
                app_type,
                target.source_fingerprint(),
                &settings_config,
            )
            .map_err(shared_store_error)?,
            None => ProviderWriteOutcome::NotApplied,
        };
        if target.is_some() {
            require_applied(outcome, "settings update")?;
        }
        tx.commit().map_err(sqlite_write_error)?;
        Ok(())
    }

    /// 添加自定义端点
    pub fn add_custom_endpoint(
        &self,
        app_type: &str,
        provider_id: &str,
        url: &str,
    ) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        let added_at = chrono::Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO provider_endpoints (provider_id, app_type, url, added_at) VALUES (?1, ?2, ?3, ?4)",
            params![provider_id, app_type, url, added_at],
        ).map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 移除自定义端点
    pub fn remove_custom_endpoint(
        &self,
        app_type: &str,
        provider_id: &str,
        url: &str,
    ) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "DELETE FROM provider_endpoints WHERE provider_id = ?1 AND app_type = ?2 AND url = ?3",
            params![provider_id, app_type, url],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use serde_json::json;

    fn provider(id: &str) -> Provider {
        Provider::with_id(
            id.to_string(),
            id.to_string(),
            json!({"env": {"BASE_URL": "https://example.com"}}),
            None,
        )
    }

    #[test]
    fn delete_provider_rejects_last_failover_queue_entry_while_active() -> Result<(), AppError> {
        let db = Database::memory()?;
        db.save_provider("claude", &provider("current"))?;
        db.save_provider("claude", &provider("queued"))?;
        db.set_current_provider("claude", "current")?;
        db.add_to_failover_queue("claude", "queued")?;
        db.set_proxy_flags_sync("claude", true, true)?;

        let err = db.delete_provider("claude", "queued").unwrap_err();

        assert!(matches!(err, AppError::InvalidInput(_)));
        assert!(db.get_provider_by_id("queued", "claude")?.is_some());
        Ok(())
    }

    #[test]
    fn set_current_provider_rolls_back_when_target_write_is_suppressed() -> Result<(), AppError> {
        let db = Database::memory()?;
        db.save_provider("claude", &provider("current"))?;
        db.save_provider("claude", &provider("next"))?;
        db.set_current_provider("claude", "current")?;

        {
            let conn = lock_conn!(db.conn);
            conn.execute_batch(
                "CREATE TRIGGER suppress_current_selection
                 BEFORE UPDATE OF is_current ON providers
                 WHEN NEW.id = 'next' AND NEW.app_type = 'claude' AND NEW.is_current = 1
                 BEGIN
                     SELECT RAISE(IGNORE);
                 END;",
            )?;
        }

        assert!(db.set_current_provider("claude", "next").is_err());
        assert_eq!(
            db.get_current_provider("claude")?.as_deref(),
            Some("current")
        );
        Ok(())
    }

    #[test]
    fn delete_provider_rejects_suppressed_existing_write_but_allows_missing() -> Result<(), AppError>
    {
        let db = Database::memory()?;
        db.save_provider("claude", &provider("kept"))?;
        {
            let conn = lock_conn!(db.conn);
            conn.execute_batch(
                "CREATE TABLE provider_delete_effect (value INTEGER NOT NULL);
                 INSERT INTO provider_delete_effect VALUES (0);
                 CREATE TRIGGER suppress_provider_delete
                 BEFORE DELETE ON providers
                 WHEN OLD.id = 'kept' AND OLD.app_type = 'claude'
                 BEGIN
                     UPDATE provider_delete_effect SET value = 1;
                     SELECT RAISE(IGNORE);
                 END;",
            )?;
        }

        assert!(db.delete_provider("claude", "kept").is_err());
        assert!(db.get_provider_by_id("kept", "claude")?.is_some());
        {
            let conn = lock_conn!(db.conn);
            assert_eq!(
                conn.query_row("SELECT value FROM provider_delete_effect", [], |row| {
                    row.get::<_, i64>(0)
                })?,
                0
            );
        }

        db.delete_provider("claude", "missing")?;
        Ok(())
    }

    #[test]
    fn delete_provider_removes_cli_owned_dependents() -> Result<(), AppError> {
        let db = Database::memory()?;
        db.save_provider("claude", &provider("deleted"))?;
        db.add_custom_endpoint("claude", "deleted", "https://edge.example.com")?;

        db.delete_provider("claude", "deleted")?;

        assert!(db.get_provider_by_id("deleted", "claude")?.is_none());
        let conn = lock_conn!(db.conn);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM provider_endpoints
                 WHERE provider_id = 'deleted' AND app_type = 'claude'",
                [],
                |row| row.get::<_, i64>(0),
            )?,
            0
        );
        Ok(())
    }

    #[test]
    fn delete_provider_allows_unknown_host_cleanup() -> Result<(), AppError> {
        let db = Database::memory()?;
        db.save_provider("claude", &provider("deleted"))?;
        {
            let conn = lock_conn!(db.conn);
            conn.execute_batch(
                "CREATE TABLE host_provider_binding (
                    provider_id TEXT NOT NULL,
                    app_type TEXT NOT NULL
                 );
                 INSERT INTO host_provider_binding VALUES ('deleted', 'claude');
                 CREATE TRIGGER clean_host_provider_binding
                 AFTER DELETE ON providers
                 WHEN OLD.id = 'deleted' AND OLD.app_type = 'claude'
                 BEGIN
                    DELETE FROM host_provider_binding
                    WHERE provider_id = OLD.id AND app_type = OLD.app_type;
                 END;",
            )?;
        }

        db.delete_provider("claude", "deleted")?;

        let conn = lock_conn!(db.conn);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM host_provider_binding", [], |row| {
                row.get::<_, i64>(0)
            })?,
            0
        );
        Ok(())
    }

    #[test]
    fn delete_provider_rolls_back_dependent_trigger_catalog_rewrite() -> Result<(), AppError> {
        let db = Database::memory()?;
        db.save_provider("claude", &provider("deleted"))?;
        db.save_provider("claude", &provider("other"))?;
        db.add_custom_endpoint("claude", "deleted", "https://edge.example.com")?;
        {
            let conn = lock_conn!(db.conn);
            conn.execute_batch(
                "CREATE TRIGGER rewrite_provider_from_endpoint_cleanup
                 AFTER DELETE ON provider_endpoints
                 WHEN OLD.provider_id = 'deleted' AND OLD.app_type = 'claude'
                 BEGIN
                    UPDATE providers SET name = 'rewritten'
                    WHERE id = 'other' AND app_type = 'claude';
                 END;",
            )?;
        }

        assert!(matches!(
            db.delete_provider("claude", "deleted"),
            Err(AppError::Conflict(_))
        ));
        assert!(db.get_provider_by_id("deleted", "claude")?.is_some());
        assert_eq!(
            db.get_provider_by_id("other", "claude")?
                .expect("other provider remains")
                .name,
            "other"
        );
        Ok(())
    }

    #[test]
    fn delete_provider_reports_deferred_foreign_key_rejection_as_conflict() -> Result<(), AppError>
    {
        let db = Database::memory()?;
        db.save_provider("claude", &provider("referenced"))?;
        {
            let conn = lock_conn!(db.conn);
            conn.execute_batch(
                "CREATE TABLE deferred_provider_reference (
                    provider_id TEXT NOT NULL,
                    app_type TEXT NOT NULL,
                    FOREIGN KEY (provider_id, app_type)
                    REFERENCES providers(id, app_type)
                    DEFERRABLE INITIALLY DEFERRED
                 );
                 INSERT INTO deferred_provider_reference
                 VALUES ('referenced', 'claude');",
            )?;
        }

        assert!(matches!(
            db.delete_provider("claude", "referenced"),
            Err(AppError::Conflict(_))
        ));
        assert!(db.get_provider_by_id("referenced", "claude")?.is_some());
        let conn = lock_conn!(db.conn);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM deferred_provider_reference",
                [],
                |row| row.get::<_, i64>(0),
            )?,
            1
        );
        Ok(())
    }

    #[test]
    fn settings_update_rejects_suppressed_existing_write_but_allows_missing() -> Result<(), AppError>
    {
        let db = Database::memory()?;
        db.save_provider("claude", &provider("kept"))?;
        {
            let conn = lock_conn!(db.conn);
            conn.execute_batch(
                "CREATE TRIGGER suppress_provider_settings_update
                 BEFORE UPDATE OF settings_config ON providers
                 WHEN OLD.id = 'kept' AND OLD.app_type = 'claude'
                 BEGIN
                     SELECT RAISE(IGNORE);
                 END;",
            )?;
        }

        assert!(db
            .update_provider_settings_config(
                "claude",
                "kept",
                &json!({"env": {"BASE_URL": "https://changed.example.com"}}),
            )
            .is_err());
        let kept = db
            .get_provider_by_id("kept", "claude")?
            .expect("existing provider");
        assert_eq!(
            kept.settings_config["env"]["BASE_URL"],
            "https://example.com"
        );

        db.update_provider_settings_config(
            "claude",
            "missing",
            &json!({"env": {"BASE_URL": "https://missing.example.com"}}),
        )?;
        Ok(())
    }

    #[test]
    fn set_current_provider_preserves_legacy_missing_target_behavior() -> Result<(), AppError> {
        let db = Database::memory()?;
        db.save_provider("claude", &provider("current"))?;
        db.set_current_provider("claude", "current")?;

        db.set_current_provider("claude", "missing")?;
        assert_eq!(db.get_current_provider("claude")?, None);
        Ok(())
    }

    #[test]
    fn set_current_provider_normalizes_a_noncanonical_true_value() -> Result<(), AppError> {
        let db = Database::memory()?;
        db.save_provider("claude", &provider("current"))?;
        db.save_provider("claude", &provider("next"))?;
        db.set_current_provider("claude", "current")?;
        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "UPDATE providers SET is_current = 2
                 WHERE id = 'next' AND app_type = 'claude'",
                [],
            )?;
        }

        db.set_current_provider("claude", "next")?;

        assert_eq!(db.get_current_provider("claude")?.as_deref(), Some("next"));
        let conn = lock_conn!(db.conn);
        assert_eq!(
            conn.query_row(
                "SELECT is_current FROM providers
                 WHERE id = 'next' AND app_type = 'claude'",
                [],
                |row| row.get::<_, i64>(0),
            )?,
            1
        );
        Ok(())
    }

    #[test]
    fn shared_store_update_preserves_cli_owned_columns_and_endpoints() -> Result<(), AppError> {
        let db = Database::memory()?;
        let mut original = provider("shared");
        original.website_url = Some("https://example.com".to_owned());
        original.category = Some("custom".to_owned());
        original.created_at = Some(123);
        original.sort_index = Some(4);
        original.notes = Some("before".to_owned());
        original.icon = Some("anthropic".to_owned());
        original.icon_color = Some("#123456".to_owned());
        original.meta = Some(crate::provider::ProviderMeta {
            custom_endpoints: [(
                "https://edge.example.com".to_owned(),
                crate::settings::CustomEndpoint {
                    url: "https://edge.example.com".to_owned(),
                    added_at: 456,
                    last_used: None,
                },
            )]
            .into_iter()
            .collect(),
            apply_common_config: Some(true),
            ..crate::provider::ProviderMeta::default()
        });
        db.save_provider("claude", &original)?;

        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "ALTER TABLE providers ADD COLUMN cli_owned_extension TEXT",
                [],
            )?;
            conn.execute(
                "UPDATE providers SET cli_owned_extension = 'preserved'
                 WHERE id = 'shared' AND app_type = 'claude'",
                [],
            )?;
        }

        let mut updated = original.clone();
        updated.name = "After".to_owned();
        updated.notes = Some("after".to_owned());
        updated.settings_config = json!({"env": {"BASE_URL": "https://after.example.com"}});
        updated
            .meta
            .as_mut()
            .expect("provider metadata")
            .custom_endpoints
            .clear();
        db.save_provider("claude", &updated)?;

        let mut providers = db.get_all_providers("claude")?;
        let loaded = providers.shift_remove("shared").expect("saved provider");
        assert_eq!(loaded.name, "After");
        assert_eq!(loaded.notes.as_deref(), Some("after"));
        assert_eq!(
            loaded.settings_config["env"]["BASE_URL"],
            "https://after.example.com"
        );
        assert!(loaded
            .meta
            .expect("loaded metadata")
            .custom_endpoints
            .contains_key("https://edge.example.com"));

        let conn = lock_conn!(db.conn);
        let extension = conn.query_row(
            "SELECT cli_owned_extension FROM providers
             WHERE id = 'shared' AND app_type = 'claude'",
            [],
            |row| row.get::<_, String>(0),
        )?;
        assert_eq!(extension, "preserved");
        Ok(())
    }
}
