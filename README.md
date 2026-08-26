# S-Voice

Local AI voice input for macOS. Press a global hotkey, speak, get polished text
inserted at the cursor. Everything runs on-device — no audio leaves your machine.

Inspired by [Typeless](https://typeless.com), built from scratch with local-first
components:

- **STT** — MLX belle-whisper-large-v3-turbo-zh (Chinese-tuned Whisper, Apple Silicon accelerated)
- **Polish** — Ollama qwen3.5:9b-mlx (local LLM, 30-minute auto-unload)
- **UI** — Tauri 2 (Rust backend, vanilla HTML/JS frontend)

## Architecture

```
┌────────────────┐    HTTP      ┌────────────────┐
│  Tauri App     │ ──────────►  │  STT bridge    │  ─►  MLX belle-whisper
│  (Rust + HTML) │ ◄──────────  │  (FastAPI)     │  ◄─  (always-loaded)
└────────────────┘              └────────────────┘
        │                            ▲
        │ Cmd+V paste                │ audio/wav
        ▼                            │
   ┌─────────┐                  ┌──────────┐
   │  Any    │                  │  cpal    │
   │  active │                  │  mic     │
   │  app    │                  └──────────┘
   └─────────┘
        │                            ▲
        ▼                            │
   ┌─────────┐   HTTP localhost      │
   │ Ollama  │ ◄────────────────────┘  (polish text)
   │ qwen3.5 │
   └─────────┘
```

The STT bridge is a separate Python process (FastAPI on `127.0.0.1:18787`) so the
MLX runtime stays out of the Rust app bundle and the model can stay loaded
between requests.

## Layout

```
s-voice/
├── stt/                   # FastAPI bridge + start/stop scripts
├── src-tauri/             # Rust backend (Tauri 2)
│   ├── src/
│   │   ├── main.rs        # entry
│   │   ├── lib.rs         # Tauri setup + commands
│   │   ├── audio.rs       # cpal capture, dedicated worker thread
│   │   ├── hotkey.rs      # global shortcut combo parser
│   │   ├── stt_client.rs  # HTTP to STT bridge
│   │   ├── polish.rs      # HTTP to Ollama (keep_alive=30m)
│   │   ├── output.rs      # clipboard + Cmd+V paste
│   │   ├── settings.rs    # JSON persistence
│   │   ├── pipeline.rs    # state machine orchestrator
│   │   └── tray.rs        # system tray menu
│   ├── ui/                # frontend (sibling to src-tauri)
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   └── capabilities/
└── ui/                    # vanilla HTML/JS — settings + floating panel
```

## Run

### One-time setup

```bash
# From s-voice/ root
./scripts/setup_venv.sh     # uv venv with mlx-audio + fastapi

# Pull MLX belle-whisper (one-time, ~1.5GB)
# Already at ~/.cache/huggingface/hub/models--mlx-community--belle-whisper-large-v3-turbo-zh-fp16

# Pull Ollama model
ollama pull qwen3.5:9b-mlx
```

### Launch

```bash
# Terminal 1 — STT bridge (background)
./stt/start_stt.sh

# Terminal 2 — Tauri app (dev mode)
cd src-tauri
cargo run
# or
./target/debug/s-voice
```

The Tauri app **does not auto-start the STT bridge** in v1. Start it manually
before using the app. (A future version can launch the bridge as a child process.)

### First launch (macOS permissions)

1. **Microphone** — macOS prompts on first recording. Approve.
2. **Accessibility** — required for global hotkey + Cmd+V paste.
   System Settings → Privacy & Security → Accessibility → enable S-Voice.

## Default hotkey

`Cmd+Shift+Space` — toggle (press once to start, press again to stop).

Change in the Settings window: click "录制", press your combo, save.

## Pipeline

```
press hotkey
  → state: recording (floating panel shows waveform + timer)
press hotkey again
  → state: processing (floating panel shows "处理中...")
  → record WAV via cpal (16kHz mono, on a dedicated worker thread)
  → POST /transcribe on STT bridge
  → POST /api/generate on Ollama (keep_alive=30m)
  → write final text to clipboard
  → osascript: keystroke "v" using command down
  → state: idle (floating panel hides)
```

Errors emit Tauri events; the settings window shows the last error.

## Performance (M-series, MLX Whisper, qwen3.5:9b-mlx)

实测数据（4 段录音，5-6s 中文音频，2026-08-22 `Cmd+[` hotkey 路径）：

| Stage | Time | 备注 |
|---|---|---|
| Mic stop + WAV encode | <100ms |  |
| STT (5-6s Chinese audio) | 2-10s | MLX 资源调度波动，5x 差异是正常的 |
| Ollama polish (qwen3.5:9b-mlx) | 2-7s | `think: false` 已开；首次 cold start 可能 30s timeout 然后 fallback 到 raw |
| Clipboard + paste | <100ms |  |
| **Total perceived latency** | **11-23s** | 适合"按一下、说一段、等几秒、贴上去"的工作流 |

> **不适合实时对话场景**。11-23s 总延迟对打字式语音输入尚可，对"边说边贴"的实时性需求不友好。
> 极致低延迟需求：关掉 polish（`settings.polish_enabled = false`），只跑 STT → 总延迟可压到 2-10s。

Memory when everything is warm: ~12GB total (macOS baseline + Ollama ~6GB +
STT bridge ~3GB + Tauri ~250MB).

## Known limitations (v0.1)

- Mic + Accessibility permissions must be granted manually on first launch
- Cmd+V paste doesn't work in some contexts (terminal, password fields, some
  Electron apps) — Typeless has the same limitation
- macOS only; Windows / Linux builds are untested
- The STT bridge isn't auto-spawned by the app; run `./stt/start_stt.sh` first
- Custom dictionary, per-app style, translation mode, floating history — all
  planned for v0.2+

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

MIT.
