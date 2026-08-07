declare global {
  interface DesktopRendererApi {
    setIgnoreMouseEvents(ignore: boolean): void;
    toggleForceIgnoreMouse(): void;
    showContextMenu(): void;
    setMode(mode: "window" | "pet"): void;
    getScreenCapture(): Promise<string>;
    updateComponentHover(componentId: string, isHovering: boolean): void;
    minimizeWindow(): void;
    maximizeWindow(): void;
    closeWindow(): void;
    unfullscreenWindow(): void;
    rendererReadyForModeChange(mode: "window" | "pet"): void;
    modeChangeRendered(): void;
    onWindowMaximizedChange(callback: (maximized: boolean) => void): () => void;
    onWindowFullscreenChange(
      callback: (fullscreen: boolean) => void,
    ): () => void;
    onPreModeChanged(callback: (mode: "window" | "pet") => void): () => void;
    onModeChanged(callback: (mode: "window" | "pet") => void): () => void;
    onMicToggle(callback: () => void): () => void;
    onInterrupt(callback: () => void): () => void;
    onToggleScrollToResize(callback: () => void): () => void;
    onSwitchCharacter(callback: (filename: string) => void): () => void;
    onToggleForceIgnoreMouse(callback: () => void): () => void;
    onForceIgnoreMouseChanged(callback: (forced: boolean) => void): () => void;
    updateConfigFiles(files: Array<{ filename: string; name: string }>): void;
  }

  interface Window {
    electron?: {
      process: { platform: string };
    };
    api?: DesktopRendererApi;
  }
}

export {};
