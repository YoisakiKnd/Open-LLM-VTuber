import { describe, expect, it, vi } from "vitest";
import {
  AUDIO_SAMPLE_RATE,
  float32ToPcm16Le,
  gatewaySupportsAudioProtocolV1,
  isAudioProtocolV1Capabilities,
  splitPcm16Le,
} from "./audio-protocol-v1";

describe("audio protocol V1", () => {
  it("converts and clamps Float32 samples to little-endian PCM16", () => {
    const pcm = float32ToPcm16Le(
      new Float32Array([-2, -1, -0.5, 0, 0.5, 1, 2]),
    );
    const view = new DataView(pcm.buffer);

    expect(
      Array.from({ length: 7 }, (_, index) => view.getInt16(index * 2, true)),
    ).toEqual([-32768, -32768, -16384, 0, 16384, 32767, 32767]);
  });

  it("splits only on complete PCM16 samples", () => {
    const audio = new Uint8Array(18);
    expect(splitPcm16Le(audio, 8).map((chunk) => chunk.byteLength)).toEqual([
      8, 8, 2,
    ]);
    expect(() => splitPcm16Le(audio, 3)).toThrow("positive even number");
  });

  it("strictly recognizes compatible Gateway capabilities", () => {
    const capabilities = {
      audio_protocol_version: 1,
      audio_encodings: ["pcm_s16le"],
      audio_sample_rates: [AUDIO_SAMPLE_RATE, 48_000],
    };

    expect(isAudioProtocolV1Capabilities(capabilities)).toBe(true);
    expect(
      isAudioProtocolV1Capabilities({
        ...capabilities,
        audio_protocol_version: 2,
      }),
    ).toBe(false);
  });

  it("falls back when capability discovery fails", async () => {
    const fetchImplementation = vi.fn().mockRejectedValue(new Error("offline"));

    await expect(
      gatewaySupportsAudioProtocolV1(
        "https://legacy.example",
        fetchImplementation as typeof fetch,
      ),
    ).resolves.toBe(false);
    expect(fetchImplementation).toHaveBeenCalledOnce();
  });
});
