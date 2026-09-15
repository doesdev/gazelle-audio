import { test } from "node:test";
import assert from "node:assert/strict";

import { GazelleError } from "gazelle-audio-client";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { effect } from "../src/core/signal.ts";
import { displayName, SAVE_DEBOUNCE_MS, sameValue, Store, THEME_STORAGE_KEY, type KeyValueStorage } from "../src/store/store.ts";
import { builtInThemes as builtIns, device, FakeClient, flush, MemoryStorage } from "./fake-client.ts";

function setup(client = new FakeClient(device("loopback-1", "studio", "Zen Studio+"), device("loopback-0", "quadro", "Zen Quadro"))) {
  const timers = new ManualTimers();
  const storage = new MemoryStorage();
  const frames: (() => void)[] = [];
  const store = new Store(client, { timers, storage, requestFrame: (callback) => frames.push(callback), themeSources: builtIns });
  return { client, timers, storage, frames, store };
}

test("connection state follows the client", async () => {
  const { client, store } = setup();
  assert.deepEqual(store.devices.value.map((d) => d.id), ["loopback-0", "loopback-1"], "devices are sorted by id");
  assert.equal(store.connected.value, true);
  assert.equal(store.server.value.dry_run, true);

  client.devices.set("loopback-2", device("loopback-2", "quadro", "Zen Quadro"));
  client.emit("device_added", device("loopback-2", "quadro", "Zen Quadro"));
  assert.equal(store.devices.value.length, 3);

  client.status = "reconnecting";
  client.emit("status", "reconnecting");
  assert.deepEqual([store.status.value, store.connected.value], ["reconnecting", false]);

  client.emit("lagged", 7);
  assert.deepEqual(store.notices.value.map((n) => [n.level, n.message]), [["warning", "This connection fell behind the server; 7 updates were skipped."]]);
  store.dismiss(store.notices.value[0]!.id);
  assert.deepEqual(store.notices.value, []);
  await store.close();
  assert.equal(client.closed, true);
});

test("edits apply at once and are saved together after the debounce", async () => {
  const { client, timers, store } = setup();
  await store.start();
  const quadro = store.devices.value[0]!;
  assert.equal(displayName(quadro, store.workspace.value), "Zen Quadro");

  assert.equal(store.renameDevice("loopback-0", "  Desk  "), true);
  assert.equal(displayName(quadro, store.workspace.value), "Desk", "the rename shows immediately");
  store.renameDevice("loopback-1", "Rack");
  timers.advance(SAVE_DEBOUNCE_MS - 1);
  assert.equal(client.puts.length, 0, "nothing is sent within the debounce");
  timers.advance(1);
  await flush();
  assert.deepEqual(client.puts.map((w) => w.aliases), [{ "loopback-0": "Desk", "loopback-1": "Rack" }], "both edits go in one save");

  store.renameDevice("loopback-1", "   ");
  timers.advance(SAVE_DEBOUNCE_MS);
  await flush();
  assert.deepEqual(client.stored.aliases, { "loopback-0": "Desk" }, "a blank name removes the alias");

  client.emit("status", "reconnecting");
  assert.equal(store.renameDevice("loopback-0", "Offline"), false, "nothing is edited without a connection");
  assert.equal(store.workspace.value?.aliases["loopback-0"], "Desk");
});

test("a failed save restores the last saved workspace and says so", async () => {
  const { client, timers, store } = setup();
  await store.start();
  store.renameDevice("loopback-0", "Desk");
  timers.advance(SAVE_DEBOUNCE_MS);
  await flush();

  client.failPuts = new GazelleError("storage_error", "disk full");
  store.renameDevice("loopback-0", "Lost");
  assert.equal(store.workspace.value?.aliases["loopback-0"], "Lost");
  timers.advance(SAVE_DEBOUNCE_MS);
  await flush();
  assert.equal(store.workspace.value?.aliases["loopback-0"], "Desk", "rolled back");
  assert.equal(store.saving.value, false);
  assert.deepEqual(store.notices.value.map((n) => [n.level, n.message]), [["error", "The workspace change was not saved and has been undone: disk full"]]);
});

test("group colours and collapse are edited in nested groups", async () => {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"));
  client.stored = {
    version: 1,
    links: [],
    aliases: {},
    groups: [{ id: "drums", name: "Drums", collapsed: false, hidden: false, members: [], children: [{ id: "kick", name: "Kick", collapsed: false, hidden: false, members: [], children: [] }] }],
  };
  const { timers, store } = setup(client);
  await store.start();
  store.setGroupColor("kick", "#b5473a");
  store.toggleGroup("drums");
  timers.advance(SAVE_DEBOUNCE_MS);
  await flush();
  assert.equal(client.stored.groups[0]?.collapsed, true);
  assert.equal(client.stored.groups[0]?.children[0]?.color, "#b5473a");
  store.setGroupColor("kick", undefined);
  timers.advance(SAVE_DEBOUNCE_MS);
  await flush();
  assert.equal("color" in (client.stored.groups[0]?.children[0] ?? {}), false, "clearing removes the colour");
});

