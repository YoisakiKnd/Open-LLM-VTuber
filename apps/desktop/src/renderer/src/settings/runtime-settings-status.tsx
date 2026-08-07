import { Button, Flex, Stack, Text } from "@chakra-ui/react";
import { useTranslation } from "react-i18next";
import { Alert } from "../components/ui/alert";
import { useRuntimeSettings } from "./runtime-settings-context";

export interface RuntimeSettingsStatusProps {
  transactionError: Error | null;
}

export function RuntimeSettingsStatus({
  transactionError,
}: RuntimeSettingsStatusProps): JSX.Element | null {
  const { t } = useTranslation();
  const runtimeSettings = useRuntimeSettings();
  const conflict =
    runtimeSettings.settings.status === "ready"
      ? runtimeSettings.settings.conflict
      : null;
  const displayedError = transactionError ?? runtimeSettings.operationError;

  if (!runtimeSettings.enabled) return null;

  return (
    <Stack gap="2" mb="3">
      {runtimeSettings.phase === "loading" && (
        <Alert
          status="info"
          colorPalette="blue"
          title={t("settings.runtime.loading")}
        />
      )}

      {displayedError !== null && conflict === null && (
        <Alert
          status="error"
          colorPalette="red"
          title={t("settings.runtime.operationFailed")}
        >
          {displayedError.message}
        </Alert>
      )}

      {runtimeSettings.validationErrors.length > 0 && (
        <Alert
          status="error"
          colorPalette="red"
          title={t("settings.runtime.validationFailed")}
        >
          <Stack gap="1">
            {runtimeSettings.validationErrors.map((error) => (
              <Text key={`${error.path}:${error.code}`} fontSize="sm">
                {error.path}: {error.message}
              </Text>
            ))}
          </Stack>
        </Alert>
      )}

      {conflict !== null && (
        <Alert
          status="warning"
          colorPalette="orange"
          title={t("settings.runtime.revisionConflict")}
        >
          <Stack gap="2">
            <Text fontSize="sm">
              {t("settings.runtime.revisionConflictDescription", {
                revision: conflict.revision,
              })}
            </Text>
            <Flex gap="2" wrap="wrap">
              <Button
                size="xs"
                colorPalette="orange"
                onClick={() => runtimeSettings.resolveConflict("accept-server")}
              >
                {t("settings.runtime.acceptServer")}
              </Button>
              <Button
                size="xs"
                variant="outline"
                onClick={() => runtimeSettings.resolveConflict("keep-local")}
              >
                {t("settings.runtime.keepLocal")}
              </Button>
            </Flex>
          </Stack>
        </Alert>
      )}

      {conflict === null && runtimeSettings.externalRevision !== null && (
        <Alert
          status="warning"
          colorPalette="orange"
          title={t("settings.runtime.externalChange")}
        >
          <Stack gap="2">
            <Text fontSize="sm">
              {t("settings.runtime.externalChangeDescription", {
                revision: runtimeSettings.externalRevision,
              })}
            </Text>
            <Button
              alignSelf="flex-start"
              size="xs"
              variant="outline"
              onClick={() => void runtimeSettings.reload()}
            >
              {t("settings.runtime.loadServer")}
            </Button>
          </Stack>
        </Alert>
      )}
    </Stack>
  );
}
