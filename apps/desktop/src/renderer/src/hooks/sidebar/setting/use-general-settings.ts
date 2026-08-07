/* eslint-disable import/order */
/* eslint-disable no-use-before-define */
import { useState, useEffect, useRef } from "react";
import { BgUrlContextState } from "@/context/bgurl-context";
import { commitConnectionSettings } from "@/settings/connection-settings-transaction";
import { defaultBaseUrl, defaultWsUrl } from "@/context/websocket-context";
import { useSubtitle } from "@/context/subtitle-context";
import { useCamera } from "@/context/camera-context";
import { useSwitchCharacter } from "@/hooks/utils/use-switch-character";
import { useConfig } from "@/context/character-config-context";
import { useRuntimeSettings } from "@/settings/runtime-settings-context";
import {
  mergeGeneralClientFields,
  readGeneralClientFields,
} from "@/settings/general-client-settings-bridge";
import {
  RuntimeSettingsTransactionError,
  type SettingsTransactionHandler,
} from "@/settings/settings-transaction-handler";
import {
  mirrorLegacyMediaSettings,
  readLegacyMediaSettings,
} from "@/settings/legacy-media-settings";
import { shouldPreserveTouchedDraft } from "@/settings/runtime-draft-hydration";
import i18n, { commitLanguage, previewLanguage } from "@/i18n";

interface GeneralSettings {
  language: string[];
  customBgUrl: string;
  selectedBgUrl: string[];
  backgroundUrl: string;
  wsUrl: string;
  baseUrl: string;
  imageCompressionQuality: number;
  imageMaxWidth: number;
}

interface UseGeneralSettingsProps {
  bgUrlContext: BgUrlContextState | null;
  confName: string | undefined;
  baseUrl: string;
  wsUrl: string;
  onWsUrlChange: (url: string) => void;
  onBaseUrlChange: (url: string) => void;
  onSave?: (callback: SettingsTransactionHandler) => () => void;
  onCancel?: (callback: SettingsTransactionHandler) => () => void;
}

