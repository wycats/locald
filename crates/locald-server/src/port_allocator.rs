//! Port allocator with RAII guards to prevent race conditions.
//!
//! When multiple services start in parallel, they can race for ports:
//! 1. Service A binds to :0, gets port 33771, drops listener
//! 2. Service B binds to :0, gets port 33771 (OS reused it), drops listener
//! 3. Service A tries to use 33771 - conflict!
//!
//! This allocator solves this by:
//! 1. Using a mutex to serialize allocations
//! 2. Keeping the TcpListener alive until the service is ready to bind
//! 3. Tracking "pending" ports so we don't re-allocate them

use std::collections::HashSet;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use anyhow::Result;

/// A port allocator that prevents race conditions between parallel service starts.
#[derive(Clone, Debug)]
pub struct PortAllocator {
    inner: Arc<Mutex<PortAllocatorInner>>,
}

#[derive(Debug)]
struct PortAllocatorInner {
    /// Ports that have been allocated but not yet confirmed bound by a service.
    pending: HashSet<u16>,
}

/// RAII guard for an allocated port.
///
/// While this guard exists:
/// - The port is tracked as "pending" and won't be re-allocated
/// - Initially holds the `TcpListener` to prevent OS from reusing the port
///
/// Call `release_listener()` just before starting the service to free the port
/// for the service to bind. The guard should be kept alive until the service
/// confirms it has bound successfully.
#[derive(Debug)]
pub struct PortGuard {
    allocator: Arc<Mutex<PortAllocatorInner>>,
    port: u16,
    /// The listener that reserves this port. Dropped when `release_listener()` is called.
    listener: Option<TcpListener>,
}

impl Default for PortAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl PortAllocator {
    /// Create a new port allocator.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(PortAllocatorInner {
                pending: HashSet::new(),
            })),
        }
    }

    /// Allocate an available port.
    ///
    /// Returns a guard that:
    /// - Holds the port reserved (via `TcpListener`) until `release_listener()` is called
    /// - Tracks the port as "pending" until the guard is dropped
    ///
    /// # Errors
    ///
    /// Returns an error if we can't bind to any port.
    pub fn allocate(&self) -> Result<PortGuard> {
        let mut inner = self.inner.lock().unwrap();

        // Try up to 100 times to get a port not in our pending set
        for _ in 0..100 {
            let listener = TcpListener::bind("127.0.0.1:0")?;
            let port = listener.local_addr()?.port();

            if inner.pending.insert(port) {
                // Successfully inserted (was not already present)
                return Ok(PortGuard {
                    allocator: self.inner.clone(),
                    port,
                    listener: Some(listener),
                });
            }
            // Port is in our pending set (rare but possible if OS recycles aggressively)
        }

        anyhow::bail!("Failed to allocate a unique port after 100 attempts")
    }

    /// Allocate a specific port if it's available.
    ///
    /// Returns a guard if the port is free, or None if it's already in use.
    #[must_use]
    pub fn try_allocate_specific(&self, port: u16) -> Option<PortGuard> {
        let mut inner = self.inner.lock().unwrap();

        // Check if we're already tracking this port
        if inner.pending.contains(&port) {
            return None;
        }

        // Try to bind to the specific port
        let listener = TcpListener::bind(format!("127.0.0.1:{port}")).ok()?;

        // Only insert after successful bind
        inner.pending.insert(port);
        Some(PortGuard {
            allocator: self.inner.clone(),
            port,
            listener: Some(listener),
        })
    }
}

impl PortGuard {
    /// Get the allocated port number.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Release the underlying `TcpListener` so a service can bind to this port.
    ///
    /// The port remains tracked as "pending" until this guard is dropped,
    /// preventing other allocations from getting the same port.
    ///
    /// Call this immediately before starting the service that will use this port.
    pub fn release_listener(&mut self) {
        self.listener = None;
    }

    /// Consume the guard and return the port, releasing both the listener and tracking.
    ///
    /// Use this when you're confident the service has bound and tracking is no longer needed.
    #[must_use]
    pub fn take(mut self) -> u16 {
        self.listener = None;
        let port = self.port;
        // Manually remove from pending set
        {
            let mut inner = self.allocator.lock().unwrap();
            inner.pending.remove(&port);
        }
        // Prevent Drop from running (which would try to remove again)
        std::mem::forget(self);
        port
    }
}

impl Drop for PortGuard {
    fn drop(&mut self) {
        let mut inner = self.allocator.lock().unwrap();
        inner.pending.remove(&self.port);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_unique_ports() {
        let allocator = PortAllocator::new();

        let guard1 = allocator.allocate().unwrap();
        let guard2 = allocator.allocate().unwrap();
        let guard3 = allocator.allocate().unwrap();

        // All ports should be different
        assert_ne!(guard1.port(), guard2.port());
        assert_ne!(guard2.port(), guard3.port());
        assert_ne!(guard1.port(), guard3.port());
    }

    #[test]
    fn port_released_after_guard_dropped() {
        let allocator = PortAllocator::new();

        let port = {
            let guard = allocator.allocate().unwrap();
            guard.port()
        };

        // Port should no longer be in pending set
        let inner = allocator.inner.lock().unwrap();
        assert!(!inner.pending.contains(&port));
    }

    #[test]
    fn try_allocate_specific_works() {
        let allocator = PortAllocator::new();

        // Allocate a random port first
        let guard1 = allocator.allocate().unwrap();
        let port1 = guard1.port();

        // Trying to allocate the same port should fail
        assert!(allocator.try_allocate_specific(port1).is_none());

        // Drop the guard
        drop(guard1);

        // Now it might work (if OS hasn't reassigned it)
        // We can't guarantee this test passes since OS might have the port in TIME_WAIT
    }

    #[test]
    fn release_listener_keeps_tracking() {
        let allocator = PortAllocator::new();

        let mut guard = allocator.allocate().unwrap();
        let port = guard.port();

        // Release the listener
        guard.release_listener();

        // Port should still be tracked
        {
            let inner = allocator.inner.lock().unwrap();
            assert!(inner.pending.contains(&port));
        }

        // New allocation should not get this port
        let guard2 = allocator.allocate().unwrap();
        assert_ne!(guard2.port(), port);
    }
}
