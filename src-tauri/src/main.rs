#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod error;
mod input;
mod layout;
mod net;
mod state;
mod status;

use config::AppConfig;
use state::{AppState, StateView};
use tauri::Manager;

#[tauri::command]
fn get_state(state: tauri::State<'_, AppState>) -> StateView {
    StateView::build(&state)
}

#[tauri::command]
fn start_session(mut config: AppConfig, state: tauri::State<'_, AppState>) -> Result<(), String> {
    config.passcode = config.passcode.trim().to_string();
    config.peer_addr = config.peer_addr.trim().to_string();
    config.validate()?;

    if state.engine.is_running() {
        return Err(error::code("E_SESSION_RUNNING"));
    }

    state.save_config(&config)?;
    if let Ok(mut current) = state.config.lock() {
        *current = config.clone();
    }
    log::info!(
        "starting {} session (peer={}, port={}, passcode={})",
        config.role.as_str(),
        if config.peer_addr.is_empty() {
            "-"
        } else {
            &config.peer_addr
        },
        config.listen_port,
        config.redacted_passcode()
    );
    state.engine.start(&config)
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

    let app = tauri::Builder::default()
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            std::fs::create_dir_all(&config_dir)?;
            let config_path = config_dir.join("config.json");
            let screen = input::screen_info();
            let status = status::Status::new(screen);

            let config = match config::load_or_create(&config_path) {
                Ok(config) => config,
                Err(load_error) => {
                    log::error!("{load_error}");
                    let (code, detail) = error::split(&load_error);
                    status.set_error(code, detail);
                    AppConfig::default()
                }
            };

            app.manage(AppState::new(config_path, config, status));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            start_session,
            stop_session,
            open_permissions
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
