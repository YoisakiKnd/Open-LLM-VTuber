import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import type {
  ClientSettingsV1,
  ProviderPatchV1,
  SettingsApplyResponse,
  SettingsValidationError,
  SettingsValidationResponse,
} from "./generated/settings-v1.generated";
import {
  RuntimeSettingsRepository,
  type RuntimeSettingsSaveResult,
  type RuntimeSettingsState,
  type SettingsConflictResolution,
} from "./runtime-settings-repository";
import {
  SettingsApiClient,
  SettingsValidationFailedError,
  type SettingsApi,
} from "./settings-api-client";
import {
  createSettingsChangeChannel,
  type SettingsChangeChannel,
} from "./settings-change-channel";

export type RuntimeSettingsPhase = "disabled" | "loading" | "ready" | "error";

export interface RuntimeSettingsContextValue {
  enabled: boolean;
  phase: RuntimeSettingsPhase;
  settings: RuntimeSettingsState;
  validationErrors: SettingsValidationError[];
  externalRevision: number | null;
  operationError: Error | null;
  isSaving: boolean;
  isValidating: boolean;
  replaceDraft(draft: ClientSettingsV1): void;
  cancel(): void;
  validate(): Promise<SettingsValidationResponse>;
  save(): Promise<RuntimeSettingsSaveResult>;
  applyProvider(provider: ProviderPatchV1): Promise<SettingsApplyResponse>;
  resolveConflict(resolution: SettingsConflictResolution): void;
  reload(): Promise<void>;
}

export interface RuntimeSettingsProviderProps {
  enabled: boolean;
  apiBaseUrl: string;
  fallbackClientSettings: ClientSettingsV1 | null;
  children: ReactNode;
  api?: SettingsApi;
  createChannel?: () => SettingsChangeChannel;
}

const RuntimeSettingsContext =
  createContext<RuntimeSettingsContextValue | null>(null);

