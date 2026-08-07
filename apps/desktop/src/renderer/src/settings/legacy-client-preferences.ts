export const CLIENT_PREFERENCES_SCHEMA_VERSION = 1 as const;
export const CLIENT_PREFERENCES_STORAGE_KEY = "olv.clientPreferences.v1";
export const LEGACY_SETTINGS_BACKUP_KEY = "olv.legacy.backup.v0";

export const LEGACY_STORAGE_KEYS = [
  "wsUrl",
  "baseUrl",
  "backgroundUrl",
  "modelInfo",
  "micOn",
  "autoStopMic",
  "vadSettings",
  "autoStartMicOn",
  "autoStartMicOnConvEnd",
  "proactiveSpeakSettings",
  "appImageCompressionQuality",
  "appImageMaxWidth",
  "i18nextLng",
] as const;

type LegacyStorageKey = (typeof LEGACY_STORAGE_KEYS)[number];

export type LegacySettingsSnapshot = Record<LegacyStorageKey, string | null>;

export interface ClientPreferencesV1 {
  schemaVersion: typeof CLIENT_PREFERENCES_SCHEMA_VERSION;
  appearance: {
    locale: string;
    backgroundUrl: string | null;
  };
  media: {
    imageCompressionQuality: number;
    imageMaxWidth: number;
  };
  voice: {
    autoStopMic: boolean;
    autoStartMicOnAiSpeech: boolean;
    autoStartMicOnConversationEnd: boolean;
    vad: {
      positiveSpeechThreshold: number;
      negativeSpeechThreshold: number;
      redemptionFrames: number;
    };
  };
  behavior: {
    proactiveSpeak: {
      allowButtonTrigger: boolean;
      allowProactiveSpeak: boolean;
      idleSecondsToSpeak: number;
    };
  };
  live2d: {
    pointerInteractive: boolean | null;
    scrollToResize: boolean | null;
  };
  connectionOverride: {
    wsUrl: string | null;
    baseUrl: string | null;
  } | null;
  legacy: {
    previousMicOn: boolean | null;
    modelInfo: unknown;
    raw: LegacySettingsSnapshot;
  };
}

export interface LegacyMigrationOptions {
  defaultLocale?: string;
  sameOriginBaseUrl?: string | null;
}

const DEFAULT_VAD_SETTINGS = {
  positiveSpeechThreshold: 50,
  negativeSpeechThreshold: 35,
  redemptionFrames: 35,
};

const DEFAULT_PROACTIVE_SPEAK_SETTINGS = {
  allowButtonTrigger: false,
  allowProactiveSpeak: false,
  idleSecondsToSpeak: 5,
};

const LEGACY_LOCAL_HOSTS = new Set(["127.0.0.1", "localhost"]);

export function collectLegacySettings(
  storage: Pick<Storage, "getItem">,
): LegacySettingsSnapshot {
  return Object.fromEntries(
    LEGACY_STORAGE_KEYS.map((key) => [key, storage.getItem(key)]),
  ) as LegacySettingsSnapshot;
}

