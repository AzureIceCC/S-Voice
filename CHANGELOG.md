# Changelog

All notable changes to S-Voice are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **UI i18n (zh / en)** — added a `ui_language` setting (`zh` | `en`) to
  `Settings`, with `sanitize` validation. The settings window and floating
  panel both render labels in the chosen language. The settings form has a
  "显示语言" / "Display language" dropdown at the top.

### Changed
- **`#ui-language` auto-saves on change** — switching the dropdown in the
  settings window immediately calls `cmd_update_settings` (no need to press
  "保存"). The other fields still require an explicit save because they
  trigger hotkey re-registration, model switches, etc. The change broadcasts
  a `settings-changed` event so the floating panel re-renders its state
  labels (等待中 → Waiting) in real time. Settings window itself
  pre-applies the language via `applyUiLanguage` for instant feedback.
- **Polish prompt now explicitly preserves the original language** —
  `DEFAULT_PROMPT` (in `polish.rs`) gained rule 9: "保持用户原本说的语言——
  中文保持中文，英文保持英文。如果原话是中英混合，保留混合状态。
  绝对不要翻译成另一种语言". The previous version's rule 7 ("输出内容严格
  限定在用户说过的话之内") was sometimes overridden by `qwen3.5:2b-q4_K_M`,
  which would silently translate English transcripts to Chinese on
  best-effort interpretation. Verified: a 88-character English transcript
  ("Is u still a probrem with English wors imput...") now polishes to a
  clean English version rather than a Chinese paraphrase.
- Reworked deployment documentation around Apple SpeechAnalyzer as the default
  STT backend, with separate source-build and optional Local Whisper setup,
  compatibility, permission, signing, troubleshooting, and maintainer-check
  guidance.
- Changed the project license from MIT to Apache License 2.0 and synchronized
  the README and Cargo package metadata.

## [0.2.0] — 2026-09-12

### Added
- Apple SpeechAnalyzer transcription on macOS 26, selected explicitly and used
  by default, while retaining local MLX Whisper as an offline backend. Backend
  failures are surfaced directly and never trigger an automatic fallback.
- Speech-recognition permission metadata, a bundled Swift helper, capability
  probes, and a reserved Apple Intelligence availability interface.

### Changed
- Default local polish model is `qwen3.5:2b-q4_K_M`.
- Completed #20 hot-path work: allocation-free callback conversion/downmixing,
  rolling RMS accumulation, pooled HTTP clients, event-driven settings status,
  and non-blocking STT process management. Bundled-app trials measured Apple
  STT at 224-378ms and total post-recording processing at 1.14-1.57s across
  consecutive 2.9-8.9s recordings.

