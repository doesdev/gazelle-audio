import { test } from "node:test";
import assert from "node:assert/strict";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { EFFECT_NAMES } from "../src/store/effect-catalogue.ts";
import { loadCatalogue } from "../src/store/effect-parameters.ts";
import { formatReverbLevel, formatRoomSize, REVERB_RETURN_MAX, REVERB_SEND_MAX } from "../src/store/effects.ts";
import { Store } from "../src/store/store.ts";
import { builtInThemes, device, FakeClient, flush, MemoryStorage, type Invocation } from "./fake-client.ts";

// Reading an effect's settings needs the catalogue, which the Effects page fetches as it opens
// (store/effect-parameters.ts); these tests stand in for the page, so they fetch it first.
await loadCatalogue();

type Replies = Record<string, (call: Invocation) => Record<string, unknown> | null>;

function setup(replies: Replies = {}, dryRun = false) {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"), device("loopback-1", "studio", "Zen Studio+"), device("usb:1", null, null));
  client.respond = async (call) => {
    const reply = replies[call.command];
    const response = dryRun || reply === undefined ? null : reply(call);
    if (!dryRun && call.command.startsWith("get_") && response === null) throw new Error(`${call.command} refused`);
    return { device_id: call.deviceId, command: call.command, sent_hex: "74", sent_len: 1, dry_run: dryRun, response, response_error: null };
  };
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), requestFrame: (callback) => callback(), themeSources: builtInThemes });
  const sent = (deviceId: string, command: string) => client.invocations.filter((c) => c.deviceId === deviceId && c.command === command);
  return { client, store, sent };
}

const slots = (...effects: [type: number, inst: number][]) => Array.from({ length: 8 }, (_, i) => ({ type: effects[i]?.[0] ?? 0, inst: effects[i]?.[1] ?? 0 }));

const QUADRO_CHAINS: Record<number, [number, number][]> = {
  0: [[39, 2], [1, 0]],
  1: [[39, 3], [1, 1]],
  4: [[9, 0]],
};

const quadroReplies = (): Replies => ({
  get_afx_strip_order: (call) => ({ entries: [{ slots: slots(...(QUADRO_CHAINS[Number(call.options?.["ext3"])] ?? [])) }] }),
  get_afx_links: () => ({ entries: [1, 0, 0, 0, 0, 0, 0].map((linked) => ({ linked })) }),
  get_reverb_config: () => ({ mixer_id: 0, room_size: 40, color: 10, predelay: 20, density: 100, early_ref_gain: 30, late_ref_delay: 50, richness: 60, reverb_time: 70, reverb_level: 25, on: 1 }),
  get_reverb_returns: () => ({ entries: [{ level: 12, mute: 0 }, { level: 90, mute: 1 }, { level: 0, mute: 0 }, { level: 0, mute: 0 }] }),
  get_reverb_sends: () => ({ entries: Array.from({ length: 33 }, (_, i) => ({ level: i === 0 ? 7 : 96 - i, pan: i === 3 ? 20 : 32, mute: 0, solo: 0 })) }),
});

test("the Quadro's six chains are read one at a time with get_afx_strip_order, the chain in ext3, and named from the catalogue", async () => {
  const { store, sent } = setup(quadroReplies());
  const effects = store.effects("loopback-0");
  assert.ok(effects.chains.value === undefined, "nothing is known before a read");
  assert.equal(await effects.readOnce(), true);

  assert.deepEqual(sent("loopback-0", "get_afx_strip_order").map((c) => c.options?.["ext3"]).sort(), [0, 1, 2, 3, 4, 5], "AFX IN 1-6 only, not the AFX2DAW chains");
  assert.equal(sent("loopback-0", "get_afx_order").length, 0, "the Quadro panel never reads the whole table");
  const chains = effects.chains.peek();
  assert.ok(chains);
  assert.deepEqual(chains.map((c) => c.name), ["AFX IN 1", "AFX IN 2", "AFX IN 3", "AFX IN 4", "AFX IN 5", "AFX IN 6"]);
  assert.deepEqual(chains[0]?.slots, [
    { position: 0, type: 39, inst: 2, name: EFFECT_NAMES.quadro.get(39) },
    { position: 1, type: 1, inst: 0, name: EFFECT_NAMES.quadro.get(1) },
  ]);
  assert.deepEqual(chains[4]?.slots.map((s) => s.name), ["FET-A76"]);
  assert.deepEqual(chains[2]?.slots, [], "type 0 is an empty slot");
  assert.deepEqual(chains.map((c) => [c.linked, c.partner]), [[true, 1], [true, 0], [false, 3], [false, 2], [false, 5], [false, 4]], "link byte k pairs chains 2k and 2k+1");
  assert.equal(chains[0]?.destination, 7, "the chain is fed by the AFX IN destination group");
});

