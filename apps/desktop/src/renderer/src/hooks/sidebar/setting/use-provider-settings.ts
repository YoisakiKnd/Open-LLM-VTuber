/* eslint-disable import/order */
import { useCallback, useEffect, useRef, useState } from "react";
import { useRuntimeSettings } from "@/settings/runtime-settings-context";
import {
  buildProviderPatch,
  providerFormFromCommitted,
  providerFormIsDirty,
  type ApiKeyMode,
  type ProviderFormState,
} from "@/settings/provider-settings-patch";
import {
  RuntimeSettingsTransactionError,
  type SettingsTransactionHandler,
} from "@/settings/settings-transaction-handler";

interface UseProviderSettingsProps {
  onSave?: (callback: SettingsTransactionHandler) => () => void;
  onCancel?: (callback: SettingsTransactionHandler) => () => void;
}

export interface UseProviderSettings {
  form: ProviderFormState;
  configured: boolean;
  configuredHint: string | null;
  ready: boolean;
  dirty: boolean;
  setField: <K extends keyof ProviderFormState>(
    key: K,
    value: ProviderFormState[K],
  ) => void;
}

export const useProviderSettings = ({
  onSave,
  onCancel,
}: UseProviderSettingsProps): UseProviderSettings => {
  const { settings, applyProvider } = useRuntimeSettings();
  const committed = settings.status === "ready" ? settings.committed : null;
  const [form, setForm] = useState<ProviderFormState | null>(null);
  // The save handler stays stable across renders and always reads the latest
  // form through these refs (avoids stale-closure saves).
  const formRef = useRef<ProviderFormState | null>(null);
  formRef.current = form;
  const committedRef = useRef(committed);
  committedRef.current = committed;

  // Hydrate the form from the committed provider domain whenever it changes.
  useEffect(() => {
    if (committed !== null) {
      setForm(providerFormFromCommitted(committed.provider));
    }
  }, [
    committed?.provider.kind,
    committed?.provider.baseUrl,
    committed?.provider.model,
  ]);

  const setField = useCallback(
    <K extends keyof ProviderFormState>(
      key: K,
      value: ProviderFormState[K],
    ) => {
      setForm((current) =>
        current === null ? current : { ...current, [key]: value },
      );
    },
    [],
  );

  const ready = form !== null && committed !== null;
  const committedApiKey = committed?.provider.apiKey ?? null;
  const configured = committedApiKey?.configured ?? false;
  const configuredHint = configured ? (committedApiKey?.hint ?? null) : null;

  const dirty = ready && providerFormIsDirty(form, committed.provider);

  const handleSave = useCallback(async () => {
    const currentForm = formRef.current;
    const currentCommitted = committedRef.current;
    if (currentForm === null || currentCommitted === null) {
      throw new RuntimeSettingsTransactionError(
        "provider settings are not loaded",
      );
    }
    try {
      await applyProvider(buildProviderPatch(currentForm));
    } catch (error) {
      throw new RuntimeSettingsTransactionError(
        error instanceof Error ? error.message : String(error),
      );
    }
  }, [applyProvider]);

  const handleCancel = useCallback(() => {
    if (committedRef.current !== null) {
      setForm(providerFormFromCommitted(committedRef.current.provider));
    }
  }, []);

  useEffect(() => {
    if (!onSave || !onCancel) return;
    const unsubscribeSave = onSave(handleSave);
    const unsubscribeCancel = onCancel(handleCancel);
    return () => {
      unsubscribeSave();
      unsubscribeCancel();
    };
  }, [handleCancel, handleSave, onCancel, onSave]);

  return {
    form:
      form ??
      providerFormFromCommitted({
        kind: "none",
        baseUrl: null,
        model: null,
        apiKey: { configured: false, hint: null },
      }),
    configured,
    configuredHint,
    ready,
    dirty,
    setField,
  };
};
