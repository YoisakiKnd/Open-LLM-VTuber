import { describe, expect, it, vi } from "vitest";
import { commitConnectionSettings } from "./connection-settings-transaction";

describe("connection settings transaction", () => {
  it("applies the complete draft only when explicitly committed", () => {
    const setWsUrl = vi.fn();
    const setBaseUrl = vi.fn();
    const draft = {
      wsUrl: "wss://remote.example/client-ws",
      baseUrl: "https://remote.example",
    };

    expect(setWsUrl).not.toHaveBeenCalled();
    expect(setBaseUrl).not.toHaveBeenCalled();

    commitConnectionSettings(draft, { setWsUrl, setBaseUrl });

    expect(setBaseUrl).toHaveBeenCalledOnce();
    expect(setBaseUrl).toHaveBeenCalledWith("https://remote.example");
    expect(setWsUrl).toHaveBeenCalledOnce();
    expect(setWsUrl).toHaveBeenCalledWith("wss://remote.example/client-ws");
    expect(setBaseUrl.mock.invocationCallOrder[0]).toBeLessThan(
      setWsUrl.mock.invocationCallOrder[0],
    );
  });
});
