import type {
  ClientPreferencesInitialization,
  SettingsStorage,
} from "./client-preferences-repository";
import { clientPreferencesToSettings } from "./client-settings-compatibility";
import type { ClientSettingsV1 } from "./generated/settings-v1.generated";
import { prepareClientPreferences } from "./prepare-client-preferences";

export const DEFAULT_DESKTOP_RUNTIME_BASE_URL = "http://127.0.0.1:12394";

interface RendererLocation {
  origin: string;
  protocol: string;
}

export interface RuntimeSettingsBootstrap {
  enabled: boolean;
  apiBaseUrl: string;
  fallbackClientSettings: ClientSettingsV1 | null;
  clientPreferences: ClientPreferencesInitialization;
}

export function prepareRuntimeSettingsBootstrap(
  storage: SettingsStorage,
  location: RendererLocation,
): RuntimeSettingsBootstrap {
  const clientPreferences = prepareClientPreferences(storage, location);
  return {
    enabled: clientPreferences.status === "ready",
    apiBaseUrl: resolveRuntimeSettingsApiBaseUrl(location),
    fallbackClientSettings:
      clientPreferences.status === "ready"
        ? clientPreferencesToSettings(clientPreferences.preferences)
        : null,
    clientPreferences,
  };
}

export function resolveRuntimeSettingsApiBaseUrl(
  location: RendererLocation,
): string {
  return location.protocol === "http:" || location.protocol === "https:"
    ? location.origin
    : DEFAULT_DESKTOP_RUNTIME_BASE_URL;
}
