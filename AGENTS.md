# S-Voice — Project Notes (for AI coding agents)

> **Audience**: this file is auto-loaded by AI coding agents (Mavis /
> Cursor / Aider / Devin / etc.) as project context. **Human-facing
> docs are `README.md`** (user guide) and `CHANGELOG.md` (version
> history). Keep the split: agents want constraints and decisions,
> users want features and usage.

Local AI voice input for macOS. Press a global hotkey, speak, get text
inserted at the cursor. Tauri 2 + MLX Whisper + Ollama polish.

## Current state

- **Stage**: **v0.2.0 released 2026-09-12**. Apple SpeechAnalyzer is the
  default STT backend, local MLX Whisper remains explicitly selectable, and
  automatic fallback between them is intentionally disabled. This release also
  includes the #11 and #13-#20 reliability/performance work.
- **Run command**: `cargo run` (bare binary, see #7 below)
- **Hotkey**: `Cmd+[` (default; was `Cmd+Shift+Space` but macOS grabs it for
  input-source switching)
- **Polish**: disabled by default (cold start adds 30s timeout + per-turn
  latency). Re-enable in settings once raw STT speed is acceptable.
- **STT bridge**: separate Python process (`./stt/start_stt.sh`); runs in
  background, sometimes exits after long idle (see backlog #5).
- **Settings**: persist immediately on every change; `Settings::sanitize`
  auto-saves bad-hotkey fixes; `RunEvent::Exit` adds an exit-time save
  backstop.
- **Paste**: uses `CGEventPost` (10-30ms) when Accessibility is granted;
  falls back to osascript (100-500ms) otherwise. See
  `output::paste_via_cgevent` and the `path=CGEventPost` / `path=osascript`
  log line.
- **Logs**: every `tracing::*!` event is mirrored to the macOS unified
  log (subsystem `com.s-voice.app`, category `default`). Use Console.app
  or `log stream --predicate 'subsystem == "com.s-voice.app"' --info` —
  no setup needed in release builds. File log at
  `~/Library/Logs/S-Voice/s-voice.log` is still the canonical
  long-term record.

## Known issues & future plans

### Backlog (post-v0.1 polish round)

- [x] **#2 Floating panel persistent** — done 2026-08-23. Panel stays
      visible above the dock, semi-transparent, shows per-stage
      state (waiting / recording / processing / polishing / error)
      with separate recording vs. processing timers. See `floating.js`
      and the `Polishing` state added to `pipeline.rs`.
- [ ] **#5 STT bridge watchdog** — long-idle bridge self-exits; need auto-
      restart or launchd supervision so S-Voice never starts with a dead
      bridge.
- [ ] **#6 STT latency variance** — 5x spread (1.8s–10s for similar-length
      audio). Investigate MLX scheduling / concurrency / batch options.
      (8-bit was tried and rejected on Mac MLX — see
      `CHANGELOG.md` v0.1 cleanup entry.)
- [x] **#12 Predictive model prewarming** — done 2026-08-29. When
      the user presses the global hotkey, the Rust pipeline now
      fire-and-forget POSTs `STT /prewarm` and (if polish enabled)
      `Ollama /api/generate?prompt=`. The STT bridge has a
      background idle-unloader (default 30 min) that drops the
      MLX-audio model + `mx.metal.clear_cache()` when idle, so the
      MPS allocator pool is released. The next press reloads in the
      background while the user is still speaking. `/health` now
      reports `model_loaded` and `idle_sec`. Tunable via
      `STT_IDLE_UNLOAD_SEC` env var. ~80 LOC in Python + 50 LOC in
      Rust. Trade-off: at the cost of one extra HTTP round-trip per
      hotkey press (~10ms), the worst-case STT cold-start (5-15s)
      and the worst-case Ollama MLX cold-start (165s) are both
      hidden behind the user's natural speech latency.
- [x] **#7 Microphone permission** — solved by the bundled `.app` install.
      TCC now keys on `com.s-voice.app` (stable bundle id)
      rather than the per-launch random hash, so mic + accessibility
      grants persist across rebuilds and reboots. Verified via
      `log show --predicate 'subsystem == "com.apple.TCC"'` after
      install — `staticCode for identifier com.s-voice.app at
      /Applications/S-Voice.app` recorded; subsequent relaunches
      produce no `Prompting policy` events.
- [ ] **#8 Real-time streaming transcription (deferred to v0.2)** — see below.
- [ ] **#9 First-phoneme loss on fast start** — when the user speaks
      immediately after pressing the hotkey, the first 1–2 syllables
      are missing from the transcript. Root cause: cpal stream
      warmup (50–200ms) elapses between `Cmd+[` and the first audio
      frame arriving at the callback; any speech in that window is
      dropped. Fix: always-on mic + 1 s rolling ring buffer; the
      recording file becomes "the 1 s pre-roll + everything after the
      hotkey". Industry standard (Apple Dictation, Otter, WhisperLive
      all do this). Privacy story: buffer is in-memory only, 1 s
      overwriting, never written to disk unless the user presses
      the hotkey. ~1–2h change confined to `audio.rs` worker.