test("themes merge built-in and user sources, report problems, and remember the pick", async () => {
  const client = new FakeClient();
  client.userThemes = [
    { file: "warm.json", theme: { name: "Warm", extends: "gazelle-dark", colors: { accent: "#e08a2e" } } },
    { file: "typo.json", theme: { colors: { acent: "#ffffff" } } },
    { file: "broken.json", error: "expected value at line 1 column 3" },
  ];
  const { storage, store } = setup(client);
  assert.equal(store.theme.value.id, "gazelle-dark");
  await store.loadUserThemes();

  const catalog = store.themeCatalog.value;
  assert.deepEqual(catalog.themes.map((t) => t.id), ["gazelle-dark", "gazelle-light", "user:warm"]);
  assert.deepEqual(catalog.problems, [
    { id: "user:typo", origin: "user", errors: ['unknown colour "acent"'] },
    { id: "user:broken", origin: "user", errors: ["could not be read: expected value at line 1 column 3"] },
  ]);

  store.selectTheme("user:warm");
  assert.equal(store.theme.value.colors.accent, "#e08a2e");
  assert.equal(storage.items.get(THEME_STORAGE_KEY), "user:warm");

  const later = new Store(new FakeClient(), { storage, themeSources: builtIns, timers: new ManualTimers() });
  assert.equal(later.themeId.value, "user:warm", "the pick is remembered");
  assert.equal(later.theme.value.id, "gazelle-dark", "an unavailable pick falls back to gazelle-dark");

  const unavailable: KeyValueStorage = {
    getItem: () => {
      throw new Error("storage disabled");
    },
    setItem: () => {
      throw new Error("storage disabled");
    },
  };
  const noStorage = new Store(new FakeClient(), { storage: unavailable, themeSources: builtIns, timers: new ManualTimers() });
  noStorage.selectTheme("gazelle-light");
  assert.equal(noStorage.theme.value.id, "gazelle-light", "works without storage");
});

test("cyclic fields are signals written once per frame, and unchanged values do not re-render", () => {
  const { client, frames, store } = setup();
  const off = store.watchReport("loopback-0", "0x73");
  const offAgain = store.watchReport("loopback-0", "0x73");
  const send = client.cyclic.get("loopback-0|0x73");
  assert.ok(send, "the store subscribed once");

  const preset = store.field("loopback-0", "0x73", "current_preset");
  const sources = store.field("loopback-0", "0x73", "pm_bank_src");
  let renders = 0;
  const dispose = effect(() => {
    void sources.value;
    renders++;
  });

  send({ current_preset: 1, pm_bank_src: new Uint8Array([1, 2]) });
  send({ current_preset: 2, pm_bank_src: new Uint8Array([1, 2]) });
  assert.equal(preset.value, undefined, "nothing is applied before the frame");
  assert.equal(frames.length, 1);
  frames.shift()?.();
  assert.equal(preset.value, 2, "the last report in the frame wins");
  assert.equal(renders, 2);

  send({ current_preset: 2, pm_bank_src: new Uint8Array([1, 2]) });
  frames.shift()?.();
  assert.equal(renders, 2, "an equal byte array is not a change");

  off();
  off();
  assert.ok(client.cyclic.has("loopback-0|0x73"), "another watcher still needs the subscription");
  offAgain();
  assert.equal(client.cyclic.has("loopback-0|0x73"), false, "the last watcher unsubscribes");
  dispose();

  client.devices.set("usb:1", device("usb:1", null, null));
  assert.throws(() => store.watchReport("usb:1", "0x73"), /unknown model/);
});

test("value equality treats bytes, arrays and objects structurally", () => {
  assert.equal(sameValue(new Uint8Array([1, 2]), new Uint8Array([1, 2])), true);
  assert.equal(sameValue(new Uint8Array([1, 2]), new Uint8Array([1, 3])), false);
  assert.equal(sameValue([{ volume: 1, mute: 0 }], [{ volume: 1, mute: 0 }]), true);
  assert.equal(sameValue([{ volume: 1 }], [{ volume: 2 }]), false);
  assert.equal(sameValue({ a: 1 }, { a: 1, b: 2 }), false);
});
