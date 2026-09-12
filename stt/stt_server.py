"""Local STT service for the voice input tool.

Bridges a desktop client (Tauri) to a local ASR model over plain HTTP. The
backend is pluggable via the `STTBackend` Protocol; the current implementation
is `MLXAudioBackend` (Apple-Silicon-optimised belle-whisper). Other ASR models
(faster-whisper, whisper.cpp, etc.) can be wired in by implementing the same
Protocol without changing the FastAPI layer.

Endpoints
---------
GET  /health             -> liveness + model status
GET  /config             -> current model / language / port
POST /transcribe         -> multipart file upload, returns {text, ...}
POST /transcribe_stream  -> SSE: emits {text, is_final, progress, audio_position} per partial
POST /prewarm            -> trigger model load in background (idempotent)
"""

from __future__ import annotations

import asyncio
import gc
import io
import json
import logging
import os
import threading
import time
import wave
from contextlib import asynccontextmanager
from typing import Optional, Protocol

import numpy as np
from fastapi import FastAPI, File, Form, HTTPException, UploadFile
from pydantic import BaseModel

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
)
log = logging.getLogger("stt")

MODEL_ID = os.environ.get("STT_MODEL", "mlx-community/belle-whisper-large-v3-turbo-zh-fp16")
DEFAULT_LANG = os.environ.get("STT_LANG", "zh")
HOST = os.environ.get("STT_HOST", "127.0.0.1")
PORT = int(os.environ.get("STT_PORT", "18787"))

# Idle unload: after this many seconds without a /transcribe call, drop
# the model from memory. The model is reloaded on the next /prewarm or
# /transcribe. Tunable via env so we can test shorter values during dev.
IDLE_UNLOAD_SEC = int(os.environ.get("STT_IDLE_UNLOAD_SEC", str(30 * 60)))

# Predictive prewarm: the desktop client calls /prewarm when the user
# presses the hotkey. By the time they finish speaking (typically 5-10s)
# the model should be loaded, so /transcribe pays ~0s cold-start cost.
PREWARM_ENABLED = os.environ.get("STT_PREWARM", "1") == "1"


# ---------------------------------------------------------------------------
# Backend abstraction — swap ASR engines here without touching the HTTP layer.
# v0.2 will add `transcribe_stream` here for VAD-segmented streaming
# (per the maeda.pm reference), but for now we only do one-shot.
# ---------------------------------------------------------------------------


class STTBackend(Protocol):
    """Pluggable synchronous ASR engine.

    Every potentially blocking call is serialized by the lifecycle helpers
    below and dispatched off the FastAPI event loop.
    """

    @property
    def is_loaded(self) -> bool: ...
    def load(self) -> None: ...
    def unload(self) -> None: ...
    def transcribe(self, audio: np.ndarray, language: str) -> str: ...


class MLXAudioBackend:
    """Apple-Silicon-optimised belle-whisper via the `mlx_audio` package."""

    def __init__(self, model_id: str) -> None:
        self.model_id = model_id
        self._model = None

    @property
    def is_loaded(self) -> bool:
        return self._model is not None

    def load(self) -> None:
        from mlx_audio.stt import load

        log.info("loading model: %s", self.model_id)
        t0 = time.time()
        self._model = load(self.model_id)
        log.info("model ready in %.1fs", time.time() - t0)

    def unload(self) -> None:
        """Drop the in-memory model + release the Metal command queue /
        MLX allocator pool the model was holding. Idempotent.
        Called by the idle unloader thread after IDLE_UNLOAD_SEC."""
        if self._model is None:
            return
        log.info("unloading model (idle unload)")
        t0 = time.time()
        # Drop the reference first so any later `del` actually frees.
        model = self._model
        self._model = None
        del model
        gc.collect()
        try:
            # Best-effort: flush MLX's compiled-kernel cache so the
            # memory can actually be returned to the OS instead of
            # sitting in a freed-but-pinned pool.
            import mlx.core as mx
            mx.metal.clear_cache()
        except Exception as e:
            log.debug("mx.metal.clear_cache failed (non-fatal): %s", e)
        log.info("model unloaded in %.1fs", time.time() - t0)

    def _language_arg(self, language: str) -> str:
        return None if language in ("", "auto") else language

    def transcribe(self, audio: np.ndarray, language: str) -> str:
        if self._model is None:
            # Synchronous fallback if a request sneaks in before /prewarm
            # finishes (or the model was unloaded between prewarm and
            # transcribe). Pays cold-load cost on the request thread.
            log.warning("transcribe called with no model loaded, loading now")
            self.load()
        kwargs = {}
        lang = self._language_arg(language)
        if lang is not None:
            kwargs["language"] = lang
        result = self._model.generate(audio, **kwargs)
        return (getattr(result, "text", None) or str(result)).strip()


# Future backends: drop in another class implementing STTBackend and select
# in `_make_backend()` below.
def _make_backend() -> STTBackend:
    return MLXAudioBackend(MODEL_ID)