- [ ] **#10 Cloud polish backend** — local Ollama MLX shares the
      Metal device with mlx_whisper, so opening polish drops STT
      speed from 1.7s to 11.4s (median, 6x slower) because both
      frameworks fight over the same MPS allocator pool. The fix is
      to move polish off-device: keep STT local (small, fast, works
      offline, preserves privacy) and offer a cloud provider for
      polish (OpenAI / Anthropic / 国内代理). Cloud polish returns
      1–3s stable latency, no Mac thermal load, ~$0.10–0.50/month
      for typical voice-input volume. Implementation: add a
      `polish_backend` enum + `api_key` field to `Settings`,
      branch in `pipeline.rs::stop_and_process` based on backend,
      add provider dropdown + key field to settings UI. ~150 LOC.
      Requires user to have a working HTTP proxy for OpenAI/Anthropic
      (most 国内 users do).
- [x] **#11 Smooth model switching + safe save race** — done
      2026-08-29. The `RunEvent::Exit` save-back overwrote external
      edits to `settings.json` made while S-Voice was still running.
      Concretely: when the user (or an automation script) changed
      `ollama_model` from "9b-mlx" to "2b" in `settings.json` while
      S-Voice was alive, then quit S-Voice via Cmd+Q, the exit-time
      `snapshot.save()` wrote the in-memory 9b-mlx state back to
      disk, undoing the external edit. Discovered while switching
      from `qwen3.5:9b-mlx` to `qwen3.5:2b` — same root cause as
      any future "swap model and restart" flow. Two layers of fix:
      1. **Safe save** — `Settings::save_to` in `settings.rs` has
         two layers: (a) byte-level diff-skip: if
         `to_string_pretty(self)` matches the on-disk file, skip the
         write; (b) external-edit detection: `Settings.last_known_disk`
         (`#[serde(skip)]`) records what `load()` or a previous
         successful `save_to()` last saw on disk. If the current
         disk content differs from that snapshot, `save_to` refuses
         to write and logs a warning. `Settings::save` is now
         `&mut self`; all callers (cmd_update_settings, tray
         debug_toggle, RunEvent::Exit) hold a write lock for the
         duration. 4 new unit tests pin the behaviour.
      2. **Hot reload** — `cmd_reload_settings` Tauri command
         re-reads `settings.json` and updates the in-memory
         `Arc<RwLock<Settings>>`. The settings UI gets a
         "重新加载" button (`app.js::reloadSettings`) that calls
         this. Re-runs the hotkey and debug-flag side effects
         (reregister, set_debug) so editing settings.json directly
         — for example to swap models — takes effect without a
         restart. `applySettings()` is now a shared helper between
         the initial load and reload paths.
      Together these make model switching a one-shot operation
      instead of a kill-restart dance.
- [x] **#13 Transactional settings save + hotkey rollback** — done 2026-09-08.
      `Settings::save_to` now reports `Unchanged`, `Written`, or
      `ExternalEditConflict`. `cmd_update_settings` builds a candidate while
      retaining the trusted disk snapshot, validates and registers a changed
      hotkey before persistence, saves the candidate, and only then swaps the
      shared state and logging level. Registration rejection, persistence
      failure, or external-edit conflict restores the old OS hotkey without
      rewriting the old settings file; rollback failure is included in the
      returned error. Six transaction tests cover invalid and OS-rejected
      hotkeys, conflicts, persistence failure, success, and rollback failure.
      Tray debug toggles also restore their in-memory value when persistence is
      refused. The settings UI's empty/clear fallback is synchronized to the
      Rust default `Cmd+[` instead of the obsolete `Cmd+Shift+Space`.
- [x] **#14 Bound STT HTTP latency and recover the pipeline** — done
      2026-09-07. `stt_client` now reuses one configured `reqwest::Client`
      with a 2s connect timeout and a 45s total transcription timeout (enough
      headroom for the measured 5-15s cold load plus inference). Health uses a
      tighter 3s per-request timeout and prewarm retains its 20s cap. Timeout,
      connection, and other HTTP failures are classified separately; a timeout
      reaches the existing pipeline error path instead of leaving `Processing`
      stuck forever. A local stalled-TCP regression test pins the timeout path.