test("the Studio+'s sixteen chains come from one get_afx_order, and an unknown type still shows", async () => {
  const { store, sent } = setup({
    get_afx_order: () => ({ entries: Array.from({ length: 16 }, (_, i) => ({ slots: i === 15 ? slots([2, 5], [77, 1]) : slots() })) }),
    get_afx_links: () => ({ entries: Array.from({ length: 8 }, (_, k) => ({ linked: k === 7 ? 1 : 0 })) }),
    get_reverb_config: () => ({ mixer_id: 0, room_size: 0, color: 0, predelay: 0, density: 100, early_ref_gain: 0, late_ref_delay: 0, richness: 0, reverb_time: 0, reverb_level: 25, on: 0 }),
  });
  const effects = store.effects("loopback-1");
  await effects.readOnce();
  assert.equal(sent("loopback-1", "get_afx_strip_order").length, 0);
  assert.equal(sent("loopback-1", "get_reverb_returns").length + sent("loopback-1", "get_reverb_sends").length, 0, "the Studio+ has no reverb send or return commands");
  const chains = effects.chains.value ?? [];
  assert.equal(chains.length, 16);
  assert.deepEqual(chains[15]?.slots.map((s) => s.name), ["Compressor", "Effect 77"]);
  assert.deepEqual([chains[14]?.linked, chains[15]?.linked, chains[15]?.partner], [true, true, 14]);
  assert.equal(chains[15]?.destination, 9);
  assert.equal(effects.returns.value, undefined);
  assert.equal(effects.sends.value, undefined);
  assert.throws(() => store.effects("usb:1"), /no known model/);
  assert.equal(store.effects("loopback-1"), effects, "one model per device");
});

test("bypass goes per instance with enabled 1 = processing, in each family's field names, to the linked partner's slot too", async () => {
  const { store, sent } = setup(quadroReplies());
  const effects = store.effects("loopback-0");
  await effects.readOnce();
  assert.equal(effects.bypass(39, 2).value, undefined, "bypass is not readable, so it is unknown until set");

  effects.setBypass(0, 0, true);
  await flush();
  assert.deepEqual(sent("loopback-0", "set_afx_bypass").map((c) => c.args), [
    { periph_id: 2, periph_type: 39, enabled: 0 },
    { periph_id: 3, periph_type: 39, enabled: 0 },
  ]);
  assert.equal(effects.bypass(39, 2).value, true);
  assert.equal(effects.bypass(39, 3).value, true, "the partner's instance follows");

  effects.setBypass(4, 0, false);
  await flush();
  assert.deepEqual(sent("loopback-0", "set_afx_bypass").at(-1)?.args, { periph_id: 0, periph_type: 9, enabled: 1 });
  assert.equal(sent("loopback-0", "set_afx_bypass").length, 3, "an unlinked chain sends one");
  assert.throws(() => effects.setBypass(2, 0, true), RangeError, "an empty slot has nothing to bypass");
  assert.throws(() => effects.setBypass(6, 0, true), RangeError);

  const studio = setup({
    get_afx_order: () => ({ entries: Array.from({ length: 16 }, (_, i) => ({ slots: i === 3 ? slots([39, 7]) : slots() })) }),
    get_afx_links: () => ({ entries: Array.from({ length: 8 }, () => ({ linked: 0 })) }),
    get_reverb_config: () => null,
  });
  const other = studio.store.effects("loopback-1");
  await other.readOnce();
  other.setChainBypass(3, true);
  await flush();
  assert.deepEqual(studio.sent("loopback-1", "set_afx_bypass").map((c) => c.args), [{ inst_id: 7, type_id: 39, enabled: 0 }]);
});

