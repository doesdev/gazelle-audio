import { test } from "node:test";
import assert from "node:assert/strict";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { SAVE_DEBOUNCE_MS, Store } from "../src/store/store.ts";
import { builtInThemes, device, FakeClient, flush, MemoryStorage } from "./fake-client.ts";

function setup() {
  const client = new FakeClient(device("loopback-1", "studio", "Zen Studio+"), device("loopback-0", "quadro", "Zen Quadro"), device("usb:odd", null, null));
  const timers = new ManualTimers();
  const store = new Store(client, { timers, storage: new MemoryStorage(), requestFrame: (callback) => callback(), themeSources: builtInThemes });
  return { client, timers, store };
}

test("surfaces are made, renamed and removed in the workspace, and saved like any edit", async () => {
  const { client, timers, store } = setup();
  await store.start();
  const surfaces = store.surfaces;
  assert.deepEqual(surfaces.list.value, []);

  const id = surfaces.create("  Drum tracking ");
  assert.ok(id !== undefined);
  assert.deepEqual(surfaces.surface(id), { id, name: "Drum tracking", mixes: {}, strips: [] });
  const other = surfaces.create("Music via S/PDIF") as string;
  assert.notEqual(other, id, "ids are unique");
  assert.throws(() => surfaces.create("   "), RangeError, "a surface needs a name");

  assert.equal(surfaces.rename(id, "Drums"), true);
  assert.equal(surfaces.surface(id)?.name, "Drums");
  assert.throws(() => surfaces.rename(id, ""), RangeError);
  assert.equal(surfaces.rename("nope", "X"), false);

  timers.advance(SAVE_DEBOUNCE_MS);
  await flush();
  assert.deepEqual(client.stored.surfaces?.map((s) => s.name), ["Drums", "Music via S/PDIF"]);

  assert.equal(surfaces.remove(other), true);
  assert.equal(surfaces.remove(other), false);
  assert.deepEqual(store.surfaces.list.value.map((s) => s.id), [id]);
  assert.deepEqual(client.invocations, [], "a surface is layout only: no device is touched");
});

test("strips are added with their own ids, checked for their kind, moved and removed", async () => {
  const { store } = setup();
  await store.start();
  const surfaces = store.surfaces;
  const id = surfaces.create("Drums") as string;

  const pre = surfaces.addStrip(id, { kind: "input", device_id: "loopback-1", input: { kind: "preamp", channel: 2 } }) as string;
  const vox = surfaces.addStrip(id, { kind: "channel", device_id: "loopback-0", channel: "ch-vox" }) as string;
  const master = surfaces.addStrip(id, { kind: "master", device_id: "loopback-0", mix: 3 }) as string;
  const out = surfaces.addStrip(id, { kind: "output", device_id: "loopback-1", output: 4 }) as string;
  const label = surfaces.addStrip(id, { kind: "label", text: "Drums" }) as string;
  assert.equal(new Set([pre, vox, master, out, label]).size, 5, "strip ids are unique");
  assert.deepEqual(surfaces.surface(id)?.strips.map((s) => s.kind), ["input", "channel", "master", "output", "label"]);
  assert.deepEqual(surfaces.surface(id)?.strips[1], { id: vox, kind: "channel", device_id: "loopback-0", channel: "ch-vox" });

  // As the server checks them, so a bad strip never reaches a save that would be refused.
  assert.throws(() => surfaces.addStrip(id, { kind: "master" }), /needs a device/);
  assert.throws(() => surfaces.addStrip(id, { kind: "channel", device_id: "loopback-0" }), /needs a channel/);
  assert.throws(() => surfaces.addStrip(id, { kind: "channel", device_id: "loopback-0", channel: "" }), /needs a channel/);
  assert.throws(() => surfaces.addStrip(id, { kind: "label" }), /needs text/);
  assert.throws(() => surfaces.addStrip(id, { kind: "input", device_id: "loopback-0", input: { kind: "line", channel: 0 } }), /no line inputs/);
  assert.throws(() => surfaces.addStrip(id, { kind: "input", device_id: "loopback-0", input: { kind: "preamp", channel: 4 } }), /preamp inputs 0\.\.3/);
  assert.throws(() => surfaces.addStrip(id, { kind: "output", device_id: "loopback-0", output: 4 }), /outputs 0\.\.3/);
  assert.throws(() => surfaces.addStrip(id, { kind: "master", device_id: "loopback-0", mix: 4 }), /mix 4/);
  assert.throws(() => surfaces.addStrip(id, { kind: "fader" as never, device_id: "loopback-0" }), /kind/);
  assert.equal(surfaces.addStrip("nope", { kind: "label", text: "" }), undefined);

  // Moving counts the index among the other strips, as the Mixer page's drag does.
  assert.equal(surfaces.moveStrip(id, label, 0), true);
  assert.deepEqual(surfaces.surface(id)?.strips.map((s) => s.id), [label, pre, vox, master, out]);
  assert.equal(surfaces.moveStrip(id, pre, 99), true);
  assert.deepEqual(surfaces.surface(id)?.strips.map((s) => s.id), [label, vox, master, out, pre]);
  assert.equal(surfaces.moveStrip(id, out, -3), true, "before the first is first");
  assert.deepEqual(surfaces.surface(id)?.strips.map((s) => s.id), [out, label, vox, master, pre]);
  assert.equal(surfaces.moveStrip(id, out, 3), true);
  assert.deepEqual(surfaces.surface(id)?.strips.map((s) => s.id), [label, vox, master, out, pre]);
  assert.equal(surfaces.moveStrip(id, "nope", 0), false);

  assert.equal(surfaces.removeStrip(id, master), true);
  assert.equal(surfaces.removeStrip(id, master), false);
  assert.deepEqual(surfaces.surface(id)?.strips.map((s) => s.id), [label, vox, out, pre]);
});

