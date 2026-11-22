//! What was rasterized this session, so a relayout (zoom, resize, `w`, an
//! edit while following) encodes again only what changed. Entries are keyed
//! by a hash of everything their pixels depend on and evicted least
//! recently used once over a byte budget.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use crate::kitty::Transmit;

/// Encoded images kept, in bytes of base64.
const TRANSMIT_BUDGET: usize = 256 << 20;

/// A 64-bit key for `value`; collisions are left to chance.
pub fn key(value: impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// The upload cached under `key`, or the one `make` encodes, kept for next
/// time. `None` (nothing to show) is not cached.
pub fn transmit(key: u64, make: impl FnOnce() -> Option<Transmit>) -> Option<Arc<Transmit>> {
    static CACHE: OnceLock<Mutex<Lru<Arc<Transmit>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(Lru::new(TRANSMIT_BUDGET)));
    if let Some(hit) = cache.lock().ok()?.get(key) {
        return Some(hit);
    }
    // Encoded outside the lock, so other threads keep going; two of them
    // making the same entry at once only waste the work.
    let made = Arc::new(make()?);
    if let Ok(mut cache) = cache.lock() {
        let weight = made.len();
        cache.insert(key, made.clone(), weight);
    }
    Some(made)
}

/// A map that drops its least recently used entries to stay within a
/// budget of total weight.
pub struct Lru<V> {
    entries: HashMap<u64, Entry<V>>,
    budget: usize,
    weight: usize,
    clock: u64,
}

struct Entry<V> {
    value: V,
    weight: usize,
    used: u64,
}

impl<V: Clone> Lru<V> {
    pub fn new(budget: usize) -> Self {
        Self {
            entries: HashMap::new(),
            budget,
            weight: 0,
            clock: 0,
        }
    }

    pub fn get(&mut self, key: u64) -> Option<V> {
        self.clock += 1;
        let entry = self.entries.get_mut(&key)?;
        entry.used = self.clock;
        Some(entry.value.clone())
    }

    /// Keeps `value`, unless it alone is over the budget.
    pub fn insert(&mut self, key: u64, value: V, weight: usize) {
        if weight > self.budget {
            return;
        }
        self.clock += 1;
        let entry = Entry {
            value,
            weight,
            used: self.clock,
        };
        if let Some(old) = self.entries.insert(key, entry) {
            self.weight -= old.weight;
        }
        self.weight += weight;
        while self.weight > self.budget {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.used)
                .map(|(&k, _)| k)
            else {
                break;
            };
            if let Some(e) = self.entries.remove(&oldest) {
                self.weight -= e.weight;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_should_drop_least_recently_used_over_budget() {
        let mut lru = Lru::new(10);
        lru.insert(1, 'a', 4);
        lru.insert(2, 'b', 4);
        lru.get(1);
        lru.insert(3, 'c', 4);
        assert_eq!(
            (lru.get(1), lru.get(2), lru.get(3)),
            (Some('a'), None, Some('c'))
        );
    }

    #[test]
    fn lru_should_skip_value_over_whole_budget() {
        let mut lru = Lru::new(10);
        lru.insert(1, 'a', 11);
        assert_eq!(lru.get(1), None);
    }

    #[test]
    fn transmit_should_encode_once_per_key() {
        let mut calls = 0;
        let mut make = || {
            calls += 1;
            Some(Transmit::png(b"x", 1, 1))
        };
        let k = key(("cache test", 1));
        transmit(k, &mut make);
        transmit(k, &mut make);
        assert_eq!(calls, 1);
    }
}
