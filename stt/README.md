# STT service

Local MLX belle-whisper bridge. The desktop client (Electron / Tauri / Swift) records
audio, POSTs it here, and gets back transcribed text. The model stays loaded in memory
between calls, so warm requests are sub-second.

## Run

```bash
# from repo root
./stt/start_stt.sh     # background, writes .stt.pid, logs to stt.log
./stt/stop_stt.sh      # SIGTERM, with SIGKILL fallback after 5s
```

Or foreground (for debugging):

```bash
source .venv/bin/activate
cd stt
python -m uvicorn stt_server:app --host 127.0.0.1 --port 18787 --log-level info
```

## Config (env vars)

| Var | Default | Notes |
|---|---|---|
| `STT_MODEL` | `mlx-community/belle-whisper-large-v3-turbo-zh-fp16` | any HF id mlx-audio can load |
| `STT_LANG`  | `zh` | default language if client doesn't pass one |
| `STT_HOST`  | `127.0.0.1` | bind host |
| `STT_PORT`  | `18787` | bind port |
| `STT_IDLE_UNLOAD_SEC` | `1800` | unload the model after this many idle seconds; `0` disables |
| `STT_PREWARM` | `1` | allow predictive model loading through `/prewarm` |

## API

### `GET /health`

```json
{ "status": "ok", "model": "...", "default_language": "zh", "port": 18787, "model_loaded": true, "idle_sec": 12.3 }
```

### `GET /config`

Full server config (model / lang / host / port).

### `POST /transcribe`

Multipart form:
- `file` — any audio ffmpeg can read (wav, mp3, m4a, aiff, ogg, webm, opus, ...)
- `language` (optional) — ISO code, default `STT_LANG`

Response:

```json
{
  "text": "你好，我想测试一下这个语音输入法，能不能正常工作？",
  "language": "zh",
  "duration_sec": 4.99,
  "inference_sec": 0.92,
  "model": "mlx-community/belle-whisper-large-v3-turbo-zh-fp16"
}
```

### Example client call (curl)

```bash
curl -X POST http://127.0.0.1:18787/transcribe \
  -F "file=@recording.m4a" \
  -F "language=zh"
```

## Performance (M-series, fp16 model)

- First request after startup: ~10-30s (model load)
- Warm requests: ~0.2x real-time for short clips (5s audio → ~1s inference)
- Memory: ~2GB resident when model is loaded

## Why a separate service

- The MLX runtime is Python-only — keeping it out of the desktop client's process
  avoids shipping a Python interpreter in the app bundle.
- One model loaded once, shared by any client on the machine.
- Trivial to swap model: change `STT_MODEL` and restart.