test("a port strip shows a device's digital output, checked as the server checks it", async () => {
  const { store } = setup();
  await store.start();
  const surfaces = store.surfaces;
  const id = surfaces.create("Ports") as string;
  const spdif = surfaces.addStrip(id, { kind: "port", device_id: "loopback-0", port: "SPDIF_OUT" }) as string;
  const adat = surfaces.addStrip(id, { kind: "port", device_id: "loopback-1", port: "ADAT_OUT", first: 8 }) as string;
  assert.deepEqual(surfaces.surface(id)?.strips.map((s) => s.id), [spdif, adat]);
  assert.throws(() => surfaces.addStrip(id, { kind: "port", device_id: "loopback-0" }), /a port strip needs a port/);
  assert.throws(() => surfaces.addStrip(id, { kind: "port", device_id: "loopback-0", port: "ADAT_IN" as never }), /port must be SPDIF_OUT or ADAT_OUT/);
  assert.throws(() => surfaces.addStrip(id, { kind: "port", device_id: "loopback-0", port: "ADAT_OUT" }), /the quadro has no ADAT_OUT/);
  assert.throws(() => surfaces.addStrip(id, { kind: "port", device_id: "loopback-1", port: "ADAT_OUT", first: 4 }), /multiple of 8, not 4/);
  assert.throws(() => surfaces.addStrip(id, { kind: "port", device_id: "loopback-1", port: "ADAT_OUT", first: 16 }), /channels 0\.\.15, not 16\.\.23/);
  assert.throws(() => surfaces.addStrip(id, { kind: "port", device_id: "loopback-1", port: "SPDIF_OUT", first: 2 }), /channels 0\.\.1, not 2\.\.3/);
});

test("each device on a surface has one selected mix, which a strip may pin instead", async () => {
  const { store } = setup();
  await store.start();
  const surfaces = store.surfaces;
  const id = surfaces.create("Cue") as string;
  const follows = surfaces.addStrip(id, { kind: "channel", device_id: "loopback-0", channel: "a" }) as string;
  const pinned = surfaces.addStrip(id, { kind: "master", device_id: "loopback-0", mix: 3 }) as string;
  const studio = surfaces.addStrip(id, { kind: "channel", device_id: "loopback-1", channel: "k" }) as string;
  const strip = (stripId: string) => surfaces.surface(id)?.strips.find((s) => s.id === stripId);

  assert.equal(surfaces.mixOf(id, "loopback-0"), 0, "Mix 1 until one is chosen");
  assert.equal(surfaces.setMix(id, "loopback-0", 1), true);
  assert.equal(surfaces.mixOf(id, "loopback-0"), 1);
  assert.equal(surfaces.mixOf(id, "loopback-1"), 0, "mixes are per device: the Studio+ keeps its own");
  assert.equal(surfaces.stripMix(id, strip(follows)!), 1);
  assert.equal(surfaces.stripMix(id, strip(pinned)!), 3, "a pinned strip keeps its mix");
  assert.equal(surfaces.stripMix(id, strip(studio)!), 0);
  assert.throws(() => surfaces.setMix(id, "loopback-0", 4), RangeError);
  assert.throws(() => surfaces.setMix(id, "usb:odd", 0), RangeError, "a device of unknown model has no mixes");
  // The Mixer page's selection is the device's own, and a surface's does not move it.
  assert.equal(store.selectedMix("loopback-0").value, 0);

  assert.equal(surfaces.pin(id, follows, 2), true);
  assert.equal(surfaces.stripMix(id, strip(follows)!), 2);
  assert.equal(surfaces.pin(id, follows, undefined), true);
  assert.equal("mix" in strip(follows)!, false, "unpinned, the key is gone, as the server omits it");
  assert.equal(surfaces.stripMix(id, strip(follows)!), 1);
});

