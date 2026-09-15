// gen-types [--check]: writes packages/client/src/generated from refs/schemas, or with --check
// regenerates in memory and fails when the committed files differ (spec §5).

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { generate } from "./emit.ts";

export const FAMILIES = ["quadro", "studio"] as const;

export interface RunOptions {
  schemasDir: string;
  outDir: string;
  check: boolean;
}

export interface RunResult {
  ok: boolean;
  /** Generated files that are missing or differ, sorted. */
  changed: string[];
}

export function run(options: RunOptions): RunResult {
  const inputs = FAMILIES.map((family) => {
    const topologyPath = join(options.schemasDir, `${family}_topology.json`);
    return {
      family,
      source: `refs/schemas/${family}_commands.json`,
      schema: JSON.parse(readFileSync(join(options.schemasDir, `${family}_commands.json`), "utf8")),
      ...(existsSync(topologyPath) ? { topology: JSON.parse(readFileSync(topologyPath, "utf8")) as unknown } : {}),
    };
  });
  const files = generate(inputs);
  // A checkout may have turned LF into CRLF; that is not drift.
  const current = (path: string) => (existsSync(path) ? readFileSync(path, "utf8").replace(/\r\n/g, "\n") : undefined);
  const changed = [...files].filter(([name, code]) => current(join(options.outDir, name)) !== code).map(([name]) => name).sort();
  if (options.check) return { ok: changed.length === 0, changed };
  mkdirSync(options.outDir, { recursive: true });
  for (const name of changed) writeFileSync(join(options.outDir, name), files.get(name) ?? "");
  return { ok: true, changed };
}

const here = dirname(fileURLToPath(import.meta.url));
if (process.argv[1] !== undefined && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = resolve(here, "../../../..");
  const check = process.argv.includes("--check");
  const result = run({ schemasDir: join(root, "refs", "schemas"), outDir: join(root, "web", "packages", "client", "src", "generated"), check });
  if (!result.ok) {
    console.error(`generated types are stale: ${result.changed.join(", ")}; run \`pnpm -C web gen-types\``);
    process.exit(1);
  }
  console.log(check ? "generated types are current" : result.changed.length ? `wrote ${result.changed.join(", ")}` : "generated types are current");
}
