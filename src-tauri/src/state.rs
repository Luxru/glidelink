use crate::config::{AppConfig, ConfigHandle, Role};
use crate::error;
use crate::input::{self, CaptureHandle, CaptureMsg, CaptureShared, InputMsg};
use crate::layout::{DevicePlacement, LayoutStore};
use crate::net;
use crate::status::{ScreenInfo, Status};
use crossbeam_channel::unbounded;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub struct AppState {
    pub config: ConfigHandle,
    pub engine: Engine,
    pub status: Status,
}

impl AppState {
    pub fn new(settings_path: PathBuf, config: AppConfig, status: Status) -> Self {
        Self {
            config: ConfigHandle::new(settings_path, config),
            engine: Engine::new(status.clone()),
            status,
        }
    }

    pub fn update_config<F>(&self, mutate: F) -> Result<AppConfig, String>
    where
        F: FnOnce(&mut AppConfig) -> Result<(), String>,
    {
        self.config.update(mutate)
    }
}

pub struct Engine {
    inner: Mutex<Option<Running>>,
    status: Status,
}

struct Running {
    stop: Arc<AtomicBool>,
    capture: Option<CaptureHandle>,
    threads: Vec<JoinHandle<()>>,
    layout: Option<LayoutStore>,
}

impl Running {
    fn shutdown(mut self) {
        // Capture releases state while transport is still alive. Injectors also
        // release their held input before exiting on stop.
        if let Some(capture) = self.capture.take() {
            capture.stop_and_join();
        }
        self.stop.store(true, Ordering::Release);
        for handle in self.threads {
            if handle.join().is_err() {
                log::error!("worker thread panicked");
            }
        }
    }
}

impl Engine {
    pub fn new(status: Status) -> Self {
        Self {
            inner: Mutex::new(None),
            status,
        }
    }

    pub fn is_running(&self) -> bool {
        self.reap_stopped();
        self.inner
            .lock()
            .map(|slot| slot.is_some())
            .unwrap_or(false)
    }

    fn reap_stopped(&self) {
        let Ok(mut slot) = self.inner.lock() else {
            return;
        };
        if slot
            .as_ref()
            .is_some_and(|running| running.stop.load(Ordering::Acquire))
        {
            if let Some(running) = slot.take() {
                // Keep the slot locked until all old workers finish, so a new
                // session cannot race their final status updates. Preserve the
                // connection error published by the worker for the UI.
                running.shutdown();
            }
        }
    }

    pub fn start(
        &self,
        config: &AppConfig,
        pairing_code: &str,
        config_handle: ConfigHandle,
    ) -> Result<(), String> {
        self.reap_stopped();
        let mut slot = self.inner.lock().map_err(|_| error::code("E_INTERNAL"))?;
        if slot.is_some() {
            return Err(error::code("E_SESSION_RUNNING"));
        }

        self.status.reset_traffic();
        let stop = Arc::new(AtomicBool::new(false));
        let status = self.status.clone();

        match config.role {
            Role::Controller => {
                let local_screen = input::screen_info().unwrap_or(ScreenInfo {
                    width: 0,
                    height: 0,
                });
                let world = crate::layout::world_from_placements(local_screen, &config.layout, &[]);
                let layout = LayoutStore::new(world);
                let shared = CaptureShared::new(status.clone(), layout.clone());
                let (tx, rx) = unbounded::<CaptureMsg>();
                let net_handle = match net::start_controller(
                    config,
                    config_handle,
                    layout.clone(),
                    rx,
                    Arc::clone(&stop),
                    status.clone(),
                ) {
                    Ok(handle) => handle,
                    Err(e) => {
                        status.reset();
                        return Err(e);
                    }
                };
                let capture = match input::start_capture(shared, tx) {
                    Ok(handle) => handle,
                    Err(e) => {
                        stop.store(true, Ordering::Release);
                        let _ = net_handle.join();
                        status.reset();
                        return Err(e);
                    }
                };
                *slot = Some(Running {
                    stop,
                    capture: Some(capture),
                    threads: vec![net_handle],
                    layout: Some(layout),
                });
            }
            Role::Agent => {
                let (inject_tx, inject_rx) = unbounded::<InputMsg>();
                let injector =
                    match input::start_injector(inject_rx, Arc::clone(&stop), status.clone()) {
                        Ok(handle) => handle,
                        Err(e) => {
                            status.reset();
                            return Err(e);
                        }
                    };
                let net_handle = match net::start_agent(
                    config,
                    pairing_code,
                    inject_tx,
                    Arc::clone(&stop),
                    status.clone(),
                ) {
                    Ok(handle) => handle,
                    Err(e) => {
                        stop.store(true, Ordering::Release);
                        let _ = injector.join();
                        status.reset();
                        return Err(e);
                    }
                };
                *slot = Some(Running {
                    stop,
                    capture: None,
                    threads: vec![net_handle, injector],
                    layout: None,
                });
            }
        }

        Ok(())
    }

