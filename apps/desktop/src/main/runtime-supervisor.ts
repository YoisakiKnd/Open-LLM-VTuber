import { spawn, type ChildProcess } from "node:child_process";
import { existsSync, mkdirSync } from "node:fs";
import { request } from "node:http";
import { homedir } from "node:os";
import { delimiter, join, resolve } from "node:path";

const isWindows = process.platform === "win32";

export interface RuntimeSupervisorOptions {
  projectRoot: string;
  resourcesPath: string;
  assetRoot?: string;
  dataDir?: string;
  pythonProjectRoot?: string;
  rustBinary?: string;
  pythonExecutable?: string;
  startPython?: boolean;
  startRust?: boolean;
  rustPort?: number;
  pythonPort?: number;
}

export class RuntimeSupervisor {
  private rustProcess: ChildProcess | null = null;
  private pythonProcess: ChildProcess | null = null;
  private stopPromise: Promise<void> | null = null;

  constructor(private readonly options: RuntimeSupervisorOptions) {}

  async start(): Promise<void> {
    if (
      this.options.startPython ??
      process.env.OLV_DESKTOP_START_PYTHON === "1"
    ) {
      this.pythonProcess = this.spawnPython();
      await waitForHealth(this.pythonPort, this.pythonProcess, 30_000);
    }

    if (this.options.startRust ?? process.env.OLV_DESKTOP_RUNTIME !== "0") {
      const binary = this.resolveRustBinary();
      if (!existsSync(binary)) {
        throw new Error(
          `Rust Gateway binary not found at ${binary}; build the release runtime or set OLV_RUST_GATEWAY_BINARY`,
        );
      }
      this.rustProcess = spawn(binary, this.rustArguments, {
        cwd: this.options.projectRoot,
        env: this.runtimeEnvironment,
        stdio: "inherit",
        // On Windows the gateway is a console executable; hide its console
        // window when spawned from the GUI process.
        windowsHide: true,
      });
      await waitForHealth(this.rustPort, this.rustProcess, 15_000);
    }
  }

  stop(): Promise<void> {
    if (this.stopPromise !== null) return this.stopPromise;
    this.stopPromise = (async () => {
      await stopProcess(this.rustProcess, this.rustPort);
      await stopProcess(this.pythonProcess, this.pythonPort);
      this.rustProcess = null;
      this.pythonProcess = null;
    })();
    return this.stopPromise;
  }

  private get rustPort(): number {
    return this.options.rustPort ?? 12394;
  }

  /**
   * Origin URL of the gateway's static frontend. In packaged builds the
   * renderer is loaded from here (same-origin WebSocket + fetch, no CORS).
   */
  get gatewayUrl(): string {
    return `http://127.0.0.1:${this.rustPort}`;
  }

  private get pythonPort(): number {
    return this.options.pythonPort ?? 12393;
  }

  private get assetRoot(): string {
    return this.options.assetRoot ?? this.options.projectRoot;
  }

  private get dataDir(): string {
    return this.options.dataDir ?? this.options.projectRoot;
  }

  private get pythonProjectRoot(): string {
    return this.options.pythonProjectRoot ?? this.options.projectRoot;
  }

  private get runtimeEnvironment(): NodeJS.ProcessEnv {
    return {
      ...process.env,
      OLV_RUST_GATEWAY_BINARY: undefined,
      OLV_SKIP_RUST_GATEWAY: this.pythonProcess
        ? "1"
        : process.env.OLV_SKIP_RUST_GATEWAY,
    };
  }

  private get rustArguments(): string[] {
    const root = this.assetRoot;
    const legacyConfig = existsSync(join(this.dataDir, "conf.yaml"))
      ? join(this.dataDir, "conf.yaml")
      : join(root, "conf.yaml");
    const cacheDirectory = this.pythonProcess
      ? join(this.pythonProjectRoot, "cache")
      : join(this.dataDir, "cache");
    mkdirSync(cacheDirectory, { recursive: true });
    return [
      "--listen",
      `127.0.0.1:${this.rustPort}`,
      "--python-ws-url",
      `ws://127.0.0.1:${this.pythonPort}/client-ws`,
      ...(this.pythonProcess ? [] : ["--allow-missing-python"]),
      "--frontend-dir",
      join(root, "frontend"),
      "--cache-dir",
      cacheDirectory,
      "--live2d-models-dir",
      join(root, "live2d-models"),
      "--backgrounds-dir",
      join(root, "backgrounds"),
      "--avatars-dir",
      join(root, "avatars"),
      "--web-tool-dir",
      join(root, "web_tool"),
      "--settings-file",
      join(this.dataDir, "settings.v1.json"),
      "--legacy-config-file",
      legacyConfig,
      "--legacy-characters-dir",
      join(root, "characters"),
      "--model-dict-file",
      join(root, "model_dict.json"),
      "--allowed-origins",
      `http://127.0.0.1:${this.rustPort},http://localhost:${this.rustPort},null,http://localhost:3000,http://127.0.0.1:3000`,
    ];
  }

