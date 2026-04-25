use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use super::connector::BoxProxyStream;

/// Max age for pooled connections before they're considered stale.
const DEFAULT_MAX_AGE: Duration = Duration::from_secs(30);
/// Default pool capacity (pre-established connections to keep warm).
const DEFAULT_CAPACITY: usize = 4;

/// A pre-established TCP+TLS connection waiting to be used.
struct PooledConn {
    stream: BoxProxyStream,
    created: Instant,
}

/// Async factory that creates new TCP+TLS connections to the proxy server.
/// Each outbound (VLESS, VMess, Trojan) provides its own implementation.
pub trait ConnFactory: Send + Sync + 'static {
    fn create(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<BoxProxyStream>> + Send + '_>,
    >;
}

/// Inner state of the connection pool.
struct PoolInner {
    conns: Mutex<VecDeque<PooledConn>>,
    factory: Arc<dyn ConnFactory>,
    capacity: usize,
    max_age: Duration,
    warmed: std::sync::atomic::AtomicBool,
    filling: std::sync::atomic::AtomicBool,
}

/// Connection pool that maintains pre-established TCP+TLS connections
/// to avoid 2-3 RTTs (TCP handshake + TLS handshake) on each request.
///
/// Usage: call `get()` to obtain a stream. If a warm connection is
/// available it's returned immediately; otherwise a new one is created.
/// After taking a connection, a background refill task is spawned.
#[derive(Clone)]
pub struct ConnPool {
    inner: Arc<PoolInner>,
}

impl ConnPool {
    /// Create a new pool and start the initial warm-up.
    pub fn new(factory: Arc<dyn ConnFactory>) -> Self {
        Self::with_capacity(factory, DEFAULT_CAPACITY)
    }

    pub fn with_capacity(factory: Arc<dyn ConnFactory>, capacity: usize) -> Self {
        Self {
            inner: Arc::new(PoolInner {
                conns: Mutex::new(VecDeque::with_capacity(capacity)),
                factory,
                capacity,
                max_age: DEFAULT_MAX_AGE,
                warmed: std::sync::atomic::AtomicBool::new(false),
                filling: std::sync::atomic::AtomicBool::new(false),
            }),
        }
    }

    /// Get a connection: returns a warm pooled one if available, else creates new.
    /// On first call, kicks off background pool warmup.
    pub async fn get(&self) -> anyhow::Result<BoxProxyStream> {
        // Lazy warmup: start filling on first use
        if !self
            .inner
            .warmed
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            self.schedule_fill();
        }

        // Try to grab a non-stale connection from the pool
        {
            let mut q = self.inner.conns.lock().await;
            while let Some(pc) = q.pop_front() {
                if pc.created.elapsed() < self.inner.max_age {
                    drop(q);
                    // Got a warm connection — schedule refill
                    self.schedule_fill();
                    return Ok(pc.stream);
                }
                // Stale — drop it and try next
            }
        }
        // Pool empty — create directly (no extra latency beyond normal)
        self.inner.factory.create().await
    }

    /// Schedule a background fill only if no fill is already in progress.
    fn schedule_fill(&self) {
        if !self
            .inner
            .filling
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            let p = self.clone();
            tokio::spawn(async move {
                p.fill().await;
                p.inner
                    .filling
                    .store(false, std::sync::atomic::Ordering::Relaxed);
            });
        }
    }

    /// Fill the pool up to capacity. Only one fill runs at a time.
    async fn fill(&self) {
        // Small delay to avoid thundering herd during burst
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Evict stale connections first
        {
            let mut q = self.inner.conns.lock().await;
            q.retain(|pc| pc.created.elapsed() < self.inner.max_age);
        }

        let mut consecutive_failures: u32 = 0;
        loop {
            // Check current size under lock, then release before creating
            let needs_more = {
                let q = self.inner.conns.lock().await;
                q.len() < self.inner.capacity
            };
            if !needs_more {
                break;
            }
            match self.inner.factory.create().await {
                Ok(stream) => {
                    consecutive_failures = 0;
                    let mut q = self.inner.conns.lock().await;
                    if q.len() < self.inner.capacity {
                        q.push_back(PooledConn {
                            stream,
                            created: Instant::now(),
                        });
                    }
                }
                Err(e) => {
                    tracing::debug!("Connection pool: pre-connect failed: {}", e);
                    // Don't give up on the first failure; transient errors (DNS, TCP reset)
                    // are common. Try up to 3 consecutive failures before abandoning the
                    // refill so the pool isn't left permanently underfilled.
                    consecutive_failures += 1;
                    if consecutive_failures >= 3 {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
    }
}
