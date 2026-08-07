import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SettingsSnapshotV1 } from "@/settings/generated/settings-v1.generated";
import { createSettingsSnapshot } from "@/settings/settings-test-fixtures";

const applyProvider = vi.fn(async () => ({}));

vi.mock("@/settings/runtime-settings-context", () => ({
  useRuntimeSettings: () => ({
    settings: { status: "ready", committed: committedSnapshot },
    applyProvider,
  }),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, params?: Record<string, unknown>) =>
      params ? `${key}:${JSON.stringify(params)}` : key,
  }),
}));

let committedSnapshot: SettingsSnapshotV1;

function snapshotWith(
  provider: SettingsSnapshotV1["provider"],
): SettingsSnapshotV1 {
  return { ...createSettingsSnapshot(1), provider };
}

function renderPanel(handlers?: {
  onSave?: () => () => void;
  onCancel?: () => () => void;
}) {
  return render(
    <ChakraProvider value={defaultSystem}>
      <Provider onSave={handlers?.onSave} onCancel={handlers?.onCancel} />
    </ChakraProvider>,
  );
}

import Provider from "./provider";

describe("Provider settings panel", () => {
  beforeEach(() => {
    applyProvider.mockClear();
    committedSnapshot = createSettingsSnapshot(1);
  });

  it("shows the committed provider domain and redacted key status", () => {
    committedSnapshot = snapshotWith({
      kind: "open_ai",
      baseUrl: "https://api.openai.com/v1",
      model: "gpt-4o-mini",
      apiKey: { configured: true, hint: "sk-s…1234" },
    });
    renderPanel();
    expect(
      screen.getByDisplayValue("https://api.openai.com/v1"),
    ).toBeInTheDocument();
    expect(screen.getByDisplayValue("gpt-4o-mini")).toBeInTheDocument();
    // The plaintext never renders; only the configured status with a hint.
    expect(screen.getByText(/sk-s…1234/)).toBeInTheDocument();
    expect(screen.queryByText(/sk-super-secret/)).not.toBeInTheDocument();
  });

  it("marks the form dirty and saves a provider patch with secret semantics", async () => {
    const onSave = vi.fn(() => () => {});
    const onCancel = vi.fn(() => () => {});
    committedSnapshot = snapshotWith({
      kind: "none",
      baseUrl: null,
      model: null,
      apiKey: { configured: false, hint: null },
    });
    renderPanel({ onSave, onCancel });

    const baseUrlInput = screen.getByLabelText(
      "settings.provider.baseUrl.label",
    );
    fireEvent.change(baseUrlInput, {
      target: { value: "https://api.example.com/v1" },
    });
    const modelInput = screen.getByLabelText("settings.provider.model.label");
    fireEvent.change(modelInput, { target: { value: "gpt-test" } });

    expect(
      screen.getByText("settings.provider.pendingSave"),
    ).toBeInTheDocument();

    // Wait for the re-registration that follows the form edit so the handler
    // captures the edited form state.
    await waitFor(() => expect(onSave).toHaveBeenCalled());
    const saveHandler = onSave.mock.calls[0][0];
    await saveHandler();
    // Keep mode (default): the stored secret is untouched.
    expect(applyProvider).toHaveBeenCalledWith({
      kind: "none",
      baseUrl: "https://api.example.com/v1",
      model: "gpt-test",
      apiKey: null,
    });
  });

  it("keeps the stored key when nothing was changed", async () => {
    const onSave = vi.fn(() => () => {});
    committedSnapshot = snapshotWith({
      kind: "ollama",
      baseUrl: "http://127.0.0.1:11434",
      model: "llama3.2",
      apiKey: { configured: true, hint: "…" },
    });
    renderPanel({ onSave, onCancel: () => () => {} });

    await waitFor(() => expect(onSave).toHaveBeenCalled());
    const saveHandler = onSave.mock.calls.at(-1)![0];
    await saveHandler();
    expect(applyProvider).toHaveBeenCalledWith({
      kind: "ollama",
      baseUrl: "http://127.0.0.1:11434",
      model: "llama3.2",
      apiKey: null, // keep mode: the stored secret is untouched
    });
  });

  it("does not clear the key in default keep mode", async () => {
    const onSave = vi.fn(() => () => {});
    committedSnapshot = snapshotWith({
      kind: "open_ai",
      baseUrl: null,
      model: null,
      apiKey: { configured: true, hint: "sk-s…1234" },
    });
    renderPanel({ onSave, onCancel: () => () => {} });

    await waitFor(() => expect(onSave).toHaveBeenCalled());
    const saveHandler = onSave.mock.calls.at(-1)![0];
    await saveHandler();
    expect(applyProvider).toHaveBeenCalledWith({
      kind: "open_ai",
      baseUrl: null,
      model: null,
      apiKey: null,
    });
  });
});