  private resolveRustBinary(): string {
    if (this.options.rustBinary) return this.options.rustBinary;
    if (process.env.OLV_RUST_GATEWAY_BINARY) {
      return process.env.OLV_RUST_GATEWAY_BINARY;
    }
    const executableName = isWindows ? "rust-gateway.exe" : "rust-gateway";
    const packaged = join(
      this.options.resourcesPath,
      "runtime",
      executableName,
    );
    if (existsSync(packaged)) return packaged;
    const developmentName = isWindows
      ? "open-llm-vtuber-gateway.exe"
      : "open-llm-vtuber-gateway";
    return resolve(
      this.options.projectRoot,
      "rust-gateway/target/release",
      developmentName,
    );
  }

  private spawnPython(): ChildProcess {
    const uvName = isWindows ? "uv.exe" : "uv";
    const executable =
      this.options.pythonExecutable ??
      process.env.OLV_PYTHON_EXECUTABLE ??
      join(homedir(), ".local", "bin", uvName);
    const command = existsSync(executable) ? executable : uvName;
    const root = this.pythonProjectRoot;
    const venvBin = join(root, ".venv", isWindows ? "Scripts" : "bin");
    const processEnvironment = {
      ...process.env,
      OLV_SKIP_RUST_GATEWAY: "1",
      PATH: [venvBin, process.env.PATH].filter(Boolean).join(delimiter),
    };
    return spawn(command, ["run", "--project", root, "run_server.py"], {
      cwd: root,
      env: processEnvironment,
      stdio: "inherit",
      windowsHide: true,
    });
  }
}

function waitForHealth(
  port: number,
  child: ChildProcess,
  timeoutMs: number,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  return new Promise((resolveHealth, rejectHealth) => {
    let settled = false;
    const finish = (error?: Error) => {
      if (settled) return;
      settled = true;
      child.removeListener("exit", onExit);
      if (error) rejectHealth(error);
      else resolveHealth();
    };
    const onExit = (code: number | null, signal: NodeJS.Signals | null) => {
      finish(
        new Error(
          `Runtime exited before becoming ready (code=${code}, signal=${signal})`,
        ),
      );
    };
    child.once("exit", onExit);

    const probe = () => {
      if (settled) return;
      if (Date.now() >= deadline) {
        finish(new Error(`Runtime did not become ready on 127.0.0.1:${port}`));
        return;
      }
      const req = request(
        {
          host: "127.0.0.1",
          port,
          path: "/healthz",
          method: "GET",
          timeout: 500,
        },
        (response) => {
          response.resume();
          if (response.statusCode === 200) finish();
          else setTimeout(probe, 100);
        },
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

async function stopProcess(
  child: ChildProcess | null,
  port: number,
): Promise<void> {
  if (!child || child.exitCode !== null || child.killed) return;
  // Graceful shutdown first: POST /shutdown works on every platform
  // (Windows has no SIGTERM semantics). Fall back progressively.
  const gracefulAccepted = await requestGracefulShutdown(port);
  if (await waitForExit(child, 5_000)) return;
  if (!isWindows && !gracefulAccepted) {
    // The child has no /shutdown endpoint (e.g. the Python sidecar):
    // give it a SIGTERM so it can clean up before we force-kill.
    child.kill("SIGTERM");
    if (await waitForExit(child, 5_000)) return;
  }
  if (isWindows) {
    // Force-terminate the whole tree; `taskkill` reaps children too.
    await new Promise<void>((resolveStop) => {
      const killer = spawn(
        "taskkill",
        ["/pid", String(child.pid), "/t", "/f"],
        { stdio: "ignore" },
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

/** Best-effort `POST /shutdown`; resolves `true` when the endpoint answered. */
function requestGracefulShutdown(port: number): Promise<boolean> {
  return new Promise((resolveStop) => {
    let answered = false;
    const req = request(
      {
        host: "127.0.0.1",
        port,
        path: "/shutdown",
        method: "POST",
        timeout: 1_000,
      },
      (response) => {
        response.resume();
        answered = true;
        resolveStop(true);
      },
    );
    req.on("error", () => resolveStop(answered));
    req.on("timeout", () => {
      req.destroy();
      resolveStop(answered);
    });
    req.end();
  });
}

/** Resolves `true` when the child exits within `timeoutMs`. */
function waitForExit(child: ChildProcess, timeoutMs: number): Promise<boolean> {
  if (child.exitCode !== null) return Promise.resolve(true);
  return new Promise<boolean>((resolveExit) => {
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
