use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value;

use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::cli::tui::data::QuotaTarget;
use crate::provider::Provider;
use crate::services::{EndpointLatency, HealthStatus, StreamCheckResult, SyncDecision};

use super::super::form::ProviderAddField;

pub(crate) fn next_model_fetch_request_id() -> u64 {
    static NEXT_MODEL_FETCH_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
    NEXT_MODEL_FETCH_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

pub(crate) enum SpeedtestMsg {
    Finished {
        url: String,
        result: Result<Vec<EndpointLatency>, String>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct StreamCheckReq {
    pub(crate) app_type: AppType,
    pub(crate) provider_id: String,
    pub(crate) provider_name: String,
    pub(crate) provider: Provider,
}

pub(crate) enum StreamCheckMsg {
    Finished {
        req: StreamCheckReq,
        result: Result<StreamCheckResult, String>,
    },
}

pub(crate) enum LocalEnvReq {
    Refresh,
}

pub(crate) enum LocalEnvMsg {
    Finished {
        result: Vec<crate::services::local_env_check::ToolCheckResult>,
    },
}

pub(crate) enum QuotaReq {
    Refresh { target: QuotaTarget },
}

pub(crate) enum QuotaMsg {
    Finished {
        target: QuotaTarget,
        result: Result<crate::services::SubscriptionQuota, String>,
    },
}

pub(crate) enum SkillsReq {
    Discover { query: String },
    Install { spec: String, app: AppType },
}

pub(crate) enum SkillsMsg {
    DiscoverFinished {
        query: String,
        result: Result<Vec<crate::services::skill::Skill>, String>,
    },
    InstallFinished {
        spec: String,
        result: Result<crate::services::skill::InstalledSkill, String>,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum WebDavReqKind {
    CheckConnection,
    Upload,
    Download,
    MigrateV1ToV2,
    JianguoyunQuickSetup { username: String, password: String },
}

#[derive(Debug, Clone)]
pub(crate) struct WebDavReq {
    pub(crate) request_id: u64,
    pub(crate) kind: WebDavReqKind,
}

#[derive(Debug, Clone)]
pub(crate) enum WebDavDone {
    ConnectionChecked,
    Uploaded {
        decision: SyncDecision,
        message: String,
    },
    Downloaded {
        decision: SyncDecision,
        message: String,
    },
    V1Migrated {
        message: String,
    },
    JianguoyunConfigured,
}

#[derive(Debug, Clone)]
pub(crate) enum WebDavErr {
    Generic(String),
    QuickSetupSave(String),
    QuickSetupCheck(String),
}

pub(crate) enum WebDavMsg {
    Finished {
        request_id: u64,
        req: WebDavReqKind,
        result: Result<WebDavDone, WebDavErr>,
    },
}

pub(crate) struct SpeedtestSystem {
    pub(crate) req_tx: mpsc::Sender<String>,
    pub(crate) result_rx: mpsc::Receiver<SpeedtestMsg>,
    pub(crate) _handle: std::thread::JoinHandle<()>,
}

pub(crate) struct StreamCheckSystem {
    pub(crate) req_tx: mpsc::Sender<StreamCheckReq>,
    pub(crate) result_rx: mpsc::Receiver<StreamCheckMsg>,
    pub(crate) _handle: std::thread::JoinHandle<()>,
}

pub(crate) struct LocalEnvSystem {
    pub(crate) req_tx: mpsc::Sender<LocalEnvReq>,
    pub(crate) result_rx: mpsc::Receiver<LocalEnvMsg>,
    pub(crate) _handle: std::thread::JoinHandle<()>,
}

pub(crate) struct QuotaSystem {
    pub(crate) req_tx: mpsc::Sender<QuotaReq>,
    pub(crate) result_rx: mpsc::Receiver<QuotaMsg>,
    pub(crate) _handle: std::thread::JoinHandle<()>,
}

#[derive(Debug, Clone)]
pub(crate) enum ProxyReq {
    SetManagedSessionForCurrentApp {
        request_id: u64,
        app_type: AppType,
        enabled: bool,
    },
}

pub(crate) enum ProxyMsg {
    ManagedSessionFinished {
        request_id: u64,
        app_type: AppType,
        enabled: bool,
        result: Result<(), String>,
    },
}

pub(crate) struct ProxySystem {
    pub(crate) req_tx: mpsc::Sender<ProxyReq>,
    pub(crate) result_rx: mpsc::Receiver<ProxyMsg>,
    pub(crate) _handle: std::thread::JoinHandle<()>,
}

pub(crate) struct SkillsSystem {
    pub(crate) req_tx: mpsc::Sender<SkillsReq>,
    pub(crate) result_rx: mpsc::Receiver<SkillsMsg>,
    pub(crate) _handle: std::thread::JoinHandle<()>,
}

pub(crate) struct WebDavSystem {
    pub(crate) req_tx: mpsc::Sender<WebDavReq>,
    pub(crate) result_rx: mpsc::Receiver<WebDavMsg>,
    pub(crate) _handle: std::thread::JoinHandle<()>,
}

pub(crate) enum UpdateReq {
    Check { request_id: u64 },
    Download,
}

pub(crate) enum UpdateMsg {
    CheckFinished {
        request_id: u64,
        result: Result<crate::cli::commands::update::UpdateCheckInfo, String>,
    },
    DownloadProgress {
        downloaded: u64,
        total: Option<u64>,
    },
    DownloadFinished(Result<String, String>),
}

pub(crate) struct UpdateSystem {
    pub(crate) req_tx: mpsc::Sender<UpdateReq>,
    pub(crate) result_rx: mpsc::Receiver<UpdateMsg>,
    pub(crate) _handle: std::thread::JoinHandle<()>,
}

pub(crate) enum ModelFetchReq {
    Fetch {
        request_id: u64,
        base_url: String,
        api_key: Option<String>,
        field: ProviderAddField,
        claude_idx: Option<usize>,
    },
}

pub(crate) enum ModelFetchMsg {
    Finished {
        request_id: u64,
        field: ProviderAddField,
        claude_idx: Option<usize>,
        result: Result<Vec<String>, String>,
    },
}

pub(crate) struct ModelFetchSystem {
    pub(crate) req_tx: mpsc::Sender<ModelFetchReq>,
    pub(crate) result_rx: mpsc::Receiver<ModelFetchMsg>,
    pub(crate) _handle: std::thread::JoinHandle<()>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelFetchStrategy {
    Bearer,
    Anthropic,
    GoogleApiKey,
}

pub(crate) fn model_fetch_strategy_for_field(field: ProviderAddField) -> ModelFetchStrategy {
    match field {
        ProviderAddField::GeminiModel => ModelFetchStrategy::GoogleApiKey,
        ProviderAddField::ClaudeModelConfig => ModelFetchStrategy::Anthropic,
        _ => ModelFetchStrategy::Bearer,
    }
}

pub(crate) fn build_model_fetch_candidate_urls(
    base_url: &str,
    strategy: ModelFetchStrategy,
) -> Vec<String> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Vec::new();
    }
    if base.ends_with("/models") {
        return vec![base.to_string()];
    }

    let append_models = format!("{base}/models");
    let append_v1_models = if base.ends_with("/v1") || base.ends_with("/v1beta") {
        None
    } else {
        Some(format!("{base}/v1/models"))
    };

    let mut urls: Vec<String> = Vec::new();
    match strategy {
        ModelFetchStrategy::Anthropic => {
            if let Some(v1) = append_v1_models.as_ref() {
                urls.push(v1.clone());
            }
            urls.push(append_models);
        }
        ModelFetchStrategy::Bearer | ModelFetchStrategy::GoogleApiKey => {
            urls.push(append_models);
            if let Some(v1) = append_v1_models.as_ref() {
                urls.push(v1.clone());
            }
        }
    }

    let mut seen = HashSet::new();
    urls.retain(|url| seen.insert(url.clone()));
    urls
}

pub(crate) fn parse_model_ids_from_response(payload: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    if let Some(data) = payload.get("data").and_then(|v| v.as_array()) {
        for item in data {
            if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                out.push(id.to_string());
            }
        }
    }

    if out.is_empty() {
        if let Some(models) = payload.get("models").and_then(|v| v.as_array()) {
            for item in models {
                if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                    out.push(name.strip_prefix("models/").unwrap_or(name).to_string());
                }
            }
        }
    }

    if out.is_empty() {
        if let Some(arr) = payload.as_array() {
            for item in arr {
                if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                    out.push(id.to_string());
                }
            }
        }
    }

    let mut seen = HashSet::new();
    out.retain(|model| seen.insert(model.clone()));
    out
}

pub(crate) async fn fetch_provider_models_for_tui(
    base_url: &str,
    api_key: Option<&str>,
    strategy: ModelFetchStrategy,
) -> Result<Vec<String>, String> {
    let candidate_urls = build_model_fetch_candidate_urls(base_url, strategy);
    if candidate_urls.is_empty() {
        return Err("URL cannot be empty".to_string());
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| format!("build http client failed: {e}"))?;

    let key = api_key.map(str::trim).filter(|k| !k.is_empty());
    let mut last_err = String::from("unknown error");

    for url in candidate_urls {
        let mut req = client.get(&url);
        if let Some(key) = key {
            req = match strategy {
                ModelFetchStrategy::Bearer => req.header("Authorization", format!("Bearer {key}")),
                ModelFetchStrategy::Anthropic => req
                    .header("Authorization", format!("Bearer {key}"))
                    .header("x-api-key", key)
                    .header("anthropic-version", "2023-06-01"),
                ModelFetchStrategy::GoogleApiKey => req.header("x-goog-api-key", key),
            };
        }

        match req.send().await {
            Ok(resp) => {
                if !resp.status().is_success() {
                    last_err = format!("HTTP {} ({url})", resp.status());
                    continue;
                }
                match resp.json::<Value>().await {
                    Ok(payload) => {
                        let models = parse_model_ids_from_response(&payload);
                        if models.is_empty() {
                            last_err = format!("No model list found in response ({url})");
                        } else {
                            return Ok(models);
                        }
                    }
                    Err(err) => {
                        last_err = format!("Invalid JSON response ({url}): {err}");
                    }
                }
            }
            Err(err) => {
                last_err = err.to_string();
            }
        }
    }

    Err(last_err)
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct RequestTracker {
    pub(crate) seq: u64,
    pub(crate) active: Option<u64>,
}

impl RequestTracker {
    pub(crate) fn start(&mut self) -> u64 {
        self.seq = self.seq.wrapping_add(1);
        self.active = Some(self.seq);
        self.seq
    }

    pub(crate) fn cancel(&mut self) {
        self.active = None;
    }

    pub(crate) fn is_stale(&self, request_id: u64) -> bool {
        matches!(self.active, Some(active_request_id) if active_request_id != request_id)
    }

    pub(crate) fn finish_if_active(&mut self, request_id: u64) -> bool {
        if self.active == Some(request_id) {
            self.active = None;
            true
        } else {
            false
        }
    }
}

fn stream_check_status_label(status: &HealthStatus) -> &'static str {
    match status {
        HealthStatus::Operational => texts::tui_stream_check_status_operational(),
        HealthStatus::Degraded => texts::tui_stream_check_status_degraded(),
        HealthStatus::Failed => texts::tui_stream_check_status_failed(),
    }
}

pub(crate) fn build_stream_check_result_lines(
    provider_name: &str,
    result: &StreamCheckResult,
) -> Vec<String> {
    let response_time = result
        .response_time_ms
        .map(|ms| texts::tui_latency_ms(ms as u128))
        .unwrap_or_else(|| texts::tui_na().to_string());
    let http_status = result
        .http_status
        .map(|status| status.to_string())
        .unwrap_or_else(|| texts::tui_na().to_string());
    let model = if result.model_used.trim().is_empty() {
        texts::tui_na().to_string()
    } else {
        result.model_used.clone()
    };

    vec![
        texts::tui_stream_check_line_provider(provider_name),
        texts::tui_stream_check_line_status(stream_check_status_label(&result.status)),
        texts::tui_stream_check_line_response_time(&response_time),
        texts::tui_stream_check_line_http_status(&http_status),
        texts::tui_stream_check_line_model(&model),
        texts::tui_stream_check_line_retries(&result.retry_count.to_string()),
        texts::tui_stream_check_line_message(&result.message),
    ]
}
