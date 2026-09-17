import { test } from "node:test";
import assert from "node:assert/strict";

import { GazelleError } from "gazelle-audio-client";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { ECHO_HOLD_MS } from "../src/store/inputs.ts";
import { effect } from "../src/core/signal.ts";
import { CLIP_AUTO_CLEAR_CHOICES, CLIP_HOLD_MS, displayName, OSCILLATOR_FREQUENCIES, OSCILLATOR_LEVELS, PANNING_LAWS, PRESET_SLOTS, SAVE_DEBOUNCE_MS, sameValue, Store, THEME_STORAGE_KEY, type KeyValueStorage } from "../src/store/store.ts";
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
    mixers: {},
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

test("device power: set_power carries 1 or 0, and only devices of known model have it", async () => {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"), device("usb:1", null, null));
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), themeSources: builtIns });
  await store.start();

  assert.equal(store.setPower("loopback-0", false), true);
  assert.equal(store.setPower("loopback-0", true), true);
  await flush();
  assert.deepEqual(
    client.invocations.filter((c) => c.command === "set_power").map((c) => [c.deviceId, c.args]),
    [["loopback-0", { power: 0 }], ["loopback-0", { power: 1 }]],
  );
  // Each power change is its own command, not coalesced with the previous one.
  assert.deepEqual(client.invocations.filter((c) => c.command === "set_power").map((c) => c.options?.["coalesce"]), [undefined, undefined]);
  assert.equal(store.setPower("usb:1", true), false, "a device of unknown model has no known commands");
});

test("device brightness: set_brightness is the panels' 0..100, clamped, and only for known models", async () => {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"), device("usb:1", null, null));
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), themeSources: builtIns });
  await store.start();

  store.setBrightness("loopback-0", 40);
  store.setBrightness("loopback-0", 999);
  store.setBrightness("loopback-0", -5);
  await flush();
  assert.deepEqual(
    client.invocations.filter((c) => c.command === "set_brightness").map((c) => c.args),
    [{ brightness: 40 }, { brightness: 100 }, { brightness: 0 }],
  );
  assert.equal(store.setBrightness("usb:1", 50), false, "a device of unknown model has no known commands");
});

test("clock: sample rates are the panels' seven, sources differ per model, and both send an index", async () => {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"), device("loopback-1", "studio", "Zen Studio+"), device("usb:1", null, null));
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), themeSources: builtIns });
  await store.start();

  assert.deepEqual(store.clock("loopback-0")?.rates, ["32 kHz", "44.1 kHz", "48 kHz", "88.2 kHz", "96 kHz", "176.4 kHz", "192 kHz"]);
  assert.deepEqual(store.clock("loopback-0")?.sources, ["Internal", "ADAT x1", "ADAT x2", "ADAT x4", "S/PDIF", "USB"]);
  assert.deepEqual(store.clock("loopback-1")?.sources, ["Oven", "Word clock", "ADAT", "ADAT x2", "ADAT x4", "S/PDIF", "USB"]);
  assert.equal(store.clock("usb:1"), undefined, "a device of unknown model has no known clock");

  assert.equal(store.setSampleRate("loopback-0", 2), true);
  assert.equal(store.setClockSource("loopback-1", 5), true);
  await flush();
  assert.deepEqual(client.invocations.filter((c) => c.command === "set_samp_rate").map((c) => [c.deviceId, c.args]), [["loopback-0", { srate_idx: 2 }]]);
  assert.deepEqual(client.invocations.filter((c) => c.command === "set_sync_source").map((c) => [c.deviceId, c.args]), [["loopback-1", { src_index: 5 }]]);

  // An index the model does not have is refused rather than sent.
  assert.throws(() => store.setSampleRate("loopback-0", 7), RangeError);
  assert.throws(() => store.setClockSource("loopback-0", 6), RangeError, "the Quadro has six sources");
  assert.equal(store.setSampleRate("usb:1", 0), false);
});