test("a bypass reaches the partner only when the chains are linked and the partner's slot holds the same effect", async () => {
  const chains: Record<number, [number, number][]> = { 0: [[39, 0], [2, 0]], 1: [[39, 1], [1, 4]], 2: [[9, 0]], 3: [[9, 1]] };
  const { store, sent } = setup({ ...quadroReplies(), get_afx_strip_order: (call) => ({ entries: [{ slots: slots(...(chains[Number(call.options?.["ext3"])] ?? [])) }] }) });
  const effects = store.effects("loopback-0");
  await effects.readOnce();
  effects.setBypass(0, 1, true);
  effects.setBypass(2, 0, true);
  await flush();
  assert.deepEqual(sent("loopback-0", "set_afx_bypass").map((c) => c.args), [
    { periph_id: 0, periph_type: 2, enabled: 0 },
    { periph_id: 0, periph_type: 9, enabled: 0 },
  ], "a different effect in the linked partner's slot, and an unlinked chain's neighbour, are left alone");
  assert.equal(effects.bypass(9, 1).value, undefined);
});

test("a read under way is not started again", async () => {
  const { store, sent } = setup(quadroReplies());
  const effects = store.effects("loopback-0");
  const [first, second] = await Promise.all([effects.readOnce(), effects.readOnce()]);
  assert.deepEqual([first, second], [true, false]);
  assert.equal(sent("loopback-0", "get_afx_links").length, 1);
});

test("bypass all sends one bypass per effect; a change that is not sent restores what was known", async () => {
  const { store, sent, client } = setup(quadroReplies());
  const effects = store.effects("loopback-0");
  await effects.readOnce();
  effects.setChainBypass(4, true);
  effects.setChainBypass(1, false);
  await flush();
  assert.deepEqual(sent("loopback-0", "set_afx_bypass").map((c) => c.args), [
    { periph_id: 0, periph_type: 9, enabled: 0 },
    { periph_id: 3, periph_type: 39, enabled: 1 },
    { periph_id: 2, periph_type: 39, enabled: 1 },
    { periph_id: 1, periph_type: 1, enabled: 1 },
    { periph_id: 0, periph_type: 1, enabled: 1 },
  ]);

  client.respond = async () => {
    throw new Error("gone");
  };
  effects.setBypass(4, 0, false);
  assert.equal(effects.bypass(9, 0).value, false, "shown at once");
  await flush();
  assert.equal(effects.bypass(9, 0).value, true, "and put back when it was not sent");
});

test("the reverb config is read, and on/off and level resend every field with density 100; nothing is sent before a read", async () => {
  const { store, sent } = setup({ ...quadroReplies(), get_reverb_config: () => ({ mixer_id: 0, room_size: 40, color: 10, predelay: 20, density: 55, early_ref_gain: 30, late_ref_delay: 50, richness: 60, reverb_time: 70, reverb_level: 25, on: 1 }) });
  const effects = store.effects("loopback-0");
  assert.equal(effects.setReverbOn(false), false, "unread: refused, so no default overwrites the device's reverb");
  await flush();
  assert.equal(sent("loopback-0", "set_reverb_config").length, 0);

  await effects.readOnce();
  assert.deepEqual(effects.reverb.value, { known: true, roomSize: 40, color: 10, predelay: 20, density: 55, earlyRefGain: 30, lateRefDelay: 50, richness: 60, reverbTime: 70, level: 25, on: true });
  assert.equal(effects.setReverbOn(false), true);
  assert.equal(effects.setReverbLevel(250), true);
  await flush();
  const expected = { mixer_id: 0, room_size: 40, color: 10, predelay: 20, density: 100, early_ref_gain: 30, late_ref_delay: 50, richness: 60, reverb_time: 70 };
  assert.deepEqual(sent("loopback-0", "set_reverb_config").map((c) => c.args), [
    { ...expected, reverb_level: 25, on: 0 },
    { ...expected, reverb_level: 100, on: 0 },
  ]);
  effects.setReverbLevel(-5);
  await flush();
  assert.equal(sent("loopback-0", "set_reverb_config").at(-1)?.args?.["reverb_level"], 1, "the level runs 1..100");
  assert.equal(effects.reverb.value?.level, 1);
});

