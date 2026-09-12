//! S-Voice — local AI voice input desktop app.
//!
//! Architecture:
//!   mic (cpal) -> wav bytes -> STT bridge (HTTP) -> text
//!                          -> Ollama polish (HTTP, keep_alive=30m) -> text
//!                          -> clipboard + simulate Cmd+V
//!
//! See README for the full pipeline diagram.

mod apple_speech;
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
use crate::settings::{ArcState, SaveOutcome, Settings};

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
            if settings.read().stt_backend == "local_whisper" {
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
                            tracing::warn!(
                                "STT bridge unreachable at startup ({e}); auto-starting"
                            );
                            emit("starting");
                            let script = crate::stt_client::stt_script_path();
                            let result = tokio::task::spawn_blocking(move || {
                                std::process::Command::new(&script).output()
                            })
                            .await;
                            match result {
                                Ok(Ok(out)) if out.status.success() => {
                                    tracing::info!(
                                        "STT bridge auto-started: {}",
                                        String::from_utf8_lossy(&out.stdout).trim()
                                    );
                                    emit("ready");
                                }
                                Ok(Ok(out)) => {
                                    tracing::error!(
                                        "STT bridge auto-start failed: {}",
                                        String::from_utf8_lossy(&out.stderr).trim()
                                    );
                                    emit("failed");
                                }
                                Ok(Err(e)) => {
                                    tracing::error!("STT bridge spawn failed: {e:#}");
                                    emit("failed");
                                }
                                Err(e) => {
                                    tracing::error!("STT bridge start worker failed: {e:#}");
                                    emit("failed");
                                }
                            }
                        }
                    }
                });
            } else {
                tracing::info!("Apple Speech backend selected; skipping local STT bridge startup");
            }

            tracing::info!("s-voice ready (debug={})", settings.read().debug);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            cmd_get_settings,
            cmd_update_settings,
            cmd_reload_settings,
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
                // `save` takes `&mut self` to update the in-memory
                // last_known_disk snapshot (#11); RunEvent::Exit is
                // out of the command path so the snapshot just gets
                // dropped along with the local — that's fine.
                let mut snapshot = app_handle.state::<ArcState>().read().clone();
                match snapshot.save() {
                    Ok(SaveOutcome::Written) => tracing::info!("exit-time settings save written"),
                    Ok(SaveOutcome::Unchanged) => {
                        tracing::debug!("exit-time settings save unchanged")
                    }
                    Ok(SaveOutcome::ExternalEditConflict) => tracing::warn!(
                        "exit-time settings save skipped because settings.json changed externally"
                    ),
                    Err(e) => tracing::warn!("exit-time settings save failed: {e:#}"),
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
    let mut current = state.write();
    let old_debug = current.debug;
    let committed = transactional_settings_update(
        &current,
        new_settings,
        |hotkey| {
            crate::hotkey::parse(hotkey)
                .map(|_| ())
                .map_err(|e| format!("{e:#}"))
        },
        |hotkey| crate::hotkey::reregister(&app, hotkey).map_err(|e| format!("{e:#}")),
        |candidate| candidate.save().map_err(|e| format!("{e:#}")),
    )?;
    let new_debug = committed.debug;
    *current = committed;
    drop(current);

    // Reconfigure logging if the debug flag changed.
    if new_debug != old_debug {
        if let Err(e) = crate::logging::set_debug(new_debug) {
            tracing::warn!("logging toggle failed: {e}");
        }
    }
    Ok(())
}

fn transactional_settings_update<Validate, Register, Persist>(
    current: &Settings,
    incoming: Settings,
    validate_hotkey: Validate,
    mut register_hotkey: Register,
    persist: Persist,
) -> Result<Settings, String>
where
    Validate: FnOnce(&str) -> Result<(), String>,
    Register: FnMut(&str) -> Result<(), String>,
    Persist: FnOnce(&mut Settings) -> Result<SaveOutcome, String>,
{
    let mut candidate = current.clone();
    candidate.apply_incoming(incoming);
    let hotkey_changed = candidate.hotkey != current.hotkey;

    if hotkey_changed {
        // Parse before reregister(), which unregisters the old binding first.
        // Invalid user input therefore cannot disturb the working hotkey.
        validate_hotkey(&candidate.hotkey)
            .map_err(|e| format!("invalid hotkey {:?}: {e}", candidate.hotkey))?;
        if let Err(e) = register_hotkey(&candidate.hotkey) {
            return Err(rollback_hotkey_error(
                &mut register_hotkey,
                &candidate.hotkey,
                &current.hotkey,
                format!("hotkey update failed: {e}"),
            ));
        }
    }

    let save_result = persist(&mut candidate);
    match save_result {
        Ok(SaveOutcome::Written | SaveOutcome::Unchanged) => Ok(candidate),
        Ok(SaveOutcome::ExternalEditConflict) => {
            let reason =
                "save refused: settings.json changed externally; reload settings and retry"
                    .to_string();
            if hotkey_changed {
                Err(rollback_hotkey_error(
                    &mut register_hotkey,
                    &candidate.hotkey,
                    &current.hotkey,
                    reason,
                ))
            } else {
                Err(reason)
            }
        }
        Err(e) => {
            let reason = format!("save failed: {e}");
            if hotkey_changed {
                Err(rollback_hotkey_error(
                    &mut register_hotkey,
                    &candidate.hotkey,
                    &current.hotkey,
                    reason,
                ))
            } else {
                Err(reason)
            }
        }
    }
}

