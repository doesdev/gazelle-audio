import { test } from "node:test";
import assert from "node:assert/strict";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { ECHO_HOLD_MS } from "../src/store/inputs.ts";
import { Store } from "../src/store/store.ts";
import { builtInThemes, device, FakeClient, flush, MemoryStorage } from "./fake-client.ts";

function setup() {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"), device("loopback-1", "studio", "Zen Studio+"), device("usb:1", null, null));
  const frames: (() => void)[] = [];
  const timers = new ManualTimers();
  const store = new Store(client, { timers, storage: new MemoryStorage(), requestFrame: (callback) => frames.push(callback), themeSources: builtInThemes });
  /** Delivers one 0x73 report and runs the frame that applies it. */
  const report = (deviceId: string, fields: Record<string, unknown>) => {
    const listener = client.cyclic.get(`${deviceId}|0x73`);
    if (listener === undefined) throw new Error(`${deviceId} is not watching 0x73`);
    listener(fields);
    for (const frame of frames.splice(0)) frame();
  };
  const sent = (command: string) => client.invocations.filter((call) => call.command === command).map((call) => call.args);
  return { client, store, timers, report, sent };
}

const quadroPreamp = (type: number, phantom = 0, hpf = 0, phase = 0) => ({ type, phantom, hpf, phase_inv: phase, zero_cross: 0 });

test("preamp state comes from the device's report: signed gains, each family's type field, 48V only on Mic", () => {
  const { store, report } = setup();
  const quadro = store.inputs("loopback-0");
  assert.deepEqual([quadro.preampCount, quadro.hizCount], [4, 2]);
  assert.equal(quadro.preamp(0).value.known, false, "defaults until the device reports");

  quadro.activate();
  report("loopback-0", { preamps: [quadroPreamp(1, 0, 1, 1), quadroPreamp(0, 1), quadroPreamp(2, 1), quadroPreamp(0)], preamp_gains: new Uint8Array([250, 42, 10, 0]) });
  assert.deepEqual(quadro.preamp(0).value, { known: true, type: 1, gain: -6, phantom: false, phaseInvert: true, hpf: true });
  assert.deepEqual(quadro.preamp(1).value, { known: true, type: 0, gain: 42, phantom: true, phaseInvert: false, hpf: false });
  assert.equal(quadro.preamp(2).value.phantom, false, "a reported 48V outside Mic reads as off, as the Quadro panel treats it");

  const studio = store.inputs("loopback-1");
  assert.deepEqual([studio.preampCount, studio.hizCount], [12, 4]);
  studio.activate();
  report("loopback-1", { preamps: Array.from({ length: 12 }, (_, i) => ({ pretype: i === 3 ? 2 : 0, phantom: 0, hpf: 0, phase_inv: 0, zero_cross: 0 })), preamp_gains: new Uint8Array(12).fill(20) });
  assert.deepEqual([studio.preamp(3).value.type, studio.preamp(3).value.gain], [2, 20]);
});

test("changes send the panels' commands and outrank the device's reports for half a second", async () => {
  const { store, report, sent, timers } = setup();
  const quadro = store.inputs("loopback-0");
  quadro.activate();
  const reportGain = (gain: number) => report("loopback-0", { preamps: [0, 1, 2, 3].map(() => quadroPreamp(0)), preamp_gains: new Uint8Array([0, gain, 0, 0]) });
  reportGain(10);

  quadro.setGain(1, 30);
  await flush();
  assert.deepEqual(sent("set_pre_gain"), [{ id: 1, gain: 30 }]);
  reportGain(10);
  assert.equal(quadro.preamp(1).value.gain, 30, "a report still carrying the old value does not undo the change");
  timers.advance(ECHO_HOLD_MS);
  assert.equal(quadro.preamp(1).value.gain, 10, "after the hold the device's report wins");
});

test("the type sets the gain range; Hi-Z only where the device has it; 48V only on Mic; phase uses each family's command", async () => {
  const { store, sent } = setup();
  const quadro = store.inputs("loopback-0");

  quadro.setType(0, 1);
  quadro.setGain(0, 50);
  quadro.setGain(0, -40);
  await flush();
  assert.deepEqual(sent("set_pre_type"), [{ id: 0, pretype: 1 }]);
  assert.deepEqual(sent("set_pre_gain"), [{ id: 0, gain: 20 }, { id: 0, gain: -6 }], "Line gain is -6..+20 dB");
  assert.equal(quadro.setPhantom(0, true), false, "48V is refused outside Mic");

  quadro.setType(1, 2);
  assert.throws(() => quadro.setType(2, 2), RangeError, "Quadro Hi-Z is preamps 1-2 only");
  quadro.setType(1, 0);
  assert.equal(quadro.setPhantom(1, true), true);
  await flush();
  assert.deepEqual(sent("set_pre_phantom"), [{ id: 1, phantom: 1 }]);
  assert.equal(quadro.preamp(1).value.phantom, true);
  quadro.setType(1, 1);
  assert.equal(quadro.preamp(1).value.phantom, false, "leaving Mic drops 48V");

  quadro.setPhaseInvert(3, true);
  store.inputs("loopback-1").setPhaseInvert(0, true);
  await flush();
  assert.deepEqual(sent("set_pre_phase_inv"), [{ id: 3, phase_inv: 1 }]);
  assert.deepEqual(sent("set_pre_phaseinv"), [{ id: 0, phase_inv: 1 }]);
  assert.throws(() => quadro.setGain(4, 0), RangeError);
});

