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

test("a dual-capsule microphone covers more than one preamp: the Edge Duo two, the Edge Quadro four", async () => {
  const { store, sent } = setup();
  await store.start();
  const quadro = store.inputs("loopback-0");

  // From the panel's own regrouping (link_len 4 for EDGE_QUADRO, 2 for EDGE_DUO, 1 otherwise) and
  // the Edge manual: the Duo's two membranes are two XLRs, the Quadro's two heads are four.
  assert.deepEqual([0, 1, 2, 3, 4, 5, 6].map((target) => quadro.emulationSpan(target)), [1, 2, 1, 1, 4, 1, 1]);
  assert.deepEqual(quadro.emulationChannels(3, 1), [2, 3], "a Duo on preamp 4 covers the pair it belongs to");
  assert.deepEqual(quadro.emulationChannels(2, 4), [0, 1, 2, 3], "a Quadro covers its whole block of four");
  // Channels 3 and 4 are the Top head (`is_quad_top`: ch % 4 >= 2), each head with its own emulation.
  assert.deepEqual(quadro.emulationHeads(0, 4), [{ name: "Bottom", channel: 0 }, { name: "Top", channel: 2 }]);
  assert.deepEqual(quadro.emulationHeads(0, 1), [{ name: "", channel: 0 }]);

  // The target reaches every channel of the microphone, because it is one microphone.
  quadro.setEmulationTarget(1, 1);
  await flush();
  assert.deepEqual(sent("set_mic_emulation"), [
    { preamp_ch: 0, target: 1, emu_model: 0, ch_swap: 0, pattern: 0 },
    { preamp_ch: 1, target: 1, emu_model: 0, ch_swap: 0, pattern: 0 },
  ]);
  // And the app links those preamps, absolute, so their gain and 48V move together.
  const link = store.links.linkOf("preamp", "loopback-0", 0);
  assert.equal(link?.mode, "absolute");
  assert.deepEqual(link?.members.map((m) => m.channel), [0, 1]);

  // A membrane pair shares one emulation; the swap is the microphone's, so it covers both.
  quadro.setEmulationModel(1, 3);
  quadro.setEmulationSwap(0, true);
  await flush();
  assert.deepEqual(sent("set_mic_emulation").slice(2), [
    { preamp_ch: 0, target: 1, emu_model: 3, ch_swap: 0, pattern: 0 },
    { preamp_ch: 1, target: 1, emu_model: 3, ch_swap: 0, pattern: 0 },
    { preamp_ch: 0, target: 1, emu_model: 3, ch_swap: 1, pattern: 0 },
    { preamp_ch: 1, target: 1, emu_model: 3, ch_swap: 1, pattern: 0 },
  ]);

  // Going back to a single-channel microphone takes the link away with it.
  quadro.setEmulationTarget(1, 3);
  assert.equal(store.links.linkOf("preamp", "loopback-0", 0), undefined);
  assert.deepEqual(quadro.emulation(0).value, { target: 3, model: 0, swap: false, pattern: 0 });
  assert.deepEqual(quadro.emulation(1).value, { target: 3, model: 0, swap: false, pattern: 0 });
});

test("the Edge Quadro's two heads take an emulation each, and it needs four preamps to fit", async () => {
  const { store, sent } = setup();
  await store.start();
  const quadro = store.inputs("loopback-0");
  quadro.setEmulationTarget(0, 4);
  await flush();
  assert.equal(sent("set_mic_emulation").length, 4, "all four channels are the one microphone");

  // Each head has its own emulation, so a model reaches only that head's two membranes.
  quadro.setEmulationModel(2, 5);
  await flush();
  assert.deepEqual(sent("set_mic_emulation").slice(4), [
    { preamp_ch: 2, target: 4, emu_model: 5, ch_swap: 0, pattern: 0 },
    { preamp_ch: 3, target: 4, emu_model: 5, ch_swap: 0, pattern: 0 },
  ]);
  assert.deepEqual([0, 1, 2, 3].map((i) => quadro.emulation(i).value.model), [0, 0, 5, 5]);

  // Both heads are one microphone, so the swap covers all four membranes.
  quadro.setEmulationSwap(3, true);
  await flush();
  assert.deepEqual(sent("set_mic_emulation").slice(6).map((a) => [(a as Record<string, number>)["preamp_ch"], (a as Record<string, number>)["ch_swap"]]), [
    [0, 1],
    [1, 1],
    [2, 1],
    [3, 1],
  ]);

  assert.throws(() => store.inputs("loopback-1").emulationChannels(0, 4), /no mic emulation/, "the Studio+ has none of this");
});