_backend: STTBackend = None  # type: ignore[assignment]
_last_request_at: float = 0.0
_lifecycle_lock = threading.Lock()
_state_lock = threading.Lock()
_lifecycle_state = "uninitialized"


def _set_lifecycle_state(state: str) -> None:
    global _lifecycle_state
    with _state_lock:
        _lifecycle_state = state


def _lifecycle_snapshot() -> tuple[str, float]:
    """Return state and last activity without waiting for MLX work."""
    with _state_lock:
        return _lifecycle_state, _last_request_at


def _record_activity() -> None:
    """Update the idle-unload clock. Called by every endpoint that
    proves the model is in active use."""
    global _last_request_at
    with _state_lock:
        _last_request_at = time.time()


def _ensure_loaded_locked() -> tuple[STTBackend, bool]:
    """Return a loaded backend. Caller must hold `_lifecycle_lock`."""
    global _backend
    if _backend is None:
        _backend = _make_backend()
    if _backend.is_loaded:
        return _backend, False

    _set_lifecycle_state("loading")
    try:
        _backend.load()
    except Exception:
        _set_lifecycle_state("error")
        raise
    _record_activity()
    _set_lifecycle_state("ready")
    return _backend, True


def _prewarm_sync() -> str:
    """Load the model under the shared lifecycle lock."""
    with _lifecycle_lock:
        _backend_instance, loaded_now = _ensure_loaded_locked()
        return "loaded" if loaded_now else "already_loaded"


def _transcribe_sync(samples: np.ndarray, language: str) -> str:
    """Load if needed and transcribe without racing any unload."""
    with _lifecycle_lock:
        backend, _loaded_now = _ensure_loaded_locked()
        _set_lifecycle_state("transcribing")
        try:
            return backend.transcribe(samples, language)
        finally:
            _record_activity()
            _set_lifecycle_state("ready" if backend.is_loaded else "error")


def _unload_sync(*, force: bool) -> str:
    """Unload only while holding the same lock used by inference and load."""
    with _lifecycle_lock:
        if _backend is None or not _backend.is_loaded:
            _set_lifecycle_state("unloaded")
            return "noop"

        if not force:
            _state, last_request_at = _lifecycle_snapshot()
            if last_request_at == 0.0:
                return "active"
            idle = time.time() - last_request_at
            if idle <= IDLE_UNLOAD_SEC:
                return "active"
            log.info(
                "idle %.0fs (>%ds threshold), unloading model",
                idle,
                IDLE_UNLOAD_SEC,
            )

        _set_lifecycle_state("unloading")
        try:
            _backend.unload()
        except Exception:
            _set_lifecycle_state("error")
            raise
        _set_lifecycle_state("unloaded")
        return "unloaded"


def _idle_unloader() -> None:
    """Daemon thread: every minute, check whether the model has been
    idle for > IDLE_UNLOAD_SEC. If yes, unload it from memory. The
    next /prewarm or /transcribe will reload."""
    while True:
        time.sleep(60)
        try:
            _unload_sync(force=False)
        except Exception as e:
            log.exception("idle unload failed: %s", e)


# ---------------------------------------------------------------------------
# FastAPI app + lifespan
# ---------------------------------------------------------------------------


@asynccontextmanager
async def lifespan(_app: FastAPI):
    await asyncio.to_thread(_prewarm_sync)
    # Prewarm: run a 0.5s silence transcription so the first real request
    # doesn't pay GPU kernel cache warmup cost. Failure is non-fatal — the
    # first real call will just be a bit slower, same as before.
    try:
        wav_bytes = _make_silence_wav()
        samples, _dur, _sr = _read_wav_bytes(wav_bytes)
        t0 = time.time()
        await asyncio.to_thread(_transcribe_sync, samples, DEFAULT_LANG)
        log.info("prewarm done in %.2fs", time.time() - t0)
    except Exception as e:
        log.warning("prewarm failed (non-fatal): %s", e)
    # Idle unloader daemon: drops the model from memory after
    # IDLE_UNLOAD_SEC of inactivity. /prewarm re-loads it on demand.
    if IDLE_UNLOAD_SEC > 0:
        threading.Thread(target=_idle_unloader, daemon=True).start()
        log.info("idle unloader started (threshold=%ds)", IDLE_UNLOAD_SEC)
    yield
    await asyncio.to_thread(_unload_sync, force=True)


app = FastAPI(
    title="S-Voice STT Service",
    version="0.1.0",
    description=(
        "Local ASR bridge with pluggable backends. POST audio, get text. "
        "Used by the voice input desktop client."
    ),
    lifespan=lifespan,
)


class Health(BaseModel):
    status: str
    model: str
    default_language: str
    port: int
    model_loaded: bool
    idle_sec: Optional[float] = None


class PrewarmResult(BaseModel):
    status: str  # "already_loaded" | "loaded" | "disabled"
    model_loaded: bool


class TranscribeResult(BaseModel):
    text: str
    language: str
    duration_sec: Optional[float] = None
    inference_sec: float
    model: str


