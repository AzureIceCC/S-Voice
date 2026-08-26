//! Persistent settings, stored as JSON in the OS config dir.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;

/// Tauri-managed state type alias for the settings store.
pub type ArcState = Arc<parking_lot::RwLock<Settings>>;

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    /// Every field is `#[serde(default)]` so old config files written
    /// before a field was added still deserialize, using the field's
    /// `Default` value for anything missing. Without this, a single
    /// `unwrap_or_default()` on `load()` would wipe the entire settings
    /// file the moment a new field is added or an old one is removed.
    /// The `tests::deserialize_missing_field_uses_default` test pins this.

    /// Hotkey combo, e.g. "AltRight", "Cmd+Shift+Space", "F1".
    #[serde(default)]
    pub hotkey: String,
    /// STT language hint passed to the bridge.
    #[serde(default)]
    pub language: String,
    /// STT model identifier on the bridge side.
    #[serde(default)]
    pub stt_model: String,
    /// Ollama model name.
    #[serde(default)]
    pub ollama_model: String,
    /// Ollama keep_alive window. "30m", "5m", "0" (unload immediately).
    #[serde(default)]
    pub ollama_keep_alive: String,
    /// Whether to run LLM polish before pasting.
    #[serde(default)]
    pub polish_enabled: bool,
    /// Optional system prompt override; empty = use built-in default.
    #[serde(default)]
    pub polish_prompt: String,
    /// Whether to use streaming transcription (mlx-audio's
    /// `generate_streaming` per-chunk partials). Currently NOT wired up —
    /// mlx-audio's AlignAtt-based partials are too noisy and final
    /// re-alignment drops content. Long-term backlog: v0.2 with
    /// faster-whisper. Setting is saved for future use.
    #[serde(default)]
    pub streaming_enabled: bool,
    /// If true, log file is written at debug level (more events) and
    /// pipeline emits verbose tracing. Always-on info logging regardless.
    #[serde(default)]
    pub debug: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            // Default to Cmd+[ — Cmd+Shift+Space is grabbed by macOS
            // Spotlight (输入法切换), and right-side single keys (RCmd,
            // AltRight) don't have independent scancodes on macOS so
            // tauri-plugin-global-shortcut can't register them.
            hotkey: "Cmd+[".to_string(),
            language: "auto".to_string(),
            stt_model: "mlx-community/belle-whisper-large-v3-turbo-zh-fp16".to_string(),
            ollama_model: "qwen3.5:9b-mlx".to_string(),
            ollama_keep_alive: "30m".to_string(),
            // Default to disabled: polish adds 2-7s per turn and 30s timeout
            // on first cold start. Users can re-enable in Settings once
            // they're happy with raw STT speed.
            polish_enabled: false,
            polish_prompt: String::new(),
            // Streaming is reserved for v0.2 (faster-whisper); always off
            // for now and the UI hides the option.
            streaming_enabled: false,
            debug: false,
        }
    }
}

/// Hotkey key names that tauri-plugin-global-shortcut cannot register on
/// macOS. macOS shares scancodes between left/right Cmd/Alt/Shift, so
/// side-specific single keys (AltRight, MetaRight, etc.) and pure-modifier
/// single keys (Cmd, Alt, Shift, Ctrl) all fail to register. Single-key
/// "side-specific" entries (e.g. RCmd) are silently accepted by hotkey::parse
/// but rejected by the platform layer at registration time. Use a
/// modifier+key combo instead (e.g. "Cmd+[").
pub(crate) const UNREGISTERABLE_ON_MACOS: &[&str] = &[
    // Right side, all aliases from hotkey::parse_key
    "MetaRight",
    "RCmd",
    "RightCmd",
    "CmdRight",
    "AltRight",
    "RAlt",
    "RightAlt",
    "OptionRight",
    "ShiftRight",
    // Left side, all aliases — same scancode-sharing reason
    "MetaLeft",
    "CmdLeft",
    "LeftCmd",
    "AltLeft",
    "LAlt",
    "LeftAlt",
    "OptionLeft",
    "ShiftLeft",
    // Pure-modifier single keys (no non-modifier component)
    "Cmd",
    "Super",
    "Meta",
    "Command",
    "Alt",
    "Option",
    "Shift",
    "Ctrl",
    "Control",
];

