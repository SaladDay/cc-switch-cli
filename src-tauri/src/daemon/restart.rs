//! Restart policy state machine for the supervised worker.
//!
//! Behavior (matches the approved plan):
//!  - Exponential backoff per attempt: 1s, 2s, 4s, 8s, 16s, capped at 30s.
//!  - Circuit-break after `MAX_FAILURES` consecutive failures inside
//!    `WINDOW_SECS`; the daemon should give up, log a fatal trace, clear
//!    `proxy_runtime_session`, and exit.
//!  - The attempt counter (and therefore the next-delay) resets after the
//!    worker has been running continuously for `STABLE_UPTIME_SECS`.
//!
//! The state machine is pure: it takes a "now" instant from the caller and
//! returns the chosen action. That keeps tests deterministic and avoids any
//! reliance on real wall-clock sleeps.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

const BASE_DELAY: Duration = Duration::from_secs(1);
const MAX_DELAY: Duration = Duration::from_secs(30);
const MAX_FAILURES: usize = 5;
const WINDOW: Duration = Duration::from_secs(60);
const STABLE_UPTIME: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Daemon should sleep for `delay` and then respawn the worker.
    Restart { delay: Duration, attempt: u32 },
    /// Daemon should give up, mark proxy as down, and exit.
    GiveUp,
}

#[derive(Debug)]
pub struct RestartPolicy {
    failures: VecDeque<Instant>,
    attempt: u32,
    last_started_at: Option<Instant>,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl RestartPolicy {
    pub fn new() -> Self {
        Self {
            failures: VecDeque::new(),
            attempt: 0,
            last_started_at: None,
        }
    }

    /// Called whenever the daemon is about to start (or restart) the worker.
    pub fn on_worker_started(&mut self, now: Instant) {
        self.last_started_at = Some(now);
    }

    /// Called when the worker has exited abnormally. Returns the next decision.
    pub fn on_worker_exited(&mut self, now: Instant) -> Decision {
        if let Some(started) = self.last_started_at.take() {
            if now.saturating_duration_since(started) >= STABLE_UPTIME {
                self.attempt = 0;
                self.failures.clear();
            }
        }

        self.failures.push_back(now);
        while let Some(front) = self.failures.front() {
            if now.saturating_duration_since(*front) > WINDOW {
                self.failures.pop_front();
            } else {
                break;
            }
        }

        if self.failures.len() >= MAX_FAILURES {
            return Decision::GiveUp;
        }

        let delay = backoff_for(self.attempt);
        let decision = Decision::Restart {
            delay,
            attempt: self.attempt,
        };
        self.attempt = self.attempt.saturating_add(1);
        decision
    }

    #[cfg(test)]
    pub(crate) fn attempt_count(&self) -> u32 {
        self.attempt
    }
}

fn backoff_for(attempt: u32) -> Duration {
    let secs = 1u64.checked_shl(attempt).unwrap_or(u64::MAX);
    let computed = Duration::from_secs(secs.min(MAX_DELAY.as_secs()));
    if computed < BASE_DELAY {
        BASE_DELAY
    } else if computed > MAX_DELAY {
        MAX_DELAY
    } else {
        computed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(secs: u64) -> Instant {
        Instant::now()
            .checked_sub(Duration::from_secs(0))
            .unwrap()
            .checked_add(Duration::from_secs(secs))
            .unwrap()
    }

    #[test]
    fn first_failure_returns_one_second_delay() {
        let mut p = RestartPolicy::new();
        let now = t(0);
        p.on_worker_started(now);
        match p.on_worker_exited(now + Duration::from_secs(1)) {
            Decision::Restart { delay, attempt } => {
                assert_eq!(delay, Duration::from_secs(1));
                assert_eq!(attempt, 0);
            }
            other => panic!("expected Restart, got {other:?}"),
        }
    }

    #[test]
    fn delay_doubles_per_attempt_until_circuit_break() {
        // With MAX_FAILURES = 5, the 5th consecutive failure inside the window
        // gives up, so we only see four Restart decisions before GiveUp.
        let mut p = RestartPolicy::new();
        let now = t(0);
        p.on_worker_started(now);
        let mut delays = Vec::new();
        for i in 0..5 {
            let exit = now + Duration::from_secs(i);
            match p.on_worker_exited(exit) {
                Decision::Restart { delay, .. } => {
                    delays.push(delay);
                    p.on_worker_started(exit);
                }
                Decision::GiveUp => break,
            }
        }
        assert_eq!(
            delays,
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
            ]
        );
    }

    #[test]
    fn long_run_of_failures_caps_delay_at_thirty_seconds() {
        // Force the attempt counter past the 30s cap by spacing failures
        // beyond the 60s window so the circuit doesn't trip.
        let mut p = RestartPolicy::new();
        let mut now = t(0);
        let mut last_delay = Duration::from_secs(0);
        for _ in 0..10 {
            p.on_worker_started(now);
            let exit = now + Duration::from_secs(1);
            match p.on_worker_exited(exit) {
                Decision::Restart { delay, .. } => last_delay = delay,
                Decision::GiveUp => panic!("should not give up when window evicts old failures"),
            }
            // Step ~70s forward so the rolling window evicts the prior failure.
            now = exit + Duration::from_secs(70);
        }
        assert_eq!(last_delay, Duration::from_secs(30));
    }

    #[test]
    fn circuit_breaks_after_five_failures_in_window() {
        let mut p = RestartPolicy::new();
        let start = t(0);
        for i in 0..5 {
            let exit = start + Duration::from_secs(i);
            p.on_worker_started(exit);
            let decision = p.on_worker_exited(exit + Duration::from_millis(100));
            if i < 4 {
                assert!(matches!(decision, Decision::Restart { .. }), "i={i}");
            } else {
                assert_eq!(decision, Decision::GiveUp, "i={i}");
            }
        }
    }

    #[test]
    fn failures_outside_window_do_not_count_toward_circuit_break() {
        let mut p = RestartPolicy::new();
        let start = t(0);
        // 4 failures spread well within the window but separated, so attempt
        // grows but the deque should evict old entries when `now > window`.
        for i in 0..4 {
            let exit = start + Duration::from_secs(i);
            p.on_worker_started(exit);
            assert!(matches!(p.on_worker_exited(exit), Decision::Restart { .. }));
        }
        // Far future: previous failures fall out of the rolling window.
        let later = start + Duration::from_secs(200);
        p.on_worker_started(later);
        let decision = p.on_worker_exited(later + Duration::from_secs(1));
        assert!(matches!(decision, Decision::Restart { .. }));
    }

    #[test]
    fn stable_uptime_resets_attempt_counter() {
        let mut p = RestartPolicy::new();
        let start = t(0);
        for i in 0..3 {
            let exit = start + Duration::from_secs(i);
            p.on_worker_started(exit);
            p.on_worker_exited(exit + Duration::from_millis(50));
        }
        assert_eq!(p.attempt_count(), 3);

        let stable_start = start + Duration::from_secs(1000);
        p.on_worker_started(stable_start);
        let stable_exit = stable_start + STABLE_UPTIME + Duration::from_secs(1);
        match p.on_worker_exited(stable_exit) {
            Decision::Restart { delay, attempt } => {
                assert_eq!(attempt, 0, "stable uptime should reset attempt counter");
                assert_eq!(delay, Duration::from_secs(1));
            }
            other => panic!("expected Restart, got {other:?}"),
        }
    }
}
