use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

struct TrayMenu {
    about: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
}

fn labels(language: &str) -> [&'static str; 2] {
    let chinese = language == "zh";
    if chinese {
        ["关于 rx-switch", "退出 rx-switch"]
    } else {
        ["About rx-switch", "Quit rx-switch"]
    }
}

fn show_about(app: &AppHandle) -> tauri::Result<()> {
    let window = if let Some(window) = app.get_webview_window("about") {
        window
    } else {
        WebviewWindowBuilder::new(app, "about", WebviewUrl::App("about.html".into()))
            .title("rx-switch")
            .inner_size(420.0, 390.0)
            .theme(Some(tauri::Theme::Light))
            .resizable(false)
            .minimizable(false)
            .maximizable(false)
            .skip_taskbar(true)
            .visible(false)
            .center()
            .build()?
    };
    window.show()?;
    window.set_focus()
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
        let [about, quit] = labels(language);
        let _ = menu.about.set_text(about);
        let _ = menu.quit.set_text(quit);
    }
}

pub fn setup(app: &AppHandle, language: &str) -> tauri::Result<()> {
    let [about, quit] = labels(language);
    let about = MenuItem::with_id(app, "about", about, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", quit, true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&about, &separator, &quit])?;
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("rx-switch")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "about" => {
                let app = app.clone();
                // WebView2 window creation must run outside event handlers.
                tauri::async_runtime::spawn_blocking(move || {
                    if let Err(err) = show_about(&app) {
                        log::error!("could not show About window: {err}");
                    }
                });
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
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
    app.manage(TrayMenu { about, quit });
    Ok(())
}
