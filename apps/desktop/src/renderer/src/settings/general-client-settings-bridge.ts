import type { ClientSettingsV1 } from "./generated/settings-v1.generated";

export interface GeneralClientFields {
  locale: string;
  backgroundUrl: string | null;
  imageCompressionQuality: number;
  imageMaxWidth: number;
  wsUrl: string;
  baseUrl: string;
}

export interface ManagedGeneralDefaults {
  backgroundUrl: string | null;
  wsUrl: string;
  baseUrl: string;
}

export function readGeneralClientFields(
  settings: ClientSettingsV1,
  managedDefaults: ManagedGeneralDefaults,
): GeneralClientFields {
  return {
    locale: settings.appearance.locale,
    backgroundUrl:
      settings.appearance.backgroundUrl ?? managedDefaults.backgroundUrl,
    imageCompressionQuality: settings.media.imageCompressionQuality,
    imageMaxWidth: settings.media.imageMaxWidth,
    wsUrl: settings.connectionOverride?.wsUrl ?? managedDefaults.wsUrl,
    baseUrl: settings.connectionOverride?.baseUrl ?? managedDefaults.baseUrl,
  };
}

export function mergeGeneralClientFields(
  settings: ClientSettingsV1,
  fields: GeneralClientFields,
  managedDefaults: ManagedGeneralDefaults,
): ClientSettingsV1 {
  const wsUrl = normalizeEndpoint(fields.wsUrl);
  const baseUrl = normalizeEndpoint(fields.baseUrl);
  const wsOverride = wsUrl === managedDefaults.wsUrl ? null : wsUrl;
  const baseOverride = baseUrl === managedDefaults.baseUrl ? null : baseUrl;
  const backgroundUrl =
    fields.backgroundUrl === managedDefaults.backgroundUrl
      ? null
      : fields.backgroundUrl;

  return {
    ...settings,
    appearance: {
      locale: fields.locale,
      backgroundUrl,
    },
    media: {
      imageCompressionQuality: fields.imageCompressionQuality,
      imageMaxWidth: fields.imageMaxWidth,
    },
    connectionOverride:
      wsOverride === null && baseOverride === null
        ? null
        : { wsUrl: wsOverride, baseUrl: baseOverride },
  };
}

function normalizeEndpoint(value: string): string | null {
  const normalized = value.trim();
  return normalized.length > 0 ? normalized : null;
}
