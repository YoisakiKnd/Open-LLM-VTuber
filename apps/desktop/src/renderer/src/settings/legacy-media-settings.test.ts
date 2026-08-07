import { describe, expect, it } from "vitest";
import {
  DEFAULT_IMAGE_COMPRESSION_QUALITY,
  DEFAULT_IMAGE_MAX_WIDTH,
  IMAGE_COMPRESSION_QUALITY_KEY,
  IMAGE_MAX_WIDTH_KEY,
  mirrorLegacyMediaSettings,
  readLegacyMediaSettings,
} from "./legacy-media-settings";

class MemoryStorage {
  constructor(readonly values: Record<string, string> = {}) {}

  getItem(key: string): string | null {
    return this.values[key] ?? null;
  }

  setItem(key: string, value: string): void {
    this.values[key] = value;
  }
}

describe("legacy media settings compatibility", () => {
  it("reads valid compatibility values", () => {
    expect(
      readLegacyMediaSettings(
        new MemoryStorage({
          [IMAGE_COMPRESSION_QUALITY_KEY]: "0.65",
          [IMAGE_MAX_WIDTH_KEY]: "1280",
        }),
      ),
    ).toEqual({ imageCompressionQuality: 0.65, imageMaxWidth: 1280 });
  });

  it("uses safe defaults for invalid compatibility values", () => {
    expect(
      readLegacyMediaSettings(
        new MemoryStorage({
          [IMAGE_COMPRESSION_QUALITY_KEY]: "2",
          [IMAGE_MAX_WIDTH_KEY]: "-1",
        }),
      ),
    ).toEqual({
      imageCompressionQuality: DEFAULT_IMAGE_COMPRESSION_QUALITY,
      imageMaxWidth: DEFAULT_IMAGE_MAX_WIDTH,
    });
  });

  it("mirrors committed Rust values without JSON encoding", () => {
    const storage = new MemoryStorage();

    mirrorLegacyMediaSettings(storage, {
      imageCompressionQuality: 0.7,
      imageMaxWidth: 1920,
    });

    expect(storage.values).toEqual({
      [IMAGE_COMPRESSION_QUALITY_KEY]: "0.7",
      [IMAGE_MAX_WIDTH_KEY]: "1920",
    });
  });
});
