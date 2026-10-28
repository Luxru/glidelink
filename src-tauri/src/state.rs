use crate::config::{self, AppConfig, Role};
use crate::error;
use crate::input::{self, CaptureHandle, CaptureMsg, CaptureShared, InjectMsg};
use crate::net;
use crate::status::Status;
use crossbeam_channel::unbounded;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

pub struct AppState {
    pub config: Mutex<AppConfig>,
    pub config_path: PathBuf,
    pub engine: Engine,
    pub status: Status,
}

impl AppState {
    pub fn new(config_path: PathBuf, config: AppConfig, status: Status) -> Self {
        Self {
            config: Mutex::new(config),
            config_path,
            engine: Engine::new(status.clone()),
            status,
        }
    }

    pub fn save_config(&self, config: &AppConfig) -> Result<(), String> {
        config::save(&self.config_path, config)
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

    pub fn start(&self, config: &AppConfig, pairing_code: &str) -> Result<(), String> {
        let mut slot = self.inner.lock().map_err(|_| error::code("E_INTERNAL"))?;
        if slot.is_some() {
            return Err(error::code("E_SESSION_RUNNING"));
        }

        self.status.reset_traffic();
        let stop = Arc::new(AtomicBool::new(false));
        let status = self.status.clone();

        match config.role {
            Role::Controller => {
                let shared = CaptureShared::new(status.clone());
                let (tx, rx) = unbounded::<CaptureMsg>();
                let net_handle = match net::start_controller(
                    config,
                    pairing_code,
                    rx,
                    Arc::clone(&shared),
                    Arc::clone(&stop),
                    status.clone(),
                ) {
                    Ok(handle) => handle,
                    Err(e) => {
                        status.reset();
                        return Err(e);
                    }
                };
                let capture =
                    match input::start_capture(shared, tx, config.edge, config.peer_offset) {
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
                });
            }
        }

        Ok(())
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
            // Give the controller network loop one short tick to flush the
            // final button-release packet before it sees the stop flag.
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
    pub permissions_ok: bool,
    pub local_interfaces: Vec<net::NetworkInterface>,
}

impl StateView {
    pub fn build(state: &AppState) -> Self {
        let config = state
            .config
            .lock()
            .map(|cfg| cfg.clone())
            .unwrap_or_default();
        let mut status = state.status.snapshot();
        if status.local_screen.is_none() {
            status.local_screen = input::screen_info();
        }
        Self {
            config,
            status,
            platform: input::platform_name(),
            permissions_ok: input::permissions_ok(),
            local_interfaces: net::local_interfaces(),
        }
    }
}
