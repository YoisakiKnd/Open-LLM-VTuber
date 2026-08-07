import { describe, expect, it, vi } from "vitest";
import { runSettingsTransactionHandlers } from "./settings-transaction-handler";

describe("settings transaction handlers", () => {
  it("awaits handlers in registration order", async () => {
    const calls: string[] = [];

    await runSettingsTransactionHandlers([
      async () => {
        await Promise.resolve();
        calls.push("first");
      },
      () => {
        calls.push("second");
      },
    ]);

    expect(calls).toEqual(["first", "second"]);
  });

  it("stops before later side effects when a handler fails", async () => {
    const later = vi.fn();

    await expect(
      runSettingsTransactionHandlers([
        async () => {
          throw new Error("save failed");
        },
        later,
      ]),
    ).rejects.toThrow("save failed");
    expect(later).not.toHaveBeenCalled();
  });
});