test("the devices a surface shows are listed in strip order, and each has a badge colour", async () => {
  const { store } = setup();
  await store.start();
  const surfaces = store.surfaces;
  const id = surfaces.create("Both") as string;
  surfaces.addStrip(id, { kind: "input", device_id: "loopback-1", input: { kind: "adat", channel: 0 } });
  surfaces.addStrip(id, { kind: "label", text: "Quadro" });
  surfaces.addStrip(id, { kind: "master", device_id: "loopback-0" });
  surfaces.addStrip(id, { kind: "output", device_id: "loopback-1", output: 0 });
  assert.deepEqual(surfaces.devicesOf(id), ["loopback-1", "loopback-0"]);

  // Until one is chosen, a device's colour is the theme palette's, by its place among the devices.
  const palette = store.theme.value.palette;
  assert.equal(surfaces.deviceColor("loopback-0"), palette[0]);
  assert.equal(surfaces.deviceColor("loopback-1"), palette[4], "half the palette apart (of nine), so two devices do not look alike");
  assert.equal(surfaces.deviceColor("usb:gone"), palette[0], "a device not attached still gets a colour");
  assert.equal(surfaces.deviceColor("usb:odd"), palette[8], "the third device, half the palette on again");
  assert.notEqual(surfaces.deviceColor("loopback-0"), surfaces.deviceColor("loopback-1"), "two devices are told apart");

  assert.equal(surfaces.setDeviceColor("loopback-1", "#3fae6a"), true);
  assert.equal(surfaces.deviceColor("loopback-1"), "#3fae6a");
  assert.deepEqual(store.workspace.value?.device_colors, { "loopback-1": "#3fae6a" });
  for (const bad of ["green", "#3fae6", "#3fae6g", "3fae6a0"]) assert.throws(() => surfaces.setDeviceColor("loopback-1", bad), RangeError, bad);
  assert.equal(surfaces.setDeviceColor("loopback-1", undefined), true);
  assert.deepEqual(store.workspace.value?.device_colors, {});
  assert.equal(surfaces.deviceColor("loopback-1"), palette[4]);
});

test("the mixer dock shows the device in view or a surface, remembered per browser, and the device again once that surface is gone", async () => {
  const storage = new MemoryStorage();
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"));
  const open = async () => {
    const store = new Store(client, { timers: new ManualTimers(), storage, requestFrame: (callback) => callback(), themeSources: builtInThemes });
    await store.start();
    return store;
  };
  const store = await open();
  assert.equal(store.mixerDockSurface.value, undefined, "the device in view until a surface is chosen");
  const id = store.surfaces.create("Cue") as string;
  assert.equal(store.setMixerDockSurface("nope"), false, "only a surface that exists");
  assert.equal(store.mixerDockSurface.value, undefined);
  assert.equal(store.setMixerDockSurface(id), true);
  assert.equal(store.mixerDockSurface.value, id);
  assert.equal(storage.items.get("gazelle.layout.mixerDockSurface"), JSON.stringify(id));

  // Another browser tab, or a reload, finds the same choice once the workspace has loaded.
  client.stored = structuredClone(store.workspace.value!);
  const later = await open();
  assert.equal(later.mixerDockSurface.value, id);

  // A surface deleted (here or elsewhere) hands the dock back to the device in view.
  store.surfaces.remove(id);
  assert.equal(store.mixerDockSurface.value, undefined);
  assert.equal(store.setMixerDockSurface(undefined), true);
  assert.equal(storage.items.get("gazelle.layout.mixerDockSurface"), "null");
  storage.items.set("gazelle.layout.mixerDockSurface", JSON.stringify(42));
  assert.equal((await open()).mixerDockSurface.value, undefined, "a stored value that is not an id is ignored");
});

test("without a connection nothing is edited", async () => {
  const { client, store } = setup();
  await store.start();
  const id = store.surfaces.create("Kept") as string;
  client.status = "closed";
  client.emit("status", "closed");
  assert.equal(store.surfaces.create("New"), undefined);
  assert.equal(store.surfaces.rename(id, "Gone"), false);
  assert.equal(store.surfaces.addStrip(id, { kind: "label", text: "x" }), undefined);
  assert.equal(store.surfaces.setDeviceColor("loopback-0", "#000000"), false);
  assert.deepEqual(store.surfaces.list.value.map((s) => s.name), ["Kept"]);
});
