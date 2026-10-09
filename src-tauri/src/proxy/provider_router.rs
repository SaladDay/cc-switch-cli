use std::{collections::HashMap, str::FromStr, sync::Arc};

use tokio::sync::RwLock;

use crate::{app_config::AppType, database::Database, provider::Provider};

mod route_selection;
mod upstream_endpoint;

use route_selection::SmoothWrrState;

use super::{
    circuit_breaker::{AllowResult, CircuitBreaker, CircuitBreakerConfig, CircuitBreakerStats},
    error::ProxyError,
};

pub struct ProviderRouter {
    db: Arc<Database>,
    circuit_breakers: Arc<RwLock<HashMap<String, Arc<CircuitBreaker>>>>,
    /// Route mode smooth weighted round-robin state (app → state)
    wrr: Arc<tokio::sync::Mutex<HashMap<String, SmoothWrrState>>>,
}

impl ProviderRouter {
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            circuit_breakers: Arc::new(RwLock::new(HashMap::new())),
            wrr: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }

    pub async fn select_providers(&self, app_type: &str) -> Result<Vec<Provider>, ProxyError> {
        let mut result = Vec::new();
        let mut total_providers = 0usize;
        let mut circuit_open_count = 0usize;

        // Route mode takes priority: while enabled the failover queue does
        // not participate (mutually exclusive modes)
        let route_group = self
            .db
            .get_route_mode_group(app_type)
            .map_err(|error| ProxyError::DatabaseError(error.to_string()))?;

        if route_group.enabled {
            return self.select_route_providers(app_type, &route_group).await;
        }

        let auto_failover_enabled = self
            .db
            .get_proxy_config_for_app(app_type)
            .await
            .map(|config| config.auto_failover_enabled)
            .unwrap_or(false);

        if auto_failover_enabled {
            let all_providers = self
                .db
                .get_all_providers(app_type)
                .map_err(|error| ProxyError::DatabaseError(error.to_string()))?;
            let ordered_ids = self
                .db
                .get_failover_queue(app_type)
                .map_err(|error| ProxyError::DatabaseError(error.to_string()))?
                .into_iter()
                .map(|item| item.provider_id)
                .collect::<Vec<_>>();

            total_providers = ordered_ids.len();

            for provider_id in ordered_ids {
                let Some(provider) = all_providers.get(&provider_id).cloned() else {
                    continue;
                };

                let breaker = self
                    .get_or_create_circuit_breaker(&format!("{app_type}:{}", provider.id))
                    .await;

                if breaker.is_available().await {
                    result.push(provider);
                } else {
                    circuit_open_count += 1;
                }
            }
        } else if let Some(current) = self.current_provider(app_type)? {
            total_providers = 1;
            result.push(current);
        }

        if result.is_empty() {
            return if total_providers > 0 && circuit_open_count == total_providers {
                Err(ProxyError::AllProvidersCircuitOpen)
            } else {
                Err(ProxyError::NoProvidersConfigured)
            };
        }

        Ok(result)
    }

    /// Route mode selection: smooth weighted round-robin picks the primary
    /// member; the remaining healthy members follow in stored order so an
    /// in-request failure reuses the forwarder's existing skip-on-failure
    /// replay
    async fn select_route_providers(
        &self,
        app_type: &str,
        route_group: &crate::database::RouteModeGroup,
    ) -> Result<Vec<Provider>, ProxyError> {
        let all_providers = self
            .db
            .get_all_providers(app_type)
            .map_err(|error| ProxyError::DatabaseError(error.to_string()))?;

        // Resolve members: dangling or disabled members are dropped
        let mut candidates = Vec::new();
        let mut candidate_weights = Vec::new();
        for member in &route_group.members {
            if !member.enabled {
                continue;
            }
            let Some(provider) = all_providers.get(&member.provider_id) else {
                continue;
            };
            candidates.push(provider.clone());
            candidate_weights.push((provider.id.clone(), member.weight));
        }
        let total_members = candidates.len();

        // Circuit-breaker filter, consistent with the failover branch
        let mut healthy = Vec::new();
        let mut healthy_weights = Vec::new();
        for (provider, weight) in candidates.into_iter().zip(candidate_weights) {
            let breaker = self
                .get_or_create_circuit_breaker(&format!("{app_type}:{}", provider.id))
                .await;

            if breaker.is_available().await {
                healthy.push(provider);
                healthy_weights.push(weight);
            }
        }

        if healthy.is_empty() {
            return if total_members > 0 {
                Err(ProxyError::AllProvidersCircuitOpen)
            } else {
                Err(ProxyError::NoProvidersConfigured)
            };
        }

        let winner = {
            let mut wrr = self.wrr.lock().await;
            let state = wrr.entry(app_type.to_string()).or_default();
            state.pick(&healthy_weights)
        };

        let mut ordered = Vec::with_capacity(healthy.len());
        for (index, provider) in healthy.into_iter().enumerate() {
            if index == winner {
                ordered.insert(0, provider);
            } else {
                ordered.push(provider);
            }
        }
        Ok(ordered)
    }

    pub async fn allow_provider_request(&self, provider_id: &str, app_type: &str) -> AllowResult {
        let breaker = self
            .get_or_create_circuit_breaker(&format!("{app_type}:{provider_id}"))
            .await;
        breaker.allow_request().await
    }

    pub async fn record_result(
        &self,
        provider_id: &str,
        app_type: &str,
        used_half_open_permit: bool,
        success: bool,
        error_msg: Option<String>,
    ) -> Result<(), ProxyError> {
        let failure_threshold = self
            .db
            .get_proxy_config_for_app(app_type)
            .await
            .map(|config| config.circuit_failure_threshold)
            .unwrap_or(5);

        let breaker = self
            .get_or_create_circuit_breaker(&format!("{app_type}:{provider_id}"))
            .await;

        if success {
            breaker.record_success(used_half_open_permit).await;
        } else {
            breaker.record_failure(used_half_open_permit).await;
        }

        self.db
            .update_provider_health_with_threshold(
                provider_id,
                app_type,
                success,
                error_msg,
                failure_threshold,
            )
            .await
            .map_err(|error| ProxyError::DatabaseError(error.to_string()))
    }

    pub async fn reset_circuit_breaker(&self, circuit_key: &str) {
        let breakers = self.circuit_breakers.read().await;
        if let Some(breaker) = breakers.get(circuit_key) {
            breaker.reset().await;
        }
    }

    pub async fn reset_provider_breaker(&self, provider_id: &str, app_type: &str) {
        self.reset_circuit_breaker(&format!("{app_type}:{provider_id}"))
            .await;
    }

    pub async fn release_permit_neutral(
        &self,
        provider_id: &str,
        app_type: &str,
        used_half_open_permit: bool,
    ) {
        if !used_half_open_permit {
            return;
        }

        let breaker = self
            .get_or_create_circuit_breaker(&format!("{app_type}:{provider_id}"))
            .await;
        breaker.release_half_open_permit();
    }

    pub async fn update_all_configs(&self, config: CircuitBreakerConfig) {
        let breakers = self.circuit_breakers.read().await;
        for breaker in breakers.values() {
            breaker.update_config(config.clone()).await;
        }
    }

    #[allow(dead_code)]
    pub async fn get_circuit_breaker_stats(
        &self,
        provider_id: &str,
        app_type: &str,
    ) -> Option<CircuitBreakerStats> {
        let circuit_key = format!("{app_type}:{provider_id}");
        let breakers = self.circuit_breakers.read().await;
        if let Some(breaker) = breakers.get(&circuit_key) {
            Some(breaker.get_stats().await)
        } else {
            None
        }
    }

    pub(super) fn upstream_endpoint(
        &self,
        app_type: &AppType,
        provider: &Provider,
        endpoint: &str,
    ) -> String {
        upstream_endpoint::rewrite_upstream_endpoint(app_type, provider, endpoint)
    }

    fn current_provider_id(&self, app_type: &AppType) -> Option<String> {
        let _ = crate::settings::reload_settings();
        crate::settings::get_effective_current_provider(&self.db, app_type)
            .ok()
            .flatten()
    }

    fn current_provider(&self, app_type: &str) -> Result<Option<Provider>, ProxyError> {
        let current_id = AppType::from_str(app_type)
            .ok()
            .and_then(|app_enum| self.current_provider_id(&app_enum))
            .or_else(|| self.db.get_current_provider(app_type).ok().flatten());

        match current_id {
            Some(current_id) => self
                .db
                .get_provider_by_id(&current_id, app_type)
                .map_err(|error| ProxyError::DatabaseError(error.to_string())),
            None => Ok(None),
        }
    }

    async fn get_or_create_circuit_breaker(&self, key: &str) -> Arc<CircuitBreaker> {
        {
            let breakers = self.circuit_breakers.read().await;
            if let Some(breaker) = breakers.get(key) {
                return breaker.clone();
            }
        }

        let mut breakers = self.circuit_breakers.write().await;
        if let Some(breaker) = breakers.get(key) {
            return breaker.clone();
        }

        let app_type = key.split(':').next().unwrap_or("claude");
        let config = self
            .db
            .get_proxy_config_for_app(app_type)
            .await
            .map(|app_config| CircuitBreakerConfig {
                failure_threshold: app_config.circuit_failure_threshold,
                success_threshold: app_config.circuit_success_threshold,
                timeout_seconds: app_config.circuit_timeout_seconds as u64,
                error_rate_threshold: app_config.circuit_error_rate_threshold,
                min_requests: app_config.circuit_min_requests,
            })
            .unwrap_or_default();

        let breaker = Arc::new(CircuitBreaker::new(config));
        breakers.insert(key.to_string(), breaker.clone());
        breaker
    }
}

#[cfg(test)]
mod tests;
