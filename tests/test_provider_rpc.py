import json
import unittest

from src.open_llm_vtuber.routes import (
    PROVIDER_RPC_BINARY_PREFIX,
    PROVIDER_RPC_VERSION,
    ProviderRpcWebSocket,
)


class FakeWebSocket:
    def __init__(self, messages=None):
        self.messages = list(messages or [])
        self.sent_text = []
        self.sent_bytes = []

    async def receive(self):
        return self.messages.pop(0)

    async def send_text(self, text):
        self.sent_text.append(text)

    async def send_bytes(self, data):
        self.sent_bytes.append(data)

    async def close(self, *args, **kwargs):
        return None


class ProviderRpcWebSocketTests(unittest.IsolatedAsyncioTestCase):
    async def test_text_and_binary_messages_are_unwrapped(self):
        text_envelope = json.dumps(
            {
                "version": PROVIDER_RPC_VERSION,
                "kind": "text",
                "payload": '{"type":"heartbeat"}',
            }
        )
        websocket = FakeWebSocket(
            [
                {"type": "websocket.receive", "text": text_envelope},
                {
                    "type": "websocket.receive",
                    "bytes": PROVIDER_RPC_BINARY_PREFIX + b"\x01\x02",
                },
            ]
        )
        transport = ProviderRpcWebSocket(websocket)

        self.assertEqual(
            await transport.receive(),
            {"type": "websocket.receive", "text": '{"type":"heartbeat"}'},
        )
        self.assertEqual(
            await transport.receive(),
            {"type": "websocket.receive", "bytes": b"\x01\x02"},
        )

    async def test_outgoing_messages_are_versioned(self):
        websocket = FakeWebSocket()
        transport = ProviderRpcWebSocket(websocket)

        await transport.send_text('{"type":"control"}')
        await transport.send_bytes(b"audio")

        self.assertEqual(
            json.loads(websocket.sent_text[0]),
            {
                "version": PROVIDER_RPC_VERSION,
                "kind": "text",
                "payload": '{"type":"control"}',
            },
        )
        self.assertEqual(
            websocket.sent_bytes[0], PROVIDER_RPC_BINARY_PREFIX + b"audio"
        )

    async def test_unversioned_messages_are_rejected(self):
        websocket = FakeWebSocket(
            [{"type": "websocket.receive", "text": '{"type":"heartbeat"}'}]
        )
        transport = ProviderRpcWebSocket(websocket)

        with self.assertRaisesRegex(ValueError, "Invalid provider RPC"):
            await transport.receive()


if __name__ == "__main__":
    unittest.main()
