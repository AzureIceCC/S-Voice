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
"""

from __future__ import annotations

import io
import json
import logging
import os
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


# ---------------------------------------------------------------------------
# Backend abstraction — swap ASR engines here without touching the HTTP layer.
# v0.2 will add `transcribe_stream` here for VAD-segmented streaming
# (per the maeda.pm reference), but for now we only do one-shot.
# ---------------------------------------------------------------------------


class STTBackend(Protocol):
    """Pluggable ASR engine. Methods are sync — wrap in asyncio.to_thread
    if a future backend is genuinely async."""

    def load(self) -> None: ...
    def transcribe(self, audio: np.ndarray, language: str) -> str: ...


class MLXAudioBackend:
    """Apple-Silicon-optimised belle-whisper via the `mlx_audio` package."""

    def __init__(self, model_id: str) -> None:
        self.model_id = model_id
        self._model = None

    def load(self) -> None:
        from mlx_audio.stt import load

        log.info("loading model: %s", self.model_id)
        t0 = time.time()
        self._model = load(self.model_id)
        log.info("model ready in %.1fs", time.time() - t0)

    def _language_arg(self, language: str) -> str:
        return None if language in ("", "auto") else language

    def transcribe(self, audio: np.ndarray, language: str) -> str:
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


# ---------------------------------------------------------------------------
# FastAPI app + lifespan
# ---------------------------------------------------------------------------


@asynccontextmanager
async def lifespan(_app: FastAPI):
    global _backend
    _backend = _make_backend()
    _backend.load()
    # Prewarm: run a 0.5s silence transcription so the first real request
    # doesn't pay GPU kernel cache warmup cost. Failure is non-fatal — the
    # first real call will just be a bit slower, same as before.
    try:
        wav_bytes = _make_silence_wav()
        samples, _dur, _sr = _read_wav_bytes(wav_bytes)
        t0 = time.time()
        _backend.transcribe(samples, DEFAULT_LANG)
        log.info("prewarm done in %.2fs", time.time() - t0)
    except Exception as e:
        log.warning("prewarm failed (non-fatal): %s", e)
    yield
    _backend = None


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
    return Health(
        status="ok" if _backend is not None else "loading",
        model=MODEL_ID,
        default_language=DEFAULT_LANG,
        port=PORT,
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
    backend = _require_backend()
    lang = _resolve_lang(language)

    audio_bytes = await file.read()
    try:
        samples, duration, _sr = _read_wav_bytes(audio_bytes)
    except Exception as e:
        log.warning("audio decode failed: %s", e)
        raise HTTPException(400, f"audio decode failed: {e}")

    t0 = time.time()
    try:
        text = backend.transcribe(samples, lang)
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


if __name__ == "__main__":
    import uvicorn

    uvicorn.run(
        "stt_server:app",
        host=HOST,
        port=PORT,
        log_level="info",
        access_log=False,
    )
