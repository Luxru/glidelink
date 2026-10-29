#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod error;
mod input;
mod invite;
mod layout;
mod logging;
mod net;
mod sched;
mod state;
mod status;

use config::{AppConfig, Role};
use invite::Invite;
use layout::DevicePlacement;
use state::{AppState, StateView};
use status::ScreenInfo;
use std::collections::HashSet;
use std::net::Ipv4Addr;
use tauri::Manager;

#[tauri::command]
fn get_state(state: tauri::State<'_, AppState>) -> StateView {
    StateView::build(&state)
}

#[tauri::command]
fn start_session(
    mut config: AppConfig,
    pairing_code: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let pairing_code = invite::normalize_code(&pairing_code);
    config.local_ip = config.local_ip.trim().to_string();
    if config.role == Role::Agent {
        Invite::decode(&pairing_code).map_err(|_| error::code("E_PAIRING_CODE"))?;
    }

    if state.engine.is_running() {
        return Err(error::code("E_SESSION_RUNNING"));
    }

    let incoming = config;
    let saved_code = pairing_code.clone();
    let config = state.update_config(move |next| {
        let previous = next.clone();
        *next = incoming.clone();
        next.inherit_persisted_settings(&previous);
        next.ensure_device_id();
        next.validate()?;
        if next.role == Role::Agent {
            next.last_pairing_code = saved_code.clone();
        }
        Ok(())
    })?;
    log::info!(
        "starting {} session (local_ip={})",
        config.role.as_str(),
        if config.local_ip.is_empty() {
            "-"
        } else {
            &config.local_ip
        }
    );
    let config_handle = state.config.clone();
    state.engine.start(&config, &pairing_code, config_handle)
}

#[tauri::command]
fn preview_pairing_code(
    local_ip: String,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    if let Some(code) = state.status.snapshot().pairing_code {
        return Ok(code);
    }
    let ip = local_ip
        .trim()
        .parse::<Ipv4Addr>()
        .map_err(|_| error::code("E_LOCAL_IP"))?;
    if ip.is_loopback() || ip.is_unspecified() {
        return Err(error::code("E_LOCAL_IP"));
    }
    let last = state.config.snapshot()?.last_invite;
    let code = net::preview_listen_code(ip, &last)?;
    state.update_config(|config| {
        config.last_invite = code.clone();
        Ok(())
    })?;
    Ok(code)
}

#[tauri::command]
fn save_layout(
    devices: Vec<DevicePlacement>,
    state: tauri::State<'_, AppState>,
) -> Result<AppConfig, String> {
    if state.status.snapshot().remote_active {
        return Err(error::code("E_LAYOUT_ACTIVE"));
    }
    if devices.len() > layout::MAX_DEVICES {
        return Err(error::code("E_LAYOUT_INVALID"));
    }
    let mut ids = HashSet::new();
    for device in &devices {
        let Some(id) = device.device_id() else {
            return Err(error::code("E_LAYOUT_INVALID"));
        };
        if !ids.insert(id)
            || !device.x.is_finite()
            || !device.y.is_finite()
            || device.x.abs() > 64.0
            || device.y.abs() > 64.0
        {
            return Err(error::code("E_LAYOUT_INVALID"));
        }
    }
    let host = input::screen_info().unwrap_or(ScreenInfo {
        width: 0,
        height: 0,
    });
    let world = layout::world_from_placements(host, &devices, &[]);
    if world.has_overlaps() {
        return Err(error::code("E_LAYOUT_OVERLAP"));
    }
    let config = state.update_config(|config| {
        config.layout = devices.clone();
        Ok(())
    })?;
    state.engine.update_layout(host, &devices);
    Ok(config)
}

#[tauri::command]
fn save_settings(
    patch: config::SettingsPatch,
    state: tauri::State<'_, AppState>,
) -> Result<AppConfig, String> {
    let config = state.update_config(|config| config.apply_patch(&patch))?;
    let _ = logging::set_level(&config.log_level);
    Ok(config)
}

#[tauri::command]
fn get_logs() -> Vec<logging::LogEntry> {
    logging::snapshot()
}

#[tauri::command]
fn clear_logs() {
    logging::clear();
}

#[tauri::command]
fn reopen_app(app: tauri::AppHandle) {
    app.restart()
}

#[tauri::command]
fn stop_session(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.engine.stop();
    Ok(())
}

#[tauri::command]
fn open_permissions() -> Result<(), String> {
    input::open_permissions()
}

fn main() {
    logging::init("info");

    // Hardware smoke test used to validate the macOS input backend without
    // starting the UI. See README: Development and tests.
    #[cfg(target_os = "macos")]
    if std::env::args().skip(1).any(|arg| arg == "--self-test") {
        std::process::exit(if input::self_test() { 0 } else { 1 });
    }

    let app = tauri::Builder::default()
        .setup(|app| {
            // Settings live in `~/.glidelink/settings.json`.
            let settings_path = match app.path().home_dir() {
                Ok(home) => home.join(".glidelink").join("settings.json"),
                Err(err) => {
                    log::warn!("home directory unavailable ({err}); using app config dir");
                    app.path().app_config_dir()?.join("settings.json")
                }
            };
            let screen = input::screen_info();
            let status = status::Status::new(screen);
            log::info!(
                "starting on {} (build={}, screen={:?}, permissions_ok={})",
                input::platform_name(),
                env!("GLIDELINK_BUILD"),
                screen,
                input::permissions_ok()
            );

            let config = match config::load_or_create(&settings_path) {
                Ok(config) => config,
                Err(load_error) => {
                    log::error!("{load_error}");
                    let (code, detail) = error::split(&load_error);
                    status.set_error(code, detail);
                    AppConfig::default()
                }
            };

            let _ = logging::set_level(&config.log_level);
            app.manage(AppState::new(settings_path, config, status));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            start_session,
            stop_session,
            open_permissions,
            save_settings,
            save_layout,
            preview_pairing_code,
            get_logs,
            clear_logs,
            reopen_app
        ])
        .build(tauri::generate_context!())
        .expect("failed to build GlideLink");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::Exit = event {
            if let Some(state) = app_handle.try_state::<AppState>() {
                state.engine.stop();
            }
        }
    });
}
