#[cfg(not(any(windows, target_os = "macos")))]
use crate::layout::Edge;
#[cfg(not(any(windows, target_os = "macos")))]
use crate::status::ScreenInfo;
use crate::status::Status;
#[cfg(not(any(windows, target_os = "macos")))]
use crossbeam_channel::{Receiver, Sender};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod win;

pub const BUTTON_LEFT: u8 = 1 << 0;
pub const BUTTON_RIGHT: u8 = 1 << 1;
pub const BUTTON_MIDDLE: u8 = 1 << 2;
pub const BUTTON_X1: u8 = 1 << 3;
pub const BUTTON_X2: u8 = 1 << 4;

/// Absolute position in the peer screen plus the current button state.
/// The packet is sent for movement and every button/wheel change.
#[derive(Clone, Copy, Debug, Default)]
pub struct RemoteInput {
    pub x: f32,
    pub y: f32,
    pub buttons: u8,
    pub wheel: i16,
}

#[derive(Clone, Copy, Debug)]
pub enum InjectMsg {
    State(RemoteInput),
    Reset,
}

/// Cross-thread state shared by the capture thread and networking.
pub struct CaptureShared {
    pub stop: AtomicBool,
    pub connected: AtomicBool,
    pub remote_width: AtomicU32,
    pub remote_height: AtomicU32,
    pub active: AtomicBool,
    pub status: Status,
}

impl CaptureShared {
    pub fn new(status: Status) -> Arc<Self> {
        Arc::new(Self {
            stop: AtomicBool::new(false),
            connected: AtomicBool::new(false),
            remote_width: AtomicU32::new(0),
            remote_height: AtomicU32::new(0),
            active: AtomicBool::new(false),
            status,
        })
    }

    pub fn set_peer(&self, width: u32, height: u32) {
        self.remote_width.store(width, Ordering::Release);
        self.remote_height.store(height, Ordering::Release);
        self.connected.store(true, Ordering::Release);
    }

    pub fn clear_peer(&self) {
        self.connected.store(false, Ordering::Release);
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    pub fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::Release);
        self.status.update(|status| status.remote_active = active);
    }
}

pub struct CaptureHandle {
    shared: Arc<CaptureShared>,
    join: JoinHandle<()>,
}

impl CaptureHandle {
    pub fn new(shared: Arc<CaptureShared>, join: JoinHandle<()>) -> Self {
        Self { shared, join }
    }

    pub fn stop_and_join(self) {
        self.shared.stop.store(true, Ordering::Release);
        if self.join.join().is_err() {
            log::error!("input capture thread panicked");
        }
    }
}

#[cfg(windows)]
pub use win::{open_permissions, permissions_ok, screen_info, start_capture, start_injector};

#[cfg(target_os = "macos")]
pub use macos::{open_permissions, permissions_ok, screen_info, start_capture, start_injector};

#[cfg(not(any(windows, target_os = "macos")))]
mod unsupported {
    use super::*;

    pub fn screen_info() -> Option<ScreenInfo> {
        None
    }

    pub fn permissions_ok() -> bool {
        false
    }

    pub fn open_permissions() -> Result<(), String> {
        Err(crate::error::code("E_UNSUPPORTED"))
    }

    pub fn start_capture(
        _shared: Arc<CaptureShared>,
        _tx: Sender<RemoteInput>,
        _edge: Edge,
    ) -> Result<CaptureHandle, String> {
        Err(crate::error::code("E_UNSUPPORTED"))
    }

    pub fn start_injector(
        _rx: Receiver<InjectMsg>,
        _stop: Arc<AtomicBool>,
        _status: Status,
    ) -> Result<JoinHandle<()>, String> {
        Err(crate::error::code("E_UNSUPPORTED"))
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
pub use unsupported::{
    open_permissions, permissions_ok, screen_info, start_capture, start_injector,
};

pub fn platform_name() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "unsupported"
    }
}
