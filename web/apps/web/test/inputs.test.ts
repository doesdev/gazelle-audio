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

test("inputs exist only for devices of known model", () => {
  const { store } = setup();
  assert.throws(() => store.inputs("usb:1"), /no known model/);
  assert.equal(store.inputs("loopback-0"), store.inputs("loopback-0"), "one model per device");
});
