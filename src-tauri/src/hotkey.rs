//! Global hotkey registration. Supports both single keys ("AltRight") and
//! modifier combos ("Cmd+Shift+Space").

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use thiserror::Error;

use crate::pipeline::PipelineHandle;

#[derive(Debug, Error)]
pub enum HotkeyError {
    #[error("invalid hotkey combo: {0}")]
    InvalidCombo(String),
    #[error("unknown key in combo: {0}")]
    UnknownKey(String),
}

/// (Re)register the global hotkey for the given combo string. Unregisters any
/// previously registered shortcut first. Also registers Esc as a fallback
/// toggle key — handy when the main hotkey is unresponsive.
pub fn reregister(app: &AppHandle, combo: &str) -> Result<(), HotkeyError> {
    let manager = app.global_shortcut();
    // Best-effort unregister all. Some platforms fail here if nothing's registered.
    let _ = manager.unregister_all();

    // Main hotkey — best effort. If parsing/registration fails, we still
    // want to register the Esc fallback so the user can recover.
    match parse(combo) {
        Ok(shortcut) => {
            let app_clone = app.clone();
            let combo_for_log = combo.to_string();
            if let Err(e) = manager.on_shortcut(shortcut, move |_app, _sc, event| {
                if event.state() == ShortcutState::Pressed {
                    tracing::info!("hotkey pressed (main: {combo_for_log})");
                    let handle = app_clone.state::<PipelineHandle>();
                    handle.toggle();
                }
            }) {
                tracing::error!("failed to register main hotkey {combo:?}: {e}");
            } else {
                tracing::info!("hotkey registered: {combo}");
            }
        }
        Err(e) => {
            tracing::error!("main hotkey {combo:?} is invalid: {e}");
        }
    }

    // Esc fallback — always register, regardless of main hotkey state.
    let app_clone = app.clone();
    if let Err(e) = manager.on_shortcut(
        Shortcut::new(None, Code::Escape),
        move |_app, _sc, event| {
            if event.state() == ShortcutState::Pressed {
                tracing::info!("hotkey pressed (fallback: Escape)");
                let handle = app_clone.state::<PipelineHandle>();
                handle.toggle();
            }
        },
    ) {
        tracing::warn!("could not register Esc fallback hotkey: {e}");
    } else {
        tracing::info!("hotkey registered: Escape (fallback)");
    }

    Ok(())
}

/// Parse a combo string like "AltRight", "Cmd+Shift+Space", "Ctrl+Alt+F1".
pub fn parse(s: &str) -> Result<Shortcut, HotkeyError> {
    let mut mods = Modifiers::empty();
    let mut key_code: Option<Code> = None;
    for raw in s.split('+') {
        let p = raw.trim();
        if p.is_empty() {
            continue;
        }
        match p {
            // Modifier aliases — but NOT the side-specific "RCmd" / "RAlt"
            // because those refer to *the key itself*, not a modifier flag.
            // "LCmd" / "LAlt" stay here as the user-facing modifier forms.
            "Cmd" | "Super" | "Meta" | "Command" | "LCmd" => {
                mods |= Modifiers::META;
            }
            "Shift" | "LShift" | "RShift" => mods |= Modifiers::SHIFT,
            "Alt" | "Option" | "LAlt" => mods |= Modifiers::ALT,
            "Ctrl" | "Control" | "LCtrl" | "RCtrl" => mods |= Modifiers::CONTROL,
            // Everything else is treated as a non-modifier key. parse_key
            // recognizes the side-specific names like "AltRight", "CmdLeft",
            // and the standard letter/digit/function keys.
            other => {
                if key_code.is_some() {
                    return Err(HotkeyError::InvalidCombo(format!(
                        "multiple non-modifier keys in '{s}'"
                    )));
                }
                key_code = Some(parse_key(other)?);
            }
        }
    }
    let code = key_code.ok_or_else(|| HotkeyError::InvalidCombo(s.to_string()))?;
    Ok(Shortcut::new(Some(mods), code))
}

