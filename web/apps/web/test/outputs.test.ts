import { test } from "node:test";
import assert from "node:assert/strict";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { ECHO_HOLD_MS } from "../src/store/inputs.ts";
import { formatVolume, TRIM_LABELS, VOLUME_MAX } from "../src/store/outputs.ts";
import { Store } from "../src/store/store.ts";
import { builtInThemes, device, FakeClient, flush, MemoryStorage } from "./fake-client.ts";

function setup() {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"), device("loopback-1", "studio", "Zen Studio+"), device("usb:1", null, null));
  const frames: (() => void)[] = [];
  const timers = new ManualTimers();
  const store = new Store(client, { timers, storage: new MemoryStorage(), requestFrame: (callback) => frames.push(callback), themeSources: builtInThemes });
  const report = (deviceId: string, fields: Record<string, unknown>) => {
    const listener = client.cyclic.get(`${deviceId}|0x73`);
    if (listener === undefined) throw new Error(`${deviceId} is not watching 0x73`);
    listener(fields);
    for (const frame of frames.splice(0)) frame();
  };
  const sent = (deviceId: string, command: string) => client.invocations.filter((c) => c.deviceId === deviceId && c.command === command).map((c) => c.args);
  return { store, timers, report, sent };
}

const quadroVolume = (volume: number, mute = 0, dim = 0, mono = 0) => ({ volume, mute, dim_on: dim, mono, trim: 0 });

test("each family's outputs and ids follow its panel: Quadro monitor, HP1, HP2, line out with dim; Studio+ adds reamp, without dim", () => {
  const { store } = setup();
  assert.deepEqual(store.outputs("loopback-0").outputs, [
    { id: 0, name: "Monitor", dim: true },
    { id: 1, name: "HP1", dim: true },
    { id: 2, name: "HP2", dim: true },
    { id: 3, name: "Line out", dim: true },
  ]);
  assert.deepEqual(store.outputs("loopback-1").outputs.map((o) => [o.id, o.name, o.dim]), [
    [0, "Monitor", false],
    [1, "HP1", false],
    [2, "HP2", false],
    [3, "Line out", false],
    [4, "Reamp", false],
  ]);
  assert.throws(() => store.outputs("usb:1"), /no known model/);
  assert.equal(store.outputs("loopback-0"), store.outputs("loopback-0"), "one model per device");
});

test("state comes from the report: the Quadro's volumes array by id, the Studio+'s named fields", () => {
  const { store, report } = setup();
  const quadro = store.outputs("loopback-0");
  assert.deepEqual(quadro.state(2).value, { known: false, volume: 0, mute: false, dim: false, mono: false });
  quadro.activate();
  report("loopback-0", { volumes: [quadroVolume(96, 1), quadroVolume(10, 0, 0, 1), quadroVolume(30, 0, 1), quadroVolume(0), quadroVolume(5), quadroVolume(5)] });
  assert.deepEqual(quadro.state(0).value, { known: true, volume: 96, mute: true, dim: false, mono: false });
  assert.deepEqual(quadro.state(1).value, { known: true, volume: 10, mute: false, dim: false, mono: true }, "the Quadro reports mono per output");
  assert.deepEqual(quadro.state(2).value, { known: true, volume: 30, mute: false, dim: true, mono: false });

  const studio = store.outputs("loopback-1");
  studio.activate();
  report("loopback-1", { monitor_vol: 12, monitor_mute: 0, hp1_vol: 40, hp1_mute: 1, hp2_vol: 0, hp2_mute: 0, line_out_vol: 96, line_out_mute: 0, reamp_vol: 20, reamp_mute: 1 });
  assert.deepEqual(studio.state(1).value, { known: true, volume: 40, mute: true, dim: false, mono: false });
  assert.deepEqual(studio.state(4).value, { known: true, volume: 20, mute: true, dim: false, mono: false });
  assert.throws(() => studio.state(5), RangeError);
});

