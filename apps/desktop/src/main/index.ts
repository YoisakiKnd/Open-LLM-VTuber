/* eslint-disable no-shadow */
import { app, ipcMain, globalShortcut, desktopCapturer } from "electron";
import { resolve } from "node:path";
import { RuntimeSupervisor } from "./runtime-supervisor";
import { electronApp, optimizer } from "@electron-toolkit/utils";
import { WindowManager } from "./window-manager";
import { MenuManager } from "./menu-manager";

let windowManager: WindowManager;
let menuManager: MenuManager;
let isQuitting = false;
let runtimeSupervisor: RuntimeSupervisor | null = null;
let runtimeStopping = false;

function setupIPC(): void {
  ipcMain.handle("get-platform", () => process.platform);

  ipcMain.on("set-ignore-mouse-events", (_event, ignore: boolean) => {
    const window = windowManager.getWindow();
    if (window) {
      windowManager.setIgnoreMouseEvents(ignore);
    }
  });

  ipcMain.on("get-current-mode", (event) => {
    event.returnValue = windowManager.getCurrentMode();
  });

  ipcMain.on("pre-mode-changed", (_event, newMode) => {
    if (newMode === "window" || newMode === "pet") {
      menuManager.setMode(newMode);
    }
  });

  ipcMain.on("window-minimize", () => {
    windowManager.getWindow()?.minimize();
  });

  ipcMain.on("window-maximize", () => {
    const window = windowManager.getWindow();
    if (window) {
      windowManager.maximizeWindow();
    }
  });

  ipcMain.on("window-close", () => {
    const window = windowManager.getWindow();
    if (window) {
      if (process.platform === "darwin") {
        window.hide();
      } else {
        window.close();
      }
    }
  });

  ipcMain.on(
    "update-component-hover",
    (_event, componentId: string, isHovering: boolean) => {
      windowManager.updateComponentHover(componentId, isHovering);
    },
  );

  ipcMain.on("update-config-files", (_event, files) => {
    menuManager.updateConfigFiles(files);
  });

  ipcMain.handle("get-screen-capture", async () => {
    const sources = await desktopCapturer.getSources({ types: ["screen"] });
    return sources[0].id;
  });
}

app.whenReady().then(async () => {
  electronApp.setAppUserModelId("com.electron");

  const sourceRoot =
    process.env.OLV_PROJECT_ROOT ?? resolve(app.getAppPath(), "../..");
  const packaged = app.isPackaged;
  runtimeSupervisor = new RuntimeSupervisor({
    projectRoot: sourceRoot,
    resourcesPath: process.resourcesPath,
    assetRoot: packaged ? process.resourcesPath : sourceRoot,
    dataDir: app.getPath("userData"),
    pythonProjectRoot: process.env.OLV_PYTHON_PROJECT_ROOT ?? sourceRoot,
    startPython: process.env.OLV_DESKTOP_START_PYTHON === "1",
    startRust: process.env.OLV_DESKTOP_RUNTIME !== "0",
  });
  try {
    await runtimeSupervisor.start();
  } catch (error) {
    console.error("Failed to start Open-LLM-VTuber Runtime:", error);
    await runtimeSupervisor.stop();
    app.quit();
    return;
  }

  windowManager = new WindowManager();
  menuManager = new MenuManager((mode) => windowManager.setWindowMode(mode));

  const window = windowManager.createWindow(
    {
      titleBarOverlay: {
        color: "#111111",
        symbolColor: "#FFFFFF",
        height: 30,
      },
    },
    runtimeSupervisor.gatewayUrl,
  );
  menuManager.createTray();

  window.on("close", (event) => {
    if (!isQuitting) {
      event.preventDefault();
      window.hide();
    }
    return false;
  });

  // if (process.env.NODE_ENV === "development") {
  //   globalShortcut.register("F12", () => {
  //     const window = windowManager.getWindow();
  //     if (!window) return;

  //     if (window.webContents.isDevToolsOpened()) {
  //       window.webContents.closeDevTools();
  //     } else {
  //       window.webContents.openDevTools();
  //     }
  //   });
  // }

  setupIPC();

  app.on("activate", () => {
    const window = windowManager.getWindow();
    if (window) {
      window.show();
    }
  });

  app.on("browser-window-created", (_, window) => {
    optimizer.watchWindowShortcuts(window);
  });

  app.on("web-contents-created", (_, contents) => {
    contents.session.setPermissionRequestHandler(
      (webContents, permission, callback) => {
        if (
          permission === "media" &&
          webContents === windowManager?.getWindow()?.webContents
        ) {
          callback(true);
        } else {
          callback(false);
        }
      },
    );
  });
});

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") {
    app.quit();
  }
});

app.on("before-quit", (event) => {
  if (runtimeSupervisor && !runtimeStopping) {
    event.preventDefault();
    runtimeStopping = true;
    void runtimeSupervisor.stop().finally(() => app.quit());
  }
  isQuitting = true;
  menuManager?.destroy();
  globalShortcut.unregisterAll();
});
