//! Output: write text to clipboard, then simulate Cmd+V to paste at cursor.

use tauri::AppHandle;
use tauri_plugin_clipboard_manager::ClipboardExt;
use thiserror::Error;

const MAX_OSASCRIPT_STDERR_CHARS: usize = 512;

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

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> u8;
}

/// Query the current Accessibility trust state without displaying a system
/// prompt. `AXIsProcessTrustedWithOptions` can prompt; the parameterless API
/// deliberately cannot, which keeps the paste hot path side-effect free.
#[cfg(target_os = "macos")]
fn accessibility_is_trusted() -> bool {
    // SAFETY: AXIsProcessTrusted takes no arguments and returns macOS Boolean
    // (an unsigned byte). ApplicationServices is linked above on macOS only.
    unsafe { AXIsProcessTrusted() != 0 }
}

/// Try the fast CGEventPost path: synthesize a Cmd+V keystroke directly
/// via the Quartz Event Services. This skips the AppleScript interpreter
/// and Apple Events IPC, taking ~10-30ms instead of the osascript path's
/// 100-500ms. Requires the Accessibility permission to be granted to the
/// S-Voice binary in System Settings (one-time).
///
/// Returns `Err` if the event source or keyboard events can't be created.
/// Accessibility trust is checked by the caller before entering this path.
#[cfg(target_os = "macos")]
fn paste_via_cgevent() -> Result<(), String> {
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    // CombinedSessionState mirrors the historical "unified input" semantics:
    // it sees the same HID events a real user would, ignoring events we
    // synthesize ourselves (so we don't feed our own Cmd+V back into the
    // pipeline). Private would also work but could in theory cause loops.
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|_| "CGEventSource::new(CombinedSessionState) failed")?;

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

fn bounded_stderr(stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let mut chars = stderr.chars();
    let bounded: String = chars.by_ref().take(MAX_OSASCRIPT_STDERR_CHARS).collect();
    if chars.next().is_some() {
        format!("{bounded}…")
    } else {
        bounded
    }
}

fn run_osascript_with<F>(script: &str, runner: F) -> Result<std::process::ExitStatus, OutputError>
where
    F: FnOnce(&str) -> std::io::Result<std::process::Output>,
{
    let output = runner(script)
        .map_err(|e| OutputError::Paste(format!("failed to start osascript: {e}")))?;

    if output.status.success() {
        return Ok(output.status);
    }

    let stderr = bounded_stderr(&output.stderr);
    let detail = if stderr.trim().is_empty() {
        "no stderr".to_string()
    } else {
        format!("stderr: {}", stderr.trim())
    };
    Err(OutputError::Paste(format!(
        "osascript exited with {} ({detail})",
        output.status
    )))
}

/// Write `text` to clipboard then simulate Cmd+V to paste at cursor.
///
/// On macOS, we use the fast CGEventPost path when Accessibility is trusted.
/// Otherwise, or if Quartz event creation fails, we try the slower AppleScript
/// path and propagate any AppleScript permission or execution failure.
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

    // 2. Synthesize Cmd+V. Prefer the fast CGEventPost path when trusted;
    // otherwise fall back to osascript.
    #[cfg(target_os = "macos")]
    {
        if accessibility_is_trusted() {
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
                    tracing::warn!("CGEventPost setup failed ({e}); falling back to osascript");
                }
            }
        } else {
            tracing::warn!(
                "Accessibility is not granted; skipping CGEventPost and falling back to \
                 osascript. For faster paste, grant S-Voice access in System Settings → \
                 Privacy & Security → Accessibility."
            );
        }
    }

    // 3. Fallback: AppleScript path. Slower (100-500ms); permission and
    // execution failures are surfaced through its exit status and stderr.
    let script = r#"
        tell application "System Events"
            keystroke "v" using {command down}
        end tell
    "#;
    let status = run_osascript_with(script, |script| {
        std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
    })?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    fn output(code: i32, stderr: impl Into<Vec<u8>>) -> std::process::Output {
        std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: Vec::new(),
            stderr: stderr.into(),
        }
    }

    #[test]
    fn osascript_success_is_accepted() {
        let result = run_osascript_with("ignored", |_| Ok(output(0, Vec::new())));
        assert!(result.is_ok());
    }

    #[test]
    fn osascript_nonzero_exit_includes_bounded_stderr() {
        let stderr = "x".repeat(MAX_OSASCRIPT_STDERR_CHARS + 20);
        let err = run_osascript_with("ignored", |_| Ok(output(1, stderr)))
            .expect_err("non-zero osascript exit must fail")
            .to_string();

        assert!(err.contains("osascript exited with exit status: 1"));
        assert!(err.contains(&"x".repeat(MAX_OSASCRIPT_STDERR_CHARS)));
        assert!(!err.contains(&"x".repeat(MAX_OSASCRIPT_STDERR_CHARS + 1)));
        assert!(err.ends_with("…)"));
    }

    #[test]
    fn osascript_spawn_failure_is_reported() {
        let err = run_osascript_with("ignored", |_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "missing test executable",
            ))
        })
        .expect_err("spawn failure must fail")
        .to_string();

        assert!(err.contains("failed to start osascript"));
        assert!(err.contains("missing test executable"));
    }
}
