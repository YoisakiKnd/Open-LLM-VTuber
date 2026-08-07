import { describe, expect, it } from "vitest";
import {
  CLIENT_PREFERENCES_FEATURE_FLAG_KEY,
  type SettingsStorage,
} from "./client-preferences-repository";
import { CLIENT_PREFERENCES_STORAGE_KEY } from "./legacy-client-preferences";
import {
  DEFAULT_DESKTOP_RUNTIME_BASE_URL,
  prepareRuntimeSettingsBootstrap,
} from "./runtime-settings-bootstrap";

class RecordingStorage implements SettingsStorage {
  readonly writes: Array<{ key: string; value: string }> = [];

  constructor(private readonly values: Record<string, string> = {}) {}

  getItem(key: string): string | null {
    return this.values[key] ?? null;
  }

  setItem(key: string, value: string): void {
    this.values[key] = value;
    this.writes.push({ key, value });
  }
}

describe("runtime settings bootstrap", () => {
  it("enables the Rust settings domain by default without a feature flag", () => {
    const storage = new RecordingStorage();

    const bootstrap = prepareRuntimeSettingsBootstrap(storage, {
      protocol: "https:",
      origin: "https://vtuber.example",
    });

    expect(bootstrap).toMatchObject({
      enabled: true,
      apiBaseUrl: "https://vtuber.example",
      clientPreferences: { status: "ready" },
    });
    expect(storage.writes.map(({ key }) => key)).toContain(
      CLIENT_PREFERENCES_STORAGE_KEY,
    );
  });

  it("honours an explicit feature flag opt-out", () => {
    const storage = new RecordingStorage({
      [CLIENT_PREFERENCES_FEATURE_FLAG_KEY]: "false",
    });

    const bootstrap = prepareRuntimeSettingsBootstrap(storage, {
      protocol: "https:",
      origin: "https://vtuber.example",
    });

    expect(bootstrap.enabled).toBe(false);
    expect(bootstrap.clientPreferences).toEqual({ status: "disabled" });
  });

  it("enables the Rust snapshot load only after legacy preparation succeeds", () => {
    const storage = new RecordingStorage({
      [CLIENT_PREFERENCES_FEATURE_FLAG_KEY]: "true",
      i18nextLng: '"zh"',
    });

    const bootstrap = prepareRuntimeSettingsBootstrap(storage, {
      protocol: "http:",
      origin: "http://127.0.0.1:12394",
    });

    expect(bootstrap).toMatchObject({
      enabled: true,
      fallbackClientSettings: {
        appearance: { locale: "zh" },
      },
      clientPreferences: {
        status: "ready",
        source: "migrated",
        preferences: { appearance: { locale: "zh" } },
      },
    });
    expect(storage.writes.map(({ key }) => key)).toContain(
      CLIENT_PREFERENCES_STORAGE_KEY,
    );
  });

  it("keeps Rust loading disabled when current preferences are corrupt", () => {
    const storage = new RecordingStorage({
      [CLIENT_PREFERENCES_FEATURE_FLAG_KEY]: "true",
      [CLIENT_PREFERENCES_STORAGE_KEY]: "not-json",
    });

    const bootstrap = prepareRuntimeSettingsBootstrap(storage, {
      protocol: "https:",
      origin: "https://vtuber.example",
    });

    expect(bootstrap.enabled).toBe(false);
    expect(bootstrap.fallbackClientSettings).toBeNull();
    expect(bootstrap.clientPreferences).toEqual({
      status: "blocked",
      reason: "invalid-current",
    });
    expect(storage.writes).toEqual([]);
  });

  it("uses the loopback Rust runtime for packaged file renderers", () => {
    const bootstrap = prepareRuntimeSettingsBootstrap(new RecordingStorage(), {
      protocol: "file:",
      origin: "null",
    });

    expect(bootstrap.apiBaseUrl).toBe(DEFAULT_DESKTOP_RUNTIME_BASE_URL);
  });
});
