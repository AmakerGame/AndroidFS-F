use std::hash::Hash;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use linked_hash_map::LinkedHashMap;

pub struct Cache<K: Hash + Eq + Clone, V: Clone> {
    cache: RwLock<LinkedHashMap<K, CacheItem<V>>>,
    invalidation_time: Duration,
    max_size: usize,
}

struct CacheItem<T> {
    value: T,
    cache_time: Instant,
}

impl<K: Hash + Eq + Clone, V: Clone> Cache<K, V> {
    pub fn new(invalidation_time: Duration, max_size: usize) -> Self {
        Cache {
            cache: RwLock::new(LinkedHashMap::new()),
            invalidation_time,
            max_size,
        }
    }

    pub fn erase(&self, key: &K) {
        self.cache.write().unwrap().remove(key);
    }

    pub fn put(&self, key: K, value: V) {
        let mut cache = self.cache.write().unwrap();
        if cache.len() >= self.max_size && !cache.contains_key(&key) {
            if let Some(first) = cache.keys().next().cloned() {
                cache.remove(&first);
            }
        }
        cache.insert(
            key,
            CacheItem {
                value,
                cache_time: Instant::now(),
            },
        );
    }

    pub fn try_get(&self, key: &K) -> Option<V> {
        let expired = {
            let cache = self.cache.read().unwrap();
            match cache.get(key) {
                Some(item) if item.cache_time.elapsed() < self.invalidation_time => {
                    return Some(item.value.clone());
                }
                Some(_) => true,
                None => false,
            }
        };
        if expired {
            self.cache.write().unwrap().remove(key);
        }
        None
    }
}