impl Settings {
    pub fn path() -> PathBuf {
        let mut p = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        p.push("com.s-voice.app");
        let _ = std::fs::create_dir_all(&p);
        p.push("settings.json");
        p
    }

    pub fn load() -> Result<Self, SettingsError> {
        let data = std::fs::read_to_string(Self::path())?;
        let mut s: Self = serde_json::from_str(&data).unwrap_or_default();
        // If sanitize mutated anything (e.g. a previously-stored bad hotkey),
        // persist immediately so the file is consistent with what's in memory.
        // We swallow save errors here — sanitize has already logged a warning
        // and the in-memory value is correct for this run regardless.
        if s.sanitize() {
            if let Err(e) = s.save() {
                tracing::warn!(
                    "failed to persist sanitized settings to {}: {e:#}",
                    Self::path().display()
                );
            } else {
                tracing::info!("persisted sanitized settings to {}", Self::path().display());
            }
        }
        Ok(s)
    }

    pub fn save(&self) -> Result<(), SettingsError> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(Self::path(), json)?;
        Ok(())
    }

    /// Fix up obviously bad values from a previous run. Currently:
    /// - Reset `hotkey` to the default if it doesn't contain a non-modifier key.
    ///
    /// If a mutation happened, the caller is expected to persist via
    /// [`Settings::save`] — we don't save here to keep this function pure
    /// and testable. See `Settings::load_and_sanitize` for the
    /// "load + sanitize + persist" combo used at startup.
    pub fn sanitize(&mut self) -> bool {
        let mut changed = false;

        // Empty / whitespace-only hotkey would slip past the modifier-only
        // check below (`"".split('+')` yields `[""]` which isn't a modifier).
        if self.hotkey.trim().is_empty() {
            tracing::warn!("hotkey is empty; resetting to default");
            self.hotkey = "Cmd+[".to_string();
            changed = true;
        } else {
            // A valid hotkey must mention at least one non-modifier token. We
            // don't re-parse here to avoid a hotkey <-> settings circular dep;
            // a quick membership check covers all the cases that have shown up
            // in practice (e.g. just "Cmd", "Shift", "Alt").
            // Note: "RCmd" / "RAlt" / "MetaRight" etc. are now non-modifier keys
            // (see hotkey::parse), so they don't appear in the blacklist — a
            // single "RCmd" must be accepted as a valid (non-modifier) hotkey.
            let has_real_key = self.hotkey.split('+').map(str::trim).any(|p| {
                !matches!(
                    p,
                    "Cmd"
                        | "Super"
                        | "Meta"
                        | "Command"
                        | "LCmd"
                        | "Shift"
                        | "LShift"
                        | "RShift"
                        | "Alt"
                        | "Option"
                        | "LAlt"
                        | "Ctrl"
                        | "Control"
                        | "LCtrl"
                        | "RCtrl"
                )
            });
            if !has_real_key {
                tracing::warn!(
                    "hotkey {:?} has no non-modifier key; resetting to default",
                    self.hotkey
                );
                self.hotkey = "Cmd+[".to_string();
                changed = true;
            }
        }

        // Warn on keys known to be unregisterable on macOS, but DON'T reset —
        // preserve user choice; UI layer can surface a real-time warning later.
        let bad_tokens: Vec<&str> = self
            .hotkey
            .split('+')
            .map(str::trim)
            .filter(|p| UNREGISTERABLE_ON_MACOS.contains(p))
            .collect();
        if !bad_tokens.is_empty() {
            tracing::warn!(
                "hotkey {:?} contains keys that cannot be registered on macOS: {:?}; \
                 press will not work — use a modifier+key combo instead (e.g. \"Cmd+[\")",
                self.hotkey,
                bad_tokens
            );
        }

        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_have_safe_hotkey() {
        let s = Settings::default();
        // Default must be registerable on macOS so a first-run user
        // doesn't immediately hit the "hotkey press not detected" issue.
        assert!(!s.hotkey.trim().is_empty());
        assert!(
            s.hotkey.contains('+'),
            "default hotkey {:?} should be a modifier+key combo",
            s.hotkey
        );
    }

    /// A bad hotkey ("Cmd" alone) must be reset to a working default,
    /// and sanitize must report the change so the caller can persist.
    #[test]
    fn sanitize_resets_modifier_only() {
        let mut s = Settings {
            hotkey: "Cmd".into(),
            ..Settings::default()
        };
        let changed = s.sanitize();
        assert!(changed);
        assert_ne!(s.hotkey, "Cmd", "modifier-only hotkey must be reset");
        assert!(
            s.hotkey.contains('+'),
            "reset hotkey must have a non-modifier key"
        );
    }

    #[test]
    fn sanitize_keeps_valid_hotkey() {
        let mut s = Settings {
            hotkey: "Cmd+[".into(),
            ..Settings::default()
        };
        let changed = s.sanitize();
        assert!(!changed, "valid hotkey must not be changed");
        assert_eq!(s.hotkey, "Cmd+[");
    }

    #[test]
    fn sanitize_resets_empty_hotkey() {
        let mut s = Settings {
            hotkey: "   ".into(),
            ..Settings::default()
        };
        let changed = s.sanitize();
        assert!(changed);
        assert!(s.hotkey.contains('+'));
    }

    /// Unknown keys must NOT be silently replaced — sanitize only
    /// touches modifier-only combos, leaving key typos alone for the
    /// user to notice in the UI.
    #[test]
    fn sanitize_keeps_unknown_key() {
        let mut s = Settings {
            hotkey: "Cmd+NotAKey".into(),
            ..Settings::default()
        };
        let changed = s.sanitize();
        assert!(
            !changed,
            "unknown key in combo is not sanitize's responsibility"
        );
    }

    /// Round-trip through JSON: a serialized Settings must deserialize
    /// back to the same values. Catches drift between `Default`,
    /// `Serialize`, and `Deserialize`.
    #[test]
    fn json_roundtrip() {
        let s = Settings {
            hotkey: "Cmd+Shift+F1".into(),
            language: "en".into(),
            stt_model: "mlx-community/whisper-small".into(),
            ollama_model: "llama3:8b".into(),
            ollama_keep_alive: "1h".into(),
            polish_enabled: true,
            polish_prompt: "be terse".into(),
            streaming_enabled: true,
            debug: false,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.hotkey, s.hotkey);
        assert_eq!(back.polish_enabled, s.polish_enabled);
        assert_eq!(back.streaming_enabled, s.streaming_enabled);
        assert_eq!(back.ollama_model, s.ollama_model);
    }

    /// `streaming_enabled` has `#[serde(default)]`. Old settings files
    /// written before this field existed must still deserialize (using
    /// the default `false`), not fail with "missing field".
    /// Catches the future bug where someone adds a new field without
    /// `#[serde(default)]` and silently wipes every user's settings.
    #[test]
    fn deserialize_missing_field_uses_default() {
        // Hand-written JSON missing `streaming_enabled` and `debug`.
        let json = r#"{
            "hotkey": "Cmd+[",
            "language": "auto",
            "stt_model": "mlx-community/belle",
            "ollama_model": "qwen3.5:9b-mlx",
            "ollama_keep_alive": "30m",
            "polish_enabled": false,
            "polish_prompt": ""
        }"#;
        let s: Settings = serde_json::from_str(json).expect("missing fields must default");
        assert!(!s.streaming_enabled);
        assert!(!s.debug, "missing debug must default to false");
    }
}
