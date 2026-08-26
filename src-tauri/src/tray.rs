//! System tray icon with status menu.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TrayError {
    #[error("tauri: {0}")]
    Tauri(String),
}

pub fn build<R: Runtime>(app: &AppHandle<R>) -> Result<(), TrayError> {
    let show_settings = MenuItem::with_id(app, "show_settings", "设置…", true, None::<&str>)
        .map_err(|e| TrayError::Tauri(e.to_string()))?;
    let toggle = MenuItem::with_id(app, "toggle", "开始/停止录音", true, None::<&str>)
        .map_err(|e| TrayError::Tauri(e.to_string()))?;
    let debug_toggle = MenuItem::with_id(app, "debug_toggle", "切换调试日志", true, None::<&str>)
        .map_err(|e| TrayError::Tauri(e.to_string()))?;
    let show_log = MenuItem::with_id(app, "show_log", "在 Finder 中显示日志…", true, None::<&str>)
        .map_err(|e| TrayError::Tauri(e.to_string()))?;
    let restart_stt = MenuItem::with_id(app, "restart_stt", "重启 STT 服务", true, None::<&str>)
        .map_err(|e| TrayError::Tauri(e.to_string()))?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)
        .map_err(|e| TrayError::Tauri(e.to_string()))?;
    let sep1 = PredefinedMenuItem::separator(app).map_err(|e| TrayError::Tauri(e.to_string()))?;
    let sep2 = PredefinedMenuItem::separator(app).map_err(|e| TrayError::Tauri(e.to_string()))?;

    let menu = Menu::with_items(
        app,
        &[
            &show_settings,
            &toggle,
            &sep1,
            &debug_toggle,
            &show_log,
            &restart_stt,
            &sep2,
            &quit,
        ],
    )
    .map_err(|e| TrayError::Tauri(e.to_string()))?;

    // Tray icon: embedded at compile time. This is the *menu bar* icon —
    // we want a transparent-background alpha mask (white was knocked out),
    // distinct from the app/Dock icon (which keeps the white backdrop).
    const TRAY_ICON_PNG: &[u8] = include_bytes!("../icons/tray-128x128@2x.png");
    let tray_icon = tauri::image::Image::from_bytes(TRAY_ICON_PNG)
        .map_err(|e| TrayError::Tauri(format!("tray icon: {e}")))?;

    let _tray = TrayIconBuilder::with_id("main")
        .tooltip("S-Voice")
        .icon(tray_icon)
        // Treat icon as a macOS template image: alpha channel is the mask,
        // macOS auto-tints it for light/dark menu bar. The PNG has RGB=0
        // and alpha=shape so this is safe.
        .icon_as_template(true)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show_settings" => {
                if let Some(w) = app.get_webview_window("settings") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "toggle" => {
                let handle = app.state::<crate::pipeline::PipelineHandle>();
                handle.toggle();
            }
            "debug_toggle" => {
                let settings = app.state::<crate::ArcState>();
                let new_value = !settings.read().debug;
                settings.write().debug = new_value;
                let _ = settings.read().save();
                if let Err(e) = crate::logging::set_debug(new_value) {
                    tracing::warn!("tray debug toggle failed: {e}");
                }
                tracing::info!("debug toggled via tray: {}", new_value);
            }
            "show_log" => {
                let dir = crate::logging::log_dir();
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::process::Command::new("open").arg(&dir).spawn();
            }
            "restart_stt" => {
                let _ =
                    std::process::Command::new(crate::stt_client::stt_stop_script_path()).output();
                if let Err(e) =
                    std::process::Command::new(crate::stt_client::stt_script_path()).output()
                {
                    tracing::warn!("restart stt failed: {e}");
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(w) = app.get_webview_window("settings") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
        })
        .build(app)
        .map_err(|e| TrayError::Tauri(e.to_string()))?;

    Ok(())
}
