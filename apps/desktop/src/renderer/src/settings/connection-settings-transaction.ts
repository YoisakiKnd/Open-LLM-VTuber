export interface ConnectionSettingsDraft {
  wsUrl: string;
  baseUrl: string;
}

export interface ConnectionSettingsEffects {
  setWsUrl(url: string): void;
  setBaseUrl(url: string): void;
}

export function commitConnectionSettings(
  draft: ConnectionSettingsDraft,
  effects: ConnectionSettingsEffects,
): void {
  effects.setBaseUrl(draft.baseUrl);
  effects.setWsUrl(draft.wsUrl);
}
