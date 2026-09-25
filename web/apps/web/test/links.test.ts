import { test } from "node:test";
import assert from "node:assert/strict";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { Store } from "../src/store/store.ts";
import { builtInThemes, device, FakeClient, flush, MemoryStorage, type Invocation } from "./fake-client.ts";

const Q = "loopback-0";
const S = "loopback-1";
const ref = (device_id: string, channel: number) => ({ device_id, channel });

/** A started store over a Quadro and a Studio+; `respond` fills live replies (null otherwise). */
async function setup(respond?: (call: Invocation) => unknown) {
  const client = new FakeClient(device(Q, "quadro", "Zen Quadro"), device(S, "studio", "Zen Studio+"));
  if (respond !== undefined) {
    client.respond = async (call) => ({ device_id: call.deviceId, command: call.command, sent_hex: "70", sent_len: 16, dry_run: false, response: respond(call) ?? null, response_error: null });
  }
  const timers = new ManualTimers();
  const store = new Store(client, { timers, storage: new MemoryStorage(), themeSources: builtInThemes });
  await store.start();
  const sent = (deviceId: string, command: string) => client.invocations.filter((c) => c.deviceId === deviceId && c.command === command).map((c) => c.args);
  return { client, store, timers, sent };
}

test("a link sends a change to every member, across devices; absolute members take the same value", async () => {
  const { store, sent } = await setup();
  assert.ok(store.links.create("preamp", [ref(Q, 0), ref(S, 3)]));
  store.inputs(Q).setGain(0, 30);
  store.inputs(S).setPhaseInvert(3, true);
  await flush();
  assert.deepEqual(sent(Q, "set_pre_gain"), [{ id: 0, gain: 30 }]);
  assert.deepEqual(sent(S, "set_pre_gain"), [{ id: 3, gain: 30 }]);
  assert.deepEqual([sent(S, "set_pre_phaseinv"), sent(Q, "set_pre_phase_inv")], [[{ id: 3, phase_inv: 1 }], [{ id: 0, phase_inv: 1 }]]);
  assert.equal(store.links.linkOf("preamp", Q, 0)?.mode, "absolute");
  assert.equal(store.links.linkOf("preamp", Q, 1), undefined);
});

test("relative members keep their offsets, each within its own range", async () => {
  const { store, sent } = await setup();
  const quadro = store.inputs(Q);
  quadro.setGain(1, 10);
  const id = store.links.create("preamp", [ref(Q, 0), ref(Q, 1)], "relative") as string;
  quadro.setGain(0, 5);
  quadro.setGain(0, 70);
  await flush();
  assert.deepEqual(sent(Q, "set_pre_gain").slice(1), [{ id: 0, gain: 5 }, { id: 1, gain: 15 }, { id: 0, gain: 70 }, { id: 1, gain: 75 }], "a Quadro's Mic gain tops out at 75 dB");

  store.links.setMode(id, "absolute");
  quadro.setGain(1, 20);
  await flush();
  assert.deepEqual(sent(Q, "set_pre_gain").slice(-2), [{ id: 1, gain: 20 }, { id: 0, gain: 20 }]);
});

test("a type goes to every member or none; 48V skips members not on Mic", async () => {
  const { store, sent } = await setup();
  const quadro = store.inputs(Q);
  store.links.create("preamp", [ref(Q, 0), ref(Q, 2)]);
  assert.throws(() => quadro.setType(0, 2), /Hi-Z/, "preamp 3 has no Hi-Z, so neither changes");
  await flush();
  assert.deepEqual(sent(Q, "set_pre_type"), []);
  quadro.setType(2, 1);
  await flush();
  assert.deepEqual(sent(Q, "set_pre_type"), [{ id: 2, pretype: 1 }, { id: 0, pretype: 1 }]);

  quadro.setType(3, 1);
  store.links.create("preamp", [ref(Q, 1), ref(Q, 3)]);
  assert.equal(quadro.setPhantom(1, true), true);
  await flush();
  assert.deepEqual(sent(Q, "set_pre_phantom"), [{ id: 1, phantom: 1 }], "preamp 4 is on Line, so it gets no 48V");
});

