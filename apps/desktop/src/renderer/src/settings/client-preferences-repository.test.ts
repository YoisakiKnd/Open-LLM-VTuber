import { describe, expect, it } from "vitest";
import {
  CLIENT_PREFERENCES_FEATURE_FLAG_KEY,
  ClientPreferencesRepository,
  type SettingsStorage,
} from "./client-preferences-repository";
import {
  CLIENT_PREFERENCES_STORAGE_KEY,
  LEGACY_SETTINGS_BACKUP_KEY,
  LEGACY_STORAGE_KEYS,
  type LegacySettingsSnapshot,
} from "./legacy-client-preferences";

const MIGRATION_OPTIONS = {
  defaultLocale: "en",
  sameOriginBaseUrl: "http://127.0.0.1:12394",
};

class MemorySettingsStorage implements SettingsStorage {
  readonly reads: string[] = [];
  readonly writes: Array<{ key: string; value: string }> = [];
  failNextCurrentWrite = false;

  constructor(readonly values = new Map<string, string>()) {}

  getItem(key: string): string | null {
    this.reads.push(key);
    return this.values.get(key) ?? null;
  }

  setItem(key: string, value: string): void {
    if (key === CLIENT_PREFERENCES_STORAGE_KEY && this.failNextCurrentWrite) {
      this.failNextCurrentWrite = false;
      throw new Error("simulated current preference write failure");
    }

    this.writes.push({ key, value });
    this.values.set(key, value);
  }
}

function parseStoredValue<T>(storage: MemorySettingsStorage, key: string): T {
  const value = storage.values.get(key);
  if (value === undefined) throw new Error(`Missing stored value: ${key}`);
  return JSON.parse(value) as T;
}

