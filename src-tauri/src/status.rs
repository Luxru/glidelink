use serde::Serialize;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
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
pub struct RuntimeStatus {
    pub running: bool,
    pub connected: bool,
    pub remote_active: bool,
    pub latency_ms: Option<f64>,
    pub last_error_code: Option<String>,
    pub last_error_detail: Option<String>,
    pub peer: Option<String>,
    pub local_screen: Option<ScreenInfo>,
    pub remote_screen: Option<ScreenInfo>,
}

/// Shared UI-facing status. Never touched from the per-event hot path except on
/// state transitions.
#[derive(Clone, Default)]
pub struct Status {
    inner: Arc<Mutex<RuntimeStatus>>,
}

impl Status {
    pub fn new(local_screen: Option<ScreenInfo>) -> Self {
        let status = Self::default();
        status.update(|s| s.local_screen = local_screen);
        status
    }

    pub fn snapshot(&self) -> RuntimeStatus {
        self.inner.lock().map(|s| s.clone()).unwrap_or_default()
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
