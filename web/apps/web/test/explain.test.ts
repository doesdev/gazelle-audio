// The explain mode (the user, 2026-09-18: "an optional (off by default) explain anything tooltip"):
// the rules that decide what the panel says and where it goes, the preference that turns it on, and
// the catalogue every `data-explain` key in the elements must have an entry in.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { CATALOGUE, RESERVED_KEYS } from "../src/elements/explain-catalogue.ts";
import { explainKeyIn, placePanel, resolveExplanation, type Rect } from "../src/elements/explain-rules.ts";
import { EXPLAIN_STORAGE_KEY, Store } from "../src/store/store.ts";
import { builtInThemes, FakeClient, MemoryStorage } from "./fake-client.ts";

const store = (storage: MemoryStorage) => new Store(new FakeClient(), { storage, timers: new ManualTimers(), themeSources: builtInThemes });

test("the explain mode is off by default, remembered per browser, and a bad stored value reads as off", () => {
  const storage = new MemoryStorage();
  const first = store(storage);
  assert.equal(first.explainMode.value, false, "off until someone turns it on");
  assert.equal(storage.items.has(EXPLAIN_STORAGE_KEY), false, "the default is not stored as a choice");
  first.setExplainMode(true);
  assert.equal(first.explainMode.value, true);
  assert.equal(store(storage).explainMode.value, true, "a later load finds it on");
  first.setExplainMode(false);
  assert.equal(store(storage).explainMode.value, false);
  storage.items.set(EXPLAIN_STORAGE_KEY, JSON.stringify("on"));
  assert.equal(store(storage).explainMode.value, false, "anything but true is off");
});

const overlaps = (a: Rect, b: Rect) => a.left < b.left + b.width && b.left < a.left + a.width && a.top < b.top + b.height && b.top < a.top + a.height;

test("the panel goes below what it explains, above it when there is no room below, and never over it", () => {
  const viewport = { width: 1400, height: 860 };
  const panel = { width: 280, height: 120 };
  const fader: Rect = { left: 100, top: 200, width: 40, height: 180 };
  const below = placePanel(fader, panel, viewport);
  assert.equal(below.side, "below");
  assert.equal(below.top >= fader.top + fader.height, true);
  assert.equal(overlaps({ ...below, ...panel }, fader), false);

  // A dock strip at the bottom of the window: no room below.
  const low: Rect = { left: 600, top: 700, width: 44, height: 150 };
  const above = placePanel(low, panel, viewport);
  assert.equal(above.side, "above");
  assert.equal(above.top + panel.height <= low.top, true);
  assert.equal(overlaps({ ...above, ...panel }, low), false);

  // A tall control that fills the window's height: beside it, on the side with room.
  const tall: Rect = { left: 1300, top: 10, width: 60, height: 840 };
  const beside = placePanel(tall, panel, viewport);
  assert.equal(beside.side, "left");
  assert.equal(overlaps({ ...beside, ...panel }, tall), false);
  const leftEdge: Rect = { left: 0, top: 10, width: 60, height: 840 };
  assert.equal(placePanel(leftEdge, panel, viewport).side, "right");

  // Always inside the window, however close to its edge the control is.
  for (const target of [{ left: 1390, top: 5, width: 8, height: 8 }, { left: 2, top: 845, width: 8, height: 8 }]) {
    const at = placePanel(target, panel, viewport);
    assert.equal(at.left >= 0 && at.left + panel.width <= viewport.width, true, `inside horizontally: ${JSON.stringify(at)}`);
    assert.equal(at.top >= 0 && at.top + panel.height <= viewport.height, true, `inside vertically: ${JSON.stringify(at)}`);
    assert.equal(overlaps({ ...at, ...panel }, target), false);
  }

  // A phone: the panel is narrowed to the window rather than hanging off it.
  const phone = placePanel({ left: 300, top: 400, width: 60, height: 30 }, { width: 400, height: 150 }, { width: 375, height: 812 });
  assert.equal(phone.width <= 375 - 16, true);
  assert.equal(phone.left >= 8, true);
});

