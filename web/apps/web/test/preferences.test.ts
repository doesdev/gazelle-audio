import { test } from "node:test";
import assert from "node:assert/strict";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { persisted, STRIP_WIDTH_MAX, STRIP_WIDTH_MIN } from "../src/store/preferences.ts";
import { MIXER_DOCK_STORAGE_KEY, MIXER_WIDTH_STORAGE_KEY, PANELS_STORAGE_KEY, SELECTED_DEVICE_STORAGE_KEY, SELECTED_MIXES_STORAGE_KEY, SIDEBAR_STORAGE_KEY, Store } from "../src/store/store.ts";
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

test("mixer width is remembered per browser", () => {
  const storage = new MemoryStorage();
  const first = store(storage);
  assert.deepEqual(first.mixerWidth.value, { auto: true, px: 64 });

  first.setMixerWidth({ auto: false, px: 999 });
  assert.deepEqual(first.mixerWidth.value, { auto: false, px: STRIP_WIDTH_MAX }, "the width is clamped");
  first.setMixerWidth({ px: 1 });
  assert.equal(first.mixerWidth.value.px, STRIP_WIDTH_MIN, "and never narrower than the floor");
  first.setMixerWidth({ px: 72 });

  const later = store(storage);
  assert.deepEqual(later.mixerWidth.value, { auto: false, px: 72 });

  storage.items.set(MIXER_WIDTH_STORAGE_KEY, JSON.stringify({ auto: "yes", px: 72 }));
  assert.deepEqual(store(storage).mixerWidth.value, { auto: true, px: 64 }, "an invalid stored preference falls back to its default");
});

test("the sidebar's side, whether it is collapsed and each of its sections are remembered per browser", () => {
  const storage = new MemoryStorage();
  const first = store(storage);
  assert.deepEqual(first.sidebar.value, { side: "right", collapsed: false, sections: {} }, "on the right and open by default");

  first.moveSidebar();
  first.toggleSidebar();
  first.setSidebarSection("meter", true);
  first.setSidebarSection("devices", true);
  first.setSidebarSection("devices", false);
  assert.deepEqual(first.sidebar.value, { side: "left", collapsed: true, sections: { meter: true, devices: false } });

  const later = store(storage);
  assert.deepEqual(later.sidebar.value, { side: "left", collapsed: true, sections: { meter: true, devices: false } }, "remembered across a reload");
  later.moveSidebar();
  later.toggleSidebar();
  assert.deepEqual([later.sidebar.value.side, later.sidebar.value.collapsed], ["right", false], "moving and collapsing again go back");

  storage.items.set(SIDEBAR_STORAGE_KEY, JSON.stringify({ side: "top", collapsed: 1, sections: { meter: true, devices: "no" } }));
  assert.deepEqual(store(storage).sidebar.value, { side: "right", collapsed: false, sections: { meter: true } }, "invalid parts of a stored sidebar fall back, one by one");
  storage.items.set(SIDEBAR_STORAGE_KEY, "[1, 2]");
  assert.deepEqual(store(storage).sidebar.value, { side: "right", collapsed: false, sections: {} });
});

test("the two side panels' collapse, from before the single sidebar, carries over once and is then dropped", () => {
  const panels = (value: string) => {
    const storage = new MemoryStorage();
    storage.items.set(PANELS_STORAGE_KEY, value);
    return storage;
  };
  // The left panel held the devices, the right the meter and Control Room.
  const left = panels(JSON.stringify({ leftCollapsed: true, rightCollapsed: false }));
  assert.deepEqual(store(left).sidebar.value, { side: "right", collapsed: false, sections: { devices: true, meter: false, controlRoom: false } });
  assert.equal(left.items.has(PANELS_STORAGE_KEY), false, "the old preference is removed");
  assert.deepEqual(JSON.parse(left.items.get(SIDEBAR_STORAGE_KEY) ?? "null"), { side: "right", collapsed: false, sections: { devices: true, meter: false, controlRoom: false } });

  const both = panels(JSON.stringify({ leftCollapsed: true, rightCollapsed: true }));
  assert.equal(store(both).sidebar.value.collapsed, true, "with both panels collapsed, so is the sidebar");

  const corrupt = panels("{not json");
  assert.deepEqual(store(corrupt).sidebar.value, { side: "right", collapsed: false, sections: {} }, "a corrupt old preference is ignored");
  assert.equal(corrupt.items.has(PANELS_STORAGE_KEY), false);

  const newer = panels(JSON.stringify({ leftCollapsed: true, rightCollapsed: true }));
  newer.items.set(SIDEBAR_STORAGE_KEY, JSON.stringify({ side: "left", collapsed: false, sections: {} }));
  assert.deepEqual(store(newer).sidebar.value, { side: "left", collapsed: false, sections: {} }, "a sidebar already stored wins");

  const readOnly = { getItem: (key: string) => (key === PANELS_STORAGE_KEY ? JSON.stringify({ leftCollapsed: false, rightCollapsed: true }) : null), setItem: () => { throw new Error("blocked"); } };
  assert.deepEqual(new Store(new FakeClient(), { storage: readOnly, timers: new ManualTimers(), themeSources: builtInThemes }).sidebar.value.sections, { devices: false, meter: true, controlRoom: true }, "storage that cannot be written still carries it over for the page");
});

test("whether the mixer dock is collapsed is remembered per browser; it starts open", () => {
  const storage = new MemoryStorage();
  const first = store(storage);
  assert.equal(first.mixerDockCollapsed.value, false);
  first.setMixerDockCollapsed(true);
  assert.equal(first.mixerDockCollapsed.value, true);
  assert.equal(store(storage).mixerDockCollapsed.value, true, "a later load sees it collapsed");
  first.setMixerDockCollapsed(false);
  assert.equal(store(storage).mixerDockCollapsed.value, false);

  storage.items.set(MIXER_DOCK_STORAGE_KEY, JSON.stringify("yes"));
  assert.equal(store(storage).mixerDockCollapsed.value, false, "an invalid stored value falls back to open");
  storage.items.set(MIXER_DOCK_STORAGE_KEY, JSON.stringify(true));
  assert.equal(store(storage).mixerDockCollapsed.value, true);
});

test("at phone width the mixer dock starts collapsed until a choice is kept; a kept choice wins at any width", () => {
  const phone = (storage: MemoryStorage) => new Store(new FakeClient(), { storage, timers: new ManualTimers(), themeSources: builtInThemes, narrow: true });
  const storage = new MemoryStorage();
  assert.equal(phone(storage).mixerDockCollapsed.value, true, "never set: collapsed on a phone");
  assert.equal(store(storage).mixerDockCollapsed.value, false, "never set: open elsewhere");
  assert.equal(storage.items.has(MIXER_DOCK_STORAGE_KEY), false, "the default is not stored as a choice");

  phone(storage).setMixerDockCollapsed(false);
  assert.equal(phone(storage).mixerDockCollapsed.value, false, "opened on a phone, it stays open there");
  store(storage).setMixerDockCollapsed(true);
  assert.equal(store(storage).mixerDockCollapsed.value, true);
  assert.equal(phone(storage).mixerDockCollapsed.value, true);

  storage.items.set(MIXER_DOCK_STORAGE_KEY, JSON.stringify("yes"));
  assert.equal(phone(storage).mixerDockCollapsed.value, true, "an invalid stored value falls back to the width's default");
  assert.equal(store(storage).mixerDockCollapsed.value, false);
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