export function RuntimeSettingsProvider({
  enabled,
  apiBaseUrl,
  fallbackClientSettings,
  children,
  api,
  createChannel = createSettingsChangeChannel,
}: RuntimeSettingsProviderProps): JSX.Element {
  const repository = useMemo(
    () =>
      enabled
        ? new RuntimeSettingsRepository(
            api ?? new SettingsApiClient(apiBaseUrl),
          )
        : null,
    [api, apiBaseUrl, enabled],
  );
  const [phase, setPhase] = useState<RuntimeSettingsPhase>(
    enabled ? "loading" : "disabled",
  );
  const [settings, setSettings] = useState<RuntimeSettingsState>({
    status: "uninitialized",
  });
  const [validationErrors, setValidationErrors] = useState<
    SettingsValidationError[]
  >([]);
  const [externalRevision, setExternalRevision] = useState<number | null>(null);
  const [operationError, setOperationError] = useState<Error | null>(null);
  const [isSaving, setIsSaving] = useState(false);
  const [isValidating, setIsValidating] = useState(false);
  const changeChannel = useRef<SettingsChangeChannel | null>(null);

  const load = useCallback(
    async (signal?: AbortSignal): Promise<void> => {
      if (repository === null) return;
      setPhase("loading");
      setOperationError(null);
      try {
        await repository.load(signal, fallbackClientSettings);
        setSettings(repository.getState());
        setValidationErrors([]);
        setExternalRevision(null);
        setPhase("ready");
      } catch (error) {
        if (isAbortError(error)) return;
        setOperationError(toError(error));
        setPhase("error");
      }
    },
    [fallbackClientSettings, repository],
  );

  useEffect(() => {
    if (repository === null) {
      setPhase("disabled");
      setSettings({ status: "uninitialized" });
      setValidationErrors([]);
      setExternalRevision(null);
      setOperationError(null);
      return;
    }

    const controller = new AbortController();
    void load(controller.signal);
    return () => controller.abort();
  }, [load, repository]);

  useEffect(() => {
    if (repository === null) return;

    const channel = createChannel();
    changeChannel.current = channel;
    const unsubscribe = channel.subscribe((notice) => {
      const current = repository.getState();
      if (
        current.status !== "ready" ||
        notice.revision <= current.committed.revision
      ) {
        return;
      }
      if (repository.hasUnsavedChanges()) {
        setExternalRevision((revision) =>
          Math.max(revision ?? 0, notice.revision),
        );
        return;
      }
      void load();
    });

    return () => {
      unsubscribe();
      channel.close();
      if (changeChannel.current === channel) changeChannel.current = null;
    };
  }, [createChannel, load, repository]);

  const replaceDraft = useCallback(
    (draft: ClientSettingsV1) => {
      const activeRepository = requireRepository(repository);
      setSettings(activeRepository.replaceDraft(draft));
      setValidationErrors([]);
      setOperationError(null);
    },
    [repository],
  );

  const cancel = useCallback(() => {
    const activeRepository = requireRepository(repository);
    const shouldReload = externalRevision !== null;
    setSettings(activeRepository.cancel());
    setValidationErrors([]);
    setOperationError(null);
    if (shouldReload) void load();
  }, [externalRevision, load, repository]);

  const validate = useCallback(async () => {
    const activeRepository = requireRepository(repository);
    setIsValidating(true);
    setOperationError(null);
    try {
      const response = await activeRepository.validate();
      setValidationErrors(response.errors);
      return response;
    } catch (error) {
      setOperationError(toError(error));
      throw error;
    } finally {
      setIsValidating(false);
    }
  }, [repository]);

  const save = useCallback(async () => {
    const activeRepository = requireRepository(repository);
    setIsSaving(true);
    setOperationError(null);
    try {
      const result = await activeRepository.save();
      setSettings(activeRepository.getState());
      if (result.status === "saved") {
        setValidationErrors([]);
        setExternalRevision(null);
        changeChannel.current?.publish(result.response.snapshot);
      } else {
        setExternalRevision(result.current.revision);
      }
      return result;
    } catch (error) {
      if (error instanceof SettingsValidationFailedError) {
        setValidationErrors(error.errors);
      }
      setOperationError(toError(error));
      throw error;
    } finally {
      setIsSaving(false);
    }
  }, [repository]);

  const applyProvider = useCallback(
    async (provider: ProviderPatchV1): Promise<SettingsApplyResponse> => {
      const activeRepository = requireRepository(repository);
      setIsSaving(true);
      setOperationError(null);
      try {
        const response = await activeRepository.applyProviderPatch(provider);
        setSettings(activeRepository.getState());
        setValidationErrors([]);
        setExternalRevision(null);
        changeChannel.current?.publish(response.snapshot);
        return response;
      } catch (error) {
        if (error instanceof SettingsValidationFailedError) {
          setValidationErrors(error.errors);
        }
        setOperationError(toError(error));
        throw error;
      } finally {
        setIsSaving(false);
      }
    },
    [repository],
  );

  const resolveConflict = useCallback(
    (resolution: SettingsConflictResolution) => {
      const activeRepository = requireRepository(repository);
      setSettings(activeRepository.resolveConflict(resolution));
      setExternalRevision(null);
      setOperationError(null);
    },
    [repository],
  );

  const reload = useCallback(() => load(), [load]);

  const value = useMemo<RuntimeSettingsContextValue>(
    () => ({
      enabled,
      phase,
      settings,
      validationErrors,
      externalRevision,
      operationError,
      isSaving,
      isValidating,
      replaceDraft,
      cancel,
      validate,
      save,
      applyProvider,
      resolveConflict,
      reload,
    }),
    [
      cancel,
      enabled,
      externalRevision,
      isSaving,
      isValidating,
      operationError,
      phase,
      reload,
      replaceDraft,
      resolveConflict,
      save,
      settings,
      validate,
      validationErrors,
    ],
  );

  return (
    <RuntimeSettingsContext.Provider value={value}>
      {children}
    </RuntimeSettingsContext.Provider>
  );
}

export function useRuntimeSettings(): RuntimeSettingsContextValue {
  const context = useContext(RuntimeSettingsContext);
  if (context === null) {
    throw new Error(
      "useRuntimeSettings must be used within RuntimeSettingsProvider",
    );
  }
  return context;
}

function requireRepository(
  repository: RuntimeSettingsRepository | null,
): RuntimeSettingsRepository {
  if (repository === null) {
    throw new Error("runtime settings are disabled");
  }
  return repository;
}

function toError(value: unknown): Error {
  return value instanceof Error ? value : new Error(String(value));
}

function isAbortError(value: unknown): boolean {
  return value instanceof Error && value.name === "AbortError";
}