- [x] **#15 Keep the selected CoreAudio format/config coherent** — done
      2026-09-07. Audio initialization now selects and retains one complete
      `SampleFormat` + `StreamConfig`; `start_capture` no longer mixes the
      selected range's channels/rate with `default_input_config`'s format. The
      preferred 16 kHz rate is clamped to the selected range's min/max bounds.
      Synthetic candidate tests cover differing default/supported formats and
      devices whose supported range lies below or above 16 kHz.
- [x] **#16 Detect Accessibility before CGEventPost paste** — implemented
      2026-09-08. The macOS path now queries the non-prompting
      `AXIsProcessTrusted()` API before posting Quartz events. Untrusted
      processes skip CGEventPost and go directly to the osascript fallback;
      trusted installations retain the fast path. Bundled `.app` verification
      covered both the rejected fallback and the authorized CGEventPost path.
- [x] **#17 Treat non-zero osascript exit as paste failure** — implemented
      2026-09-08. The fallback captures `osascript` output, requires a successful
      exit status, and returns a paste error containing stderr bounded to 512
      characters. A command-runner seam and unit tests cover success, non-zero
      exit, bounded diagnostics, and process-spawn failure; the existing
      pipeline error path now receives delivery failures.
- [x] **#18 Move MLX inference off the FastAPI event loop safely** — done
      2026-09-08. Startup warmup, `/transcribe`, `/prewarm`, forced unload, and
      idle unload now run outside the event loop and share one lifecycle lock.
      A lightweight state snapshot lets `/health` remain responsive while MLX
      is loading or transcribing. Idle unload rechecks the activity clock only
      after acquiring the lifecycle lock, so it cannot release the model during
      inference or immediately after a completed request. Standard-library
      concurrency tests with a fake backend cover responsive health checks,
      serialized forced/idle unload, and coalesced prewarm/transcribe loading.
      Real MLX load/inference timing remains pending manual integration testing.
- [x] **#19 Acknowledge microphone Start only after stream playback succeeds** —
      done 2026-09-07. `Cmd::Start` now carries a bounded response channel and
      returns the real build/play result. The Pipeline enters `Recording` only
      after worker success; failure triggers the existing one-time controller
      rebuild/retry with the concrete error logged. Shared response handling
      distinguishes worker errors, a 3s timeout, and disconnection. Tests cover
      success, propagated play failure, timeout, and disconnected worker.
- [x] **#20 Profile and trim avoidable hot-path overhead** — completed
      2026-09-12. The implementation work and release microbenchmark were
      followed by bundled-app testing on real microphone input. Three
      consecutive Apple Speech turns (2.9s, 3.0s, and 8.9s recordings) completed
      without capture, pipeline, or paste errors: SpeechAnalyzer took 224-378ms,
      total post-recording processing took 1.14-1.57s with local 2B Q4 polish,
      and CGEventPost paste took 2-22ms. CPU/RSS sampling was unavailable in the
      sandbox and remains optional observability rather than a release blocker.
      The optimization work was guided by allocation and end-to-end latency
      baselines; deeper CPU/RSS sampling can be added as future observability.
      Phase 1 completed 2026-09-08: a release-mode microbenchmark of 20,000
      480-sample callbacks measured the old sliding RMS window at 167-194ms
      versus 11.2-11.4ms for a fixed ring/sum-of-squares accumulator
      (14.9-17.2x for window maintenance, identical RMS). The callback now
      converts and downmixes F32/I16/U16 directly into the capture buffer with
      no per-callback temporary Vec. Ollama requests reuse one connection-pooled
      client; the settings badge consumes `state-changed` instead of polling
      twice per second; redundant prewarm AppHandle capture was removed; and
      STT start/restart subprocess waits run on blocking workers instead of
      async/UI callback threads. Unit tests pin ring-RMS equivalence and typed
      stereo downmix. Real-device end-to-end testing confirmed the aggregate
      responsiveness improvement without a functional regression.
      High-confidence candidates: eliminate per-callback `Vec` allocation and
      the O(n) `level_window.remove(0)` loop in `audio::on_f32` with direct
      downmix plus a fixed ring/RMS accumulator; reuse HTTP clients so STT and
      Ollama requests share connection pools; remove the redundant AppHandle
      capture in the STT prewarm task; replace the settings window's 500 ms
      `cmd_get_state` polling with the existing `state-changed` event; and move
      `Command::output` calls for STT start/restart off async/UI callback
      threads. Benchmark each change independently and reject complexity that
      does not improve measured latency or responsiveness.

