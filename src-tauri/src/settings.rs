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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveOutcome {
    Unchanged,
    Written,
    ExternalEditConflict,
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
    /// UI display language (`zh` or `en`), independent from STT language.
    #[serde(default = "default_ui_language")]
    pub ui_language: String,
    /// Explicit speech recognizer selection. `local_whisper` preserves the
    /// MLX bridge; `apple_speech` uses the system-managed SpeechAnalyzer.
    /// The pipeline never silently switches between them.
    #[serde(default = "default_stt_backend")]
    pub stt_backend: String,
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
    /// Cached snapshot of the on-disk file content from the last
    /// `load()` or successful `save()`. Used by `save_to` to detect
    /// external edits (#11): if the on-disk file no longer matches
    /// this snapshot, someone has edited `settings.json` outside
    /// the Rust app and we refuse to clobber the change. Pair with
    /// `cmd_reload_settings` for explicit reconciliation.
    /// `#[serde(skip)]` because this is purely runtime state, not
    /// user-tunable config.
    #[serde(skip)]
    pub last_known_disk: Option<String>,
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
            ui_language: default_ui_language(),
            stt_backend: default_stt_backend(),
            stt_model: "mlx-community/belle-whisper-large-v3-turbo-zh-fp16".to_string(),
            ollama_model: "qwen3.5:2b-q4_K_M".to_string(),
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
            // No disk snapshot yet — `load` will populate this on first
            // call. Default::default() is only used when the on-disk
            // file is missing or corrupt (in which case there's
            // nothing to reconcile against anyway).
            last_known_disk: None,
        }
    }
}

fn default_stt_backend() -> String {
    "apple_speech".to_string()
}