test("reverb level and room size show as the panel shows them", () => {
  assert.deepEqual([formatReverbLevel(25), formatReverbLevel(100), formatReverbLevel(1), formatReverbLevel(50)], ["0 dB", "+12 dB", "-28 dB", "+6 dB"]);
  assert.deepEqual([formatRoomSize(0), formatRoomSize(90)], ["4 / 2 / 2", "42 / 23 / 16"]);
});

test("Quadro returns (mixes 1-2) and sends (mix 1 channels 1-16) are read and set with the panel's fields", async () => {
  const { store, sent } = setup(quadroReplies());
  const effects = store.effects("loopback-0");
  assert.equal(effects.setReturn(0, { level: 5 }), false, "unread: refused");
  await effects.readOnce();
  assert.deepEqual(effects.returns.value, { known: true, entries: [{ level: 12, mute: false }, { level: 90, mute: true }] }, "only the two the panel drives");
  const sends = effects.sends.value;
  assert.equal(sends?.entries.length, 16);
  assert.deepEqual(sends?.entries[0], { level: 95, pan: 32 }, "entry 0 is skipped: channel 1 is entry 1");
  assert.deepEqual(sends?.entries[2], { level: 93, pan: 20 });

  effects.setReturn(1, { mute: false });
  effects.setReturn(0, { level: 200 });
  effects.setSend(3, { pan: 29 });
  effects.setSend(16, { level: 500 });
  await flush();
  assert.deepEqual(sent("loopback-0", "set_reverb_return").map((c) => c.args), [
    { mixer_id: 1, level: 90, mute: 0 },
    { mixer_id: 0, level: REVERB_RETURN_MAX, mute: 0 },
  ]);
  assert.deepEqual(sent("loopback-0", "set_reverb_send").map((c) => c.args), [
    { mixer_id: 0, channel: 3, level: 93, pan: 29, mute: 0, solo: 0 },
    { mixer_id: 0, channel: 16, level: REVERB_SEND_MAX, pan: 32, mute: 0, solo: 0 },
  ], "pan is held to its range but not snapped (only a drag snaps); level and pan travel together");
  assert.throws(() => effects.setReturn(2, { level: 0 }), RangeError, "returns 2-3 are never driven");
  assert.throws(() => effects.setSend(0, { level: 0 }), RangeError, "channel 0 is not a send");
  assert.throws(() => store.effects("loopback-1").setSend(1, { level: 0 }), /no reverb sends/);
});

