/* eslint-disable import/no-extraneous-dependencies */
import { Field, Input, Select, createListCollection } from "@chakra-ui/react";
import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import type { ProviderKindSetting } from "@/settings/generated/settings-v1.generated";
import { useProviderSettings } from "@/hooks/sidebar/setting/use-provider-settings";
import type { SettingsTransactionHandler } from "@/settings/settings-transaction-handler";
import { settingStyles } from "./setting-styles";

interface ProviderProps {
  onSave?: (callback: SettingsTransactionHandler) => () => void;
  onCancel?: (callback: SettingsTransactionHandler) => () => void;
}

const PROVIDER_KINDS: Array<{
  value: ProviderKindSetting;
  labelKey: string;
}> = [
  { value: "none", labelKey: "settings.provider.kind.none" },
  { value: "open_ai", labelKey: "settings.provider.kind.openAi" },
  { value: "anthropic", labelKey: "settings.provider.kind.anthropic" },
  { value: "ollama", labelKey: "settings.provider.kind.ollama" },
];

function Provider({ onSave, onCancel }: ProviderProps): JSX.Element {
  const { t } = useTranslation();
  const { form, configured, configuredHint, ready, dirty, setField } =
    useProviderSettings({ onSave, onCancel });

  const kindCollection = useMemo(
    () =>
      createListCollection({
        items: PROVIDER_KINDS.map((entry) => ({
          value: entry.value,
          label: t(entry.labelKey),
        })),
        itemToString: (item) => item.label,
        itemToValue: (item) => item.value,
      }),
    [t],
  );
  const keyModeCollection = useMemo(
    () =>
      createListCollection({
        items: ["keep", "replace", "clear"] as const,
        itemToString: (item) => t(`settings.provider.apiKey.mode.${item}`),
        itemToValue: (item) => item,
      }),
    [t],
  );

  if (!ready) {
    return (
      <div {...settingStyles.settingUI.bodyText}>
        {t("settings.provider.loading")}
      </div>
    );
  }

  return (
    <div>
      <Field.Root {...settingStyles.settingUI.fieldRoot}>
        <Field.Label {...settingStyles.settingUI.fieldLabel}>
          {t("settings.provider.kind.label")}
        </Field.Label>
        <Select.Root
          collection={kindCollection}
          value={[form.kind]}
          onValueChange={(details) =>
            setField("kind", details.value[0] as ProviderKindSetting)
          }
        >
          <Select.Trigger>
            <Select.ValueText />
          </Select.Trigger>
          <Select.Content>
            {PROVIDER_KINDS.map((entry) => (
              <Select.Item
                key={entry.value}
                item={{ value: entry.value, label: t(entry.labelKey) }}
              >
                {t(entry.labelKey)}
              </Select.Item>
            ))}
          </Select.Content>
        </Select.Root>
      </Field.Root>

      <Field.Root {...settingStyles.settingUI.fieldRoot}>
        <Field.Label {...settingStyles.settingUI.fieldLabel}>
          {t("settings.provider.baseUrl.label")}
        </Field.Label>
        <Input
          value={form.baseUrl}
          onChange={(event) => setField("baseUrl", event.target.value)}
          placeholder={t("settings.provider.baseUrl.placeholder")}
        />
        <Field.HelperText {...settingStyles.settingUI.fieldHelper}>
          {t("settings.provider.baseUrl.helper")}
        </Field.HelperText>
      </Field.Root>

      <Field.Root {...settingStyles.settingUI.fieldRoot}>
        <Field.Label {...settingStyles.settingUI.fieldLabel}>
          {t("settings.provider.model.label")}
        </Field.Label>
        <Input
          value={form.model}
          onChange={(event) => setField("model", event.target.value)}
          placeholder={t("settings.provider.model.placeholder")}
        />
      </Field.Root>

      <Field.Root {...settingStyles.settingUI.fieldRoot}>
        <Field.Label {...settingStyles.settingUI.fieldLabel}>
          {t("settings.provider.apiKey.label")}
        </Field.Label>
        {configured ? (
          <div {...settingStyles.settingUI.bodyText}>
            {t("settings.provider.apiKey.configured", { hint: configuredHint })}
          </div>
        ) : (
          <div {...settingStyles.settingUI.bodyText}>
            {t("settings.provider.apiKey.notConfigured")}
          </div>
        )}
        <Select.Root
          collection={keyModeCollection}
          value={[form.apiKeyMode]}
          onValueChange={(details) =>
            setField(
              "apiKeyMode",
              details.value[0] as "keep" | "replace" | "clear",
            )
          }
        >
          <Select.Trigger>
            <Select.ValueText />
          </Select.Trigger>
          <Select.Content>
            <Select.Item item="keep">
              {t("settings.provider.apiKey.mode.keep")}
            </Select.Item>
            <Select.Item item="replace">
              {t("settings.provider.apiKey.mode.replace")}
            </Select.Item>
            <Select.Item item="clear">
              {t("settings.provider.apiKey.mode.clear")}
            </Select.Item>
          </Select.Content>
        </Select.Root>
        {form.apiKeyMode === "replace" ? (
          <Input
            type="password"
            value={form.apiKeyInput}
            onChange={(event) => setField("apiKeyInput", event.target.value)}
            placeholder={t("settings.provider.apiKey.placeholder")}
          />
        ) : null}
        <Field.HelperText {...settingStyles.settingUI.fieldHelper}>
          {t("settings.provider.apiKey.helper")}
        </Field.HelperText>
      </Field.Root>

      {dirty ? (
        <div {...settingStyles.settingUI.bodyText}>
          {t("settings.provider.pendingSave")}
        </div>
      ) : null}
    </div>
  );
}

export default Provider;
