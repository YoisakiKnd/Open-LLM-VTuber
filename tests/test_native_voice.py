"""Unit tests for the native voice handlers (asr-transcribe / tts-speak)."""

import asyncio
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

from src.open_llm_vtuber.audio_buffer import AudioBuffer
from src.open_llm_vtuber.websocket_handler import WebSocketHandler


class FakeWebSocket:
    """Collects text frames exactly like the real send_text path."""

    def __init__(self):
        self.sent = []

    async def send_text(self, payload: str):
        self.sent.append(payload)


class FakeAsrEngine:
    def __init__(self, transcript: str = "你好世界"):
        self.transcript = transcript
        self.transcribed_audio = None

    async def async_transcribe_np(self, audio):
        self.transcribed_audio = audio
        return self.transcript


class FakeTtsEngine:
    def __init__(self):
        self.generated = []

    async def async_generate_audio(self, text: str, file_name_no_ext: str):
        directory = tempfile.mkdtemp(prefix="olv-tts-test-")
        path = Path(directory) / f"{file_name_no_ext}.wav"
        # Write a minimal valid WAV with a non-silent 440 Hz tone.
        import math
        import struct
        import wave

        frames = bytearray()
        for index in range(8000):
            sample = int(0.5 * 32767 * math.sin(2 * math.pi * 440 * index / 16000))
            frames += struct.pack("<h", sample)
        with wave.open(str(path), "wb") as wav:
            wav.setnchannels(1)
            wav.setsampwidth(2)
            wav.setframerate(16000)
            wav.writeframes(bytes(frames))
        self.generated.append((text, str(path)))
        return str(path)

    def remove_file(self, path: str):
        Path(path).unlink(missing_ok=True)


def make_handler(asr_engine, tts_engine):
    handler = WebSocketHandler(default_context_cache=None)
    context = SimpleNamespace(
        asr_engine=asr_engine,
        tts_engine=tts_engine,
        live2d_model=None,
        character_config=SimpleNamespace(
            character_name="Mao",
            avatar="mao.png",
        ),
    )
    handler.client_contexts["uid-1"] = context
    handler.received_data_buffers["uid-1"] = AudioBuffer()
    return handler


class AsrTranscribeTests(unittest.TestCase):
    def test_transcribes_buffered_audio_and_returns_result(self):
        handler = make_handler(FakeAsrEngine(), None)
        websocket = FakeWebSocket()
        handler.received_data_buffers["uid-1"].append_pcm16le(b"\x00\x00" * 1600)

        asyncio.run(handler._handle_asr_transcribe(websocket, "uid-1", {}))

        self.assertEqual(len(websocket.sent), 1)
        payload = json.loads(websocket.sent[0])
        self.assertEqual(payload["type"], "asr-result")
        self.assertEqual(payload["text"], "你好世界")
        # The buffer was drained.
        self.assertEqual(len(handler.received_data_buffers["uid-1"]), 0)

    def test_empty_audio_returns_empty_result(self):
        handler = make_handler(FakeAsrEngine(), None)
        websocket = FakeWebSocket()

        asyncio.run(handler._handle_asr_transcribe(websocket, "uid-1", {}))

        self.assertEqual(len(websocket.sent), 1)
        self.assertIn('"text": ""', websocket.sent[0])

    def test_missing_engine_returns_empty_result_with_error(self):
        handler = make_handler(None, None)
        websocket = FakeWebSocket()
        handler.received_data_buffers["uid-1"].append_pcm16le(b"\x00\x00" * 1600)

        asyncio.run(handler._handle_asr_transcribe(websocket, "uid-1", {}))

        self.assertEqual(len(websocket.sent), 1)
        self.assertIn('"error": "no ASR engine configured"', websocket.sent[0])

    def test_unknown_client_is_ignored(self):
        handler = make_handler(FakeAsrEngine(), None)
        websocket = FakeWebSocket()

        asyncio.run(handler._handle_asr_transcribe(websocket, "missing-uid", {}))

        self.assertEqual(websocket.sent, [])


class TtsSpeakTests(unittest.TestCase):
    def test_synthesizes_and_emits_audio_payload(self):
        handler = make_handler(None, FakeTtsEngine())
        websocket = FakeWebSocket()

        asyncio.run(handler._handle_tts_speak(websocket, "uid-1", {"text": "你好呀"}))

        self.assertEqual(len(websocket.sent), 1)
        payload = json.loads(websocket.sent[0])
        self.assertEqual(payload["type"], "audio")
        self.assertTrue(payload["audio"], "expected base64 audio payload")
        self.assertEqual(payload["display_text"]["text"], "你好呀")
        self.assertEqual(payload["display_text"]["name"], "Mao")
        self.assertEqual(payload["display_text"]["avatar"], "mao.png")

    def test_empty_text_is_ignored(self):
        handler = make_handler(None, FakeTtsEngine())
        websocket = FakeWebSocket()

        asyncio.run(handler._handle_tts_speak(websocket, "uid-1", {"text": "  "}))

        self.assertEqual(websocket.sent, [])

    def test_missing_engine_is_ignored(self):
        handler = make_handler(None, None)
        websocket = FakeWebSocket()

        asyncio.run(handler._handle_tts_speak(websocket, "uid-1", {"text": "hi"}))

        self.assertEqual(websocket.sent, [])


if __name__ == "__main__":
    unittest.main()