test("digital input gains: the Studio+ sets line, ADAT and S/PDIF; the Quadro only shows them", async () => {
  const { store, report, sent } = setup();
  const studio = store.inputs("loopback-1");
  assert.deepEqual(studio.digital.map((d) => [d.kind, d.count, d.editable]), [["line", 8, true], ["adat", 16, true], ["spdif", 2, true]]);
  studio.setDigitalGain("line", 2, 20);
  studio.setDigitalGain("spdif", 1, -9);
  await flush();
  assert.deepEqual(sent("set_line_gain"), [{ id: 2, gain: 12 }], "digital gain is -6..+12 dB");
  assert.deepEqual(sent("set_spdif_gain"), [{ id: 1, gain: -6 }]);
  assert.equal(studio.digitalGain("line", 2).value, 12);

  const quadro = store.inputs("loopback-0");
  assert.deepEqual(quadro.digital.map((d) => [d.kind, d.count, d.editable]), [["adat", 8, false], ["spdif", 2, false]]);
  assert.throws(() => quadro.setDigitalGain("adat", 0, 3), /does not set/);
  assert.throws(() => quadro.digitalGain("line", 0), RangeError, "the Quadro has no line inputs");
  quadro.activate();
  report("loopback-0", { adat_gains: new Uint8Array([253, 0, 0, 0, 0, 0, 0, 0]) });
  assert.equal(quadro.digitalGain("adat", 0).value, -3);
  assert.deepEqual(sent("set_adat_gain"), []);
});

/** A fake device that answers get_preamps_links with these pairs and records link writes. */
function answerLinks(client: FakeClient, pairs: Record<string, number[]>) {
  client.respond = async (call) => ({
    device_id: call.deviceId,
    command: call.command,
    sent_hex: "70",
    sent_len: 16,
    dry_run: false,
    response: call.command === "get_preamps_links" ? { entries: (pairs[call.deviceId] ?? []).map((linked) => ({ linked })) } : null,
    response_error: null,
  });
}

test("device preamp link flags are read and set per pair (which inputs change together is the workspace's)", async () => {
  const { client, store, sent } = setup();
  answerLinks(client, { "loopback-1": [0, 1, 0, 0, 0, 0] });
  const studio = store.inputs("loopback-1");
  assert.equal(studio.pairCount, 6);
  assert.equal(studio.pairLinked(1).value, false, "unknown until read");
  assert.equal(await studio.loadLinks(), true);
  assert.deepEqual([0, 1, 2].map((p) => studio.pairLinked(p).value), [false, true, false]);
  studio.setType(3, 1);

  studio.setPairLinked(0, true);
  await flush();
  assert.deepEqual(sent("set_stereo_link"), [{ periph_id: 0, channel_id: 0, linked: 1 }]);
  assert.equal(studio.pairLinked(0).value, true);
  assert.throws(() => studio.setPairLinked(6, true), RangeError);
});

test("Studio+ digital inputs link in pairs like its panel: lines, ADAT and S/PDIF each read and set with their own peripheral id", async () => {
  const { client, store, sent } = setup();
  const replies: Record<string, number[]> = { get_preamps_links: [0, 0, 0, 0, 0, 0], get_lines_links: [0, 1, 0, 0], get_adats_links: [1, 0, 0, 0, 0, 0, 0, 0], get_spdifs_links: [0] };
  client.respond = async (call) => ({ device_id: call.deviceId, command: call.command, sent_hex: "74", sent_len: 16, dry_run: false, response: call.command in replies ? { entries: (replies[call.command] ?? []).map((linked) => ({ linked })) } : null, response_error: null });

  const studio = store.inputs("loopback-1");
  assert.deepEqual(studio.digital.map((d) => [d.kind, d.linkPairs]), [["line", 4], ["adat", 8], ["spdif", 1]]);
  await studio.loadLinks();
  assert.deepEqual(client.invocations.map((c) => c.command).sort(), ["get_adats_links", "get_lines_links", "get_preamps_links", "get_spdifs_links"]);
  assert.deepEqual([studio.digitalPairLinked("line", 1).value, studio.digitalPairLinked("line", 0).value, studio.digitalPairLinked("adat", 0).value], [true, false, true]);

  studio.setDigitalPairLinked("line", 2, true);
  studio.setDigitalPairLinked("spdif", 0, true);
  await flush();
  assert.deepEqual(sent("set_stereo_link"), [{ periph_id: 1, channel_id: 2, linked: 1 }, { periph_id: 3, channel_id: 0, linked: 1 }]);
  assert.throws(() => studio.setDigitalPairLinked("adat", 8, true), RangeError);

  const quadro = store.inputs("loopback-0");
  assert.deepEqual(quadro.digital.map((d) => d.linkPairs), [0, 0], "the Quadro panel never links its digital inputs");
  assert.throws(() => quadro.setDigitalPairLinked("adat", 0, true), /does not link/);
});

