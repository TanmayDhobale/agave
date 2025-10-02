#![allow(clippy::arithmetic_side_effects)]
pub mod counter;
pub mod datapoint;
pub mod metrics;
pub use crate::metrics::{flush, query, set_host_id, set_panic_hook, submit};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

// To track an external counter which cannot be reset and is always increasing
#[derive(Default)]
pub struct MovingStat {
    value: AtomicU64,
}

impl MovingStat {
    pub fn update_stat(&self, old_value: &MovingStat, new_value: u64) {
        let old = old_value.value.swap(new_value, Ordering::Acquire);
        self.value
            .fetch_add(new_value.saturating_sub(old), Ordering::Release);
    }

    pub fn load_and_reset(&self) -> u64 {
        self.value.swap(0, Ordering::Acquire)
    }
}

#[cfg(feature = "agent-push")]
mod agent_sink {
    use std::net::UdpSocket;
    use std::sync::Mutex;
    use std::time::Duration;

    static SOCK: Mutex<Option<UdpSocket>> = Mutex::new(None);
    static mut TARGET: Option<std::net::SocketAddr> = None;

    pub fn agent_init_from_env() {
        if let Ok(addr) = std::env::var("AGAVE_AGENT_PUSH_ADDR") {
            if let Ok(target) = addr.parse() {
                let sock = UdpSocket::bind("127.0.0.1:0").ok();
                if let Some(sock) = sock {
                    let _ = sock.set_nonblocking(true);
                    let _ = sock.set_write_timeout(Some(Duration::from_millis(1)));
                    *SOCK.lock().unwrap() = Some(sock);
                    unsafe { TARGET = Some(target); }
                }
            }
        }
    }

    pub fn agent_push_gauge(name: &str, value: f64) {
        send(kind::GAUGE, name, value);
    }

    pub fn agent_push_counter(name: &str, value: f64) {
        send(kind::COUNTER, name, value);
    }

    mod kind { pub const GAUGE: &str = "gauge"; pub const COUNTER: &str = "counter"; }

    fn send(kind: &str, name: &str, value: f64) {
        let payload = format!("{{\"kind\":\"{}\",\"name\":\"{}\",\"value\":{}}}", kind, name, value);
        let guard = SOCK.lock().unwrap();
        if let Some(sock) = guard.as_ref() {
            let target = unsafe { TARGET };
            if let Some(target) = target {
                let _ = sock.send_to(payload.as_bytes(), target);
            }
        }
    }
}

#[cfg(not(feature = "agent-push"))]
mod agent_sink {
    pub fn agent_init_from_env() {}
    pub fn agent_push_gauge(_name: &str, _value: f64) {}
    pub fn agent_push_counter(_name: &str, _value: f64) {}
}

pub use agent_sink::{agent_init_from_env, agent_push_counter, agent_push_gauge};

/// A helper that sends the count of created tokens as a datapoint.
#[allow(clippy::redundant_allocation)]
pub struct TokenCounter(Arc<&'static str>);

impl TokenCounter {
    /// Creates a new counter with the specified metrics `name`.
    pub fn new(name: &'static str) -> Self {
        Self(Arc::new(name))
    }

    /// Creates a new token for this counter. The metric's value will be equal
    /// to the number of `CounterToken`s.
    pub fn create_token(&self) -> CounterToken {
        // new_count = strong_count
        //    - 1 (in TokenCounter)
        //    + 1 (token that's being created)
        datapoint_info!(*self.0, ("count", Arc::strong_count(&self.0), i64));
        CounterToken(self.0.clone())
    }
}

/// A token for `TokenCounter`.
#[allow(clippy::redundant_allocation)]
pub struct CounterToken(Arc<&'static str>);

impl Clone for CounterToken {
    fn clone(&self) -> Self {
        // new_count = strong_count
        //    - 1 (in TokenCounter)
        //    + 1 (token that's being created)
        datapoint_info!(*self.0, ("count", Arc::strong_count(&self.0), i64));
        CounterToken(self.0.clone())
    }
}

impl Drop for CounterToken {
    fn drop(&mut self) {
        // new_count = strong_count
        //    - 1 (in TokenCounter, if it still exists)
        //    - 1 (token that's being dropped)
        datapoint_info!(
            *self.0,
            ("count", Arc::strong_count(&self.0).saturating_sub(2), i64)
        );
    }
}

impl Drop for TokenCounter {
    fn drop(&mut self) {
        datapoint_info!(
            *self.0,
            ("count", Arc::strong_count(&self.0).saturating_sub(2), i64)
        );
    }
}
