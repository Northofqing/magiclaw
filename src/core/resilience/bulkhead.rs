use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Semaphore;

/// Bulkhead: limits concurrent executions to isolate failure domains.
pub struct Bulkhead {
    semaphore: Semaphore,
    max_concurrent: usize,
    active_count: AtomicUsize,
}

impl Bulkhead {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            semaphore: Semaphore::new(max_concurrent),
            max_concurrent,
            active_count: AtomicUsize::new(0),
        }
    }

    /// Try to acquire a permit. Returns a guard that releases on drop.
    pub async fn acquire(&self) -> Result<BulkheadGuard<'_>, BulkheadError> {
        let permit = self.semaphore.acquire().await.map_err(|_| BulkheadError::Closed)?;
        self.active_count.fetch_add(1, Ordering::SeqCst);
        Ok(BulkheadGuard {
            _permit: permit,
            bulkhead: self,
        })
    }

    /// Try to acquire without waiting.
    pub fn try_acquire(&self) -> Result<BulkheadGuard<'_>, BulkheadError> {
        let permit = self.semaphore.try_acquire().map_err(|_| BulkheadError::Full)?;
        self.active_count.fetch_add(1, Ordering::SeqCst);
        Ok(BulkheadGuard {
            _permit: permit,
            bulkhead: self,
        })
    }

    pub fn active_count(&self) -> usize {
        self.active_count.load(Ordering::SeqCst)
    }

    pub fn max_concurrent(&self) -> usize {
        self.max_concurrent
    }
}

pub struct BulkheadGuard<'a> {
    _permit: tokio::sync::SemaphorePermit<'a>,
    bulkhead: &'a Bulkhead,
}

impl Drop for BulkheadGuard<'_> {
    fn drop(&mut self) {
        self.bulkhead.active_count.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BulkheadError {
    #[error("bulkhead is full")]
    Full,
    #[error("bulkhead is closed")]
    Closed,
}

/// Pre-configured bulkhead pools.
pub struct BulkheadPools {
    pub ai: Bulkhead,
    pub send: Bulkhead,
}

impl BulkheadPools {
    pub fn new(ai_max: usize, send_max: usize) -> Self {
        Self {
            ai: Bulkhead::new(ai_max),
            send: Bulkhead::new(send_max),
        }
    }
}

impl Default for BulkheadPools {
    fn default() -> Self {
        Self::new(5, 50)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bulkhead_acquire_and_release() {
        let bh = Bulkhead::new(2);
        let g1 = bh.acquire().await.unwrap();
        assert_eq!(bh.active_count(), 1);
        let g2 = bh.acquire().await.unwrap();
        assert_eq!(bh.active_count(), 2);
        drop(g1);
        assert_eq!(bh.active_count(), 1);
        drop(g2);
        assert_eq!(bh.active_count(), 0);
    }

    #[tokio::test]
    async fn bulkhead_try_acquire_when_full() {
        let bh = Bulkhead::new(1);
        let _g = bh.acquire().await.unwrap();
        assert!(bh.try_acquire().is_err());
    }
}