test("clock state: the measured frequency and lock come from the status report", () => {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"));
  const frames: (() => void)[] = [];
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), requestFrame: (cb) => frames.push(cb), themeSources: builtIns });
  const listen = store.watchReport("loopback-0", "0x73");
  const report = (fields: Record<string, unknown>) => {
    client.cyclic.get("loopback-0|0x73")?.(fields);
    for (const frame of frames.splice(0)) frame();
  };
  // 48000 Hz over three bytes, high first.
  report({ sync_freq_hi: 0, sync_freq_mid: 0xbb, sync_freq_low: 0x80, sync_source: 3, locked: 1, base_index: 2 });
  assert.deepEqual(store.clockState("loopback-0"), { source: 3, hz: 48000, locked: true, rate: 2 });
  // The devices run at 96 kHz as `base_index` 4 with the three bytes 1,119,0 (hardware session 2).
  report({ sync_freq_hi: 1, sync_freq_mid: 119, sync_freq_low: 0, sync_source: 0, locked: 0, base_index: 4 });
  assert.deepEqual(store.clockState("loopback-0"), { source: 0, hz: 96000, locked: false, rate: 4 });
  listen();
});

test("S/PDIF SRC: the Studio+'s switch, reported back as spdif_src and sent as 0 or 1; the Quadro panel never sends it", async () => {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"), device("loopback-1", "studio", "Zen Studio+"), device("usb:1", null, null));
  const frames: (() => void)[] = [];
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), requestFrame: (cb) => frames.push(cb), themeSources: builtIns });
  await store.start();
  const listen = store.watchReport("loopback-1", "0x73");
  const report = (fields: Record<string, unknown>) => {
    client.cyclic.get("loopback-1|0x73")?.(fields);
    for (const frame of frames.splice(0)) frame();
  };

  assert.equal(store.hasSpdifSrc("loopback-1"), true);
  assert.equal(store.hasSpdifSrc("loopback-0"), false, "the Quadro's command table has it, but nothing in its panel sends it or reads it back");
  assert.equal(store.hasSpdifSrc("usb:1"), false);
  assert.equal(store.spdifSrc("loopback-0"), undefined);

  assert.equal(store.spdifSrc("loopback-1"), false, "off until the device says otherwise");
  report({ spdif_src: 1 });
  assert.equal(store.spdifSrc("loopback-1"), true);
  report({ spdif_src: 0 });
  assert.equal(store.spdifSrc("loopback-1"), false);

  assert.equal(store.setSpdifSrc("loopback-1", true), true);
  assert.equal(store.setSpdifSrc("loopback-1", false), true);
  await flush();
  assert.deepEqual(client.invocations.filter((c) => c.command === "set_spdif_src").map((c) => [c.deviceId, c.args]), [
    ["loopback-1", { spdif_src: 1 }],
    ["loopback-1", { spdif_src: 0 }],
  ]);
  assert.equal(store.setSpdifSrc("loopback-0", true), false, "nothing is sent to a model without it");
  assert.equal(store.setSpdifSrc("usb:1", true), false);
  listen();
});

test("device presets: five slots numbered from one, recall and save, refusing anything else", async () => {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"), device("usb:1", null, null));
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), themeSources: builtIns });
  await store.start();

  assert.equal(PRESET_SLOTS, 5, "both panels show five presets");
  assert.equal(store.recallPreset("loopback-0", 3), true);
  assert.equal(store.savePreset("loopback-0", 5), true);
  await flush();
  assert.deepEqual(client.invocations.filter((c) => c.command === "preset_recall").map((c) => c.args), [{ preset_idx: 3 }]);
  assert.deepEqual(client.invocations.filter((c) => c.command === "preset_save").map((c) => c.args), [{ preset_idx: 5 }]);

  // The panels number presets 1..5; slot 0 is not one of them.
  assert.throws(() => store.recallPreset("loopback-0", 0), RangeError);
  assert.throws(() => store.savePreset("loopback-0", 6), RangeError);
  assert.equal(store.recallPreset("usb:1", 1), false, "a device of unknown model has no known commands");
});

