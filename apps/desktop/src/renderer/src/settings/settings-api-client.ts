import {
  SETTINGS_SCHEMA_VERSION,
  type ApplyEffect,
  type ClientSettingsV1,
  type SettingOwner,
  type SettingsApplyResponse,
  type SettingsPatchRequestV1,
  type SettingsSchemaResponse,
  type SettingsSnapshotV1,
  type SettingsValidationError,
  type SettingsValidationResponse,
} from "./generated/settings-v1.generated";

const SETTINGS_ENDPOINT = "/api/v1/settings";
const SETTINGS_SCHEMA_ENDPOINT = "/api/v1/settings/schema";
const SETTINGS_SNAPSHOT_ENDPOINT = "/api/v1/settings/snapshot";
const SETTINGS_VALIDATE_ENDPOINT = "/api/v1/settings/validate";

export type SettingsFetch = (
  input: string,
  init?: RequestInit,
) => Promise<Response>;

export interface SettingsApi {
  getSchema(signal?: AbortSignal): Promise<SettingsSchemaResponse>;
  getSnapshot(signal?: AbortSignal): Promise<SettingsSnapshotV1>;
  validate(
    snapshot: SettingsSnapshotV1,
    signal?: AbortSignal,
  ): Promise<SettingsValidationResponse>;
  apply(
    request: SettingsPatchRequestV1,
    signal?: AbortSignal,
  ): Promise<SettingsApplyResponse>;
}

export class SettingsApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly code: string,
  ) {
    super(message);
    this.name = "SettingsApiError";
  }
}

export class SettingsRevisionConflictError extends SettingsApiError {
  constructor(
    readonly snapshot: SettingsSnapshotV1,
    message: string,
  ) {
    super(message, 409, "revision_conflict");
    this.name = "SettingsRevisionConflictError";
  }
}

export class SettingsValidationFailedError extends SettingsApiError {
  constructor(
    readonly errors: SettingsValidationError[],
    message: string,
  ) {
    super(message, 422, "validation_failed");
    this.name = "SettingsValidationFailedError";
  }
}

export class SettingsApiClient implements SettingsApi {
  private readonly baseUrl: string;

  constructor(
    baseUrl = "",
    private readonly fetchImpl: SettingsFetch = globalThis.fetch.bind(
      globalThis,
    ),
  ) {
    this.baseUrl = baseUrl.replace(/\/+$/, "");
  }

  async getSchema(signal?: AbortSignal): Promise<SettingsSchemaResponse> {
    const payload = await this.request<unknown>(SETTINGS_SCHEMA_ENDPOINT, {
      method: "GET",
      signal,
    });
    if (!isSchemaResponse(payload)) {
      throw protocolError("settings schema response is malformed");
    }
    return payload;
  }

  async getSnapshot(signal?: AbortSignal): Promise<SettingsSnapshotV1> {
    const payload = await this.request<unknown>(SETTINGS_SNAPSHOT_ENDPOINT, {
      method: "GET",
      signal,
    });
    if (!isSettingsSnapshot(payload)) {
      throw protocolError("settings snapshot response is malformed");
    }
    return payload;
  }

  async validate(
    snapshot: SettingsSnapshotV1,
    signal?: AbortSignal,
  ): Promise<SettingsValidationResponse> {
    const payload = await this.request<unknown>(SETTINGS_VALIDATE_ENDPOINT, {
      method: "POST",
      signal,
      body: JSON.stringify(snapshot),
    });
    if (!isValidationResponse(payload)) {
      throw protocolError("settings validation response is malformed");
    }
    return payload;
  }

  async apply(
    request: SettingsPatchRequestV1,
    signal?: AbortSignal,
  ): Promise<SettingsApplyResponse> {
    const payload = await this.request<unknown>(SETTINGS_ENDPOINT, {
      method: "PATCH",
      signal,
      body: JSON.stringify(request),
    });
    if (!isApplyResponse(payload)) {
      throw protocolError("settings apply response is malformed");
    }
    return payload;
  }

  private async request<T>(path: string, init: RequestInit): Promise<T> {
    const response = await this.fetchImpl(`${this.baseUrl}${path}`, {
      ...init,
      cache: "no-store",
      credentials: "same-origin",
      headers: {
        Accept: "application/json",
        ...(init.body === undefined
          ? {}
          : { "Content-Type": "application/json" }),
      },
    });
    const payload = await readJson(response);
    if (!response.ok) throwApiError(response.status, payload);
    return payload as T;
  }
}

async function readJson(response: Response): Promise<unknown> {
  try {
    return await response.json();
  } catch {
    throw protocolError(
      `settings API returned non-JSON status ${response.status}`,
    );
  }
}

function throwApiError(status: number, payload: unknown): never {
  const record = asRecord(payload);
  const error = asRecord(record?.error);
  const code = asString(error?.code) ?? "request_failed";
  const message =
    asString(error?.message) ?? `settings request failed (${status})`;

  if (status === 409 && code === "revision_conflict") {
    const snapshot = record?.snapshot;
    if (!isSettingsSnapshot(snapshot)) {
      throw protocolError("revision conflict response is missing its snapshot");
    }
    throw new SettingsRevisionConflictError(snapshot, message);
  }

  if (status === 422 && code === "validation_failed") {
    const errors = record?.errors;
    if (!isValidationErrors(errors)) {
      throw protocolError("validation failure response is missing its errors");
    }
    throw new SettingsValidationFailedError(errors, message);
  }

  throw new SettingsApiError(message, status, code);
}

