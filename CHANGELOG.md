# Changelog

All notable changes to S-Voice are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] — 2026-08-23 — initial release

### Fixed

- **Audio duration halved on stereo input devices** — `stop_capture` was
  re-downmixing an already-mono `samples: Vec<f32>`, halving the recorded
  duration whenever the input device reported `channels > 1`. Bug was
  invisible on built-in mono mics (channels=1 path was a no-op identity
  copy) and was masked in tests because STT still produced plausible
  text on the shortened audio. `WorkerState.channels` is now kept only
  for diagnostics; the downmix step is removed in favour of a single
  `f32_to_pcm16` helper that converts straight to 16-bit PCM.
- `output::paste` reported a `paste_duration` of ~0ms because the
  function returned as soon as `osascript` was `spawn()`-ed. The paste
  now uses `osascript.status()` to wait for the synthetic `Cmd+V` to
  actually deliver, and the floating panel's "处理中" timer no longer
  ends before the text is on the clipboard. Per-stage timing
  (`paste timing: clipboard=… cgevent=…`) and a `turn timing` summary
  are now logged on every turn.

### Changed
- **CGEventPost path for paste** — when Accessibility is granted, `output::paste`
  now uses `CGEventPost` (Quartz Event Services) instead of `osascript`
  Apple Events, dropping the paste leg from ~100-500ms to ~10-30ms.
  Falls back to the osascript path automatically if Accessibility is
  denied; logs `path=CGEventPost` or `path=osascript` so the active
  path is visible per turn.
- **Pipeline state split into Processing + Polishing** — a new
  `State::Polishing` transition lets the floating bar (and the
  settings badge) show "润色中…" in blue while Ollama is rewriting
  the transcript, distinct from the yellow "处理中…" STT phase.
  Only set when `polish_enabled` is on; falls through to Idle on error.
- **Floating bar timers split** — `recordingStart` and `processingStart`
  are tracked independently, so the panel shows the recording
  duration during Recording and the post-STT duration during
  Processing/Polishing (matching the `turn timing` log).
- **Polish prompt (default) simplified** — replaced the example-heavy
  "删除口頭禪" wording with a 7-line generic Chinese rule set
  covering 语气词 / 重複 / 标点 / **错别字** / 语法 / 不要添加 / 不要总结.
  Suitable for any prose style instead of being tuned to a specific
  domain.
- **Polish `num_predict` 1024 → 512, `num_ctx` 2048** — polish output
  rarely exceeds 200 tokens; the headroom was wasted memory. The
  `num_ctx` cap prunes Ollama's default 256k context (which would
  otherwise allocate a 256k-token KV cache on Mac unified memory and
  blow past the 30s timeout on the first call after an idle period).
- **STT model not switched to 8-bit** — evaluated
  `mlx-community/belle-whisper-large-v3-turbo-zh-8bit` against the
  current `fp16` model on 5.5s synthetic audio. 8bit averaged 5.74s vs
  fp16's 5.62s, and cold start was 165s (vs ~15s for fp16). The Mac
  MLX backend's 8-bit kernel is not mature enough to deliver the
  2x speedup that RTX-class hardware sees. STT settings remain
  configurable so users can experiment.

### Added
- **Per-stage timing log** for every turn:
  `captured N bytes of WAV (recording took Xs)`,
  `STT took Xs (N chars)`,
  `polish took Xs (final N chars)`,
  `turn timing: recording=Xs stt=Xs polish=Xs paste=Xs total_processing=Xs`,
  `paste timing: clipboard=Xs cgevent=Xs total=Xs path=CGEventPost`.
- **Per-stage floating bar colors** —
  recording (green / breathe), processing (yellow / fade),
  polishing (blue / fade), error (red / static).
- **`bridge-status` UI event** — surfaces the STT bridge's startup
  state (starting / ready / failed) so the floating bar can show
  "STT 启动中…" / "STT 启动失败" instead of looking like the app
  itself is broken.

### Removed (dead code / unused paths)
- Unused imports: `tracing_subscriber::layer::Layer`,
  `serde::Serialize` (polish), `tauri::Manager` (pipeline),
  `tracing_subscriber::filter::LevelFilter` (logging),
  `tauri::menu::Submenu` (tray).
- Unused fields: `AudioController.{device_name,sample_rate,channels}`,
  `HealthResponse.port`, `HotkeyError::Plugin`.
- Unused functions: `show_floating`, `hide_floating`,
  `_force_submenu_import`, `_force_use`, `logging::log_path`.
- Unused Tauri commands: `cmd_get_audio_level`, `cmd_toggle_recording`,
  `cmd_start_stt_service`, `cmd_stop_stt_service` (no callers in Rust
  or UI; the floating bar uses the `audio-level-changed` event
  instead of polling).
- `PipelineHandle::level()` and the `Arc<AtomicU32>` mirror — the
  UI consumes level through events; the atomic publish was dead.
- Manual `#[allow(dead_code)]` markers: now zero (was four).
- **Code line count: 2149 → 2071** (-78 lines, all redundant).

