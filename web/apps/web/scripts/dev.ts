// `pnpm -C web dev`: the control server (loopback devices pushing cyclic reports every 50 ms,
// workspace in memory) and Vite, which proxies /api and the WebSocket to it. Ctrl-C stops both.

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { connect } from "node:net";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { createServer } from "vite";

import { SERVER_PORT } from "../vite.config.ts";

const app = fileURLToPath(new URL("..", import.meta.url));
const repo = resolve(app, "../../..");

function cargo(): string {
  if (process.env["CARGO"]) return process.env["CARGO"];
  const local = join(homedir(), ".cargo", "bin", process.platform === "win32" ? "cargo.exe" : "cargo");
  return existsSync(local) ? local : "cargo";
}

const server = spawn(
  cargo(),
  ["run", "-p", "gazelle-audio-server", "--", "--bind", `127.0.0.1:${SERVER_PORT}`, "--no-persist", "--no-web-ui", "--no-tray", "--loopback-cyclic-ms", "50"],
  // A separate target directory: Windows locks a running executable, so a dev server running from
  // target/debug would make every `cargo build` and test run meanwhile fail to replace it.
  { cwd: repo, stdio: "inherit", env: { ...process.env, CARGO_TARGET_DIR: join(repo, "target", "dev") } },
);
server.on("exit", (code) => {
  console.error(`gazelle-audio-server exited (${code ?? "signal"})`);
  process.exit(code ?? 1);
});

// The first `cargo run` compiles the server; start Vite only once it accepts connections, so the
// page never boots against a proxy with nothing behind it.
const listening = (): Promise<boolean> =>
  new Promise((resolve) => {
    const socket = connect({ host: "127.0.0.1", port: SERVER_PORT });
    socket.once("connect", () => {
      socket.destroy();
      resolve(true);
    });
    socket.once("error", () => resolve(false));
  });
const deadline = Date.now() + 180_000;
while (!(await listening())) {
  if (Date.now() > deadline) {
    console.error(`gazelle-audio-server did not accept connections on port ${SERVER_PORT} within 3 minutes`);
    server.kill();
    process.exit(1);
  }
  await new Promise((resolve) => setTimeout(resolve, 250));
}

const vite = await createServer({ configFile: join(app, "vite.config.ts") });
await vite.listen();
vite.printUrls();

const stop = async () => {
  server.kill();
  await vite.close();
  process.exit(0);
};
process.on("SIGINT", stop);
process.on("SIGTERM", stop);
