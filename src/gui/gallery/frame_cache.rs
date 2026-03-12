use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone)]
pub struct CachedFrame {
    pub rgba_data: Arc<Vec<u8>>,
    pub width: u32,
    pub height: u32,
    pub timestamp_secs: f64,
    pub last_accessed: Instant,
    pub memory_size: usize,
}

pub struct FrameCache {
    entries: HashMap<i64, CachedFrame>,
    insertion_order: VecDeque<i64>,
    memory_budget: usize,
    current_memory: usize,
}

impl FrameCache {
    pub fn new(memory_budget_mb: usize) -> Self {
        Self {
            entries: HashMap::new(),
            insertion_order: VecDeque::new(),
            memory_budget: memory_budget_mb * 1024 * 1024,
            current_memory: 0,
        }
    }

    pub fn time_to_key(time_secs: f64) -> i64 {
        ((time_secs * 10.0).floor() as i64).max(0)
    }

    pub fn key_to_time(key: i64) -> f64 {
        key as f64 * 0.1
    }

    pub fn get(&mut self, time_secs: f64) -> Option<CachedFrame> {
        let key = Self::time_to_key(time_secs);
        if let Some(frame) = self.entries.get_mut(&key) {
            frame.last_accessed = Instant::now();
            return Some(frame.clone());
        }
        None
    }

    pub fn get_closest(&mut self, time_secs: f64, tolerance_secs: f64) -> Option<CachedFrame> {
        let key = Self::time_to_key(time_secs);
        let tolerance_keys = (tolerance_secs * 10.0).ceil() as i64;

        for offset in 0..=tolerance_keys {
            if offset == 0 {
                if let Some(frame) = self.get(time_secs) {
                    return Some(frame);
                }
            } else {
                if let Some(frame) = self.get(Self::key_to_time(key - offset)) {
                    return Some(frame);
                }
                if let Some(frame) = self.get(Self::key_to_time(key + offset)) {
                    return Some(frame);
                }
            }
        }
        None
    }

    pub fn insert(&mut self, time_secs: f64, rgba_data: Vec<u8>, width: u32, height: u32) {
        let key = Self::time_to_key(time_secs);
        let memory_size = rgba_data.len();

        if memory_size > self.memory_budget {
            return;
        }

        if self.entries.contains_key(&key) {
            if let Some(old_frame) = self.entries.remove(&key) {
                self.current_memory -= old_frame.memory_size;
            }
            self.insertion_order.retain(|&k| k != key);
        }

        while self.current_memory + memory_size > self.memory_budget {
            if let Some(old_key) = self.insertion_order.pop_front() {
                if let Some(old_frame) = self.entries.remove(&old_key) {
                    self.current_memory -= old_frame.memory_size;
                }
            } else {
                break;
            }
        }

        let frame = CachedFrame {
            rgba_data: Arc::new(rgba_data),
            width,
            height,
            timestamp_secs: time_secs,
            last_accessed: Instant::now(),
            memory_size,
        };

        self.current_memory += memory_size;
        self.entries.insert(key, frame);
        self.insertion_order.push_back(key);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.insertion_order.clear();
        self.current_memory = 0;
    }

    pub fn memory_usage(&self) -> usize {
        self.current_memory
    }

    pub fn memory_usage_mb(&self) -> f64 {
        self.current_memory as f64 / (1024.0 * 1024.0)
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn prefetch_range(&self, center_time: f64, range_secs: f64) -> Vec<f64> {
        let center_key = Self::time_to_key(center_time);
        let range_keys = (range_secs * 10.0).ceil() as i64;

        let mut missing = Vec::new();

        for offset in -range_keys..=range_keys {
            let key = center_key + offset;
            if key < 0 {
                continue;
            }
            if !self.entries.contains_key(&key) {
                missing.push(Self::key_to_time(key));
            }
        }

        missing
    }

    pub fn prefetch_keys_around(&self, time_secs: f64, count: usize) -> Vec<f64> {
        let center_key = Self::time_to_key(time_secs);
        let mut result = Vec::with_capacity(count);

        for offset in 0..count {
            let key = center_key + offset as i64;
            if key >= 0 && !self.entries.contains_key(&key) {
                result.push(Self::key_to_time(key));
            }
            if offset > 0 {
                let key = center_key - offset as i64;
                if key >= 0 && !self.entries.contains_key(&key) {
                    result.push(Self::key_to_time(key));
                }
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_insert_get() {
        let mut cache = FrameCache::new(1);
        let data = vec![0u8; 100];
        cache.insert(1.0, data.clone(), 10, 10);

        let frame = cache.get(1.0);
        assert!(frame.is_some());
        assert_eq!(frame.unwrap().rgba_data.as_ref(), &data);
    }

    #[test]
    fn test_cache_time_quantum() {
        let mut cache = FrameCache::new(1);
        let data = vec![0u8; 100];

        cache.insert(1.03, data.clone(), 10, 10);

        assert!(cache.get(1.0).is_some());
        assert!(cache.get(1.05).is_some());
        assert!(cache.get(1.1).is_none());
    }

    #[test]
    fn test_cache_eviction() {
        let mut cache = FrameCache::new(1);

        let large_data = vec![0u8; 400_000];
        for i in 0..5 {
            cache.insert(i as f64, large_data.clone(), 100, 100);
        }

        assert!(cache.memory_usage() <= 1024 * 1024);
    }
}
