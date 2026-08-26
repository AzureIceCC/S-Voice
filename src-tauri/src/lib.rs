//! S-Voice — local AI voice input desktop app.
//!
//! Architecture:
//!   mic (cpal) -> wav bytes -> STT bridge (HTTP) -> text
//!                          -> Ollama polish (HTTP, keep_alive=30m) -> text
//!                          -> clipboard + simulate Cmd+V
//!
//! See README for the full pipeline diagram.

mod audio;
mod hotkey;
mod logging;
mod output;
mod pipeline;
mod polish;
mod settings;
mod stt_client;
mod tray;

use std::sync::Arc;
use tauri::{Emitter, LogicalPosition, Manager};

use crate::pipeline::PipelineActor;
use crate::settings::{ArcState, Settings};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Load settings first so logging can pick up the debug flag.
    let initial_settings = Settings::load().unwrap_or_default();
    let _ = logging::init(initial_settings.debug);

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(move |app| {
            let settings = Arc::new(parking_lot::RwLock::new(initial_settings));

            // Build the pipeline actor and get its handle (stored as Tauri state).
            let handle = PipelineActor::spawn(app.handle().clone(), Arc::clone(&settings));
            app.manage(handle);
            app.manage(Arc::clone(&settings));

            // Register initial hotkey.
            let combo = settings.read().hotkey.clone();
            if let Err(e) = crate::hotkey::reregister(app.handle(), &combo) {
                tracing::error!("failed to register hotkey: {e:#}");
            }

            // Build tray menu.
            if let Err(e) = crate::tray::build(app.handle()) {
                tracing::error!("failed to build tray: {e:#}");
            }

            // Position the persistent floating panel just above the macOS dock,
            // centered horizontally on the primary monitor. The dock height
            // is platform-defined (≈80pt); we add a small margin so the panel
            // doesn't visually touch the dock.
            if let Some(floating) = app.get_webview_window("floating") {
                if let Some(monitor) = app.primary_monitor().ok().flatten() {
                    let mon_size = monitor.size();
                    let scale = monitor.scale_factor();
                    let win_w: f64 = 280.0;
                    let win_h: f64 = 64.0;
                    let dock_h: f64 = 80.0;
                    let margin: f64 = 24.0;
                    // Convert physical pixels to logical points.
                    let logical_w = (mon_size.width as f64) / scale;
                    let logical_h = (mon_size.height as f64) / scale;
                    let x = (logical_w - win_w) / 2.0;
                    let y = logical_h - win_h - dock_h - margin;
                    let _ = floating.set_position(LogicalPosition::new(x, y));
                }
            }

            // STT bridge startup protection: if the bridge isn't reachable,
            // try to start it via start_stt.sh. Long-idle bridges have been
            // observed to exit (likely MLX resource reaping) so we don't want
            // S-Voice to come up with a dead bridge silently. The floating
            // panel listens for `bridge-status` to surface the result.
            let bridge_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let emit = |status: &str| {
                    let _ = bridge_app.emit("bridge-status", status);
                };
                match crate::stt_client::health().await {
                    Ok(h) => {
                        tracing::info!(
                            "STT bridge healthy at startup: model={} lang={}",
                            h.model,
                            h.default_language
                        );
                        emit("ready");
                    }
                    Err(e) => {
                        tracing::warn!("STT bridge unreachable at startup ({e}); auto-starting");
                        emit("starting");
                        let script = crate::stt_client::stt_script_path();
                        match std::process::Command::new(&script).output() {
                            Ok(out) if out.status.success() => {
                                tracing::info!(
                                    "STT bridge auto-started: {}",
                                    String::from_utf8_lossy(&out.stdout).trim()
                                );
                                emit("ready");
                            }
                            Ok(out) => {
                                tracing::error!(
                                    "STT bridge auto-start failed: {}",
                                    String::from_utf8_lossy(&out.stderr).trim()
                                );
                                emit("failed");
                            }
                            Err(e) => {
                                tracing::error!("STT bridge spawn failed: {e:#}");
                                emit("failed");
                            }
                        }
                    }
                }
            });

            tracing::info!("s-voice ready (debug={})", settings.read().debug);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            cmd_get_settings,
            cmd_update_settings,
            cmd_get_state,
            // Debug-only test commands. Compiled into dev builds (for
            // interactive troubleshooting) and stripped from release builds
            // so they don't ship in the .app bundle. The settings UI
            // handles the missing-command error gracefully (it'll show
            // "test command not available" on release).
            #[cfg(debug_assertions)]
            cmd_test_stt,
            #[cfg(debug_assertions)]
            cmd_test_polish,
            cmd_log_path,
            cmd_open_log_dir,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Hide instead of quit when user closes the settings window.
                if window.label() == "settings" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // Exit-time settings save: defensive backstop. Normal flow is
            // "every UI change immediately calls Settings::save", so this
            // is a no-op in the common case — but it covers the rare
            // scenario where a setting was mutated in memory (e.g. via
            // the tray menu toggle) and the process exits before the
            // explicit save commits. Doesn't help against SIGKILL but
            // covers graceful Cmd+Q / Quit / app.exit() paths.
            if let tauri::RunEvent::Exit = event {
                // Clone the Settings out of the RwLock guard so the temp
                // borrow ends with this line and doesn't outlive `state`.
                let snapshot = app_handle.state::<ArcState>().read().clone();
                if let Err(e) = snapshot.save() {
                    tracing::warn!("exit-time settings save failed: {e:#}");
                } else {
                    tracing::info!("exit-time settings save ok");
                }
            }
        });
}

