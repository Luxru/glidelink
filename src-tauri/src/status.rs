use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenInfo {
    pub width: u32,
    pub height: u32,
}

impl ScreenInfo {
    pub fn new(width: i32, height: i32) -> Self {
        Self {
            width: width.max(0) as u32,
            height: height.max(0) as u32,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerStatus {
    pub device_id: String,
    pub label: String,
    pub addr: String,
    pub platform: Option<u8>,
    pub screen: Option<ScreenInfo>,
    pub online: bool,
    pub active: bool,
    pub latency_ms: Option<f64>,
    pub jitter_ms: Option<f64>,
    pub packet_loss_percent: Option<f64>,
    pub last_error_code: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub running: bool,
    pub connected: bool,
    pub remote_active: bool,
    pub latency_ms: Option<f64>,
    pub last_error_code: Option<String>,
    pub last_error_detail: Option<String>,
    pub peer: Option<String>,
    /// Platform reported by the peer during the handshake (1 = macOS, 2 = Windows).
    pub peer_platform: Option<u8>,
    pub local_screen: Option<ScreenInfo>,
    pub remote_screen: Option<ScreenInfo>,
    pub started_at_ms: Option<u64>,
    pub packets_sent: u64,
    pub packets_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub jitter_ms: Option<f64>,
    pub input_interval_ms: Option<f64>,
    pub packet_loss_percent: Option<f64>,
    pub pairing_code: Option<String>,
    pub devices: Vec<PeerStatus>,
    pub target_device: Option<String>,
}

#[derive(Default)]
struct Traffic {
    packets_sent: AtomicU64,
    packets_received: AtomicU64,
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
}

/// Shared UI-facing status. State transitions still take the mutex; high-rate
/// traffic counters live in atomics so the network loops never contend on it.
#[derive(Clone, Default)]
pub struct Status {
    inner: Arc<Mutex<RuntimeStatus>>,
    traffic: Arc<Traffic>,
}

impl Status {
    pub fn new(local_screen: Option<ScreenInfo>) -> Self {
        let status = Self::default();
        status.update(|s| s.local_screen = local_screen);
        status
    }

    pub fn snapshot(&self) -> RuntimeStatus {
        let mut status = self.inner.lock().map(|s| s.clone()).unwrap_or_default();
        status.packets_sent = self.traffic.packets_sent.load(Ordering::Relaxed);
        status.packets_received = self.traffic.packets_received.load(Ordering::Relaxed);
        status.bytes_sent = self.traffic.bytes_sent.load(Ordering::Relaxed);
        status.bytes_received = self.traffic.bytes_received.load(Ordering::Relaxed);
        status
    }

    pub fn add_sent(&self, bytes: usize) {
        self.traffic.packets_sent.fetch_add(1, Ordering::Relaxed);
        self.traffic
            .bytes_sent
            .fetch_add(bytes as u64, Ordering::Relaxed);
    }

    pub fn add_received(&self, bytes: usize) {
        self.traffic
            .packets_received
            .fetch_add(1, Ordering::Relaxed);
        self.traffic
            .bytes_received
            .fetch_add(bytes as u64, Ordering::Relaxed);
    }

    pub fn reset_traffic(&self) {
        self.traffic.packets_sent.store(0, Ordering::Relaxed);
        self.traffic.packets_received.store(0, Ordering::Relaxed);
        self.traffic.bytes_sent.store(0, Ordering::Relaxed);
        self.traffic.bytes_received.store(0, Ordering::Relaxed);
    }

    pub fn update<F>(&self, f: F)
    where
        F: FnOnce(&mut RuntimeStatus),
    {
        if let Ok(mut inner) = self.inner.lock() {
            f(&mut inner);
        }
    }

    pub fn set_error(&self, code: impl Into<String>, detail: Option<String>) {
        self.update(|s| {
            s.last_error_code = Some(code.into());
            s.last_error_detail = detail;
        });
    }

    pub fn reset(&self) {
        let local = self.snapshot().local_screen;
        self.update(|s| {
            *s = RuntimeStatus {
                running: false,
                local_screen: local,
                ..RuntimeStatus::default()
            };
        });
    }
}
