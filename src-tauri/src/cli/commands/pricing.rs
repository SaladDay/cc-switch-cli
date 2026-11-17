use clap::Subcommand;
use serde_json::json;

use crate::database::Database;
use crate::error::AppError;
use crate::services::{global_proxy, model_pricing_sync};

#[derive(Subcommand, Debug, Clone)]
pub enum PricingCommand {
    /// Import base USD-per-million text model prices from models.dev (explicit network request)
    Sync {
        /// models.dev provider ID, e.g. openai or anthropic; unrelated to configured provider IDs
        #[arg(long)]
        provider: String,
        /// Exact catalog model ID; repeat to select multiple models (default: all text models)
        #[arg(long)]
        model: Vec<String>,
        /// Preview selected prices without changing the price catalog or usage costs
        #[arg(long)]
        dry_run: bool,
        /// Replace existing prices and restore explicitly deleted model prices
        #[arg(long)]
        overwrite: bool,
        /// Fill missing costs for selected models; preserve costs already calculated
        #[arg(long, conflicts_with = "dry_run")]
        backfill: bool,
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
}

pub fn execute(command: PricingCommand) -> Result<(), AppError> {
    execute_with_catalog_fetch(command, || {
        let runtime =
            tokio::runtime::Runtime::new().map_err(|e| AppError::Message(e.to_string()))?;
        runtime.block_on(model_pricing_sync::fetch_catalog())
    })
}

fn execute_with_catalog_fetch(
    command: PricingCommand,
    fetch: impl FnOnce() -> Result<serde_json::Value, AppError>,
) -> Result<(), AppError> {
    let PricingCommand::Sync {
        provider,
        model,
        dry_run,
        overwrite,
        backfill,
        json: json_output,
    } = command;
    if !dry_run {
        global_proxy::initialize_http_client_from_disk_best_effort();
    }
    let catalog = fetch()?;
    let selection = model_pricing_sync::select_catalog(&catalog, &provider, &model)?;
    finish_sync(
        selection,
        &provider,
        dry_run,
        overwrite,
        backfill,
        json_output,
    )
}

fn finish_sync(
    selection: model_pricing_sync::CatalogSelection,
    provider: &str,
    dry_run: bool,
    overwrite: bool,
    backfill: bool,
    json_output: bool,
) -> Result<(), AppError> {
    let (changed, preserved, backfilled) = if dry_run {
        (0, 0, 0)
    } else {
        let db = Database::init_for_pricing_sync()?;
        let (changed, preserved) = db.import_model_prices(&selection.prices, overwrite)?;
        let mut backfilled = 0;
        if backfill {
            for price in &selection.prices {
                backfilled += db
                    .backfill_missing_usage_costs_for_model(&price.model_id)
                    .map_err(|e| {
                        AppError::Message(format!("Prices saved, but usage backfill failed: {e}"))
                    })?;
            }
        }
        (changed, preserved, backfilled)
    };
    let result = json!({
        "source":"https://models.dev/api.json", "provider":provider, "dryRun":dry_run,
        "selected":selection.prices.len(), "changed":changed, "preserved":preserved,
        "skippedUnsupported":selection.skipped, "basePriceOnlyModels":selection.tiered,
        "backfilled":backfilled,
        "prices":selection.prices.iter().map(|p| json!({"modelId":p.model_id,"input":p.input_cost_per_million,"output":p.output_cost_per_million,"cacheRead":p.cache_read_cost_per_million,"cacheWrite":p.cache_creation_cost_per_million})).collect::<Vec<_>>()
    });
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&result)
                .map_err(|source| AppError::JsonSerialize { source })?
        );
    } else {
        if dry_run {
            for price in &selection.prices {
                println!(
                    "{}: input={}, output={}, cache read={}, cache write={} USD / 1M tokens",
                    price.model_id,
                    price.input_cost_per_million,
                    price.output_cost_per_million,
                    price.cache_read_cost_per_million,
                    price.cache_creation_cost_per_million
                );
            }
        }
        println!(
            "{} {}: selected {}, saved {}, preserved {}, unsupported {}, backfilled {}.",
            crate::t!("Pricing sync", "定价同步"),
            provider,
            selection.prices.len(),
            changed,
            preserved,
            selection.skipped,
            backfilled
        );
        println!("{}", crate::t!(
            "Reference base prices only; provider billing, context tiers and discounts may differ. Existing prices are preserved unless --overwrite is used.",
            "仅导入参考基础价格；供应商账单、长上下文分层及折扣可能不同。默认保留已有价格，使用 --overwrite 才会覆盖。"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::ModelPricingUpdate;

    fn selection(input: &str) -> model_pricing_sync::CatalogSelection {
        model_pricing_sync::CatalogSelection {
            prices: vec![
                ModelPricingUpdate::new("Vendor/SyncModel", "test", input, "0", "0", "0").unwrap(),
            ],
            skipped: 0,
            tiered: 0,
        }
    }

    #[test]
    fn pricing_sync_preview_does_not_open_database() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let dir = home.path().join(".cc-switch");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cc-switch.db");
        std::fs::write(&path, b"not a database").unwrap();
        execute_with_catalog_fetch(
            PricingCommand::Sync { provider: "test".into(), model: vec![], dry_run: true, overwrite: false, backfill: false, json: true },
            || Ok(json!({"test":{"models":{"Vendor/SyncModel":{"modalities":{"output":["text"]},"cost":{"input":2,"output":0}}}}})),
        ).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"not a database");
    }

    #[test]
    fn pricing_sync_disk_path_preserves_history_and_backfills_with_new_exact_price(
    ) -> Result<(), AppError> {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        {
            let db = Database::init_for_pricing_sync()?;
            db.import_model_prices(&selection("99").prices, true)?;
            db.import_model_prices(
                &[
                    ModelPricingUpdate::new("syncmodel", "fallback", "77", "0", "0", "0")?,
                    ModelPricingUpdate::new("unrelated", "other", "88", "0", "0", "0")?,
                ],
                true,
            )?;
            let conn = crate::database::lock_conn!(db.conn);
            conn.execute_batch("INSERT INTO proxy_request_logs
                (request_id, provider_id, app_type, model, input_tokens, output_tokens,
                 total_cost_usd, latency_ms, status_code, created_at)
                VALUES ('selected', '_codex_session', 'codex', 'Vendor/SyncModel', 1000000, 0, '0', 0, 200, 1),
                       ('unrelated', '_codex_session', 'codex', 'unrelated', 1000000, 0, '0', 0, 200, 1);")?;
        }
        finish_sync(selection("2"), "test", false, true, false, true)?;
        {
            let db = Database::init_for_pricing_sync()?;
            let conn = crate::database::lock_conn!(db.conn);
            let unpriced: i64 = conn.query_row(
                "SELECT COUNT(*) FROM proxy_request_logs WHERE total_cost_usd='0'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(unpriced, 2);
        }
        finish_sync(selection("3"), "test", false, true, true, true)?;
        let db = Database::init_for_pricing_sync()?;
        let conn = crate::database::lock_conn!(db.conn);
        let cost: String = conn.query_row(
            "SELECT total_cost_usd FROM proxy_request_logs WHERE request_id='selected'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(cost, "3.000000");
        let unrelated: String = conn.query_row(
            "SELECT total_cost_usd FROM proxy_request_logs WHERE request_id='unrelated'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(unrelated, "0");
        Ok(())
    }
}
