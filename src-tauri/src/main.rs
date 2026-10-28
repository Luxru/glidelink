#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod error;
mod input;
mod invite;
mod layout;
mod net;
mod sched;
mod state;
mod status;

use config::{AppConfig, Role};
use invite::Invite;
use state::{AppState, StateView};
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
    let invite = Invite::decode(&pairing_code).map_err(|_| error::code("E_PAIRING_CODE"))?;
    config.local_ip = config.local_ip.trim().to_string();
    config.validate()?;
    if config.role == Role::Agent && config.local_ipv4()? != invite.ip {
        return Err(error::code("E_LOCAL_IP"));
    }

    if state.engine.is_running() {
        return Err(error::code("E_SESSION_RUNNING"));
    }

    let incoming = config;
    let config = state.update_config(move |next| {
        let previous = next.clone();
        *next = incoming.clone();
        next.inherit_persisted_settings(&previous);
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
    state.engine.start(&config, &pairing_code)
}

#[tauri::command]
fn save_settings(
    patch: config::SettingsPatch,
    state: tauri::State<'_, AppState>,
) -> Result<AppConfig, String> {
    state.update_config(|config| config.apply_patch(&patch))
}

#[tauri::command]
fn preview_invite(local_ip: String) -> Result<String, String> {
    let ip = local_ip
        .trim()
        .parse::<Ipv4Addr>()
        .map_err(|_| error::code("E_LOCAL_IP"))?;
    if ip.is_loopback() || ip.is_unspecified() {
        return Err(error::code("E_LOCAL_IP"));
    }
    net::preview_invite(ip)
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
    let _ = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("glidelink=info"),
    )
    .try_init();

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

            app.manage(AppState::new(settings_path, config, status));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            start_session,
            stop_session,
            open_permissions,
            preview_invite,
            save_settings,
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