test("reads are quiet, a failed read is tried again, a dry run counts as read with the panel's starting values, and forget reads anew", async () => {
  const failing = setup({});
  const effects = failing.store.effects("loopback-0");
  await effects.readOnce();
  assert.equal(failing.store.notices.value.length, 0, "a page's own read posts no notice");
  assert.equal(effects.needsRead.value, true, "tried again next time");
  assert.equal(effects.chains.value, undefined);

  const dry = setup(quadroReplies(), true);
  const model = dry.store.effects("loopback-0");
  await model.readOnce();
  assert.equal(model.needsRead.value, false);
  assert.deepEqual(model.chains.value?.map((c) => c.known), [false, false, false, false, false, false]);
  assert.deepEqual(model.reverb.value, { known: false, roomSize: 0, color: 0, predelay: 0, density: 100, earlyRefGain: 0, lateRefDelay: 0, richness: 0, reverbTime: 0, level: 25, on: true });
  assert.deepEqual(model.returns.value?.entries, [{ level: 0, mute: false }, { level: 0, mute: false }]);
  assert.deepEqual(model.sends.value?.entries[0], { level: REVERB_SEND_MAX, pan: 32 });
  assert.equal(model.setReverbOn(false), true, "in dry run the controls work from the starting values");
  assert.equal(await model.readOnce(), false, "read once");

  const live = setup(quadroReplies());
  const kept = live.store.effects("loopback-0");
  await kept.readOnce();
  const reads = () => live.sent("loopback-0", "get_afx_links").length;
  assert.equal(reads(), 1);
  live.client.emit("status", "reconnecting");
  assert.equal(kept.needsRead.value, true, "a dropped connection forgets");
  await kept.readOnce();
  live.client.emit("device_removed", "loopback-0");
  assert.equal(kept.needsRead.value, true, "so does the device going");
  assert.equal(reads(), 2);
  assert.equal(await kept.load(), true, "load always reads");
  assert.equal(reads(), 3);
});

// Editing a chain: add, remove and reorder (a hardware probe confirmed inserting and removing one
// effect on the Quadro). The chain always goes out whole, packed from slot 1 with the rest empty.

/** The sixteen bytes `set_afx_order` carries: each slot's type and instance, trailing slots zero. */
const orderBytes = (...effects: [type: number, inst: number][]) => Uint8Array.from(Array.from({ length: 8 }, (_, i) => [effects[i]?.[0] ?? 0, effects[i]?.[1] ?? 0]).flat());

const instanceReplies = (free: Record<number, number> = { 39: 14, 1: 15, 9: 16, 3: 4, 2: 16 }, featured: Record<number, number> = {}): Replies => ({
  get_afx_available_instances: () => ({ entries: Object.entries(free).map(([type_id, inst_count]) => ({ type_id: Number(type_id), inst_count })) }),
  get_afx_remaining_featured_instances: () => ({ entries: Object.entries(featured).map(([type_id, inst_count]) => ({ type_id: Number(type_id), inst_count })) }),
});

const orders = (sent: (deviceId: string, command: string) => Invocation[]) => sent("loopback-0", "set_afx_order").map((c) => [c.args?.["ch_id"], c.args?.["slots"]]);

test("the Quadro reads its free instance counts with the chains, taking the lower of the free and the licensed count", async () => {
  const { store, sent } = setup({ ...quadroReplies(), ...instanceReplies({ 39: 14, 3: 4, 9: 0 }, { 39: 2, 3: 9, 9: 6 }) });
  const effects = store.effects("loopback-0");
  await effects.readOnce();
  assert.equal(sent("loopback-0", "get_afx_available_instances").length, 1);
  assert.equal(sent("loopback-0", "get_afx_remaining_featured_instances").length, 1);

  const offers = effects.offers(2);
  const offer = (type: number) => offers.find((o) => o.type === type);
  assert.deepEqual([offer(39)?.free, offer(39)?.counted], [2, true], "the licence allows fewer than are free");
  assert.equal(offer(3)?.free, 4, "and here the pool is smaller than the licence");
  assert.equal(offer(9)?.unavailable, "No free instance of this effect is left.");
  assert.equal(offer(2)?.counted, false, "a type the device did not count falls back to what the chains show");
  assert.ok(offers.length > 60 && offers.every((o) => o.name !== undefined));
});

