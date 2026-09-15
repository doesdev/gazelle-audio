import { test } from "node:test";
import assert from "node:assert/strict";

import { GazelleError } from "gazelle-audio-client";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { clampPan, formatLevel, formatPan, formatSend, levelFromDb, meterDeflection, PAN_CENTRE } from "../src/store/mixer.ts";
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
  assert.equal(mixer.stateKnown.value, false, "until loaded, values are defaults");

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
    { mixer_id: 3, channel: 1, level: 0, pan: PAN_CENTRE, mute: 0, solo: 0, send: 96 },
    { mixer_id: 3, channel: 1, level: 0, pan: PAN_CENTRE, mute: 0, solo: 1, send: 96 },
  ], "send is dB of attenuation, 0 (loudest) to 96 (off), as captured in hardware session 1");
  assert.deepEqual([formatSend(0), formatSend(50), formatSend(96)], ["0 dB", "-50 dB", "-inf"]);
});

test("a mixer channel link sends level, mute and solo to every member in the same mix, but not pan; an exact slot pair sets the flag on every mixer", async () => {
  const { client, store } = setup();
  await store.start();
  const mixer = store.mixer("loopback-0", 2);
  store.links.create("mixer", [{ device_id: "loopback-0", channel: 5 }, { device_id: "loopback-0", channel: 4 }]);
  await flush();
  assert.deepEqual(
    sent(client, "set_stereo_link").map((c) => c.args),
    [0, 1, 2, 3].map((m) => ({ periph_id: 3, channel_id: (4 + m * 32) / 2, linked: 1 })),
    "Quadro periph 3, link id (strip + mixer*32)/2, on each mixer since a channel is in every mix",
  );
  assert.equal(mixer.strip(4).value.linked && mixer.strip(5).value.linked, true);

  client.invocations.length = 0;
  mixer.setLevel(5, 30);
  mixer.setPan(5, 10);
  await flush();
  assert.deepEqual(sent(client, "set_mixer").map((c) => [c.args?.["mixer_id"], c.args?.["channel"], c.args?.["level"], c.args?.["pan"]]), [
    [2, 6, 30, PAN_CENTRE],
    [2, 5, 30, PAN_CENTRE],
    [2, 6, 30, 10],
  ]);
  assert.equal(mixer.strip(4).value.pan, PAN_CENTRE, "the other member keeps its pan");

  const studio = store.mixer("loopback-1", 2);
  studio.setLevel(1, 6);
  store.links.create("mixer", [{ device_id: "loopback-0", channel: 0 }, { device_id: "loopback-1", channel: 1 }], "relative");
  await flush();
  assert.deepEqual(sent(client, "set_stereo_link").length, 0, "a link across devices sets no device flag");
  client.invocations.length = 0;
  mixer.setLevel(0, 4);
  mixer.toggleMute(0);
  store.mixer("loopback-0", 1).setLevel(0, 2);
  await flush();
  assert.deepEqual(sent(client, "set_mixer_cfg").map((c) => [c.args?.["mixer_id"], c.args?.["channel"], c.args?.["level"], c.args?.["mute"]]), [
    [2, 2, 10, 0],
    [2, 2, 10, 1],
    [1, 2, 2, 0],
  ], "relative: the Studio+ strip moves by the same step, in the same mix");
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

test("loading reads the mixer's strips (master first) and its links from the device, and marks the state known", async () => {
  const { client, store } = setup();
  const strips = Array.from({ length: 33 }, (_, i) => ({ level: i === 0 ? 4 : i === 4 ? 20 : 0, pan: i === 4 ? 50 : PAN_CENTRE, mute: i === 4 ? 1 : 0, solo: 0 }));
  // Mixer 1's pairs are entries 16..31; entry 17 is its pair 1, strips 2 and 3.
  const links = Array.from({ length: 64 }, (_, i) => ({ linked: i === 17 ? 1 : 0 }));
  const reply = (call: { deviceId: string; command: string }, response: unknown, dryRun = false) => ({ device_id: call.deviceId, command: call.command, sent_hex: "74", sent_len: 16, dry_run: dryRun, response, response_error: null });
  client.respond = async (call) => reply(call, call.command === "get_mixer" ? { entries: strips } : call.command === "get_mixer_links" ? { entries: links } : null);

  const mixer = store.mixer("loopback-0", 1);
  assert.equal(mixer.stateKnown.value, false);
  assert.equal(await mixer.load(), true);
  assert.deepEqual(sent(client, "get_mixer").map((c) => c.options?.["ext3"]), [1], "ext3 names the mixer");
  assert.deepEqual(mixer.strip("master").value, { level: 4, pan: PAN_CENTRE, mute: false, solo: false, send: 0, linked: false });
  assert.deepEqual(mixer.strip(3).value, { level: 20, pan: 50, mute: true, solo: false, send: 0, linked: true }, "entry 4 is strip 3");
  assert.deepEqual([mixer.strip(2).value.linked, mixer.strip(4).value.linked], [true, false]);
  assert.equal(mixer.stateKnown.value, true);
  assert.equal(sent(client, "set_mixer").length, 0, "loading sends nothing back");

  client.respond = async (call) => reply(call, call.command === "get_mixer" ? { entries: strips.map((s) => ({ ...s, send: 77 })) } : { entries: links });
  const studio = store.mixer("loopback-1", 0);
  await studio.load();
  assert.equal(studio.strip(3).value.send, 77, "Studio+ entries carry send");

  const dry = store.mixer("loopback-0", 2);
  client.respond = async (call) => reply(call, null, true);
  assert.equal(await dry.load(), false, "a dry run reads nothing");
  assert.equal(dry.stateKnown.value, false);
  assert.equal(dry.strip(3).value.level, 0);
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

test("a mix in mono centres its channels' pans and restores them after; pans moved meanwhile are kept, not sent", async () => {
  const { client, store } = setup();
  await store.start();
  const channels = store.channels("loopback-0");
  channels.add();
  channels.add(); // slots 6 and 7
  const mixer = store.mixer("loopback-0", 1);
  mixer.setPan(6, 10);
  mixer.setPan(7, 50);
  await flush();
  client.invocations.length = 0;
  const pans = () => sent(client, "set_mixer").map((c) => [c.args?.["mixer_id"], c.args?.["channel"], c.args?.["pan"]]);

  assert.equal(channels.setMono(1, true), true);
  await flush();
  assert.deepEqual(pans(), [[1, 7, PAN_CENTRE], [1, 8, PAN_CENTRE]], "every channel in mix 2 pans to centre");
  assert.deepEqual([channels.isMono(1), channels.isMono(0)], [true, false]);
  assert.deepEqual(store.workspace.value?.mixers["loopback-0"]?.mixes[1]?.mono, { pans: { "6": 10, "7": 50 } }, "the pans to restore are saved in the workspace");

  client.invocations.length = 0;
  mixer.setPan(6, 20);
  await flush();
  assert.deepEqual(pans(), [], "a pan moved while mono is saved for later, not sent");
  assert.equal(mixer.monoPan(6), 20);
  assert.equal(store.mixer("loopback-0", 0).monoPan(6), undefined, "other mixes are not mono");

  assert.equal(channels.setMono(1, false), true);
  await flush();
  assert.deepEqual(pans(), [[1, 7, 20], [1, 8, 50]], "mono off restores the saved pans, including the one moved meanwhile");
  assert.equal(channels.isMono(1), false);
  assert.equal(mixer.monoPan(6), undefined);
});
