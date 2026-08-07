import { describe, expect, it } from "vitest";
import {
  mergeGeneralClientFields,
  readGeneralClientFields,
} from "./general-client-settings-bridge";
import { createClientSettings } from "./settings-test-fixtures";

const managedConnection = {
  backgroundUrl: "https://app.example/bg/default.jpeg",
  wsUrl: "wss://app.example/client-ws",
  baseUrl: "https://app.example",
};

describe("general client settings bridge", () => {
  it("reads Runtime-managed endpoints when no override exists", () => {
    const settings = createClientSettings();

    expect(readGeneralClientFields(settings, managedConnection)).toMatchObject({
      locale: "en",
      backgroundUrl: managedConnection.backgroundUrl,
      imageCompressionQuality: 0.8,
      imageMaxWidth: 0,
      wsUrl: managedConnection.wsUrl,
      baseUrl: managedConnection.baseUrl,
    });
  });

  it("preserves untouched Client settings while updating General fields", () => {
    const settings = createClientSettings();
    settings.voice.autoStopMic = true;

    const merged = mergeGeneralClientFields(
      settings,
      {
        locale: "zh",
        backgroundUrl: "https://cdn.example/room.jpeg",
        imageCompressionQuality: 0.6,
        imageMaxWidth: 1280,
        wsUrl: managedConnection.wsUrl,
        baseUrl: managedConnection.baseUrl,
      },
      managedConnection,
    );

    expect(merged).toMatchObject({
      appearance: {
        locale: "zh",
        backgroundUrl: "https://cdn.example/room.jpeg",
      },
      media: { imageCompressionQuality: 0.6, imageMaxWidth: 1280 },
      voice: { autoStopMic: true },
      connectionOverride: null,
    });
  });

  it("keeps a Runtime-managed background null when other fields change", () => {
    const settings = createClientSettings();
    const fields = readGeneralClientFields(settings, managedConnection);
    fields.imageMaxWidth = 1440;

    const merged = mergeGeneralClientFields(
      settings,
      fields,
      managedConnection,
    );

    expect(merged.appearance.backgroundUrl).toBeNull();
    expect(merged.media.imageMaxWidth).toBe(1440);
  });

  it("stores only endpoint values that differ from Runtime management", () => {
    const settings = createClientSettings();

    const merged = mergeGeneralClientFields(
      settings,
      {
        ...readGeneralClientFields(settings, managedConnection),
        wsUrl: "wss://remote.example/client-ws",
      },
      managedConnection,
    );

    expect(merged.connectionOverride).toEqual({
      wsUrl: "wss://remote.example/client-ws",
      baseUrl: null,
    });
    expect(readGeneralClientFields(merged, managedConnection)).toMatchObject({
      wsUrl: "wss://remote.example/client-ws",
      baseUrl: managedConnection.baseUrl,
    });
  });

  it("turns cleared or restored endpoints back into Runtime management", () => {
    const settings = createClientSettings();
    settings.connectionOverride = {
      wsUrl: "wss://remote.example/client-ws",
      baseUrl: "https://remote.example",
    };

    const merged = mergeGeneralClientFields(
      settings,
      {
        ...readGeneralClientFields(settings, managedConnection),
        wsUrl: managedConnection.wsUrl,
        baseUrl: "   ",
      },
      managedConnection,
    );

    expect(merged.connectionOverride).toBeNull();
  });
});