test("changes send set_volume (clamped to 0..96), set_mute and, on the Quadro, set_dim; they outrank stale reports for the hold", async () => {
  const { store, report, sent, timers } = setup();
  const quadro = store.outputs("loopback-0");
  quadro.activate();
  const volumes = (monitor: number) => report("loopback-0", { volumes: [quadroVolume(monitor), quadroVolume(0), quadroVolume(0), quadroVolume(0), quadroVolume(0), quadroVolume(0)] });
  volumes(30);

  quadro.setVolume(0, 120);
  quadro.setVolume(3, -4);
  quadro.setMute(1, true);
  quadro.setDim(2, true);
  await flush();
  assert.deepEqual(sent("loopback-0", "set_volume"), [{ id: 0, volume: VOLUME_MAX }, { id: 3, volume: 0 }]);
  assert.deepEqual(sent("loopback-0", "set_mute"), [{ id: 1, mute: 1 }]);
  assert.deepEqual(sent("loopback-0", "set_dim"), [{ periph_id: 2, dim: 1 }]);

  volumes(30);
  assert.equal(quadro.state(0).value.volume, 96, "a report still carrying the old value does not undo the change");
  timers.advance(ECHO_HOLD_MS);
  assert.equal(quadro.state(0).value.volume, 30, "after the hold the device's report wins");

  const studio = store.outputs("loopback-1");
  studio.setVolume(4, 50);
  await flush();
  assert.deepEqual(sent("loopback-1", "set_volume"), [{ id: 4, volume: 50 }]);
  assert.throws(() => studio.setDim(0, true), /no dim/);
});

test("volumes show as dB of attenuation, with 96 as -inf", () => {
  assert.deepEqual([formatVolume(0), formatVolume(10), formatVolume(95), formatVolume(96)], ["0 dB", "-10 dB", "-95 dB", "-inf"]);
});

test("trims: seven steps from 20 to 14 dBu, set with each model's command and read from the report", async () => {
  const { store, report, sent } = setup();
  assert.deepEqual(TRIM_LABELS, ["20 dBu", "19 dBu", "18 dBu", "17 dBu", "16 dBu", "15 dBu", "14 dBu"]);
  const quadro = store.outputs("loopback-0");
  const studio = store.outputs("loopback-1");
  assert.deepEqual(quadro.trims, [{ id: 0, name: "Monitor" }, { id: 1, name: "Line out" }]);
  assert.deepEqual(studio.trims, [{ id: 0, name: "Monitor" }, { id: 1, name: "Line out" }, { id: 2, name: "ADC" }]);

  assert.deepEqual(quadro.trim(1).value, { known: false, index: 0 });
  quadro.activate();
  report("loopback-0", { monitor_trim: 2, line_out_trim: 5 });
  assert.deepEqual([quadro.trim(0).value, quadro.trim(1).value], [{ known: true, index: 2 }, { known: true, index: 5 }]);

  quadro.setTrim(1, 9);
  studio.setTrim(2, 3);
  await flush();
  const [config] = sent("loopback-0", "set_trim_config") as { trim_id: number; control: number; level: Uint8Array[] }[];
  assert.ok(config);
  assert.deepEqual([config.trim_id, config.control, config.level.length], [1, 1, 32], "the Quadro panel sends trim id, control 1 and 32 level pairs");
  assert.deepEqual([...(config.level[0] as Uint8Array)], [6, 0], "the first pair carries the step (clamped to 14 dBu)");
  assert.ok(config.level.slice(1).every((pair) => [...pair].every((b) => b === 0)));
  assert.equal(quadro.trim(1).value.index, 6);
  assert.deepEqual(sent("loopback-1", "set_trim"), [{ id: 2, trim_idx: 3 }]);
  assert.throws(() => quadro.setTrim(2, 0), RangeError, "the Quadro has no ADC trim");
});

test("talkback is the Studio+'s: talk, mic level and where it goes; the Quadro has none", async () => {
  const { store, report, sent } = setup();
  const studio = store.outputs("loopback-1");
  const quadro = store.outputs("loopback-0");
  assert.equal(quadro.talkback, undefined);
  assert.throws(() => quadro.setTalk(true), /no talkback/);
  assert.deepEqual(studio.talkback?.destinations, [{ id: 0, name: "HP1" }, { id: 1, name: "HP2" }, { id: 2, name: "Monitor" }]);

  studio.activate();
  report("loopback-1", { talkback_on: 1, tb_mic_volume: 40, hp1_enabled: 1, hp2_enabled: 0, mon_enabled: 1 });
  assert.deepEqual(studio.talk.value, { known: true, on: true, volume: 40, to: [true, false, true] });

  studio.setTalk(false);
  studio.setTalkbackVolume(300);
  studio.setTalkbackTo(1, true);
  await flush();
  assert.deepEqual(sent("loopback-1", "set_talk"), [{ on: 0 }]);
  assert.deepEqual(sent("loopback-1", "set_tbk_vol"), [{ volume: 255 }]);
  assert.deepEqual(sent("loopback-1", "set_tbk_enable"), [{ id: 1, enabled: 1 }]);
  assert.deepEqual(studio.talk.value, { known: true, on: false, volume: 255, to: [true, true, true] });
  assert.throws(() => studio.setTalkbackTo(3, true), RangeError);
});
