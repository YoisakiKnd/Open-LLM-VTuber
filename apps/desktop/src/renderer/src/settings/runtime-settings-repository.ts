import {
  SETTINGS_SCHEMA_VERSION,
  type ClientSettingsV1,
  type ProviderPatchV1,
  type SettingsApplyResponse,
  type SettingsSnapshotV1,
  type SettingsValidationResponse,
} from "./generated/settings-v1.generated";
import {
  SettingsRevisionConflictError,
  type SettingsApi,
} from "./settings-api-client";

/**
 * Builds a provider patch that preserves the committed provider domain and
 * never touches the stored secret (`apiKey: null` keeps it).
 */
function providerPatchFromSnapshot(
  snapshot: SettingsSnapshotV1,
): ProviderPatchV1 {
  return {
    kind: snapshot.provider.kind,
    baseUrl: snapshot.provider.baseUrl,
    model: snapshot.provider.model,
    apiKey: null,
  };
}

export interface RuntimeSettingsReadyState {
  status: "ready";
  committed: SettingsSnapshotV1;
  draft: ClientSettingsV1;
  conflict: SettingsSnapshotV1 | null;
}

export type RuntimeSettingsState =
  | { status: "uninitialized" }
  | RuntimeSettingsReadyState;

export type RuntimeSettingsSaveResult =
  | { status: "saved"; response: SettingsApplyResponse }
  | {
      status: "conflict";
      current: SettingsSnapshotV1;
      draft: ClientSettingsV1;
    };

export type SettingsConflictResolution = "accept-server" | "keep-local";

export class RuntimeSettingsRepository {
  private state: RuntimeSettingsReadyState | null = null;
  private loadInFlight: Promise<RuntimeSettingsReadyState> | null = null;
  private saveInFlight: Promise<RuntimeSettingsSaveResult> | null = null;

  constructor(private readonly api: SettingsApi) {}

  load(
    signal?: AbortSignal,
    fallbackClientSettings?: ClientSettingsV1 | null,
  ): Promise<RuntimeSettingsReadyState> {
    if (this.loadInFlight !== null) return this.loadInFlight;

    const operation = this.performLoad(signal, fallbackClientSettings);

    this.loadInFlight = operation;
    const clear = () => {
      if (this.loadInFlight === operation) this.loadInFlight = null;
    };
    operation.then(clear, clear);
    return operation;
  }

  private async performLoad(
    signal?: AbortSignal,
    fallbackClientSettings?: ClientSettingsV1 | null,
  ): Promise<RuntimeSettingsReadyState> {
    const snapshot = await this.api.getSnapshot(signal);
    const effectiveSnapshot = await this.importFallbackIfNeeded(
      snapshot,
      fallbackClientSettings,
      signal,
    );
    this.state = {
      status: "ready",
      committed: cloneJson(effectiveSnapshot),
      draft: cloneJson(effectiveSnapshot.client),
      conflict: null,
    };
    return this.getReadyState();
  }

  private async importFallbackIfNeeded(
    snapshot: SettingsSnapshotV1,
    fallbackClientSettings: ClientSettingsV1 | null | undefined,
    signal?: AbortSignal,
  ): Promise<SettingsSnapshotV1> {
    if (
      snapshot.revision !== 0 ||
      !fallbackClientSettings ||
      jsonEquals(snapshot.client, fallbackClientSettings)
    ) {
      return snapshot;
    }

    const validation = await this.api.validate(
      {
        schemaVersion: SETTINGS_SCHEMA_VERSION,
        revision: snapshot.revision,
        client: cloneJson(fallbackClientSettings),
        provider: snapshot.provider,
      },
      signal,
    );
    if (!validation.valid) {
      throw new SettingsFallbackValidationError(validation);
    }

    try {
      const response = await this.api.apply(
        {
          baseRevision: snapshot.revision,
          client: cloneJson(fallbackClientSettings),
          provider: providerPatchFromSnapshot(snapshot),
        },
        signal,
      );
      return response.snapshot;
    } catch (error) {
      if (error instanceof SettingsRevisionConflictError) {
        return error.snapshot;
      }
      throw error;
    }
  }

  getState(): RuntimeSettingsState {
    return this.state === null
      ? { status: "uninitialized" }
      : cloneJson(this.state);
  }

