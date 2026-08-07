import type { ClientSettingsV1 } from "./generated/settings-v1.generated";
import type { ClientPreferencesV1 } from "./legacy-client-preferences";

export function clientPreferencesToSettings(
  preferences: ClientPreferencesV1,
): ClientSettingsV1 {
  return {
    appearance: { ...preferences.appearance },
    media: { ...preferences.media },
    voice: {
      ...preferences.voice,
      vad: { ...preferences.voice.vad },
    },
    behavior: {
      proactiveSpeak: { ...preferences.behavior.proactiveSpeak },
    },
    live2d: { ...preferences.live2d },
    connectionOverride:
      preferences.connectionOverride === null
        ? null
        : { ...preferences.connectionOverride },
  };
}

export function mergeClientSettingsIntoPreferences(
  preferences: ClientPreferencesV1,
  settings: ClientSettingsV1,
): ClientPreferencesV1 {
  return {
    ...preferences,
    appearance: { ...settings.appearance },
    media: { ...settings.media },
    voice: {
      ...settings.voice,
      vad: { ...settings.voice.vad },
    },
    behavior: {
      proactiveSpeak: { ...settings.behavior.proactiveSpeak },
    },
    live2d: { ...settings.live2d },
    connectionOverride:
      settings.connectionOverride === null
        ? null
        : { ...settings.connectionOverride },
    legacy: {
      ...preferences.legacy,
      raw: { ...preferences.legacy.raw },
    },
  };
}
