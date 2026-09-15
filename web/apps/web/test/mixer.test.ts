import { test } from "node:test";
import assert from "node:assert/strict";

import { GazelleError } from "gazelle-audio-client";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { clampPan, formatLevel, formatPan, levelFromDb, meterDeflection, PAN_CENTRE } from "../src/store/mixer.ts";
import { Store } from "../src/store/store.ts";
import { builtInThemes, device, FakeClient, flush, MemoryStorage } from "./fake-client.ts";

function setup() {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"), device("loopback-1", "studio", "Zen Studio+"), device("usb:1", null, null));
  const frames: (() => void)[] = [];
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), requestFrame: (callback) => frames.push(callback), themeSources: builtInThemes });
  return { client, frames, store };
}

const sent = (client: FakeClient, command: string) => client.invocations.filter((call) => call.command === command);

test("levels, pans and meters use the panels' scales", () => {
  assert.deepEqual([formatLevel(0), formatLevel(6), formatLevel(90)], ["0 dB", "-6 dB", "-90 dB"]);
  assert.deepEqual([levelFromDb(-6), levelFromDb(3), levelFromDb(-120)], [6, 0, 90]);
  assert.deepEqual([clampPan(0), clampPan(26), clampPan(27), clampPan(38), clampPan(39), clampPan(63)], [2, 26, 32, 32, 39, 62], "2..62, with 27..38 snapping to centre");
  assert.deepEqual([formatPan(PAN_CENTRE), formatPan(40), formatPan(2)], ["0", "+8", "-30"]);
  assert.deepEqual([96, 60, 50, 40, 30, 20, 10, 0].map(meterDeflection), [0, 0, 5, 15, 30, 50, 75, 100], "Antelope's piecewise meter anchors");
});

test("Quadro strips send set_mixer with the whole strip, coalesced per strip, master on channel 0", async () => {
  const { client, store } = setup();
  const mixer = store.mixer("loopback-0", 1);
  assert.equal(mixer.hasSend, false);
  assert.equal(mixer.stateKnown, false, "state is not readable yet, so values are defaults");

  mixer.setLevel(3, 20);
  mixer.setPan(3, 50);
  mixer.toggleMute(3);
  mixer.setLevel("master", 10);
  await flush();

  const calls = sent(client, "set_mixer");
  assert.deepEqual(calls.map((c) => c.args), [
    { mixer_id: 1, channel: 4, level: 20, pan: PAN_CENTRE, mute: 0, solo: 0 },
    { mixer_id: 1, channel: 4, level: 20, pan: 50, mute: 0, solo: 0 },
    { mixer_id: 1, channel: 4, level: 20, pan: 50, mute: 1, solo: 0 },
    { mixer_id: 1, channel: 0, level: 10, pan: PAN_CENTRE, mute: 0, solo: 0 },
  ]);
  assert.deepEqual(calls.map((c) => c.options), [{ coalesce: "mixer:loopback-0:1:4" }, { coalesce: "mixer:loopback-0:1:4" }, { coalesce: "mixer:loopback-0:1:4" }, { coalesce: "mixer:loopback-0:1:0" }]);
  assert.deepEqual(mixer.strip(3).value, { level: 20, pan: 50, mute: true, solo: false, send: 0, linked: false });
  assert.deepEqual(store.lastSent.value, { deviceId: "loopback-0", command: "set_mixer", hex: "70", dryRun: true });
});

test("Studio+ strips send set_mixer_cfg including send", async () => {
  const { client, store } = setup();
  const mixer = store.mixer("loopback-1", 3);
  assert.equal(mixer.hasSend, true);
  mixer.setSend(0, 300);
  mixer.toggleSolo(0);
  await flush();
  assert.deepEqual(sent(client, "set_mixer_cfg").map((c) => c.args), [
    { mixer_id: 3, channel: 1, level: 0, pan: PAN_CENTRE, mute: 0, solo: 0, send: 255 },
    { mixer_id: 3, channel: 1, level: 0, pan: PAN_CENTRE, mute: 0, solo: 1, send: 255 },
  ]);
});

