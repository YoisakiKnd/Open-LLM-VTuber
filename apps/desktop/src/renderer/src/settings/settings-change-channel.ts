import {
  SETTINGS_SCHEMA_VERSION,
  type SettingsSnapshotV1,
} from "./generated/settings-v1.generated";

export const SETTINGS_CHANGED_CHANNEL_NAME = "olv.settings-changed.v1";

export interface SettingsChangedNotice {
  schemaVersion: typeof SETTINGS_SCHEMA_VERSION;
  revision: number;
}

export interface SettingsChangeChannel {
  publish(snapshot: SettingsSnapshotV1): void;
  subscribe(listener: (notice: SettingsChangedNotice) => void): () => void;
  close(): void;
}

interface BroadcastChannelLike {
  postMessage(message: unknown): void;
  addEventListener(
    type: "message",
    listener: (event: MessageEvent<unknown>) => void,
  ): void;
  removeEventListener(
    type: "message",
    listener: (event: MessageEvent<unknown>) => void,
  ): void;
  close(): void;
}

export type BroadcastChannelFactory = (name: string) => BroadcastChannelLike;

export function createSettingsChangeChannel(
  factory: BroadcastChannelFactory | null = defaultBroadcastChannelFactory(),
): SettingsChangeChannel {
  if (factory === null) return NOOP_SETTINGS_CHANGE_CHANNEL;

  const channel = factory(SETTINGS_CHANGED_CHANNEL_NAME);
  return {
    publish(snapshot) {
      channel.postMessage({
        schemaVersion: SETTINGS_SCHEMA_VERSION,
        revision: snapshot.revision,
      } satisfies SettingsChangedNotice);
    },
    subscribe(listener) {
      const receive = (event: MessageEvent<unknown>) => {
        const notice = decodeSettingsChangedNotice(event.data);
        if (notice !== null) listener(notice);
      };
      channel.addEventListener("message", receive);
      return () => channel.removeEventListener("message", receive);
    },
    close() {
      channel.close();
    },
  };
}

export function decodeSettingsChangedNotice(
  value: unknown,
): SettingsChangedNotice | null {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    return null;
  }
  const record = value as Record<string, unknown>;
  if (
    record.schemaVersion !== SETTINGS_SCHEMA_VERSION ||
    typeof record.revision !== "number" ||
    !Number.isSafeInteger(record.revision) ||
    record.revision < 0
  ) {
    return null;
  }
  return {
    schemaVersion: SETTINGS_SCHEMA_VERSION,
    revision: record.revision,
  };
}

const NOOP_SETTINGS_CHANGE_CHANNEL: SettingsChangeChannel = {
  publish() {},
  subscribe() {
    return () => {};
  },
  close() {},
};

function defaultBroadcastChannelFactory(): BroadcastChannelFactory | null {
  return typeof globalThis.BroadcastChannel === "function"
    ? (name) => new globalThis.BroadcastChannel(name)
    : null;
}
