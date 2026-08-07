from __future__ import annotations

import numpy as np


class AudioBuffer:
    """Collect audio chunks without repeatedly copying prior samples."""

    DEFAULT_MAX_SAMPLES = 16_000 * 120

    def __init__(self, max_samples: int = DEFAULT_MAX_SAMPLES) -> None:
        if max_samples <= 0:
            raise ValueError("max_samples must be positive")
        self._chunks: list[np.ndarray] = []
        self._sample_count = 0
        self._max_samples = max_samples

    def append(self, audio: np.ndarray | list[float]) -> None:
        chunk = np.asarray(audio, dtype=np.float32)
        if chunk.size == 0:
            return
        if self._sample_count + chunk.size > self._max_samples:
            raise ValueError(
                f"audio buffer exceeds {self._max_samples} samples"
            )
        self._chunks.append(chunk.copy())
        self._sample_count += chunk.size

    def append_pcm16le(self, audio: bytes) -> None:
        if len(audio) % 2 != 0:
            raise ValueError("PCM16-LE audio must contain complete samples")
        samples = np.frombuffer(audio, dtype="<i2").astype(np.float32) / 32768.0
        self.append(samples)

    def drain(self) -> np.ndarray:
        if not self._chunks:
            return np.array([], dtype=np.float32)
        audio = np.concatenate(self._chunks)
        self.clear()
        return audio

    def clear(self) -> None:
        self._chunks.clear()
        self._sample_count = 0

    def __len__(self) -> int:
        return self._sample_count