test("panning law: the Quadro's four choices, read back with get_panning_law and set by index", async () => {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"), device("loopback-1", "studio", "Zen Studio+"), device("usb:1", null, null));
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), themeSources: builtIns });
  await store.start();

  // The Quadro panel's settings dialog, in its own order: how much a centred signal is attenuated.
  assert.deepEqual(PANNING_LAWS, ["0 dB", "-6 dB", "-3 dB", "-4.5 dB"]);
  assert.deepEqual(store.panningLaws("loopback-0"), PANNING_LAWS);
  assert.equal(store.panningLaws("loopback-1"), undefined, "the Studio+ has no panning law command");
  assert.equal(store.panningLaws("usb:1"), undefined);

  assert.equal(store.panningLaw("loopback-0").value, 0, "until it is read, the first choice shows");
  client.respond = async (call) => ({ device_id: call.deviceId, command: call.command, sent_hex: "74", sent_len: 1, dry_run: false, response: { panning: 2 }, response_error: null });
  assert.equal(await store.loadPanningLaw("loopback-0"), true);
  assert.equal(store.panningLaw("loopback-0").value, 2);
  assert.deepEqual(client.invocations.filter((c) => c.command === "get_panning_law").map((c) => c.deviceId), ["loopback-0"]);
  assert.equal(await store.loadPanningLaw("loopback-1"), false, "nothing is read from a model without it");

  assert.equal(store.setPanningLaw("loopback-0", 1), true);
  await flush();
  assert.deepEqual(client.invocations.filter((c) => c.command === "set_panning_law").map((c) => c.args), [{ panning: 1 }]);
  assert.equal(store.panningLaw("loopback-0").value, 1, "the choice shows at once");
  assert.throws(() => store.setPanningLaw("loopback-0", 4), RangeError);
  assert.equal(store.setPanningLaw("loopback-1", 1), false);
});

test("a device that will not answer get_panning_law leaves the choice unknown, without an error notice", async () => {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"));
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), themeSources: builtIns });
  await store.start();

  // Nothing asked for this read: the page does it on its own, so a refusal is not the user's problem.
  client.respond = async () => {
    throw new Error("unsupported");
  };
  assert.equal(await store.loadPanningLaw("loopback-0"), false);
  assert.equal(store.panningLaw("loopback-0").value, 0);
  assert.deepEqual(store.notices.value, []);
});

test("a read the user asked for still says so when it fails", async () => {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"));
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), themeSources: builtIns });
  await store.start();

  client.respond = async () => {
    throw new Error("unsupported");
  };
  await store.inputs("loopback-0").loadLinks();
  assert.deepEqual(store.notices.value.map((n) => [n.level, n.message]), [["error", "get_preamps_links could not be read: unsupported"]]);
});

test("DC coupling: the Quadro's two switches, one per side, reported back and sent with the side's id", async () => {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"), device("loopback-1", "studio", "Zen Studio+"), device("usb:1", null, null));
  const frames: (() => void)[] = [];
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), requestFrame: (cb) => frames.push(cb), themeSources: builtIns });
  await store.start();
  const listen = store.watchReport("loopback-0", "0x73");
  const report = (fields: Record<string, unknown>) => {
    client.cyclic.get("loopback-0|0x73")?.(fields);
    for (const frame of frames.splice(0)) frame();
  };

  assert.equal(store.hasDcCoupling("loopback-0"), true);
  assert.equal(store.hasDcCoupling("loopback-1"), false, "the Studio+ has no DC coupling command");
  assert.equal(store.hasDcCoupling("usb:1"), false);
  assert.equal(store.dcCoupling("loopback-1"), undefined);

  assert.deepEqual(store.dcCoupling("loopback-0"), { inputs: false, outputs: false });
  report({ dc_coupled_in: 1, dc_coupled_out: 0 });
  assert.deepEqual(store.dcCoupling("loopback-0"), { inputs: true, outputs: false });

  // `dc_coupled_io` names the side: 0 the inputs, 1 the outputs (the panel's two check buttons).
  assert.equal(store.setDcCoupled("loopback-0", "inputs", false), true);
  assert.equal(store.setDcCoupled("loopback-0", "outputs", true), true);
  await flush();
  assert.deepEqual(client.invocations.filter((c) => c.command === "set_dc_coupled").map((c) => c.args), [
    { dc_coupled: 0, dc_coupled_io: 0 },
    { dc_coupled: 1, dc_coupled_io: 1 },
  ]);
  assert.equal(store.setDcCoupled("loopback-1", "inputs", true), false, "nothing is sent to a model without it");
  listen();
});

