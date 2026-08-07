import {
  ClientPreferencesRepository,
  type ClientPreferencesInitialization,
  type SettingsStorage,
} from "./client-preferences-repository";

interface RendererLocation {
  origin: string;
  protocol: string;
}

export function prepareClientPreferences(
  storage: SettingsStorage,
  location: RendererLocation,
): ClientPreferencesInitialization {
  const sameOriginBaseUrl =
    location.protocol === "http:" || location.protocol === "https:"
      ? location.origin
      : null;

  return new ClientPreferencesRepository(storage, {
    defaultLocale: "en",
    sameOriginBaseUrl,
    // The Rust gateway is the backend for both desktop and web: the runtime
    // settings domain is enabled by default (the feature flag can still opt
    // out explicitly).
    enabledByDefault: true,
  }).initialize();
}
