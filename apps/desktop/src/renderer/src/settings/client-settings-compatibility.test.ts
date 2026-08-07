import { describe, expect, it } from "vitest";
import {
  clientPreferencesToSettings,
  mergeClientSettingsIntoPreferences,
} from "./client-settings-compatibility";
import {
  LEGACY_STORAGE_KEYS,
  migrateLegacyClientPreferences,
  type LegacySettingsSnapshot,
} from "./legacy-client-preferences";

function createLegacySnapshot(): LegacySettingsSnapshot {
  return Object.fromEntries(
    LEGACY_STORAGE_KEYS.map((key) => [key, null]),
  ) as LegacySettingsSnapshot;
}

describe("client settings compatibility", () => {
  it("strips migration metadata from the Rust-owned client document", () => {
    const legacy = createLegacySnapshot();
    legacy.i18nextLng = '"zh"';
    legacy.micOn = "true";
    const preferences = migrateLegacyClientPreferences(legacy, {});

    const settings = clientPreferencesToSettings(preferences);

    expect(settings.appearance.locale).toBe("zh");
    expect(settings).not.toHaveProperty("schemaVersion");
    expect(settings).not.toHaveProperty("legacy");
    expect(settings).not.toHaveProperty("micOn");
  });

  it("updates formal settings while preserving the immutable legacy envelope", () => {
    const legacy = createLegacySnapshot();
    legacy.modelInfo = '{"name":"legacy-model","url":"/model.json"}';
    const preferences = migrateLegacyClientPreferences(legacy, {});
    const settings = clientPreferencesToSettings(preferences);
    settings.appearance.locale = "ja";
    settings.connectionOverride = {
      wsUrl: "wss://remote.example/client-ws",
      baseUrl: "https://remote.example",
    };

    const merged = mergeClientSettingsIntoPreferences(preferences, settings);
    settings.appearance.locale = "changed-after-merge";
    legacy.modelInfo = "changed-after-merge";

    expect(merged.appearance.locale).toBe("ja");
    expect(merged.connectionOverride).toEqual({
      wsUrl: "wss://remote.example/client-ws",
      baseUrl: "https://remote.example",
    });
    expect(merged.legacy.modelInfo).toEqual({
      name: "legacy-model",
      url: "/model.json",
    });
    expect(merged.legacy.raw.modelInfo).toBe(
      '{"name":"legacy-model","url":"/model.json"}',
    );
  });
});