test("adding an effect writes the whole chain as bytes, packed from slot 1, on the lowest free instance", async () => {
  const { store, sent } = setup({ ...quadroReplies(), ...instanceReplies() });
  const effects = store.effects("loopback-0");
  await effects.readOnce();
  const reads = () => sent("loopback-0", "get_afx_available_instances").length;
  assert.equal(reads(), 1, "read when the page opens");

  // AFX IN 5 holds FET-A76 #1 (type 9, instance 0); PowerGate (39) instances 2 and 3 are in chains 1-2.
  assert.equal(effects.addEffect(4, 39), true);
  assert.deepEqual(effects.chains.value?.[4]?.slots.map((s) => [s.position, s.type, s.inst]), [[0, 9, 0], [1, 39, 0]], "shown at once, after what was there");
  await flush();
  assert.deepEqual(orders(sent), [[4, orderBytes([9, 0], [39, 0])]]);
  assert.ok(sent("loopback-0", "set_afx_order")[0]?.args?.["slots"] instanceof Uint8Array, "bytes, which the client sends as hex: the server refuses an array of objects");
  assert.equal(effects.bypass(39, 0).value, false, "the device sets enabled on insert, so no bypass is sent");
  assert.equal(sent("loopback-0", "set_afx_bypass").length, 0);
  assert.equal(reads(), 2, "the counts are read again after the change");
});

test("adding to a linked chain gives the partner the same effect on its own instance", async () => {
  const { store, sent } = setup({ ...quadroReplies(), ...instanceReplies() });
  const effects = store.effects("loopback-0");
  await effects.readOnce();
  // Chains 1 and 2 are linked, and each holds PowerGate and ClearQ already.
  assert.equal(effects.addEffect(0, 9), true);
  await flush();
  assert.deepEqual(orders(sent), [
    [0, orderBytes([39, 2], [1, 0], [9, 1])],
    [1, orderBytes([39, 3], [1, 1], [9, 2])],
  ], "the linked partner gets its own instances, the two lowest free");
  assert.deepEqual([effects.bypass(9, 1).value, effects.bypass(9, 2).value], [false, false]);
});

test("removing writes the chain without it and the partner follows; the device clears the instance's enabled", async () => {
  const { store, sent } = setup({ ...quadroReplies(), ...instanceReplies() });
  const effects = store.effects("loopback-0");
  await effects.readOnce();
  const reads = () => sent("loopback-0", "get_afx_strip_order").length;
  assert.equal(reads(), 6);
  assert.equal(effects.removeEffect(0, 0), true);
  assert.deepEqual(effects.chains.value?.[0]?.slots.map((s) => [s.position, s.type]), [[0, 1]], "what follows moves up a slot, at once");
  await flush();
  assert.deepEqual(orders(sent), [
    [0, orderBytes([1, 0])],
    [1, orderBytes([1, 1])],
  ], "the rest keeps its order and the trailing slots are empty");
  assert.equal(reads(), 8, "the two chains written are read back");
  assert.deepEqual(effects.chains.value?.[0]?.slots.map((s) => [s.position, s.type]), [[0, 39], [1, 1]], "and what the device then reports is what is shown");
  assert.deepEqual([effects.bypass(39, 2).value, effects.bypass(39, 3).value], [true, true], "removal sets enabled 0 on the device");
  assert.equal(sent("loopback-0", "set_afx_bypass").length, 0);
  assert.throws(() => effects.removeEffect(2, 0), RangeError);
});

test("moving an effect earlier or later writes its chain once, and the linked partner's too", async () => {
  const { store, sent } = setup({ ...quadroReplies(), ...instanceReplies() });
  const effects = store.effects("loopback-0");
  await effects.readOnce();
  assert.equal(effects.moveEffect(0, 1, -1), true, "ClearQ moves ahead of PowerGate");
  assert.deepEqual(effects.chains.value?.[0]?.slots.map((s) => [s.position, s.type]), [[0, 1], [1, 39]], "shown at once, then read back from the device");
  await flush();
  assert.deepEqual(orders(sent), [
    [0, orderBytes([1, 0], [39, 2])],
    [1, orderBytes([1, 1], [39, 3])],
  ]);

  // The chains were read back after the write and the fake device reports the order it always had.
  assert.equal(effects.moveEffect(0, 0, 1), true, "the same move the other way round");
  await flush();
  assert.deepEqual(orders(sent).slice(2), [
    [0, orderBytes([1, 0], [39, 2])],
    [1, orderBytes([1, 1], [39, 3])],
  ], "one set_afx_order each, never a swap of two");
  assert.equal(effects.moveEffect(0, 0, -1), false, "the first cannot move earlier");
  assert.equal(effects.moveEffect(4, 0, 1), false, "nor the last later");
  assert.throws(() => effects.moveEffect(2, 0, 1), RangeError);
});

