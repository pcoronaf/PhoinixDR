//! A small least-recently-used cache of decoded units (chunks, blocks,
//! grains) shared by the container readers.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

/// Decoded units keyed by index, bounded by a byte budget.
#[derive(Debug)]
pub struct UnitCache {
    inner: Mutex<Inner>,
    budget: usize,
}

#[derive(Debug, Default)]
struct Inner {
    map: HashMap<u64, Arc<Vec<u8>>>,
    order: VecDeque<u64>,
    bytes: usize,
}

impl UnitCache {
    /// Creates a cache holding at most `budget` bytes of decoded data.
    #[must_use]
    pub fn new(budget: usize) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            budget,
        }
    }

    /// The cached unit `index`, if present.
    #[must_use]
    pub fn get(&self, index: u64) -> Option<Arc<Vec<u8>>> {
        let inner = self.inner.lock().ok()?;
        inner.map.get(&index).cloned()
    }

    /// Stores unit `index`, evicting the oldest units beyond the budget.
    pub fn put(&self, index: u64, data: Arc<Vec<u8>>) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if inner.map.contains_key(&index) {
            return;
        }
        inner.bytes = inner.bytes.saturating_add(data.len());
        inner.map.insert(index, data);
        inner.order.push_back(index);
        while inner.bytes > self.budget && inner.order.len() > 1 {
            let Some(old) = inner.order.pop_front() else {
                break;
            };
            if let Some(gone) = inner.map.remove(&old) {
                inner.bytes = inner.bytes.saturating_sub(gone.len());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn evicts_oldest_beyond_budget() {
        let cache = UnitCache::new(10);
        cache.put(1, Arc::new(vec![0; 4]));
        cache.put(2, Arc::new(vec![0; 4]));
        cache.put(3, Arc::new(vec![0; 4]));
        assert!(cache.get(1).is_none());
        assert!(cache.get(2).is_some());
        assert!(cache.get(3).is_some());
    }
}
