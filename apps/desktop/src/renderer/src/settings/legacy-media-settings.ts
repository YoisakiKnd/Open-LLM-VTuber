export const IMAGE_COMPRESSION_QUALITY_KEY = "appImageCompressionQuality";
export const DEFAULT_IMAGE_COMPRESSION_QUALITY = 0.8;
export const IMAGE_MAX_WIDTH_KEY = "appImageMaxWidth";
export const DEFAULT_IMAGE_MAX_WIDTH = 0;

export interface LegacyMediaSettingsStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

export interface LegacyMediaSettings {
  imageCompressionQuality: number;
  imageMaxWidth: number;
}

export function readLegacyMediaSettings(
  storage: Pick<LegacyMediaSettingsStorage, "getItem">,
): LegacyMediaSettings {
  return {
    imageCompressionQuality: readCompressionQuality(storage),
    imageMaxWidth: readImageMaxWidth(storage),
  };
}

export function mirrorLegacyMediaSettings(
  storage: Pick<LegacyMediaSettingsStorage, "setItem">,
  settings: LegacyMediaSettings,
): void {
  storage.setItem(
    IMAGE_COMPRESSION_QUALITY_KEY,
    settings.imageCompressionQuality.toString(),
  );
  storage.setItem(IMAGE_MAX_WIDTH_KEY, settings.imageMaxWidth.toString());
}

function readCompressionQuality(
  storage: Pick<LegacyMediaSettingsStorage, "getItem">,
): number {
  const storedQuality = storage.getItem(IMAGE_COMPRESSION_QUALITY_KEY);
  if (storedQuality) {
    const quality = Number.parseFloat(storedQuality);
    if (!Number.isNaN(quality) && quality >= 0.1 && quality <= 1) {
      return quality;
    }
  }
  return DEFAULT_IMAGE_COMPRESSION_QUALITY;
}

function readImageMaxWidth(
  storage: Pick<LegacyMediaSettingsStorage, "getItem">,
): number {
  const storedMaxWidth = storage.getItem(IMAGE_MAX_WIDTH_KEY);
  if (storedMaxWidth) {
    const maxWidth = Number.parseInt(storedMaxWidth, 10);
    if (!Number.isNaN(maxWidth) && maxWidth >= 0) return maxWidth;
  }
  return DEFAULT_IMAGE_MAX_WIDTH;
}