### Fixed
- Transactional settings and hotkey updates, safe external-edit handling,
  bounded STT requests, coherent CoreAudio configuration, acknowledged capture
  startup, responsive MLX lifecycle operations, and reliable paste failure
  reporting (#13-#19).

### Detailed fixes
- **#13: transactional settings updates and consistent default hotkey** — UI
  updates now validate/register a changed hotkey, persist a candidate settings
  object, and only then replace shared runtime state. Registration errors,
  persistence failures, and external-edit conflicts restore the previous OS
  hotkey without overwriting the externally changed file; rollback failures
  are surfaced explicitly. `save_to` now distinguishes written, unchanged, and
  conflict outcomes. Six transaction tests cover all success/failure paths.
  The settings form's empty and clear fallbacks now use `Cmd+[` to match Rust.
- **#20 phase 1: lower hot-path overhead** — audio callbacks now convert and
  downmix F32/I16/U16 input directly into the mono capture buffer, avoiding a
  temporary allocation for every callback. A fixed 480-sample ring with a
  running sum of squares replaces repeated `Vec::remove(0)` and full-window
  rescans; a release microbenchmark measured its window maintenance at
  14.9-17.2x faster with identical RMS output. Ollama requests now share a
  connection-pooled client, the settings state badge uses pipeline events
  instead of polling twice per second, and STT service scripts are awaited on
  blocking workers rather than async/UI callback threads. Real-device aggregate
  validation and closure are recorded in v0.2.0 above.
- **#18: non-blocking, serialized STT model lifecycle** — synchronous MLX load,
  inference, warmup, and unload operations now run through worker threads and a
  single lifecycle lock. FastAPI's event loop can continue serving health and
  configuration requests during inference, while idle and forced unload cannot
  release the model underneath an active transcription. A lightweight state
  snapshot reports loading/transcribing/unloaded status without waiting for the
  model lock. Fake-backend concurrency tests cover overlapping health,
  prewarm, transcription, forced unload, and idle unload operations.
- **#16/#17: reliable paste-path selection and failure reporting** — the macOS
  output path now checks `AXIsProcessTrusted()` without prompting before using
  CGEventPost, and immediately selects the osascript fallback when S-Voice is
  not trusted. The fallback now captures the child process output and treats a
  non-zero exit as a paste failure, preserving up to 512 stderr characters for
  diagnosis. Unit tests cover success, non-zero exit, truncation, and spawn
  failure; bundled-app permission-state testing remains a manual follow-up.
- **#15/#19: coherent audio configuration and acknowledged startup** — audio
  capture now keeps the selected sample format, channels, and sample rate as
  one configuration instead of combining a supported range with the device's
  potentially different default format. The preferred 16 kHz rate is clamped
  to the selected device range. `Cmd::Start` also returns the worker's actual
  stream-build/play result through a bounded channel, so the UI enters
  `Recording` only after the microphone stream is live. New tests cover config
  selection, rate bounds, worker failure, timeout, and disconnection.
- **#14: bounded STT requests** — the Rust STT client now reuses a single
  connection-pooled `reqwest::Client`, limits connection setup to 2 seconds,
  and caps a full transcription at 45 seconds. A wedged MLX request now exits
  through the pipeline's existing error state instead of remaining in
  `Processing` forever. Health checks use a 3-second cap; predictive prewarm
  keeps its 20-second cap. A stalled local TCP test covers timeout handling.
- **#11: race between external `settings.json` edits and the
  `RunEvent::Exit` save-back** — previously, when a user (or an
  automation script) edited `settings.json` while s-voice was alive
  (e.g. swapping `ollama_model` from `9b-mlx` to `2b`) and then quit
  via Cmd+Q, the exit-time `Settings::save()` clobbered the external
  edit by writing the stale in-memory snapshot back to disk. Two-layer
  fix in `Settings::save_to`:
  1. **Byte-level diff-skip** — if `to_string_pretty(self)` matches
     the on-disk file, skip the write. `to_string_pretty` is
     deterministic so byte-level comparison is stable; `trim_end`
     tolerates a trailing-newline difference.
  2. **External-edit detection** — `Settings.last_known_disk`
     (`#[serde(skip)]`) records what `load()` or a previous
     `save_to()` last saw on disk. If the current disk content
     differs from that snapshot, `save_to` refuses to write and
     logs a warning; the user can reconcile via the new
     "重新加载" button in the settings window, which calls
     `cmd_reload_settings` to refresh the in-memory state from disk.
  3. **`Settings::save` is now `&mut self`** (was `&self`); callers
     that need to mutate before saving (`cmd_update_settings`,
     `tray::debug_toggle`, the `RunEvent::Exit` backstop) all hold
     a write lock for the duration. 4 new unit tests pin the
     behaviour: layer-1 skip, layer-1 write-on-change, layer-2
     external-edit preservation, and reload-after-edit.
- **Settings UI "重新加载" button** — invokes `cmd_reload_settings`
  which re-reads `settings.json` and applies the new hotkey / debug
  flag, then refreshes the form fields. Shared `applySettings()`
  helper between the initial load and reload paths keeps them in
  sync.

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

### Predictive model prewarming (2026-08-29)

Added background prewarming for both ASR and LLM models so the
cold-start cost is hidden during the user's speech window.

**STT bridge** (`stt/stt_server.py`):
- New `POST /prewarm` endpoint — idempotent. If the model is unloaded
  it kicks off a background thread to reload it. Returns
  `already_loaded` / `loading` / `disabled`.
- New `MLXAudioBackend.unload()` — drops the in-memory reference,
  `gc.collect()`, and `mlx.core.metal.clear_cache()` so the MPS
  allocator pool actually releases.
- Background idle-unloader thread: every 60s checks if the model
  has been idle for > `STT_IDLE_UNLOAD_SEC` (default 30min), then
  unloads. New `/transcribe` calls reset the idle clock.
- `/health` now reports `model_loaded` and `idle_sec` so the
  client can see exactly what state the bridge is in.

**LLM (Ollama)** (`polish.rs`):
- New `polish::prewarm(model, keep_alive)` — fire-and-forget
  `/api/generate` with empty prompt + `num_predict: 1`. Nudges
  Ollama to (re)load the model without blocking the call site.

**Client** (`pipeline.rs::start_recording`):
- When the user presses the global hotkey, immediately spawn two
  background tasks: `stt_client::prewarm()` and (if polish is
  enabled) `polish::prewarm()`. Both have a 500ms hard timeout
  and swallow all errors, so the press-to-record path is never
  blocked by Ollama or the STT bridge.
- Typical timing: STT load 5-15s, polish load 165s (Ollama MLX cold
  boot). With prewarming, the user is still speaking during the
  load, so by the time the recording stops the model is hot.

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

## [0.1.0] — 2026-08-23 — initial release

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