test("a device reporting a mixed set of microphones is not written over by its neighbour's head", async () => {
  const { client, store, sent } = setup();
  await store.start();
  const quadro = store.inputs("loopback-0");
  // The device can report what the app would not have made: an Edge Quadro on three preamps and
  // something else on the fourth, which is what unplugging one of its XLRs leaves (the Edge manual).
  client.respond = async (call) => ({
    device_id: call.deviceId,
    command: call.command,
    sent_hex: "74",
    sent_len: 16,
    dry_run: false,
    response:
      call.command === "get_mic_emulations"
        ? { entries: [{ target: 4, emu_model: 1, ch_swap: 0, pattern: 0 }, { target: 4, emu_model: 1, ch_swap: 0, pattern: 0 }, { target: 3, emu_model: 2, ch_swap: 0, pattern: 0 }, { target: 4, emu_model: 1, ch_swap: 0, pattern: 0 }] }
        : null,
    response_error: null,
  });
  assert.equal(await quadro.loadEmulations(), true);

  quadro.setEmulationModel(3, 5);
  await flush();
  assert.deepEqual(sent("set_mic_emulation"), [{ preamp_ch: 3, target: 4, emu_model: 5, ch_swap: 0, pattern: 0 }], "preamp 3 has its own microphone and keeps it");
  assert.equal(quadro.emulation(2).value.model, 2);
});

test("polar pattern: a head's pattern is its model's, from omni through cardioid to figure-8", async () => {
  const { store, sent } = setup();
  await store.start();
  const quadro = store.inputs("loopback-0");

  // Only the Edge Duo, Edge Quadro and Accord models have a polar pattern; the single-membrane
  // microphones inherit a base whose pattern means nothing (`MicModelBase.pattern_to_pangle`).
  quadro.setEmulationTarget(0, 3);
  assert.equal(quadro.emulationPattern(0), undefined, "an Edge Solo has no polar pattern");

  quadro.setEmulationTarget(0, 1);
  // Index 0 is the microphone itself: a continuous sweep, 0..100 with 50 the middle.
  assert.deepEqual(quadro.emulationPattern(0), { value: 0, min: 0, max: 100, angle: 1, label: "Omni", steps: undefined });

  // Berlin 67 is a three-position switch, as the microphone it models is.
  quadro.setEmulationModel(0, 3);
  const berlin67 = quadro.emulationPattern(0);
  assert.deepEqual(berlin67?.steps, [
    { value: 0, label: "Omni" },
    { value: 1, label: "Cardioid" },
    { value: 2, label: "Figure-8" },
  ]);
  // Berlin 47 FT has one fixed pattern, so there is nothing to choose.
  quadro.setEmulationModel(0, 1);
  assert.deepEqual(quadro.emulationPattern(0)?.steps, [{ value: 0, label: "Cardioid" }]);

  quadro.setEmulationModel(0, 3);
  await flush();
  const before = sent("set_mic_emulation").length;
  quadro.setEmulationPattern(0, 2);
  await flush();
  // The pattern belongs to the head, so it reaches both of its membranes and no further.
  assert.deepEqual(sent("set_mic_emulation").slice(before).map((a) => [(a as Record<string, number>)["preamp_ch"], (a as Record<string, number>)["pattern"]]), [
    [0, 2],
    [1, 2],
  ]);
  assert.equal(quadro.emulationPattern(0)?.label, "Figure-8");
  assert.throws(() => quadro.setEmulationPattern(0, 3), RangeError);
});

