use std::str::FromStr;

use rust_decimal::Decimal;

use crate::{app_config::AppType, database::Database, provider::Provider};

use super::parser::TokenUsage;

#[derive(Debug, Clone)]
pub struct CostBreakdown {
    pub input_cost: Decimal,
    pub output_cost: Decimal,
    pub cache_read_cost: Decimal,
    pub cache_creation_cost: Decimal,
    pub total_cost: Decimal,
}

#[derive(Debug, Clone)]
pub struct PricingConfig {
    pub cost_multiplier: Decimal,
    pub cost_multiplier_raw: String,
    pub pricing_model_source: String,
}

#[derive(Debug, Clone)]
pub struct ModelPricing {
    pub input_cost_per_million: Decimal,
    pub output_cost_per_million: Decimal,
    pub cache_read_cost_per_million: Decimal,
    pub cache_creation_cost_per_million: Decimal,
}

pub async fn resolve_pricing_config(
    db: &Database,
    app_type: &AppType,
    provider: &Provider,
) -> PricingConfig {
    let default_multiplier_raw = db
        .get_default_cost_multiplier(app_type.as_str())
        .await
        .unwrap_or_else(|_| "1".to_string());
    let default_multiplier = parse_decimal_or(&default_multiplier_raw, Decimal::ONE);

    let default_pricing_model_source = db
        .get_pricing_model_source(app_type.as_str())
        .await
        .unwrap_or_else(|_| "response".to_string());
    let default_pricing_model_source = sanitize_pricing_model_source(&default_pricing_model_source)
        .unwrap_or_else(|| "response".to_string());

    let provider_meta = provider.meta.as_ref();
    let cost_multiplier_raw = provider_meta
        .and_then(|meta| meta.cost_multiplier.clone())
        .unwrap_or(default_multiplier_raw);
    let pricing_model_source = provider_meta
        .and_then(|meta| meta.pricing_model_source.clone())
        .and_then(|value| sanitize_pricing_model_source(&value))
        .unwrap_or(default_pricing_model_source);

    PricingConfig {
        cost_multiplier: parse_decimal_or(&cost_multiplier_raw, default_multiplier),
        cost_multiplier_raw,
        pricing_model_source,
    }
}

pub fn pricing_model<'a>(
    request_model: &'a str,
    response_model: &'a str,
    pricing_model_source: &str,
) -> &'a str {
    if pricing_model_source == "request" || response_model.trim().is_empty() {
        request_model
    } else {
        response_model
    }
}

pub fn lookup_model_pricing(db: &Database, model_id: &str) -> Option<ModelPricing> {
    let conn = db.conn.lock().ok()?;
    conn.query_row(
        "SELECT input_cost_per_million, output_cost_per_million, cache_read_cost_per_million, cache_creation_cost_per_million
         FROM model_pricing WHERE model_id = ?1",
        [model_id],
        |row| {
            Ok(ModelPricing {
                input_cost_per_million: Decimal::from_str(&row.get::<_, String>(0)?).map_err(
                    |error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    },
                )?,
                output_cost_per_million: Decimal::from_str(&row.get::<_, String>(1)?).map_err(
                    |error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    },
                )?,
                cache_read_cost_per_million: Decimal::from_str(&row.get::<_, String>(2)?).map_err(
                    |error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            2,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    },
                )?,
                cache_creation_cost_per_million: Decimal::from_str(
                    &row.get::<_, String>(3)?,
                )
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
            })
        },
    )
    .ok()
}

pub fn calculate_cost(
    usage: &TokenUsage,
    pricing: Option<&ModelPricing>,
    cost_multiplier: Decimal,
) -> Option<CostBreakdown> {
    let pricing = pricing?;
    let million = Decimal::from(1_000_000u32);
    let billable_input_tokens = usage.input_tokens.saturating_sub(usage.cache_read_tokens);

    let input_cost =
        Decimal::from(billable_input_tokens) * pricing.input_cost_per_million / million;
    let output_cost =
        Decimal::from(usage.output_tokens) * pricing.output_cost_per_million / million;
    let cache_read_cost =
        Decimal::from(usage.cache_read_tokens) * pricing.cache_read_cost_per_million / million;
    let cache_creation_cost = Decimal::from(usage.cache_creation_tokens)
        * pricing.cache_creation_cost_per_million
        / million;
    let total_cost =
        (input_cost + output_cost + cache_read_cost + cache_creation_cost) * cost_multiplier;

    Some(CostBreakdown {
        input_cost,
        output_cost,
        cache_read_cost,
        cache_creation_cost,
        total_cost,
    })
}

pub fn format_decimal(value: Decimal) -> String {
    if value.is_zero() {
        "0".to_string()
    } else {
        value.normalize().to_string()
    }
}

fn parse_decimal_or(raw: &str, fallback: Decimal) -> Decimal {
    Decimal::from_str(raw.trim()).unwrap_or(fallback)
}

fn sanitize_pricing_model_source(raw: &str) -> Option<String> {
    match raw.trim() {
        "request" => Some("request".to_string()),
        "response" => Some("response".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculate_cost_applies_multiplier_to_total_only() {
        let usage = TokenUsage {
            input_tokens: 11,
            output_tokens: 7,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        };
        let pricing = ModelPricing {
            input_cost_per_million: Decimal::from_str("1.75").expect("parse input pricing"),
            output_cost_per_million: Decimal::from_str("14").expect("parse output pricing"),
            cache_read_cost_per_million: Decimal::ZERO,
            cache_creation_cost_per_million: Decimal::ZERO,
        };

        let cost = calculate_cost(&usage, Some(&pricing), Decimal::from(2u32)).expect("cost");

        assert_eq!(format_decimal(cost.input_cost), "0.00001925");
        assert_eq!(format_decimal(cost.output_cost), "0.000098");
        assert_eq!(format_decimal(cost.total_cost), "0.0002345");
    }

    #[test]
    fn pricing_model_prefers_request_when_configured() {
        assert_eq!(
            pricing_model("claude-3-7-sonnet", "gpt-5.2", "request"),
            "claude-3-7-sonnet"
        );
        assert_eq!(
            pricing_model("claude-3-7-sonnet", "gpt-5.2", "response"),
            "gpt-5.2"
        );
    }
}
