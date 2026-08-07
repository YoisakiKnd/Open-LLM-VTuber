import {
  CLIENT_PREFERENCES_SCHEMA_VERSION,
  CLIENT_PREFERENCES_STORAGE_KEY,
  LEGACY_SETTINGS_BACKUP_KEY,
  LEGACY_STORAGE_KEYS,
  collectLegacySettings,
  migrateLegacyClientPreferences,
  type ClientPreferencesV1,
  type LegacyMigrationOptions,
  type LegacySettingsSnapshot,
} from "./legacy-client-preferences";

export const CLIENT_PREFERENCES_FEATURE_FLAG_KEY =
  "olv.features.clientPreferencesV1";

export interface SettingsStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

export interface LegacySettingsBackupV0 {
  schemaVersion: 0;
  values: LegacySettingsSnapshot;
}

export interface ClientPreferencesRepositoryOptions
  extends LegacyMigrationOptions {
  enabledByDefault?: boolean;
}

export type ClientPreferencesInitialization =
  | { status: "disabled" }
  | {
      status: "ready";
      source: "current" | "migrated";
      preferences: ClientPreferencesV1;
    }
  | {
      status: "blocked";
      reason: "invalid-current" | "invalid-backup" | "unsupported-schema";
      storedSchemaVersion?: number;
    };

export class ClientPreferencesRepository {
  constructor(
    private readonly storage: SettingsStorage,
    private readonly options: ClientPreferencesRepositoryOptions,
  ) {}

  initialize(): ClientPreferencesInitialization {
    if (!this.isEnabled()) return { status: "disabled" };

    const current = this.storage.getItem(CLIENT_PREFERENCES_STORAGE_KEY);
    if (current !== null) return decodeCurrentPreferences(current);

    const backup = this.loadOrCreateBackup();
    if (backup.status === "blocked") return backup;

    const preferences = migrateLegacyClientPreferences(
      backup.backup.values,
      this.options,
    );
    this.storage.setItem(
      CLIENT_PREFERENCES_STORAGE_KEY,
      JSON.stringify(preferences),
    );

    return { status: "ready", source: "migrated", preferences };
  }

  private isEnabled(): boolean {
    const flag = this.storage.getItem(CLIENT_PREFERENCES_FEATURE_FLAG_KEY);
    if (flag === null) return this.options.enabledByDefault ?? false;
    return parseFeatureFlag(flag) ?? false;
  }

  private loadOrCreateBackup():
    | { status: "ready"; backup: LegacySettingsBackupV0 }
    | {
        status: "blocked";
        reason: "invalid-backup";
      } {
    const storedBackup = this.storage.getItem(LEGACY_SETTINGS_BACKUP_KEY);
    if (storedBackup !== null) {
      const backup = decodeLegacyBackup(storedBackup);
      return backup
        ? { status: "ready", backup }
        : { status: "blocked", reason: "invalid-backup" };
    }

    const backup: LegacySettingsBackupV0 = {
      schemaVersion: 0,
      values: collectLegacySettings(this.storage),
    };
    this.storage.setItem(LEGACY_SETTINGS_BACKUP_KEY, JSON.stringify(backup));
    return { status: "ready", backup };
  }
}

function decodeCurrentPreferences(
  raw: string,
): ClientPreferencesInitialization {
  const parsed = parseJsonRecord(raw);
  if (!parsed) return { status: "blocked", reason: "invalid-current" };

  const storedSchemaVersion = parsed.schemaVersion;
  if (
    typeof storedSchemaVersion === "number" &&
    storedSchemaVersion > CLIENT_PREFERENCES_SCHEMA_VERSION
  ) {
    return {
      status: "blocked",
      reason: "unsupported-schema",
      storedSchemaVersion,
    };
  }

  if (!isClientPreferencesV1(parsed)) {
    return { status: "blocked", reason: "invalid-current" };
  }

  return {
    status: "ready",
    source: "current",
    preferences: parsed,
  };
}

function decodeLegacyBackup(raw: string): LegacySettingsBackupV0 | null {
  const parsed = parseJsonRecord(raw);
  if (
    !parsed ||
    parsed.schemaVersion !== 0 ||
    !isLegacySettingsSnapshot(parsed.values)
  ) {
    return null;
  }

  return {
    schemaVersion: 0,
    values: parsed.values,
  };
}

