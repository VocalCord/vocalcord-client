//! In-memory rate-limit gate honoring 429 + `Retry-After`.
//!
//! Design rationale: the OpenClaw plugin's pull-sync runs on a fixed
//! 6-hour cron. When vocalcord 429s, we want SUBSEQUENT calls within
//! the Retry-After window to no-op rather than firing again. We do
//! NOT want to add an in-process retry inside the failing call —
//! the cron rhythm already provides retries, and stacking them
//! would double up on backoff (and spin a 24h Retry-After into a
//! 24h thread sleep).
//!
//! State is in-memory only. On process restart it is lost — the
//! next call after restart hits vocalcord, may receive 429 again,
//! and reseeds the gate. This trades correctness during a restart
//! window for zero persistence complexity, which the design
//! explicitly chose.

use parking_lot::RwLock;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
pub struct Gate {
    until: Arc<RwLock<Option<Instant>>>,
}

impl Gate {
    pub fn new() -> Self {
        Self::default()
    }

    /// True iff the gate is currently rate-limited.
    pub fn is_blocked(&self) -> bool {
        match *self.until.read() {
            Some(t) => Instant::now() < t,
            None => false,
        }
    }

    /// Remaining duration until the gate opens (None if not blocked).
    pub fn remaining(&self) -> Option<Duration> {
        let until = (*self.until.read())?;
        let now = Instant::now();
        if now < until {
            Some(until.duration_since(now))
        } else {
            None
        }
    }

    /// Record a 429. The retry-after value is taken at face value
    /// — no upper bound is imposed (a 24h Retry-After means the
    /// gate stays closed for 24h, exactly as the server requested).
    pub fn record_429(&self, retry_after: Duration) {
        *self.until.write() = Some(Instant::now() + retry_after);
    }

    /// Manually clear the gate (e.g. after a successful manual
    /// override or after the user explicitly says "try again now").
    pub fn clear(&self) {
        *self.until.write() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unblocked_by_default() {
        let g = Gate::new();
        assert!(!g.is_blocked());
        assert!(g.remaining().is_none());
    }

    #[test]
    fn record_blocks_then_clears() {
        let g = Gate::new();
        g.record_429(Duration::from_millis(50));
        assert!(g.is_blocked());
        std::thread::sleep(Duration::from_millis(80));
        assert!(!g.is_blocked());
    }

    #[test]
    fn explicit_clear() {
        let g = Gate::new();
        g.record_429(Duration::from_secs(60));
        assert!(g.is_blocked());
        g.clear();
        assert!(!g.is_blocked());
    }

    #[test]
    fn remaining_is_positive_when_blocked() {
        let g = Gate::new();
        g.record_429(Duration::from_secs(1));
        let r = g.remaining().unwrap();
        assert!(r.as_millis() > 0 && r.as_millis() <= 1000);
    }
}
