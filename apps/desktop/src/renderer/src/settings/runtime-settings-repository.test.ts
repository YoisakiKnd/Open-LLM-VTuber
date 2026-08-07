import { describe, expect, it, vi } from "vitest";
import type {
  SettingsApplyResponse,
  SettingsSchemaResponse,
  SettingsSnapshotV1,
  SettingsValidationResponse,
} from "./generated/settings-v1.generated";
import {
  RuntimeSettingsRepository,
  SettingsFallbackValidationError,
} from "./runtime-settings-repository";
import {
  SettingsRevisionConflictError,
  type SettingsApi,
} from "./settings-api-client";
import { createSettingsSnapshot } from "./settings-test-fixtures";

function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
} {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((complete) => {
    resolve = complete;
  });
  return { promise, resolve };
}

function createApi(snapshot = createSettingsSnapshot()): SettingsApi & {
  getSnapshot: ReturnType<typeof vi.fn>;
  validate: ReturnType<typeof vi.fn>;
  apply: ReturnType<typeof vi.fn>;
} {
  return {
    getSchema: vi.fn<() => Promise<SettingsSchemaResponse>>(),
    getSnapshot: vi.fn(async () => snapshot),
    validate: vi.fn(
      async () =>
        ({ valid: true, errors: [] }) satisfies SettingsValidationResponse,
    ),
    apply: vi.fn(),
  };
}