fn parse_key(s: &str) -> Result<Code, HotkeyError> {
    let code = match s {
        // Letters
        "A" => Code::KeyA,
        "B" => Code::KeyB,
        "C" => Code::KeyC,
        "D" => Code::KeyD,
        "E" => Code::KeyE,
        "F" => Code::KeyF,
        "G" => Code::KeyG,
        "H" => Code::KeyH,
        "I" => Code::KeyI,
        "J" => Code::KeyJ,
        "K" => Code::KeyK,
        "L" => Code::KeyL,
        "M" => Code::KeyM,
        "N" => Code::KeyN,
        "O" => Code::KeyO,
        "P" => Code::KeyP,
        "Q" => Code::KeyQ,
        "R" => Code::KeyR,
        "S" => Code::KeyS,
        "T" => Code::KeyT,
        "U" => Code::KeyU,
        "V" => Code::KeyV,
        "W" => Code::KeyW,
        "X" => Code::KeyX,
        "Y" => Code::KeyY,
        "Z" => Code::KeyZ,
        // Digits
        "0" => Code::Digit0,
        "1" => Code::Digit1,
        "2" => Code::Digit2,
        "3" => Code::Digit3,
        "4" => Code::Digit4,
        "5" => Code::Digit5,
        "6" => Code::Digit6,
        "7" => Code::Digit7,
        "8" => Code::Digit8,
        "9" => Code::Digit9,
        // Common keys
        "Space" => Code::Space,
        "Tab" => Code::Tab,
        "Enter" | "Return" => Code::Enter,
        "Escape" | "Esc" => Code::Escape,
        "Backspace" => Code::Backspace,
        "Delete" | "Del" => Code::Delete,
        // Arrow keys
        "Up" => Code::ArrowUp,
        "Down" => Code::ArrowDown,
        "Left" => Code::ArrowLeft,
        "Right" => Code::ArrowRight,
        // Function keys
        "F1" => Code::F1,
        "F2" => Code::F2,
        "F3" => Code::F3,
        "F4" => Code::F4,
        "F5" => Code::F5,
        "F6" => Code::F6,
        "F7" => Code::F7,
        "F8" => Code::F8,
        "F9" => Code::F9,
        "F10" => Code::F10,
        "F11" => Code::F11,
        "F12" => Code::F12,
        // Punctuation
        "Minus" | "-" => Code::Minus,
        "Equal" | "=" => Code::Equal,
        "Comma" | "," => Code::Comma,
        "Period" | "." => Code::Period,
        "Slash" | "/" => Code::Slash,
        "Backslash" | "\\" => Code::Backslash,
        "Semicolon" | ";" => Code::Semicolon,
        "Quote" | "'" => Code::Quote,
        "Backquote" | "`" => Code::Backquote,
        "BracketLeft" | "[" => Code::BracketLeft,
        "BracketRight" | "]" => Code::BracketRight,
        // Convenience aliases
        "AltRight" | "RAlt" | "RightAlt" | "OptionRight" => Code::AltRight,
        "AltLeft" | "LAlt" | "LeftAlt" | "OptionLeft" => Code::AltLeft,
        "MetaRight" | "RCmd" | "RightCmd" | "CmdRight" => Code::MetaRight,
        "MetaLeft" | "LCmd" | "LeftCmd" | "CmdLeft" => Code::MetaLeft,
        "ShiftRight" => Code::ShiftRight,
        "ShiftLeft" => Code::ShiftLeft,
        _ => return Err(HotkeyError::UnknownKey(s.to_string())),
    };
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each valid combo should parse to a non-zero shortcut.
    /// Just smoke-test that no panics and the `Result` is `Ok`;
    /// the `Shortcut` struct internals are owned by tauri-plugin-global-shortcut.
    #[test]
    fn parse_standard_combos() {
        for combo in [
            "Cmd+[",
            "Cmd+Shift+Space",
            "Ctrl+Alt+F1",
            "Alt+Tab",
            "Cmd+,", // Cmd+Comma
            "Shift+Escape",
        ] {
            let result = parse(combo);
            assert!(result.is_ok(), "expected Ok for {combo:?}, got {result:?}");
        }
    }

    /// Combos that contain only modifier keys (no actual key) must be rejected.
    /// Otherwise the global-shortcut plugin would register a useless hotkey
    /// and `sanitize` would have nothing to fall back to.
    #[test]
    fn parse_rejects_modifier_only() {
        for bad in ["Cmd", "Cmd+Shift", "Alt+Ctrl", "Shift"] {
            let result = parse(bad);
            assert!(result.is_err(), "expected Err for {bad:?}, got {result:?}");
            // Should be InvalidCombo, not UnknownKey
            match result.unwrap_err() {
                HotkeyError::InvalidCombo(_) => {}
                other => panic!("expected InvalidCombo for {bad:?}, got {other:?}"),
            }
        }
    }

    /// Two non-modifier keys must be rejected, e.g. "Cmd+A+B".
    #[test]
    fn parse_rejects_multiple_keys() {
        let result = parse("Cmd+A+B");
        assert!(result.is_err());
        match result.unwrap_err() {
            HotkeyError::InvalidCombo(msg) => assert!(msg.contains("multiple")),
            other => panic!("expected InvalidCombo, got {other:?}"),
        }
    }

    /// Unknown keys return UnknownKey, not InvalidCombo — so the error
    /// message tells the user which key name is wrong.
    #[test]
    fn parse_rejects_unknown_key() {
        let result = parse("Cmd+NotAKey");
        assert!(result.is_err());
        match result.unwrap_err() {
            HotkeyError::UnknownKey(name) => assert_eq!(name, "NotAKey"),
            other => panic!("expected UnknownKey, got {other:?}"),
        }
    }

    /// `UNREGISTERABLE_ON_MACOS` must at least contain the known-bad keys
    /// that prompted the list. If a future contributor adds a side-specific
    /// variant and forgets to blacklist it, this fails.
    #[test]
    fn unregisterable_keys_listed() {
        for key in ["RCmd", "RAlt", "MetaRight", "AltRight"] {
            assert!(
                crate::settings::UNREGISTERABLE_ON_MACOS.contains(&key),
                "{key:?} must be in UNREGISTERABLE_ON_MACOS"
            );
        }
    }
}
