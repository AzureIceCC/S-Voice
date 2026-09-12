//! Pipeline orchestrator. Owns the audio recorder, drives the state machine,
//! and emits Tauri events to the UI.

use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;
use tokio::time::{interval, Duration, MissedTickBehavior};

use crate::audio::{AudioController, AudioInitKind};
use crate::settings::Settings;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Recording,
    /// STT (audio → text) is in progress.
    Processing,
    /// Ollama polish is in progress (only set when polish_enabled).
    Polishing,
    Error(String),
}

impl State {
    pub fn as_str(&self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Recording => "recording",
            State::Processing => "processing",
            State::Polishing => "polishing",
            State::Error(_) => "error",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub state: State,
    pub last_error: Option<String>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            state: State::Idle,
            last_error: None,
        }
    }
}

pub enum Cmd {
    Toggle,
}

#[derive(Clone)]
pub struct PipelineHandle {
    tx: mpsc::UnboundedSender<Cmd>,
    snapshot: Arc<parking_lot::Mutex<Snapshot>>,
}

impl PipelineHandle {
    pub fn toggle(&self) {
        let _ = self.tx.send(Cmd::Toggle);
    }
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.lock().clone()
    }
}

pub struct PipelineActor {
    app: AppHandle,
    settings: Arc<parking_lot::RwLock<Settings>>,
    snapshot: Arc<parking_lot::Mutex<Snapshot>>,
    recorder: Result<AudioController, String>,
    rx: mpsc::UnboundedReceiver<Cmd>,
    /// Set true if the recorder failed to initialize with a terminal
    /// reason (no input device, OS refused to spawn the worker). When
    /// true, `start_recording` short-circuits to an error instead of
    /// attempting a rebuild that we know will fail.
    recorder_permanently_failed: bool,
    /// Wall-clock time when the current Recording phase began. Set in
    /// `start_recording`, consumed in `stop_and_process` to log the
    /// recording duration. Single-threaded (driven by the Cmd channel),
    /// so a plain Option is safe — no concurrent access.
    recording_started_at: Option<std::time::Instant>,
}

impl PipelineActor {
    /// Spawn the actor on the tokio runtime and return a handle for the UI.
    pub fn spawn(app: AppHandle, settings: Arc<parking_lot::RwLock<Settings>>) -> PipelineHandle {
        let (tx, rx) = mpsc::unbounded_channel();
        let snapshot = Arc::new(parking_lot::Mutex::new(Snapshot::default()));

        let init_result = AudioController::new();
        let recorder_permanently_failed = matches!(init_result, Err((_, AudioInitKind::Terminal)));
        let recorder = init_result.map_err(|(e, _)| format!("{e:#}"));
        if let Err(e) = &recorder {
            tracing::error!("recorder init failed: {e}");
        }

        let actor = Self {
            app,
            settings,
            snapshot: Arc::clone(&snapshot),
            recorder,
            rx,
            recorder_permanently_failed,
            recording_started_at: None,
        };
        let handle = PipelineHandle { tx, snapshot };
        tauri::async_runtime::spawn(actor.run());
        handle
    }

