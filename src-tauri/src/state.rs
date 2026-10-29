use crate::config::{AppConfig, ConfigHandle, Role};
use crate::error;
use crate::input::{self, CaptureHandle, CaptureMsg, CaptureShared, InjectMsg};
use crate::layout::{DevicePlacement, LayoutStore};
use crate::net;
use crate::status::{ScreenInfo, Status};
use crossbeam_channel::unbounded;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

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

impl Engine {
    pub fn new(status: Status) -> Self {
        Self {
            inner: Mutex::new(None),
            status,
        }
    }

    pub fn is_running(&self) -> bool {
        self.inner
            .lock()
            .map(|slot| slot.is_some())
            .unwrap_or(false)
    }

    pub fn start(
        &self,
        config: &AppConfig,
        pairing_code: &str,
        config_handle: ConfigHandle,
    ) -> Result<(), String> {
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
                let (inject_tx, inject_rx) = unbounded::<InjectMsg>();
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
            .map(|screen| screen.id)
            .collect();
        let world = crate::layout::world_from_placements(host, placements, &online);
        layout.replace(world);
    }

    pub fn stop(&self) {
        let running = match self.inner.lock() {
            Ok(mut slot) => slot.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        let Some(mut running) = running else {
            return;
        };

        // Capture must stop first so it can emit a final button-release state
        // while the network thread is still draining input.
        if let Some(capture) = running.capture.take() {
            capture.stop_and_join();
            std::thread::sleep(Duration::from_millis(3));
        }
        running.stop.store(true, Ordering::Release);
        for handle in running.threads {
            if handle.join().is_err() {
                log::error!("worker thread panicked");
            }
        }
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
        let config = state.config.snapshot().unwrap_or_default();
        let mut status = state.status.snapshot();
        if status.local_screen.is_none() {
            status.local_screen = input::screen_info();
        }
        Self {
            config,
            status,
            platform: input::platform_name(),
            build: env!("GLIDELINK_BUILD"),
            permissions_ok: input::permissions_ok(),
            local_interfaces: net::local_interfaces(),
        }
    }
}
