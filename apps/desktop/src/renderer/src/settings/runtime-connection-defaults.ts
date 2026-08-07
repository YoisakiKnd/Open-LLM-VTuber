export interface RuntimeLocation {
  protocol: string;
  host: string;
  origin: string;
}

export interface RuntimeConnectionMigrationResult {
  wsUrl: string;
  baseUrl: string;
  changedKeys: string[];
}

const LEGACY_WS_DEFAULTS = new Set([
  "ws://127.0.0.1:12393/client-ws",
  "ws://localhost:12393/client-ws",
]);
const LEGACY_BASE_DEFAULTS = new Set([
  "http://127.0.0.1:12393",
  "http://localhost:12393",
]);

export function migrateRuntimeConnectionDefaults(
  storage: Pick<Storage, "getItem" | "setItem">,
  location: RuntimeLocation,
): RuntimeConnectionMigrationResult {
  const fileRenderer = location.protocol === "file:";
  const onGateway =
    location.origin === "http://127.0.0.1:12394" ||
    location.origin === "http://localhost:12394";
  const onLoopback = /^https?:\/\/(127\.0\.0\.1|localhost):\d+$/.test(
    location.origin,
  );
  // Gateway-served pages stay same-origin; loopback origins that are not the
  // gateway (e.g. the Vite dev server) and file:// pages use the loopback
  // gateway defaults. Remote deployments keep their own same-origin gateway.
  const useLoopback = fileRenderer || (onLoopback && !onGateway);
  const wsUrl = useLoopback
    ? "ws://127.0.0.1:12394/client-ws"
    : `${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}/client-ws`;
  const baseUrl = useLoopback ? "http://127.0.0.1:12394" : location.origin;
  const changedKeys: string[] = [];

  // Drop corrupt values written by older builds (literal "undefined"/"null"
  // strings) so the default endpoints take effect.
  for (const key of LEGACY_STORAGE_KEYS) {
    if (removeInvalidStoredValue(storage, key)) {
      changedKeys.push(key);
    }
  }

  if (migrateKnownDefault(storage, "wsUrl", LEGACY_WS_DEFAULTS, wsUrl)) {
    changedKeys.push("wsUrl");
  }
  if (migrateKnownDefault(storage, "baseUrl", LEGACY_BASE_DEFAULTS, baseUrl)) {
    changedKeys.push("baseUrl");
  }

  return { wsUrl, baseUrl, changedKeys };
}

const LEGACY_STORAGE_KEYS = [
  "wsUrl",
  "baseUrl",
  "modelInfo",
  "backgroundUrl",
  "i18nextLng",
] as const;

const INVALID_STORED_LITERALS = new Set(["undefined", "null", "NaN"]);

/**
 * Removes a stored value that is not valid JSON, or that JSON-parses to a
 * literal "undefined"/"null"/"NaN" string (older builds wrote these).
 * Returns `true` when the value was removed.
 */
function removeInvalidStoredValue(
  storage: Pick<Storage, "getItem" | "removeItem">,
  key: string,
): boolean {
  const raw = storage.getItem(key);
  if (raw === null) return false;
  // Values written by older builds can be the raw string "undefined"/"null"
  // (not valid JSON) or JSON-encoded literals; both are corrupt for these
  // application-owned keys and are removed so defaults take effect. Custom
  // remote values (valid JSON strings) are preserved.
  let invalid = false;
  try {
    const parsed: unknown = JSON.parse(raw);
    invalid =
      (typeof parsed === "string" && INVALID_STORED_LITERALS.has(parsed)) ||
      parsed === null;
  } catch {
    invalid = true;
  }
  if (invalid) {
    storage.removeItem(key);
    return true;
  }
  return false;
}

function migrateKnownDefault(
  storage: Pick<Storage, "getItem" | "setItem">,
  key: string,
  legacyDefaults: ReadonlySet<string>,
  nextValue: string,
): boolean {
  const rawValue = storage.getItem(key);
  if (rawValue !== null && rawValue.trim().length > 0) {
    try {
      const currentValue: unknown = JSON.parse(rawValue);
      if (
        typeof currentValue !== "string" ||
        !legacyDefaults.has(currentValue)
      ) {
        return false;
      }
    } catch {
      return false;
    }
  }
  storage.setItem(key, JSON.stringify(nextValue));
  return true;
}
