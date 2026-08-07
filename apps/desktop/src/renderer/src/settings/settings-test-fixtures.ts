import {
  SETTINGS_SCHEMA_VERSION,
  type ClientSettingsV1,
  type ProviderPatchV1,
  type SettingsSnapshotV1,
} from "./generated/settings-v1.generated";

export function createClientSettings(): ClientSettingsV1 {
  return {
    appearance: { locale: "en", backgroundUrl: null },
    media: { imageCompressionQuality: 0.8, imageMaxWidth: 0 },
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
    behavior: {
      proactiveSpeak: {
        allowButtonTrigger: false,
        allowProactiveSpeak: false,
        idleSecondsToSpeak: 5,
      },
    },
    live2d: { pointerInteractive: null, scrollToResize: null },
    connectionOverride: null,
  };
}

export function createSettingsSnapshot(revision = 0): SettingsSnapshotV1 {
  return {
    schemaVersion: SETTINGS_SCHEMA_VERSION,
    revision,
    client: createClientSettings(),
    provider: {
      kind: "none",
      baseUrl: null,
      model: null,
      apiKey: { configured: false, hint: null },
    },
  };
}

export function createProviderPatch(
  overrides: Partial<ProviderPatchV1> = {},
): ProviderPatchV1 {
  return {
    kind: "none",
    baseUrl: null,
    model: null,
    apiKey: null,
    ...overrides,
  };
}
