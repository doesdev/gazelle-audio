// Spec §10 row 2: the `npm pack` tarball imports and connects from a bare project. The package is
// built, packed and installed into an empty temporary project (it has no dependencies, so no
// network is needed), then a plain .mjs script connects to the real server and invokes a
// command, and a consumer .ts file typechecks against the installed declarations. Node 22 runs
// when GAZELLE_NODE22 names its executable; the current Node always runs.

import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { REPO_ROOT, startServer, type RunningServer } from "./server.ts";

const CLIENT = join(REPO_ROOT, "web", "packages", "client");
const TSC = join(REPO_ROOT, "web", "node_modules", "typescript", "bin", "tsc");
const NODE22 = process.env["GAZELLE_NODE22"];

let server: RunningServer;
let project: string;

function run(command: string, args: readonly string[], cwd: string): string {
  // npm is a .cmd shim on Windows, which Node only spawns through a shell.
  const shell = process.platform === "win32" && command === "npm";
  const result = spawnSync(command, args, { cwd, encoding: "utf8", shell });
  if (result.status !== 0) throw new Error(`${command} ${args.join(" ")} failed (${result.error?.message ?? `exit ${result.status}`}):\n${result.stdout}\n${result.stderr}`);
  return result.stdout;
}

before(async () => {
  project = mkdtempSync(join(tmpdir(), "gazelle-client-pack-"));
  run(process.execPath, [TSC, "-p", "tsconfig.build.json"], CLIENT);
  run("npm", ["pack", "--pack-destination", project], CLIENT);
  const tarball = readdirSync(project).find((name) => name.startsWith("gazelle-audio-client-") && name.endsWith(".tgz"));
  assert.ok(tarball, "npm pack wrote a tarball");
  writeFileSync(join(project, "package.json"), JSON.stringify({ name: "bare-consumer", private: true, type: "module" }));
  run("npm", ["install", "--no-audit", "--no-fund", `./${tarball}`], project);
  writeFileSync(
    join(project, "smoke.mjs"),
    [
      'import { connect, GazelleError } from "gazelle-audio-client";',
      "const client = await connect(process.argv[2]);",
      'const dev = client.device("loopback-0");',
      'const result = await dev.invoke("set_volume", { id: 1, volume: 64 }, { dryRun: true });',
      "console.log(JSON.stringify({ node: process.version, status: client.status, family: dev.family, dry_run: result.dry_run, devices: client.devices.size, error: typeof GazelleError }));",
      "await client.close();",
      "",
    ].join("\n"),
  );
  writeFileSync(
    join(project, "consumer.ts"),
    [
      'import { connect, type Client, type FamilyTypes } from "gazelle-audio-client";',
      "export async function main(url: string): Promise<number | undefined> {",
      "  const client: Client = await connect(url);",
      '  const dev = client.device("loopback-0");',
      '  if (dev.family !== "quadro") return undefined;',
      '  const levels: FamilyTypes["quadro"]["cyclic"]["0x73"]["volumes"] = [];',
      '  await dev.invoke("set_volume", { id: 1, volume: 64 }, { dryRun: true });',
      "  await client.close();",
      "  return levels[0]?.volume;",
      "}",
      "",
    ].join("\n"),
  );
  writeFileSync(
    join(project, "misuse.ts"),
    [
      'import { connect } from "gazelle-audio-client";',
      "export async function misuse(url: string): Promise<void> {",
      "  const client = await connect(url);",
      '  const dev = client.device("loopback-0");',
      '  if (dev.family !== "quadro") return;',
      '  await dev.invoke("set_volume", { id: 1, volume: "loud" });',
      "}",
      "",
    ].join("\n"),
  );
  server = await startServer(["--backend", "loopback"]);
});

after(async () => {
  await server?.stop();
  if (project) rmSync(project, { recursive: true, force: true });
});

function smoke(node: string): void {
  const output = run(node, ["smoke.mjs", server.url], project).trim().split("\n").at(-1) ?? "";
  const report = JSON.parse(output) as Record<string, unknown>;
  assert.deepEqual({ ...report, node: undefined }, { node: undefined, status: "open", family: "quadro", dry_run: true, devices: 2, error: "function" });
}

test(`the packed client imports and connects on Node ${process.version}`, () => {
  smoke(process.execPath);
});

test("the packed client imports and connects on Node 22", { skip: NODE22 === undefined ? "set GAZELLE_NODE22 to a Node 22 executable" : false }, () => {
  const version = run(NODE22 ?? "", ["--version"], project).trim();
  assert.match(version, /^v22\./);
  smoke(NODE22 ?? "");
});

const TSC_FLAGS = ["--noEmit", "--strict", "--target", "es2022", "--module", "nodenext", "--moduleResolution", "nodenext"];

test("a TypeScript consumer typechecks against the installed declarations", () => {
  run(process.execPath, [TSC, ...TSC_FLAGS, "consumer.ts"], project);
});

test("the installed declarations carry the generated types, so misuse fails to typecheck", () => {
  const result = spawnSync(process.execPath, [TSC, ...TSC_FLAGS, "misuse.ts"], { cwd: project, encoding: "utf8" });
  assert.notEqual(result.status, 0, "a string volume must not typecheck");
  assert.match(result.stdout, /misuse\.ts\(6,\d+\): error TS2322/, result.stdout);
});
