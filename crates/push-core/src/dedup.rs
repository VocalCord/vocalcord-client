//! Recent-message-id LRU so a message that arrives via both the
//! webhook path and the WebSocket path (vocalcord delivers via both
//! in parallel) only fires once into the agent.
//!
//! 256 entries is enough to absorb a burst that overlaps both paths
//! without growing unbounded; older ids age out as new ones arrive.

use lru::LruCache;
use parking_lot::Mutex;
use std::num::NonZeroUsize;
use std::sync::Arc;

#[derive(Clone)]
pub struct Dedup {
    inner: Arc<Mutex<LruCache<String, ()>>>,
}

impl Dedup {
    pub fn new(capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity.max(1)).unwrap();
        Self {
            inner: Arc::new(Mutex::new(LruCache::new(cap))),
        }
    }

    /// Returns true iff this id has been seen recently. Records the
    /// id as the most-recently-seen as a side effect.
    pub fn check_and_record(&self, id: &str) -> bool {
        let mut g = self.inner.lock();
        if g.get(id).is_some() {
            return true;
        }
        g.put(id.to_owned(), ());
        false
    }
}

impl Default for Dedup {
    fn default() -> Self {
        Self::new(256)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sighting_returns_false() {
        let d = Dedup::default();
        assert!(!d.check_and_record("a"));
    }

    #[test]
    fn repeat_returns_true() {
        let d = Dedup::default();
        d.check_and_record("a");
        assert!(d.check_and_record("a"));
    }

    #[test]
    fn unrelated_ids_independent() {
        let d = Dedup::default();
        assert!(!d.check_and_record("a"));
        assert!(!d.check_and_record("b"));
        assert!(d.check_and_record("a"));
    }

    #[test]
    fn evicts_at_capacity() {
        let d = Dedup::new(2);
        d.check_and_record("a");
        d.check_and_record("b");
        d.check_and_record("c");
        // "a" was the oldest, should have been evicted.
        assert!(!d.check_and_record("a"));
    }
}
