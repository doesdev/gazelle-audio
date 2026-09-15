import { test } from "node:test";
import assert from "node:assert/strict";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { ECHO_HOLD_MS } from "../src/store/inputs.ts";
import { formatVolume, VOLUME_MAX } from "../src/store/outputs.ts";
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

const quadroVolume = (volume: number, mute = 0, dim = 0) => ({ volume, mute, dim_on: dim, mono: 0, trim: 0 });

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
  assert.deepEqual(quadro.state(2).value, { known: false, volume: 0, mute: false, dim: false });
  quadro.activate();
  report("loopback-0", { volumes: [quadroVolume(96, 1), quadroVolume(10), quadroVolume(30, 0, 1), quadroVolume(0), quadroVolume(5), quadroVolume(5)] });
  assert.deepEqual(quadro.state(0).value, { known: true, volume: 96, mute: true, dim: false });
  assert.deepEqual(quadro.state(2).value, { known: true, volume: 30, mute: false, dim: true });

  const studio = store.outputs("loopback-1");
  studio.activate();
  report("loopback-1", { monitor_vol: 12, monitor_mute: 0, hp1_vol: 40, hp1_mute: 1, hp2_vol: 0, hp2_mute: 0, line_out_vol: 96, line_out_mute: 0, reamp_vol: 20, reamp_mute: 1 });
  assert.deepEqual(studio.state(1).value, { known: true, volume: 40, mute: true, dim: false });
  assert.deepEqual(studio.state(4).value, { known: true, volume: 20, mute: true, dim: false });
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