test("an explanation names what it is about, and falls back to the entry's own words", () => {
  const entry = { title: "{name} level", what: "How loud {name} is in the mix.", effect: "Moves the level.", name: "this channel" };
  assert.deepEqual(resolveExplanation(entry, "Vox"), { title: "Vox level", what: "How loud Vox is in the mix.", effect: "Moves the level.", watch: undefined });
  assert.deepEqual(resolveExplanation(entry, undefined), { title: "this channel level", what: "How loud this channel is in the mix.", effect: "Moves the level.", watch: undefined });
  assert.equal(resolveExplanation({ title: "{name}", what: "x" }, "").title, "", "an empty name with no fallback stays empty rather than printing a placeholder");
});

test("the key is the nearest one along the event's path, through shadow roots, and nothing when none carries one", () => {
  const node = (key?: string, name?: string) => ({ getAttribute: (a: string) => (a === "data-explain" ? (key ?? null) : a === "data-explain-name" ? (name ?? null) : null) });
  const path = [node(), node("mixer.strip.fader", "Vox"), node("mixer.page")];
  assert.deepEqual(explainKeyIn(path), { index: 1, key: "mixer.strip.fader", name: "Vox" });
  assert.equal(explainKeyIn([node(), node()]), undefined);
  assert.equal(explainKeyIn([{}, "text", node("a.b")] as unknown[])?.key, "a.b", "nodes that are not elements are passed over");
});

const ELEMENTS = fileURLToPath(new URL("../src/elements/", import.meta.url));

/**
 * Every key written in the elements' source: any quoted literal shaped like a key ("mixer.strip.fader")
 * whose first part is one the catalogue uses. Keys are written as whole literals, never put together
 * from parts, so that this finds every one of them, including those in a table of keys.
 */
function keysInSource(): Map<string, string> {
  const namespaces = new Set(Object.keys(CATALOGUE).map((key) => key.split(".")[0] as string));
  const found = new Map<string, string>();
  for (const file of readdirSync(ELEMENTS).filter((f) => f.endsWith(".ts") && f !== "explain-catalogue.ts")) {
    const source = readFileSync(`${ELEMENTS}${file}`, "utf8");
    for (const match of source.matchAll(/"([a-z][a-z0-9-]*(?:\.[a-z0-9-]+)+)"/g)) {
      const key = match[1] as string;
      if (namespaces.has(key.split(".")[0] as string)) found.set(key, file);
    }
  }
  return found;
}

test("every key the elements name has an entry in the catalogue, and every entry is used", () => {
  const used = keysInSource();
  assert.equal(used.size > 100, true, `the elements name ${used.size} keys`);
  const missing = [...used].filter(([key]) => CATALOGUE[key] === undefined).map(([key, file]) => `${key} (${file})`);
  assert.deepEqual(missing, [], "keys with no explanation");
  const unused = Object.keys(CATALOGUE).filter((key) => !used.has(key) && !RESERVED_KEYS.includes(key));
  assert.deepEqual(unused, [], "entries nothing uses");
});

test("every entry says what the thing is, and the text is plain", () => {
  for (const [key, entry] of Object.entries(CATALOGUE)) {
    assert.equal(entry.title.trim().length > 0, true, `${key} has a title`);
    assert.equal(entry.what.trim().length > 10, true, `${key} says what it is`);
    for (const text of [entry.title, entry.what, entry.effect ?? "", entry.watch ?? ""]) {
      assert.equal([0x2013, 0x2014].some((code) => text.includes(String.fromCharCode(code))), false, `${key}: no en or em dash`);
      assert.doesNotMatch(text, /\{(?!name\})/, `${key}: the only placeholder is {name}`);
      assert.equal(text.length < 600, true, `${key}: short enough to read in a tooltip`);
    }
    if (/\{name\}/.test(entry.title + entry.what + (entry.effect ?? "") + (entry.watch ?? ""))) assert.equal(typeof entry.name, "string", `${key} uses {name}, so it says what to call the thing when no name is given`);
  }
});