export const useGeneralSettings = ({
  bgUrlContext,
  confName,
  baseUrl,
  wsUrl,
  onWsUrlChange,
  onBaseUrlChange,
  onSave,
  onCancel,
}: UseGeneralSettingsProps) => {
  const { showSubtitle, setShowSubtitle } = useSubtitle();
  const { setUseCameraBackground, setBackgroundPreview } = bgUrlContext || {};
  const { startBackgroundCamera, stopBackgroundCamera } = useCamera();
  const { configFiles, getFilenameByName } = useConfig();
  const { switchCharacter } = useSwitchCharacter();
  const runtimeSettings = useRuntimeSettings();
  const runtimeSettingsRef = useRef(runtimeSettings);
  runtimeSettingsRef.current = runtimeSettings;
  const managedConnectionRef = useRef({
    backgroundUrl: bgUrlContext?.backgroundUrl || null,
    wsUrl: wsUrl || defaultWsUrl,
    baseUrl: baseUrl || defaultBaseUrl,
  });
  const runtimeReadyState =
    runtimeSettings.enabled && runtimeSettings.settings.status === "ready"
      ? runtimeSettings.settings
      : null;
  const runtimeGeneralFields = runtimeReadyState
    ? readGeneralClientFields(
        runtimeReadyState.draft,
        managedConnectionRef.current,
      )
    : null;
  const legacyMediaSettings = readLegacyMediaSettings(localStorage);

  const getCurrentBgKey = (backgroundUrl?: string | null): string[] => {
    if (!backgroundUrl) return [];
    const path = backgroundUrl.replace(
      runtimeGeneralFields?.baseUrl || baseUrl,
      "",
    );
    return path.startsWith("/bg/") ? [path] : [];
  };

  const getCurrentCharacterFilename = (): string[] => {
    if (!confName) return [];
    const filename = getFilenameByName(confName);
    return filename ? [filename] : [];
  };

  const initialBackgroundUrl =
    runtimeGeneralFields?.backgroundUrl || bgUrlContext?.backgroundUrl || "";
  const initialSettings: GeneralSettings = {
    language: [runtimeGeneralFields?.locale || i18n.language || "en"],
    customBgUrl: !initialBackgroundUrl.includes("/bg/")
      ? initialBackgroundUrl
      : "",
    selectedBgUrl: getCurrentBgKey(initialBackgroundUrl),
    backgroundUrl: initialBackgroundUrl,
    wsUrl: runtimeGeneralFields?.wsUrl || wsUrl || defaultWsUrl,
    baseUrl: runtimeGeneralFields?.baseUrl || baseUrl || defaultBaseUrl,
    imageCompressionQuality:
      runtimeGeneralFields?.imageCompressionQuality ??
      legacyMediaSettings.imageCompressionQuality,
    imageMaxWidth:
      runtimeGeneralFields?.imageMaxWidth ?? legacyMediaSettings.imageMaxWidth,
  };

  const [settings, setSettings] = useState<GeneralSettings>(initialSettings);
  const [originalSettings, setOriginalSettings] =
    useState<GeneralSettings>(initialSettings);
  const settingsRef = useRef(settings);
  const originalSettingsRef = useRef(originalSettings);
  const runtimeDraftRef = useRef(runtimeReadyState?.draft ?? null);
  const clientFieldsTouchedRef = useRef(false);
  const hydratedRuntimeRevisionRef = useRef<number | null>(
    runtimeReadyState?.committed.revision ?? null,
  );
  settingsRef.current = settings;
  originalSettingsRef.current = originalSettings;
  if (runtimeReadyState) runtimeDraftRef.current = runtimeReadyState.draft;

  const syncRuntimeDraft = (nextSettings: GeneralSettings): void => {
    const currentRuntime = runtimeSettingsRef.current;
    const currentDraft = runtimeDraftRef.current;
    if (
      !currentRuntime.enabled ||
      currentRuntime.phase !== "ready" ||
      currentRuntime.settings.status !== "ready" ||
      currentDraft === null
    ) {
      return;
    }

    const rawBackgroundUrl =
      nextSettings.customBgUrl || nextSettings.selectedBgUrl[0] || "";
    const backgroundUrl = rawBackgroundUrl
      ? rawBackgroundUrl.startsWith("http")
        ? rawBackgroundUrl
        : `${nextSettings.baseUrl}${rawBackgroundUrl}`
      : null;
    const nextDraft = mergeGeneralClientFields(
      currentDraft,
      {
        locale: nextSettings.language[0] || "en",
        backgroundUrl,
        imageCompressionQuality: nextSettings.imageCompressionQuality,
        imageMaxWidth: nextSettings.imageMaxWidth,
        wsUrl: nextSettings.wsUrl,
        baseUrl: nextSettings.baseUrl,
      },
      managedConnectionRef.current,
    );
    runtimeDraftRef.current = nextDraft;
    currentRuntime.replaceDraft(nextDraft);
  };

  useEffect(() => {
    if (
      runtimeReadyState === null ||
      hydratedRuntimeRevisionRef.current ===
        runtimeReadyState.committed.revision
    ) {
      return;
    }

    if (
      shouldPreserveTouchedDraft(
        clientFieldsTouchedRef.current,
        runtimeReadyState.draft,
        runtimeReadyState.committed.client,
      )
    ) {
      hydratedRuntimeRevisionRef.current = runtimeReadyState.committed.revision;
      syncRuntimeDraft(settingsRef.current);
      return;
    }
    clientFieldsTouchedRef.current = false;

    const fields = readGeneralClientFields(
      runtimeReadyState.draft,
      managedConnectionRef.current,
    );
    const backgroundUrl = fields.backgroundUrl || "";
    const selectedPath = backgroundUrl.replace(fields.baseUrl, "");
    const updates: Partial<GeneralSettings> = {
      language: [fields.locale],
      customBgUrl: selectedPath.startsWith("/bg/") ? "" : backgroundUrl,
      selectedBgUrl: selectedPath.startsWith("/bg/") ? [selectedPath] : [],
      backgroundUrl,
      wsUrl: fields.wsUrl,
      baseUrl: fields.baseUrl,
      imageCompressionQuality: fields.imageCompressionQuality,
      imageMaxWidth: fields.imageMaxWidth,
    };
    const nextSettings = { ...settingsRef.current, ...updates };
    const nextOriginalSettings = {
      ...originalSettingsRef.current,
      ...updates,
    };
    hydratedRuntimeRevisionRef.current = runtimeReadyState.committed.revision;
    settingsRef.current = nextSettings;
    originalSettingsRef.current = nextOriginalSettings;
    setSettings(nextSettings);
    setOriginalSettings(nextOriginalSettings);
  }, [runtimeReadyState?.committed.revision]);

  useEffect(() => {
    const newBgUrl = settings.customBgUrl || settings.selectedBgUrl[0];
    if (bgUrlContext) {
      const fullUrl = newBgUrl
        ? newBgUrl.startsWith("http")
          ? newBgUrl
          : `${settings.baseUrl}${newBgUrl}`
        : null;
      setBackgroundPreview?.(fullUrl);
    }

    // Apply language change if it differs from current language
    if (
      settings.language &&
      settings.language[0] &&
      settings.language[0] !== i18n.language
    ) {
      void previewLanguage(settings.language[0]);
    }
  }, [settings, bgUrlContext, setBackgroundPreview]);

  useEffect(
    () => () => {
      setBackgroundPreview?.(null);
    },
    [setBackgroundPreview],
  );

  // Add save/cancel effect
  useEffect(() => {
    if (!onSave || !onCancel) return;

    const cleanupSave = onSave(handleSave);
    const cleanupCancel = onCancel(handleCancel);

    return () => {
      cleanupSave?.();
      cleanupCancel?.();
    };
  }, [onSave, onCancel]);

  const handleSettingChange = (
    key: keyof GeneralSettings,
    value: GeneralSettings[keyof GeneralSettings],
  ): void => {
    const nextSettings = { ...settingsRef.current, [key]: value };
    if (isRuntimeClientField(key)) clientFieldsTouchedRef.current = true;
    settingsRef.current = nextSettings;
    setSettings(nextSettings);
    syncRuntimeDraft(nextSettings);
  };

  const handleSave = async (): Promise<void> => {
    const committedSettings = settingsRef.current;
    const currentRuntime = runtimeSettingsRef.current;
    if (currentRuntime.enabled) {
      if (
        currentRuntime.phase !== "ready" ||
        currentRuntime.settings.status !== "ready"
      ) {
        throw new Error("Rust settings are not ready");
      }
      let result;
      try {
        result = await currentRuntime.save();
      } catch (error) {
        throw new RuntimeSettingsTransactionError(error);
      }
      if (result.status === "conflict") {
        throw new RuntimeSettingsTransactionError(
          new Error(
            `Settings changed in another window at revision ${result.current.revision}`,
          ),
        );
      }
    }

    await commitLanguage(committedSettings.language[0] || "en");
    commitConnectionSettings(committedSettings, {
      setWsUrl: onWsUrlChange,
      setBaseUrl: onBaseUrlChange,
    });
    const backgroundUrl =
      committedSettings.customBgUrl || committedSettings.selectedBgUrl[0];
    if (backgroundUrl && bgUrlContext) {
      bgUrlContext.setBackgroundUrl(
        backgroundUrl.startsWith("http")
          ? backgroundUrl
          : `${committedSettings.baseUrl}${backgroundUrl}`,
      );
    } else {
      setBackgroundPreview?.(null);
    }
    mirrorLegacyMediaSettings(localStorage, {
      imageCompressionQuality: committedSettings.imageCompressionQuality,
      imageMaxWidth: committedSettings.imageMaxWidth,
    });
    clientFieldsTouchedRef.current = false;
    originalSettingsRef.current = committedSettings;
    setOriginalSettings(committedSettings);
  };

  const handleCancel = (): void => {
    const currentRuntime = runtimeSettingsRef.current;
    if (
      currentRuntime.enabled &&
      currentRuntime.phase === "ready" &&
      currentRuntime.settings.status === "ready"
    ) {
      currentRuntime.cancel();
    }
    clientFieldsTouchedRef.current = false;
    const committedSettings = originalSettingsRef.current;
    settingsRef.current = committedSettings;
    setSettings(committedSettings);

    // Restore the transactional previews only. Session actions stay applied.
    setBackgroundPreview?.(null);
  };

  const handleCharacterPresetChange = (value: string[]): void => {
    const selectedFilename = value[0];
    const selectedConfig = configFiles.find(
      (config) => config.filename === selectedFilename,
    );
    const currentFilename = confName ? getFilenameByName(confName) : "";

    if (currentFilename === selectedFilename) return;

    if (selectedConfig) switchCharacter(selectedFilename);
  };

  const handleCameraToggle = async (checked: boolean) => {
    if (!setUseCameraBackground) return;

    if (checked) {
      try {
        await startBackgroundCamera();
        setUseCameraBackground(true);
      } catch (error) {
        console.error("Failed to start camera:", error);
        setUseCameraBackground(false);
      }
    } else {
      stopBackgroundCamera();
      setUseCameraBackground(false);
    }
  };

  return {
    settings,
    handleSettingChange,
    handleSave,
    handleCancel,
    handleCameraToggle,
    handleCharacterPresetChange,
    selectedCharacterPreset: getCurrentCharacterFilename(),
    useCameraBackground: bgUrlContext?.useCameraBackground || false,
    showSubtitle,
    setShowSubtitle,
  };
};

function isRuntimeClientField(key: keyof GeneralSettings): boolean {
  return (
    key === "language" ||
    key === "customBgUrl" ||
    key === "selectedBgUrl" ||
    key === "backgroundUrl" ||
    key === "wsUrl" ||
    key === "baseUrl" ||
    key === "imageCompressionQuality" ||
    key === "imageMaxWidth"
  );
}