### Refactored
- `audio.rs::on_f32` and `stop_capture` shared a "downmix if
  multi-channel" pattern; consolidated by removing the redundant
  downmix and introducing `f32_to_pcm16(&[f32]) -> Vec<i16>`.
- `floating.js` rewrite: `recordingStart` / `processingStart`
  clocks are independent; idle briefly retains the last processing
  duration before resetting, so the user can see the previous turn's
  cost.
- `Settings::sanitize` now returns `bool` (whether it mutated
  anything) and `Settings::load` auto-saves when sanitize changed
  something — bad hotkeys get fixed on disk in the same run, not
  silently re-fixed on every boot.
- `RunEvent::Exit` handler added: an exit-time `Settings::save()`
  backstop catches anything mutated in memory but never flushed
  (e.g. via the tray menu's debug toggle).
- `output::paste` and `Settings::sanitize` no longer `spawn()` or
  fall through; both paths now `wait()` for the actual work to
  complete, eliminating the "panel flipped to 等待中 before text
  arrived" race.

### Build
- `cargo build` (dev): **0 warnings, 0 errors**
- `cargo build --release`: **0 warnings, 0 errors**
- `Cargo.toml`: added `core-graphics = "0.25"` for the CGEventPost
  path; `tauri` features extended with `image-png` so `Image::from_bytes`
  can load the embedded tray icon.
- `tauri.conf.json`: settings window now starts hidden (`visible: false`).
  The user opens it from the menu bar tray (`设置…`); closing the
  settings window hides it (does not quit).

### Known issues
- **Ollama polish latency** — `qwen3.5:9b-mlx` on the MLX backend
  varies 2-13s per call (median ~6s) due to nvfp4 dequant overhead
  and qwen3.5's residual thinking compute path. Switch to
  `qwen3.5:9b` (GGUF Q4_K_M, 6.6GB) once the model is pulled for a
  more stable 1-3s polish. Tracked as a config change, not a code
  change.
- **STT bridge latency** — `belle-whisper-large-v3-turbo-zh-fp16`
  on a warm model is ~5s for 3s of audio. 8-bit was evaluated and
  rejected (see "Changed" above). Real streaming + parallel polish
  is on the v0.2 backlog.
- **Recording requires Accessibility permission for CGEventPost** —
  if not granted, paste falls back to osascript silently (logged as
  a warning). One-time grant in
  System Settings → Privacy & Security → Accessibility.
- **Tracing mirrored to macOS unified log** (subsystem
  `com.s-voice.app`, category `default`). Every `tracing::*!` call now
  appears in Console.app and `log stream` alongside the file log.
  Useful for remote SSH diagnosis and for users who don't know where
  `~/Library/Logs/S-Voice/s-voice.log` lives. Implementation: a
  `tracing-oslog` layer added next to the existing stdout + file
  layers; `cfg!(target_os = "macos")` keeps the dependency out of
  non-macOS targets.

  ```bash
  log stream --predicate 'subsystem == "com.s-voice.app"' --info
  log show    --predicate 'subsystem == "com.s-voice.app"' --last 1h
  ```

### Polish prompt tuned (2026-08-23)

The original `DEFAULT_PROMPT` (v0.1 cleanup) was tuned in two
increments after a brief usage session revealed it was making
output too formal / written for a speech-input use case:

- **v1** (initial v0.1): listed 7 generic rules — delete all
  语气词, dedupe repeats, add punctuation, fix typos, fix obvious
  grammar, no additions, no summary. A user noted this produced
  output that read like a polished essay, not what they had just
  said.
- **v2** (today, first edit): added a final paragraph instructing
  the model to retain sentence-final particles (吧/呢/啊) and
  avoid over-formal output. Helped slightly but still too formal.
- **v3** (today, second edit): restructured the rules so
  "delete 语气词" became "only delete pure fillers (嗯/啊/呃/um/uh)"
  with an explicit "保留所有口语特征" rule. Added "保留原本用户的语法"
  (positive phrasing) and reworded the no-summary rule to "只输出
  整理后的纯文本,不尝试总结、解释、回应" (lead with the desired
  output, then negate the alternatives). Dropped the closing
  "这不是书面作文..." line as redundant. The user then confirmed
  v3 reads natural.

The model is now `qwen3.5:2b` (GGUF, 2.7 GB) on the user side,
chosen to avoid the MLX-vs-MLX Metal allocator pool contention that
made `qwen3.5:9b-mlx` slow STT 6x (11.4s vs 1.7s median).

## [0.1.0] — initial feature set

- Tauri 2 desktop app for local AI voice input.
- Pipeline: mic (cpal) → WAV bytes → STT bridge (HTTP) → text →
  optional Ollama polish → clipboard + simulate Cmd+V.
- Settings window, system tray menu, persistent floating panel.
- Global hotkey (Cmd+[ default), Esc fallback.
- Whisper large-v3-turbo-zh via FastAPI bridge; Ollama polish via
  qwen3.5:9b-mlx.

> The polish/cleanup section above (`### Fixed / Changed / Added /
> Removed / Refactored / Build / Known issues`) was applied on top
> of this initial feature set on the same release date.
