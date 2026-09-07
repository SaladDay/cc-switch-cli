use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

impl std::fmt::Display for CircuitState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CircuitState::Closed => write!(f, "closed"),
            CircuitState::Open => write!(f, "open"),
            CircuitState::HalfOpen => write!(f, "half_open"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CircuitBreakerConfig {
    pub failure_threshold: u32,
    pub success_threshold: u32,
    pub timeout_seconds: u64,
    pub error_rate_threshold: f64,
    pub min_requests: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 4,
            success_threshold: 2,
            timeout_seconds: 60,
            error_rate_threshold: 0.6,
            min_requests: 10,
        }
    }
}

pub struct CircuitBreaker {
    state: Arc<RwLock<CircuitState>>,
    consecutive_failures: Arc<AtomicU32>,
    consecutive_successes: Arc<AtomicU32>,
    total_requests: Arc<AtomicU32>,
    failed_requests: Arc<AtomicU32>,
    last_opened_at: Arc<RwLock<Option<Instant>>>,
    config: Arc<RwLock<CircuitBreakerConfig>>,
    half_open_permits: Arc<Mutex<HalfOpenPermitState>>,
}

#[derive(Default)]
struct HalfOpenPermitState {
    generation: u64,
    in_flight: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct AllowResult {
    pub allowed: bool,
    pub used_half_open_permit: bool,
    pub(super) half_open_generation: Option<u64>,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            state: Arc::new(RwLock::new(CircuitState::Closed)),
            consecutive_failures: Arc::new(AtomicU32::new(0)),
            consecutive_successes: Arc::new(AtomicU32::new(0)),
            total_requests: Arc::new(AtomicU32::new(0)),
            failed_requests: Arc::new(AtomicU32::new(0)),
            last_opened_at: Arc::new(RwLock::new(None)),
            config: Arc::new(RwLock::new(config)),
            half_open_permits: Arc::new(Mutex::new(HalfOpenPermitState::default())),
        }
    }

    pub async fn update_config(&self, new_config: CircuitBreakerConfig) {
        *self.config.write().await = new_config;
    }

    pub async fn is_available(&self) -> bool {
        let state = *self.state.read().await;
        let config = self.config.read().await;

        match state {
            CircuitState::Closed | CircuitState::HalfOpen => true,
            CircuitState::Open => {
                if let Some(opened_at) = *self.last_opened_at.read().await {
                    if opened_at.elapsed().as_secs() >= config.timeout_seconds {
                        drop(config);
                        self.transition_to_half_open().await;
                        return true;
                    }
                }
                false
            }
        }
    }

    pub async fn allow_request(&self) -> AllowResult {
        let state = *self.state.read().await;

        match state {
            CircuitState::Closed => AllowResult {
                allowed: true,
                used_half_open_permit: false,
                half_open_generation: None,
            },
            CircuitState::Open => {
                let config = self.config.read().await;
                if let Some(opened_at) = *self.last_opened_at.read().await {
                    if opened_at.elapsed().as_secs() >= config.timeout_seconds {
                        drop(config);
                        self.transition_to_half_open().await;

                        return match *self.state.read().await {
                            CircuitState::Closed => AllowResult {
                                allowed: true,
                                used_half_open_permit: false,
                                half_open_generation: None,
                            },
                            CircuitState::HalfOpen => self.allow_half_open_probe(),
                            CircuitState::Open => AllowResult {
                                allowed: false,
                                used_half_open_permit: false,
                                half_open_generation: None,
                            },
                        };
                    }
                }

                AllowResult {
                    allowed: false,
                    used_half_open_permit: false,
                    half_open_generation: None,
                }
            }
            CircuitState::HalfOpen => self.allow_half_open_probe(),
        }
    }

    pub async fn record_success(&self, used_half_open_permit: bool) {
        let state = *self.state.read().await;
        let config = self.config.read().await;

        if used_half_open_permit {
            self.release_half_open_permit();
        }

        self.consecutive_failures.store(0, Ordering::SeqCst);
        self.total_requests.fetch_add(1, Ordering::SeqCst);

        if state == CircuitState::HalfOpen {
            let successes = self.consecutive_successes.fetch_add(1, Ordering::SeqCst) + 1;
            if successes >= config.success_threshold {
                drop(config);
                self.transition_to_closed().await;
            }
        }
    }

    pub async fn record_failure(&self, used_half_open_permit: bool) {
        let state = *self.state.read().await;
        let config = self.config.read().await;

        if used_half_open_permit {
            self.release_half_open_permit();
        }

        let failures = self.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;
        self.total_requests.fetch_add(1, Ordering::SeqCst);
        self.failed_requests.fetch_add(1, Ordering::SeqCst);
        self.consecutive_successes.store(0, Ordering::SeqCst);

        match state {
            CircuitState::HalfOpen => {
                drop(config);
                self.transition_to_open().await;
            }
            CircuitState::Closed => {
                if failures >= config.failure_threshold {
                    drop(config);
                    self.transition_to_open().await;
                } else {
                    let total = self.total_requests.load(Ordering::SeqCst);
                    let failed = self.failed_requests.load(Ordering::SeqCst);

                    if total >= config.min_requests {
                        let error_rate = failed as f64 / total as f64;
                        if error_rate >= config.error_rate_threshold {
                            drop(config);
                            self.transition_to_open().await;
                        }
                    }
                }
            }
            CircuitState::Open => {}
        }
    }

    #[allow(dead_code)]
    pub async fn get_state(&self) -> CircuitState {
        *self.state.read().await
    }

    #[allow(dead_code)]
    pub async fn get_stats(&self) -> CircuitBreakerStats {
        CircuitBreakerStats {
            state: *self.state.read().await,
            consecutive_failures: self.consecutive_failures.load(Ordering::SeqCst),
            consecutive_successes: self.consecutive_successes.load(Ordering::SeqCst),
            total_requests: self.total_requests.load(Ordering::SeqCst),
            failed_requests: self.failed_requests.load(Ordering::SeqCst),
        }
    }

    #[allow(dead_code)]
    pub async fn reset(&self) {
        self.transition_to_closed().await;
    }

    fn allow_half_open_probe(&self) -> AllowResult {
        let max_half_open_requests = 1u32;
        let mut permits = self
            .half_open_permits
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if permits.in_flight < max_half_open_requests {
            permits.in_flight += 1;
            AllowResult {
                allowed: true,
                used_half_open_permit: true,
                half_open_generation: Some(permits.generation),
            }
        } else {
            AllowResult {
                allowed: false,
                used_half_open_permit: false,
                half_open_generation: None,
            }
        }
    }

    pub fn release_half_open_permit(&self) {
        let mut permits = self
            .half_open_permits
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        permits.in_flight = permits.in_flight.saturating_sub(1);
    }

    /// 仅归还指定半开代际的许可，避免旧请求释放新一轮探测名额。
    pub(super) fn release_half_open_permit_for_generation(&self, generation: u64) {
        let mut permits = self
            .half_open_permits
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if permits.generation == generation {
            permits.in_flight = permits.in_flight.saturating_sub(1);
        }
    }

    async fn transition_to_open(&self) {
        *self.state.write().await = CircuitState::Open;
        *self.last_opened_at.write().await = Some(Instant::now());
        self.consecutive_failures.store(0, Ordering::SeqCst);
        self.consecutive_successes.store(0, Ordering::SeqCst);
    }

    async fn transition_to_half_open(&self) {
        let mut state = self.state.write().await;
        if *state != CircuitState::Open {
            return;
        }

        let mut permits = self
            .half_open_permits
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        permits.generation = permits.generation.wrapping_add(1);
        permits.in_flight = 0;
        self.consecutive_successes.store(0, Ordering::SeqCst);
        *state = CircuitState::HalfOpen;
    }

    async fn transition_to_closed(&self) {
        *self.state.write().await = CircuitState::Closed;
        self.consecutive_failures.store(0, Ordering::SeqCst);
        self.consecutive_successes.store(0, Ordering::SeqCst);
        self.total_requests.store(0, Ordering::SeqCst);
        self.failed_requests.store(0, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CircuitBreakerStats {
    pub state: CircuitState,
    pub consecutive_failures: u32,
    pub consecutive_successes: u32,
    pub total_requests: u32,
    pub failed_requests: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn transitions_from_closed_to_open_after_threshold_failures() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 3,
            ..Default::default()
        });

        assert_eq!(breaker.get_state().await, CircuitState::Closed);
        assert!(breaker.allow_request().await.allowed);

        for _ in 0..3 {
            breaker.record_failure(false).await;
        }

        assert_eq!(breaker.get_state().await, CircuitState::Open);
        assert!(!breaker.allow_request().await.allowed);
    }

    #[tokio::test]
    async fn closes_again_after_half_open_success_threshold() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 2,
            success_threshold: 2,
            ..Default::default()
        });

        breaker.record_failure(false).await;
        breaker.record_failure(false).await;
        assert_eq!(breaker.get_state().await, CircuitState::Open);

        breaker.transition_to_half_open().await;
        assert_eq!(breaker.get_state().await, CircuitState::HalfOpen);

        breaker.record_success(false).await;
        breaker.record_success(false).await;

        assert_eq!(breaker.get_state().await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn repeated_half_open_transition_keeps_inflight_probe_reserved() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig {
            timeout_seconds: 0,
            ..Default::default()
        });

        breaker.transition_to_open().await;
        let first = breaker.allow_request().await;
        assert!(first.allowed);
        assert!(first.used_half_open_permit);
        assert_eq!(breaker.get_state().await, CircuitState::HalfOpen);

        breaker.transition_to_half_open().await;

        let second = breaker.allow_request().await;
        assert!(!second.allowed);
        assert!(!second.used_half_open_permit);
    }

    #[tokio::test]
    async fn reset_restores_closed_state() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 2,
            ..Default::default()
        });

        breaker.record_failure(false).await;
        breaker.record_failure(false).await;
        assert_eq!(breaker.get_state().await, CircuitState::Open);

        breaker.reset().await;

        assert_eq!(breaker.get_state().await, CircuitState::Closed);
        assert!(breaker.allow_request().await.allowed);
    }
}
