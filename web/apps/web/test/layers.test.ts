// Spec §6.1: imports point downward only (elements → store → core, themes beside store), and
// only the store imports gazelle-audio-client. Decision P18: a small import scan instead of a
// lint rule. Files directly in src/ (the entry point) may import anything.

import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const SRC = resolve(dirname(fileURLToPath(import.meta.url)), "../src");
const CLIENT = "gazelle-audio-client";

/** What each layer may import: other src/ layers, and `client` for gazelle-audio-client. */
export const ALLOWED: Readonly<Record<string, readonly string[]>> = {
  core: ["core"],
  themes: ["core", "themes"],
  store: ["core", "themes", "store", "client"],
  elements: ["core", "themes", "store", "elements"],
};

const SPECIFIERS = /(?:\bimport\s+(?:type\s+)?(?:[\w*{}\s,]+\s+from\s+)?|\bexport\s+(?:type\s+)?[\w*{}\s,]+\s+from\s+|\bimport\s*\(\s*)["']([^"']+)["']/g;

function sourceFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return sourceFiles(path);
    return /\.(ts|js)$/.test(entry.name) ? [path] : [];
  });
}

/** The layer a file belongs to, or `undefined` for files directly in src/. */
function layerOf(root: string, path: string): string | undefined {
  const parts = relative(root, path).split(sep);
  return parts.length > 1 ? parts[0] : undefined;
}

/** Every import that breaks the layering, as `file: specifier (reason)`. */
export function violations(root: string = SRC): string[] {
  const found: string[] = [];
  for (const file of sourceFiles(root)) {
    const layer = layerOf(root, file);
    if (layer === undefined) continue;
    const allowed = ALLOWED[layer];
    const name = relative(root, file).split(sep).join("/");
    if (allowed === undefined) {
      found.push(`${name}: ${layer}/ is not a known layer`);
      continue;
    }
    for (const match of readFileSync(file, "utf8").matchAll(SPECIFIERS)) {
      const specifier = match[1] ?? "";
      let target: string;
      if (specifier.startsWith(".")) {
        target = layerOf(root, resolve(dirname(file), specifier)) ?? "the entry point";
      } else if (specifier === CLIENT || specifier.startsWith(`${CLIENT}/`)) {
        target = "client";
      } else {
        target = `the package ${specifier}`;
      }
      if (!allowed.includes(target)) found.push(`${name}: ${specifier} (${layer} may not import ${target})`);
    }
  }
  return found;
}

test("the scan catches upward, sideways, client and third-party imports", () => {
  const root = mkdtempSync(join(tmpdir(), "gazelle-layers-"));
  try {
    const put = (path: string, text: string) => {
      mkdirSync(dirname(join(root, path)), { recursive: true });
      writeFileSync(join(root, path), text);
    };
    put("main.ts", 'import "./elements/app.ts";\nimport "@fontsource-variable/inter";\n');
    put("core/signal.ts", 'import { store } from "../store/store.ts";\n');
    put("core/lazy.ts", 'const m = await import("some-lib");\n');
    put("themes/resolve.ts", 'export { signal } from "../core/signal.ts";\n');
    put("store/store.ts", 'import { connect, type Client } from "gazelle-audio-client";\nimport type { Theme } from "../themes/resolve.ts";\n');
    put("elements/app.ts", 'import { connect } from "gazelle-audio-client";\nimport { store } from "../store/store.ts";\n');
    put("elements/boot.ts", 'import "../main.ts";\n');
    assert.deepEqual(violations(root).sort(), [
      "core/lazy.ts: some-lib (core may not import the package some-lib)",
      "core/signal.ts: ../store/store.ts (core may not import store)",
      "elements/app.ts: gazelle-audio-client (elements may not import client)",
      "elements/boot.ts: ../main.ts (elements may not import the entry point)",
    ]);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("web UI imports point downward only", () => {
  assert.deepEqual(violations(), []);
});
