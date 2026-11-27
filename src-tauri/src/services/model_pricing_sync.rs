//! Explicit, provider-scoped imports of base USD-per-million prices from models.dev.

use crate::database::ModelPricingUpdate;
use crate::error::AppError;
use serde_json::Value;
use std::collections::BTreeSet;
use std::time::Duration;

const CATALOG_URL: &str = "https://models.dev/api.json";
const MAX_CATALOG_BYTES: usize = 32 * 1024 * 1024;

pub(crate) struct CatalogSelection {
    pub prices: Vec<ModelPricingUpdate>,
    pub skipped: usize,
    pub tiered: usize,
}

pub(crate) async fn fetch_catalog() -> Result<Value, AppError> {
    // Honor the existing outbound proxy configuration without sending provider credentials.
    let client = crate::proxy::http_client::get();
    fetch_catalog_from(&client, CATALOG_URL).await
}

async fn fetch_catalog_from(client: &reqwest::Client, url: &str) -> Result<Value, AppError> {
    let mut response = client
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| AppError::Message(format!("models.dev download failed: {e}")))?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_CATALOG_BYTES as u64)
    {
        return Err(AppError::Message(
            "models.dev catalog exceeds 32 MiB".into(),
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| AppError::Message(format!("models.dev download failed: {e}")))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_CATALOG_BYTES {
            return Err(AppError::Message(
                "models.dev catalog exceeds 32 MiB".into(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Message(format!("Invalid models.dev catalog: {e}")))
}

pub(crate) fn select_catalog(
    catalog: &Value,
    provider: &str,
    models: &[String],
) -> Result<CatalogSelection, AppError> {
    let entries = catalog
        .get(provider)
        .and_then(|p| p.get("models"))
        .and_then(Value::as_object)
        .ok_or_else(|| {
            AppError::InvalidInput(format!("Unknown models.dev provider: {provider}"))
        })?;
    let requested: BTreeSet<&str> = models.iter().map(String::as_str).collect();
    for id in &requested {
        if !entries.contains_key(*id) {
            return Err(AppError::InvalidInput(format!(
                "Model {id} not found in models.dev provider {provider}"
            )));
        }
    }
    let mut selection = CatalogSelection {
        prices: Vec::new(),
        skipped: 0,
        tiered: 0,
    };
    // Sorting gives deterministic output independent of the catalog's JSON object order.
    let sorted: std::collections::BTreeMap<_, _> = entries.iter().collect();
    for (id, model) in sorted {
        if !requested.is_empty() && !requested.contains(id.as_str()) {
            continue;
        }
        match parse_price(id, model) {
            Ok(price) => {
                let cost = &model["cost"];
                if cost.get("tiers").is_some() || cost.get("context_over_200k").is_some() {
                    selection.tiered += 1;
                }
                selection.prices.push(price);
            }
            Err(error) if requested.contains(id.as_str()) => return Err(error),
            Err(_) => selection.skipped += 1,
        }
    }
    if selection.prices.is_empty() {
        return Err(AppError::InvalidInput(
            "No supported text model prices found in selection".into(),
        ));
    }
    Ok(selection)
}

fn parse_price(id: &str, model: &Value) -> Result<ModelPricingUpdate, AppError> {
    let invalid = || {
        AppError::InvalidInput(format!(
            "Model {id} has unsupported or invalid text pricing"
        ))
    };
    if model.get("status").and_then(Value::as_str) == Some("deprecated") {
        return Err(invalid());
    }
    let output = model
        .pointer("/modalities/output")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    if output.is_empty() || output.iter().any(|v| v.as_str() != Some("text")) {
        return Err(invalid());
    }
    let cost = model
        .get("cost")
        .and_then(Value::as_object)
        .ok_or_else(invalid)?;
    let number = |field: &str, optional: bool| -> Result<String, AppError> {
        match cost.get(field) {
            None | Some(Value::Null) if optional => Ok("0".into()),
            Some(Value::Number(n)) => {
                let raw = n.to_string();
                let value = raw
                    .parse::<rust_decimal::Decimal>()
                    .or_else(|_| rust_decimal::Decimal::from_scientific(&raw))
                    .map_err(|_| invalid())?;
                if value < rust_decimal::Decimal::ZERO
                    || value >= rust_decimal::Decimal::from(1_000_000_000_000u64)
                {
                    return Err(invalid());
                }
                Ok(value.normalize().to_string())
            }
            _ => Err(invalid()),
        }
    };
    // Keep exact catalog IDs: stripping a vendor prefix could overwrite an unrelated model.
    ModelPricingUpdate::new(
        id,
        model
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(id),
        number("input", false)?,
        number("output", false)?,
        number("cache_read", true)?,
        number("cache_write", true)?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn pricing_sync_download_handles_http_json_and_size_errors() {
        use std::io::{Read, Write};
        for (response, valid) in [
            ("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}", true),
            (
                "HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\n\r\n",
                false,
            ),
            ("HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nbad", false),
            ("HTTP/1.1 200 OK\r\nContent-Length: 33554433\r\n\r\n", false),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = [0; 4096];
                let n = stream.read(&mut request).unwrap();
                assert!(!String::from_utf8_lossy(&request[..n])
                    .to_lowercase()
                    .contains("authorization:"));
                stream.write_all(response.as_bytes()).unwrap();
            });
            let client = reqwest::Client::builder().no_proxy().build().unwrap();
            assert_eq!(fetch_catalog_from(&client, &url).await.is_ok(), valid);
            server.join().unwrap();
        }
    }

    fn model(cost: Value) -> Value {
        json!({"name":"Test", "modalities":{"output":["text"]}, "cost":cost})
    }

    #[test]
    fn pricing_sync_preserves_exact_ids_and_provider_selection() {
        let catalog = json!({"openai":{"models":{"vendor/test": model(json!({"input":2,"output":10,"cache_read":0.1,"tiers":[]}))}}, "other":{"models":{"vendor/test":model(json!({"input":99,"output":99}))}}});
        let selected = select_catalog(&catalog, "openai", &[]).unwrap();
        assert_eq!(selected.prices[0].model_id, "vendor/test");
        assert_eq!(selected.prices[0].input_cost_per_million, "2");
        assert_eq!(selected.prices[0].cache_read_cost_per_million, "0.1");
        assert_eq!(selected.prices[0].cache_creation_cost_per_million, "0");
        assert_eq!(selected.tiered, 1);
        assert!(select_catalog(&catalog, "missing", &[]).is_err());
        assert!(select_catalog(&catalog, "openai", &["missing".into()]).is_err());
    }

    #[test]
    fn pricing_sync_rejects_invalid_costs_but_accepts_free_models() {
        for cost in [
            json!({"input":-1,"output":1}),
            json!({"input":"2","output":1}),
            json!({"input":2}),
            json!({"input":2,"output":1,"cache_read":-1}),
        ] {
            assert!(parse_price("test", &model(cost)).is_err());
        }
        assert_eq!(
            parse_price("free", &model(json!({"input":0,"output":0})))
                .unwrap()
                .input_cost_per_million,
            "0"
        );
    }

    #[test]
    fn pricing_sync_skips_non_text_and_invalid_entries_unless_explicit() {
        let mut audio = model(json!({"input":1,"output":2}));
        audio["modalities"]["output"] = json!(["text", "audio"]);
        let catalog = json!({"p":{"models":{"good":model(json!({"input":1,"output":2})),"audio":audio,"missing":{}}}});
        let selection = select_catalog(&catalog, "p", &[]).unwrap();
        assert_eq!(selection.prices.len(), 1);
        assert_eq!(selection.skipped, 2);
        assert!(select_catalog(&catalog, "p", &["audio".into()]).is_err());
    }
}