test("linking sends the stereo link and mirrors level, mute and solo but not pan", async () => {
  const { client, store } = setup();
  const mixer = store.mixer("loopback-0", 2);
  mixer.toggleLink(5);
  await flush();
  assert.deepEqual(sent(client, "set_stereo_link").map((c) => c.args), [{ periph_id: 3, channel_id: (4 + 2 * 32) / 2, linked: 1 }], "Quadro periph 3, link id (strip + mixer*32)/2");
  assert.equal(mixer.strip(4).value.linked && mixer.strip(5).value.linked, true);

  client.invocations.length = 0;
  mixer.setLevel(5, 30);
  mixer.setPan(5, 10);
  await flush();
  assert.deepEqual(sent(client, "set_mixer").map((c) => [c.args?.["channel"], c.args?.["level"], c.args?.["pan"]]), [
    [6, 30, PAN_CENTRE],
    [5, 30, PAN_CENTRE],
    [6, 30, 10],
  ]);
  assert.equal(mixer.strip(4).value.pan, PAN_CENTRE, "the partner keeps its pan");

  const studio = store.mixer("loopback-1", 0);
  studio.toggleLink(0);
  await flush();
  assert.deepEqual(sent(client, "set_stereo_link").at(-1)?.args, { periph_id: 4, channel_id: 0, linked: 1 }, "Studio+ periph 4");
});

test("activating a mixer points the meters at it where the source is known, and meters latch clips", async () => {
  const { client, frames, store } = setup();
  const studio = store.mixer("loopback-1", 3);
  const stopStudio = studio.activate();
  const quadroFirst = store.mixer("loopback-0", 0);
  const stopQuadro = quadroFirst.activate();
  const quadroFourth = store.mixer("loopback-0", 3);
  const stopFourth = quadroFourth.activate();
  await flush();
  assert.deepEqual(sent(client, "set_peak_source").map((c) => [c.deviceId, c.args]), [
    ["loopback-1", { bank_id: 1, source_id: 3 }],
    ["loopback-0", { bank_id: 0, source_id: 15 }],
  ]);
  assert.equal(quadroFourth.meterSourceSelectable, false, "Quadro mixer 4's meter source is not known");

  const report = client.cyclic.get("loopback-1|0x73");
  assert.ok(report);
  const peaks = new Uint8Array(32).fill(60);
  peaks[2] = 0;
  peaks[5] = 18;
  report({ peaks_mixer: peaks });
  frames.shift()?.();
  assert.deepEqual([studio.meter(5).value, studio.meter(2).value], [18, 0]);
  assert.deepEqual([studio.clipped(2).value, studio.clipped(5).value], [true, false]);
  peaks[2] = 30;
  report({ peaks_mixer: new Uint8Array(peaks) });
  frames.shift()?.();
  assert.equal(studio.clipped(2).value, true, "clip stays latched");
  studio.clearClip(2);
  assert.equal(studio.clipped(2).value, false);

  stopStudio();
  stopQuadro();
  stopFourth();
  assert.equal(client.cyclic.size, 0, "deactivating stops following the reports");
});

test("failed commands post a notice; superseded ones do not", async () => {
  const { client, store } = setup();
  const mixer = store.mixer("loopback-0", 0);
  client.respond = async () => {
    throw new GazelleError("superseded", "replaced");
  };
  mixer.setLevel(0, 5);
  await flush();
  assert.equal(store.notices.value.length, 0, "a superseded command is not a failure");

  client.respond = async () => {
    throw new GazelleError("bad_value", "level out of range");
  };
  mixer.setLevel(0, 6);
  await flush();
  assert.deepEqual(store.notices.value.map((n) => [n.level, n.message]), [["error", "set_mixer failed: level out of range"]]);
});

test("mixers exist only for known models and within the topology", () => {
  const { store } = setup();
  assert.equal(store.mixer("loopback-0", 0), store.mixer("loopback-0", 0), "one model per mixer");
  assert.throws(() => store.mixer("usb:1", 0), /no known model/);
  assert.throws(() => store.mixer("loopback-0", 4), /mixers 0..3/);
  assert.throws(() => store.mixer("loopback-0", 0).setLevel(32, 0), /outside 0..31/);
  assert.equal(store.topology("loopback-1")?.mixers.command, "set_mixer_cfg");
  assert.equal(store.topology("usb:1"), undefined);
});
