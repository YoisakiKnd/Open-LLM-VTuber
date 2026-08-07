import { chmod, cp, mkdir, rm, stat } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const desktopRoot = resolve(scriptDirectory, "..");
const repositoryRoot = resolve(desktopRoot, "../..");
const executableExtension = process.platform === "win32" ? ".exe" : "";
const source = resolve(
  repositoryRoot,
  `rust-gateway/target/release/open-llm-vtuber-gateway${executableExtension}`,
);
const destinationDirectory = resolve(desktopRoot, "resources/runtime");
const destination = resolve(
  destinationDirectory,
  `rust-gateway${executableExtension}`,
);

try {
  const metadata = await stat(source);
  if (!metadata.isFile()) throw new Error("not a file");
} catch {
  throw new Error(
    `Rust Runtime not found at ${source}. Run cargo build --release --locked first.`,
  );
}

await rm(destinationDirectory, { recursive: true, force: true });
await mkdir(destinationDirectory, { recursive: true });
await cp(source, destination);
if (process.platform !== "win32") await chmod(destination, 0o755);
console.log(`Prepared Rust Runtime: ${destination}`);
