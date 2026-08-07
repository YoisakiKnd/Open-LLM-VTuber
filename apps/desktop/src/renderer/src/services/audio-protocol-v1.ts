export const AUDIO_PROTOCOL_VERSION = 1;
export const AUDIO_SAMPLE_RATE = 16_000;
export const AUDIO_BINARY_CHUNK_BYTES = 8_192;

const capabilityRequests = new Map<string, Promise<boolean>>();

export function float32ToPcm16Le(audio: Float32Array): Uint8Array {
  const output = new Uint8Array(audio.length * 2);
  const view = new DataView(output.buffer);
  for (let index = 0; index < audio.length; index += 1) {
    const sample = Math.max(-1, Math.min(1, audio[index] ?? 0));
    const pcm =
      sample < 0 ? Math.round(sample * 32_768) : Math.round(sample * 32_767);
    view.setInt16(index * 2, pcm, true);
  }
  return output;
}

export function splitPcm16Le(
  audio: Uint8Array,
  chunkBytes = AUDIO_BINARY_CHUNK_BYTES,
): Uint8Array[] {
  if (chunkBytes <= 0 || chunkBytes % 2 !== 0) {
    throw new Error("PCM16 chunk size must be a positive even number");
  }
  const chunks: Uint8Array[] = [];
  for (let offset = 0; offset < audio.byteLength; offset += chunkBytes) {
    chunks.push(
      audio.subarray(offset, Math.min(offset + chunkBytes, audio.byteLength)),
    );
  }
  return chunks;
}

export function isAudioProtocolV1Capabilities(value: unknown): boolean {
  if (typeof value !== "object" || value === null || Array.isArray(value))
    return false;
  const capabilities = value as Record<string, unknown>;
  return (
    capabilities.audio_protocol_version === AUDIO_PROTOCOL_VERSION &&
    Array.isArray(capabilities.audio_encodings) &&
    capabilities.audio_encodings.includes("pcm_s16le") &&
    Array.isArray(capabilities.audio_sample_rates) &&
    capabilities.audio_sample_rates.includes(AUDIO_SAMPLE_RATE)
  );
}

export function gatewaySupportsAudioProtocolV1(
  baseUrl: string,
  fetchImplementation: typeof fetch = fetch,
): Promise<boolean> {
  const normalizedBaseUrl = baseUrl.replace(/\/$/, "");
  const existing = capabilityRequests.get(normalizedBaseUrl);
  if (existing) return existing;

  const request = fetchImplementation(
    `${normalizedBaseUrl}/gateway/capabilities`,
    {
      cache: "no-store",
      credentials: "same-origin",
      headers: { Accept: "application/json" },
    },
  )
    .then(async (response) => {
      if (!response.ok) return false;
      return isAudioProtocolV1Capabilities(await response.json());
    })
    .catch(() => false);
  capabilityRequests.set(normalizedBaseUrl, request);
  return request;
}