fn default_ui_language() -> String {
    "zh".to_string()
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
        // Record what we just read so `save_to` can later detect
        // external edits: if disk content no longer matches `data`
        // at the next save, an external actor (editor, automation
        // script) changed it (#11).
        s.last_known_disk = Some(data);
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

    pub fn save(&mut self) -> Result<SaveOutcome, SettingsError> {
        self.save_to(&Self::path())
    }

    /// Core save implementation, factored out so the safe-save
    /// behaviour can be unit-tested with a per-test temp dir rather
    /// than the user's real `~/Library/Application Support/com.s-voice.app/`.
    ///
    /// Two layers of protection against clobbering user intent (#11):
    ///
    /// **Layer 1 — diff skip.** If the on-disk file already contains
    /// a JSON document that serialises to the same bytes as `self`,
    /// skip the write. `to_string_pretty` is deterministic for a
    /// given struct value, so byte-level comparison is stable.
    /// `trim_end` on both sides tolerates a trailing-newline
    /// difference (editors add one, `serde_json` doesn't).
    ///
    /// **Layer 2 — external-edit detection.** This is the actual
    /// race-fix. `self.last_known_disk` records what `load()` (or a
    /// previous successful `save`) last saw on disk. If the current
    /// on-disk content differs from that snapshot, someone has
    /// edited `settings.json` outside the Rust app — refuse to
    /// write so we don't clobber their change. The user can
    /// reconcile via the "重新加载" button, which calls
    /// `cmd_reload_settings` and refreshes the in-memory state to
    /// match disk. Without this layer, layer-1 alone wouldn't
    /// help: the in-memory stale value (e.g. `9b-mlx`) and the
    /// external edit (e.g. `2b`) are different, so layer-1
    /// triggers a write and clobbers the edit.
    ///
    /// The explicit [`SaveOutcome`] lets transactional callers distinguish a
    /// successful/no-op save from a refused external-edit overwrite.
    pub(crate) fn save_to(&mut self, path: &std::path::Path) -> Result<SaveOutcome, SettingsError> {
        let json = serde_json::to_string_pretty(self)?;
        let disk_now = std::fs::read_to_string(path).ok();

        // Layer 1: in-memory matches disk → nothing to write.
        if let Some(ref existing) = disk_now {
            if existing.trim_end() == json.trim_end() {
                tracing::debug!("save skipped (no change to {})", path.display());
                self.last_known_disk = Some(existing.clone());
                return Ok(SaveOutcome::Unchanged);
            }
        }

        // Layer 2: disk has been touched since we last loaded/saved
        // → external edit detected, refuse to clobber.
        if let (Some(last_known), Some(current)) = (&self.last_known_disk, &disk_now) {
            if last_known.trim_end() != current.trim_end() {
                tracing::warn!(
                    "save skipped: external edit detected at {} (disk changed since last load/save). \
                     Use '重新加载' to reconcile in-memory state with disk.",
                    path.display()
                );
                return Ok(SaveOutcome::ExternalEditConflict);
            }
        }

        std::fs::write(path, &json)?;
        // Refresh the snapshot to what we just wrote. A subsequent
        // save (e.g. another field change in the same session) will
        // see `last_known_disk` matching the new disk content, so
        // layer-2 only fires on genuine external edits.
        self.last_known_disk = Some(json);
        Ok(SaveOutcome::Written)
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

        if !matches!(self.stt_backend.as_str(), "local_whisper" | "apple_speech") {
            tracing::warn!(
                "unknown STT backend {:?}; resetting to apple_speech",
                self.stt_backend
            );
            self.stt_backend = default_stt_backend();
            changed = true;
        }

        if !matches!(self.ui_language.as_str(), "zh" | "en") {
            tracing::warn!(
                "unknown UI language {:?}; resetting to Chinese",
                self.ui_language
            );
            self.ui_language = default_ui_language();
            changed = true;
        }

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

    /// Merge an externally-donated `Settings` (typically a
    /// `cmd_update_settings` DTO deserialized from JS) into `self`,
    /// preserving the trusted in-memory `last_known_disk` snapshot.
    ///
    /// Why this exists (#11): `cmd_update_settings` receives a
    /// `Settings` value from the frontend over the Tauri command
    /// boundary. The `last_known_disk` field is `#[serde(skip)]`, so
    /// every inbound DTO deserializes with `last_known_disk = None`.
    /// A naive `*self = incoming` would clobber the trusted snapshot
    /// that `Settings::save_to` layer-2 relies on to detect external
    /// `settings.json` edits — silently disabling the race-fix for the
    /// rest of the session until the next `cmd_reload_settings`. This
    /// method overwrites every user-tunable field from `incoming` but
    /// preserves `self.last_known_disk`, so subsequent saves still
    /// see the disk state as-of the last load or successful save and
    /// can refuse to clobber a concurrent external edit.
    ///
    /// `cmd_reload_settings` does NOT use this method: its incoming
    /// value comes from `Settings::load`, which already populates
    /// `last_known_disk` correctly.
    pub fn apply_incoming(&mut self, incoming: Settings) {
        let preserved_last_known_disk = self.last_known_disk.clone();
        *self = incoming;
        self.last_known_disk = preserved_last_known_disk;
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
            ui_language: "en".into(),
            stt_backend: "apple_speech".into(),
            stt_model: "mlx-community/whisper-small".into(),
            ollama_model: "llama3:8b".into(),
            ollama_keep_alive: "1h".into(),
            polish_enabled: true,
            polish_prompt: "be terse".into(),
            streaming_enabled: true,
            debug: false,
            last_known_disk: None, // serde-skipped; round-trip must be None
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.hotkey, s.hotkey);
        assert_eq!(back.polish_enabled, s.polish_enabled);
        assert_eq!(back.streaming_enabled, s.streaming_enabled);
        assert_eq!(back.stt_backend, s.stt_backend);
        assert_eq!(back.ui_language, s.ui_language);
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
        assert_eq!(s.stt_backend, "apple_speech");
        assert_eq!(s.ui_language, "zh");
        assert!(!s.streaming_enabled);
        assert!(!s.debug, "missing debug must default to false");
    }

    #[test]
    fn sanitize_resets_unknown_stt_backend_to_default() {
        let mut s = Settings {
            stt_backend: "automatic".into(),
            ..Settings::default()
        };
        assert!(s.sanitize());
        assert_eq!(s.stt_backend, "apple_speech");
    }

    #[test]
    fn sanitize_resets_unknown_ui_language_to_chinese() {
        let mut s = Settings {
            ui_language: "fr".into(),
            ..Settings::default()
        };
        assert!(s.sanitize());
        assert_eq!(s.ui_language, "zh");
    }

    // ----- #11: safe save (diff-skip + external-edit detection) + reload -----

    /// Re-saving an unchanged `Settings` must not rewrite the file —
    /// this is **layer 1** of the safe-save fix (diff-skip). We
    /// assert the file's mtime is preserved across two consecutive
    /// saves of the same value. Without layer 1, every save
    /// (RunEvent::Exit, every cmd_update_settings, every tray
    /// toggle) would touch the file.
    #[test]
    fn save_to_skips_when_unchanged() {
        let dir =
            std::env::temp_dir().join(format!("s-voice-settings-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        let mut s = Settings::default();
        s.last_known_disk = Some(serde_json::to_string_pretty(&s).unwrap());
        assert_eq!(s.save_to(&path).unwrap(), SaveOutcome::Written);
        let mtime_after_first = std::fs::metadata(&path).unwrap().modified().unwrap();
        // Bump mtime forward so a no-op write that happens to land
        // on the same nanosecond would still register. (In practice,
        // save_to skips the write before std::fs::write runs, so
        // mtime must be unchanged.)
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(s.save_to(&path).unwrap(), SaveOutcome::Unchanged);
        let mtime_after_second = std::fs::metadata(&path).unwrap().modified().unwrap();

        assert_eq!(
            mtime_after_first, mtime_after_second,
            "save_to must not touch the file when content is unchanged"
        );

        // Cleanup: don't leak temp dirs into /tmp.
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unchanged_save_refreshes_disk_snapshot() {
        let dir = std::env::temp_dir().join(format!(
            "s-voice-settings-snapshot-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        let mut candidate = Settings {
            hotkey: "Cmd+F6".into(),
            ..Settings::default()
        };
        let disk_json = serde_json::to_string_pretty(&candidate).unwrap();
        std::fs::write(&path, &disk_json).unwrap();
        candidate.last_known_disk = Some("older snapshot".into());

        assert_eq!(candidate.save_to(&path).unwrap(), SaveOutcome::Unchanged);
        assert_eq!(
            candidate.last_known_disk.as_deref(),
            Some(disk_json.as_str())
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Re-saving a *changed* `Settings` (no external edit, just a
    /// fresh UI change) must rewrite the file. Negative control for
    /// the test above — layer 1 triggers only when the in-memory
    /// state matches disk.
    #[test]
    fn save_to_writes_when_changed() {
        let dir =
            std::env::temp_dir().join(format!("s-voice-settings-test2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        let mut s1 = Settings::default();
        s1.last_known_disk = Some(serde_json::to_string_pretty(&s1).unwrap());
        s1.save_to(&path).unwrap();
        let bytes1 = std::fs::read(&path).unwrap();

        // Mutate one field. last_known_disk still points at the
        // pre-mutation disk content, so layer 2 sees
        // last_known == disk (no external edit) and lets the write
        // through. After save, last_known_disk is refreshed to the
        // new content.
        let mut s2 = s1.clone();
        s2.ollama_model = "qwen3.5:2b".into();
        s2.save_to(&path).unwrap();
        let bytes2 = std::fs::read(&path).unwrap();

        assert_ne!(bytes1, bytes2, "save_to must rewrite on UI change");
        let on_disk: Settings = serde_json::from_slice(&bytes2).unwrap();
        assert_eq!(on_disk.ollama_model, "qwen3.5:2b");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The headline #11 invariant: a user (or automation script)
    /// edits `settings.json` on disk to swap the Ollama model from
    /// `9b-mlx` to `2b` while s-voice is running. The in-memory
    /// state hasn't been told. On `RunEvent::Exit`, s-voice calls
    /// `save()` with the stale in-memory snapshot. Layer 2
    /// (external-edit detection) must fire: `last_known_disk`
    /// doesn't match the current disk content, so `save_to`
    /// refuses to write and the user's external edit survives.
    #[test]
    fn external_edit_survives_exit_save() {
        let dir =
            std::env::temp_dir().join(format!("s-voice-settings-test3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        // Phase 1: app starts, loads from disk, records snapshot.
        let mut s = Settings::default();
        assert_eq!(s.ollama_model, "qwen3.5:2b-q4_K_M");
        // Simulate what `Settings::load` does: read disk, snapshot it.
        let disk_json = serde_json::to_string_pretty(&s).unwrap();
        std::fs::write(&path, &disk_json).unwrap();
        s.last_known_disk = Some(disk_json);

        // Phase 2: user (or automation) edits the file externally to
        // swap the model. Bypasses the Rust app entirely.
        let mut external = s.clone();
        external.ollama_model = "qwen3.5:2b".into();
        let external_json = serde_json::to_string_pretty(&external).unwrap();
        std::fs::write(&path, &external_json).unwrap();

        // Phase 3: user quits s-voice. `RunEvent::Exit` fires
        // `state.read().clone().save()`. The in-memory state is
        // still the stale default because nothing told it about
        // the external edit.
        let mut stale = s.clone();
        // Defensive: make sure stale.last_known_disk is exactly
        // what we wrote in phase 1, not the (unwritten) external
        // edit. (The app would have this from the real `load()`.)
        stale.last_known_disk = Some(serde_json::to_string_pretty(&s).unwrap());
        // Layer 1: stale != external, doesn't skip.
        // Layer 2: last_known != disk_now, MUST skip.
        assert_eq!(
            stale.save_to(&path).unwrap(),
            SaveOutcome::ExternalEditConflict
        );

        // The external edit must survive.
        let on_disk: Settings = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            on_disk.ollama_model, "qwen3.5:2b",
            "external edit must not be clobbered by stale exit-save (layer 2)"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// After an external edit is preserved by layer 2, a
    /// `cmd_reload_settings` (simulated here as a re-load) must
    /// pick up the on-disk value. This is the user-facing fix:
    /// they edit settings.json, then click "重新加载", and the
    /// in-memory state catches up.
    #[test]
    fn reload_after_external_edit_picks_up_new_value() {
        let dir =
            std::env::temp_dir().join(format!("s-voice-settings-test4-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        // Phase 1: app starts with the configured default on disk.
        let mut initial = Settings::default();
        let initial_json = serde_json::to_string_pretty(&initial).unwrap();
        std::fs::write(&path, &initial_json).unwrap();
        initial.last_known_disk = Some(initial_json);

        // Phase 2: external edit. App doesn't know yet.
        let mut external = initial.clone();
        external.ollama_model = "qwen3.5:2b".into();
        std::fs::write(&path, serde_json::to_string_pretty(&external).unwrap()).unwrap();

        // Phase 3: user clicks "重新加载" → cmd_reload_settings →
        // Settings::load reads disk, refreshes last_known_disk.
        // We simulate by re-reading the file and updating the
        // last_known_disk snapshot.
        let on_disk_text = std::fs::read_to_string(&path).unwrap();
        let mut reloaded: Settings = serde_json::from_str(&on_disk_text).unwrap();
        reloaded.last_known_disk = Some(on_disk_text);

        assert_eq!(reloaded.ollama_model, "qwen3.5:2b");

        // Phase 4: a subsequent save (e.g. user toggles polish
        // on) must now succeed — the in-memory state matches the
        // new disk content, layer 1 still applies (no change
        // yet), but layer 2 sees last_known == disk_now and lets
        // it through.
        reloaded.polish_enabled = true;
        reloaded.save_to(&path).unwrap();
        let final_text = std::fs::read_to_string(&path).unwrap();
        let final_s: Settings = serde_json::from_str(&final_text).unwrap();
        assert!(final_s.polish_enabled, "save after reload must succeed");
        assert_eq!(final_s.ollama_model, "qwen3.5:2b", "model must persist");

        let _ = std::fs::remove_dir_all(&dir);
    }
    /// #11 + #11 cmd_update_settings DTO replacement regression:
    /// applying an externally-donated `Settings` (the shape that
    /// `cmd_update_settings` receives from the JS frontend) must
    /// overwrite every user-tunable field while preserving the
    /// trusted in-memory `last_known_disk` snapshot — otherwise
    /// `Settings::save_to` layer-2 external-edit detection is
    /// silently disabled for the rest of the session. This test
    /// goes beyond the `save_to` direct tests above and exercises
    /// the full "DTO arrives → apply → save against a concurrently
    /// externally-edited disk" flow that the production command
    /// implements, but on a temp dir.
    #[test]
    fn apply_incoming_preserves_last_known_disk() {
        let dir =
            std::env::temp_dir().join(format!("s-voice-settings-test5-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        // Phase 1: app starts, loads defaults from disk, snapshots
        // the on-disk content as `last_known_disk`. This mirrors
        // what `Settings::load` does on startup.
        let mut current = Settings::default();
        let original_json = serde_json::to_string_pretty(&current).unwrap();
        std::fs::write(&path, &original_json).unwrap();
        current.last_known_disk = Some(original_json.clone());

        // Phase 2: external actor (editor, automation script)
        // swaps the model on disk. App's in-memory state has not
        // been told about this edit.
        let mut external = current.clone();
        external.ollama_model = "qwen3.5:2b".into();
        let external_json = serde_json::to_string_pretty(&external).unwrap();
        std::fs::write(&path, &external_json).unwrap();
        assert_eq!(
            current.ollama_model, "qwen3.5:2b-q4_K_M",
            "in-memory state must still be stale"
        );

        // Phase 3: simulate `cmd_update_settings` receiving an
        // inbound DTO from JS. The DTO is serialised + deserialised
        // to match what Tauri does on the command boundary; this
        // exercises the `#[serde(skip)]` behaviour that gives every
        // inbound DTO `last_known_disk = None`.
        let dto_json = serde_json::to_string(&current).unwrap();
        let mut inbound: Settings = serde_json::from_str(&dto_json).unwrap();
        inbound.polish_enabled = true;
        inbound.hotkey = "Cmd+Shift+F1".into();
        assert_eq!(
            inbound.last_known_disk, None,
            "DTO deserialized from JSON must have last_known_disk=None"
        );

        // Phase 4: production cmd_update_settings path — apply the
        // DTO into the trusted in-memory state.
        current.apply_incoming(inbound);

        // All user-tunable fields must have been overwritten by the
        // DTO. last_known_disk must NOT have been clobbered to None.
        assert!(
            current.polish_enabled,
            "incoming polish_enabled must be applied"
        );
        assert_eq!(
            current.hotkey, "Cmd+Shift+F1",
            "incoming hotkey must be applied"
        );
        assert_eq!(
            current.ollama_model, "qwen3.5:2b-q4_K_M",
            "model stays stale until reload (apply_incoming takes the DTO as-is)"
        );
        assert_eq!(
            current.last_known_disk.as_deref(),
            Some(original_json.as_str()),
            "apply_incoming must preserve the trusted last_known_disk snapshot"
        );

        // Phase 5: the headline invariant. A subsequent save — e.g.
        // triggered by the same UI change — must still trigger
        // layer-2 because last_known_disk != disk_now (the external
        // edit). With the buggy `*s = incoming` replacement the
        // snapshot would be `None` and layer-2 would not fire, so
        // the user's external edit (qwen3.5:2b) would be silently
        // clobbered.
        current.save_to(&path).unwrap();
        let on_disk: Settings = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            on_disk.ollama_model, "qwen3.5:2b",
            "external edit must survive a stale in-memory save (layer 2 preserved by apply_incoming)"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Negative control for `apply_incoming`: when there is *no*
    /// external edit on disk, applying an inbound DTO and saving
    /// must still propagate the UI change. Pins that the snapshot
    /// preservation in `apply_incoming` doesn't accidentally
    /// suppress legitimate self-edits.
    #[test]
    fn apply_incoming_then_save_writes_ui_change() {
        let dir =
            std::env::temp_dir().join(format!("s-voice-settings-test6-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        let mut current = Settings::default();
        let initial_json = serde_json::to_string_pretty(&current).unwrap();
        std::fs::write(&path, &initial_json).unwrap();
        current.last_known_disk = Some(initial_json);

        // Inbound DTO from JS: enable polish.
        let dto_json = serde_json::to_string(&current).unwrap();
        let mut inbound: Settings = serde_json::from_str(&dto_json).unwrap();
        inbound.polish_enabled = true;

        current.apply_incoming(inbound);
        assert!(current.polish_enabled);

        // No external edit; layer 2 sees last_known_disk == disk_now
        // and lets the save through.
        current.save_to(&path).unwrap();
        let on_disk: Settings = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(
            on_disk.polish_enabled,
            "save must persist the UI change when no external edit happened"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
