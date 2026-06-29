//! Model pricing catalog data access.

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::BTreeSet;

const DELETED_MODEL_PRICING_IDS_KEY: &str = "model_pricing_deleted_ids";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelPricingUpdate {
    pub model_id: String,
    pub display_name: String,
    pub input_cost_per_million: String,
    pub output_cost_per_million: String,
    pub cache_read_cost_per_million: String,
    pub cache_creation_cost_per_million: String,
}

impl ModelPricingUpdate {
    pub(crate) fn new(
        model_id: impl Into<String>,
        display_name: impl Into<String>,
        input_cost_per_million: impl Into<String>,
        output_cost_per_million: impl Into<String>,
        cache_read_cost_per_million: impl Into<String>,
        cache_creation_cost_per_million: impl Into<String>,
    ) -> Result<Self, AppError> {
        Ok(Self {
            model_id: required_pricing_field("model_id", model_id.into())?,
            display_name: required_pricing_field("display_name", display_name.into())?,
            input_cost_per_million: pricing_decimal_field(
                "input_cost_per_million",
                input_cost_per_million.into(),
            )?,
            output_cost_per_million: pricing_decimal_field(
                "output_cost_per_million",
                output_cost_per_million.into(),
            )?,
            cache_read_cost_per_million: pricing_decimal_field(
                "cache_read_cost_per_million",
                cache_read_cost_per_million.into(),
            )?,
            cache_creation_cost_per_million: pricing_decimal_field(
                "cache_creation_cost_per_million",
                cache_creation_cost_per_million.into(),
            )?,
        })
    }

    fn normalized(&self) -> Result<Self, AppError> {
        Self::new(
            self.model_id.as_str(),
            self.display_name.as_str(),
            self.input_cost_per_million.as_str(),
            self.output_cost_per_million.as_str(),
            self.cache_read_cost_per_million.as_str(),
            self.cache_creation_cost_per_million.as_str(),
        )
    }
}

impl Database {
    pub(crate) fn upsert_model_pricing(
        &self,
        pricing: &ModelPricingUpdate,
    ) -> Result<(), AppError> {
        let pricing = pricing.normalized()?;
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT INTO model_pricing (
                model_id,
                display_name,
                input_cost_per_million,
                output_cost_per_million,
                cache_read_cost_per_million,
                cache_creation_cost_per_million
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(model_id) DO UPDATE SET
                display_name = excluded.display_name,
                input_cost_per_million = excluded.input_cost_per_million,
                output_cost_per_million = excluded.output_cost_per_million,
                cache_read_cost_per_million = excluded.cache_read_cost_per_million,
                cache_creation_cost_per_million = excluded.cache_creation_cost_per_million",
            params![
                &pricing.model_id,
                &pricing.display_name,
                &pricing.input_cost_per_million,
                &pricing.output_cost_per_million,
                &pricing.cache_read_cost_per_million,
                &pricing.cache_creation_cost_per_million,
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Self::forget_deleted_model_pricing_on_conn(&conn, &pricing.model_id)?;
        Ok(())
    }

    pub(crate) fn delete_model_pricing(&self, model_id: &str) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let deleted = conn
            .execute(
                "DELETE FROM model_pricing WHERE model_id = ?1",
                params![model_id],
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        if deleted > 0 {
            Self::remember_deleted_model_pricing_on_conn(&conn, model_id)?;
        }
        Ok(deleted > 0)
    }

    pub(crate) fn prune_deleted_model_pricing_on_conn(conn: &Connection) -> Result<(), AppError> {
        for model_id in Self::deleted_model_pricing_ids_on_conn(conn)? {
            conn.execute(
                "DELETE FROM model_pricing WHERE model_id = ?1",
                params![model_id],
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        }
        Ok(())
    }

    fn remember_deleted_model_pricing_on_conn(
        conn: &Connection,
        model_id: &str,
    ) -> Result<(), AppError> {
        let model_id = model_id.trim();
        if model_id.is_empty() {
            return Ok(());
        }
        let mut deleted = Self::deleted_model_pricing_ids_on_conn(conn)?;
        deleted.insert(model_id.to_string());
        Self::save_deleted_model_pricing_ids_on_conn(conn, &deleted)
    }

    fn forget_deleted_model_pricing_on_conn(
        conn: &Connection,
        model_id: &str,
    ) -> Result<(), AppError> {
        let mut deleted = Self::deleted_model_pricing_ids_on_conn(conn)?;
        if deleted.remove(model_id.trim()) {
            Self::save_deleted_model_pricing_ids_on_conn(conn, &deleted)?;
        }
        Ok(())
    }

    fn deleted_model_pricing_ids_on_conn(conn: &Connection) -> Result<BTreeSet<String>, AppError> {
        let raw = conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![DELETED_MODEL_PRICING_IDS_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| AppError::Database(e.to_string()))?;

        let Some(raw) = raw else {
            return Ok(BTreeSet::new());
        };

        let parsed = serde_json::from_str::<Vec<String>>(&raw).unwrap_or_default();
        Ok(parsed
            .into_iter()
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
            .collect())
    }

    fn save_deleted_model_pricing_ids_on_conn(
        conn: &Connection,
        deleted: &BTreeSet<String>,
    ) -> Result<(), AppError> {
        if deleted.is_empty() {
            conn.execute(
                "DELETE FROM settings WHERE key = ?1",
                params![DELETED_MODEL_PRICING_IDS_KEY],
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
            return Ok(());
        }

        let ids = deleted.iter().cloned().collect::<Vec<_>>();
        let value = serde_json::to_string(&ids)
            .map_err(|e| AppError::Database(format!("serialize deleted pricing ids: {e}")))?;
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            params![DELETED_MODEL_PRICING_IDS_KEY, value],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

fn required_pricing_field(field: &str, value: String) -> Result<String, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::InvalidInput(format!("{field} is required.")));
    }
    Ok(trimmed.to_string())
}

fn pricing_decimal_field(field: &str, value: String) -> Result<String, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::InvalidInput(format!("{field} is required.")));
    }
    let parsed = trimmed
        .parse::<rust_decimal::Decimal>()
        .map_err(|_| AppError::InvalidInput(format!("{field} must be a number.")))?;
    if parsed < rust_decimal::Decimal::ZERO {
        return Err(AppError::InvalidInput(format!(
            "{field} must be non-negative."
        )));
    }
    Ok(trimmed.to_string())
}