// ----- Tauri commands (called from JS via invoke) -----

#[tauri::command]
fn cmd_get_settings(state: tauri::State<Arc<parking_lot::RwLock<Settings>>>) -> Settings {
    state.read().clone()
}

#[tauri::command]
fn cmd_update_settings(
    new_settings: Settings,
    state: tauri::State<Arc<parking_lot::RwLock<Settings>>>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let new_hotkey;
    let new_debug;
    {
        let mut s = state.write();
        new_hotkey = new_settings.hotkey.clone();
        new_debug = new_settings.debug;
        *s = new_settings;
        if let Err(e) = s.save() {
            return Err(format!("save failed: {e:#}"));
        }
    }
    // Re-register hotkey if it changed.
    if let Err(e) = crate::hotkey::reregister(&app, &new_hotkey) {
        return Err(format!("hotkey update failed: {e:#}"));
    }
    // Reconfigure logging if the debug flag changed.
    if let Err(e) = crate::logging::set_debug(new_debug) {
        tracing::warn!("logging toggle failed: {e}");
    }
    Ok(())
}

#[tauri::command]
fn cmd_log_path() -> String {
    crate::logging::current_log_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(log dir not available)".to_string())
}

#[tauri::command]
fn cmd_open_log_dir() -> Result<(), String> {
    let dir = crate::logging::log_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("create dir: {e}"))?;
    std::process::Command::new("open")
        .arg(&dir)
        .spawn()
        .map_err(|e| format!("open finder: {e}"))?;
    Ok(())
}

#[tauri::command]
fn cmd_get_state(handle: tauri::State<crate::pipeline::PipelineHandle>) -> String {
    handle.snapshot().state.as_str().to_string()
}

/// Debug-only: poke the STT bridge and return its `health` summary.
#[cfg(debug_assertions)]
#[tauri::command]
async fn cmd_test_stt() -> Result<String, String> {
    crate::stt_client::health_string()
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Debug-only: run a single sample through the polish step end-to-end.
#[cfg(debug_assertions)]
#[tauri::command]
async fn cmd_test_polish(
    text: String,
    settings: tauri::State<'_, Arc<parking_lot::RwLock<Settings>>>,
) -> Result<String, String> {
    let s = settings.read().clone();
    crate::polish::polish(&text, &s.ollama_model, &s.ollama_keep_alive)
        .await
        .map_err(|e| format!("{e:#}"))
}