    async fn run(mut self) {
        tracing::info!("pipeline actor running");
        // 100ms tick: forwards the current mic RMS to the UI via
        // `audio-level-changed` while we're in the Recording state.
        let mut tick = interval(Duration::from_millis(100));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                cmd = self.rx.recv() => {
                    match cmd {
                        Some(Cmd::Toggle) => self.handle_toggle().await,
                        None => break,
                    }
                }
                _ = tick.tick() => {
                    self.emit_level_if_recording();
                }
            }
        }
    }

    fn current_level(&self) -> f32 {
        self.recorder.as_ref().map(|r| r.level()).unwrap_or(0.0)
    }

    fn emit_level_if_recording(&self) {
        if matches!(self.snapshot.lock().state, State::Recording) {
            // The audio level is consumed only by the floating panel
            // via the `audio-level-changed` event. No need to mirror it
            // into a shared atomic — that path was dead after the
            // `cmd_get_audio_level` command was removed.
            let _ = self.app.emit("audio-level-changed", self.current_level());
        }
    }

    async fn handle_toggle(&mut self) {
        let current = self.snapshot.lock().state.clone();
        match current {
            State::Idle => {
                if let Err(e) = self.start_recording() {
                    self.set_error(e);
                }
            }
            State::Recording => {
                self.stop_and_process().await;
            }
            State::Processing => {
                tracing::debug!("toggle ignored while processing");
            }
            State::Polishing => {
                tracing::debug!("toggle ignored while polishing");
            }
            State::Error(_) => {
                *self.snapshot.lock() = Snapshot::default();
                if let Err(e) = self.start_recording() {
                    self.set_error(e);
                }
            }
        }
    }

    fn start_recording(&mut self) -> Result<(), String> {
        // If the recorder has been marked permanently failed (e.g. no input
        // device on startup), don't bother rebuilding — surface a clear
        // error so the user knows to fix the device and restart.
        if self.recorder_permanently_failed {
            return Err(
                "audio device unavailable; restart the app after reconnecting a microphone"
                    .to_string(),
            );
        }
        // If the previous controller failed to build, try once before giving up.
        if self.recorder.is_err() {
            self.recreate_recorder();
        }
        let recorder = self.recorder.as_mut().map_err(|e| e.clone())?;
        if let Err(e) = recorder.start() {
            // A previous Stop may have wedged the audio worker (recv_timeout
            // returned WorkerGone), so `cmd_tx.send(Start)` is permanently
            // broken on this controller. Recreate and retry once.
            tracing::warn!("mic start failed ({e}); recreating audio controller and retrying");
            self.recreate_recorder();
            let recorder = self.recorder.as_mut().map_err(|e| e.clone())?;
            recorder
                .start()
                .map_err(|e| format!("mic start failed: {e}"))?;
        }
        self.set_state(State::Recording);
        self.recording_started_at = Some(std::time::Instant::now());
        // The floating panel is persistent (visible: true in tauri.conf);
        // we just flip its state in JS. No need to .show() here.
        tracing::info!("recording started");
        // Predictive prewarm: kick the STT bridge to (re)load its model
        // in the background. While the user is speaking (5-10s) the
        // model loads, so the subsequent /transcribe hits a warm model.
        // Fire-and-forget — a failure here just means we eat the cold-
        // start cost the next time around, same as before.
        if self.settings.read().stt_backend == "local_whisper" {
            tokio::spawn(async move {
                crate::stt_client::prewarm().await;
            });
        }
        // Same idea for the polish model: nudge Ollama to (re)load it
        // now so the model is hot by the time we run polish 5-10s
        // later. Skipped if polish is disabled in settings — no point
        // loading a model we won't use.
        let polish_enabled = self.settings.read().polish_enabled;
        if polish_enabled {
            let (model, keep_alive) = {
                let s = self.settings.read();
                (s.ollama_model.clone(), s.ollama_keep_alive.clone())
            };
            tokio::spawn(async move {
                crate::polish::prewarm(&model, &keep_alive).await;
            });
        }
        Ok(())
    }

    /// Drop the old `AudioController` (which sends `Cmd::Shutdown` to its
    /// worker thread and joins it) and spin up a fresh one with a new
    /// device handle. If the rebuild fails with a `Terminal` reason
    /// (no input device, worker thread refused by the OS), set
    /// `recorder_permanently_failed` so subsequent start attempts do
    /// not loop rebuilding.
    fn recreate_recorder(&mut self) {
        match AudioController::new() {
            Ok(r) => {
                tracing::info!("audio controller recreated");
                self.recorder = Ok(r);
            }
            Err((e, kind)) => {
                tracing::error!("recorder recreate failed: {e:#}");
                self.recorder = Err(format!("{e:#}"));
                if kind == AudioInitKind::Terminal {
                    self.recorder_permanently_failed = true;
                    tracing::error!(
                        "recorder permanently failed; further start attempts \
                         will short-circuit until the app is restarted"
                    );
                }
            }
        }
    }

    async fn stop_and_process(&mut self) {
        self.set_state(State::Processing);
        // Floating panel is persistent now (visible: true in tauri.conf);
        // we just transition its state to "processing" instead of hiding it.

        // Recording phase done — capture the duration for the per-stage
        // timing log below. Clear it eagerly so an early-return path
        // (e.g. empty WAV) doesn't leave stale data for the next turn.
        let recording_duration = self
            .recording_started_at
            .take()
            .map(|t| t.elapsed())
            .unwrap_or_default();

        // Pull the recorder out of self before calling its blocking stop
        // method, so the borrow doesn't span the subsequent set_error calls.
        let wav_result: Result<Vec<u8>, String> = match self.recorder.as_mut() {
            Ok(r) => r.stop().map_err(|e| format!("mic stop failed: {e}")),
            Err(e) => Err(e.clone()),
        };
        let wav = match wav_result {
            Ok(b) if !b.is_empty() => b,
            Ok(_) => {
                tracing::warn!(
                    "empty recording, nothing to transcribe (recording took {:?})",
                    recording_duration
                );
                self.set_state(State::Idle);
                return;
            }
            Err(e) => {
                self.set_error(e);
                return;
            }
        };
        tracing::info!(
            "captured {} bytes of WAV (recording took {:?})",
            wav.len(),
            recording_duration
        );
        // Start the processing clock now — STT, polish, and paste are all
        // the "processing" phase the user sees in the floating panel.
        let process_start = std::time::Instant::now();

        // Read settings snapshot.
        let (stt_backend, lang, ollama_model, keep_alive, polish_enabled, custom_prompt) = {
            let s = self.settings.read();
            (
                s.stt_backend.clone(),
                s.language.clone(),
                s.ollama_model.clone(),
                s.ollama_keep_alive.clone(),
                s.polish_enabled,
                s.polish_prompt.clone(),
            )
        };

        // STT — one-shot path. mlx-audio's AlignAtt-based streaming emits
        // low-quality partials ("嗯") and frequently drops content in the
        // final re-alignment, so we stick to the deterministic single-shot
        // `transcribe()` call. Latency is ~4-5s for the large-v3 Chinese
        // model; v0.2 will revisit streaming via faster-whisper.
        let raw_text = if stt_backend == "apple_speech" {
            match crate::apple_speech::transcribe(wav, &lang).await {
                Ok(text) => text,
                Err(e) => {
                    // Backend choice is explicit. Do not hide Apple/network/
                    // permission failures by silently sending audio to MLX.
                    self.set_error(format!("Apple Speech failed: {e}"));
                    return;
                }
            }
        } else {
            match crate::stt_client::transcribe(wav, &lang).await {
                Ok(r) => r.text,
                Err(e) => {
                    self.set_error(format!("local Whisper failed: {e}"));
                    return;
                }
            }
        };
        let stt_duration = process_start.elapsed();
        tracing::info!(
            "STT backend={} took {:?} ({} chars) -> {raw_text:?}",
            stt_backend,
            stt_duration,
            raw_text.chars().count()
        );
        // Skip polish + paste if the transcript is effectively empty:
        //   - "" (no speech at all)
        //   - only whitespace / punctuation
        //   - only filler words (嗯/啊/呃/um/uh) that STT hallucinated from noise
        // Without this guard the LLM tends to "respond" to the empty
        // transcript with helpful-sounding prose ("我没有听清..."), which
        // gets pasted into the user's text field — surprising and bad.
        if is_transcript_noise(&raw_text) {
            tracing::info!(
                "STT output is noise/empty ({} chars), skipping polish + paste",
                raw_text.chars().count()
            );
            self.set_state(State::Idle);
            return;
        }

        // Polish (optional). Transition to a dedicated Polishing state so
        // the floating panel (and any other observers) can distinguish
        // "STT done, polish running" from "STT running".
        let polish_duration = std::time::Instant::now();
        let final_text = if polish_enabled {
            self.set_state(State::Polishing);
            let res = if custom_prompt.trim().is_empty() {
                crate::polish::polish(&raw_text, &ollama_model, &keep_alive).await
            } else {
                crate::polish::polish_with_prompt(
                    &raw_text,
                    &ollama_model,
                    &keep_alive,
                    &custom_prompt,
                )
                .await
            };
            match res {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!("polish failed, falling back to raw: {e:#}");
                    raw_text
                }
            }
        } else {
            raw_text
        };
        let polish_duration = polish_duration.elapsed();

        tracing::info!(
            "polish took {:?} (final {} chars)",
            polish_duration,
            final_text.chars().count()
        );
        tracing::info!("final -> {final_text:?}");

        // Output
        if let Err(e) = crate::output::paste(&self.app, &final_text) {
            self.set_error(format!("paste failed: {e}"));
            return;
        }
        let paste_duration = process_start.elapsed() - stt_duration - polish_duration;
        let total_processing = process_start.elapsed();
        tracing::info!(
            "turn timing: recording={:?} stt={:?} polish={:?} paste={:?} total_processing={:?}",
            recording_duration,
            stt_duration,
            polish_duration,
            paste_duration,
            total_processing
        );

        // Emit a copy of the final text to the UI for preview/notification.
        let _ = self.app.emit("text-ready", &final_text);
        self.set_state(State::Idle);
    }

    fn set_state(&self, new_state: State) {
        let mut s = self.snapshot.lock();
        s.state = new_state.clone();
        if !matches!(new_state, State::Error(_)) {
            s.last_error = None;
        }
        let state_str = s.state.as_str().to_string();
        drop(s);
        let _ = self.app.emit("state-changed", state_str);
    }

    fn set_error(&self, msg: String) {
        tracing::error!("{msg}");
        let mut s = self.snapshot.lock();
        s.state = State::Error(msg.clone());
        s.last_error = Some(msg.clone());
        drop(s);
        let _ = self.app.emit("state-changed", "error");
        let _ = self.app.emit("error", msg);
    }
}