test("oscillator: both models, two tones with a shared level, sent as one command and held over stale reports", async () => {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"), device("loopback-1", "studio", "Zen Studio+"), device("usb:1", null, null));
  const frames: (() => void)[] = [];
  const timers = new ManualTimers();
  const store = new Store(client, { timers, storage: new MemoryStorage(), requestFrame: (cb) => frames.push(cb), themeSources: builtIns });
  await store.start();
  const listen = store.watchReport("loopback-0", "0x73");
  const report = (fields: Record<string, unknown>) => {
    client.cyclic.get("loopback-0|0x73")?.(fields);
    for (const frame of frames.splice(0)) frame();
  };
  const sent = () => client.invocations.filter((c) => c.command === "set_sine_gen").map((c) => c.args);

  // The panels' own lists: only two of the four frequency codes are used.
  assert.deepEqual(OSCILLATOR_FREQUENCIES, ["1 kHz", "440 Hz"]);
  assert.deepEqual(OSCILLATOR_LEVELS, ["0 dBFS", "-6 dBFS", "-12 dBFS", "-18 dBFS"]);
  assert.equal(store.oscillator("usb:1"), undefined, "a device of unknown model has no known oscillator");
  assert.notEqual(store.oscillator("loopback-1"), undefined, "the Studio+ has one too");

  // Before any report nothing is known, and a tone shown as running would be the misleading half.
  assert.deepEqual(store.oscillator("loopback-1"), { left: 0, right: 0, level: 0, onLeft: false, onRight: false });

  // A muted side is a side with no tone, so the app shows tones on rather than mutes.
  report({ freq_left: 1, freq_right: 0, level: 2, mute_left: 0, mute_right: 1 });
  assert.deepEqual(store.oscillator("loopback-0"), { left: 1, right: 0, level: 2, onLeft: true, onRight: false });

  // Every change carries all five fields, because they share one byte.
  assert.equal(store.setOscillator("loopback-0", { onRight: true }), true);
  await flush();
  assert.deepEqual(sent(), [{ freq_left: 1, freq_right: 0, level: 2, mute_left: 0, mute_right: 0 }]);

  // A second change before the device has echoed the first must not undo it.
  report({ freq_left: 1, freq_right: 0, level: 2, mute_left: 0, mute_right: 1 });
  assert.equal(store.oscillator("loopback-0")?.onRight, true, "the change outranks a report still carrying the old value");
  store.setOscillator("loopback-0", { right: 1 });
  await flush();
  assert.deepEqual(sent().at(-1), { freq_left: 1, freq_right: 1, level: 2, mute_left: 0, mute_right: 0 });

  timers.advance(ECHO_HOLD_MS);
  report({ freq_left: 1, freq_right: 0, level: 2, mute_left: 0, mute_right: 1 });
  assert.deepEqual(store.oscillator("loopback-0"), { left: 1, right: 0, level: 2, onLeft: true, onRight: false }, "after the hold the device's report wins");

  // A device that has reported nothing has only what the app sent it, so that stays past the hold.
  assert.equal(store.setOscillator("loopback-1", { onLeft: true }), true);
  timers.advance(ECHO_HOLD_MS);
  assert.equal(store.oscillator("loopback-1")?.onLeft, true);

  assert.throws(() => store.setOscillator("loopback-0", { level: 4 }), RangeError);
  assert.throws(() => store.setOscillator("loopback-0", { left: 2 }), RangeError);
  assert.equal(store.setOscillator("usb:1", { onLeft: true }), false);
  listen();
});

