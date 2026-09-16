import { test } from "node:test";
import assert from "node:assert/strict";

import { GazelleError } from "gazelle-audio-client";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { ECHO_HOLD_MS } from "../src/store/inputs.ts";
import { effect } from "../src/core/signal.ts";
import { displayName, OSCILLATOR_FREQUENCIES, OSCILLATOR_LEVELS, PANNING_LAWS, PRESET_SLOTS, SAVE_DEBOUNCE_MS, sameValue, Store, THEME_STORAGE_KEY, type KeyValueStorage } from "../src/store/store.ts";
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
