import { describe, expect, it } from "vitest";
import {
  LEGACY_STORAGE_KEYS,
  collectLegacySettings,
  migrateLegacyClientPreferences,
  type LegacySettingsSnapshot,
} from "./legacy-client-preferences";

function emptySnapshot(
  overrides: Partial<LegacySettingsSnapshot> = {},
): LegacySettingsSnapshot {
  return {
    ...Object.fromEntries(LEGACY_STORAGE_KEYS.map((key) => [key, null])),
    ...overrides,
  } as LegacySettingsSnapshot;
}

describe("legacy client preference migration", () => {
  it("uses safe defaults without restoring microphone runtime state", () => {
    const preferences = migrateLegacyClientPreferences(emptySnapshot(), {
      sameOriginBaseUrl: "http://127.0.0.1:12394",
    });

    expect(preferences).toMatchObject({
      schemaVersion: 1,
      appearance: {
        locale: "en",
        backgroundUrl: null,
      },
      media: {
        imageCompressionQuality: 0.8,
        imageMaxWidth: 0,
      },
      voice: {
        autoStopMic: false,
        autoStartMicOnAiSpeech: false,
        autoStartMicOnConversationEnd: false,
        vad: {
          positiveSpeechThreshold: 50,
          negativeSpeechThreshold: 35,
          redemptionFrames: 35,
        },
      },
      connectionOverride: null,
      legacy: {
        previousMicOn: null,
        modelInfo: null,
      },
    });
  });

  it("migrates JSON values and retains the complete legacy model snapshot", () => {
    const snapshot = emptySnapshot({
      i18nextLng: '"zh"',
      autoStopMic: "true",
      autoStartMicOn: "true",
      autoStartMicOnConvEnd: "false",
      micOn: "true",
      vadSettings: JSON.stringify({
        positiveSpeechThreshold: 65,
        negativeSpeechThreshold: 30,
        redemptionFrames: 20,
      }),
      proactiveSpeakSettings: JSON.stringify({
        allowButtonTrigger: true,
        allowProactiveSpeak: true,
        idleSecondsToSpeak: 12,
      }),
      modelInfo: JSON.stringify({
        url: "",
        kScale: 1.2,
        pointerInteractive: false,
        scrollToResize: true,
      }),
    });

    const preferences = migrateLegacyClientPreferences(snapshot, {
      sameOriginBaseUrl: "http://127.0.0.1:12394",
    });

    expect(preferences.appearance.locale).toBe("zh");
    expect(preferences.voice).toEqual({
      autoStopMic: true,
      autoStartMicOnAiSpeech: true,
      autoStartMicOnConversationEnd: false,
      vad: {
        positiveSpeechThreshold: 65,
        negativeSpeechThreshold: 30,
        redemptionFrames: 20,
      },
    });
    expect(preferences.behavior.proactiveSpeak).toEqual({
      allowButtonTrigger: true,
      allowProactiveSpeak: true,
      idleSecondsToSpeak: 12,
    });
    expect(preferences.live2d).toEqual({
      pointerInteractive: false,
      scrollToResize: true,
    });
    expect(preferences.legacy.previousMicOn).toBe(true);
    expect(preferences.legacy.modelInfo).toEqual({
      url: "",
      kScale: 1.2,
      pointerInteractive: false,
      scrollToResize: true,
    });
    expect(preferences.legacy.raw).toEqual(snapshot);
  });

  it("moves only legacy default endpoints to runtime-managed same-origin values", () => {
    const preferences = migrateLegacyClientPreferences(
      emptySnapshot({
        wsUrl: '"ws://localhost:12393/client-ws"',
        baseUrl: '"http://127.0.0.1:12393"',
        backgroundUrl: '"http://localhost:12393/bg/room.jpeg?variant=night"',
      }),
      {
        sameOriginBaseUrl: "https://vtuber.example",
      },
    );

    expect(preferences.connectionOverride).toBeNull();
    expect(preferences.appearance.backgroundUrl).toBe(
      "https://vtuber.example/bg/room.jpeg?variant=night",
    );
  });

  it("preserves custom remote endpoints and backgrounds exactly", () => {
    const preferences = migrateLegacyClientPreferences(
      emptySnapshot({
        wsUrl: '"wss://remote.example/realtime"',
        baseUrl: '"https://remote.example/api"',
        backgroundUrl: '"https://cdn.example/background.png"',
      }),
      {
        sameOriginBaseUrl: "http://127.0.0.1:12394",
      },
    );

    expect(preferences.connectionOverride).toEqual({
      wsUrl: "wss://remote.example/realtime",
      baseUrl: "https://remote.example/api",
    });
    expect(preferences.appearance.backgroundUrl).toBe(
      "https://cdn.example/background.png",
    );
  });

  it("accepts raw legacy strings and rejects invalid numeric settings", () => {
    const preferences = migrateLegacyClientPreferences(
      emptySnapshot({
        i18nextLng: "zh",
        appImageCompressionQuality: "4.2",
        appImageMaxWidth: "-100",
        vadSettings: "{invalid-json",
        proactiveSpeakSettings: JSON.stringify({ idleSecondsToSpeak: 0 }),
      }),
      {
        defaultLocale: "en",
        sameOriginBaseUrl: "http://127.0.0.1:12394",
      },
    );

    expect(preferences.appearance.locale).toBe("zh");
    expect(preferences.media).toEqual({
      imageCompressionQuality: 0.8,
      imageMaxWidth: 0,
    });
    expect(preferences.voice.vad).toEqual({
      positiveSpeechThreshold: 50,
      negativeSpeechThreshold: 35,
      redemptionFrames: 35,
    });
    expect(preferences.behavior.proactiveSpeak.idleSecondsToSpeak).toBe(5);
  });

  it("collects known values without reading unrelated browser storage", () => {
    const values = new Map<string, string>([
      ["wsUrl", '"wss://remote.example/client-ws"'],
      ["unrelatedSecret", "do-not-read"],
    ]);
    const requestedKeys: string[] = [];
    const storage = {
      getItem(key: string) {
        requestedKeys.push(key);
        return values.get(key) ?? null;
      },
    };

    const snapshot = collectLegacySettings(storage);

    expect(snapshot.wsUrl).toBe('"wss://remote.example/client-ws"');
    expect(requestedKeys).toEqual(LEGACY_STORAGE_KEYS);
    expect(requestedKeys).not.toContain("unrelatedSecret");
  });
});
