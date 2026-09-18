// The build is more than one file, and the effect parameter catalogue is not in the one every page
// loads. The catalogue is about 116 kB of generated tables that only the Effects page needs, so it
// travels in a chunk of its own, fetched when that page opens (store/effect-parameters.ts). This
// builds the app into a temporary directory and looks at what came out: a static import of the
// catalogue anywhere in the app would put it back in the entry chunk and fail here.

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

const app = fileURLToPath(new URL("..", import.meta.url));

async function chunks(): Promise<Rollup.OutputChunk[]> {
  const out = mkdtempSync(join(tmpdir(), "gazelle-bundle-"));
  try {
    const result = await build({ configFile: join(app, "vite.config.ts"), logLevel: "silent", build: { outDir: out, emptyOutDir: true } });
    const outputs = (Array.isArray(result) ? result : [result]) as Rollup.RollupOutput[];
    return outputs.flatMap((o) => o.output.filter((part): part is Rollup.OutputChunk => part.type === "chunk")).map((chunk) => ({ ...chunk, code: readFileSync(join(out, chunk.fileName), "utf8") }));
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
}

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