test("inputs exist only for devices of known model", () => {
  const { store } = setup();
  assert.throws(() => store.inputs("usb:1"), /no known model/);
  assert.equal(store.inputs("loopback-0"), store.inputs("loopback-0"), "one model per device");
});

test("mic emulation: a target and one of its models per preamp, Quadro only, read with get_mic_emulations", async () => {
  const { client, store, sent } = setup();
  const quadro = store.inputs("loopback-0");
  const studio = store.inputs("loopback-1");
  assert.equal(quadro.hasMicEmulation, true);
  assert.equal(studio.hasMicEmulation, false, "only the Quadro panel has the mic emulation feature");
  assert.throws(() => studio.setEmulationTarget(0, 3), /no mic emulation/);

  // The targets are the Antelope microphones a preamp can have on it; "None" is MicTarget.ANY.
  assert.deepEqual(quadro.micTargets.map((t) => [t.value, t.name]), [
    [0, "None"],
    [1, "Edge Duo"],
    [2, "Verge"],
    [3, "Edge Solo"],
    [4, "Edge Quadro"],
    [5, "Accord"],
    [6, "Edge Note"],
  ]);
  // Each target has its own catalogue, and the same microphone sits at a different index in each.
  assert.equal(quadro.emulationModels(0).length, 0, "with no microphone there is nothing to emulate");
  assert.equal(quadro.emulationModels(3)[0], "Edge Solo", "index 0 is the microphone itself");
  assert.equal(quadro.emulationModels(3)[2], "Berlin 47 FT");
  assert.equal(quadro.emulationModels(1)[1], "Berlin 47 FT");

  // Every microphone has a catalogue, so a regenerated table that dropped one would be caught.
  for (const { value, name } of quadro.micTargets.slice(1)) {
    const models = quadro.emulationModels(value);
    assert.ok(models.length > 1, `${name} has a catalogue`);
    assert.equal(models[0], name, `${name}'s index 0 is the microphone itself`);
  }

  assert.deepEqual(quadro.emulation(0).value, { target: 0, model: 0, swap: false, pattern: 0 });
  client.respond = async (call) => ({
    device_id: call.deviceId,
    command: call.command,
    sent_hex: "74",
    sent_len: 16,
    dry_run: false,
    response: call.command === "get_mic_emulations" ? { entries: [{ target: 3, emu_model: 2, ch_swap: 0, pattern: 0 }, { target: 1, emu_model: 4, ch_swap: 1, pattern: 2 }] } : null,
    response_error: null,
  });
  assert.equal(await quadro.loadEmulations(), true);
  assert.deepEqual(quadro.emulation(0).value, { target: 3, model: 2, swap: false, pattern: 0 });
  assert.deepEqual(quadro.emulation(1).value, { target: 1, model: 4, swap: true, pattern: 2 });
  assert.equal(await studio.loadEmulations(), false, "nothing is read from a model without it");

  // Every change carries all five fields, and the pattern is passed back as read: the stereo
  // pattern presets are a pair-wide feature this does not yet drive.
  quadro.setEmulationModel(1, 6);
  quadro.setEmulationSwap(1, false);
  await flush();
  assert.deepEqual(sent("set_mic_emulation"), [
    { preamp_ch: 1, target: 1, emu_model: 6, ch_swap: 1, pattern: 2 },
    { preamp_ch: 1, target: 1, emu_model: 6, ch_swap: 0, pattern: 2 },
  ]);

  // A microphone's catalogue is its own, so changing the target cannot keep the old model.
  quadro.setEmulationTarget(0, 6);
  await flush();
  assert.deepEqual(sent("set_mic_emulation").at(-1), { preamp_ch: 0, target: 6, emu_model: 0, ch_swap: 0, pattern: 0 });
  assert.deepEqual(quadro.emulation(0).value, { target: 6, model: 0, swap: false, pattern: 0 });

  assert.throws(() => quadro.setEmulationTarget(0, 7), RangeError);
  assert.throws(() => quadro.setEmulationModel(0, 14), RangeError, "the Edge Note has fourteen, 0..13");
  assert.throws(() => quadro.setEmulationModel(4, 0), RangeError, "the Quadro has four preamps");
});
