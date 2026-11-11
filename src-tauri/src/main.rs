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
mod tray;

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
    app: tauri::AppHandle,
) -> Result<AppConfig, String> {
    let config = state.update_config(|config| config.apply_patch(&patch))?;
    let _ = logging::set_level(&config.log_level);
    tray::update_language(&app, &config.language);
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
fn open_permissions(role: Role) -> Result<(), String> {
    input::open_permissions(role)
}

#[tauri::command]
fn set_tray_language(app: tauri::AppHandle, language: String) {
    tray::update_language(&app, &language);
}

#[tauri::command]
fn close_about(app: tauri::AppHandle) -> Result<(), String> {
    tray::hide_about(&app).map_err(|err| err.to_string())
}

#[derive(serde::Serialize)]
struct AboutInfo {
    language: String,
    build: &'static str,
    platform: &'static str,
}

#[tauri::command]
fn get_about(state: tauri::State<'_, AppState>) -> AboutInfo {
    AboutInfo {
        language: state.config.snapshot().unwrap_or_default().language,
        build: env!("RX_SWITCH_BUILD"),
        platform: input::platform_name(),
    }
}

#[tauri::command]
fn about_ready(app: tauri::AppHandle) -> Result<(), String> {
    tray::about_ready(&app).map_err(|err| err.to_string())
}

fn main() {
    logging::init("info");

    // Hardware smoke test used to validate the macOS input backend without
    // starting the UI.
    #[cfg(target_os = "macos")]
    if std::env::args().skip(1).any(|arg| arg == "--self-test") {
        std::process::exit(if input::self_test() { 0 } else { 1 });
    }

    let app = tauri::Builder::default()
        // Tao's own foreground Raw Input registration can suppress our global
        // keyboard hook while the settings WebView has focus. Capture owns its
        // own mouse INPUTSINK registration and dedicated message loop.
        .device_event_filter(tauri::DeviceEventFilter::Always)
        .setup(|app| {
            let settings_path = match app.path().home_dir() {
                Ok(home) => home.join(".rx-switch").join("settings.json"),
                Err(err) => {
                    log::warn!("home directory unavailable ({err}); using app config dir");
                    app.path().app_config_dir()?.join("settings.json")
                }
            };
            let screen = input::screen_info();
            let status = status::Status::new(screen);
            log::info!(
                "starting on {} (build={}, screen={:?}, permissions={:?})",
                input::platform_name(),
                env!("RX_SWITCH_BUILD"),
                screen,
                input::permissions()
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
            let language = config.language.clone();
            app.manage(AppState::new(settings_path, config, status));
            #[cfg(target_os = "macos")]
            input::initialize(app.handle());
            tray::setup(app.handle(), &language)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                match window.label() {
                    "main" => {
                        api.prevent_close();
                        if let Err(err) = tray::hide_window(window) {
                            log::error!("could not hide window in the tray: {err}");
                        }
                    }
                    "about" => {
                        api.prevent_close();
                        if let Err(err) = tray::hide_about(window.app_handle()) {
                            log::error!("could not hide About window: {err}");
                        }
                    }
                    _ => {}
                }
            }
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
            reopen_app,
            set_tray_language,
            close_about,
            get_about,
            about_ready
        ])
        .build(tauri::generate_context!())
        .expect("failed to build rx-switch");

    app.run(|app_handle, event| {
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen {
            has_visible_windows: false,
            ..
        } = event
        {
            tray::show_window(app_handle);
        }
        if let tauri::RunEvent::Exit = event {
            if let Some(state) = app_handle.try_state::<AppState>() {
                state.engine.stop();
            }
        }
    });
}
