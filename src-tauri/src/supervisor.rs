//! Restart budget shared by the background supervisors.
//!
//! The Telegram proxy and the VPN core both die on their own now and then, and
//! both are brought back by the same arithmetic: a short list of growing delays
//! and a window that forgives a service which stayed alive for a while. The
//! policy is pure — its only inputs are the current time and the restarts
//! already made — which is what makes it testable without any processes.

use std::collections::VecDeque;
use std::time::Duration;

/// Delays before each automatic restart, indexed by the restarts already made.
pub const RESTART_DELAYS_SECS: [u64; 5] = [2, 5, 15, 30, 60];

/// Restart budget window: at most [`RESTART_DELAYS_SECS`] restarts inside it.
pub const RESTART_WINDOW_SECS: u64 = 600;

/// What the supervisor does after the service died or stopped answering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartDecision {
    /// Wait `delay`, then start the service again. `attempt` is 1-based.
    Retry { attempt: u32, delay: Duration },
    /// The budget for the current window is spent: report `failed` and stop
    /// instead of looping forever.
    GiveUp { attempts: u32 },
}

/// Restart budget of the supervisor.
#[derive(Debug, Default)]
pub struct RestartPolicy {
    /// Unix time of each restart in the current window, oldest first.
    history: VecDeque<u64>,
}

impl RestartPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    /// A manual start or stop begins a fresh cycle.
    pub fn reset(&mut self) {
        self.history.clear();
    }

    /// Decides what to do about an unexpected exit observed at `now`.
    ///
    /// Delays grow as 2, 5, 15, 30 and 60 seconds; a sixth failure inside ten
    /// minutes gives up.
    pub fn next_attempt(&mut self, now: u64) -> RestartDecision {
        self.forget_outside_window(now);
        let index = self.history.len();
        let Some(delay) = RESTART_DELAYS_SECS.get(index) else {
            return RestartDecision::GiveUp {
                attempts: index as u32,
            };
        };
        self.history.push_back(now);
        RestartDecision::Retry {
            attempt: index as u32 + 1,
            delay: Duration::from_secs(*delay),
        }
    }

    /// Drops restarts older than the window: a service that ran for ten minutes
    /// is healthy again, so it earns a full budget.
    fn forget_outside_window(&mut self, now: u64) {
        while let Some(at) = self.history.front() {
            if now.saturating_sub(*at) < RESTART_WINDOW_SECS {
                break;
            }
            self.history.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_policy_backs_off_then_gives_up() {
        let mut policy = RestartPolicy::new();
        let expected = [2u64, 5, 15, 30, 60];
        let mut now = 1_000;
        for (index, seconds) in expected.iter().enumerate() {
            assert_eq!(
                policy.next_attempt(now),
                RestartDecision::Retry {
                    attempt: index as u32 + 1,
                    delay: Duration::from_secs(*seconds),
                }
            );
            // The service dies again right away.
            now += 1;
        }
        // Five attempts spent: no sixth restart, ever.
        assert_eq!(
            policy.next_attempt(now),
            RestartDecision::GiveUp { attempts: 5 }
        );
        assert_eq!(
            policy.next_attempt(now + 1),
            RestartDecision::GiveUp { attempts: 5 }
        );
    }

    #[test]
    fn restart_policy_forgets_attempts_outside_the_window() {
        let mut policy = RestartPolicy::new();
        for step in 0..5u64 {
            assert!(matches!(
                policy.next_attempt(1_000 + step),
                RestartDecision::Retry { .. }
            ));
        }
        assert!(matches!(
            policy.next_attempt(1_004),
            RestartDecision::GiveUp { .. }
        ));

        // The window slides one entry at a time: at t=1600 only the first
        // restart is old enough to forget, so the fifth delay is next.
        assert_eq!(
            policy.next_attempt(1_000 + RESTART_WINDOW_SECS),
            RestartDecision::Retry {
                attempt: 5,
                delay: Duration::from_secs(60),
            }
        );
        // Ten seconds later the whole old history is outside the window and
        // only the restart from t=1600 still counts.
        assert_eq!(
            policy.next_attempt(1_010 + RESTART_WINDOW_SECS),
            RestartDecision::Retry {
                attempt: 2,
                delay: Duration::from_secs(5),
            }
        );
    }

    #[test]
    fn manual_start_and_stop_restore_the_budget() {
        let mut policy = RestartPolicy::new();
        for step in 0..5u64 {
            let _ = policy.next_attempt(2_000 + step);
        }
        assert!(matches!(
            policy.next_attempt(2_010),
            RestartDecision::GiveUp { .. }
        ));
        policy.reset();
        assert_eq!(
            policy.next_attempt(2_020),
            RestartDecision::Retry {
                attempt: 1,
                delay: Duration::from_secs(2),
            }
        );
    }
}
