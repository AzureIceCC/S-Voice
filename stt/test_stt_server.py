"""Concurrency regression tests for the STT model lifecycle."""

from __future__ import annotations

import asyncio
import threading
import time
import unittest

import numpy as np

from stt import stt_server


class FakeBackend:
    def __init__(self, *, loaded: bool) -> None:
        self._loaded = loaded
        self.load_calls = 0
        self.transcribe_calls = 0
        self.unload_calls = 0
        self.load_started = threading.Event()
        self.allow_load = threading.Event()
        self.transcribe_started = threading.Event()
        self.allow_transcribe = threading.Event()
        self.allow_load.set()
        self.allow_transcribe.set()

    @property
    def is_loaded(self) -> bool:
        return self._loaded

    def load(self) -> None:
        self.load_calls += 1
        self.load_started.set()
        if not self.allow_load.wait(timeout=2):
            raise TimeoutError("test did not release load")
        self._loaded = True

    def transcribe(self, _audio: np.ndarray, _language: str) -> str:
        self.transcribe_calls += 1
        self.transcribe_started.set()
        if not self.allow_transcribe.wait(timeout=2):
            raise TimeoutError("test did not release transcription")
        return "ok"

    def unload(self) -> None:
        self.unload_calls += 1
        self._loaded = False


class LifecycleConcurrencyTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self) -> None:
        self.original_backend = stt_server._backend
        self.original_state = stt_server._lifecycle_state
        self.original_last_request_at = stt_server._last_request_at

    def tearDown(self) -> None:
        stt_server._backend = self.original_backend
        stt_server._lifecycle_state = self.original_state
        stt_server._last_request_at = self.original_last_request_at

    async def test_health_responds_and_unload_waits_during_transcription(self) -> None:
        backend = FakeBackend(loaded=True)
        backend.allow_transcribe.clear()
        stt_server._backend = backend
        stt_server._set_lifecycle_state("ready")

        transcription = asyncio.create_task(
            asyncio.to_thread(
                stt_server._transcribe_sync,
                np.zeros(16, dtype=np.float32),
                "zh",
            )
        )
        self.assertTrue(await asyncio.to_thread(backend.transcribe_started.wait, 1))

        health = await asyncio.wait_for(stt_server.health(), timeout=0.1)
        self.assertEqual(health.status, "ok")
        self.assertTrue(health.model_loaded)

        unload = asyncio.create_task(stt_server.admin_unload())
        await asyncio.sleep(0.05)
        self.assertEqual(backend.unload_calls, 0)

        backend.allow_transcribe.set()
        self.assertEqual(await transcription, "ok")
        self.assertEqual((await unload)["status"], "unloaded")
        self.assertEqual(backend.unload_calls, 1)

    async def test_prewarm_and_transcribe_share_one_load(self) -> None:
        backend = FakeBackend(loaded=False)
        backend.allow_load.clear()
        stt_server._backend = backend
        stt_server._set_lifecycle_state("unloaded")

        prewarm = asyncio.create_task(stt_server.prewarm())
        self.assertTrue(await asyncio.to_thread(backend.load_started.wait, 1))

        transcription = asyncio.create_task(
            asyncio.to_thread(
                stt_server._transcribe_sync,
                np.zeros(16, dtype=np.float32),
                "zh",
            )
        )
        await asyncio.sleep(0.05)
        self.assertEqual(backend.transcribe_calls, 0)

        health = await asyncio.wait_for(stt_server.health(), timeout=0.1)
        self.assertEqual(health.status, "loading")
        self.assertFalse(health.model_loaded)

        backend.allow_load.set()
        self.assertEqual((await prewarm).status, "loaded")
        self.assertEqual(await transcription, "ok")
        self.assertEqual(backend.load_calls, 1)
        self.assertEqual(backend.transcribe_calls, 1)

    async def test_idle_unload_rechecks_activity_after_waiting_for_inference(self) -> None:
        backend = FakeBackend(loaded=True)
        backend.allow_transcribe.clear()
        stt_server._backend = backend
        stt_server._set_lifecycle_state("ready")
        stt_server._last_request_at = time.time() - stt_server.IDLE_UNLOAD_SEC - 1

        transcription = asyncio.create_task(
            asyncio.to_thread(
                stt_server._transcribe_sync,
                np.zeros(16, dtype=np.float32),
                "zh",
            )
        )
        self.assertTrue(await asyncio.to_thread(backend.transcribe_started.wait, 1))

        idle_unload = asyncio.create_task(
            asyncio.to_thread(stt_server._unload_sync, force=False)
        )
        await asyncio.sleep(0.05)
        self.assertEqual(backend.unload_calls, 0)

        backend.allow_transcribe.set()
        self.assertEqual(await transcription, "ok")
        self.assertEqual(await idle_unload, "active")
        self.assertEqual(backend.unload_calls, 0)


if __name__ == "__main__":
    unittest.main()
