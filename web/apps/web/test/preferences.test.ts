import { test } from "node:test";
import assert from "node:assert/strict";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { persisted, STRIP_WIDTH_MAX, STRIP_WIDTH_MIN } from "../src/store/preferences.ts";
import { MIXER_WIDTH_STORAGE_KEY, PANELS_STORAGE_KEY, Store } from "../src/store/store.ts";
import { builtInThemes, FakeClient, MemoryStorage } from "./fake-client.ts";

const store = (storage: MemoryStorage) => new Store(new FakeClient(), { storage, timers: new ManualTimers(), themeSources: builtInThemes });

test("a persisted preference saves writes and validates what it loads", () => {
  const storage = new MemoryStorage();
  const parse = (v: unknown) => (typeof v === "number" && v > 0 ? v : undefined);
  const size = persisted(storage, "size", 10, parse);
  assert.equal(size.value, 10);
  size.value = 42;
  assert.equal(storage.items.get("size"), "42");
  assert.equal(persisted(storage, "size", 10, parse).value, 42, "a later load sees the saved value");

  storage.items.set("size", "-5");
  assert.equal(persisted(storage, "size", 10, parse).value, 10, "an invalid stored value falls back");
  storage.items.set("size", "{not json");
  assert.equal(persisted(storage, "size", 10, parse).value, 10, "a corrupt stored value falls back");

  const broken = { getItem: () => { throw new Error("blocked"); }, setItem: () => { throw new Error("blocked"); } };
  const unsaved = persisted(broken, "size", 10, parse);
  unsaved.value = 7;
  assert.equal(unsaved.value, 7, "works without storage, for the page only");
});

test("mixer width and collapsed panels are remembered per browser", () => {
  const storage = new MemoryStorage();
  const first = store(storage);
  assert.deepEqual(first.mixerWidth.value, { auto: true, px: 64 });
  assert.deepEqual(first.panels.value, { leftCollapsed: false, rightCollapsed: false });

  first.setMixerWidth({ auto: false, px: 999 });
  assert.deepEqual(first.mixerWidth.value, { auto: false, px: STRIP_WIDTH_MAX }, "the width is clamped");
  first.setMixerWidth({ px: 1 });
  assert.equal(first.mixerWidth.value.px, STRIP_WIDTH_MIN, "and never narrower than the floor");
  first.setMixerWidth({ px: 72 });
  first.togglePanel("left");
  first.togglePanel("right");
  first.togglePanel("right");

  const later = store(storage);
  assert.deepEqual(later.mixerWidth.value, { auto: false, px: 72 });
  assert.deepEqual(later.panels.value, { leftCollapsed: true, rightCollapsed: false });

  storage.items.set(MIXER_WIDTH_STORAGE_KEY, JSON.stringify({ auto: "yes", px: 72 }));
  storage.items.set(PANELS_STORAGE_KEY, JSON.stringify({ leftCollapsed: 1 }));
  const reset = store(storage);
  assert.deepEqual([reset.mixerWidth.value, reset.panels.value], [{ auto: true, px: 64 }, { leftCollapsed: false, rightCollapsed: false }], "invalid stored preferences fall back to defaults");
});
