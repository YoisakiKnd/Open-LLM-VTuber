import { describe, expect, it, vi } from "vitest";
import type {
  SettingsApplyResponse,
  SettingsSchemaResponse,
} from "./generated/settings-v1.generated";
import {
  SettingsApiClient,
  SettingsApiError,
  SettingsRevisionConflictError,
  SettingsValidationFailedError,
  type SettingsFetch,
} from "./settings-api-client";
import {
  createClientSettings,
  createProviderPatch,
  createSettingsSnapshot,
} from "./settings-test-fixtures";

function jsonResponse(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function createFetch(response: Response): {
  fetch: SettingsFetch;
  mock: ReturnType<typeof vi.fn>;
} {
  const mock = vi.fn(async () => response);
  return { fetch: mock as SettingsFetch, mock };
}

describe("settings API client", () => {
  it("loads a no-store same-origin snapshot", async () => {
    const snapshot = createSettingsSnapshot(7);
    const { fetch, mock } = createFetch(jsonResponse(snapshot));
    const client = new SettingsApiClient("http://127.0.0.1:12394/", fetch);

    await expect(client.getSnapshot()).resolves.toEqual(snapshot);
    expect(mock).toHaveBeenCalledOnce();
    expect(mock).toHaveBeenCalledWith(
      "http://127.0.0.1:12394/api/v1/settings/snapshot",
      expect.objectContaining({
        method: "GET",
        cache: "no-store",
        credentials: "same-origin",
        headers: { Accept: "application/json" },
      }),
    );
  });

  it("loads the generated schema contract", async () => {
    const schema = {
      schemaVersion: 1,
      owners: ["client", "desktop", "runtime", "character", "session"],
      applyEffects: ["preview", "live", "reconnect", "restart"],
      fields: [],
      schema: {},
      patchSchema: {},
      patchResponseSchema: {},
    } satisfies SettingsSchemaResponse;
    const { fetch, mock } = createFetch(jsonResponse(schema));

    await expect(new SettingsApiClient("", fetch).getSchema()).resolves.toEqual(
      schema,
    );
    expect(mock.mock.calls[0]?.[0]).toBe("/api/v1/settings/schema");
  });

  it("applies the complete client document with its base revision", async () => {
    const request = {
      baseRevision: 3,
      client: createClientSettings(),
      provider: createProviderPatch(),
    };
    request.client.appearance.locale = "zh";
    const response = {
      snapshot: {
        ...createSettingsSnapshot(4),
        client: request.client,
      },
      changedPaths: ["client.appearance.locale"],
      applyEffects: ["live"],
    } satisfies SettingsApplyResponse;
    const { fetch, mock } = createFetch(jsonResponse(response));

    await expect(
      new SettingsApiClient("", fetch).apply(request),
    ).resolves.toEqual(response);
    const init = mock.mock.calls[0]?.[1] as RequestInit;
    expect(mock.mock.calls[0]?.[0]).toBe("/api/v1/settings");
    expect(init.method).toBe("PATCH");
    expect(init.headers).toEqual({
      Accept: "application/json",
      "Content-Type": "application/json",
    });
    expect(JSON.parse(init.body as string)).toEqual(request);
  });

  it("returns the current server snapshot as a typed revision conflict", async () => {
    const snapshot = createSettingsSnapshot(9);
    const { fetch } = createFetch(
      jsonResponse(
        {
          error: {
            code: "revision_conflict",
            message: "settings changed since the draft was created",
          },
          snapshot,
        },
        409,
      ),
    );

    const error = await new SettingsApiClient("", fetch)
      .apply({
        baseRevision: 3,
        client: createClientSettings(),
        provider: createProviderPatch(),
      })
      .catch((reason: unknown) => reason);

    expect(error).toBeInstanceOf(SettingsRevisionConflictError);
    expect((error as SettingsRevisionConflictError).snapshot).toEqual(snapshot);
  });

  it("returns structured field errors for validation failures", async () => {
    const errors = [
      {
        path: "client.appearance.locale",
        code: "required",
        message: "locale must not be empty",
      },
    ];
    const { fetch } = createFetch(
      jsonResponse(
        {
          error: {
            code: "validation_failed",
            message: "settings validation failed",
          },
          errors,
        },
        422,
      ),
    );

    const error = await new SettingsApiClient("", fetch)
      .apply({
        baseRevision: 0,
        client: createClientSettings(),
        provider: createProviderPatch(),
      })
      .catch((reason: unknown) => reason);

    expect(error).toBeInstanceOf(SettingsValidationFailedError);
    expect((error as SettingsValidationFailedError).errors).toEqual(errors);
  });

  it("rejects malformed successful and error responses", async () => {
    const malformedSuccess = createFetch(jsonResponse({ revision: 1 }));
    const malformedConflict = createFetch(
      jsonResponse(
        {
          error: { code: "revision_conflict", message: "conflict" },
        },
        409,
      ),
    );

    await expect(
      new SettingsApiClient("", malformedSuccess.fetch).getSnapshot(),
    ).rejects.toMatchObject({
      code: "invalid_response",
      status: 0,
    } satisfies Partial<SettingsApiError>);
    await expect(
      new SettingsApiClient("", malformedConflict.fetch).apply({
        baseRevision: 0,
        client: createClientSettings(),
        provider: createProviderPatch(),
      }),
    ).rejects.toMatchObject({
      code: "invalid_response",
      status: 0,
    } satisfies Partial<SettingsApiError>);
  });
});
