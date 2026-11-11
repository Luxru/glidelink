use crate::state::AppState;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

struct TrayMenu {
    show: MenuItem<tauri::Wry>,
    stop: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
}

fn labels(language: &str) -> [&'static str; 3] {
    let chinese = language == "zh";
    if chinese {
        ["显示 rx-switch", "停止会话", "退出 rx-switch"]
    } else {
        ["Show rx-switch", "Stop session", "Quit rx-switch"]
    }
}

pub fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        #[cfg(target_os = "macos")]
        if let Err(err) = app.set_activation_policy(tauri::ActivationPolicy::Regular) {
            log::error!("could not restore Dock visibility: {err}");
        }
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn hide_window(window: &tauri::Window) -> tauri::Result<()> {
    window.hide()?;
    #[cfg(target_os = "macos")]
    window
        .app_handle()
        .set_activation_policy(tauri::ActivationPolicy::Accessory)?;
    Ok(())
}

pub fn update_language(app: &AppHandle, language: &str) {
    if let Some(menu) = app.try_state::<TrayMenu>() {
        let [show, stop, quit] = labels(language);
        let _ = menu.show.set_text(show);
        let _ = menu.stop.set_text(stop);
        let _ = menu.quit.set_text(quit);
    }
}

pub fn setup(app: &AppHandle, language: &str) -> tauri::Result<()> {
    let [show, stop, quit] = labels(language);
    let show = MenuItem::with_id(app, "show", show, true, None::<&str>)?;
    let stop = MenuItem::with_id(app, "stop", stop, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", quit, true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show, &stop, &separator, &quit])?;
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("rx-switch")
        .menu(&menu)
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_window(app),
            "stop" => app.state::<AppState>().engine.stop(),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if !cfg!(target_os = "macos")
                && matches!(
                    event,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    }
                )
            {
                show_window(tray.app_handle());
            }
        });
    #[cfg(target_os = "macos")]
    {
        tray = tray
            .icon(tauri::include_image!("icons/tray-template.png"))
            .icon_as_template(true);
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    app.manage(TrayMenu { show, stop, quit });
    Ok(())
}