function parseFeatureFlag(raw: string): boolean | null {
  const normalized = raw.trim().toLowerCase();
  if (normalized === "true" || normalized === "1" || normalized === '"true"') {
    return true;
  }
  if (
    normalized === "false" ||
    normalized === "0" ||
    normalized === '"false"'
  ) {
    return false;
  }
  return null;
}

function parseJsonRecord(raw: string): Record<string, unknown> | null {
  try {
    const value: unknown = JSON.parse(raw);
    return isRecord(value) ? value : null;
  } catch {
    return null;
  }
}

function isClientPreferencesV1(
  value: Record<string, unknown>,
): value is Record<string, unknown> & ClientPreferencesV1 {
  const appearance = asRecord(value.appearance);
  const media = asRecord(value.media);
  const voice = asRecord(value.voice);
  const vad = asRecord(voice?.vad);
  const behavior = asRecord(value.behavior);
  const proactiveSpeak = asRecord(behavior?.proactiveSpeak);
  const live2d = asRecord(value.live2d);
  const connectionOverride =
    value.connectionOverride === null
      ? null
      : asRecord(value.connectionOverride);
  const legacy = asRecord(value.legacy);

  return Boolean(
    value.schemaVersion === CLIENT_PREFERENCES_SCHEMA_VERSION &&
      appearance &&
      isNonEmptyString(appearance.locale) &&
      isOptionalString(appearance.backgroundUrl) &&
      media &&
      isNumberInRange(media.imageCompressionQuality, 0.1, 1) &&
      isNonNegativeInteger(media.imageMaxWidth) &&
      voice &&
      typeof voice.autoStopMic === "boolean" &&
      typeof voice.autoStartMicOnAiSpeech === "boolean" &&
      typeof voice.autoStartMicOnConversationEnd === "boolean" &&
      vad &&
      isNumberInRange(vad.positiveSpeechThreshold, 0, 100) &&
      isNumberInRange(vad.negativeSpeechThreshold, 0, 100) &&
      isPositiveInteger(vad.redemptionFrames) &&
      behavior &&
      proactiveSpeak &&
      typeof proactiveSpeak.allowButtonTrigger === "boolean" &&
      typeof proactiveSpeak.allowProactiveSpeak === "boolean" &&
      isPositiveNumber(proactiveSpeak.idleSecondsToSpeak) &&
      live2d &&
      isOptionalBoolean(live2d.pointerInteractive) &&
      isOptionalBoolean(live2d.scrollToResize) &&
      isConnectionOverride(connectionOverride) &&
      legacy &&
      isOptionalBoolean(legacy.previousMicOn) &&
      isLegacySettingsSnapshot(legacy.raw),
  );
}

function isLegacySettingsSnapshot(
  value: unknown,
): value is LegacySettingsSnapshot {
  const record = asRecord(value);
  return Boolean(
    record &&
      LEGACY_STORAGE_KEYS.every(
        (key) => record[key] === null || typeof record[key] === "string",
      ),
  );
}

function isConnectionOverride(value: Record<string, unknown> | null): boolean {
  if (value === null) return true;
  return (
    isOptionalNonEmptyString(value.wsUrl) &&
    isOptionalNonEmptyString(value.baseUrl) &&
    (value.wsUrl !== null || value.baseUrl !== null)
  );
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return isRecord(value) ? value : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function isNonEmptyString(value: unknown): boolean {
  return typeof value === "string" && value.trim().length > 0;
}

function isOptionalString(value: unknown): boolean {
  return value === null || typeof value === "string";
}

function isOptionalNonEmptyString(value: unknown): boolean {
  return value === null || isNonEmptyString(value);
}

function isOptionalBoolean(value: unknown): boolean {
  return value === null || typeof value === "boolean";
}

function isNumberInRange(
  value: unknown,
  minimum: number,
  maximum: number,
): boolean {
  return (
    typeof value === "number" &&
    Number.isFinite(value) &&
    value >= minimum &&
    value <= maximum
  );
}

function isNonNegativeInteger(value: unknown): boolean {
  return typeof value === "number" && Number.isInteger(value) && value >= 0;
}

function isPositiveInteger(value: unknown): boolean {
  return typeof value === "number" && Number.isInteger(value) && value > 0;
}

function isPositiveNumber(value: unknown): boolean {
  return typeof value === "number" && Number.isFinite(value) && value > 0;
}