describe("runtime settings repository", () => {
  it("requires an explicit load before draft operations", () => {
    const repository = new RuntimeSettingsRepository(createApi());

    expect(repository.getState()).toEqual({ status: "uninitialized" });
    expect(() => repository.cancel()).toThrow(
      "runtime settings must be loaded before use",
    );
  });

  it("loads independent committed and draft snapshots", async () => {
    const serverSnapshot = createSettingsSnapshot(2);
    const repository = new RuntimeSettingsRepository(createApi(serverSnapshot));

    const loaded = await repository.load();
    loaded.draft.appearance.locale = "changed-outside";
    serverSnapshot.client.appearance.locale = "changed-at-source";

    expect(repository.getState()).toMatchObject({
      status: "ready",
      committed: { client: { appearance: { locale: "en" } } },
      draft: { appearance: { locale: "en" } },
    });
  });

  it("deduplicates concurrent loads", async () => {
    const api = createApi();
    const pending = deferred<SettingsSnapshotV1>();
    api.getSnapshot.mockReturnValue(pending.promise);
    const repository = new RuntimeSettingsRepository(api);

    const firstLoad = repository.load();
    const secondLoad = repository.load();
    pending.resolve(createSettingsSnapshot(8));

    expect(firstLoad).toBe(secondLoad);
    await expect(firstLoad).resolves.toMatchObject({
      committed: { revision: 8 },
    });
    expect(api.getSnapshot).toHaveBeenCalledOnce();
  });

  it("imports a different legacy fallback into an untouched Rust snapshot", async () => {
    const api = createApi(createSettingsSnapshot(0));
    const fallback = createSettingsSnapshot().client;
    fallback.appearance.locale = "zh";
    const imported = createSettingsSnapshot(1);
    imported.client = fallback;
    const response = {
      snapshot: imported,
      changedPaths: ["client.appearance.locale"],
      applyEffects: ["live"],
    } satisfies SettingsApplyResponse;
    api.apply.mockResolvedValue(response);
    const repository = new RuntimeSettingsRepository(api);

    await expect(repository.load(undefined, fallback)).resolves.toMatchObject({
      committed: { revision: 1, client: { appearance: { locale: "zh" } } },
      draft: { appearance: { locale: "zh" } },
    });
    expect(api.validate).toHaveBeenCalledWith(
      {
        schemaVersion: 1,
        revision: 0,
        client: fallback,
        provider: createSettingsSnapshot(0).provider,
      },
      undefined,
    );
    expect(api.apply).toHaveBeenCalledWith(
      {
        baseRevision: 0,
        client: fallback,
        provider: { kind: "none", baseUrl: null, model: null, apiKey: null },
      },
      undefined,
    );
  });

  it("does not write when legacy fallback equals Rust defaults", async () => {
    const api = createApi(createSettingsSnapshot(0));
    const repository = new RuntimeSettingsRepository(api);

    await repository.load(undefined, createSettingsSnapshot().client);

    expect(api.validate).not.toHaveBeenCalled();
    expect(api.apply).not.toHaveBeenCalled();
  });

  it("never replays legacy fallback over an established Rust revision", async () => {
    const server = createSettingsSnapshot(3);
    server.client.appearance.locale = "server";
    const fallback = createSettingsSnapshot().client;
    fallback.appearance.locale = "legacy";
    const api = createApi(server);
    const repository = new RuntimeSettingsRepository(api);

    await expect(repository.load(undefined, fallback)).resolves.toMatchObject({
      draft: { appearance: { locale: "server" } },
    });
    expect(api.validate).not.toHaveBeenCalled();
    expect(api.apply).not.toHaveBeenCalled();
  });

  it("blocks an invalid legacy fallback before persistence", async () => {
    const api = createApi(createSettingsSnapshot(0));
    api.validate.mockResolvedValue({
      valid: false,
      errors: [
        {
          path: "client.appearance.locale",
          code: "required",
          message: "locale must not be empty",
        },
      ],
    } satisfies SettingsValidationResponse);
    const fallback = createSettingsSnapshot().client;
    fallback.appearance.locale = "";
    const repository = new RuntimeSettingsRepository(api);

    await expect(repository.load(undefined, fallback)).rejects.toBeInstanceOf(
      SettingsFallbackValidationError,
    );
    expect(api.apply).not.toHaveBeenCalled();
    expect(repository.getState()).toEqual({ status: "uninitialized" });
  });

  it("adopts the winning server snapshot when fallback import conflicts", async () => {
    const api = createApi(createSettingsSnapshot(0));
    const fallback = createSettingsSnapshot().client;
    fallback.appearance.locale = "legacy";
    const winner = createSettingsSnapshot(1);
    winner.client.appearance.locale = "other-window";
    api.apply.mockRejectedValue(
      new SettingsRevisionConflictError(winner, "conflict"),
    );
    const repository = new RuntimeSettingsRepository(api);

    await expect(repository.load(undefined, fallback)).resolves.toMatchObject({
      committed: { revision: 1 },
      draft: { appearance: { locale: "other-window" } },
      conflict: null,
    });
  });

  it("edits and cancels without calling the API", async () => {
    const api = createApi();
    const repository = new RuntimeSettingsRepository(api);
    await repository.load();
    const draft = createSettingsSnapshot().client;
    draft.appearance.locale = "zh";

    repository.replaceDraft(draft);
    expect(repository.hasUnsavedChanges()).toBe(true);
    expect(repository.getState()).toMatchObject({
      status: "ready",
      draft: { appearance: { locale: "zh" } },
    });
    expect(api.apply).not.toHaveBeenCalled();
    expect(api.validate).not.toHaveBeenCalled();

    const cancelled = repository.cancel();
    expect(cancelled.draft.appearance.locale).toBe("en");
    expect(repository.hasUnsavedChanges()).toBe(false);
    expect(api.apply).not.toHaveBeenCalled();
  });

  it("validates a complete draft without persisting it", async () => {
    const api = createApi(createSettingsSnapshot(4));
    const repository = new RuntimeSettingsRepository(api);
    await repository.load();
    const draft = createSettingsSnapshot().client;
    draft.media.imageCompressionQuality = 0.6;
    repository.replaceDraft(draft);

    await expect(repository.validate()).resolves.toEqual({
      valid: true,
      errors: [],
    });
    expect(api.validate).toHaveBeenCalledWith(
      {
        schemaVersion: 1,
        revision: 4,
        client: draft,
        provider: createSettingsSnapshot(4).provider,
      },
      undefined,
    );
    expect(api.apply).not.toHaveBeenCalled();
  });

  it("commits with the loaded revision and adopts actual server values", async () => {
    const api = createApi(createSettingsSnapshot(5));
    const repository = new RuntimeSettingsRepository(api);
    await repository.load();
    const draft = createSettingsSnapshot().client;
    draft.appearance.locale = "zh";
    repository.replaceDraft(draft);

    const actual = createSettingsSnapshot(6);
    actual.client.appearance.locale = "zh";
    actual.client.media.imageMaxWidth = 1280;
    const response = {
      snapshot: actual,
      changedPaths: ["client.appearance.locale", "client.media.imageMaxWidth"],
      applyEffects: ["live"],
    } satisfies SettingsApplyResponse;
    api.apply.mockResolvedValue(response);

    await expect(repository.save()).resolves.toEqual({
      status: "saved",
      response,
    });
    expect(api.apply).toHaveBeenCalledWith(
      {
        baseRevision: 5,
        client: draft,
        provider: { kind: "none", baseUrl: null, model: null, apiKey: null },
      },
      undefined,
    );
    expect(repository.getState()).toMatchObject({
      status: "ready",
      committed: { revision: 6 },
      draft: { media: { imageMaxWidth: 1280 } },
      conflict: null,
    });
  });

  it("deduplicates concurrent saves and preserves edits made in flight", async () => {
    const api = createApi(createSettingsSnapshot(5));
    const repository = new RuntimeSettingsRepository(api);
    await repository.load();
    const submittedDraft = createSettingsSnapshot().client;
    submittedDraft.appearance.locale = "zh";
    repository.replaceDraft(submittedDraft);

    const pending = deferred<SettingsApplyResponse>();
    api.apply.mockReturnValue(pending.promise);
    const firstSave = repository.save();
    const secondSave = repository.save();

    const newerDraft = createSettingsSnapshot().client;
    newerDraft.appearance.locale = "ja";
    repository.replaceDraft(newerDraft);
    const response = {
      snapshot: {
        ...createSettingsSnapshot(6),
        client: submittedDraft,
      },
      changedPaths: ["client.appearance.locale"],
      applyEffects: ["live"],
    } satisfies SettingsApplyResponse;
    pending.resolve(response);

    expect(firstSave).toBe(secondSave);
    await expect(firstSave).resolves.toEqual({ status: "saved", response });
    expect(api.apply).toHaveBeenCalledOnce();
    expect(repository.getState()).toMatchObject({
      committed: { revision: 6, client: { appearance: { locale: "zh" } } },
      draft: { appearance: { locale: "ja" } },
    });
  });

  it("preserves the local draft on conflict until the caller resolves it", async () => {
    const api = createApi(createSettingsSnapshot(2));
    const repository = new RuntimeSettingsRepository(api);
    await repository.load();
    const localDraft = createSettingsSnapshot().client;
    localDraft.appearance.locale = "local";
    repository.replaceDraft(localDraft);

    const serverSnapshot = createSettingsSnapshot(3);
    serverSnapshot.client.appearance.locale = "server";
    api.apply.mockRejectedValue(
      new SettingsRevisionConflictError(serverSnapshot, "conflict"),
    );

    await expect(repository.save()).resolves.toEqual({
      status: "conflict",
      current: serverSnapshot,
      draft: localDraft,
    });
    expect(repository.getState()).toMatchObject({
      status: "ready",
      committed: { revision: 2 },
      draft: { appearance: { locale: "local" } },
      conflict: { revision: 3 },
    });

    repository.resolveConflict("keep-local");
    const retryResponse = {
      snapshot: { ...serverSnapshot, revision: 4, client: localDraft },
      changedPaths: ["client.appearance.locale"],
      applyEffects: ["live"],
    } satisfies SettingsApplyResponse;
    api.apply.mockResolvedValue(retryResponse);

    await expect(repository.save()).resolves.toMatchObject({ status: "saved" });
    expect(api.apply).toHaveBeenLastCalledWith(
      {
        baseRevision: 3,
        client: localDraft,
        provider: { kind: "none", baseUrl: null, model: null, apiKey: null },
      },
      undefined,
    );
  });

  it("can discard a conflicted draft in favor of the server snapshot", async () => {
    const api = createApi(createSettingsSnapshot(1));
    const repository = new RuntimeSettingsRepository(api);
    await repository.load();
    const localDraft = createSettingsSnapshot().client;
    localDraft.appearance.locale = "local";
    repository.replaceDraft(localDraft);

    const serverSnapshot = createSettingsSnapshot(2);
    serverSnapshot.client.appearance.locale = "server";
    api.apply.mockRejectedValue(
      new SettingsRevisionConflictError(serverSnapshot, "conflict"),
    );
    await repository.save();

    expect(repository.resolveConflict("accept-server")).toMatchObject({
      committed: { revision: 2 },
      draft: { appearance: { locale: "server" } },
      conflict: null,
    });
  });
});