def _read_wav_bytes(audio_bytes: bytes) -> tuple[np.ndarray, float, int]:
    """Decode WAV bytes to a mono float32 array at the file's native sample rate.

    Returns (samples, duration_sec, sample_rate). Backends consume the array
    directly — no tempfile, no ffprobe, no ffmpeg.
    """
    with wave.open(io.BytesIO(audio_bytes), "rb") as w:
        sampwidth = w.getsampwidth()
        n_channels = w.getnchannels()
        sample_rate = w.getframerate()
        n_frames = w.getnframes()
        raw = w.readframes(n_frames)
    if sampwidth == 2:
        samples = np.frombuffer(raw, dtype=np.int16).astype(np.float32) / 32768.0
    elif sampwidth == 4:
        samples = np.frombuffer(raw, dtype=np.int32).astype(np.float32) / 2147483648.0
    else:
        raise ValueError(f"unsupported sample width: {sampwidth} bytes")
    if n_channels > 1:
        samples = samples.reshape(-1, n_channels).mean(axis=1)
    duration = n_frames / float(sample_rate) if sample_rate else 0.0
    return samples.astype(np.float32), duration, sample_rate


def _make_silence_wav(duration_sec: float = 0.5, sample_rate: int = 16000) -> bytes:
    """Build a 16-bit mono PCM WAV of silence, in memory, for prewarm."""
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(sample_rate)
        w.writeframes(b"\x00\x00" * int(duration_sec * sample_rate))
    return buf.getvalue()


def _resolve_lang(language: Optional[str]) -> str:
    raw = (language or "").strip().lower()
    return "auto" if raw in ("", "auto") else raw


def _require_backend() -> STTBackend:
    if _backend is None:
        raise HTTPException(503, "model not loaded yet")
    return _backend


# ---------------------------------------------------------------------------
# Endpoints
# ---------------------------------------------------------------------------


@app.get("/health", response_model=Health)
async def health() -> Health:
    state, last_request_at = _lifecycle_snapshot()
    model_loaded = state in ("ready", "transcribing")
    idle = (time.time() - last_request_at) if last_request_at else None
    return Health(
        status="ok" if model_loaded else state,
        model=MODEL_ID,
        default_language=DEFAULT_LANG,
        port=PORT,
        model_loaded=model_loaded,
        idle_sec=idle,
    )


@app.get("/config")
async def config() -> dict:
    return {
        "model": MODEL_ID,
        "host": HOST,
        "port": PORT,
        "default_language": DEFAULT_LANG,
    }


@app.post("/transcribe", response_model=TranscribeResult)
async def transcribe(
    file: UploadFile = File(...),
    language: Optional[str] = Form(None),
) -> TranscribeResult:
    _require_backend()
    lang = _resolve_lang(language)

    audio_bytes = await file.read()
    try:
        samples, duration, _sr = _read_wav_bytes(audio_bytes)
    except Exception as e:
        log.warning("audio decode failed: %s", e)
        raise HTTPException(400, f"audio decode failed: {e}")

    t0 = time.time()
    try:
        text = await asyncio.to_thread(_transcribe_sync, samples, lang)
    except Exception as e:
        log.exception("transcription failed")
        raise HTTPException(500, f"transcription failed: {e}")
    inference_sec = time.time() - t0

    log.info(
        "transcribed %.1fs audio in %.2fs (lang=%s) -> %d chars",
        duration, inference_sec, lang, len(text),
    )
    return TranscribeResult(
        text=text,
        language=lang,
        duration_sec=duration,
        inference_sec=inference_sec,
        model=MODEL_ID,
    )


@app.post("/prewarm", response_model=PrewarmResult)
async def prewarm() -> PrewarmResult:
    """Trigger lazy model load on a worker thread. Idempotent.

    The desktop client calls this when the user presses the global
    hotkey. While the user is speaking (typically 5-10s) the model
    loads in the background, so the subsequent /transcribe pays
    ~0s cold-start cost. Without this, a /transcribe after idle
    unload would block for the full 5-15s model load.
    """
    if not PREWARM_ENABLED:
        state, _last_activity = _lifecycle_snapshot()
        return PrewarmResult(
            status="disabled",
            model_loaded=state in ("ready", "transcribing"),
        )
    _record_activity()  # the user's about to use it
    try:
        status = await asyncio.to_thread(_prewarm_sync)
    except Exception as e:
        log.exception("prewarm failed: %s", e)
        raise HTTPException(500, f"prewarm failed: {e}")
    return PrewarmResult(status=status, model_loaded=True)


@app.post("/admin/unload")
async def admin_unload() -> dict:
    """Dev/test hook: force the model to unload right now regardless of
    the idle timer. Used by the integration test harness to simulate
    the post-idle-unload state without waiting IDLE_UNLOAD_SEC."""
    try:
        status = await asyncio.to_thread(_unload_sync, force=True)
    except Exception as e:
        log.exception("forced unload failed: %s", e)
        raise HTTPException(500, f"unload failed: {e}")
    return {"status": status, "model_loaded": False}


if __name__ == "__main__":
    import uvicorn

    uvicorn.run(
        "stt_server:app",
        host=HOST,
        port=PORT,
        log_level="info",
        access_log=False,
    )
