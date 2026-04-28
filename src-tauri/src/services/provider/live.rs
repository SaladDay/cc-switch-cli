use std::collections::HashMap;

use indexmap::IndexMap;
use serde_json::Value;

use crate::app_config::AppType;
use crate::codex_config::{get_codex_auth_path, get_codex_config_path};
use crate::config::{delete_file, get_claude_settings_path, read_json_file, write_json_file};
use crate::error::AppError;
use crate::provider::{Provider, ProviderMeta};
use crate::store::AppState;

#[derive(Clone)]
pub(super) enum LiveSnapshot {
    Claude {
        settings: Option<Value>,
    },
    Codex {
        auth: Option<Value>,
        config: Option<String>,
    },
    Gemini {
        env: Option<HashMap<String, String>>,
        config: Option<Value>,
    },
    OpenCode {
        config: Option<Value>,
    },
    OpenClaw {
        config_source: Option<String>,
    },
}

impl LiveSnapshot {
    pub(super) fn restore(&self) -> Result<(), AppError> {
        match self {
            LiveSnapshot::Claude { settings } => {
                let path = get_claude_settings_path();
                if let Some(value) = settings {
                    write_json_file(&path, value)?;
                } else if path.exists() {
                    delete_file(&path)?;
                }
            }
            LiveSnapshot::Codex { auth, config } => {
                let auth_path = get_codex_auth_path();
                let config_path = get_codex_config_path();
                if let Some(value) = auth {
                    write_json_file(&auth_path, value)?;
                } else if auth_path.exists() {
                    delete_file(&auth_path)?;
                }

                if let Some(text) = config {
                    crate::config::write_text_file(&config_path, text)?;
                } else if config_path.exists() {
                    delete_file(&config_path)?;
                }
            }
            LiveSnapshot::Gemini { env, config } => {
                use crate::gemini_config::{
                    get_gemini_env_path, get_gemini_settings_path, write_gemini_env_atomic,
                };

                let path = get_gemini_env_path();
                if let Some(env_map) = env {
                    write_gemini_env_atomic(env_map)?;
                } else if path.exists() {
                    delete_file(&path)?;
                }

                let settings_path = get_gemini_settings_path();
                match config {
                    Some(cfg) => {
                        write_json_file(&settings_path, cfg)?;
                    }
                    None if settings_path.exists() => {
                        delete_file(&settings_path)?;
                    }
                    _ => {}
                }
            }
            LiveSnapshot::OpenCode { config } => {
                let path = crate::opencode_config::get_opencode_config_path();
                if let Some(value) = config {
                    write_json_file(&path, value)?;
                } else if path.exists() {
                    delete_file(&path)?;
                }
            }
            LiveSnapshot::OpenClaw { config_source } => {
                let path = crate::openclaw_config::get_openclaw_config_path();
                if let Some(source) = config_source {
                    crate::openclaw_config::write_openclaw_config_source(source)?;
                } else if path.exists() {
                    delete_file(&path)?;
                }
            }
        }
        Ok(())
    }
}

pub(super) fn capture_live_snapshot(app_type: &AppType) -> Result<LiveSnapshot, AppError> {
    match app_type {
        AppType::Claude => {
            let path = get_claude_settings_path();
            let settings = if path.exists() {
                Some(read_json_file(&path)?)
            } else {
                None
            };
            Ok(LiveSnapshot::Claude { settings })
        }
        AppType::Codex => {
            let auth_path = get_codex_auth_path();
            let config_path = get_codex_config_path();
            let auth = if auth_path.exists() {
                Some(read_json_file(&auth_path)?)
            } else {
                None
            };
            let config = if config_path.exists() {
                Some(crate::codex_config::read_and_validate_codex_config_text()?)
            } else {
                None
            };
            Ok(LiveSnapshot::Codex { auth, config })
        }
        AppType::Gemini => {
            use crate::gemini_config::{
                get_gemini_env_path, get_gemini_settings_path, read_gemini_env,
            };

            let env_path = get_gemini_env_path();
            let env = if env_path.exists() {
                Some(read_gemini_env()?)
            } else {
                None
            };
            let settings_path = get_gemini_settings_path();
            let config = if settings_path.exists() {
                Some(read_json_file(&settings_path)?)
            } else {
                None
            };
            Ok(LiveSnapshot::Gemini { env, config })
        }
        AppType::OpenCode => {
            let path = crate::opencode_config::get_opencode_config_path();
            let config = if path.exists() {
                Some(crate::opencode_config::read_opencode_config()?)
            } else {
                None
            };
            Ok(LiveSnapshot::OpenCode { config })
        }
        AppType::OpenClaw => {
            let config_source = crate::openclaw_config::read_openclaw_config_source()?;
            Ok(LiveSnapshot::OpenClaw { config_source })
        }
    }
}

