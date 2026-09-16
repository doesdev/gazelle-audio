import { test } from "node:test";
import assert from "node:assert/strict";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { persisted, STRIP_WIDTH_MAX, STRIP_WIDTH_MIN } from "../src/store/preferences.ts";
import { MIXER_WIDTH_STORAGE_KEY, PANELS_STORAGE_KEY, SELECTED_DEVICE_STORAGE_KEY, SELECTED_MIXES_STORAGE_KEY, Store } from "../src/store/store.ts";
import { effect } from "../src/core/signal.ts";
import { builtInThemes, device, FakeClient, MemoryStorage } from "./fake-client.ts";

const store = (storage: MemoryStorage, client = new FakeClient()) => new Store(client, { storage, timers: new ManualTimers(), themeSources: builtInThemes });
const QUADRO = device("loopback-0", "quadro", "Zen Quadro");
const STUDIO = device("loopback-1", "studio", "Zen Studio+");
const UNKNOWN = device("hid-9", null, null);

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

test("the device last selected is remembered per browser, and shown where a page names none", () => {
  const storage = new MemoryStorage();
  const client = new FakeClient(UNKNOWN, QUADRO, STUDIO);
  const first = store(storage, client);
  assert.equal(first.selectedDevice.value, undefined);
  assert.deepEqual([first.deviceInView(false), first.deviceInView(true)], ["hid-9", "loopback-0"], "with nothing selected, the first device, or the first of known model");

  assert.equal(first.selectDevice("loopback-1"), true);
  assert.deepEqual([first.deviceInView(false), first.deviceInView(true)], ["loopback-1", "loopback-1"]);
  assert.equal(first.selectDevice("usb-gone"), false, "a device that is not connected is not remembered");
  assert.equal(first.selectedDevice.value, "loopback-1");

  assert.equal(first.selectDevice("hid-9"), true, "the Devices page shows any model, so any connected device can be selected");
  assert.deepEqual([first.deviceInView(false), first.deviceInView(true)], ["hid-9", "loopback-0"], "a page for known models falls back past it");

  first.selectDevice("loopback-1");
  const later = store(storage, new FakeClient(QUADRO, STUDIO));
  assert.equal(later.deviceInView(true), "loopback-1", "remembered across a reload");
  const unplugged = store(storage, new FakeClient(QUADRO));
  assert.equal(unplugged.deviceInView(true), "loopback-0", "while it is not connected, the first device shows");
  assert.equal(unplugged.selectedDevice.value, "loopback-1", "and it is still remembered for when it is back");

  storage.items.set(SELECTED_DEVICE_STORAGE_KEY, "42");
  assert.equal(store(storage, client).selectedDevice.value, undefined, "an invalid stored device is ignored");
});

test("the mix last selected is remembered per device and per browser, within the device's mixes", () => {
  const storage = new MemoryStorage();
  const client = new FakeClient(QUADRO, STUDIO);
  const first = store(storage, client);
  assert.deepEqual([first.selectedMix("loopback-0").value, first.selectedMix("loopback-1").value], [0, 0]);

  const seen: number[] = [];
  const stop = effect(() => void seen.push(first.selectedMix("loopback-0").value));
  assert.equal(first.selectMix("loopback-0", 2), true);
  assert.equal(first.selectMix("loopback-1", 1), true);
  assert.deepEqual(seen, [0, 2], "choosing another device's mix does not disturb this one's watchers");
  stop();
  assert.equal(first.selectMix("loopback-0", 4), false, "the Quadro has mixes 0..3");
  assert.equal(first.selectMix("loopback-0", 1.5), false);
  assert.equal(first.selectMix("hid-9", 0), false, "a device of unknown model has no mixes");
  assert.equal(first.selectedMix("loopback-0").value, 2);

  assert.equal(first.channels("loopback-0").meteredMix.value, 2, "the Mixer page's metered mix is the selected mix");
  first.channels("loopback-0").meteredMix.value = 3;
  assert.equal(first.selectedMix("loopback-0").value, 3);

  const later = store(storage, client);
  assert.deepEqual([later.selectedMix("loopback-0").value, later.selectedMix("loopback-1").value], [3, 1], "remembered across a reload");

  storage.items.set(SELECTED_MIXES_STORAGE_KEY, JSON.stringify({ "loopback-0": 9, "loopback-1": "two" }));
  const clamped = store(storage, client);
  assert.deepEqual([clamped.selectedMix("loopback-0").value, clamped.selectedMix("loopback-1").value], [3, 0], "a stored mix past the device's last is clamped, and a malformed one ignored");
});

test("view state lasts for the tab: one signal per key, not saved to storage", () => {
  const storage = new MemoryStorage();
  const first = store(storage);
  const scroll = first.view("scroll:inputs/loopback-0", 0);
  assert.equal(scroll.value, 0);
  scroll.value = 240;
  assert.equal(first.view("scroll:inputs/loopback-0", 0), scroll, "the same key is the same signal, so a rebuilt page finds it");
  assert.equal(first.view("scroll:inputs/loopback-0", 0).value, 240);
  assert.equal(first.view("scroll:outputs/loopback-0", 0).value, 0);
  assert.deepEqual([...storage.items.keys()], [], "nothing is written to storage");
  assert.equal(store(storage).view("scroll:inputs/loopback-0", 0).value, 0);
});
