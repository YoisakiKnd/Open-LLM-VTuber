import unittest

import numpy as np
from starlette.websockets import WebSocketDisconnect

from src.open_llm_vtuber.service_context import ServiceContext
from src.open_llm_vtuber.websocket_handler import WebSocketHandler


class FakeWebSocket:
    def __init__(self, messages):
        self._messages = iter(messages)

    async def receive(self):
        return next(self._messages)

    async def send_text(self, message):
        raise AssertionError(f"unexpected server message: {message}")


class WebSocketHandlerProtocolTests(unittest.IsolatedAsyncioTestCase):
    async def test_binary_pcm_is_buffered_before_disconnect(self):
        handler = WebSocketHandler(ServiceContext())
        client_uid = "test-client"
        from src.open_llm_vtuber.audio_buffer import AudioBuffer

        handler.received_data_buffers[client_uid] = AudioBuffer()
        websocket = FakeWebSocket(
            [
                {"type": "websocket.receive", "bytes": bytes([0x00, 0x40])},
                {"type": "websocket.disconnect", "code": 1000},
            ]
        )

        with self.assertRaises(WebSocketDisconnect):
            await handler.handle_websocket_communication(websocket, client_uid)

        np.testing.assert_array_equal(
            handler.received_data_buffers[client_uid].drain(),
            np.array([0.5], dtype=np.float32),
        )


if __name__ == "__main__":
    unittest.main()