function protocolError(message: string): SettingsApiError {
  return new SettingsApiError(message, 0, "invalid_response");
}

function isSchemaResponse(value: unknown): value is SettingsSchemaResponse {
  const record = asRecord(value);
  return Boolean(
    record &&
      record.schemaVersion === SETTINGS_SCHEMA_VERSION &&
      Array.isArray(record.owners) &&
      record.owners.length === 5 &&
      record.owners.every(isSettingOwner) &&
      Array.isArray(record.applyEffects) &&
      record.applyEffects.length === 4 &&
      record.applyEffects.every(isApplyEffect) &&
      Array.isArray(record.fields) &&
      record.fields.every((field) => {
        const policy = asRecord(field);
        return Boolean(
          policy &&
            typeof policy.path === "string" &&
            isSettingOwner(policy.owner) &&
            isApplyEffect(policy.applyEffect) &&
            typeof policy.secret === "boolean",
        );
      }) &&
      "schema" in record &&
      "patchSchema" in record &&
      "patchResponseSchema" in record,
  );
}

function isClientSettings(value: unknown): value is ClientSettingsV1 {
  const client = asRecord(value);
  const appearance = asRecord(client?.appearance);
  const media = asRecord(client?.media);
  const voice = asRecord(client?.voice);
  const vad = asRecord(voice?.vad);
  const behavior = asRecord(client?.behavior);
  const proactiveSpeak = asRecord(behavior?.proactiveSpeak);
  const live2d = asRecord(client?.live2d);
  const connection =
    client?.connectionOverride === null
      ? null
      : asRecord(client?.connectionOverride);

  return Boolean(
    client &&
      appearance &&
      typeof appearance.locale === "string" &&
      isStringOrNull(appearance.backgroundUrl) &&
      media &&
      isFiniteNumber(media.imageCompressionQuality) &&
      isSafeInteger(media.imageMaxWidth) &&
      voice &&
      typeof voice.autoStopMic === "boolean" &&
      typeof voice.autoStartMicOnAiSpeech === "boolean" &&
      typeof voice.autoStartMicOnConversationEnd === "boolean" &&
      vad &&
      isFiniteNumber(vad.positiveSpeechThreshold) &&
      isFiniteNumber(vad.negativeSpeechThreshold) &&
      isSafeInteger(vad.redemptionFrames) &&
      behavior &&
      proactiveSpeak &&
      typeof proactiveSpeak.allowButtonTrigger === "boolean" &&
      typeof proactiveSpeak.allowProactiveSpeak === "boolean" &&
      isFiniteNumber(proactiveSpeak.idleSecondsToSpeak) &&
      live2d &&
      isBooleanOrNull(live2d.pointerInteractive) &&
      isBooleanOrNull(live2d.scrollToResize) &&
      (client.connectionOverride === null ||
        Boolean(
          connection &&
            isStringOrNull(connection.wsUrl) &&
            isStringOrNull(connection.baseUrl),
        )),
  );
}

function isSettingsSnapshot(value: unknown): value is SettingsSnapshotV1 {
  const record = asRecord(value);
  return Boolean(
    record &&
      record.schemaVersion === SETTINGS_SCHEMA_VERSION &&
      typeof record.revision === "number" &&
      Number.isSafeInteger(record.revision) &&
      record.revision >= 0 &&
      isClientSettings(record.client),
  );
}

function isApplyResponse(value: unknown): value is SettingsApplyResponse {
  const record = asRecord(value);
  return Boolean(
    record &&
      isSettingsSnapshot(record.snapshot) &&
      isStringArray(record.changedPaths) &&
      isApplyEffectArray(record.applyEffects),
  );
}

function isValidationResponse(
  value: unknown,
): value is SettingsValidationResponse {
  const record = asRecord(value);
  return Boolean(
    record &&
      typeof record.valid === "boolean" &&
      isValidationErrors(record.errors),
  );
}

function isValidationErrors(
  value: unknown,
): value is SettingsValidationError[] {
  return (
    Array.isArray(value) &&
    value.every((item) => {
      const record = asRecord(item);
      return Boolean(
        record &&
          typeof record.path === "string" &&
          typeof record.code === "string" &&
          typeof record.message === "string",
      );
    })
  );
}

function isApplyEffectArray(value: unknown): value is ApplyEffect[] {
  return Array.isArray(value) && value.every(isApplyEffect);
}

function isApplyEffect(value: unknown): value is ApplyEffect {
  return (
    value === "preview" ||
    value === "live" ||
    value === "reconnect" ||
    value === "restart"
  );
}

function isSettingOwner(value: unknown): value is SettingOwner {
  return (
    value === "client" ||
    value === "desktop" ||
    value === "runtime" ||
    value === "character" ||
    value === "session"
  );
}

function isStringArray(value: unknown): value is string[] {
  return (
    Array.isArray(value) && value.every((item) => typeof item === "string")
  );
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function asString(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

function isStringOrNull(value: unknown): value is string | null {
  return value === null || typeof value === "string";
}

function isBooleanOrNull(value: unknown): value is boolean | null {
  return value === null || typeof value === "boolean";
}

function isFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

function isSafeInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value);
}
