// The build is more than one file, and neither the effect parameter catalogue nor a page only one
// route shows is in what the app loads. The catalogue is about 116 kB of generated tables that only
// the Effects page needs (P125); the pages are the Effects, Workspace, Inputs, Outputs, Routing,
// Devices and surface pages, each fetched when its route opens (elements/lazy.ts). This builds the
// app into a temporary directory and looks at what came out: a static import of any of them from
// somewhere the app loads eagerly would put it back in the app's own chunks and fail here.

import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { build, type Rollup } from "vite";

/** A string only the generated catalogue holds: the reason it leaves the Quadro's ClearQ out. */
const IN_THE_CATALOGUE = "The panel's read names no instance";
/** Vite warns above this, and the whole point of the split is to stay under it. */
const CHUNK_WARNING_KB = 500;
/**
 * What the entry chunk has to come in under. The point of the page split is room for the next page
 * as well, not another few kB of headroom: P125 left about 9 kB and the snapshots UI used all of it.
 */
const ENTRY_BUDGET_KB = 420;

const app = fileURLToPath(new URL("..", import.meta.url));

/** One build for every test here: they all ask the same question of the same output. */
let built: Promise<Rollup.OutputChunk[]> | undefined;
const chunks = () => (built ??= buildOnce());

async function buildOnce(): Promise<Rollup.OutputChunk[]> {
  const out = mkdtempSync(join(tmpdir(), "gazelle-bundle-"));
  try {
    const result = await build({ configFile: join(app, "vite.config.ts"), logLevel: "silent", build: { outDir: out, emptyOutDir: true } });
    const outputs = (Array.isArray(result) ? result : [result]) as Rollup.RollupOutput[];
    return outputs.flatMap((o) => o.output.filter((part): part is Rollup.OutputChunk => part.type === "chunk")).map((chunk) => ({ ...chunk, code: readFileSync(join(out, chunk.fileName), "utf8") }));
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
}

/**
 * The chunks a browser loads before anything is clicked: the entry and everything it imports
 * statically, however deep. A page that reaches any of these is loaded with the app, whether or not
 * it is in the entry chunk itself.
 */
function onStartup(built: readonly Rollup.OutputChunk[]): Rollup.OutputChunk[] {
  const byName = new Map(built.map((chunk) => [chunk.fileName, chunk]));
  const reached = new Set<string>();
  const visit = (name: string) => {
    if (reached.has(name)) return;
    reached.add(name);
    for (const next of byName.get(name)?.imports ?? []) visit(next);
  };
  for (const chunk of built) if (chunk.isEntry) visit(chunk.fileName);
  return [...reached].map((name) => byName.get(name) as Rollup.OutputChunk);
}

/** The modules `lazy.ts` fetches, as it names them: the source of truth for what must be split out. */
function lazyModules(): string[] {
  const source = readFileSync(join(app, "src/elements/lazy.ts"), "utf8");
  return [...new Set([...source.matchAll(/\bimport\("\.\/([\w-]+\.ts)"\)/g)].map((m) => `elements/${m[1] as string}`))];
}

const holds = (chunk: Rollup.OutputChunk, module: string) => (chunk.moduleIds ?? []).some((id) => id.replace(/\\/g, "/").endsWith(`/src/${module}`));

test("the effect catalogue is a chunk of its own, and the entry chunk stays under Vite's warning", async () => {
  const built = await chunks();
  const entry = built.find((chunk) => chunk.isEntry);
  assert.notEqual(entry, undefined, "there is an entry chunk");
  const holding = built.filter((chunk) => chunk.code.includes(IN_THE_CATALOGUE));
  assert.equal(holding.length, 1, `exactly one chunk holds the catalogue, not ${holding.map((c) => c.fileName).join(", ")}`);
  assert.equal(holding[0]?.isEntry, false, "and it is not the chunk every page loads");
  assert.equal(built.length > 1, true, "so the app is served as more than one file");

  const kb = Buffer.byteLength(entry?.code ?? "") / 1024;
  assert.equal(kb < CHUNK_WARNING_KB, true, `the entry chunk is ${kb.toFixed(1)} kB, and Vite warns above ${CHUNK_WARNING_KB} kB`);
});

test("a page only one route shows travels with that route, not with the app", async () => {
  const built = await chunks();
  const startup = onStartup(built);
  const modules = lazyModules();
  assert.equal(modules.length > 0, true, "lazy.ts names the modules it fetches");

  for (const module of modules) {
    const where = built.filter((chunk) => holds(chunk, module));
    assert.equal(where.length, 1, `${module} is in exactly one chunk, not ${where.length}`);
    const loaded = startup.filter((chunk) => holds(chunk, module)).map((c) => c.fileName);
    assert.deepEqual(loaded, [], `${module} is loaded with the app, through ${loaded.join(", ")}`);
  }
});

test("the explanations travel in a chunk of their own, fetched when the explain mode is first turned on", async () => {
  const built = await chunks();
  const module = "elements/explain-catalogue.ts";
  const where = built.filter((chunk) => holds(chunk, module));
  assert.equal(where.length, 1, `${module} is in exactly one chunk, not ${where.length}`);
  assert.deepEqual(where[0]?.moduleIds.map((id) => id.replace(/\\/g, "/").replace(/^.*\/src\//, "")), [module], "and that chunk holds nothing else");
  const loaded = onStartup(built).filter((chunk) => holds(chunk, module)).map((c) => c.fileName);
  assert.deepEqual(loaded, [], `the explanations are loaded with the app, through ${loaded.join(", ")}`);
  // The mode's own plumbing is in the app, since it has to be there to be turned on.
  assert.equal(onStartup(built).some((chunk) => holds(chunk, "elements/explain.ts")), true, "the mode itself comes with the app");
});

test("the entry chunk has room for the next page, not a few kB", async () => {
  const built = await chunks();
  const entry = built.find((chunk) => chunk.isEntry) as Rollup.OutputChunk;
  const kb = Buffer.byteLength(entry.code) / 1024;
  assert.equal(kb < ENTRY_BUDGET_KB, true, `the entry chunk is ${kb.toFixed(1)} kB, over the ${ENTRY_BUDGET_KB} kB this project keeps it under`);
});
