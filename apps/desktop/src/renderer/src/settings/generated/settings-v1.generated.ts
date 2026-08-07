// Generated from rust-gateway/src/settings.rs. Do not edit.

export const SETTINGS_SCHEMA_VERSION = 1 as const;
export const MAX_SETTINGS_REVISION = 9007199254740991 as const;

export type JsonValue = null | boolean | number | string | Array<JsonValue> | { [key: string]: JsonValue };

export type SettingOwner = "client" | "desktop" | "runtime" | "character" | "session";

export type ApplyEffect = "preview" | "live" | "reconnect" | "restart";

export type SettingFieldPolicy = { path: string, owner: SettingOwner, applyEffect: ApplyEffect, secret: boolean, };

export type SettingsSchemaResponse = { schemaVersion: number, owners: [SettingOwner, SettingOwner, SettingOwner, SettingOwner, SettingOwner], applyEffects: [ApplyEffect, ApplyEffect, ApplyEffect, ApplyEffect], fields: Array<SettingFieldPolicy>, schema: JsonValue, patchSchema: JsonValue, patchResponseSchema: JsonValue, };

export type AppearancePreferences = { locale: string, backgroundUrl: string | null, };

export type MediaPreferences = { imageCompressionQuality: number, imageMaxWidth: number, };

export type VadPreferences = { positiveSpeechThreshold: number, negativeSpeechThreshold: number, redemptionFrames: number, };

export type VoicePreferences = { autoStopMic: boolean, autoStartMicOnAiSpeech: boolean, autoStartMicOnConversationEnd: boolean, vad: VadPreferences, };

export type ProactiveSpeakPreferences = { allowButtonTrigger: boolean, allowProactiveSpeak: boolean, idleSecondsToSpeak: number, };

export type BehaviorPreferences = { proactiveSpeak: ProactiveSpeakPreferences, };

export type Live2dPreferences = { pointerInteractive: boolean | null, scrollToResize: boolean | null, };

export type ConnectionOverride = { wsUrl: string | null, baseUrl: string | null, };

export type ProviderKindSetting = "none" | "open_ai" | "anthropic" | "ollama";

export type SecretValue = { configured: boolean, 
/**
 * Masked hint, e.g. `sk-...abcd`.
 */
hint: string | null, };

export type ProviderSettingsV1 = { kind: ProviderKindSetting, baseUrl: string | null, model: string | null, apiKey: SecretValue, };

export type ProviderPatchV1 = { kind: ProviderKindSetting, baseUrl: string | null, model: string | null, apiKey: string | null, };

export type ClientSettingsV1 = { appearance: AppearancePreferences, media: MediaPreferences, voice: VoicePreferences, behavior: BehaviorPreferences, live2d: Live2dPreferences, connectionOverride: ConnectionOverride | null, };

export type SettingsSnapshotV1 = { schemaVersion: number, revision: number, client: ClientSettingsV1, provider: ProviderSettingsV1, };

export type SettingsPatchRequestV1 = { baseRevision: number, client: ClientSettingsV1, provider: ProviderPatchV1, };

export type SettingsApplyResponse = { snapshot: SettingsSnapshotV1, changedPaths: Array<string>, applyEffects: Array<ApplyEffect>, };

export type SettingsValidationError = { path: string, code: string, message: string, };

export type SettingsValidationResponse = { valid: boolean, errors: Array<SettingsValidationError>, };
