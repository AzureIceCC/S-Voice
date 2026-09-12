# S-Voice

AI voice input for **macOS**. Press a global hotkey, speak, get polished text
inserted at the cursor. Apple SpeechAnalyzer is the default on macOS 26 and
newer; local Whisper remains available as an explicitly selected offline backend.

Inspired by [Typeless](https://typeless.com), built from scratch with local-first
components:

- **Default STT** — Apple SpeechAnalyzer on-device recognition on macOS 26
- **Optional STT** — MLX belle-whisper-large-v3-turbo-zh (explicit offline selection; no automatic fallback)
- **Polish** — Ollama qwen3.5:2b-q4_K_M (local LLM, 30-minute auto-unload)
- **UI** — Tauri 2 (Rust backend, vanilla HTML/JS frontend)

## Architecture

```
┌────────────────┐
│ cpal microphone│
└───────┬─────────┘
       │ WAV
       ▼
┌────────────────┐       explicitly selected STT backend
│ Tauri pipeline │──┬──► Apple SpeechAnalyzer (default, on-device)
│ (Rust + HTML)  │  └──► FastAPI STT bridge ──► MLX Whisper (optional)
└───────┬─────────┘
       │ recognized text
       ▼
┌────────────────┐       optional HTTP       ┌────────────────┐
│ Tauri pipeline │ ─────────────────────► │ Ollama polish  │
└───────┬─────────┘ ◄────────────────────── └────────────────┘
       │ clipboard + CGEventPost Cmd+V
       ▼
┌────────────────┐
│ active macOS app│
└────────────────┘
```

The selected backend is authoritative: failures are surfaced to the user and do
not automatically switch backends. Apple SpeechAnalyzer is the default and runs
through a bundled Swift helper using system-managed on-device model assets. The
Python FastAPI bridge on `127.0.0.1:18787` is only used when Local Whisper is
explicitly selected; it keeps the MLX runtime outside the Rust app bundle.

## Layout

```
s-voice/
├── scripts/               # setup helpers + Apple capability probe
├── stt/                   # optional MLX/FastAPI backend + scripts
├── src-tauri/             # Rust backend (Tauri 2)
│   ├── native/
│   │   └── apple_speech_helper.swift # SpeechAnalyzer helper
│   ├── src/
│   │   ├── main.rs        # entry
│   │   ├── lib.rs         # Tauri setup + commands
│   │   ├── audio.rs       # cpal capture, dedicated worker thread
│   │   ├── apple_speech.rs # Swift helper lifecycle + IPC
│   │   ├── hotkey.rs      # global shortcut combo parser
│   │   ├── stt_client.rs  # HTTP to optional STT bridge
│   │   ├── polish.rs      # HTTP to Ollama (keep_alive=30m)
│   │   ├── output.rs      # clipboard + Cmd+V paste
│   │   ├── settings.rs    # JSON persistence
│   │   ├── pipeline.rs    # state machine orchestrator
│   │   └── tray.rs        # system tray menu
│   ├── build.rs             # compiles and embeds Swift helper
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   └── capabilities/
└── ui/                    # vanilla HTML/JS — settings + floating panel
```

## Run

### Requirements

- Apple Silicon Mac. Intel builds are not part of the tested deployment path.
- macOS 26 or newer for the default Apple SpeechAnalyzer backend.
- Xcode 26 with its command-line tools selected (`xcode-select -p`).
- Rust 1.77 or newer and Tauri CLI 2 (`cargo install tauri-cli --version '^2'`).
- Ollama only when local text polish is enabled.
- `uv` and Python 3.12 only when the optional Local Whisper backend is used.

The application bundle declares macOS 12 as its minimum because Local Whisper
can run without SpeechAnalyzer on older macOS releases. Building the current
source still requires Xcode 26 because the bundled Swift helper references the
macOS 26 Speech framework APIs.

### Build and install the default Apple Speech configuration

```bash
# Clone and enter the repository
git clone https://github.com/AzureIceCC/S-Voice.git
cd S-Voice

# Optional local polish model
ollama pull qwen3.5:2b-q4_K_M

# Build the bundled app (requires Xcode 26 for SpeechAnalyzer)
cd src-tauri
cargo tauri build --bundles app

# Install the result
ditto target/release/bundle/macos/S-Voice.app /Applications/S-Voice.app
```

Launch `/Applications/S-Voice.app` from Finder. Apple SpeechAnalyzer is selected
by default and does not require Python, MLX, or the STT bridge. Its language
asset is managed by macOS and may be downloaded the first time it is needed.

No prebuilt, notarized binary is currently published in GitHub Releases. A local
source build may trigger Gatekeeper, and replacing or re-signing the app can make
macOS treat it as a new Accessibility entry. If permission stops working after
a rebuild, remove the stale S-Voice entry and add `/Applications/S-Voice.app`
again.

### Optional Local Whisper setup

```bash
# From the repository root
./scripts/setup_venv.sh

# Start the bridge before selecting Local Whisper in Settings
./stt/start_stt.sh
```

The STT bridge is only needed when the local Whisper backend is selected. The
app checks it at startup and attempts to run `stt/start_stt.sh`; a source checkout
or an explicit `STT_HOME` is currently required for that script. For an installed
app backed by this checkout, set `STT_HOME` to the absolute repository path before
launching S-Voice, or start the bridge manually from the checkout. The first
transcription downloads the configured Hugging Face model if it is not cached;
allow roughly 1.5GB for the model and about 2-3GB of warm runtime memory.

### Development launch

```bash
cd src-tauri
cargo run
```

For persistent macOS microphone, Speech Recognition, and Accessibility grants,
use the installed `.app` rather than relying on the bare development binary.

### First launch (macOS permissions)

1. **Microphone** — macOS prompts on first recording. Approve.
2. **Accessibility** — required for global hotkey + Cmd+V paste.
   System Settings → Privacy & Security → Accessibility → enable S-Voice.
3. **Speech Recognition** — macOS may request access when first selecting the
   Apple backend. Failures do not switch to local Whisper.

## Troubleshooting

- **Hotkey works but text is not inserted** — enable S-Voice in System Settings
  → Privacy & Security → Accessibility. After rebuilding/reinstalling, remove a
  stale entry and add `/Applications/S-Voice.app` again.
- **Microphone recording fails** — enable S-Voice under Privacy & Security →
  Microphone, then relaunch the installed app.
- **Apple Speech fails immediately** — confirm macOS 26+, grant Speech
  Recognition access, and allow the system language asset to download. S-Voice
  will report the error; it will not switch to Local Whisper automatically.
- **Local Whisper is unreachable** — run `./stt/start_stt.sh`, then check
  `curl http://127.0.0.1:18787/health`. Installed builds need a valid `STT_HOME`
  or a bridge started manually from a checkout.
- **Local polish fails** — confirm Ollama is running and that
  `ollama list` contains the model configured in Settings. Disable polish to use
  the raw STT result.
- **Inspect logs** — the canonical file is
  `~/Library/Logs/S-Voice/s-voice.log`; unified logs use subsystem
  `com.s-voice.app`. Avoid launching with `RUST_LOG=warn` when collecting INFO
  performance timings.

## Default hotkey

`Cmd+[` — toggle (press once to start, press again to stop).

Change in the Settings window: click "录制", press your combo, save.

## Pipeline

```
press hotkey
  → state: recording (floating panel shows waveform + timer)
press hotkey again
  → state: processing (floating panel shows "处理中...")
  → record WAV via cpal (16kHz mono, on a dedicated worker thread)
  → transcribe with the explicitly selected Apple Speech or local Whisper backend
  → POST /api/generate on Ollama (keep_alive=30m)
  → write final text to clipboard
  → CGEventPost Cmd+V (osascript fallback when Accessibility is unavailable)
  → state: idle (floating panel hides)
```

Errors emit Tauri events; the settings window shows the last error.

## Performance

Bundled-app validation on 2026-09-12 with Apple SpeechAnalyzer and
`qwen3.5:2b-q4_K_M` local polish:

| Stage | Observed time |
|---|---:|
| Apple SpeechAnalyzer | 224-378ms |
| Local 2B Q4 polish | 769ms-1.19s |
| CGEventPost paste | 2-22ms |
| **Total after recording** | **1.14-1.57s** |

The three consecutive recordings were 2.9s, 3.0s, and 8.9s long and completed
without capture, pipeline, or paste errors.

### Optional Local Whisper baseline

实测数据（4 段录音，5-6s 中文音频，2026-08-22 `Cmd+[` hotkey 路径）：

| Stage | Time | 备注 |
|---|---|---|
| Mic stop + WAV encode | <100ms |  |
| STT (5-6s Chinese audio) | 2-10s | MLX 资源调度波动，5x 差异是正常的 |
| Ollama polish (qwen3.5:9b-mlx) | 2-7s | `think: false` 已开；首次 cold start 可能 30s timeout 然后 fallback 到 raw |
| Clipboard + paste | <100ms |  |
| **Total perceived latency** | **11-23s** | 适合"按一下、说一段、等几秒、贴上去"的工作流 |

> These figures describe the optional 2026-08-22 MLX/9B configuration, not the
> current Apple SpeechAnalyzer + 2B Q4 default path above.

That legacy all-local stack measured about 12GB total when warm (macOS baseline
+ Ollama 9B ~6GB + STT bridge ~3GB + Tauri ~250MB). It is not representative
of the current default Apple SpeechAnalyzer + 2B Q4 configuration.

## Known limitations (v0.2)

- Mic + Accessibility permissions must be granted manually on first launch
- Cmd+V paste doesn't work in some contexts (terminal, password fields, some
  Electron apps) — Typeless has the same limitation
- macOS only; Windows / Linux builds are untested
- The installed app cannot yet bundle the Python/MLX runtime; local Whisper may
  require `STT_HOME` or manually running `./stt/start_stt.sh` from a checkout.
- Apple Speech requires macOS 26; Apple Intelligence is capability-reserved but
  unavailable on devices Apple reports as ineligible.
- Custom dictionary, per-app style, translation mode, and floating history are
  future work.

## Development

```bash
# Rebuild
cd src-tauri && cargo build

# Release build (smaller binary, slower compile)
cd src-tauri && cargo build --release
# → src-tauri/target/release/s-voice (~10MB)

# Run the STT bridge with auto-reload (for prompt iteration)
cd stt && ../.venv/bin/python -m uvicorn stt_server:app --reload
```

## License

Licensed under the [Apache License 2.0](LICENSE).
