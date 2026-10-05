//! A token bucket that many tasks can wait on. Rates can change while tasks are waiting.

use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub struct Limiter {
    /// Bytes per second; 0 means unlimited.
    rate: AtomicU64,
    bucket: Mutex<Bucket>,
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

impl Limiter {
    pub fn new(rate: Option<u64>) -> Self {
        Self {
            rate: AtomicU64::new(rate.unwrap_or(0)),
            bucket: Mutex::new(Bucket { tokens: 0.0, last: Instant::now() }),
        }
    }

    pub fn set_rate(&self, rate: Option<u64>) {
        self.rate.store(rate.unwrap_or(0), Ordering::Relaxed);
    }

    pub fn rate(&self) -> Option<u64> {
        match self.rate.load(Ordering::Relaxed) {
            0 => None,
            r => Some(r),
        }
    }

    /// Reserve `n` bytes and return how long the caller has to wait before using them.
    fn reserve(&self, n: u64) -> Duration {
        let rate = self.rate.load(Ordering::Relaxed);
        if rate == 0 {
            return Duration::ZERO;
        }
        let rate = rate as f64;
        let mut b = self.bucket.lock();
        let now = Instant::now();
        // Allow a quarter second of burst so short reads don't stutter.
        let burst = (rate / 4.0).max(32.0 * 1024.0);
        b.tokens = (b.tokens + now.duration_since(b.last).as_secs_f64() * rate).min(burst);
        b.last = now;
        b.tokens -= n as f64;
        if b.tokens >= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(-b.tokens / rate)
        }
    }

    pub async fn acquire(&self, n: u64) {
        let wait = self.reserve(n);
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
}

/// Wait on several limiters at once (global, network, job). The longest wait wins.
pub async fn acquire_all(limiters: &[&Limiter], n: u64) {
    let wait = limiters.iter().map(|l| l.reserve(n)).max().unwrap_or(Duration::ZERO);
    if !wait.is_zero() {
        tokio::time::sleep(wait).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unlimited_never_waits() {
        let l = Limiter::new(None);
        let t = Instant::now();
        for _ in 0..1000 {
            l.acquire(1 << 20).await;
        }
        assert!(t.elapsed() < Duration::from_millis(50));
    }

    #[tokio::test]
    async fn limited_rate_is_respected() {
        let l = Limiter::new(Some(1_000_000));
        let t = Instant::now();
        for _ in 0..10 {
            l.acquire(100_000).await;
        }
        // 1 MB at 1 MB/s with a 250 ms burst allowance.
        let e = t.elapsed().as_secs_f64();
        assert!(e > 0.6 && e < 1.3, "took {e}");
    }
}