test("a device card sums up the status report: power, preset, clock, and whether any input has signal or clipped", async () => {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"), device("loopback-1", "studio", "Zen Studio+"), device("usb:1", null, null));
  const frames: (() => void)[] = [];
  const timers = new ManualTimers();
  const store = new Store(client, { timers, storage: new MemoryStorage(), requestFrame: (cb) => frames.push(cb), themeSources: builtIns });
  await store.start();
  const listen = [store.watchReport("loopback-0", "0x73"), store.watchReport("loopback-1", "0x73")];
  const report = (id: string, fields: Record<string, unknown>) => {
    client.cyclic.get(`${id}|0x73`)?.(fields);
    for (const frame of frames.splice(0)) frame();
  };
  const quiet = (n: number) => new Uint8Array(n).fill(96);

  assert.equal(store.deviceCard("usb:1"), undefined, "an unknown model's report cannot be read");
  assert.deepEqual(store.deviceCard("loopback-0"), { reporting: false, power: undefined, preset: undefined, clock: undefined, input: "quiet" });

  report("loopback-0", {
    power_on: 1,
    current_preset: 2,
    sync_freq_hi: 1,
    sync_freq_mid: 119,
    sync_freq_low: 0,
    locked: 1,
    base_index: 4,
    sync_source: 0,
    peaks_preamp: quiet(4),
    peaks_spdif: quiet(2),
    peaks_adat: quiet(8),
  });
  assert.deepEqual(store.deviceCard("loopback-0"), { reporting: true, power: true, preset: 2, clock: { source: 0, hz: 96000, locked: true, rate: 4 }, input: "quiet" });

  // A meter byte is dB below full scale: anything above -60 dBFS is signal, and 0 is a clip.
  const preamps = quiet(4);
  preamps[2] = 18;
  report("loopback-0", { peaks_preamp: preamps, peaks_spdif: quiet(2), peaks_adat: quiet(8) });
  assert.equal(store.deviceCard("loopback-0")?.input, "signal");

  const adat = quiet(8);
  adat[5] = 0;
  report("loopback-0", { peaks_preamp: quiet(4), peaks_spdif: quiet(2), peaks_adat: adat });
  assert.equal(store.deviceCard("loopback-0")?.input, "clip");

  // A clip is one report long, so it holds long enough to be seen, then lets go.
  report("loopback-0", { peaks_preamp: quiet(4), peaks_spdif: quiet(2), peaks_adat: quiet(8) });
  assert.equal(store.deviceCard("loopback-0")?.input, "clip", "a clip holds past the report that carried it");
  timers.advance(CLIP_HOLD_MS);
  assert.equal(store.deviceCard("loopback-0")?.input, "quiet");

  // The Studio+ reports its lock as `locked_wc`, and has line inputs to meter as well.
  const lines = quiet(8);
  lines[0] = 0;
  report("loopback-1", { power_on: 0, locked_wc: 0, peaks_preamp: quiet(12), peaks_line: lines, peaks_spdif: quiet(2), peaks_adat: quiet(16) });
  const studio = store.deviceCard("loopback-1");
  assert.equal(studio?.power, false);
  assert.equal(studio?.clock?.locked, false);
  assert.equal(studio?.input, "clip");
  for (const stop of listen) stop();
});

test("input meters come from each input's own peak field, per model; an input without one has no meter", async () => {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"), device("loopback-1", "studio", "Zen Studio+"));
  const frames: (() => void)[] = [];
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), requestFrame: (cb) => frames.push(cb), themeSources: builtIns });
  await store.start();
  const listen = [store.watchReport("loopback-0", "0x73"), store.watchReport("loopback-1", "0x73")];
  const report = (id: string, fields: Record<string, unknown>) => {
    client.cyclic.get(`${id}|0x73`)?.(fields);
    for (const frame of frames.splice(0)) frame();
  };
  const bytes = (n: number, at: Record<number, number>) => Uint8Array.from({ length: n }, (_, i) => at[i] ?? 96);

  // Quadro: PREAMP is topology group 0, USB 1 PLAY group 1. Its mixer meters stay on Mix 1's inputs
  // whatever is asked of them (hardware, 2026-09-16), so a channel is metered at its input instead.
  const preamp2 = store.inputMeter("loopback-0", { group: 0, channel: 1 });
  assert.ok(preamp2);
  assert.equal(store.inputMeter("loopback-0", { group: 0, channel: 1 }), preamp2, "one meter per input, shared by every strip on it");
  const usb4 = store.inputMeter("loopback-0", { group: 1, channel: 3 });
  report("loopback-0", { peaks_preamp: bytes(4, { 1: 5 }), peaks_usb_play: bytes(18, { 3: 40 }) });
  assert.equal(preamp2.level.value, 5);
  assert.equal(usb4?.level.value, 40);

  // A byte of 0 is full scale: the clip light latches until it is cleared.
  assert.equal(preamp2.clipped.value, false);
  report("loopback-0", { peaks_preamp: bytes(4, { 1: 0 }) });
  report("loopback-0", { peaks_preamp: bytes(4, { 1: 30 }) });
  assert.equal(preamp2.clipped.value, true);
  preamp2.clearClip();
  assert.equal(preamp2.clipped.value, false);

  // Studio+: LINE IN is group 1; USB PLAY (group 3) has no meter field of its own.
  const line3 = store.inputMeter("loopback-1", { group: 1, channel: 2 });
  report("loopback-1", { peaks_line: bytes(8, { 2: 12 }) });
  assert.equal(line3?.level.value, 12);
  assert.equal(store.inputMeter("loopback-1", { group: 3, channel: 0 }), undefined);
  for (const stop of listen) stop();
});

