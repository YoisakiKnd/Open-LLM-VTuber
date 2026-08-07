import { describe, expect, it, vi } from "vitest";
import {
  SETTINGS_CHANGED_CHANNEL_NAME,
  createSettingsChangeChannel,
  decodeSettingsChangedNotice,
} from "./settings-change-channel";
import { createSettingsSnapshot } from "./settings-test-fixtures";

describe("settings change channel", () => {
  it("publishes only the versioned revision notice", () => {
    const postMessage = vi.fn();
    const factory = vi.fn(() => ({
      postMessage,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      close: vi.fn(),
    }));
    const channel = createSettingsChangeChannel(factory);

    channel.publish(createSettingsSnapshot(12));

    expect(factory).toHaveBeenCalledWith(SETTINGS_CHANGED_CHANNEL_NAME);
    expect(postMessage).toHaveBeenCalledWith({
      schemaVersion: 1,
      revision: 12,
    });
  });

  it("ignores malformed, future and unsafe revision messages", () => {
    expect(decodeSettingsChangedNotice(null)).toBeNull();
    expect(
      decodeSettingsChangedNotice({ schemaVersion: 2, revision: 1 }),
    ).toBeNull();
    expect(
      decodeSettingsChangedNotice({ schemaVersion: 1, revision: -1 }),
    ).toBeNull();
    expect(
      decodeSettingsChangedNotice({
        schemaVersion: 1,
        revision: Number.MAX_SAFE_INTEGER + 1,
      }),
    ).toBeNull();
    expect(
      decodeSettingsChangedNotice({ schemaVersion: 1, revision: 4 }),
    ).toEqual({ schemaVersion: 1, revision: 4 });
  });

  it("validates incoming messages and releases its listener", () => {
    let receive: ((event: MessageEvent<unknown>) => void) | null = null;
    const removeEventListener = vi.fn();
    const close = vi.fn();
    const channel = createSettingsChangeChannel(() => ({
      postMessage: vi.fn(),
      addEventListener: (_type, listener) => {
        receive = listener;
      },
      removeEventListener,
      close,
    }));
    const listener = vi.fn();
    const unsubscribe = channel.subscribe(listener);

    const dispatch = receive as unknown as (
      event: MessageEvent<unknown>,
    ) => void;
    dispatch({ data: { schemaVersion: 1, revision: "bad" } } as MessageEvent);
    dispatch({ data: { schemaVersion: 1, revision: 5 } } as MessageEvent);
    unsubscribe();
    channel.close();

    expect(listener).toHaveBeenCalledOnce();
    expect(listener).toHaveBeenCalledWith({ schemaVersion: 1, revision: 5 });
    expect(removeEventListener).toHaveBeenCalledOnce();
    expect(close).toHaveBeenCalledOnce();
  });
});
