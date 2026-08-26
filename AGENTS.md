# S-Voice — Project Notes (for AI coding agents)

> **Audience**: this file is auto-loaded by AI coding agents (Mavis /
> Cursor / Aider / Devin / etc.) as project context. **Human-facing
> docs are `README.md`** (user guide) and `CHANGELOG.md` (version
> history). Keep the split: agents want constraints and decisions,
> users want features and usage.

Local AI voice input for macOS. Press a global hotkey, speak, get text
inserted at the cursor. Tauri 2 + MLX Whisper + Ollama polish.

## Current state

- **Stage**: **v0.1.0 released 2026-08-23** (see `CHANGELOG.md` for the
  full list of changes). Future work tracked in the Backlog section
  below.
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
- [x] **#7 Microphone permission** — solved by the v0.1 `.app` bundle
      install. TCC now keys on `com.s-voice.app` (stable bundle id)
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
- [ ] **#11 Smooth model switching + safe save race** — discovered
      2026-08-23: the `RunEvent::Exit` save-back overwrites external
      edits to `settings.json` made while S-Voice is still running.
      Concretely: when the user (or an automation script) changes
      `ollama_model` from "9b-mlx" to "2b" in `settings.json` while
      S-Voice is alive, then quits S-Voice via Cmd+Q, the exit-time
      `snapshot.save()` writes the in-memory 9b-mlx state back to
      disk, undoing the external edit. Discovered while switching
      from `qwen3.5:9b-mlx` to `qwen3.5:2b` — same root cause as
      any future "swap model and restart" flow. Two layers of fix:
      1. **Safe save** — `Settings::save` should diff against the
         on-disk hash; if identical, skip the write. ~15 LOC.
         Eliminates the race.
      2. **Hot reload** — a `cmd_reload_settings` Tauri command that
         re-reads `settings.json` from disk and updates the in-memory
         `Arc<RwLock<Settings>>`. ~30 LOC. Lets users edit
         `settings.json` (e.g. swap model, tweak hotkey) without
         quitting S-Voice, and without losing the change on next quit.
         Bonus: settings UI gets a "重新加载" button that calls this.
      Together these make model switching a one-shot operation
      instead of a kill-restart dance.

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