test("adding is refused with no free instance, with a full chain, and before the chains are read", async () => {
  const { store, sent } = setup({ ...quadroReplies(), ...instanceReplies({ 39: 0 }) });
  const effects = store.effects("loopback-0");
  assert.equal(effects.addEffect(4, 39), false, "unread: refused, so no chain is written from nothing");
  await effects.readOnce();
  assert.equal(effects.addEffect(4, 39), false, "the device reports no free instance");
  assert.equal(effects.offers(4).find((o) => o.type === 39)?.unavailable, "No free instance of this effect is left.");
  assert.throws(() => effects.addEffect(4, 999), RangeError);

  const full: Record<number, [number, number][]> = { 3: [[9, 0], [9, 1], [9, 2], [9, 3], [9, 4], [9, 5], [9, 6], [9, 7]] };
  const packed = setup({ ...quadroReplies(), ...instanceReplies(), get_afx_strip_order: (call) => ({ entries: [{ slots: slots(...(full[Number(call.options?.["ext3"])] ?? [])) }] }) });
  const other = packed.store.effects("loopback-0");
  await other.readOnce();
  assert.equal(other.offers(3).find((o) => o.type === 39)?.unavailable, "This chain has all eight slots filled.");
  assert.equal(other.addEffect(3, 39), false);
  await flush();
  assert.equal(packed.sent("loopback-0", "set_afx_order").length + sent("loopback-0", "set_afx_order").length, 0);
});

test("a chain write that is not sent puts the chain back", async () => {
  const { store, client } = setup({ ...quadroReplies(), ...instanceReplies() });
  const effects = store.effects("loopback-0");
  await effects.readOnce();
  const before = effects.chains.value?.[4]?.slots;
  client.respond = async () => {
    throw new Error("gone");
  };
  assert.equal(effects.addEffect(4, 39), true);
  assert.equal(effects.chains.value?.[4]?.slots.length, 2, "shown at once");
  await flush();
  assert.deepEqual(effects.chains.value?.[4]?.slots, before, "and put back when it was not sent");
});

test("the Studio+ works out free instances from its sixteen chains, since its count reply has no length in the schema", async () => {
  const studio = setup({
    get_afx_order: () => ({ entries: Array.from({ length: 16 }, (_, i) => ({ slots: i < 4 ? slots([3, i]) : slots() })) }),
    get_afx_links: () => ({ entries: Array.from({ length: 8 }, () => ({ linked: 0 })) }),
    get_reverb_config: () => ({ mixer_id: 0, room_size: 0, color: 0, predelay: 0, density: 100, early_ref_gain: 0, late_ref_delay: 0, richness: 0, reverb_time: 0, reverb_level: 25, on: 0 }),
  });
  const effects = studio.store.effects("loopback-1");
  await effects.readOnce();
  assert.equal(studio.sent("loopback-1", "get_afx_available_instances").length, 0, "its reply's count is unresolved, so it is not read");
  const offers = effects.offers(5);
  assert.deepEqual([offers.find((o) => o.type === 3)?.free, offers.find((o) => o.type === 3)?.counted], [0, false], "the Guitar Amp has four instances and all four are in chains");
  assert.equal(offers.find((o) => o.type === 3)?.unavailable, "No free instance of this effect is left.");
  assert.equal(offers.find((o) => o.type === 2)?.free, 16);

  assert.equal(effects.addEffect(5, 2), true);
  await flush();
  assert.deepEqual(studio.sent("loopback-1", "set_afx_order").map((c) => [c.args?.["ch_id"], c.args?.["slots"]]), [[5, orderBytes([2, 0])]]);
});

