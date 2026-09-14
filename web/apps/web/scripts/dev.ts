// `pnpm -C web dev`: the control server (loopback devices pushing cyclic reports every 50 ms,
// workspace in memory) and Vite, which proxies /api and the WebSocket to it. Ctrl-C stops both.

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
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
  ["run", "-p", "gazelle-audio-server", "--", "--bind", `127.0.0.1:${SERVER_PORT}`, "--no-persist", "--no-web-ui", "--loopback-cyclic-ms", "50"],
  { cwd: repo, stdio: "inherit" },
);
server.on("exit", (code) => {
  console.error(`gazelle-audio-server exited (${code ?? "signal"})`);
  process.exit(code ?? 1);
});

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
