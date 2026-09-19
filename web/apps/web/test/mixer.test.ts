import { test } from "node:test";
import assert from "node:assert/strict";

import { GazelleError } from "gazelle-audio-client";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { effect } from "../src/core/signal.ts";
import { clampPan, faderPosition, panAtPosition, formatLevel, formatPan, formatSend, LEVEL_MAX, levelAtFaderPosition, levelFromDb, meterDeflection, PAN_CENTRE } from "../src/store/mixer.ts";
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
  assert.deepEqual([clampPan(0), clampPan(26), clampPan(27), clampPan(33), clampPan(38), clampPan(63)], [2, 26, 27, 33, 38, 62], "2..62, every step of it: the wheel and keys move one step (the user, 2026-09-17)");
  // Dragging keeps the panel's centre detent: a pointer anywhere in 27..38 lands on centre.
  const at = (pan: number) => (pan - 2) / 60;
  assert.deepEqual([panAtPosition(0), panAtPosition(at(26)), panAtPosition(at(27)), panAtPosition(0.5), panAtPosition(at(38)), panAtPosition(at(39)), panAtPosition(1), panAtPosition(1.2)], [2, 26, 32, 32, 32, 39, 62, 62]);
  // Pan reads as a side and a share of the way there (the user, 2026-09-17): the byte's ±30 steps are ±100%.
  assert.deepEqual([formatPan(PAN_CENTRE), formatPan(40), formatPan(2), formatPan(62), formatPan(31), formatPan(3)], ["C", "R 27%", "L 100%", "R 100%", "L 3%", "L 97%"]);
  assert.deepEqual([96, 60, 50, 40, 30, 20, 10, 0].map(meterDeflection), [0, 0, 5, 15, 30, 50, 75, 100], "Antelope's piecewise meter anchors");
});

test("the fader has an audio taper on the meters' scale: each 10 dB down takes less travel, and position and level convert both ways (the user, 2026-09-16)", () => {
  // Positions are measured from the top of the fader, as a pointer finds them.
  assert.equal(faderPosition(0), 0, "0 dB at the top");
  assert.equal(faderPosition(LEVEL_MAX), 1, "-90 dB at the bottom");
  // Down to -60 dB it follows Antelope's meter scale, in the top nine tenths; -60 to -90 is the last tenth.
  const expected: [number, number][] = [[10, 0.225], [20, 0.45], [30, 0.63], [40, 0.765], [50, 0.855], [60, 0.9]];
  for (const [level, at] of expected) assert.ok(Math.abs(faderPosition(level) - at) < 1e-9, `-${level} dB at ${at} (${faderPosition(level)})`);
  const travel = [10, 20, 30, 40, 50, 60].map((level) => faderPosition(level) - faderPosition(level - 10));
  travel.reduce((above, step) => (assert.ok(step <= above + 1e-9, `each 10 dB down takes no more travel than the one above (${travel})`), step));
  assert.ok(faderPosition(60) - faderPosition(50) < faderPosition(10) / 4, "the 10 dB above -60 takes a small part of what the top 10 dB does");
  assert.ok(Math.abs(levelAtFaderPosition(0.5) - 22.78) < 0.01, `halfway down is about -23 dB (${levelAtFaderPosition(0.5)})`);
  let previous = -1;
  for (let level = 0; level <= LEVEL_MAX; level++) {
    const at = faderPosition(level);
    assert.ok(at > previous, "further down is always quieter");
    previous = at;
    assert.ok(Math.abs(levelAtFaderPosition(at) - level) < 1e-9, "position and level convert back exactly");
  }
  assert.equal(levelAtFaderPosition(-0.2), 0, "past the top is 0 dB");
  assert.equal(levelAtFaderPosition(1.3), LEVEL_MAX, "past the bottom is the floor");
});