describe("client preference repository", () => {
  it("stays read-only while the feature flag is disabled", () => {
    const storage = new MemorySettingsStorage(
      new Map([["wsUrl", '"wss://remote.example/client-ws"']]),
    );
    const repository = new ClientPreferencesRepository(
      storage,
      MIGRATION_OPTIONS,
    );

    expect(repository.initialize()).toEqual({ status: "disabled" });
    expect(storage.reads).toEqual([CLIENT_PREFERENCES_FEATURE_FLAG_KEY]);
    expect(storage.writes).toEqual([]);
  });

  it("backs up legacy values before creating current preferences", () => {
    const storage = new MemorySettingsStorage(
      new Map([
        [CLIENT_PREFERENCES_FEATURE_FLAG_KEY, "true"],
        ["wsUrl", '"wss://remote.example/client-ws"'],
        ["baseUrl", '"https://remote.example"'],
        ["micOn", "true"],
      ]),
    );
    const result = new ClientPreferencesRepository(
      storage,
      MIGRATION_OPTIONS,
    ).initialize();

    expect(result.status).toBe("ready");
    if (result.status !== "ready") throw new Error("migration was not ready");
    expect(result.source).toBe("migrated");
    expect(result.preferences.connectionOverride).toEqual({
      wsUrl: "wss://remote.example/client-ws",
      baseUrl: "https://remote.example",
    });
    expect(result.preferences.legacy.previousMicOn).toBe(true);
    expect(storage.writes.map(({ key }) => key)).toEqual([
      LEGACY_SETTINGS_BACKUP_KEY,
      CLIENT_PREFERENCES_STORAGE_KEY,
    ]);

    const backup = parseStoredValue<{
      schemaVersion: number;
      values: LegacySettingsSnapshot;
    }>(storage, LEGACY_SETTINGS_BACKUP_KEY);
    expect(backup.schemaVersion).toBe(0);
    expect(backup.values.wsUrl).toBe('"wss://remote.example/client-ws"');
    expect(backup.values.micOn).toBe("true");
    expect(Object.keys(backup.values)).toEqual(LEGACY_STORAGE_KEYS);
  });

  it("is idempotent and does not re-read changed legacy values", () => {
    const storage = new MemorySettingsStorage(
      new Map([
        [CLIENT_PREFERENCES_FEATURE_FLAG_KEY, '"true"'],
        ["wsUrl", '"wss://first.example/client-ws"'],
      ]),
    );
    const repository = new ClientPreferencesRepository(
      storage,
      MIGRATION_OPTIONS,
    );
    const first = repository.initialize();
    expect(first.status).toBe("ready");

    storage.values.set("wsUrl", '"wss://changed.example/client-ws"');
    storage.reads.length = 0;
    storage.writes.length = 0;

    const second = repository.initialize();

    expect(second.status).toBe("ready");
    if (second.status !== "ready")
      throw new Error("preferences were not ready");
    expect(second.source).toBe("current");
    expect(second.preferences.connectionOverride?.wsUrl).toBe(
      "wss://first.example/client-ws",
    );
    expect(storage.reads).toEqual([
      CLIENT_PREFERENCES_FEATURE_FLAG_KEY,
      CLIENT_PREFERENCES_STORAGE_KEY,
    ]);
    expect(storage.writes).toEqual([]);
  });

  it("recovers from a failed current write using the immutable backup", () => {
    const storage = new MemorySettingsStorage(
      new Map([
        [CLIENT_PREFERENCES_FEATURE_FLAG_KEY, "1"],
        ["wsUrl", '"wss://before-failure.example/client-ws"'],
      ]),
    );
    storage.failNextCurrentWrite = true;
    const repository = new ClientPreferencesRepository(
      storage,
      MIGRATION_OPTIONS,
    );

    expect(() => repository.initialize()).toThrow(
      "simulated current preference write failure",
    );
    expect(storage.values.has(LEGACY_SETTINGS_BACKUP_KEY)).toBe(true);
    expect(storage.values.has(CLIENT_PREFERENCES_STORAGE_KEY)).toBe(false);

    storage.values.set("wsUrl", '"wss://after-failure.example/client-ws"');
    const recovered = repository.initialize();

    expect(recovered.status).toBe("ready");
    if (recovered.status !== "ready") throw new Error("recovery was not ready");
    expect(recovered.preferences.connectionOverride?.wsUrl).toBe(
      "wss://before-failure.example/client-ws",
    );
  });

  it("blocks malformed current preferences without overwriting them", () => {
    const malformed = "{not-json";
    const storage = new MemorySettingsStorage(
      new Map([
        [CLIENT_PREFERENCES_FEATURE_FLAG_KEY, "true"],
        [CLIENT_PREFERENCES_STORAGE_KEY, malformed],
      ]),
    );

    const result = new ClientPreferencesRepository(
      storage,
      MIGRATION_OPTIONS,
    ).initialize();

    expect(result).toEqual({
      status: "blocked",
      reason: "invalid-current",
    });
    expect(storage.values.get(CLIENT_PREFERENCES_STORAGE_KEY)).toBe(malformed);
    expect(storage.values.has(LEGACY_SETTINGS_BACKUP_KEY)).toBe(false);
    expect(storage.writes).toEqual([]);
  });

  it("does not downgrade a future preference schema", () => {
    const future = JSON.stringify({ schemaVersion: 2, data: "future" });
    const storage = new MemorySettingsStorage(
      new Map([
        [CLIENT_PREFERENCES_FEATURE_FLAG_KEY, "true"],
        [CLIENT_PREFERENCES_STORAGE_KEY, future],
      ]),
    );

    const result = new ClientPreferencesRepository(
      storage,
      MIGRATION_OPTIONS,
    ).initialize();

    expect(result).toEqual({
      status: "blocked",
      reason: "unsupported-schema",
      storedSchemaVersion: 2,
    });
    expect(storage.values.get(CLIENT_PREFERENCES_STORAGE_KEY)).toBe(future);
    expect(storage.writes).toEqual([]);
  });

  it("does not migrate when an existing backup is invalid", () => {
    const invalidBackup = JSON.stringify({ schemaVersion: 0, values: {} });
    const storage = new MemorySettingsStorage(
      new Map([
        [CLIENT_PREFERENCES_FEATURE_FLAG_KEY, "true"],
        [LEGACY_SETTINGS_BACKUP_KEY, invalidBackup],
        ["wsUrl", '"wss://remote.example/client-ws"'],
      ]),
    );

    const result = new ClientPreferencesRepository(
      storage,
      MIGRATION_OPTIONS,
    ).initialize();

    expect(result).toEqual({
      status: "blocked",
      reason: "invalid-backup",
    });
    expect(storage.values.get(LEGACY_SETTINGS_BACKUP_KEY)).toBe(invalidBackup);
    expect(storage.values.has(CLIENT_PREFERENCES_STORAGE_KEY)).toBe(false);
    expect(storage.writes).toEqual([]);
  });

  it("can be enabled by a rollout default without persisting the flag", () => {
    const storage = new MemorySettingsStorage();
    const result = new ClientPreferencesRepository(storage, {
      ...MIGRATION_OPTIONS,
      enabledByDefault: true,
    }).initialize();

    expect(result.status).toBe("ready");
    expect(storage.values.has(CLIENT_PREFERENCES_FEATURE_FLAG_KEY)).toBe(false);
  });
});
