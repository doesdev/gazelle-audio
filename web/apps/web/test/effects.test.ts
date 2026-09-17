import { test } from "node:test";
import assert from "node:assert/strict";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { EFFECT_NAMES } from "../src/store/effect-catalogue.ts";
import { formatReverbLevel, formatRoomSize, REVERB_RETURN_MAX, REVERB_SEND_MAX } from "../src/store/effects.ts";
import { Store } from "../src/store/store.ts";
import { builtInThemes, device, FakeClient, flush, MemoryStorage, type Invocation } from "./fake-client.ts";

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
    { mixer_id: 0, channel: 3, level: 93, pan: 32, mute: 0, solo: 0 },
    { mixer_id: 0, channel: 16, level: REVERB_SEND_MAX, pan: 32, mute: 0, solo: 0 },
  ], "pan snaps to the centre as the mixer's does; level and pan travel together");
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
