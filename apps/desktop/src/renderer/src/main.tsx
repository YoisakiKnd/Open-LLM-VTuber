import { createRoot } from "react-dom/client";
import "./index.css";
import App from "./App";
import { LAppAdapter } from "../WebSDK/src/lappadapter";
import "./i18n";
import { RuntimeSettingsProvider } from "./settings/runtime-settings-context";
import { migrateRuntimeConnectionDefaults } from "./settings/runtime-connection-defaults";
import {
  prepareRuntimeSettingsBootstrap,
  resolveRuntimeSettingsApiBaseUrl,
} from "./settings/runtime-settings-bootstrap";

const originalConsoleWarn = console.warn;
console.warn = (...args) => {
  if (typeof args[0] === "string" && args[0].includes("onnxruntime")) {
    return;
  }
  originalConsoleWarn.apply(console, args);
};

// Suppress specific console.error messages from @chatscope/chat-ui-kit-react
const originalConsoleError = console.error;
const errorMessagesToIgnore = ["Warning: Failed"];
console.error = (...args: any[]) => {
  if (typeof args[0] === "string") {
    const shouldIgnore = errorMessagesToIgnore.some((msg) =>
      args[0].startsWith(msg),
    );
    if (shouldIgnore) {
      return; // Suppress the warning
    }
  }
  // Call the original console.error for other messages
  originalConsoleError.apply(console, args);
};

if (typeof window !== "undefined") {
  migrateRuntimeConnectionDefaults(window.localStorage, window.location);
  let runtimeSettingsEnabled = false;
  let runtimeSettingsApiBaseUrl = resolveRuntimeSettingsApiBaseUrl(
    window.location,
  );
  let fallbackClientSettings = null as ReturnType<
    typeof prepareRuntimeSettingsBootstrap
  >["fallbackClientSettings"];

  try {
    const bootstrap = prepareRuntimeSettingsBootstrap(
      window.localStorage,
      window.location,
    );
    runtimeSettingsEnabled = bootstrap.enabled;
    runtimeSettingsApiBaseUrl = bootstrap.apiBaseUrl;
    fallbackClientSettings = bootstrap.fallbackClientSettings;
    if (bootstrap.clientPreferences.status === "blocked") {
      console.warn(
        "Client preference preparation blocked:",
        bootstrap.clientPreferences,
      );
    }
  } catch (error) {
    console.warn("Client preference preparation failed:", error);
  }

  (window as any).getLAppAdapter = () => LAppAdapter.getInstance();

  // Dynamically load the Live2D Core script
  const loadLive2DCore = () => {
    return new Promise<void>((resolve, reject) => {
      const script = document.createElement("script");
      script.src = "./libs/live2dcubismcore.js"; // Path to the copied script
      script.onload = () => {
        console.log("Live2D Cubism Core loaded successfully.");
        resolve();
      };
      script.onerror = (error) => {
        console.error("Failed to load Live2D Cubism Core:", error);
        reject(error);
      };
      document.head.appendChild(script);
    });
  };

  // Load the script and then render the app
  loadLive2DCore()
    .then(() => {
      createRoot(document.getElementById("root")!).render(
        <RuntimeSettingsProvider
          enabled={runtimeSettingsEnabled}
          apiBaseUrl={runtimeSettingsApiBaseUrl}
          fallbackClientSettings={fallbackClientSettings}
        >
          <App />
        </RuntimeSettingsProvider>,
      );
    })
    .catch((error) => {
      console.error(
        "Application failed to start due to script loading error:",
        error,
      );
      // Optionally render an error message to the user
      const rootElement = document.getElementById("root");
      if (rootElement) {
        rootElement.innerHTML =
          "Error loading required components. Please check the console for details.";
      }
    });
}
