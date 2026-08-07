import unittest

import numpy as np

from src.open_llm_vtuber.audio_buffer import AudioBuffer


class AudioBufferTests(unittest.TestCase):
    def test_drain_concatenates_chunks_and_clears_buffer(self):
        buffer = AudioBuffer()
        buffer.append([0.25, -0.5])
        buffer.append(np.array([1.0], dtype=np.float64))

        np.testing.assert_array_equal(
            buffer.drain(), np.array([0.25, -0.5, 1.0], dtype=np.float32)
        )
        self.assertEqual(len(buffer), 0)
        np.testing.assert_array_equal(buffer.drain(), np.array([], dtype=np.float32))

    def test_append_pcm16le_normalizes_samples(self):
        buffer = AudioBuffer()
        buffer.append_pcm16le(bytes([0x00, 0x80, 0x00, 0x00, 0xFF, 0x7F]))

        np.testing.assert_array_equal(
            buffer.drain(),
            np.array([-1.0, 0.0, 32767.0 / 32768.0], dtype=np.float32),
        )

    def test_append_pcm16le_rejects_incomplete_sample(self):
        buffer = AudioBuffer()
        with self.assertRaisesRegex(ValueError, "complete samples"):
            buffer.append_pcm16le(b"\x00")
        self.assertEqual(len(buffer), 0)

    def test_rejects_audio_over_limit_without_changing_buffer(self):
        buffer = AudioBuffer(max_samples=2)
        buffer.append([0.25])

        with self.assertRaisesRegex(ValueError, "audio buffer exceeds"):
            buffer.append([0.5, 0.75])

        np.testing.assert_array_equal(buffer.drain(), np.array([0.25], dtype=np.float32))

    def test_append_copies_mutable_input(self):
        source = np.array([0.5], dtype=np.float32)
        buffer = AudioBuffer()
        buffer.append(source)
        source[0] = 1.0

        np.testing.assert_array_equal(buffer.drain(), np.array([0.5], dtype=np.float32))


if __name__ == "__main__":
    unittest.main()