test("stereo presets set both of an Edge Quadro's heads, and only where its models allow", async () => {
  const { store, sent } = setup();
  await store.start();
  const quadro = store.inputs("loopback-0");
  quadro.setEmulationTarget(0, 4);
  // Berlin 67 on both heads: three positions each, so every preset is reachable.
  quadro.setEmulationModel(0, 3);
  quadro.setEmulationModel(2, 3);

  assert.deepEqual(quadro.emulationPresets(0), [
    { value: 0, name: "None", available: true },
    { value: 1, name: "XY", available: true },
    { value: 2, name: "M/S", available: true },
    { value: 3, name: "Blumlein", available: true },
  ]);
  assert.equal(quadro.emulationPreset(0), 0, "nothing is a preset until the patterns say so");

  await flush();
  const before = sent("set_mic_emulation").length;
  quadro.setEmulationPreset(0, 1);
  await flush();
  // XY is both capsules at cardioid; the offset of 90 degrees is the user turning the head.
  assert.deepEqual(sent("set_mic_emulation").slice(before).map((a) => [(a as Record<string, number>)["preamp_ch"], (a as Record<string, number>)["pattern"]]), [
    [0, 1],
    [1, 1],
    [2, 1],
    [3, 1],
  ]);
  assert.equal(quadro.emulationPreset(0), 1);

  // Blumlein is both at figure-8; M/S is the top head at cardioid over the bottom at figure-8.
  quadro.setEmulationPreset(0, 3);
  assert.deepEqual([0, 1, 2, 3].map((i) => quadro.emulationPattern(i)?.label), ["Figure-8", "Figure-8", "Figure-8", "Figure-8"]);
  assert.equal(quadro.emulationPreset(0), 3);
  quadro.setEmulationPreset(0, 2);
  assert.deepEqual([0, 2].map((i) => quadro.emulationPattern(i)?.label), ["Figure-8", "Cardioid"], "the bottom head is the side, the top the mid");
  assert.equal(quadro.emulationPreset(0), 2);

  // Oxford 4038 is fixed at figure-8, so a head carrying it can be Blumlein or the side of M/S,
  // and never XY (`xy_supported`: both heads must have cardioid).
  quadro.setEmulationModel(2, 6);
  assert.deepEqual(quadro.emulationPresets(0).map((p) => p.available), [true, false, true, true]);

  // A head's pattern is its own: pointing one capsule leaves the other where it was.
  quadro.setEmulationModel(2, 3);
  quadro.setEmulationPreset(0, 1);
  const mark = sent("set_mic_emulation").length;
  quadro.setEmulationPattern(2, 2);
  await flush();
  assert.deepEqual(sent("set_mic_emulation").slice(mark).map((a) => (a as Record<string, number>)["preamp_ch"]), [2, 3]);
  assert.deepEqual([0, 2].map((i) => quadro.emulationPattern(i)?.label), ["Cardioid", "Figure-8"]);

  // Berlin 47 FT is fixed at cardioid, so a head carrying it can never be Blumlein, and the
  // microphone can still be XY or the mid of M/S.
  quadro.setEmulationModel(2, 1);
  assert.deepEqual(quadro.emulationPresets(0).map((p) => p.available), [true, true, true, false]);

  // A microphone with one head has no stereo preset at all.
  quadro.setEmulationTarget(0, 1);
  assert.deepEqual(quadro.emulationPresets(0), []);
});

/** A `get_feature_mask` payload with these licence bits set, counted as the panel counts them. */
function featureMask(...bits: number[]): Uint8Array {
  const payload = new Uint8Array(290);
  // The panel's `parse_feature_mask` skips the first byte, so filling it must change nothing.
  payload[0] = 0xff;
  for (const bit of bits) payload[1 + (bit >> 3)] = (payload[1 + (bit >> 3)] as number) | (1 << (bit & 7));
  return payload;
}

