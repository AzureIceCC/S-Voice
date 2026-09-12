# S-Voice

[中文](#s-voice-中文) | [English](#s-voice-english)

---

<a name="s-voice-中文"></a>

# S-Voice

macOS 上的 AI 语音输入，纯本地化项目，无需部署线上第三方API。按下全局热键，讲话，润色后的文本自动插入到光标位置。**macOS 26 及以上默认使用 Apple SpeechAnalyzer（系统级识别）**；本地 Whisper 仍可作为离线后端被显式选用。

灵感来自 [Typeless](https://typeless.com)，从零开始用本地优先的组件构建：

- **默认 STT** — macOS 26 上的 Apple SpeechAnalyzer（设备端识别）
- **可选 STT** — MLX belle-whisper-large-v3-turbo-zh（显式选用作为离线后端；不会自动 fallback）
- **润色（Polish）** — Ollama qwen3.5:2b-q4_K_M（本地 LLM，30 分钟自动卸载）
- **UI** — Tauri 2（Rust 后端 + 原生 HTML/JS 前端）；i18n（zh / en）

## 架构

```
┌────────────────┐
│ cpal 麦克风    │
└───────┬────────┘
        │ WAV
        ▼
┌────────────────┐        显式选用的 STT 后端
│ Tauri 流水线   │──┬──► Apple SpeechAnalyzer（默认，设备端）
│（Rust + HTML） │  └──► FastAPI STT 网关 ──► MLX Whisper（可选）
└───────┬────────┘
        │ 识别文本
        ▼
┌────────────────┐        可选 HTTP        ┌────────────────┐
│ Tauri 流水线   │ ─────────────────────► │ Ollama 润色    │
└───────┬────────┘ ◄────────────────────── └────────────────┘
        │ 剪贴板 + CGEventPost Cmd+V
        ▼
┌────────────────┐
│ 当前 macOS 应用│
└────────────────┘
```

所选后端是唯一权威：失败会直接展示给用户，**不会**自动切换后端。Apple SpeechAnalyzer 是默认后端，通过打包的 Swift 辅助进程调用系统管理的设备端模型资源。Python FastAPI 网关监听 `127.0.0.1:18787`，仅在显式选择 Local Whisper 时使用——它把 MLX 运行时隔离在 Rust app bundle 之外。

## 目录结构

```
s-voice/
├── scripts/               # 安装辅助脚本 + Apple 能力探测
├── stt/                   # 可选 MLX/FastAPI 后端 + 启动脚本
├── src-tauri/             # Rust 后端（Tauri 2）
│   ├── native/
│   │   └── apple_speech_helper.swift # SpeechAnalyzer 辅助进程
│   ├── src/
│   │   ├── main.rs        # 入口
│   │   ├── lib.rs         # Tauri 初始化 + commands
│   │   ├── audio.rs       # cpal 采集，专用 worker 线程
│   │   ├── apple_speech.rs # Swift 辅助进程生命周期 + IPC
│   │   ├── hotkey.rs      # 全局快捷键解析
│   │   ├── stt_client.rs  # HTTP 客户端（可选 STT 网关）
│   │   ├── polish.rs      # HTTP 客户端（Ollama，keep_alive=30m）
│   │   ├── output.rs      # 剪贴板 + Cmd+V 粘贴
│   │   ├── settings.rs    # JSON 持久化
│   │   ├── pipeline.rs    # 状态机编排
│   │   └── tray.rs        # 系统托盘菜单
│   ├── build.rs             # 编译并嵌入 Swift 辅助进程
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   └── capabilities/
└── ui/                    # 原生 HTML/JS —— 设置 + 悬浮窗
```

## 运行

### 系统要求

- Apple Silicon Mac。Intel 构建未在已测试的部署路径中。
- macOS 26 或更高版本以使用默认的 Apple SpeechAnalyzer 后端。
- Xcode 26 并已选择命令行工具（`xcode-select -p` 验证）。
- Rust 1.77 或更高版本 + Tauri CLI 2（`cargo install tauri-cli --version '^2'`）。
- 仅当启用本地文本润色时才需要 Ollama。
- 仅当使用可选的 Local Whisper 后端时才需要 `uv` 和 Python 3.12。

应用 bundle 声明 macOS 12 为最低系统版本，因为 Local Whisper 在更老的 macOS 上也能跑（不需要 SpeechAnalyzer）。但当前源码构建需要 Xcode 26，因为打包的 Swift 辅助进程引用了 macOS 26 的 Speech 框架 API。

### 编译并安装默认 Apple Speech 配置

```bash
# 克隆并进入仓库
git clone https://github.com/AzureIceCC/S-Voice.git
cd S-Voice

# 可选：拉取本地润色模型
ollama pull qwen3.5:2b-q4_K_M

# 编译打包版 app（SpeechAnalyzer 需要 Xcode 26）
cd src-tauri
cargo tauri build --bundles app

# 安装产物
ditto target/release/bundle/macos/S-Voice.app /Applications/S-Voice.app
```

从 Finder 启动 `/Applications/S-Voice.app`。Apple SpeechAnalyzer 是默认后端，不需要 Python、MLX 或 STT 网关。语言资源由 macOS 管理，首次需要时可能自动下载。

目前 GitHub Releases 不发布已公证的预编译二进制。本地源码构建可能触发 Gatekeeper，重新打包或重签名后 macOS 可能把 Accessibility 授权当作新条目处理。如果重编译后权限失效，先移除旧的 S-Voice 条目，再重新添加 `/Applications/S-Voice.app`。

### 可选 Local Whisper 设置

```bash
# 在仓库根目录
./scripts/setup_venv.sh

# 在设置里选择 Local Whisper 之前先启动网关
./stt/start_stt.sh
```

STT 网关仅在选择本地 Whisper 后端时才需要。app 启动时会检查网关并尝试运行 `stt/start_stt.sh`；当前该脚本需要源码 checkout 或显式的 `STT_HOME` 环境变量。对于用此 checkout 支撑的已安装 app，启动 S-Voice 前把 `STT_HOME` 设为仓库的绝对路径，或者手动从 checkout 启动网关。第一次转写会下载配置的 Hugging Face 模型（约 1.5GB），运行时大约需要 2-3GB 热内存。

### 开发模式启动

```bash
cd src-tauri
cargo run
```

要获得持久的 macOS 麦克风、Speech Recognition、Accessibility 授权，请使用安装好的 `.app` 而不是裸的 dev binary。

### 首次启动（macOS 权限）

1. **麦克风** — 首次录音时 macOS 会弹窗提示。同意。
2. **辅助功能（Accessibility）** — 全局热键 + Cmd+V 粘贴需要。
   系统设置 → 隐私与安全 → 辅助功能 → 启用 S-Voice。
3. **Speech Recognition** — 首次选择 Apple 后端时 macOS 可能请求授权。失败不会自动切到 Local Whisper。

## 故障排查

- **热键有效但文本未插入** — 系统设置 → 隐私与安全 → 辅助功能中启用 S-Voice。重新编译/重新安装后，移除旧条目后重新添加 `/Applications/S-Voice.app`。
- **麦克风录音失败** — 隐私与安全 → 麦克风中启用 S-Voice，然后重新启动已安装的 app。
- **Apple Speech 立刻失败** — 确认 macOS 26+、授予 Speech Recognition 权限、等待系统语言资源下载完成。S-Voice 会显示错误，**不会**自动切到 Local Whisper。
- **Local Whisper 不可达** — 运行 `./stt/start_stt.sh`，然后 `curl http://127.0.0.1:18787/health` 验证。已安装的 build 需要有效的 `STT_HOME` 或从 checkout 手动启动网关。
- **本地润色失败** — 确认 Ollama 在跑、`ollama list` 里包含设置中配置的模型。关闭润色则用原始 STT 结果。
- **查看日志** — 规范文件是 `~/Library/Logs/S-Voice/s-voice.log`；unified log 的 subsystem 是 `com.s-voice.app`。采集 INFO 性能计时数据时不要用 `RUST_LOG=warn` 启动。

## 默认热键

`Cmd+[` — 按一次开始，再按一次停止。

在设置窗口里改：点击"录制"，按想要的组合键，保存。

## 语言

设置窗口和悬浮窗支持 **中文**（`zh`，默认）和 **英文**（`en`）。在设置窗口顶部的"显示语言"下拉框切换 —— 改动立即应用到悬浮窗并持久化到磁盘。其他设置仍需要点"保存"按钮，因为它们会触发副作用（热键重新注册、模型切换等）。

润色保持原语言：中文转写润色为中文，英文转写润色为英文，中英混合输入保留原样。本地 LLM（`qwen3.5:2b-q4_K_M`）是多语言模型，但被显式 prompt 不要翻译。

## 处理流程

```
按下热键
  → 状态：recording（悬浮窗显示波形 + 计时器）
再次按下热键
  → 状态：processing（悬浮窗显示"处理中..."）
  → cpal 录制 WAV（16kHz 单声道，在专用 worker 线程上）
  → 用显式选用的 Apple Speech 或 Local Whisper 后端转写
  → POST /api/generate 到 Ollama（keep_alive=30m）
  → 写入最终文本到剪贴板
  → CGEventPost Cmd+V（Accessibility 不可用时 fallback 到 osascript）
  → 状态：idle（悬浮窗隐藏）
```

错误通过 Tauri event 发出；设置窗口会显示最近一次的错误。

## 性能

`2026-09-12` 在打包 app 上用 Apple SpeechAnalyzer + `qwen3.5:2b-q4_K_M` 本地润色做的实测：

| 阶段 | 实测耗时 |
|---|---:|
| Apple SpeechAnalyzer | 224-378ms |
| 本地 2B Q4 润色 | 769ms-1.19s |
| CGEventPost 粘贴 | 2-22ms |
| **录音后总耗时** | **1.14-1.57s** |

3 段连续录音分别为 2.9s、3.0s、8.9s，全部录音、流水线、粘贴无错误。

### 可选 Local Whisper 基线

实测数据（4 段录音，5-6s 中文音频，2026-08-22 `Cmd+[` 热键路径）：

| 阶段 | 耗时 | 备注 |
|---|---|---|
| 麦克风停止 + WAV 编码 | <100ms |  |
| STT（5-6s 中文音频） | 2-10s | MLX 资源调度波动，5x 差异是正常的 |
| Ollama 润色（qwen3.5:9b-mlx） | 2-7s | `think: false` 已开；首次冷启动可能 30s timeout 然后 fallback 到原文 |
| 剪贴板 + 粘贴 | <100ms |  |
| **总感知延迟** | **11-23s** | 适合"按一下、说一段、等几秒、贴上去"的工作流 |

> 这些数据是 2026-08-22 的可选 MLX/9B 配置，不是当前默认的 Apple SpeechAnalyzer + 2B Q4 路径。

那套老的全本地栈热态约 12GB（macOS 基础 + Ollama 9B ~6GB + STT 网关 ~3GB + Tauri ~250MB）。不适用于当前默认的 Apple SpeechAnalyzer + 2B Q4 配置。

## 已知限制（v0.2）

- 首次启动需手动授予麦克风 + 辅助功能权限
- Cmd+V 粘贴在某些场景不生效（终端、密码框、某些 Electron app）—— Typeless 也有同样限制
- 仅支持 macOS；Windows / Linux 构建未测试
- 安装版 app 还不能打包 Python/MLX 运行时；Local Whisper 可能需要 `STT_HOME` 或手动从 checkout 跑 `./stt/start_stt.sh`
- Apple Speech 需要 macOS 26；Apple Intelligence 是能力预留，在 Apple 判定为不可用的设备上无法使用
- 自定义词库、按 app 风格、翻译模式、悬浮历史是未来工作

## 开发

```bash
# 重新编译
cd src-tauri && cargo build

# Release 编译（更小的 binary，更慢的编译）
cd src-tauri && cargo build --release
# → src-tauri/target/release/s-voice（约 10MB）

# 用 auto-reload 跑 STT 网关（用于 prompt 迭代）
cd stt && ../.venv/bin/python -m uvicorn stt_server:app --reload
```

## 许可证

基于 [Apache License 2.0](LICENSE) 开源。

---

<a name="s-voice-english"></a>

# S-Voice

AI voice input for **macOS**, a pure localized program without necessary to deploy with an online model. Press a global hotkey, speak, get polished text
inserted at the cursor. Apple SpeechAnalyzer is the default on macOS 26 and
newer; local Whisper remains available as an explicitly selected offline backend.

Inspired by [Typeless](https://typeless.com), built from scratch with local-first
components:

- **Default STT** — Apple SpeechAnalyzer on-device recognition on macOS 26
- **Optional STT** — MLX belle-whisper-large-v3-turbo-zh (explicit offline selection; no automatic fallback)
- **Polish** — Ollama qwen3.5:2b-q4_K_M (local LLM, 30-minute auto-unload)
- **UI** — Tauri 2 (Rust backend, vanilla HTML/JS frontend); i18n (zh / en)

## Architecture

```
┌────────────────┐
│ cpal microphone│
└───────┬────────┘
       │ WAV
       ▼
┌────────────────┐       explicitly selected STT backend
│ Tauri pipeline │──┬──► Apple SpeechAnalyzer (default, on-device)
│ (Rust + HTML)  │  └──► FastAPI STT bridge ──► MLX Whisper (optional)
└───────┬────────┘
       │ recognized text
       ▼
┌────────────────┐       optional HTTP       ┌────────────────┐
│ Tauri pipeline │ ─────────────────────► │ Ollama polish  │
└───────┬────────┘ ◄────────────────────── └────────────────┘
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

## Language

The settings window and floating panel render in **Chinese** (`zh`, default)
or **English** (`en`). Switch via the "显示语言" / "Display language"
dropdown at the top of the settings window — the change is applied
immediately to the floating panel and persisted to disk. Other settings
still require the "保存" button because they trigger side effects
(hotkey re-registration, model switches).

Polish preserves the spoken language: a Chinese transcript polishes to
Chinese, an English transcript polishes to English, and mixed-language
input keeps the original mix. The local LLM (`qwen3.5:2b-q4_K_M`) is
multilingual but is explicitly prompted not to translate.

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

Measured data (4 recordings, 5-6s Chinese audio, 2026-08-22 `Cmd+[` hotkey path):

| Stage | Time | Note |
|---|---|---|
| Mic stop + WAV encode | <100ms |  |
| STT (5-6s Chinese audio) | 2-10s | MLX resource scheduling variance; 5x spread is normal |
| Ollama polish (qwen3.5:9b-mlx) | 2-7s | `think: false` is on; first cold start may 30s-timeout then fall back to raw |
| Clipboard + paste | <100ms |  |
| **Total perceived latency** | **11-23s** | Fits a "press, speak a paragraph, wait, paste" workflow |

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
