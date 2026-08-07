import { describe, expect, it } from "vitest";
import { migrateRuntimeConnectionDefaults } from "./runtime-connection-defaults";

class MemoryStorage {
  readonly values = new Map<string, string>();

  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }

  setItem(key: string, value: string): void {
    this.values.set(key, value);
  }

  removeItem(key: string): void {
    this.values.delete(key);
  }
}

describe("Runtime connection default migration", () => {
  it("uses the supervised Rust loopback Runtime for file renderers", () => {
    const storage = new MemoryStorage();

    const result = migrateRuntimeConnectionDefaults(storage, {
      protocol: "file:",
      host: "",
      origin: "null",
    });

    expect(result.changedKeys).toEqual(["wsUrl", "baseUrl"]);
    expect(JSON.parse(storage.values.get("wsUrl")!)).toBe(
      "ws://127.0.0.1:12394/client-ws",
    );
    expect(JSON.parse(storage.values.get("baseUrl")!)).toBe(
      "http://127.0.0.1:12394",
    );
  });

  it("moves legacy local defaults to the current HTTP origin", () => {
    const storage = new MemoryStorage();
    storage.setItem("wsUrl", JSON.stringify("ws://localhost:12393/client-ws"));
    storage.setItem("baseUrl", JSON.stringify("http://127.0.0.1:12393"));

    migrateRuntimeConnectionDefaults(storage, {
      protocol: "https:",
      host: "vtuber.example",
      origin: "https://vtuber.example",
    });

    expect(JSON.parse(storage.values.get("wsUrl")!)).toBe(
      "wss://vtuber.example/client-ws",
    );
    expect(JSON.parse(storage.values.get("baseUrl")!)).toBe(
      "https://vtuber.example",
    );
  });

  it("preserves custom remote values and repairs malformed ones", () => {
    const storage = new MemoryStorage();
    storage.setItem("wsUrl", JSON.stringify("wss://remote.example/client-ws"));
    storage.setItem("baseUrl", "not-json");

    const result = migrateRuntimeConnectionDefaults(storage, {
      protocol: "file:",
      host: "",
      origin: "null",
    });

    // The custom remote WebSocket URL survives untouched...
    expect(result.changedKeys).not.toContain("wsUrl");
    expect(JSON.parse(storage.values.get("wsUrl")!)).toBe(
      "wss://remote.example/client-ws",
    );
    // ...while the malformed baseUrl is repaired to the loopback gateway.
    expect(JSON.parse(storage.values.get("baseUrl")!)).toBe(
      "http://127.0.0.1:12394",
    );
  });
});

describe("invalid stored literal cleanup", () => {
  function createStorage(): {
    values: Map<string, string>;
    getItem: (k: string) => string | null;
    setItem: (k: string, v: string) => void;
    removeItem: (k: string) => void;
  } {
    const values = new Map<string, string>();
    return {
      values,
      getItem: (k) => values.get(k) ?? null,
      setItem: (k, v) => values.set(k, v),
      removeItem: (k) => values.delete(k),
    };
  }

  it("removes literal undefined/null values written by older builds", () => {
    const storage = createStorage();
    storage.setItem("baseUrl", JSON.stringify("undefined"));
    storage.setItem("modelInfo", '"null"');
    storage.setItem(
      "backgroundUrl",
      JSON.stringify("http://127.0.0.1:12394/bg/x.jpeg"),
    );
    const result = migrateRuntimeConnectionDefaults(storage, {
      protocol: "file:",
      host: "127.0.0.1:12394",
      origin: "http://127.0.0.1:12394",
    });
    // baseUrl is cleaned and then re-migrated to the loopback gateway default.
    expect(storage.values.get("baseUrl")).toBe(
      JSON.stringify("http://127.0.0.1:12394"),
    );
    expect(storage.values.has("modelInfo")).toBe(false);
    expect(storage.values.get("backgroundUrl")).toBe(
      JSON.stringify("http://127.0.0.1:12394/bg/x.jpeg"),
    );
    expect(result.changedKeys).toContain("baseUrl");
  });
});
