import { describe, expect, it } from "vitest";
import { CLIENT_PREFERENCES_FEATURE_FLAG_KEY } from "./client-preferences-repository";
import { prepareClientPreferences } from "./prepare-client-preferences";

class MemoryStorage {
  constructor(private readonly values: Record<string, string>) {}

  getItem(key: string): string | null {
    return this.values[key] ?? null;
  }

  setItem(key: string, value: string): void {
    this.values[key] = value;
  }
}

describe("renderer client preference preparation", () => {
  it("normalizes legacy local backgrounds for an HTTP renderer", () => {
    const result = prepareClientPreferences(
      new MemoryStorage({
        [CLIENT_PREFERENCES_FEATURE_FLAG_KEY]: "true",
        backgroundUrl: '"http://127.0.0.1:12393/bg/room.jpeg"',
      }),
      { protocol: "https:", origin: "https://vtuber.example" },
    );

    expect(result.status).toBe("ready");
    if (result.status !== "ready")
      throw new Error("preferences were not ready");
    expect(result.preferences.appearance.backgroundUrl).toBe(
      "https://vtuber.example/bg/room.jpeg",
    );
  });

  it("preserves legacy local backgrounds for a file renderer", () => {
    const result = prepareClientPreferences(
      new MemoryStorage({
        [CLIENT_PREFERENCES_FEATURE_FLAG_KEY]: "true",
        backgroundUrl: '"http://127.0.0.1:12393/bg/room.jpeg"',
      }),
      { protocol: "file:", origin: "null" },
    );

    expect(result.status).toBe("ready");
    if (result.status !== "ready")
      throw new Error("preferences were not ready");
    expect(result.preferences.appearance.backgroundUrl).toBe(
      "http://127.0.0.1:12393/bg/room.jpeg",
    );
  });
});
