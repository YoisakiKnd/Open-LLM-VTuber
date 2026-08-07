/* eslint-disable import/no-extraneous-dependencies */
import {
  Tabs,
  Button,
  DrawerRoot,
  DrawerContent,
  DrawerHeader,
  DrawerTitle,
  DrawerBody,
  DrawerFooter,
  DrawerBackdrop,
  DrawerCloseTrigger,
} from "@chakra-ui/react";
import { useState, useMemo, useCallback, useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { CloseButton } from "@/components/ui/close-button";
import {
  RuntimeSettingsTransactionError,
  runSettingsTransactionHandlers,
  type SettingsTransactionHandler,
} from "@/settings/settings-transaction-handler";
import { RuntimeSettingsStatus } from "@/settings/runtime-settings-status";

import { settingStyles } from "./setting-styles";
import General from "./general";
import Provider from "./provider";
import Live2D from "./live2d";
import ASR from "./asr";
import TTS from "./tts";
import Agent from "./agent";
import About from "./about";

interface SettingUIProps {
  open: boolean;
  onClose: () => void;
  onToggle: () => void;
}

function SettingUI({ open, onClose }: SettingUIProps): JSX.Element {
  const { t } = useTranslation();
  const [saveHandlers, setSaveHandlers] = useState<
    SettingsTransactionHandler[]
  >([]);
  const [cancelHandlers, setCancelHandlers] = useState<
    SettingsTransactionHandler[]
  >([]);
  const [isSaving, setIsSaving] = useState(false);
  const [transactionError, setTransactionError] = useState<Error | null>(null);
  const [activeTab, setActiveTab] = useState("general");
  const closingRef = useRef(false);

  useEffect(() => {
    if (open) {
      closingRef.current = false;
      setTransactionError(null);
    }
  }, [open]);

  const handleSaveCallback = useCallback(
    (handler: SettingsTransactionHandler) => {
      setSaveHandlers((prev) => [...prev, handler]);
      return (): void => {
        setSaveHandlers((prev) => prev.filter((item) => item !== handler));
      };
    },
    [],
  );

  const handleCancelCallback = useCallback(
    (handler: SettingsTransactionHandler) => {
      setCancelHandlers((prev) => [...prev, handler]);
      return (): void => {
        setCancelHandlers((prev) => prev.filter((item) => item !== handler));
      };
    },
    [],
  );

  const handleSave = useCallback(async (): Promise<void> => {
    if (isSaving) return;
    setIsSaving(true);
    setTransactionError(null);
    try {
      await runSettingsTransactionHandlers(saveHandlers);
      closingRef.current = true;
      onClose();
    } catch (error) {
      closingRef.current = false;
      if (!(error instanceof RuntimeSettingsTransactionError)) {
        setTransactionError(toError(error));
      }
      console.error("Settings save failed:", error);
    } finally {
      setIsSaving(false);
    }
  }, [isSaving, saveHandlers, onClose]);

  const handleCancel = useCallback((): void => {
    if (closingRef.current) return;
    closingRef.current = true;
    setTransactionError(null);
    void runSettingsTransactionHandlers(cancelHandlers).then(
      onClose,
      (error) => {
        closingRef.current = false;
        console.error("Settings cancel failed:", error);
      },
    );
  }, [cancelHandlers, onClose]);

  const tabsContent = useMemo(
    () => (
      <Tabs.ContentGroup>
        <Tabs.Content value="general" {...settingStyles.settingUI.tabs.content}>
          <General
            onSave={handleSaveCallback}
            onCancel={handleCancelCallback}
          />
        </Tabs.Content>
        <Tabs.Content
          value="provider"
          {...settingStyles.settingUI.tabs.content}
        >
          <Provider
            onSave={handleSaveCallback}
            onCancel={handleCancelCallback}
          />
        </Tabs.Content>
        <Tabs.Content value="live2d" {...settingStyles.settingUI.tabs.content}>
          <Live2D onSave={handleSaveCallback} onCancel={handleCancelCallback} />
        </Tabs.Content>
        <Tabs.Content value="asr" {...settingStyles.settingUI.tabs.content}>
          <ASR onSave={handleSaveCallback} onCancel={handleCancelCallback} />
        </Tabs.Content>
        <Tabs.Content value="tts" {...settingStyles.settingUI.tabs.content}>
          <TTS />
        </Tabs.Content>
        <Tabs.Content value="agent" {...settingStyles.settingUI.tabs.content}>
          <Agent onSave={handleSaveCallback} onCancel={handleCancelCallback} />
        </Tabs.Content>
        <Tabs.Content value="about" {...settingStyles.settingUI.tabs.content}>
          <About />
        </Tabs.Content>
      </Tabs.ContentGroup>
    ),
    [handleSaveCallback, handleCancelCallback],
  );

  return (
    <DrawerRoot
      open={open}
      onOpenChange={(event) => {
        if (!event.open && !isSaving && !closingRef.current) handleCancel();
      }}
      placement="start"
    >
      <DrawerBackdrop />
      <DrawerContent {...settingStyles.settingUI.drawerContent}>
        <DrawerHeader {...settingStyles.settingUI.drawerHeader}>
          <DrawerTitle {...settingStyles.settingUI.drawerTitle}>
            {t("common.settings")}
          </DrawerTitle>
          <div {...settingStyles.settingUI.closeButton}>
            <DrawerCloseTrigger asChild>
              <CloseButton size="sm" color="white" />
            </DrawerCloseTrigger>
          </div>
        </DrawerHeader>

        <DrawerBody>
          <RuntimeSettingsStatus transactionError={transactionError} />
          <Tabs.Root
            defaultValue="general"
            value={activeTab}
            onValueChange={(details) => setActiveTab(details.value)}
            {...settingStyles.settingUI.tabs.root}
          >
            <Tabs.List {...settingStyles.settingUI.tabs.list}>
              <Tabs.Trigger
                value="general"
                {...settingStyles.settingUI.tabs.trigger}
              >
                {t("settings.tabs.general")}
              </Tabs.Trigger>
              <Tabs.Trigger
                value="provider"
                {...settingStyles.settingUI.tabs.trigger}
              >
                {t("settings.tabs.provider")}
              </Tabs.Trigger>
              <Tabs.Trigger
                value="live2d"
                {...settingStyles.settingUI.tabs.trigger}
              >
                {t("settings.tabs.live2d")}
              </Tabs.Trigger>
              <Tabs.Trigger
                value="asr"
                {...settingStyles.settingUI.tabs.trigger}
              >
                {t("settings.tabs.asr")}
              </Tabs.Trigger>
              <Tabs.Trigger
                value="tts"
                {...settingStyles.settingUI.tabs.trigger}
              >
                {t("settings.tabs.tts")}
              </Tabs.Trigger>
              <Tabs.Trigger
                value="agent"
                {...settingStyles.settingUI.tabs.trigger}
              >
                {t("settings.tabs.agent")}
              </Tabs.Trigger>
              <Tabs.Trigger
                value="about"
                {...settingStyles.settingUI.tabs.trigger}
              >
                {t("settings.tabs.about")}
              </Tabs.Trigger>
            </Tabs.List>

            {tabsContent}
          </Tabs.Root>
        </DrawerBody>

        <DrawerFooter>
          <Button colorPalette="red" onClick={handleCancel} disabled={isSaving}>
            {t("common.cancel")}
          </Button>
          <Button colorPalette="blue" onClick={handleSave} disabled={isSaving}>
            {t("common.save")}
          </Button>
        </DrawerFooter>
      </DrawerContent>
    </DrawerRoot>
  );
}

export default SettingUI;

function toError(value: unknown): Error {
  return value instanceof Error ? value : new Error(String(value));
}