    /// Update the live controller world after the UI saved a new arrangement.
    /// Online devices keep their connection state; geometry is rebuilt from
    /// placements.
    pub fn update_layout(&self, host: ScreenInfo, placements: &[DevicePlacement]) {
        let layout = self
            .inner
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().and_then(|running| running.layout.clone()));
        let Some(layout) = layout else {
            return;
        };
        let (_, current) = layout.snapshot();
        let online: Vec<_> = current
            .screens
            .iter()
            .filter(|screen| screen.online)
            .map(|screen| (screen.id, screen.session))
            .collect();
        let world = crate::layout::world_from_placements(host, placements, &online);
        layout.replace(world);
    }

    pub fn stop(&self) {
        let running = match self.inner.lock() {
            Ok(mut slot) => slot.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        let Some(running) = running else {
            return;
        };

        running.shutdown();
        self.status.reset();
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateView {
    pub config: AppConfig,
    pub status: crate::status::RuntimeStatus,
    pub platform: &'static str,
    /// Short git commit (plus `-dirty`) the binary was built from.
    pub build: &'static str,
    pub permissions_ok: bool,
    pub local_interfaces: Vec<net::NetworkInterface>,
}

impl StateView {
    pub fn build(state: &AppState) -> Self {
        state.engine.reap_stopped();
        let config = state.config.snapshot().unwrap_or_default();
        let mut status = state.status.snapshot();
        if status.local_screen.is_none() {
            status.local_screen = input::screen_info();
        }
        Self {
            config,
            status,
            platform: input::platform_name(),
            build: env!("RX_SWITCH_BUILD"),
            permissions_ok: input::permissions_ok(),
            local_interfaces: net::local_interfaces(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn expired_session_is_reaped_without_erasing_the_connection_error() {
        let status = Status::new(None);
        let engine = Engine::new(status.clone());
        let stop = Arc::new(AtomicBool::new(true));
        let finished = Arc::new(AtomicUsize::new(0));
        let worker_finished = finished.clone();
        let worker_status = status.clone();
        let join = std::thread::spawn(move || {
            worker_status
                .finish_session(Some("NET_CONNECTION_EXPIRED"), Some("old address".into()));
            worker_finished.fetch_add(1, Ordering::Release);
        });
        *engine.inner.lock().unwrap() = Some(Running {
            stop,
            capture: None,
            threads: vec![join],
            layout: None,
        });
        // Checks and subsequent starts must both see a free slot after joining
        // the old workers; their final writes cannot overwrite a new session.
        assert!(!engine.is_running());
        assert_eq!(finished.load(Ordering::Acquire), 1);
        assert!(engine.inner.lock().unwrap().is_none());
        assert_eq!(
            status.snapshot().last_error_code.as_deref(),
            Some("NET_CONNECTION_EXPIRED")
        );
    }

    #[test]
    fn reaping_a_live_session_does_not_disconnect_it() {
        let status = Status::new(None);
        let engine = Engine::new(status);
        let stop = Arc::new(AtomicBool::new(false));
        *engine.inner.lock().unwrap() = Some(Running {
            stop: stop.clone(),
            capture: None,
            threads: vec![],
            layout: None,
        });
        assert!(engine.is_running());
        assert!(!stop.load(Ordering::Acquire));
        engine.stop();
        assert!(!engine.is_running());
        assert!(stop.load(Ordering::Acquire));
    }
}
