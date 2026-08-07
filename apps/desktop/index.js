"use strict";
const electron = require("electron");
const node_path = require("node:path");
const node_child_process = require("node:child_process");
const node_fs = require("node:fs");
const node_http = require("node:http");
const node_os = require("node:os");
const utils = require("@electron-toolkit/utils");
const path = require("path");
const isWindows = process.platform === "win32";
class RuntimeSupervisor {
  constructor(options) {
    this.options = options;
    this.rustProcess = null;
    this.pythonProcess = null;
    this.stopPromise = null;
  }
  async start() {
    if (this.options.startPython ?? process.env.OLV_DESKTOP_START_PYTHON === "1") {
      this.pythonProcess = this.spawnPython();
      await waitForHealth(this.pythonPort, this.pythonProcess, 3e4);
    }
    if (this.options.startRust ?? process.env.OLV_DESKTOP_RUNTIME !== "0") {
      const binary = this.resolveRustBinary();
      if (!node_fs.existsSync(binary)) {
        throw new Error(
          `Rust Gateway binary not found at ${binary}; build the release runtime or set OLV_RUST_GATEWAY_BINARY`
        );
      }
      this.rustProcess = node_child_process.spawn(binary, this.rustArguments, {
        cwd: this.options.projectRoot,
        env: this.runtimeEnvironment,
        stdio: "inherit",
        // On Windows the gateway is a console executable; hide its console
        // window when spawned from the GUI process.
        windowsHide: true
      });
      await waitForHealth(this.rustPort, this.rustProcess, 15e3);
    }
  }
  stop() {
    if (this.stopPromise !== null) return this.stopPromise;
    this.stopPromise = (async () => {
      await stopProcess(this.rustProcess, this.rustPort);
      await stopProcess(this.pythonProcess, this.pythonPort);
      this.rustProcess = null;
      this.pythonProcess = null;
    })();
    return this.stopPromise;
  }
  get rustPort() {
    return this.options.rustPort ?? 12394;
  }
  /**
   * Origin URL of the gateway's static frontend. In packaged builds the
   * renderer is loaded from here (same-origin WebSocket + fetch, no CORS).
   */
  get gatewayUrl() {
    return `http://127.0.0.1:${this.rustPort}`;
  }
  get pythonPort() {
    return this.options.pythonPort ?? 12393;
  }
  get assetRoot() {
    return this.options.assetRoot ?? this.options.projectRoot;
  }
  get dataDir() {
    return this.options.dataDir ?? this.options.projectRoot;
  }
  get pythonProjectRoot() {
    return this.options.pythonProjectRoot ?? this.options.projectRoot;
  }
  get runtimeEnvironment() {
    return {
      ...process.env,
      OLV_RUST_GATEWAY_BINARY: void 0,
      OLV_SKIP_RUST_GATEWAY: this.pythonProcess ? "1" : process.env.OLV_SKIP_RUST_GATEWAY
    };
  }
  get rustArguments() {
    const root = this.assetRoot;
    const legacyConfig = node_fs.existsSync(node_path.join(this.dataDir, "conf.yaml")) ? node_path.join(this.dataDir, "conf.yaml") : node_path.join(root, "conf.yaml");
    const cacheDirectory = this.pythonProcess ? node_path.join(this.pythonProjectRoot, "cache") : node_path.join(this.dataDir, "cache");
    node_fs.mkdirSync(cacheDirectory, { recursive: true });
    return [
      "--listen",
      `127.0.0.1:${this.rustPort}`,
      "--python-ws-url",
      `ws://127.0.0.1:${this.pythonPort}/client-ws`,
      ...this.pythonProcess ? [] : ["--allow-missing-python"],
      "--frontend-dir",
      node_path.join(root, "frontend"),
      "--cache-dir",
      cacheDirectory,
      "--live2d-models-dir",
      node_path.join(root, "live2d-models"),
      "--backgrounds-dir",
      node_path.join(root, "backgrounds"),
      "--avatars-dir",
      node_path.join(root, "avatars"),
      "--web-tool-dir",
      node_path.join(root, "web_tool"),
      "--settings-file",
      node_path.join(this.dataDir, "settings.v1.json"),
      "--legacy-config-file",
      legacyConfig,
      "--legacy-characters-dir",
      node_path.join(root, "characters"),
      "--model-dict-file",
      node_path.join(root, "model_dict.json"),
      "--allowed-origins",
      `http://127.0.0.1:${this.rustPort},http://localhost:${this.rustPort},null,http://localhost:3000,http://127.0.0.1:3000`
    ];
  }
  resolveRustBinary() {
    if (this.options.rustBinary) return this.options.rustBinary;
    if (process.env.OLV_RUST_GATEWAY_BINARY) {
      return process.env.OLV_RUST_GATEWAY_BINARY;
    }
    const executableName = isWindows ? "rust-gateway.exe" : "rust-gateway";
    const packaged = node_path.join(
      this.options.resourcesPath,
      "runtime",
      executableName
    );
    if (node_fs.existsSync(packaged)) return packaged;
    const developmentName = isWindows ? "open-llm-vtuber-gateway.exe" : "open-llm-vtuber-gateway";
    return node_path.resolve(
      this.options.projectRoot,
      "rust-gateway/target/release",
      developmentName
    );
  }
  spawnPython() {
    const uvName = isWindows ? "uv.exe" : "uv";
    const executable = this.options.pythonExecutable ?? process.env.OLV_PYTHON_EXECUTABLE ?? node_path.join(node_os.homedir(), ".local", "bin", uvName);
    const command = node_fs.existsSync(executable) ? executable : uvName;
    const root = this.pythonProjectRoot;
    const venvBin = node_path.join(root, ".venv", isWindows ? "Scripts" : "bin");
    const processEnvironment = {
      ...process.env,
      OLV_SKIP_RUST_GATEWAY: "1",
      PATH: [venvBin, process.env.PATH].filter(Boolean).join(node_path.delimiter)
    };
    return node_child_process.spawn(command, ["run", "--project", root, "run_server.py"], {
      cwd: root,
      env: processEnvironment,
      stdio: "inherit",
      windowsHide: true
    });
  }
}
function waitForHealth(port, child, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  return new Promise((resolveHealth, rejectHealth) => {
    let settled = false;
    const finish = (error) => {
      if (settled) return;
      settled = true;
      child.removeListener("exit", onExit);
      if (error) rejectHealth(error);
      else resolveHealth();
    };
    const onExit = (code, signal) => {
      finish(
        new Error(
          `Runtime exited before becoming ready (code=${code}, signal=${signal})`
        )
      );
    };
    child.once("exit", onExit);
    const probe = () => {
      if (settled) return;
      if (Date.now() >= deadline) {
        finish(new Error(`Runtime did not become ready on 127.0.0.1:${port}`));
        return;
      }
      const req = node_http.request(
        {
          host: "127.0.0.1",
          port,
          path: "/healthz",
          method: "GET",
          timeout: 500
        },
        (response) => {
          response.resume();
          if (response.statusCode === 200) finish();
          else setTimeout(probe, 100);
        }
      );
      req.on("error", () => setTimeout(probe, 100));
      req.on("timeout", () => {
        req.destroy();
        setTimeout(probe, 100);
      });
      req.end();
    };
    probe();
  });
}
async function stopProcess(child, port) {
  if (!child || child.exitCode !== null || child.killed) return;
  const gracefulAccepted = await requestGracefulShutdown(port);
  if (await waitForExit(child, 5e3)) return;
  if (!isWindows && !gracefulAccepted) {
    child.kill("SIGTERM");
    if (await waitForExit(child, 5e3)) return;
  }
  if (isWindows) {
    await new Promise((resolveStop) => {
      const killer = node_child_process.spawn(
        "taskkill",
        ["/pid", String(child.pid), "/t", "/f"],
        { stdio: "ignore" }
      );
      killer.once("error", () => child?.kill());
      killer.once("exit", () => {
        if (child.exitCode === null) child.kill();
        resolveStop();
      });
    });
    return;
  }
  child.kill("SIGKILL");
}
function requestGracefulShutdown(port) {
  return new Promise((resolveStop) => {
    let answered = false;
    const req = node_http.request(
      {
        host: "127.0.0.1",
        port,
        path: "/shutdown",
        method: "POST",
        timeout: 1e3
      },
      (response) => {
        response.resume();
        answered = true;
        resolveStop(true);
      }
    );
    req.on("error", () => resolveStop(answered));
    req.on("timeout", () => {
      req.destroy();
      resolveStop(answered);
    });
    req.end();
  });
}
function waitForExit(child, timeoutMs) {
  if (child.exitCode !== null) return Promise.resolve(true);
  return new Promise((resolveExit) => {
    const onExit = () => {
      clearTimeout(timer);
      resolveExit(true);
    };
    const timer = setTimeout(() => {
      child.removeListener("exit", onExit);
      resolveExit(false);
    }, timeoutMs);
    child.once("exit", onExit);
  });
}
const isMac = process.platform === "darwin";
class WindowManager {
  constructor() {
    this.window = null;
    this.windowedBounds = null;
    this.gatewayUrl = null;
    this.hoveringComponents = /* @__PURE__ */ new Set();
    this.currentMode = "window";
    this.forceIgnoreMouse = false;
    electron.ipcMain.on("renderer-ready-for-mode-change", (_event, newMode) => {
      if (newMode === "pet") {
        this.continueSetWindowModePet();
      } else {
        this.continueSetWindowModeWindow();
      }
    });
    electron.ipcMain.on("mode-change-rendered", () => {
      this.window?.setOpacity(1);
    });
    electron.screen.on("display-added", () => this.refreshPetBounds());
    electron.screen.on("display-removed", () => this.refreshPetBounds());
    electron.screen.on("display-metrics-changed", () => this.refreshPetBounds());
    electron.ipcMain.on("window-unfullscreen", () => {
      const window = this.getWindow();
      if (window && window.isFullScreen()) {
        window.setFullScreen(false);
      }
    });
    electron.ipcMain.on("toggle-force-ignore-mouse", () => {
      this.toggleForceIgnoreMouse();
    });
  }
  createWindow(options, gatewayUrl) {
    this.gatewayUrl = gatewayUrl ?? null;
    this.window = new electron.BrowserWindow({
      width: 900,
      height: 670,
      show: false,
      transparent: true,
      backgroundColor: "#ffffff",
      autoHideMenuBar: true,
      frame: false,
      icon: process.platform === "win32" ? path.join(__dirname, "../../resources/icon.ico") : path.join(__dirname, "../../resources/icon.png"),
      ...isMac ? { titleBarStyle: "hiddenInset" } : {},
      webPreferences: {
        preload: path.join(__dirname, "../preload/index.js"),
        sandbox: true,
        contextIsolation: true,
        nodeIntegration: false
      },
      hasShadow: false,
      paintWhenInitiallyHidden: true,
      ...options
    });
    this.setupWindowEvents();
    this.loadContent();
    this.window.on("enter-full-screen", () => {
      this.window?.webContents.send("window-fullscreen-change", true);
    });
    this.window.on("leave-full-screen", () => {
      this.window?.webContents.send("window-fullscreen-change", false);
    });
    return this.window;
  }
  setupWindowEvents() {
    if (!this.window) return;
    this.window.on("ready-to-show", () => {
      this.window?.show();
      this.window?.webContents.send(
        "window-maximized-change",
        this.window.isMaximized()
      );
    });
    this.window.on("maximize", () => {
      this.window?.webContents.send("window-maximized-change", true);
    });
    this.window.on("unmaximize", () => {
      this.window?.webContents.send("window-maximized-change", false);
    });
    this.window.on("resize", () => {
      const window = this.getWindow();
      if (window) {
        const bounds = window.getBounds();
        const { width, height } = electron.screen.getPrimaryDisplay().workArea;
        const isMaximized = bounds.width >= width && bounds.height >= height;
        window.webContents.send("window-maximized-change", isMaximized);
      }
    });
    this.window.webContents.on("will-navigate", (event, url) => {
      const isAllowed = utils.is.dev && url.startsWith(process.env.ELECTRON_RENDERER_URL ?? "") || url.startsWith("file:") || this.gatewayUrl !== null && url.startsWith(this.gatewayUrl);
      if (!isAllowed) event.preventDefault();
    });
    this.window.webContents.setWindowOpenHandler((details) => {
      electron.shell.openExternal(details.url);
      return { action: "deny" };
    });
  }
  loadContent() {
    if (!this.window) return;
    if (utils.is.dev && process.env.ELECTRON_RENDERER_URL) {
      this.window.loadURL(process.env.ELECTRON_RENDERER_URL);
    } else if (this.gatewayUrl !== null) {
      this.window.loadURL(this.gatewayUrl);
    } else {
      this.window.loadFile(path.join(__dirname, "../renderer/index.html"));
    }
  }
  setWindowMode(mode) {
    if (!this.window) return;
    this.currentMode = mode;
    this.window.setOpacity(0);
    if (mode === "window") {
      this.setWindowModeWindow();
    } else {
      this.setWindowModePet();
    }
  }
  setWindowModeWindow() {
    if (!this.window) return;
    this.window.setAlwaysOnTop(false);
    this.window.setIgnoreMouseEvents(false);
    this.window.setSkipTaskbar(false);
    this.window.setResizable(true);
    this.window.setFocusable(true);
    this.window.setAlwaysOnTop(false);
    this.window.setBackgroundColor("#ffffff");
    this.window.webContents.send("pre-mode-changed", "window");
  }
  continueSetWindowModeWindow() {
    if (!this.window) return;
    if (this.windowedBounds) {
      this.window.setBounds(this.windowedBounds);
    } else {
      this.window.setSize(900, 670);
      this.window.center();
    }
    if (isMac) {
      this.window.setWindowButtonVisibility(true);
      this.window.setVisibleOnAllWorkspaces(false, {
        visibleOnFullScreen: false
      });
    }
    this.window?.setIgnoreMouseEvents(false, { forward: true });
    this.window.webContents.send("mode-changed", "window");
  }
  setWindowModePet() {
    if (!this.window) return;
    this.windowedBounds = this.window.getBounds();
    if (this.window.isFullScreen()) {
      this.window.setFullScreen(false);
    }
    this.window.setBackgroundColor("#00000000");
    this.window.setAlwaysOnTop(true, "screen-saver");
    this.window.setPosition(0, 0);
    this.window.webContents.send("pre-mode-changed", "pet");
  }
  continueSetWindowModePet() {
    if (!this.window) return;
    const displays = electron.screen.getAllDisplays();
    const minX = Math.min(...displays.map((d) => d.bounds.x));
    const minY = Math.min(...displays.map((d) => d.bounds.y));
    const maxX = Math.max(...displays.map((d) => d.bounds.x + d.bounds.width));
    const maxY = Math.max(...displays.map((d) => d.bounds.y + d.bounds.height));
    const combinedWidth = maxX - minX;
    const combinedHeight = maxY - minY;
    this.window.setBounds({
      x: minX,
      y: minY,
      width: combinedWidth,
      height: combinedHeight
    });
    if (isMac) {
      this.window.setWindowButtonVisibility(false);
    }
    this.window.setResizable(false);
    this.window.setSkipTaskbar(true);
    this.window.setFocusable(false);
    if (isMac) {
      this.window.setIgnoreMouseEvents(true);
      this.window.setVisibleOnAllWorkspaces(true, {
        visibleOnFullScreen: true
      });
    } else {
      this.window.setIgnoreMouseEvents(true, { forward: true });
    }
    this.window.webContents.send("mode-changed", "pet");
  }
  /**
   * Recomputes the pet-mode window bounds on display changes (add/remove/
   * resolution change). No-op outside pet mode. The window keeps its
   * properties; only the cross-display bounds are refreshed.
   */
  refreshPetBounds() {
    if (this.currentMode !== "pet" || !this.window) return;
    const displays = electron.screen.getAllDisplays();
    if (displays.length === 0) return;
    const minX = Math.min(...displays.map((d) => d.bounds.x));
    const minY = Math.min(...displays.map((d) => d.bounds.y));
    const maxX = Math.max(...displays.map((d) => d.bounds.x + d.bounds.width));
    const maxY = Math.max(...displays.map((d) => d.bounds.y + d.bounds.height));
    this.window.setBounds({
      x: minX,
      y: minY,
      width: maxX - minX,
      height: maxY - minY
    });
  }
  getWindow() {
    return this.window;
  }
  setIgnoreMouseEvents(ignore) {
    if (!this.window) return;
    if (isMac) {
      this.window.setIgnoreMouseEvents(ignore);
    } else {
      this.window.setIgnoreMouseEvents(ignore, { forward: true });
    }
  }
  maximizeWindow() {
    if (!this.window) return;
    if (this.isWindowMaximized()) {
      if (this.windowedBounds) {
        this.window.setBounds(this.windowedBounds);
        this.windowedBounds = null;
        this.window.webContents.send("window-maximized-change", false);
      }
    } else {
      this.windowedBounds = this.window.getBounds();
      const { width, height } = electron.screen.getPrimaryDisplay().workArea;
      this.window.setBounds({
        x: 0,
        y: 0,
        width,
        height
      });
      this.window.webContents.send("window-maximized-change", true);
    }
  }
  isWindowMaximized() {
    if (!this.window) return false;
    const bounds = this.window.getBounds();
    const { width, height } = electron.screen.getPrimaryDisplay().workArea;
    return bounds.width >= width && bounds.height >= height;
  }
  updateComponentHover(componentId, isHovering) {
    if (this.currentMode === "window") return;
    if (this.forceIgnoreMouse) return;
    if (isHovering) {
      this.hoveringComponents.add(componentId);
    } else {
      this.hoveringComponents.delete(componentId);
    }
    if (this.window) {
      const shouldIgnore = this.hoveringComponents.size === 0;
      if (isMac) {
        this.window.setIgnoreMouseEvents(shouldIgnore);
      } else {
        this.window.setIgnoreMouseEvents(shouldIgnore, { forward: true });
      }
      if (!shouldIgnore) {
        this.window.setFocusable(true);
      }
    }
  }
  // Toggle force ignore mouse events
  toggleForceIgnoreMouse() {
    this.forceIgnoreMouse = !this.forceIgnoreMouse;
    if (this.forceIgnoreMouse) {
      if (isMac) {
        this.window?.setIgnoreMouseEvents(true);
      } else {
        this.window?.setIgnoreMouseEvents(true, { forward: true });
      }
    } else {
      const shouldIgnore = this.hoveringComponents.size === 0;
      if (isMac) {
        this.window?.setIgnoreMouseEvents(shouldIgnore);
      } else {
        this.window?.setIgnoreMouseEvents(shouldIgnore, { forward: true });
      }
    }
    this.window?.webContents.send(
      "force-ignore-mouse-changed",
      this.forceIgnoreMouse
    );
  }
  // Get current force ignore state
  isForceIgnoreMouse() {
    return this.forceIgnoreMouse;
  }
  // Get current mode
  getCurrentMode() {
    return this.currentMode;
  }
}
const trayIcon = path.join(__dirname, "../../resources/icon.png");
class MenuManager {
  constructor(onModeChange) {
    this.onModeChange = onModeChange;
    this.tray = null;
    this.currentMode = "window";
    this.configFiles = [];
    this.setupContextMenu();
  }
  createTray() {
    const icon = electron.nativeImage.createFromPath(trayIcon);
    const trayIconResized = icon.resize({
      width: process.platform === "win32" ? 16 : 18,
      height: process.platform === "win32" ? 16 : 18
    });
    this.tray = new electron.Tray(trayIconResized);
    this.updateTrayMenu();
  }
  getModeMenuItems() {
    return [
      {
        label: "Window Mode",
        type: "radio",
        checked: this.currentMode === "window",
        click: () => {
          this.setMode("window");
        }
      },
      {
        label: "Pet Mode",
        type: "radio",
        checked: this.currentMode === "pet",
        click: () => {
          this.setMode("pet");
        }
      }
    ];
  }
  updateTrayMenu() {
    if (!this.tray) return;
    const contextMenu = electron.Menu.buildFromTemplate([
      ...this.getModeMenuItems(),
      { type: "separator" },
      // Only show toggle mouse ignore in pet mode
      ...this.currentMode === "pet" ? [
        {
          label: "Toggle Mouse Passthrough",
          click: () => {
            const windows = electron.BrowserWindow.getAllWindows();
            windows.forEach((window) => {
              window.webContents.send("toggle-force-ignore-mouse");
            });
          }
        },
        { type: "separator" }
      ] : [],
      {
        label: "Show",
        click: () => {
          const windows = electron.BrowserWindow.getAllWindows();
          windows.forEach((window) => {
            window.show();
          });
        }
      },
      {
        label: "Hide",
        click: () => {
          const windows = electron.BrowserWindow.getAllWindows();
          windows.forEach((window) => {
            window.hide();
          });
        }
      },
      {
        label: "Exit",
        click: () => {
          electron.app.quit();
        }
      }
    ]);
    this.tray.setToolTip("Open LLM VTuber");
    this.tray.setContextMenu(contextMenu);
  }
  getContextMenuItems(event) {
    const template = [
      {
        label: "Toggle Microphone",
        click: () => {
          event.sender.send("mic-toggle");
        }
      },
      {
        label: "Interrupt",
        click: () => {
          event.sender.send("interrupt");
        }
      },
      { type: "separator" },
      // Only show in pet mode
      ...this.currentMode === "pet" ? [
        {
          label: "Toggle Mouse Passthrough",
          click: () => {
            event.sender.send("toggle-force-ignore-mouse");
          }
        }
      ] : [],
      {
        label: "Toggle Scrolling to Resize",
        click: () => {
          event.sender.send("toggle-scroll-to-resize");
        }
      },
      // Only show this item in pet mode
      ...this.currentMode === "pet" ? [
        {
          label: "Toggle InputBox and Subtitle",
          click: () => {
            event.sender.send("toggle-input-subtitle");
          }
        }
      ] : [],
      { type: "separator" },
      ...this.getModeMenuItems(),
      { type: "separator" },
      {
        label: "Switch Character",
        visible: this.currentMode === "pet",
        submenu: this.configFiles.map((config) => ({
          label: config.name,
          click: () => {
            event.sender.send("switch-character", config.filename);
          }
        }))
      },
      { type: "separator" },
      {
        label: "Hide",
        click: () => {
          const windows = electron.BrowserWindow.getAllWindows();
          windows.forEach((window) => {
            window.hide();
          });
        }
      },
      {
        label: "Exit",
        click: () => {
          electron.app.quit();
        }
      }
    ];
    return template;
  }
  setupContextMenu() {
    electron.ipcMain.on("show-context-menu", (event) => {
      const win = electron.BrowserWindow.fromWebContents(event.sender);
      if (win) {
        const screenPoint = electron.screen.getCursorScreenPoint();
        const menu = electron.Menu.buildFromTemplate(this.getContextMenuItems(event));
        menu.popup({
          window: win,
          x: Math.round(screenPoint.x),
          y: Math.round(screenPoint.y)
        });
      }
    });
  }
  setMode(mode) {
    this.currentMode = mode;
    this.updateTrayMenu();
    this.onModeChange(mode);
  }
  destroy() {
    this.tray?.destroy();
    this.tray = null;
  }
  updateConfigFiles(files) {
    this.configFiles = files;
  }
}
let windowManager;
let menuManager;
let isQuitting = false;
let runtimeSupervisor = null;
let runtimeStopping = false;
function setupIPC() {
  electron.ipcMain.handle("get-platform", () => process.platform);
  electron.ipcMain.on("set-ignore-mouse-events", (_event, ignore) => {
    const window = windowManager.getWindow();
    if (window) {
      windowManager.setIgnoreMouseEvents(ignore);
    }
  });
  electron.ipcMain.on("get-current-mode", (event) => {
    event.returnValue = windowManager.getCurrentMode();
  });
  electron.ipcMain.on("pre-mode-changed", (_event, newMode) => {
    if (newMode === "window" || newMode === "pet") {
      menuManager.setMode(newMode);
    }
  });
  electron.ipcMain.on("window-minimize", () => {
    windowManager.getWindow()?.minimize();
  });
  electron.ipcMain.on("window-maximize", () => {
    const window = windowManager.getWindow();
    if (window) {
      windowManager.maximizeWindow();
    }
  });
  electron.ipcMain.on("window-close", () => {
    const window = windowManager.getWindow();
    if (window) {
      if (process.platform === "darwin") {
        window.hide();
      } else {
        window.close();
      }
    }
  });
  electron.ipcMain.on(
    "update-component-hover",
    (_event, componentId, isHovering) => {
      windowManager.updateComponentHover(componentId, isHovering);
    }
  );
  electron.ipcMain.on("update-config-files", (_event, files) => {
    menuManager.updateConfigFiles(files);
  });
  electron.ipcMain.handle("get-screen-capture", async () => {
    const sources = await electron.desktopCapturer.getSources({ types: ["screen"] });
    return sources[0].id;
  });
}
electron.app.whenReady().then(async () => {
  utils.electronApp.setAppUserModelId("com.electron");
  const sourceRoot = process.env.OLV_PROJECT_ROOT ?? node_path.resolve(electron.app.getAppPath(), "../..");
  const packaged = electron.app.isPackaged;
  runtimeSupervisor = new RuntimeSupervisor({
    projectRoot: sourceRoot,
    resourcesPath: process.resourcesPath,
    assetRoot: packaged ? process.resourcesPath : sourceRoot,
    dataDir: electron.app.getPath("userData"),
    pythonProjectRoot: process.env.OLV_PYTHON_PROJECT_ROOT ?? sourceRoot,
    startPython: process.env.OLV_DESKTOP_START_PYTHON === "1",
    startRust: process.env.OLV_DESKTOP_RUNTIME !== "0"
  });
  try {
    await runtimeSupervisor.start();
  } catch (error) {
    console.error("Failed to start Open-LLM-VTuber Runtime:", error);
    await runtimeSupervisor.stop();
    electron.app.quit();
    return;
  }
  windowManager = new WindowManager();
  menuManager = new MenuManager((mode) => windowManager.setWindowMode(mode));
  const window = windowManager.createWindow(
    {
      titleBarOverlay: {
        color: "#111111",
        symbolColor: "#FFFFFF",
        height: 30
      }
    },
    runtimeSupervisor.gatewayUrl
  );
  menuManager.createTray();
  window.on("close", (event) => {
    if (!isQuitting) {
      event.preventDefault();
      window.hide();
    }
    return false;
  });
  setupIPC();
  electron.app.on("activate", () => {
    const window2 = windowManager.getWindow();
    if (window2) {
      window2.show();
    }
  });
  electron.app.on("browser-window-created", (_, window2) => {
    utils.optimizer.watchWindowShortcuts(window2);
  });
  electron.app.on("web-contents-created", (_, contents) => {
    contents.session.setPermissionRequestHandler(
      (webContents, permission, callback) => {
        if (permission === "media" && webContents === windowManager?.getWindow()?.webContents) {
          callback(true);
        } else {
          callback(false);
        }
      }
    );
  });
});
electron.app.on("window-all-closed", () => {
  if (process.platform !== "darwin") {
    electron.app.quit();
  }
});
electron.app.on("before-quit", (event) => {
  if (runtimeSupervisor && !runtimeStopping) {
    event.preventDefault();
    runtimeStopping = true;
    void runtimeSupervisor.stop().finally(() => electron.app.quit());
  }
  isQuitting = true;
  menuManager?.destroy();
  electron.globalShortcut.unregisterAll();
});
