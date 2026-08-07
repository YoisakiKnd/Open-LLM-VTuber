import { describe, expect, it } from "vitest";
import {
  buildProviderPatch,
  providerFormFromCommitted,
  providerFormIsDirty,
  resolveApiKey,
} from "./provider-settings-patch";
import { createSettingsSnapshot } from "./settings-test-fixtures";

const committed = () => createSettingsSnapshot(1).provider;

describe("provider settings patch", () => {
  it("derives the initial form from the committed provider domain", () => {
    const form = providerFormFromCommitted({
      kind: "ollama",
      baseUrl: "http://127.0.0.1:11434",
      model: "llama3.2",
      apiKey: { configured: false, hint: null },
    });
    expect(form).toEqual({
      kind: "ollama",
      baseUrl: "http://127.0.0.1:11434",
      model: "llama3.2",
      apiKeyMode: "keep",
      apiKeyInput: "",
    });
  });

  it("keeps the stored key with keep mode or empty replace input", () => {
    const base = providerFormFromCommitted(committed());
    expect(resolveApiKey(base)).toBeNull();
    expect(
      resolveApiKey({ ...base, apiKeyMode: "replace", apiKeyInput: "  " }),
    ).toBeNull();
  });

  it("clears the key with clear mode", () => {
    const base = providerFormFromCommitted(committed());
    expect(resolveApiKey({ ...base, apiKeyMode: "clear" })).toBe("");
  });

  it("stores trimmed plaintext with replace mode", () => {
    const base = providerFormFromCommitted(committed());
    expect(
      resolveApiKey({
        ...base,
        apiKeyMode: "replace",
        apiKeyInput: "  sk-abc  ",
      }),
    ).toBe("sk-abc");
  });

  it("builds a full patch preserving client-agnostic semantics", () => {
    const base = providerFormFromCommitted(committed());
    const patch = buildProviderPatch({
      ...base,
      kind: "open_ai",
      baseUrl: " https://api.example.com/v1 ",
      apiKeyMode: "replace",
      apiKeyInput: "sk-new",
    });
    expect(patch).toEqual({
      kind: "open_ai",
      baseUrl: "https://api.example.com/v1",
      model: null,
      apiKey: "sk-new",
    });
  });

  it("normalizes empty base URL and model to null", () => {
    const base = providerFormFromCommitted(committed());
    const patch = buildProviderPatch({ ...base, baseUrl: "   ", model: "" });
    expect(patch.baseUrl).toBeNull();
    expect(patch.model).toBeNull();
  });

  it("detects dirty state only for real changes", () => {
    const base = providerFormFromCommitted(committed());
    expect(providerFormIsDirty(base, committed())).toBe(false);
    expect(providerFormIsDirty({ ...base, kind: "ollama" }, committed())).toBe(
      true,
    );
    expect(
      providerFormIsDirty({ ...base, apiKeyMode: "replace" }, committed()),
    ).toBe(false);
    expect(
      providerFormIsDirty(
        { ...base, apiKeyMode: "replace", apiKeyInput: "x" },
        committed(),
      ),
    ).toBe(true);
    expect(
      providerFormIsDirty({ ...base, apiKeyMode: "clear" }, committed()),
    ).toBe(true);
  });
});
