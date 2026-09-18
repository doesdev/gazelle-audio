// The two halves of how the app is put together must agree: `elements/index.ts` registers what the
// app loads with it, `elements/lazy.ts` names what a route fetches, and every page the shell can
// build has to be in exactly one of them. A page added to `pageFor` and to neither would be an
// element that is never defined — an empty page with nothing to explain it — so this reads the
// three files and checks they line up. `bundle-split.test.ts` checks the other half, that a lazy
// page really does leave the app's own chunks.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { LAZY_TAGS } from "../src/elements/lazy.ts";

const read = (path: string) => readFileSync(fileURLToPath(new URL(`../src/elements/${path}`, import.meta.url)), "utf8");

/** Every `<ga-*>` the shell builds for a route, from `pageFor`'s `h("ga-…")` calls. */
function pageTags(): string[] {
  const source = read("app.ts");
  const body = source.slice(source.indexOf("function pageFor("));
  return [...new Set([...body.matchAll(/h\("(ga-[a-z-]+)"/g)].map((m) => m[1] as string))];
}

/** The tags `index.ts` defines when the app starts. */
function eagerTags(): string[] {
  return [...read("index.ts").matchAll(/\["(ga-[a-z-]+)", Ga\w+\]/g)].map((m) => m[1] as string);
}

test("every page the shell can build is registered with the app or fetched with its route, never both and never neither", () => {
  const eager = eagerTags();
  const missing = pageTags().filter((tag) => !eager.includes(tag) && !LAZY_TAGS.includes(tag));
  assert.deepEqual(missing, [], "these pages would never be defined");
});

test("nothing is in both halves: a lazy page registered with the app would be in the entry chunk again", () => {
  const both = eagerTags().filter((tag) => LAZY_TAGS.includes(tag));
  assert.deepEqual(both, []);
});

test("the shell's own elements stay with the app", () => {
  // Header, sidebar (device list, meter, Control Room), mixer, dock and notices are on screen
  // before any route is chosen, so none of them may be fetched with a route.
  const shell = ["ga-header", "ga-device-list", "ga-output-meters", "ga-control-room", "ga-monitor", "ga-mixer", "ga-mixer-dock", "ga-notices", "ga-section", "ga-strip", "ga-app"];
  const eager = eagerTags();
  for (const tag of shell) assert.equal(eager.includes(tag), true, `${tag} is registered with the app`);
});

test("lazy.ts names each tag's chunk with a dynamic import, which is what puts it in one", () => {
  const source = read("lazy.ts");
  for (const tag of LAZY_TAGS) assert.match(source, new RegExp(`"${tag}": async \\(\\) =>`), `${tag} has a loader`);
  assert.equal(/\bimport\s+\{[^}]*\}\s+from\s+"\.\/(device-status|workspace|inputs-page|outputs-page|routing-page|effects-page|surface-page|surface-strip)\.ts"/.test(source), false, "and none of them is imported statically");
});
