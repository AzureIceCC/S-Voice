//! Output: write text to clipboard, then simulate Cmd+V to paste at cursor.

use tauri::AppHandle;
use tauri_plugin_clipboard_manager::ClipboardExt;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OutputError {
    #[error("clipboard: {0}")]
    Clipboard(String),
    #[error("paste: {0}")]
    Paste(String),
}

/// macOS virtual keycode for the V key (US ANSI layout). CGEventPost
/// takes keycodes, not characters — `0x09` is the canonical code for V
/// regardless of the active keyboard layout's text output.
#[cfg(target_os = "macos")]
const K_VK_ANSI_V: u16 = 0x09;

/// Try the fast CGEventPost path: synthesize a Cmd+V keystroke directly
/// via the Quartz Event Services. This skips the AppleScript interpreter
/// and Apple Events IPC, taking ~10-30ms instead of the osascript path's
/// 100-500ms. Requires the Accessibility permission to be granted to the
/// S-Voice binary in System Settings (one-time).
///
/// Returns `Err` if the event can't be created (typically: Accessibility
/// permission missing, or running on an unusual keyboard layout where the
/// keycode doesn't map to V). Caller should fall back to osascript.
#[cfg(target_os = "macos")]
fn paste_via_cgevent() -> Result<(), String> {
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    // CombinedSessionState mirrors the historical "unified input" semantics:
    // it sees the same HID events a real user would, ignoring events we
    // synthesize ourselves (so we don't feed our own Cmd+V back into the
    // pipeline). Private would also work but could in theory cause loops.
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .expect("CGEventSource::new(CombinedSessionState) should not fail on macOS");

    // Key down with Cmd modifier.
    let event_down = CGEvent::new_keyboard_event(source.clone(), K_VK_ANSI_V, true)
        .map_err(|_| "CGEventCreateKeyboardEvent(down) returned null")?;
    event_down.set_flags(CGEventFlags::CGEventFlagCommand);
    event_down.post(CGEventTapLocation::HID);

    // Key up — same keycode, no need to set flags (already released).
    let event_up = CGEvent::new_keyboard_event(source, K_VK_ANSI_V, false)
        .map_err(|_| "CGEventCreateKeyboardEvent(up) returned null")?;
    event_up.set_flags(CGEventFlags::CGEventFlagCommand);
    event_up.post(CGEventTapLocation::HID);

    Ok(())
}

/// Write `text` to clipboard then simulate Cmd+V to paste at cursor.
///
/// On macOS, we try the fast CGEventPost path first; if it fails (typically
/// because Accessibility hasn't been granted yet) we fall back to the
/// AppleScript path, which is slower but works as long as AppleScript
/// itself has Accessibility (it does by default in the user session).
///
/// The `Accessibility` permission is required for the synthetic keystroke
/// to land in the active app.
pub fn paste(app: &AppHandle, text: &str) -> Result<(), OutputError> {
    if text.is_empty() {
        return Ok(());
    }

    // 1. Copy to clipboard
    let t_paste = std::time::Instant::now();
    app.clipboard()
        .write_text(text.to_string())
        .map_err(|e| OutputError::Clipboard(e.to_string()))?;
    let clipboard_dur = t_paste.elapsed();
    // Logged at the call site (pipeline.rs) too, but duplicating here gives us
    // a single grep target for "did the paste actually fire" questions.
    tracing::info!(
        "paste fired (text len={}, preview={:?})",
        text.len(),
        text.chars().take(80).collect::<String>()
    );

    // 2. Synthesize Cmd+V. Prefer the fast CGEventPost path; on failure
    // (usually Accessibility not granted), fall back to osascript.
    #[cfg(target_os = "macos")]
    {
        let t_post = std::time::Instant::now();
        match paste_via_cgevent() {
            Ok(()) => {
                let cgevent_dur = t_post.elapsed();
                let total_dur = t_paste.elapsed();
                tracing::info!(
                    "paste timing: clipboard={:?} cgevent={:?} total={:?} path=CGEventPost",
                    clipboard_dur,
                    cgevent_dur,
                    total_dur
                );
                return Ok(());
            }
            Err(e) => {
                tracing::warn!(
                    "CGEventPost failed ({e}); falling back to osascript. \
                     For ~10x faster paste, grant Accessibility in \
                     System Settings → Privacy & Security → Accessibility."
                );
            }
        }
    }

    // 3. Fallback: AppleScript path. Slower (100-500ms) but works without
    // explicitly granting Accessibility (Apple Events has its own session).
    let script = r#"
        tell application "System Events"
            keystroke "v" using {command down}
        end tell
    "#;
    let status = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .status()
        .map_err(|e| OutputError::Paste(e.to_string()))?;
    let total_dur = t_paste.elapsed();
    tracing::info!(
        "paste timing: clipboard={:?} osascript={:?} total={:?} status={} path=osascript",
        clipboard_dur,
        total_dur - clipboard_dur,
        total_dur,
        status
    );

    Ok(())
}
