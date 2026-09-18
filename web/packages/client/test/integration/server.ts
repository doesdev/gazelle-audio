// Starts the real gazelle-audio-server for integration tests (decision P8). The server is built
// with cargo once per process, then the built binary is spawned directly, not through
// `cargo run`, so killing it stops the server itself. It binds port 0 and the tests read the
// port it logs. It runs with --no-tray, or every test server would add an icon to the taskbar.
// stop() kills it and waits for it to exit; on Windows kill() terminates the process outright,
// which loses nothing under --no-persist. A process exit handler kills any server a crashed or
// interrupted test run leaves behind.

import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

export const REPO_ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "../../../../..");
// CARGO_TARGET_DIR is honoured as cargo honours it (relative to the repository root, where cargo runs),
// so the tests can build into their own folder while a server someone is using holds target/debug.
const TARGET = resolve(REPO_ROOT, process.env["CARGO_TARGET_DIR"] ?? "target");
const BINARY = join(TARGET, "debug", process.platform === "win32" ? "gazelle-audio-server.exe" : "gazelle-audio-server");
const START_TIMEOUT_MS = 30_000;
const STOP_TIMEOUT_MS = 10_000;

export interface RunningServer {
  readonly url: string;
  readonly port: number;
  readonly child: ChildProcess;
  /** Everything the server printed so far. */
  readonly output: string[];
  stop(): Promise<void>;
}

const live = new Set<ChildProcess>();
process.on("exit", () => {
  for (const child of live) child.kill();
});

let built = false;

function cargo(): string {
  if (process.env["CARGO"]) return process.env["CARGO"];
  const local = join(homedir(), ".cargo", "bin", process.platform === "win32" ? "cargo.exe" : "cargo");
  return existsSync(local) ? local : "cargo";
}

/** Builds the server once per test process. */
export function buildServer(): string {
  if (built) return BINARY;
  const result = spawnSync(cargo(), ["build", "-p", "gazelle-audio-server"], { cwd: REPO_ROOT, encoding: "utf8" });
  if (result.status !== 0) throw new Error(`cargo build -p gazelle-audio-server failed (${result.error?.message ?? `exit ${result.status}`}):\n${result.stderr}`);
  built = true;
  return BINARY;
}

const ANSI = /\x1b\[[0-9;]*m/g;

export interface StartOptions {
  /** Serve the embedded web UI at `/` (browser tests); API-only by default. */
  webUi?: boolean;
}

/**
 * The variable that makes the server refuse the USB backend outright. Set on every server this
 * harness starts, because `--backend` now defaults to `usb` (decision 0018) and a test suite must
 * not be one forgotten flag away from opening the devices on the machine running it.
 */
export const NO_HARDWARE = "GAZELLE_NO_HARDWARE";

/** The environment every server started here runs in: this process's, plus the refusal. */
export function childEnv(): NodeJS.ProcessEnv {
  return { ...process.env, [NO_HARDWARE]: "1" };
}

export async function startServer(extraArgs: readonly string[] = [], options: StartOptions = {}): Promise<RunningServer> {
  // Named, never assumed. The harness could quietly supply `--backend loopback` and every caller
  // would be safe, but then "which backend is this server?" would be a fact about the harness
  // rather than about the test, and the first suite that wanted something else would inherit a
  // default it never saw. GAZELLE_NO_HARDWARE below is the guard; this is the rule the guard
  // backs up.
  if (!extraArgs.includes("--backend")) {
    throw new Error("startServer needs an explicit backend: pass [\"--backend\", \"loopback\", ...]. --backend defaults to usb, which would open real devices.");
  }
  const binary = buildServer();
  const args = ["--bind", "127.0.0.1:0", "--no-persist", "--no-tray", ...(options.webUi ? [] : ["--no-web-ui"]), ...extraArgs];
  const child = spawn(binary, args, { cwd: REPO_ROOT, stdio: ["ignore", "pipe", "pipe"], env: childEnv() });
  live.add(child);
  const exited = new Promise<void>((done) => child.once("exit", () => done()));
  exited.then(() => live.delete(child));
  const output: string[] = [];

  const port = await new Promise<number>((found, fail) => {
    const timer = setTimeout(() => fail(new Error(`the server did not report its port within ${START_TIMEOUT_MS} ms:\n${output.join("\n")}`)), START_TIMEOUT_MS);
    const onLine = (raw: string) => {
      const line = raw.replace(ANSI, "");
      output.push(line);
      const match = /listening on http:\/\/127\.0\.0\.1:(\d+)/.exec(line);
      if (match?.[1] !== undefined) {
        clearTimeout(timer);
        found(Number(match[1]));
      }
    };
    for (const stream of [child.stdout, child.stderr]) {
      if (stream !== null) createInterface({ input: stream }).on("line", onLine);
    }
    child.once("exit", (code) => {
      clearTimeout(timer);
      fail(new Error(`the server exited (${code}) before reporting its port:\n${output.join("\n")}`));
    });
  }).catch(async (error: unknown) => {
    child.kill();
    await exited;
    throw error;
  });

  return {
    url: `http://127.0.0.1:${port}`,
    port,
    child,
    output,
    async stop() {
      if (child.exitCode !== null || child.signalCode !== null) return;
      child.kill();
      let timer: NodeJS.Timeout | undefined;
      const late = new Promise<never>((_, fail) => {
        timer = setTimeout(() => fail(new Error(`the server did not exit within ${STOP_TIMEOUT_MS} ms of being killed`)), STOP_TIMEOUT_MS);
      });
      try {
        await Promise.race([exited, late]);
      } finally {
        clearTimeout(timer);
      }
    },
  };
}
