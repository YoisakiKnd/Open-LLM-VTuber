import { describe, expect, it } from "vitest";
import { createClientSettings } from "./settings-test-fixtures";
import { shouldPreserveTouchedDraft } from "./runtime-draft-hydration";

describe("Runtime draft hydration", () => {
  it("hydrates an accepted server snapshot even after local edits", () => {
    const server = createClientSettings();

    expect(shouldPreserveTouchedDraft(true, server, server)).toBe(false);
  });

  it("preserves a keep-local draft that differs from the server", () => {
    const server = createClientSettings();
    const local = createClientSettings();
    local.appearance.locale = "zh";

    expect(shouldPreserveTouchedDraft(true, local, server)).toBe(true);
  });

  it("hydrates normally when no Client field was touched", () => {
    const server = createClientSettings();
    const stale = createClientSettings();
    stale.appearance.locale = "zh";

    expect(shouldPreserveTouchedDraft(false, stale, server)).toBe(false);
  });
});