### #7 — Microphone permission requires bundled .app

**Problem.** S-Voice uses cpal (Rust audio crate) which calls CoreAudio
directly. macOS TCC (Transparency, Consent, and Control) persists
permission grants keyed by **bundle identifier**, not binary path. The dev
build (`cargo run` / `cargo tauri dev`) runs the bare binary
`target/debug/s-voice`, which has no `.app` bundle — so macOS treats each
launch as a different app and may re-prompt or fail to persist the grant.

**Status**: deferred until s-voice feature set is stable. Bundle build adds
compile time (no hot reload) and dev workflow friction, so not worth it
during active dev.

**Solution (when ready)**:
1. Add `NSMicrophoneUsageDescription` to Info.plist via
   `tauri.conf.json > bundle.macOS` (or `entitlements`).
2. `cargo tauri build` (5–10 min) → produces
   `src-tauri/target/release/bundle/macos/S-Voice.app`.
3. Install to `/Applications/` and launch via Finder (not `cargo run`).
4. macOS will then persist mic permission under `com.s-voice.app`.

**Quick verification (without full build)**: manually wrap the dev binary
in a tmp `.app` with a minimal `Info.plist` containing
`CFBundleIdentifier=com.s-voice.app` and
`NSMicrophoneUsageDescription`, then `open tmp.app`. Confirms TCC will
behave correctly once bundled.

### #8 — Real-time streaming transcription (deferred to v0.2)

**Goal.** While the user is speaking, surface each recognised word in the
floating panel as soon as it lands, instead of waiting 4-5s for the
full audio to be transcribed in one shot.

**Why deferred.** mlx-audio's `generate_streaming()` uses AlignAtt-based
streaming. Empirically (2026-08-23, 5s Chinese TTS test "你好，世界。今天天气很好。"):
- partial 1 (1s) = `"你好�"` — recognises first word, garbage byte at end
- partial 2 (3s) = `"�"` — one garbage byte, model still aligning
- final (5s) = `""` — **alignatt re-alignment drops all content**

So AlignAtt streaming is unusable: partials are noisy, final discards
content. We reverted to one-shot `transcribe()` (4-5s latency, 100% accuracy).
The UI has a "实时转写（开发中）" radio that is disabled.

**v0.2 plan.** Switch the ASR backend to **faster-whisper**
(`pip install faster-whisper`, CTranslate2 backend, no Python GIL). It
exposes a proper streaming generator that yields progressively-completed
segments, not alignatt token increments. The `STTBackend` Protocol in
`stt_server.py` already abstracts the ASR engine; only `_make_backend()`
needs to return a `FasterWhisperBackend`.

**Reference.** John Maeda's blog post "Real-Time Speech-to-Text on MacOS
with MLX Whisper" (maeda.pm, 2024-11-10) — uses VAD-segmented chunks
(`SILENCE_THRESHOLD` + `SILENCE_CHUNKS`) rather than true streaming.
This is a fallback approach if faster-whisper's streaming is also
unsatisfactory: each detected silence commits the prior segment via
`mlx_whisper.transcribe()` and appends to the running transcript.

## Recent changes (2026-08-22)

v0.1 hardening round — 5 critical/minor fixes + 3 risk root-causes:

- `floating.js`: drop 60ms `setInterval` polling, drive audio level via
  `event.listen('audio-level-changed')`.
- `pipeline.rs`: actor gains 100ms `tokio::interval` tick, writes
  `AtomicU32` (lock-free), emits `audio-level-changed`. `Snapshot` no
  longer carries `audio_level`.
- `audio.rs`: `AudioController::new` returns
  `Result<Self, (AudioError, AudioInitKind)>` (`Recoverable` / `Terminal`).
  Drop uses done-channel + `JoinHandle::drop` (auto-detach) instead of
  blocking `join()` — closes the path where a wedged worker would hang
  Drop forever.
- `settings.rs::sanitize`: warn (no reset) on hotkey tokens that
  tauri-plugin-global-shortcut can't register on macOS
  (`MetaRight`, `AltRight`, `RCmd`, single modifiers, etc.).
- `polish.rs`: `reqwest` gets `.timeout(30s)`; body adds `"think": false`
  to skip qwen3.5's thinking chain.
- `settings.rs`: default `hotkey = "Cmd+["`, `polish_enabled = false`.

README performance table updated with measured data (2026-08-22,
4 trials, 5–6s Chinese audio):
- STT 2–10s, polish 2–7s, **total 11–23s**.
- Disable polish to drop to 2–10s.
