import type {
  ProviderKindSetting,
  ProviderPatchV1,
  ProviderSettingsV1,
} from "./generated/settings-v1.generated";

export type ApiKeyMode = "keep" | "replace" | "clear";

export interface ProviderFormState {
  kind: ProviderKindSetting;
  baseUrl: string;
  model: string;
  apiKeyMode: ApiKeyMode;
  apiKeyInput: string;
}

/**
 * Builds the provider PATCH from the form state. Secret semantics:
 * - `apiKeyMode === "clear"` → `Some("")` clears the stored key.
 * - `apiKeyMode === "replace"` with non-empty input → `Some(plaintext)` stores it.
 * - otherwise → `null` keeps the stored key untouched.
 */
export function buildProviderPatch(form: ProviderFormState): ProviderPatchV1 {
  return {
    kind: form.kind,
    baseUrl: form.baseUrl.trim() === "" ? null : form.baseUrl.trim(),
    model: form.model.trim() === "" ? null : form.model.trim(),
    apiKey: resolveApiKey(form),
  };
}

export function resolveApiKey(form: ProviderFormState): string | null {
  if (form.apiKeyMode === "clear") return "";
  if (form.apiKeyMode === "replace" && form.apiKeyInput.trim().length > 0) {
    return form.apiKeyInput.trim();
  }
  return null;
}

/**
 * True when the form differs from the committed provider domain. A "replace"
 * mode with empty input is not dirty (nothing to store yet).
 */
export function providerFormIsDirty(
  form: ProviderFormState,
  committed: ProviderSettingsV1,
): boolean {
  return (
    form.kind !== committed.kind ||
    (form.baseUrl.trim() === "" ? null : form.baseUrl.trim()) !==
      committed.baseUrl ||
    (form.model.trim() === "" ? null : form.model.trim()) !== committed.model ||
    form.apiKeyMode === "clear" ||
    (form.apiKeyMode === "replace" && form.apiKeyInput.trim().length > 0)
  );
}

/** Initial form derived from the committed provider domain. */
export function providerFormFromCommitted(
  committed: ProviderSettingsV1,
): ProviderFormState {
  return {
    kind: committed.kind,
    baseUrl: committed.baseUrl ?? "",
    model: committed.model ?? "",
    apiKeyMode: "keep",
    apiKeyInput: "",
  };
}