fn rollback_hotkey_error<Register>(
    register_hotkey: &mut Register,
    attempted: &str,
    previous: &str,
    reason: String,
) -> String
where
    Register: FnMut(&str) -> Result<(), String>,
{
    match register_hotkey(previous) {
        Ok(()) => format!("{reason}; rolled back hotkey from {attempted:?} to {previous:?}"),
        Err(rollback_error) => format!(
            "{reason}; rollback from {attempted:?} to {previous:?} also failed: {rollback_error}"
        ),
    }
}

/// Re-read `settings.json` from disk and replace the in-memory store.
/// Lets the user (or an automation script) edit `settings.json`
/// directly — for example to swap `ollama_model` from `9b-mlx` to
/// `2b` — without quitting s-voice and without losing the edit to
/// the next `RunEvent::Exit` save-back. Pairs with the diff-skip in
/// `Settings::save` (#11): that fix guarantees the on-disk edit
/// isn't clobbered; this command gives the in-memory side a way to
/// learn about the new value.
///
/// Side effects: if `hotkey` or `debug` changed, re-register the
/// hotkey and toggle the log level. Returns the freshly-loaded
/// `Settings` so the UI can refresh its form fields.
#[tauri::command]
fn cmd_reload_settings(
    state: tauri::State<ArcState>,
    app: tauri::AppHandle,
) -> Result<Settings, String> {
    // Read the freshly-loaded settings from disk first. We deliberately
    // do NOT touch shared state until the OS-side registration has
    // succeeded — see the transactional steps below.
    let new_settings = Settings::load().map_err(|e| format!("load failed: {e:#}"))?;

    // Capture the current in-memory settings so we can detect a real
    // hotkey change (no-op if the user reloaded after a self-edit that
    // already registered) and so we can apply the logging toggle
    // against the previous `debug` value. Cloned under the read lock
    // to keep the borrow brief.
    let old_settings = state.read().clone();
    let hotkey_changed = old_settings.hotkey != new_settings.hotkey;

    // Transactional order:
    //   1. Register the new hotkey with the OS *before* mutating shared
    //      state. If this fails, we best-effort roll back to the old
    //      hotkey (reregister already unregister_all'd, so the user
    //      would otherwise be left with no working toggle) and surface
    //      the error. Shared state is never mutated in the failure
    //      path — the in-memory store still holds the old hotkey.
    //   2. Swap shared state to the freshly-loaded settings.
    //   3. Apply the logging toggle (failures here are non-fatal; the
    //      `tracing::warn!` already in place swallows them).
    //
    // The previous order swapped state first and registered after,
    // which left shared state pointing at the new hotkey while the OS
    // still had the old hotkey bound if `reregister` failed. The next
    // `cmd_update_settings` would then write the new hotkey to disk
    // and re-register, masking the inconsistency — but only until a
    // crash. This order keeps state and OS in lockstep on every path.
    //
    // Note: this method does not introduce any new disk writes, so
    // external `settings.json` edits picked up by `Settings::load` are
    // not clobbered here.
    if hotkey_changed {
        if let Err(e) = crate::hotkey::reregister(&app, &new_settings.hotkey) {
            // reregister already called unregister_all(), so the old OS
            // binding is gone. Best-effort restore the old hotkey so the
            // user isn't left without a working toggle. Shared state is
            // intentionally not touched in either branch — the on-disk
            // reload was a no-op from the in-memory store's perspective.
            match crate::hotkey::reregister(&app, &old_settings.hotkey) {
                Ok(()) => {
                    return Err(format!(
                        "hotkey re-register failed for {:?}: {e:#}; rolled back to {:?}",
                        new_settings.hotkey, old_settings.hotkey
                    ));
                }
                Err(rollback_err) => {
                    return Err(format!(
                        "hotkey re-register failed for {:?}: {e:#}; rollback to {:?} also failed: {rollback_err:#}",
                        new_settings.hotkey, old_settings.hotkey
                    ));
                }
            }
        }
    }
    {
        let mut s = state.write();
        *s = new_settings.clone();
    }
    if new_settings.debug != old_settings.debug {
        if let Err(e) = crate::logging::set_debug(new_settings.debug) {
            tracing::warn!("logging toggle failed: {e}");
        }
    }
    tracing::info!(
        "settings reloaded from disk: hotkey={:?} model={:?} polish_enabled={}",
        new_settings.hotkey,
        new_settings.ollama_model,
        new_settings.polish_enabled
    );
    Ok(new_settings)
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

#[cfg(test)]
mod settings_transaction_tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    fn incoming_with_hotkey(current: &Settings, hotkey: &str) -> Settings {
        let mut incoming: Settings =
            serde_json::from_str(&serde_json::to_string(current).unwrap()).unwrap();
        incoming.hotkey = hotkey.to_string();
        incoming
    }

    #[test]
    fn invalid_hotkey_does_not_register_or_persist() {
        let current = Settings::default();
        let registered = Cell::new(false);
        let persisted = Cell::new(false);
        let result = transactional_settings_update(
            &current,
            incoming_with_hotkey(&current, "Cmd"),
            |_| Err("modifier only".into()),
            |_| {
                registered.set(true);
                Ok(())
            },
            |_| {
                persisted.set(true);
                Ok(SaveOutcome::Written)
            },
        );

        assert!(result.unwrap_err().contains("invalid hotkey"));
        assert!(!registered.get());
        assert!(!persisted.get());
    }

    #[test]
    fn rejected_hotkey_restores_previous_registration() {
        let current = Settings::default();
        let calls = RefCell::new(Vec::new());
        let result = transactional_settings_update(
            &current,
            incoming_with_hotkey(&current, "Cmd+F1"),
            |_| Ok(()),
            |hotkey| {
                calls.borrow_mut().push(hotkey.to_string());
                if hotkey == "Cmd+F1" {
                    Err("occupied".into())
                } else {
                    Ok(())
                }
            },
            |_| panic!("persistence must not run after registration failure"),
        );

        assert!(result.unwrap_err().contains("rolled back hotkey"));
        assert_eq!(*calls.borrow(), vec!["Cmd+F1", "Cmd+["]);
    }

    #[test]
    fn external_edit_conflict_rolls_back_hotkey_and_rejects_update() {
        let current = Settings::default();
        let calls = RefCell::new(Vec::new());
        let result = transactional_settings_update(
            &current,
            incoming_with_hotkey(&current, "Cmd+F2"),
            |_| Ok(()),
            |hotkey| {
                calls.borrow_mut().push(hotkey.to_string());
                Ok(())
            },
            |_| Ok(SaveOutcome::ExternalEditConflict),
        );

        assert!(result.unwrap_err().contains("changed externally"));
        assert_eq!(*calls.borrow(), vec!["Cmd+F2", "Cmd+["]);
        assert_eq!(current.hotkey, "Cmd+[");
    }

    #[test]
    fn persistence_failure_rolls_back_hotkey() {
        let current = Settings::default();
        let calls = RefCell::new(Vec::new());
        let result = transactional_settings_update(
            &current,
            incoming_with_hotkey(&current, "Cmd+F3"),
            |_| Ok(()),
            |hotkey| {
                calls.borrow_mut().push(hotkey.to_string());
                Ok(())
            },
            |_| Err("disk full".into()),
        );

        assert!(result.unwrap_err().contains("disk full"));
        assert_eq!(*calls.borrow(), vec!["Cmd+F3", "Cmd+["]);
    }

    #[test]
    fn successful_update_commits_candidate_without_rollback() {
        let current = Settings::default();
        let calls = RefCell::new(Vec::new());
        let committed = transactional_settings_update(
            &current,
            incoming_with_hotkey(&current, "Cmd+F4"),
            |_| Ok(()),
            |hotkey| {
                calls.borrow_mut().push(hotkey.to_string());
                Ok(())
            },
            |_| Ok(SaveOutcome::Written),
        )
        .unwrap();

        assert_eq!(committed.hotkey, "Cmd+F4");
        assert_eq!(*calls.borrow(), vec!["Cmd+F4"]);
    }

    #[test]
    fn rollback_failure_is_reported() {
        let current = Settings::default();
        let result = transactional_settings_update(
            &current,
            incoming_with_hotkey(&current, "Cmd+F5"),
            |_| Ok(()),
            |hotkey| {
                if hotkey == "Cmd+F5" {
                    Ok(())
                } else {
                    Err("old shortcut unavailable".into())
                }
            },
            |_| Ok(SaveOutcome::ExternalEditConflict),
        );

        let error = result.unwrap_err();
        assert!(error.contains("rollback"));
        assert!(error.contains("old shortcut unavailable"));
    }
}