test("output meters: the Quadro reports Monitor, HP1, HP2 and Line Out; the Studio+ has no fixed output meters", async () => {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"), device("loopback-1", "studio", "Zen Studio+"));
  const frames: (() => void)[] = [];
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), requestFrame: (cb) => frames.push(cb), themeSources: builtIns });
  await store.start();
  const stop = store.watchReport("loopback-0", "0x73");
  const meters = store.outputMeters("loopback-0");
  assert.deepEqual(meters?.map((m) => m.name), ["Monitor", "HP1", "HP2", "Line out"]);
  client.cyclic.get("loopback-0|0x73")?.({ peaks_monitor: Uint8Array.of(96, 96), peaks_hp1: Uint8Array.of(96, 96), peaks_hp2: Uint8Array.of(10, 11), line_out: Uint8Array.of(12, 13) });
  for (const frame of frames.splice(0)) frame();
  const lineOut = meters?.[3];
  assert.deepEqual([lineOut?.left.value, lineOut?.right.value], [12, 13]);
  assert.deepEqual([meters?.[0]?.left.value, meters?.[0]?.right.value], [96, 96]);
  assert.equal(store.outputMeters("loopback-1"), undefined);
  stop();
});

test("clip lights latch, clear when clicked or all at once, and auto-clear after a remembered delay (default 5 s, or never)", async () => {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"));
  const frames: (() => void)[] = [];
  const timers = new ManualTimers();
  const storage = new MemoryStorage();
  const store = new Store(client, { timers, storage, requestFrame: (cb) => frames.push(cb), themeSources: builtIns });
  await store.start();
  const stop = store.watchReport("loopback-0", "0x73");
  const report = (fields: Record<string, unknown>) => {
    client.cyclic.get("loopback-0|0x73")?.(fields);
    for (const frame of frames.splice(0)) frame();
  };
  const preamps = (clipAt?: number) => Uint8Array.from({ length: 4 }, (_, i) => (i === clipAt ? 0 : 40));
  const pair = (clip: boolean) => Uint8Array.of(clip ? 0 : 30, 30);

  assert.deepEqual(CLIP_AUTO_CLEAR_CHOICES, [2000, 5000, 10000, 30000, null]);
  assert.equal(store.clipAutoClear.value, 5000, "clip lights clear themselves after 5 s unless told otherwise");

  const preamp = store.inputMeter("loopback-0", { group: 0, channel: 2 });
  const lineOut = store.outputMeters("loopback-0")?.[3];
  assert.ok(preamp && lineOut);

  // Auto-clear counts from when the signal stops clipping: a device repeats the same report while it
  // clips, so a light must not clear while the clip goes on.
  report({ peaks_preamp: preamps(2), line_out: pair(true) });
  assert.deepEqual([preamp.clipped.value, lineOut.clipped.value], [true, true]);
  timers.advance(60_000);
  assert.deepEqual([preamp.clipped.value, lineOut.clipped.value], [true, true], "held for as long as it clips");
  report({ peaks_preamp: preamps(), line_out: pair(false) });
  timers.advance(3000);
  report({ peaks_preamp: preamps(2) });
  report({ peaks_preamp: preamps() });
  timers.advance(4999);
  assert.equal(preamp.clipped.value, true, "a second clip restarts the preamp's countdown");
  assert.equal(lineOut.clipped.value, false, "the line out has not clipped since, so it has cleared");
  timers.advance(1);
  assert.equal(preamp.clipped.value, false);

  // Clicked clear, and clear all.
  report({ peaks_preamp: preamps(2), line_out: pair(true) });
  lineOut.clearClip();
  assert.deepEqual([preamp.clipped.value, lineOut.clipped.value], [true, false]);
  report({ peaks_preamp: preamps(2), line_out: pair(true) });
  store.clearAllClips();
  assert.deepEqual([preamp.clipped.value, lineOut.clipped.value], [false, false]);

  // Never: a clip holds until it is cleared, and the choice is remembered.
  store.setClipAutoClear(null);
  report({ peaks_preamp: preamps(), line_out: pair(false) });
  report({ peaks_preamp: preamps(2) });
  report({ peaks_preamp: preamps() });
  timers.advance(600_000);
  assert.equal(preamp.clipped.value, true);
  assert.equal(new Store(client, { timers, storage, themeSources: builtIns }).clipAutoClear.value, null);
  // Turning auto-clear back on starts the countdown for a light already lit.
  store.setClipAutoClear(2000);
  timers.advance(2000);
  assert.equal(preamp.clipped.value, false);
  assert.throws(() => store.setClipAutoClear(1234), RangeError);
  stop();
});