test("digital input gains link the same way, and members must exist on their device", async () => {
  const { store, sent } = await setup();
  store.inputs(S).setDigitalGain("line", 0, -2);
  store.links.create("line", [ref(S, 0), ref(S, 5)], "relative");
  store.inputs(S).setDigitalGain("line", 5, 4);
  await flush();
  assert.deepEqual(sent(S, "set_line_gain"), [{ id: 0, gain: -2 }, { id: 5, gain: 4 }, { id: 0, gain: 2 }]);
  assert.throws(() => store.links.create("preamp", [ref(S, 0), ref("usb:9", 0)]), RangeError, "a device that is not connected");
  assert.throws(() => store.links.create("line", [ref(S, 0), ref(Q, 0)]), RangeError, "the Quadro has no line inputs");
  assert.throws(() => store.links.create("preamp", [ref(S, 0)]), RangeError, "a link needs two channels");
});

test("a link of exactly one device pair also sets that device's link flag; other links do not, and removing it clears the flag", async () => {
  const { store, sent } = await setup();
  const pair = store.links.create("preamp", [ref(S, 3), ref(S, 2)]) as string;
  store.links.create("preamp", [ref(S, 4), ref(S, 7)]);
  store.links.create("adat", [ref(S, 14), ref(S, 15)]);
  await flush();
  assert.deepEqual(sent(S, "set_stereo_link"), [{ periph_id: 0, channel_id: 1, linked: 1 }, { periph_id: 2, channel_id: 7, linked: 1 }]);
  store.links.remove(pair);
  await flush();
  assert.deepEqual(sent(S, "set_stereo_link").at(-1), { periph_id: 0, channel_id: 1, linked: 0 });
  assert.equal(store.inputs(S).pairLinked(1).value, false);
});

test("a channel is in one link per kind: linking it again moves it, and a link left with one channel goes", async () => {
  const { store } = await setup();
  const first = store.links.create("preamp", [ref(Q, 0), ref(Q, 1)]) as string;
  const second = store.links.create("preamp", [ref(Q, 1), ref(Q, 2)]) as string;
  assert.equal(store.links.links.value.some((l) => l.id === first), false);
  assert.equal(store.links.linkOf("preamp", Q, 1)?.id, second);
  assert.equal(store.links.linkOf("preamp", Q, 0), undefined);
  store.links.removeMember("preamp", Q, 2);
  assert.deepEqual(store.links.links.value, []);
});

test("mixer channels link like inputs: members must be slots on a known device, and pairs a loaded mixer reports linked are imported", async () => {
  const pairs = Array.from({ length: 64 }, (_, i) => ({ linked: i === 17 ? 1 : 0 }));
  const { store } = await setup((call) => (call.command === "get_mixer" ? { entries: Array.from({ length: 33 }, () => ({ level: 0, pan: 32, mute: 0, solo: 0 })) } : call.command === "get_mixer_links" ? { entries: pairs } : null));
  assert.throws(() => store.links.create("mixer", [ref(Q, 0), ref(Q, 32)]), RangeError, "32 slots");
  await store.mixer(Q, 1).load();
  await store.links.importDevicePairs(Q);
  assert.deepEqual(store.links.links.value.map((l) => [l.kind, l.members]), [["mixer", [ref(Q, 2), ref(Q, 3)]]], "mixer 1's pair 1 is slots 2 and 3");
});

test("pairs the device reports linked become absolute links once, saved in the workspace", async () => {
  const { client, store, timers } = await setup((call) => (call.command === "get_preamps_links" ? { entries: [0, 1, 0, 0, 0, 0].map((linked) => ({ linked })) } : null));
  await store.links.importDevicePairs(S);
  await store.links.importDevicePairs(S);
  assert.deepEqual(store.links.links.value.map((l) => [l.kind, l.mode, l.members]), [["preamp", "absolute", [ref(S, 2), ref(S, 3)]]]);
  timers.advance(1000);
  await flush();
  assert.equal(client.stored.links.length, 1);

  store.links.setMode(store.links.links.value[0]?.id as string, "relative");
  await store.links.importDevicePairs(S);
  assert.equal(store.links.links.value[0]?.mode, "relative", "a pair already in a link is left as the user made it");
});