/// Returns true if the transcript has no usable content — i.e. it's
/// empty, only whitespace/punctuation, or only STT hallucinated
/// filler words from background noise. Used to short-circuit polish
/// and paste so we don't hand the LLM a meaningless string and get
/// back helpful-sounding prose.
fn is_transcript_noise(s: &str) -> bool {
    // Strip whitespace and CJK + ASCII punctuation, then check what's left.
    let stripped: String = s
        .chars()
        .filter(|c| !c.is_whitespace() && !is_punctuation(*c))
        .collect();
    if stripped.is_empty() {
        return true;
    }
    // Every remaining char must be a known STT filler (嗯/啊/呃/哦 etc.
    // for CJK, u/h/m for "um"/"uh"/"hm" in ASCII). One by one to avoid
    // char-literal multi-byte issues with the Rust lexer.
    stripped.chars().all(is_filler_char)
}

fn is_filler_char(c: char) -> bool {
    c == '嗯'
        || c == '啊'
        || c == '呃'
        || c == '哦'
        || c == '欸'
        || c == '呣'
        || c == '唔'
        || c == 'u'
        || c == 'U'
        || c == 'h'
        || c == 'H'
        || c == 'm'
        || c == 'M'
}

fn is_punctuation(c: char) -> bool {
    matches!(
        c,
        // ASCII punctuation
        '.' | ',' | '!' | '?' | ';' | ':' | '"' | '\'' | '(' | ')' | '[' | ']'
            | '{' | '}' | '-' | '/' | '\\' | '|' | '@' | '#' | '$' | '%' | '^'
            | '&' | '*' | '_' | '+' | '=' | '<' | '>' | '~' | '`' | '…' | '—' | '–'
            | '·'
        // CJK punctuation
        | '。' | '，' | '！' | '？' | '；' | '：' | '“' | '”' | '‘' | '’'
            | '（' | '）' | '【' | '】' | '《' | '》' | '、' | '～' | '「' | '」'
            | '『' | '』' | '〈' | '〉' | '　'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Nothing said" cases — must skip polish/paste so the LLM doesn't
    /// see empty input and respond with helpful prose.
    #[test]
    fn empty_and_whitespace_is_noise() {
        assert!(is_transcript_noise(""));
        assert!(is_transcript_noise("   "));
        assert!(is_transcript_noise("\n\t  "));
    }

    /// Punctuation-only — STT on a beep or click sometimes returns "..."
    /// or "，。" etc. Must not be sent to the LLM.
    #[test]
    fn punctuation_only_is_noise() {
        assert!(is_transcript_noise("..."));
        assert!(is_transcript_noise("。。"));
        assert!(is_transcript_noise("，"));
        assert!(is_transcript_noise("、,;!?"));
    }

    /// Filler-only — STT on background noise may output "嗯。" or
    /// "啊…" or "嗯嗯啊". The user wants these short-circuited too.
    #[test]
    fn filler_only_is_noise() {
        assert!(is_transcript_noise("嗯"));
        assert!(is_transcript_noise("嗯。"));
        assert!(is_transcript_noise("啊…"));
        assert!(is_transcript_noise("嗯嗯啊呃"));
        assert!(is_transcript_noise("um uh"));
        assert!(is_transcript_noise("嗯嗯")); // consecutive filler
    }

    /// Real content must NOT be flagged as noise. Including very short
    /// but meaningful utterances like "好" or "ok".
    /// `嗯。嗯。` is intentionally noise — even though it has
    /// punctuation between fillers, the user produced no real content;
    /// the LLM would just respond with helpful prose.
    #[test]
    fn real_content_is_not_noise() {
        assert!(!is_transcript_noise("好"));
        assert!(!is_transcript_noise("ok"));
        assert!(!is_transcript_noise("嗯……那个"));
        assert!(!is_transcript_noise("嗯明天开会")); // "嗯" + real content
        assert!(!is_transcript_noise("。你好。"));
        // "嗯。嗯。" → is_transcript_noise == true (only filler chars remain)
    }

    #[test]
    fn filler_with_punctuation_between_is_still_noise() {
        assert!(is_transcript_noise("嗯。嗯。"));
        assert!(is_transcript_noise("啊…嗯"));
        assert!(is_transcript_noise("um uh hm"));
    }

    /// `is_punctuation` must classify every common CJK + ASCII mark
    /// so the noise check above doesn't accidentally keep a stray "。"
    /// as "real content".
    #[test]
    fn punctuation_helper_catches_all() {
        for c in [
            '.', ',', '!', '?', '。', '，', '！', '？', '…', '—', '"', '"', '\'', '\'', '（', '）',
        ] {
            assert!(is_punctuation(c), "expected {c:?} to be punctuation");
        }
    }
}
