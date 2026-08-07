import { contextBridge, ipcRenderer } from "electron";
import type { ConfigFile } from "../main/menu-manager";

type Mode = "window" | "pet";
type Unsubscribe = () => void;

declare global {
  interface Window {
    electron?: {
      process: { platform: NodeJS.Platform };
    };
    api: DesktopApi;
  }
}

export interface DesktopApi {
  setIgnoreMouseEvents(ignore: boolean): void;
  toggleForceIgnoreMouse(): void;
  showContextMenu(): void;
  setMode(mode: Mode): void;
  getScreenCapture(): Promise<string>;
  updateComponentHover(componentId: string, isHovering: boolean): void;
  minimizeWindow(): void;
  maximizeWindow(): void;
  closeWindow(): void;
  unfullscreenWindow(): void;
  rendererReadyForModeChange(mode: Mode): void;
  modeChangeRendered(): void;
  onWindowMaximizedChange(callback: (maximized: boolean) => void): Unsubscribe;
  onWindowFullscreenChange(
    callback: (fullscreen: boolean) => void,
  ): Unsubscribe;
  onPreModeChanged(callback: (mode: Mode) => void): Unsubscribe;
  onModeChanged(callback: (mode: Mode) => void): Unsubscribe;
  onMicToggle(callback: () => void): Unsubscribe;
  onInterrupt(callback: () => void): Unsubscribe;
  onToggleScrollToResize(callback: () => void): Unsubscribe;
  onSwitchCharacter(callback: (filename: string) => void): Unsubscribe;
  onToggleForceIgnoreMouse(callback: () => void): Unsubscribe;
  onForceIgnoreMouseChanged(callback: (forced: boolean) => void): Unsubscribe;
  updateConfigFiles(files: ConfigFile[]): void;
}

function subscribe<T extends unknown[]>(
  channel: string,
  callback: (...args: T) => void,
): Unsubscribe {
  const handler = (_event: Electron.IpcRendererEvent, ...args: T) =>
    callback(...args);
  ipcRenderer.on(channel, handler);
  return () => ipcRenderer.removeListener(channel, handler);
}

const api: DesktopApi = {
  setIgnoreMouseEvents: (ignore) =>
    ipcRenderer.send("set-ignore-mouse-events", ignore),
  toggleForceIgnoreMouse: () => ipcRenderer.send("toggle-force-ignore-mouse"),
  showContextMenu: () => ipcRenderer.send("show-context-menu"),
  setMode: (mode) => ipcRenderer.send("pre-mode-changed", mode),
  getScreenCapture: () => ipcRenderer.invoke("get-screen-capture"),
  updateComponentHover: (componentId, isHovering) =>
    ipcRenderer.send("update-component-hover", componentId, isHovering),
  minimizeWindow: () => ipcRenderer.send("window-minimize"),
  maximizeWindow: () => ipcRenderer.send("window-maximize"),
  closeWindow: () => ipcRenderer.send("window-close"),
  unfullscreenWindow: () => ipcRenderer.send("window-unfullscreen"),
  rendererReadyForModeChange: (mode) =>
    ipcRenderer.send("renderer-ready-for-mode-change", mode),
  modeChangeRendered: () => ipcRenderer.send("mode-change-rendered"),
  onWindowMaximizedChange: (callback) =>
    subscribe("window-maximized-change", callback),
  onWindowFullscreenChange: (callback) =>
    subscribe("window-fullscreen-change", callback),
  onPreModeChanged: (callback) => subscribe("pre-mode-changed", callback),
  onModeChanged: (callback) => subscribe("mode-changed", callback),
  onMicToggle: (callback) => subscribe("mic-toggle", callback),
  onInterrupt: (callback) => subscribe("interrupt", callback),
  onToggleScrollToResize: (callback) =>
    subscribe("toggle-scroll-to-resize", callback),
  onSwitchCharacter: (callback) => subscribe("switch-character", callback),
  onToggleForceIgnoreMouse: (callback) =>
    subscribe("toggle-force-ignore-mouse", callback),
  onForceIgnoreMouseChanged: (callback) =>
    subscribe("force-ignore-mouse-changed", callback),
  updateConfigFiles: (files) => ipcRenderer.send("update-config-files", files),
};

if (process.contextIsolated) {
  contextBridge.exposeInMainWorld("electron", {
    process: { platform: process.platform },
  });
  contextBridge.exposeInMainWorld("api", api);
} else {
  window.electron = { process: { platform: process.platform } };
  window.api = api;
}