test("mic emulation licence: the device's feature mask decides which microphones and emulations are offered", async () => {
  const { client, store, sent } = setup();
  await store.start();
  const quadro = store.inputs("loopback-0");
  const offered = () => quadro.micTargets.map((t) => [t.name, t.licensed]);

  // Until the mask is read, nothing is known to be unlicensed, so everything is offered.
  assert.ok(quadro.micTargets.every((t) => t.licensed));
  assert.equal(quadro.emulationLicensed(2, 3), true);

  // An Edge Duo with Berlin 47 FT and Berlin 67, and an Edge Solo with nothing but itself; bit 0
  // is `device_assigned`. Edge Duo's features are mic_emu_edge (408), then 410, 411, 412 for
  // Berlin 47 FT, 87 and 67; Edge Solo's own is 499, and 500 is its Tokyo 800T.
  let mask: unknown = featureMask(0, 408, 410, 412, 499);
  client.respond = async (call) => ({
    device_id: call.deviceId,
    command: call.command,
    sent_hex: "74",
    sent_len: 16,
    dry_run: false,
    response: call.command === "get_feature_mask" ? { payload: mask } : call.command === "get_mic_emulations" ? { entries: [{ target: 2, emu_model: 5, ch_swap: 0, pattern: 0 }, { target: 1, emu_model: 2, ch_swap: 0, pattern: 0 }] } : null,
    response_error: null,
  });
  assert.equal(await quadro.loadLicence(), true);
  assert.deepEqual(offered(), [
    ["None", true],
    ["Edge Duo", true],
    ["Verge", false],
    ["Edge Solo", true],
    ["Edge Quadro", false],
    ["Accord", false],
    ["Edge Note", false],
  ]);
  // Licensing is per emulation as well as per microphone, and index 0 is the microphone's own.
  assert.deepEqual([0, 1, 2, 3, 4].map((model) => quadro.emulationLicensed(1, model)), [true, true, false, true, false]);
  assert.deepEqual([0, 1].map((model) => quadro.emulationLicensed(3, model)), [true, false]);
  assert.equal(quadro.emulationLicensed(0, 0), true, "no microphone needs no licence");
  // The catalogue itself is unchanged: an index is the device's, so nothing may shift.
  assert.equal(quadro.emulationModels(1)[3], "Berlin 67");

  // What the licence does not cover cannot be picked.
  assert.throws(() => quadro.setEmulationTarget(0, 2), /not licensed/);
  quadro.setEmulationTarget(0, 1);
  assert.throws(() => quadro.setEmulationModel(0, 2), /not licensed/);
  quadro.setEmulationModel(0, 3);
  await flush();
  assert.deepEqual(sent("set_mic_emulation").at(-1), { preamp_ch: 1, target: 1, emu_model: 3, ch_swap: 0, pattern: 0 });

  // A device already on an emulation its licence does not cover still shows it as it is, and
  // what else is on that microphone can still change.
  assert.equal(await quadro.loadEmulations(), true);
  assert.deepEqual(quadro.emulation(0).value, { target: 2, model: 5, swap: false, pattern: 0 });
  assert.deepEqual(quadro.emulation(1).value, { target: 1, model: 2, swap: false, pattern: 0 });
  assert.equal(quadro.emulationLicensed(2, 5), false);
  quadro.setEmulationTarget(0, 3);
  assert.equal(quadro.emulation(0).value.target, 3);

  // A reply too short to hold the microphones' bits is not a licence: everything is offered again.
  mask = new Uint8Array(100);
  assert.equal(await quadro.loadLicence(), false);
  assert.ok(quadro.micTargets.every((t) => t.licensed));
  assert.deepEqual(store.notices.value, []);
  // The read is the page's own, and asks nothing of the Studio+, which has no mic emulation.
  assert.equal(await store.inputs("loopback-1").loadLicence(), false);
  assert.equal(client.invocations.filter((call) => call.command === "get_feature_mask" && call.deviceId === "loopback-1").length, 0);
});

test("a feature mask that cannot be read offers every microphone and emulation, without an error notice", async () => {
  const { client, store } = setup();
  await store.start();
  const quadro = store.inputs("loopback-0");
  // The loopback answers every read with an empty payload, and a dry run with none at all; neither
  // may hide what the device might well be licensed for.
  client.respond = async (call) => ({
    device_id: call.deviceId,
    command: call.command,
    sent_hex: "74",
    sent_len: 16,
    dry_run: false,
    response: null,
    response_error: "could not decode 0 bytes of response for 'get_feature_mask': TruncatedPayload",
  });
  assert.equal(await quadro.loadLicence(), false);
  assert.ok(quadro.micTargets.every((t) => t.licensed));
  assert.ok([1, 2, 3, 4, 5, 6].every((target) => quadro.emulationModels(target).every((_, model) => quadro.emulationLicensed(target, model))));
  quadro.setEmulationTarget(0, 5);
  quadro.setEmulationModel(0, 18);
  assert.deepEqual(store.notices.value, []);
});

test("a device that will not answer get_mic_emulations leaves emulation unknown, without an error notice", async () => {
  const { client, store } = setup();
  await store.start();
  const quadro = store.inputs("loopback-0");
  // The Inputs page reads this on its own every time it opens, so a refusal is not the user's
  // problem (P63) — the loopback, for one, answers every read with an empty payload.
  client.respond = async (call) => ({
    device_id: call.deviceId,
    command: call.command,
    sent_hex: "74",
    sent_len: 16,
    dry_run: false,
    response: null,
    response_error: "could not decode 0 bytes of response for 'get_mic_emulations': TruncatedPayload",
  });
  assert.equal(await quadro.loadEmulations(), false);
  assert.deepEqual(quadro.emulation(0).value, { target: 0, model: 0, swap: false, pattern: 0 });
  assert.deepEqual(store.notices.value, []);
});
