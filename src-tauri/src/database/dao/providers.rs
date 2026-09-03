//! 供应商数据访问对象
//!
//! 提供供应商（Provider）的 CRUD 操作。

use crate::database::dao::providers_seed::{is_official_seed_id, OFFICIAL_SEEDS};
use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::provider::Provider;
use cc_switch_store::{
    ProviderInsert as SharedProviderInsert, ProviderRow as SharedProviderRow, ProviderWriteOutcome,
};
use indexmap::IndexMap;
use rusqlite::params;
use std::collections::{HashMap, HashSet};

fn shared_store_error(error: cc_switch_store::SharedStoreError) -> AppError {
    AppError::Database(error.to_string())
}

fn require_applied(outcome: ProviderWriteOutcome, action: &str) -> Result<(), AppError> {
    match outcome {
        ProviderWriteOutcome::Applied => Ok(()),
        ProviderWriteOutcome::NotApplied => Err(AppError::Database(format!(
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
        let conn = lock_conn!(self.conn);
        let provider_rows = cc_switch_store::read_provider_rows(&conn, Some(app_type))
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

        if existing.is_some() {
            let outcome = cc_switch_store::update_provider_row(&mut tx, &shared_provider)
                .map_err(shared_store_error)?;
            require_applied(outcome, "update")?;
        } else {
            let outcome = cc_switch_store::insert_provider(&mut tx, &shared_provider)
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

        tx.commit().map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 删除供应商
    pub fn delete_provider(&self, app_type: &str, id: &str) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let mut tx =
            cc_switch_store::begin_immediate_transaction(&mut conn).map_err(shared_store_error)?;
        let (takeover_enabled, auto_failover_enabled, queued_count, deleting_queued): (
            bool,
            bool,
            i64,
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
                                 AND id COLLATE BINARY = ?2), 0)",
                params![app_type, id],
                |row| {
                    Ok((
                        row.get::<_, i32>(0)? != 0,
                        row.get::<_, i32>(1)? != 0,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i32>(3)? != 0,
                    ))
                },
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        if takeover_enabled && auto_failover_enabled && queued_count == 1 && deleting_queued {
            return Err(AppError::InvalidInput(
                "At least one provider must remain in the failover queue while proxy failover is active.".to_string(),
            ));
        }

        let _outcome =
            cc_switch_store::delete_provider(&mut tx, id, app_type).map_err(shared_store_error)?;
        tx.commit()
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(())
    }

    /// 设置当前供应商
    pub fn set_current_provider(&self, app_type: &str, id: &str) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let mut tx =
            cc_switch_store::begin_immediate_transaction(&mut conn).map_err(shared_store_error)?;
        let providers =
            cc_switch_store::read_provider_rows(&tx, Some(app_type)).map_err(shared_store_error)?;
        if !providers.iter().any(|provider| provider.id == id) {
            return Err(AppError::InvalidInput(format!("Provider not found: {id}")));
        }

        for provider in providers
            .iter()
            .filter(|provider| provider.is_current != 0 && provider.id != id)
        {
            let outcome =
                cc_switch_store::set_provider_current(&mut tx, &provider.id, app_type, false)
                    .map_err(shared_store_error)?;
            require_applied(outcome, "current-provider reset")?;
        }

        let outcome = cc_switch_store::set_provider_current(&mut tx, id, app_type, true)
            .map_err(shared_store_error)?;
        require_applied(outcome, "current-provider selection")?;

        tx.commit().map_err(|e| AppError::Database(e.to_string()))?;
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
        let outcome = cc_switch_store::update_provider_settings_config(
            &mut tx,
            provider_id,
            app_type,
            &settings_config,
        )
        .map_err(shared_store_error)?;
        require_applied(outcome, "settings update")?;
        tx.commit()
            .map_err(|error| AppError::Database(error.to_string()))?;
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
    fn set_current_provider_rejects_a_missing_target_without_clearing_current(
    ) -> Result<(), AppError> {
        let db = Database::memory()?;
        db.save_provider("claude", &provider("current"))?;
        db.set_current_provider("claude", "current")?;

        assert!(matches!(
            db.set_current_provider("claude", "missing"),
            Err(AppError::InvalidInput(_))
        ));
        assert_eq!(
            db.get_current_provider("claude")?.as_deref(),
            Some("current")
        );
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
}
