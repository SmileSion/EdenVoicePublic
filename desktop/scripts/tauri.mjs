import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { createServer } from "node:net";
import { fileURLToPath } from "node:url";

const requestedArgs = process.argv.slice(2);
const tauriScript = fileURLToPath(new URL("../node_modules/@tauri-apps/cli/tauri.js", import.meta.url));

function tauriEnvironment() {
  const environment = { ...process.env };
  const localBuildTools = fileURLToPath(new URL("../../.build/meson-venv/bin", import.meta.url));
  if (existsSync(`${localBuildTools}/meson`)) {
    environment.PATH = `${localBuildTools}${process.platform === "win32" ? ";" : ":"}${environment.PATH ?? ""}`;
  }
  environment.CMAKE_POLICY_VERSION_MINIMUM ??= "3.5";
  if (requestedArgs[0] === "build" && !environment.TAURI_SIGNING_PRIVATE_KEY) {
    const key = fileURLToPath(new URL("../../.release-keys/updater.key", import.meta.url));
    if (existsSync(key)) {
      environment.TAURI_SIGNING_PRIVATE_KEY = key;
      environment.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ??= "";
    }
  }

  // bindgen 0.72 can panic while parsing WebRTC C++ headers with newer
  // Homebrew LLVM releases. Apple's libclang is compatible with the crate and
  // is also what the native macOS compiler uses. Preserve an explicit override.
  const appleLibclang = "/Library/Developer/CommandLineTools/usr/lib";
  if (process.platform === "darwin" && !environment.LIBCLANG_PATH && existsSync(`${appleLibclang}/libclang.dylib`)) {
    environment.LIBCLANG_PATH = appleLibclang;
  }
  return environment;
}

function portIsAvailable(port) {
  return new Promise((resolve) => {
    const server = createServer();
    server.unref();
    server.once("error", () => resolve(false));
    server.listen({ host: "127.0.0.1", port, exclusive: true }, () => {
      server.close(() => resolve(true));
    });
  });
}

async function findAvailablePort(start, attempts = 100) {
  for (let port = start; port < start + attempts; port += 1) {
    if (await portIsAvailable(port)) return port;
  }
  throw new Error(`没有在 ${start}-${start + attempts - 1} 范围内找到可用的开发端口`);
}

async function main() {
  const [command] = requestedArgs;
  let forwardedArgs = requestedArgs;

  if (command === "dev") {
    const port = await findAvailablePort(1420);
    const override = JSON.stringify({
      build: {
        beforeDevCommand: `npm run dev -- --port ${port} --strictPort`,
        devUrl: `http://127.0.0.1:${port}`,
      },
    });
    console.log(`[EdenVoice] 使用开发端口 ${port}`);
    forwardedArgs = ["dev", "--config", override, ...requestedArgs.slice(1)];
  }

  // Invoke the JavaScript entry point through Node on every platform. Directly
  // spawning node_modules/.bin/tauri.cmd is unreliable on Windows runners
  // unless a shell is involved, and previously made release builds fail before
  // Tauri itself started.
  const child = spawn(process.execPath, [tauriScript, ...forwardedArgs], {
    env: tauriEnvironment(),
    stdio: "inherit",
  });

  for (const signal of ["SIGINT", "SIGTERM"]) {
    process.once(signal, () => child.kill(signal));
  }

  child.once("error", (error) => {
    console.error(`[EdenVoice] 无法启动 Tauri CLI: ${error.message}`);
    process.exitCode = 1;
  });
  child.once("exit", (code, signal) => {
    process.exitCode = signal ? 1 : (code ?? 1);
  });
}

main().catch((error) => {
  console.error(`[EdenVoice] ${error.message}`);
  process.exitCode = 1;
});