export function migrateLegacyClientPreferences(
  snapshot: LegacySettingsSnapshot,
  options: LegacyMigrationOptions,
): ClientPreferencesV1 {
  const modelInfo = asRecord(parseLegacyValue(snapshot.modelInfo));
  const vadSettings = asRecord(parseLegacyValue(snapshot.vadSettings));
  const proactiveSpeakSettings = asRecord(
    parseLegacyValue(snapshot.proactiveSpeakSettings),
  );
  const wsUrl = asNonEmptyString(parseLegacyValue(snapshot.wsUrl));
  const baseUrl = asNonEmptyString(parseLegacyValue(snapshot.baseUrl));
  const customWsUrl = wsUrl && !isLegacyDefaultWsUrl(wsUrl) ? wsUrl : null;
  const customBaseUrl =
    baseUrl && !isLegacyDefaultBaseUrl(baseUrl) ? baseUrl : null;

  return {
    schemaVersion: CLIENT_PREFERENCES_SCHEMA_VERSION,
    appearance: {
      locale:
        asNonEmptyString(parseLegacyValue(snapshot.i18nextLng)) ??
        options.defaultLocale ??
        "en",
      backgroundUrl: normalizeBackgroundUrl(
        asNonEmptyString(parseLegacyValue(snapshot.backgroundUrl)),
        options.sameOriginBaseUrl,
      ),
    },
    media: {
      imageCompressionQuality: numberInRange(
        parseLegacyValue(snapshot.appImageCompressionQuality),
        0.1,
        1,
        0.8,
      ),
      imageMaxWidth: nonNegativeInteger(
        parseLegacyValue(snapshot.appImageMaxWidth),
        0,
      ),
    },
    voice: {
      autoStopMic: booleanValue(parseLegacyValue(snapshot.autoStopMic), false),
      autoStartMicOnAiSpeech: booleanValue(
        parseLegacyValue(snapshot.autoStartMicOn),
        false,
      ),
      autoStartMicOnConversationEnd: booleanValue(
        parseLegacyValue(snapshot.autoStartMicOnConvEnd),
        false,
      ),
      vad: {
        positiveSpeechThreshold: numberInRange(
          vadSettings?.positiveSpeechThreshold,
          0,
          100,
          DEFAULT_VAD_SETTINGS.positiveSpeechThreshold,
        ),
        negativeSpeechThreshold: numberInRange(
          vadSettings?.negativeSpeechThreshold,
          0,
          100,
          DEFAULT_VAD_SETTINGS.negativeSpeechThreshold,
        ),
        redemptionFrames: positiveInteger(
          vadSettings?.redemptionFrames,
          DEFAULT_VAD_SETTINGS.redemptionFrames,
        ),
      },
    },
    behavior: {
      proactiveSpeak: {
        allowButtonTrigger: booleanValue(
          proactiveSpeakSettings?.allowButtonTrigger,
          DEFAULT_PROACTIVE_SPEAK_SETTINGS.allowButtonTrigger,
        ),
        allowProactiveSpeak: booleanValue(
          proactiveSpeakSettings?.allowProactiveSpeak,
          DEFAULT_PROACTIVE_SPEAK_SETTINGS.allowProactiveSpeak,
        ),
        idleSecondsToSpeak: positiveNumber(
          proactiveSpeakSettings?.idleSecondsToSpeak,
          DEFAULT_PROACTIVE_SPEAK_SETTINGS.idleSecondsToSpeak,
        ),
      },
    },
    live2d: {
      pointerInteractive: optionalBoolean(modelInfo?.pointerInteractive),
      scrollToResize: optionalBoolean(modelInfo?.scrollToResize),
    },
    connectionOverride:
      customWsUrl || customBaseUrl
        ? { wsUrl: customWsUrl, baseUrl: customBaseUrl }
        : null,
    legacy: {
      previousMicOn: optionalBoolean(parseLegacyValue(snapshot.micOn)),
      modelInfo: modelInfo ?? null,
      raw: { ...snapshot },
    },
  };
}

function parseLegacyValue(value: string | null): unknown {
  if (value === null) return null;

  try {
    return JSON.parse(value);
  } catch {
    return value;
  }
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function asNonEmptyString(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value : null;
}

function booleanValue(value: unknown, fallback: boolean): boolean {
  return typeof value === "boolean" ? value : fallback;
}

function optionalBoolean(value: unknown): boolean | null {
  return typeof value === "boolean" ? value : null;
}

function finiteNumber(value: unknown): number | null {
  const parsed = typeof value === "number" ? value : Number(value);
  return Number.isFinite(parsed) ? parsed : null;
}

function numberInRange(
  value: unknown,
  minimum: number,
  maximum: number,
  fallback: number,
): number {
  const parsed = finiteNumber(value);
  return parsed !== null && parsed >= minimum && parsed <= maximum
    ? parsed
    : fallback;
}

function nonNegativeInteger(value: unknown, fallback: number): number {
  const parsed = finiteNumber(value);
  return parsed !== null && Number.isInteger(parsed) && parsed >= 0
    ? parsed
    : fallback;
}

function positiveInteger(value: unknown, fallback: number): number {
  const parsed = finiteNumber(value);
  return parsed !== null && Number.isInteger(parsed) && parsed > 0
    ? parsed
    : fallback;
}

function positiveNumber(value: unknown, fallback: number): number {
  const parsed = finiteNumber(value);
  return parsed !== null && parsed > 0 ? parsed : fallback;
}

function isLegacyDefaultWsUrl(value: string): boolean {
  const url = parseUrl(value);
  return Boolean(
    url &&
      url.protocol === "ws:" &&
      LEGACY_LOCAL_HOSTS.has(url.hostname) &&
      url.port === "12393" &&
      url.pathname === "/client-ws" &&
      !url.search &&
      !url.hash,
  );
}

function isLegacyDefaultBaseUrl(value: string): boolean {
  const url = parseUrl(value);
  return Boolean(
    url &&
      url.protocol === "http:" &&
      LEGACY_LOCAL_HOSTS.has(url.hostname) &&
      url.port === "12393" &&
      (url.pathname === "/" || url.pathname === "") &&
      !url.search &&
      !url.hash,
  );
}

function normalizeBackgroundUrl(
  value: string | null,
  sameOriginBaseUrl: string | null | undefined,
): string | null {
  if (!value) return null;

  const url = parseUrl(value);
  if (
    !url ||
    !sameOriginBaseUrl ||
    url.protocol !== "http:" ||
    !LEGACY_LOCAL_HOSTS.has(url.hostname) ||
    url.port !== "12393"
  ) {
    return value;
  }

  return `${sameOriginBaseUrl.replace(/\/$/, "")}${url.pathname}${url.search}${url.hash}`;
}

function parseUrl(value: string): URL | null {
  try {
    return new URL(value);
  } catch {
    return null;
  }
}