test("an imported workspace replaces the server's whole, as given, and sends nothing to devices", async () => {
  const { client, timers, store } = setup();
  await store.start();
  const imported = { version: 1, groups: [], links: [], aliases: { "loopback-0": "Imported" }, mixers: {}, future: { kept: true } };

  // An edit still waiting for its save is superseded: the person chose to replace everything.
  store.renameDevice("loopback-1", "Pending");
  assert.equal(await store.replaceWorkspace(imported as never), undefined);
  assert.deepEqual(client.puts, [imported], "sent unchanged, unknown fields included");
  assert.deepEqual(store.workspace.value, imported, "the page shows what the server accepted");
  timers.advance(SAVE_DEBOUNCE_MS);
  await flush();
  assert.equal(client.puts.length, 1, "the superseded edit is not saved over the import");
  assert.deepEqual(client.invocations, [], "a workspace is layout only: no device is touched");
  assert.deepEqual(store.notices.value, []);
});

test("a refused import leaves the workspace as it was and says why", async () => {
  const { client, timers, store } = setup();
  await store.start();
  store.renameDevice("loopback-0", "Desk");
  timers.advance(SAVE_DEBOUNCE_MS);
  await flush();
  const before = structuredClone(store.workspace.value);
  const empty = () => ({ version: 1, groups: [], links: [], aliases: {}, mixers: {} });

  client.failPuts = new GazelleError("bad_value", "bad value: link 'l': a link needs at least two channels");
  assert.equal(await store.replaceWorkspace(empty()), "The server refused it: bad value: link 'l': a link needs at least two channels");
  assert.deepEqual(store.workspace.value, before);

  // The server answers a document it cannot deserialise with plain text, which the client reports by status only.
  client.failPuts = new GazelleError("http_422", "PUT http://x/api/v1/workspace returned HTTP 422");
  assert.equal(await store.replaceWorkspace(empty()), "The server could not read it as a workspace: a part of it is missing or has the wrong type.");
  assert.deepEqual(store.workspace.value, before);
  assert.deepEqual(store.notices.value, [], "the page that asked shows the reason; no notice as well");

  // An edit waiting for its save is still saved when the import is refused.
  client.failPuts = new GazelleError("storage_error", "disk full");
  store.renameDevice("loopback-1", "Rack");
  assert.equal(await store.replaceWorkspace(empty()), "The server refused it: disk full");
  client.failPuts = undefined;
  timers.advance(SAVE_DEBOUNCE_MS);
  await flush();
  assert.deepEqual(client.stored.aliases, { "loopback-0": "Desk", "loopback-1": "Rack" });

  // A save on its way could land after the import and undo it, so the import waits its turn.
  store.renameDevice("loopback-1", "Shelf");
  timers.advance(SAVE_DEBOUNCE_MS);
  assert.equal(store.saving.value, true);
  const puts = client.puts.length;
  assert.equal(await store.replaceWorkspace(empty()), "A change is still being saved; try again in a moment.");
  await flush();
  assert.equal(client.puts.length, puts);
  assert.equal(client.stored.aliases["loopback-1"], "Shelf");

  client.emit("status", "reconnecting");
  assert.equal(await store.replaceWorkspace(empty()), "Not connected to the server.");
  assert.equal(client.puts.length, puts);
});