  hasUnsavedChanges(): boolean {
    return Boolean(
      this.state &&
        (this.state.conflict !== null ||
          !jsonEquals(this.state.draft, this.state.committed.client)),
    );
  }

  replaceDraft(draft: ClientSettingsV1): RuntimeSettingsReadyState {
    const state = this.requireReady();
    state.draft = cloneJson(draft);
    return this.getReadyState();
  }

  cancel(): RuntimeSettingsReadyState {
    const state = this.requireReady();
    if (state.conflict !== null) {
      state.committed = cloneJson(state.conflict);
      state.conflict = null;
    }
    state.draft = cloneJson(state.committed.client);
    return this.getReadyState();
  }

  async validate(signal?: AbortSignal): Promise<SettingsValidationResponse> {
    const state = this.requireReady();
    return this.api.validate(
      {
        schemaVersion: SETTINGS_SCHEMA_VERSION,
        revision: state.committed.revision,
        client: cloneJson(state.draft),
        provider: state.committed.provider,
      },
      signal,
    );
  }

  save(signal?: AbortSignal): Promise<RuntimeSettingsSaveResult> {
    if (this.saveInFlight !== null) return this.saveInFlight;

    const operation = this.performSave(signal);
    this.saveInFlight = operation;
    const clear = () => {
      if (this.saveInFlight === operation) this.saveInFlight = null;
    };
    operation.then(clear, clear);
    return operation;
  }

  private async performSave(
    signal?: AbortSignal,
  ): Promise<RuntimeSettingsSaveResult> {
    const state = this.requireReady();
    if (state.conflict !== null) {
      return {
        status: "conflict",
        current: cloneJson(state.conflict),
        draft: cloneJson(state.draft),
      };
    }

    const submittedDraft = cloneJson(state.draft);
    try {
      const response = await this.api.apply(
        {
          baseRevision: state.committed.revision,
          client: submittedDraft,
          provider: providerPatchFromSnapshot(state.committed),
        },
        signal,
      );
      state.committed = cloneJson(response.snapshot);
      if (jsonEquals(state.draft, submittedDraft)) {
        state.draft = cloneJson(response.snapshot.client);
      }
      return { status: "saved", response: cloneJson(response) };
    } catch (error) {
      if (!(error instanceof SettingsRevisionConflictError)) throw error;
      state.conflict = cloneJson(error.snapshot);
      return {
        status: "conflict",
        current: cloneJson(error.snapshot),
        draft: cloneJson(state.draft),
      };
    }
  }

  /**
   * Applies a provider-domain patch while preserving the committed client
   * document and the stored secret (`apiKey: null` keeps it). On a revision
   * conflict the error is rethrown so the caller can surface it.
   */
  async applyProviderPatch(
    provider: ProviderPatchV1,
    signal?: AbortSignal,
  ): Promise<SettingsApplyResponse> {
    const state = this.requireReady();
    if (state.conflict !== null) {
      throw new Error(
        "provider settings cannot be saved while a revision conflict is pending",
      );
    }
    const response = await this.api.apply(
      {
        baseRevision: state.committed.revision,
        client: cloneJson(state.committed.client),
        provider,
      },
      signal,
    );
    state.committed = cloneJson(response.snapshot);
    return response;
  }

  resolveConflict(
    resolution: SettingsConflictResolution,
  ): RuntimeSettingsReadyState {
    const state = this.requireReady();
    if (state.conflict === null) {
      throw new Error("runtime settings have no revision conflict to resolve");
    }

    const serverSnapshot = cloneJson(state.conflict);
    state.committed = serverSnapshot;
    state.conflict = null;
    if (resolution === "accept-server") {
      state.draft = cloneJson(serverSnapshot.client);
    }
    return this.getReadyState();
  }

  private requireReady(): RuntimeSettingsReadyState {
    if (this.state === null) {
      throw new Error("runtime settings must be loaded before use");
    }
    return this.state;
  }

  private getReadyState(): RuntimeSettingsReadyState {
    return cloneJson(this.requireReady());
  }
}

export class SettingsFallbackValidationError extends Error {
  constructor(readonly response: SettingsValidationResponse) {
    super("legacy client settings failed Rust validation");
    this.name = "SettingsFallbackValidationError";
  }
}

function cloneJson<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function jsonEquals(left: unknown, right: unknown): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}