test("the free instance counts failing does not fail the whole read", async () => {
  const { store } = setup(quadroReplies());
  const effects = store.effects("loopback-0");
  await effects.readOnce();
  assert.equal(effects.needsRead.value, false, "the chains and the reverb were read");
  assert.equal(effects.offers(2).find((o) => o.type === 39)?.counted, false, "with the counts unread, what the chains show is all that is known");
});

test("an added effect's settings are read again when it is opened, since the device may have changed them", async () => {
  const { store, sent } = setup({ ...quadroReplies(), ...instanceReplies(), get_powergate_conf: () => ({ entries: [{ enabled: 1, threshold: 20, range: 0, attack: 1, decay: 1, hold: 1, gain: 0 }] }) });
  const effects = store.effects("loopback-0");
  await effects.readOnce();
  const reads = () => sent("loopback-0", "get_powergate_conf").length;
  await effects.readParameters(39, 0);
  assert.equal(reads(), 1);
  await effects.readParameters(39, 0);
  assert.equal(reads(), 1, "read once until something changes");

  assert.equal(effects.addEffect(4, 39), true);
  await flush();
  await effects.readParameters(39, 0);
  assert.equal(reads(), 2, "the instance it was given is read again");
});

test("the Studio+ forgets the whole type, since one read covers every instance of it", async () => {
  const gate = { enabled: 1, threshold: 60, range: 4, attack: 250, decay: 80, hold: 1200, gain: 0, linked: 0 };
  const studio = setup({
    get_afx_order: () => ({ entries: Array.from({ length: 16 }, (_, i) => ({ slots: i === 0 ? slots([39, 0]) : slots() })) }),
    get_afx_links: () => ({ entries: Array.from({ length: 8 }, () => ({ linked: 0 })) }),
    get_reverb_config: () => ({ mixer_id: 0, room_size: 0, color: 0, predelay: 0, density: 100, early_ref_gain: 0, late_ref_delay: 0, richness: 0, reverb_time: 0, reverb_level: 25, on: 0 }),
    get_powergate_configs: () => ({ entries: Array.from({ length: 16 }, () => gate) }),
  });
  const effects = studio.store.effects("loopback-1");
  await effects.readOnce();
  const reads = () => studio.sent("loopback-1", "get_powergate_configs").length;
  await effects.readParameters(39, 0);
  assert.equal(reads(), 1);
  await effects.readParameters(39, 0);
  assert.equal(reads(), 1, "read once until something changes");

  // A second PowerGate goes into another chain, on instance 1: the read that covers instance 0 too
  // is forgotten, so opening the first one reads the type again.
  assert.equal(effects.addEffect(1, 39), true);
  await flush();
  await effects.readParameters(39, 0);
  assert.equal(reads(), 2, "the type is forgotten, not only the instance that was given out");
});

test("readChainsOnce reads the chains alone, once, for pages that only need to know what is loaded", async () => {
  const { store, sent } = setup(quadroReplies());
  const effects = store.effects("loopback-0");
  assert.equal(await effects.readChainsOnce(), true);
  assert.equal(sent("loopback-0", "get_afx_strip_order").length, 6, "every chain, and nothing else");
  assert.equal(sent("loopback-0", "get_reverb_config").length + sent("loopback-0", "get_afx_available_instances").length, 0, "the reverb and the instance counts are the Effects page's own reads");
  assert.deepEqual(effects.chains.value?.[4]?.slots.map((s) => s.name), ["FET-A76"]);

  assert.equal(await effects.readChainsOnce(), false, "read once");
  assert.equal(sent("loopback-0", "get_afx_strip_order").length, 6);

  // The Effects page's own read still happens, and forgetting brings both back.
  assert.equal(effects.needsRead.value, true, "the chains are not the whole page");
  assert.equal(await effects.readOnce(), true);
  effects.forget();
  assert.equal(await effects.readChainsOnce(), true);
});