pub fn sync_openclaw_providers_from_live(state: &AppState) -> Result<usize, AppError> {
    if !crate::openclaw_config::get_openclaw_config_path().exists() {
        return Ok(0);
    }

    let providers = crate::openclaw_config::get_providers()?;
    let mut live_providers = IndexMap::new();
    for (id, live_provider) in providers {
        if id.trim().is_empty() {
            log::warn!("Skipping OpenClaw live provider with blank id during local mirror");
            continue;
        }

        let config = match super::ProviderService::parse_openclaw_provider_settings(&live_provider)
        {
            Ok(config) => config,
            Err(err) => {
                log::warn!(
                    "Skipping malformed OpenClaw live provider '{id}' during local mirror: {err}"
                );
                continue;
            }
        };

        if let Err(err) = super::ProviderService::validate_openclaw_provider_models(&id, &config) {
            log::warn!(
                "Skipping model-less OpenClaw live provider '{id}' during local mirror: {err}"
            );
            continue;
        }

        if config.models.iter().any(|model| model.id.trim().is_empty()) {
            log::warn!(
                "Skipping OpenClaw live provider '{id}' during local mirror because a model id is blank"
            );
            continue;
        }

        let canonical =
            serde_json::to_value(config).map_err(|source| AppError::JsonSerialize { source })?;
        live_providers.insert(id, canonical);
    }

    let mut changed = 0;
    {
        let mut config = state.config.write().map_err(AppError::from)?;
        config.ensure_app(&AppType::OpenClaw);
        let manager = config
            .get_manager_mut(&AppType::OpenClaw)
            .ok_or_else(|| AppError::Config("OpenClaw manager missing".to_string()))?;

        for (id, live_provider) in live_providers {
            if let Some(existing) = manager.providers.get_mut(&id) {
                let mut provider_changed = false;
                if existing.id != id {
                    existing.id = id.clone();
                    provider_changed = true;
                }
                if is_auto_mirrored_openclaw_snapshot(existing) {
                    if existing.name != id {
                        existing.name = id.clone();
                        provider_changed = true;
                    }
                }
                if existing.settings_config != live_provider {
                    existing.settings_config = live_provider;
                    provider_changed = true;
                }
                if provider_changed {
                    changed += 1;
                }
                continue;
            }

            manager.providers.insert(
                id.clone(),
                Provider::with_id(id.clone(), id.clone(), live_provider, None),
            );
            changed += 1;
        }
    }

    if changed > 0 {
        state.save()?;
    }

    Ok(changed)
}

pub(super) fn is_auto_mirrored_openclaw_snapshot(provider: &Provider) -> bool {
    provider.website_url.is_none()
        && provider.category.is_none()
        && provider.created_at.is_none()
        && provider.sort_index.is_none()
        && provider.notes.is_none()
        && provider
            .meta
            .as_ref()
            .map_or(true, is_default_openclaw_common_config_marker)
        && provider.icon.is_none()
        && provider.icon_color.is_none()
        && !provider.in_failover_queue
}

fn is_default_openclaw_common_config_marker(meta: &ProviderMeta) -> bool {
    meta.apply_common_config == Some(false)
        && meta.codex_official.is_none()
        && meta.custom_endpoints.is_empty()
        && meta.usage_script.is_none()
        && meta.endpoint_auto_select.is_none()
        && meta.is_partner.is_none()
        && meta.partner_promotion_key.is_none()
        && meta.cost_multiplier.is_none()
        && meta.pricing_model_source.is_none()
        && meta.limit_daily_usd.is_none()
        && meta.limit_monthly_usd.is_none()
        && meta.test_config.is_none()
        && meta.proxy_config.is_none()
        && meta.api_format.is_none()
        && meta.prompt_cache_key.is_none()
        && meta.live_config_managed.is_none()
}

pub fn import_openclaw_providers_from_live(state: &AppState) -> Result<usize, AppError> {
    sync_openclaw_providers_from_live(state)
}