test("Quadro strips send set_mixer with the whole strip, coalesced per strip, master on channel 0", async () => {
  const { client, store } = setup();
  const mixer = store.mixer("loopback-0", 1);
  assert.equal(mixer.hasSend, false);
  assert.equal(store.mixer("loopback-0", 0).hasReverbSend, false, "the Quadro's reverb sends are the Effects page's");
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
  // The send byte is the reverb send, which the vendor panel shows on Mix 1 only (the user's choice).
  assert.deepEqual([0, 1, 2, 3].map((mix) => store.mixer("loopback-1", mix).hasReverbSend), [true, false, false, false]);
  mixer.setSend(0, 300);
  mixer.toggleSolo(0);
  await flush();
  assert.deepEqual(sent(client, "set_mixer_cfg").map((c) => c.args), [
    { mixer_id: 3, channel: 1, level: 0, pan: PAN_CENTRE, mute: 0, solo: 0, send: 95 },
    { mixer_id: 3, channel: 1, level: 0, pan: PAN_CENTRE, mute: 0, solo: 1, send: 95 },
  ], "send is dB of attenuation, 0 (loudest) to 95 (-inf), as captured in hardware sessions 1 and 2");
  assert.deepEqual([formatSend(0), formatSend(50), formatSend(94), formatSend(95)], ["0 dB", "-50 dB", "-94 dB", "-inf"]);
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

test("activating a mixer follows the device's report and points no meter bank: strips meter their inputs (hardware, 2026-09-16)", async () => {
  const { client, store } = setup();
  const stopStudio = store.mixer("loopback-1", 3).activate();
  const stopQuadro = store.mixer("loopback-0", 0).activate();
  await flush();
  // The Quadro ignored every set_peak_source tried on it and kept metering Mix 1's inputs.
  assert.deepEqual(sent(client, "set_peak_source"), []);
  assert.equal(client.cyclic.size, 2, "each active mixer follows its device's report");
  stopStudio();
  stopQuadro();
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

test("a mix is read once and then reused, and read again after the connection drops or the device goes", async () => {
  const { client, store } = setup();
  const strips = Array.from({ length: 33 }, (_, i) => ({ level: i === 4 ? 20 : 0, pan: PAN_CENTRE, mute: 0, solo: 0 }));
  const reply = (call: { deviceId: string; command: string }, response: unknown, dryRun = false) => ({ device_id: call.deviceId, command: call.command, sent_hex: "74", sent_len: 16, dry_run: dryRun, response, response_error: null });
  client.respond = async (call) => reply(call, call.command === "get_mixer" ? { entries: strips } : call.command === "get_mixer_links" ? { entries: [] } : null);
  const reads = () => sent(client, "get_mixer").length;

  const mixer = store.mixer("loopback-0", 0);
  assert.equal(mixer.needsRead.value, true);
  assert.deepEqual(await Promise.all([mixer.readOnce(), mixer.readOnce()]), [true, false], "one read while one is in flight");
  assert.equal(reads(), 1);
  assert.equal(mixer.needsRead.value, false);
  assert.equal(await mixer.readOnce(), false, "read already");
  assert.equal(reads(), 1);

  // What the app sends is what the cache holds, so it stays current without reading.
  mixer.setLevel(3, 12);
  assert.equal(await mixer.readOnce(), false);
  assert.equal(mixer.strip(3).value.level, 12);

  // The connection dropping: the server may have restarted or the device changed meanwhile.
  client.status = "reconnecting";
  client.emit("status", "reconnecting");
  assert.equal(mixer.needsRead.value, true);
  client.status = "open";
  client.emit("status", "open");
  assert.equal(await mixer.readOnce(), true);
  assert.equal(reads(), 2);
  assert.equal(mixer.strip(3).value.level, 20, "the device's state again");

  // A device going (unplugged, or re-attached): its mixes only.
  const other = store.mixer("loopback-1", 0);
  await other.readOnce();
  const removed = client.devices.get("loopback-0");
  client.devices.delete("loopback-0");
  client.emit("device_removed", "loopback-0");
  assert.equal(mixer.needsRead.value, true);
  assert.equal(other.needsRead.value, false);
  if (removed !== undefined) client.devices.set("loopback-0", removed);
  client.emit("device_added", removed as never);
  assert.equal(await mixer.readOnce(), true);

  // A read that fails is tried again next time; a dry run, which has nothing to read, is not.
  const failing = store.mixer("loopback-0", 1);
  client.respond = async () => {
    throw new GazelleError("timeout", "no reply");
  };
  assert.equal(await failing.readOnce(), true);
  assert.equal(failing.needsRead.value, true);
  const dry = store.mixer("loopback-0", 2);
  client.respond = async (call) => reply(call, null, true);
  assert.equal(await dry.readOnce(), true);
  assert.equal(dry.needsRead.value, false);
  assert.equal(dry.stateKnown.value, false);

  // A read still in flight when the device goes does not count, nor hold back the next one.
  const held: (() => void)[] = [];
  client.respond = (call) => (call.command === "get_mixer" ? new Promise((resolve) => held.push(() => resolve(reply(call, { entries: strips })))) : Promise.resolve(reply(call, { entries: [] })));
  const late = store.mixer("loopback-0", 3);
  const pending = late.readOnce();
  client.emit("device_removed", "loopback-0");
  client.emit("device_added", removed as never);
  const again = late.readOnce();
  assert.equal(sent(client, "get_mixer").filter((c) => c.options?.["ext3"] === 3).length, 2, "the next read is not held back");
  held[0]?.();
  assert.equal(await pending, true);
  assert.equal(late.needsRead.value, true, "a read from before the device went does not count");
  held[1]?.();
  assert.equal(await again, true);
  assert.equal(late.needsRead.value, false);
});

test("a device's mixes want reading only while connected and attached; they are read together, then its linked pairs once", async () => {
  const { client, store } = setup();
  assert.equal(store.mixesToRead("loopback-1"), true);
  assert.equal(store.mixesToRead("usb:1"), false, "an unknown model has no mixes");

  await Promise.all([store.readMixes("loopback-1"), store.readMixes("loopback-1")]);
  assert.deepEqual(sent(client, "get_mixer").map((c) => [c.deviceId, c.options?.["ext3"]]), [0, 1, 2, 3].map((mix) => ["loopback-1", mix]), "each mix once");
  assert.equal(sent(client, "get_preamps_links").length, 1, "the device's own link flags after the read, once");
  assert.equal(store.mixesToRead("loopback-1"), false);
  await store.readMixes("loopback-1");
  assert.equal(sent(client, "get_preamps_links").length, 1, "nothing read, so nothing to import");

  // Followed as a page does, in an effect.
  const seen: boolean[] = [];
  const stop = effect(() => void seen.push(store.mixesToRead("loopback-1")));
  client.status = "reconnecting";
  client.emit("status", "reconnecting");
  assert.equal(seen.at(-1), false, "not while the connection is down");
  client.status = "open";
  client.emit("status", "open");
  assert.equal(seen.at(-1), true, "but once it is back");
  await store.readMixes("loopback-1");
  const studio = client.devices.get("loopback-1");
  client.devices.delete("loopback-1");
  client.emit("device_removed", "loopback-1");
  assert.equal(seen.at(-1), false, "not while the device is gone");
  if (studio !== undefined) client.devices.set("loopback-1", studio);
  client.emit("device_added", studio as never);
  assert.equal(seen.at(-1), true, "but once it is back");
  stop();
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
