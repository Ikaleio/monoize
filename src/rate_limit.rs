use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Sliding-window rate limiter keyed by string (typically client IP).
///
/// Invariant: for any key, `check(key)` returns true at most `max_requests`
/// times within any contiguous `window` duration. Stale entries are lazily
/// evicted on each `check` call.
#[derive(Clone)]
pub struct RateLimiter {
    /// Map from key → list of request timestamps within the current window.
    entries: Arc<DashMap<String, Vec<Instant>>>,
    max_requests: usize,
    window: Duration,
}

impl RateLimiter {
    pub fn new(max_requests: usize, window: Duration) -> Self {
        Self {
            entries: Arc::new(DashMap::new()),
            max_requests,
            window,
        }
    }

    /// Returns `true` if the request is allowed, `false` if rate-limited.
    /// Each allowed call consumes one slot in the window.
    pub fn check(&self, key: &str) -> bool {
        let now = Instant::now();
        let cutoff = now - self.window;

        let mut entry = self.entries.entry(key.to_string()).or_default();
        // Evict timestamps outside the window
        entry.retain(|&t| t > cutoff);

        if entry.len() >= self.max_requests {
            return false;
        }
        entry.push(now);
        true
    }

    /// Remove entries that have been idle longer than the window.
    /// Call periodically from a background task to bound memory.
    pub fn cleanup(&self) {
        let cutoff = Instant::now() - self.window;
        self.entries.retain(|_, timestamps| {
            timestamps.retain(|&t| t > cutoff);
            !timestamps.is_empty()
        });
    }
}
