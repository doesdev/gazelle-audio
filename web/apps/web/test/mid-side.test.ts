// Monitoring a mid and a side microphone as stereo (store/mid-side.ts): the plan lists every change
// and sends nothing until it is applied, both side strips made through matching effect chains (one
// inverting) with the side channel muted, the refusal without two free chains, the Width, what the
// guard notices and how it is mended, removal putting everything back, and decodes saved by earlier
// ways still loading. Tested against a fake Quadro that keeps routing, strips, chains and effect
// settings, so what is read back after a change is the device's.

import { test } from "node:test";
import assert from "node:assert/strict";

import type { MidSide, MixerChannel, Workspace } from "gazelle-audio-client";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { loadCatalogue } from "../src/store/effect-parameters.ts";
import { addToMixPlan, decodeOf, decodes, freeChains, midSideGuard, problems, removePlan, repairPlan, setupPlan, setWidth, UndoneError, width, type Planned, type Plan } from "../src/store/mid-side.ts";
import type { RouteSlot } from "../src/store/routing.ts";
import { Store } from "../src/store/store.ts";
import { builtInThemes, device, FakeClient, flush, MemoryStorage } from "./fake-client.ts";

// The effects' settings are read, which needs the catalogue the Effects page fetches.
await loadCatalogue();

const Q = "loopback-0";
/** Quadro topology positions, as `refs/schemas/quadro_topology.json` lists them. */
const PREAMP = 0;
const AFX_OUT = 5;
const MUTE = 10;
const AFX_IN = 7;
const MIX_IN = [8, 9, 10, 11];
/** The BAE-1084: the first effect with a polarity switch that is tried. */
const BAE = 25;
const BAE_1073 = 7;
/** Each equaliser's command name and its starting values, as its set command carries them (polarity apart). */
const EQUALISERS: Record<number, { name: string; flat: Record<string, number> }> = {
  25: { name: "neve_1084", flat: { gain: 0, high_freq: 0, high_gain: 50, peak_freq: 0, peak_gain: 50, low_freq: 0, low_gain: 50, high_pass: 0, low_pass: 0, hi_q: 0 } },
  24: { name: "neve_1023", flat: { gain: 0, high_freq: 0, high_gain: 50, peak_freq: 0, peak_gain: 50, low_freq: 0, low_gain: 50, high_pass: 0 } },
  7: { name: "neve_1073", flat: { gain: 0, high_shelf: 8, peak_freq: 0, peak_gain: 8, low_freq: 0, low_gain: 8, high_pass: 0 } },
};
const FLAT = (EQUALISERS[BAE] as { flat: Record<string, number> }).flat;
const equaliser = (command: string) => Number(Object.entries(EQUALISERS).find(([, e]) => command === `get_${e.name}_conf` || command === `set_${e.name}_conf`)?.[0] ?? -1);

interface Bench {
  /** Each chain's effects as `[type, inst]`; a chain not named is empty. */
  chains?: Record<number, [number, number][]>;
  /** Mix 1's pans by slot, and levels, as the device holds them. */
  strips?: Record<string, { level?: number; pan?: number; mute?: boolean }>;
  /** Free instances by type, as the device counts them; not answered when absent. */
  instances?: Record<number, number>;
  links?: Workspace["links"];
  groups?: Workspace["mixers"][string]["groups"];
  channels?: MixerChannel[];
  /** The Quadro's effect returns, AFX Out k on slot k of mix 1, as the vendor keeps them. */
  returns?: boolean;
  dryRun?: boolean;
}

const channel = (id: string, name: string, slot: number, preamp: number, extra: Partial<MixerChannel> = {}): MixerChannel => ({ id, name, slot, source: { group: PREAMP, channel: preamp }, main_mix: 0, sends: [], ...extra });

async function bench(options: Bench = {}) {
  const client = new FakeClient(device(Q, "quadro", "Zen Quadro"));
  const dryRun = options.dryRun === true;
  client.server = { ...client.server, dry_run: dryRun };
  const muted = (): RouteSlot[] => Array.from({ length: 32 }, () => ({ source: MUTE, channel: 0 }));
  const routes: Record<number, RouteSlot[]> = {};
  const group = (index: number) => (routes[index] ??= muted());
  const strips: Record<string, { level: number; pan: number; mute: boolean; solo: boolean }> = {};
  const stripOf = (mix: number, slot: number) => (strips[`${mix}:${slot}`] ??= { level: 0, pan: 32, mute: false, solo: false });
  for (const [key, values] of Object.entries(options.strips ?? {})) Object.assign(stripOf(0, Number(key)), values);
  const chains: Record<number, [number, number][]> = { ...options.chains };
  const settings: Record<string, Record<string, number>> = {};
  const channels = options.channels ?? [channel("m", "Mid", 6, 0), channel("s", "Side", 7, 1)];
  // The device routes what the layout says: each channel's input on its slot, in its mixes.
  for (const c of channels) for (const mix of [c.main_mix, ...c.sends]) if (mix !== undefined && c.source !== undefined) group(MIX_IN[mix] as number)[c.slot] = { source: c.source.group, channel: c.source.channel };
  if (options.returns === true) for (let k = 0; k < 6; k++) group(MIX_IN[0] as number)[k] = { source: AFX_OUT, channel: k };
  client.respond = async (call) => {
    const args = (call.args ?? {}) as Record<string, unknown>;
    const ext3 = Number(call.options?.["ext3"]);
    let response: Record<string, unknown> | null = null;
    if (!dryRun) {
      switch (call.command) {
        case "set_routing":
          routes[Number(args["bank_idx"])] = (args["bank_configs"] as Uint8Array[]).map((pair) => ({ source: pair[0] as number, channel: pair[1] as number }));
          break;
        case "get_routing":
          response = { bank_idx: ext3, bank_configs: group(ext3).map((s) => ({ in_periph_id: s.source, in_chann: s.channel })) };
          break;
        case "set_mixer":
          Object.assign(stripOf(Number(args["mixer_id"]), Number(args["channel"]) - 1), { level: args["level"], pan: args["pan"], mute: args["mute"] === 1, solo: args["solo"] === 1 });
          break;
        case "get_mixer":
          response = { entries: [{ level: 0, pan: 32, mute: 0, solo: 0 }, ...Array.from({ length: 32 }, (_, i) => stripOf(ext3, i)).map((s) => ({ level: s.level, pan: s.pan, mute: s.mute ? 1 : 0, solo: s.solo ? 1 : 0 }))] };
          break;
        case "get_mixer_links":
          response = { entries: Array.from({ length: 64 }, () => ({ linked: 0 })) };
          break;
        case "get_afx_strip_order":
          response = { entries: [{ slots: Array.from({ length: 8 }, (_, i) => ({ type: chains[ext3]?.[i]?.[0] ?? 0, inst: chains[ext3]?.[i]?.[1] ?? 0 })) }] };
          break;
        case "set_afx_order": {
          const bytes = args["slots"] as Uint8Array;
          chains[Number(args["ch_id"])] = Array.from({ length: 8 }, (_, i): [number, number] => [bytes[i * 2] as number, bytes[i * 2 + 1] as number]).filter(([type]) => type !== 0);
          break;
        }
        case "get_afx_links":
          response = { entries: Array.from({ length: 7 }, () => ({ linked: 0 })) };
          break;
        case "get_afx_available_instances":
          if (options.instances !== undefined) response = { entries: Object.entries(options.instances).map(([type, count]) => ({ type_id: Number(type), inst_count: count })) };
          break;
        default: {
          // The equalisers' settings, by type and instance.
          const type = equaliser(call.command);
          if (type < 0) break;
          if (call.command.startsWith("get_")) {
            response = { entries: [{ enabled: 1, ...EQUALISERS[type]?.flat, phase_inv: 0, ...settings[`${type}:${Number(args["id"])}`] }] };
          } else {
            const { type_id: _type, inst_id, ...values } = args as Record<string, number>;
            settings[`${type}:${inst_id}`] = values;
          }
        }
      }
    }
    return { device_id: call.deviceId, command: call.command, sent_hex: "70", sent_len: 1, dry_run: dryRun, response, response_error: null };
  };
  client.stored = { version: 1, groups: [], links: options.links ?? [], aliases: {}, mixers: { [Q]: { mixes: [{ name: "Tracking" }, { name: "Cue" }], groups: options.groups ?? [], channels } } };
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), requestFrame: (callback) => callback(), themeSources: builtInThemes });
  await store.start();
  // What the Mixer page reads as it opens: the mixes, the routing and the chains.
  await store.readMixes(Q);
  await store.readRoutes(Q);
  await store.effects(Q).readChainsOnce();
  client.invocations.length = 0;
  client.puts.length = 0;
  /** Every command that changes the device, in order, since the last call. */
  const sent = () => client.invocations.splice(0).filter((c) => c.command.startsWith("set_"));
  const settle = async () => {
    for (let i = 0; i < 5; i++) await flush();
  };
  const layout = () => store.channels(Q).layout.peek();
  return { client, store, routes, strips: stripOf, chains, settings, sent, settle, layout };
}

function ok(planned: Planned): Plan {
  if (!planned.ok) assert.fail(`refused: ${planned.why}`);
  return planned.plan;
}

const texts = (store: Store, group: string, mix = 0) => problems(store, Q, group, mix).map((p) => p.text);

/** A decode set up on a fresh bench: Mid on Preamp 1 at slot 6, Side on Preamp 2 at slot 7, Side L through AFX In 1 on slot 8, Side Ø through AFX In 2 on slot 9. */
async function decoded(options: Bench = {}) {
  const made = await bench(options);
  await ok(setupPlan(made.store, Q, 0, "m", "s")).apply();
  await made.settle();
  made.sent();
  const found = decodes(made.store, Q)[0];
  assert.ok(found, "the decode is in the layout");
  return { ...made, group: found.group.id, left: found.left as MixerChannel, copy: found.inverted as MixerChannel };
}

test("both side strips go through free chains holding the same effect, one inverting, the side channel is muted, and nothing is sent until it is applied", async () => {
  const pair = { id: "l0", kind: "mixer" as const, mode: "absolute" as const, members: [{ device_id: Q, channel: 6 }, { device_id: Q, channel: 7 }] };
  const { client, store, routes, strips, chains, settings, sent, settle, layout } = await bench({ chains: { 0: [[9, 0]] }, returns: true, strips: { 6: { pan: 20 }, 7: { pan: 44, level: 6 } }, links: [pair] });
  const side = layout().channels[1] as MixerChannel;
  assert.deepEqual(freeChains(store, Q, side), { chains: [1, 2], type: BAE, label: "BAE-1084 in AFX In 2 and AFX In 3" }, "AFX In 1 holds an effect, so the next two are taken");

  const plan = ok(setupPlan(store, Q, 0, "m", "s"));
  assert.deepEqual(plan.lines, [
    "Route Preamp 2 into AFX In 2 as well, in place of Mute",
    "Route Preamp 2 into AFX In 3 as well, in place of Mute",
    "Add BAE-1084 to AFX In 2, which is empty, with every setting at its starting value",
    "Add BAE-1084 to AFX In 3, which is empty, with the same settings and its polarity switch on",
    "Mute the effect return AFX Out 2 on slot 2 of Tracking, which would play the side signal a second time",
    "Mute the effect return AFX Out 3 on slot 3 of Tracking, which would play the inverted copy a second time",
    "Pan Mid to the centre in Tracking (it is at L 40%)",
    "Add a channel, Side L, on AFX Out 2 to Tracking: the side signal, panned hard left at Side's level, -6 dB",
    "Add a channel, Side Ø, on AFX Out 3 to Tracking: the inverted copy, panned hard right at the same level",
    "Mute Side in Tracking while the decode plays there: Side L carries the side signal instead, with the same delay as Side Ø",
    "Link Side L and Side Ø, so their levels, mutes and solos always match",
    'Group the four as "M/S: Mid"',
  ]);
  assert.match(plan.notes[0] ?? "", /^Measured on a Quadro at 96 kHz, at a modest level: BAE-1084 at its starting values delays by 4 samples \(42 microseconds\) and leaves the level as it is; an empty chain adds nothing\. Louder signals may add some saturation\./);
  assert.match(plan.notes[0] ?? "", /leaves the mono sum untouched\. To check by ear, mute the mid and switch the mix to mono: the side strips should cancel to near silence\.$/);
  assert.match(plan.notes[1] ?? "", /^What is recorded does not change: Preamp 1 and Preamp 2 reach your DAW raw/);
  await settle();
  assert.deepEqual(sent(), [], "working the plan out sends nothing");
  assert.deepEqual([layout().groups, layout().channels.length, store.links.links.peek()], [[], 2, [pair]], "and changes nothing in the workspace");

  await plan.apply();
  await settle();
  assert.deepEqual([chains[1], chains[2]], [[[BAE, 0]], [[BAE, 1]]], "one instance each");
  assert.deepEqual([settings[`${BAE}:0`], settings[`${BAE}:1`]], [{ ...FLAT, phase_inv: 0 }, { ...FLAT, phase_inv: 1 }], "both flat, the second inverting");
  assert.deepEqual([routes[AFX_IN]?.[1], routes[AFX_IN]?.[2]], [{ source: PREAMP, channel: 1 }, { source: PREAMP, channel: 1 }]);
  assert.deepEqual([strips(0, 1).mute, strips(0, 2).mute], [true, true], "both effect returns are muted in this mix");
  assert.deepEqual([strips(0, 6).pan, strips(0, 7), strips(0, 8), strips(0, 9)], [32, { level: 6, pan: 44, mute: true, solo: false }, { level: 6, pan: 2, mute: false, solo: false }, { level: 6, pan: 62, mute: false, solo: false }], "the mid centred, the side muted where it was, and the two side strips hard left and right at its level");
  assert.equal(client.invocations.some((c) => c.command.startsWith("set_pre")), false, "no preamp is touched");

  const { groups, channels } = layout();
  const left = channels.find((c) => c.name === "Side L");
  const copy = channels.find((c) => c.name === "Side Ø");
  assert.deepEqual([left?.slot, left?.source, copy?.slot, copy?.source], [8, { group: AFX_OUT, channel: 1 }, 9, { group: AFX_OUT, channel: 2 }]);
  assert.deepEqual(channels.map((c) => [c.id, c.group]), [["m", groups[0]?.id], ["s", groups[0]?.id], [left?.id, groups[0]?.id], [copy?.id, groups[0]?.id]], "the four sit together in the group, in order");
  const expected: MidSide = {
    mid: "m",
    side: "s",
    left: left?.id ?? "",
    inverted: copy?.id ?? "",
    via: "effect",
    chain: 2,
    effect_type: BAE,
    effect_inst: 1,
    left_chain: 1,
    left_effect_inst: 0,
    mid_source: { group: PREAMP, channel: 0 },
    side_source: { group: PREAMP, channel: 1 },
    pans: { "0": { mid: 20, side: 44 } },
    left_returns_muted: [0],
    returns_muted: [0],
    side_muted: [0],
  };
  assert.deepEqual(groups[0]?.mid_side, expected);
  assert.deepEqual(store.links.links.peek().map((l) => [l.kind, l.mode, l.members.map((m) => m.channel)]), [["mixer", "absolute", [6, 7]], ["mixer", "absolute", [8, 9]]], "the side strips are tied, and Mid and Side keep their link");
  const group = groups[0]?.id ?? "";
  assert.deepEqual(texts(store, group), [], "a decode just set up is sound");
});

test("the guard: an effect bypassed, its polarity switch off, the two set apart, a chain rerouted, a return or the dry side playing; each is mended", async () => {
  const { store, group, settings, settle } = await decoded({ returns: true });
  const effects = store.effects(Q);
  const mixer = store.mixer(Q, 0);
  effects.setBypass(1, 0, true);
  effects.setParameter(1, 0, "phase_inv", 0);
  effects.setParameter(0, 0, "gain", 3);
  effects.setParameter(0, 0, "high_pass", 2);
  await store.routing(Q).route(AFX_IN, 0, { source: PREAMP, channel: 3 });
  await mixer.setAlone(1, { mute: false });
  await mixer.setAlone(7, { mute: false });
  await settle();
  assert.deepEqual(texts(store, group), [
    "AFX In 1 takes Preamp 4, not Preamp 2, the side microphone.",
    "BAE-1084 in AFX In 2 is bypassed, so the side does not cancel in mono.",
    "BAE-1084's polarity switch in AFX In 2 is off, so the copy is not inverted.",
    "BAE-1084 in AFX In 2 is not set like the one in AFX In 1 (Gain, High pass), so the two side strips do not match.",
    "The effect return AFX Out 2 also plays in Tracking, so the inverted copy is heard twice.",
    "Side plays dry in Tracking as well, ahead of the side strips, so the side does not cancel in mono.",
  ]);
  assert.equal(midSideGuard(store).strip(Q, 0, 7)?.role, "dry");
  assert.equal(midSideGuard(store).strip(Q, 0, 8)?.role, "side");
  const plan = ok(repairPlan(store, Q, group, 0));
  assert.deepEqual(plan.lines, [
    "Route Preamp 2 into AFX In 1 again",
    "Let BAE-1084 in AFX In 2 process again",
    "Switch BAE-1084's polarity in AFX In 2 on again",
    "Set BAE-1084 in AFX In 2 like the one in AFX In 1, polarity apart",
    "Mute the effect return AFX Out 2 in Tracking",
    "Mute Side in Tracking",
  ]);
  await plan.apply();
  await settle();
  assert.deepEqual(texts(store, group), []);
  assert.deepEqual(settings[`${BAE}:1`], { ...FLAT, gain: 3, high_pass: 2, phase_inv: 1 }, "set like the first, and inverting");

  // The polarity switched on where it should be off: both strips inverted, which a decode cannot tell from right.
  effects.setParameter(0, 0, "phase_inv", 1);
  await settle();
  assert.deepEqual(texts(store, group), ["BAE-1084's polarity switch in AFX In 1 is on, so both side strips are inverted."]);
  await ok(repairPlan(store, Q, group, 0)).apply();
  await settle();
  assert.equal(settings[`${BAE}:0`]?.["phase_inv"], 0);

  // An effect taken out of its chain is put back set like the other, and another effect added is taken out.
  effects.removeEffect(1, 0);
  await settle();
  assert.deepEqual(texts(store, group), ["AFX In 2 is empty, not the BAE-1084 that inverts the copy, so the side does not cancel in mono."]);
  const mend = ok(repairPlan(store, Q, group, 0));
  assert.deepEqual(mend.lines, ["Add BAE-1084 to AFX In 2 again, set like the one in AFX In 1 but with its polarity switch on"]);
  await mend.apply();
  await settle();
  assert.deepEqual(settings[`${BAE}:1`], { ...FLAT, gain: 3, high_pass: 2, phase_inv: 1 });
  assert.deepEqual(texts(store, group), []);
  effects.addEffect(0, 9);
  await settle();
  assert.deepEqual(texts(store, group), ["AFX In 1 also holds FET-A76, so the two side strips no longer match."]);
  await ok(repairPlan(store, Q, group, 0)).apply();
  await settle();
  assert.deepEqual(texts(store, group), []);
});

test("the Width moves both side strips through their link, and never the mid", async () => {
  const { store, group, strips, sent, settle } = await decoded({ strips: { 6: { level: 4 } } });
  assert.equal(width(store, Q, group, 0), 4, "the side strips at 0 dB sit 4 dB above a mid at -4 dB");
  setWidth(store, Q, group, 0, -5);
  await settle();
  assert.deepEqual(sent().map((c) => [c.args?.["channel"], c.args?.["level"]]), [[9, 9], [10, 9]], "one command each for the two side strips");
  assert.deepEqual([strips(0, 6).level, strips(0, 8).level, strips(0, 9).level], [4, 9, 9]);
  assert.equal(width(store, Q, group, 0), -5);
  setWidth(store, Q, group, 0, 30);
  await settle();
  assert.deepEqual([strips(0, 8).level, strips(0, 9).level], [0, 0], "held at the top of the fader");
  assert.equal(width(store, Q, group, 0), 4);
  assert.equal(width(store, Q, group, 1), undefined, "it does not play in the Cue mix");
  assert.deepEqual(texts(store, group), []);
});

test("the guard: side strips' levels and mutes apart, a pan off its side, and the link gone are each named and mended", async () => {
  const { store, group, strips, settle } = await decoded();
  const mixer = store.mixer(Q, 0);
  // Linked, the copy follows the hard-left strip: no drift.
  mixer.setLevel(8, 12, true);
  mixer.toggleMute(8, true);
  await settle();
  assert.deepEqual([strips(0, 9).level, strips(0, 9).mute], [12, true]);
  assert.deepEqual(texts(store, group), []);
  mixer.toggleMute(8, true);

  // The link removed by hand, then one strip moved and muted alone, and the pans touched.
  store.links.removeMember("mixer", Q, 9);
  mixer.setLevel(9, 20, true);
  await mixer.setAlone(9, { mute: true });
  mixer.setPan(8, 10);
  mixer.setPan(9, 2);
  mixer.setPan(6, 40);
  assert.deepEqual(texts(store, group), [
    "Side L and Side Ø are no longer linked, so their levels and mutes can drift apart.",
    "Side L is panned L 73% in Tracking, not hard left, so left and right no longer decode.",
    "Side Ø is panned hard left in Tracking, not hard right, so left and right no longer decode.",
    "Mid is panned R 27% in Tracking, not centre, so the image leans to one side.",
    "Side L is at -12 dB and Side Ø at -20 dB in Tracking: the two must match.",
    "Side Ø is muted in Tracking and Side L is not, so only one side plays.",
  ]);
  assert.match(midSideGuard(store).strip(Q, 0, 6)?.warning ?? "", /no longer linked/);
  assert.match(midSideGuard(store).group(Q, 0, group) ?? "", /the two must match/);

  const plan = ok(repairPlan(store, Q, group, 0));
  assert.deepEqual(plan.lines, [
    "Link Side L and Side Ø again, each taking the same value",
    "Pan Side L hard left in Tracking",
    "Pan Side Ø hard right in Tracking",
    "Pan Mid to the centre in Tracking",
    "Set Side Ø to -12 dB in Tracking",
    "Unmute Side Ø in Tracking",
  ]);
  await plan.apply();
  await settle();
  assert.deepEqual([strips(0, 6).pan, strips(0, 8), strips(0, 9)], [32, { level: 12, pan: 2, mute: false, solo: false }, { level: 12, pan: 62, mute: false, solo: false }]);
  assert.deepEqual(texts(store, group), []);
  assert.equal(repairPlan(store, Q, group, 0).ok, false, "nothing left to put back");
});

test("the guard: an input changed, a channel taken out of the mix, a channel removed, and a mono mix left alone", async () => {
  const { store, group, left, copy, settle } = await decoded();
  const channels = store.channels(Q);

  await channels.setSource("m", { group: PREAMP, channel: 1 });
  await channels.setSource(copy.id, { group: AFX_OUT, channel: 4 });
  assert.deepEqual(texts(store, group), ["Mid is on Preamp 2, not Preamp 1, the mid microphone.", "Side Ø is on AFX Out 5, not AFX Out 2, which carries the inverted copy."]);
  await ok(repairPlan(store, Q, group, 0)).apply();
  await settle();
  assert.deepEqual(texts(store, group), []);

  // Mono centres every pan and remembers where each returns to: the decode is still sound.
  channels.setMono(0, true);
  await settle();
  assert.deepEqual(texts(store, group), [], "a mono mix is not a broken decode");
  channels.setMono(0, false);
  await settle();

  await channels.setMainMix(copy.id, 1);
  assert.deepEqual(texts(store, group), ["Side Ø is not in Tracking, so the decode is incomplete here."]);
  await ok(repairPlan(store, Q, group, 0)).apply();
  await settle();
  assert.deepEqual(decodeOf(store, Q, group)?.inverted?.sends, [0]);

  // The side channel itself is not needed for the decode: removing it breaks nothing.
  await channels.remove("s");
  assert.deepEqual(texts(store, group), []);
  await channels.remove(left.id);
  assert.deepEqual(texts(store, group), ["The hard-left side channel has been removed, so nothing is decoded any more. Remove this decode to put the rest back."]);
  assert.equal(repairPlan(store, Q, group, 0).ok, false, "only removing it is left");
  assert.equal(removePlan(store, Q, group).ok, true);
});

test("removing a decode puts back the pans, the side channel's mute, the links, both chains and the groups, and takes the side strips out", async () => {
  const stale = { id: "l9", kind: "mixer" as const, mode: "absolute" as const, members: [{ device_id: Q, channel: 8 }, { device_id: Q, channel: 12 }] };
  const { store, group, left, copy, strips, chains, routes, sent, settle, layout } = await decoded({
    strips: { 6: { pan: 20 }, 7: { pan: 44 } },
    links: [stale],
    returns: true,
    groups: [{ id: "g0", name: "Room", collapsed: false }],
    channels: [channel("m", "Mid", 6, 0, { group: "g0" }), channel("s", "Side", 7, 1, { group: "g0" })],
  });
  assert.deepEqual([chains[0], chains[1]], [[[BAE, 0]], [[BAE, 1]]]);
  const plan = ok(removePlan(store, Q, group));
  assert.deepEqual(plan.lines, [
    "Pan Mid back to L 40% in Tracking",
    "Unlink Side L and Side Ø",
    "Remove the channel Side L, the side signal through its effect chain",
    "Remove the channel Side Ø, the inverted copy",
    "Unmute Side in Tracking",
    "Remove BAE-1084 from AFX In 1",
    "Route Mute into AFX In 1 again",
    "Unmute the effect return AFX Out 1 in Tracking",
    "Remove BAE-1084 from AFX In 2",
    "Route Mute into AFX In 2 again",
    "Unmute the effect return AFX Out 2 in Tracking",
    "Link Side L and strip 13 again",
    'Remove the group "M/S: Mid"; its channels stay, and Mid goes back to the group Room, and Side goes back to the group Room',
  ]);
  await settle();
  assert.deepEqual(sent(), [], "nothing is sent until it is confirmed");
  assert.equal(layout().channels.length, 4, "and nothing is taken out");
  await plan.apply();
  await settle();
  assert.deepEqual([strips(0, 6).pan, strips(0, 7).pan, strips(0, 7).mute], [20, 44, false]);
  assert.deepEqual([chains[0], chains[1]], [[], []], "both chains are empty again");
  assert.deepEqual([routes[AFX_IN]?.[0], routes[AFX_IN]?.[1]], [{ source: MUTE, channel: 0 }, { source: MUTE, channel: 0 }]);
  assert.deepEqual([strips(0, 0).mute, strips(0, 1).mute], [false, false]);
  assert.deepEqual([routes[MIX_IN[0] as number]?.[left.slot], routes[MIX_IN[0] as number]?.[copy.slot]], [{ source: MUTE, channel: 0 }, { source: MUTE, channel: 0 }], "the side strips' slots are muted, as removing any channel leaves them");
  assert.deepEqual(layout().channels.map((c) => [c.id, c.group]), [["m", "g0"], ["s", "g0"]]);
  assert.deepEqual(layout().groups.map((g) => g.id), ["g0"]);
  assert.deepEqual(store.links.links.peek().map((l) => [l.kind, l.members.map((m) => m.channel)]), [["mixer", [8, 12]]], "the link an earlier channel left on the free slot is as it was, and nothing else is linked");
  assert.deepEqual(decodes(store, Q), []);
});

test("a decode can be played in another mix the same way, and removing it puts both mixes back", async () => {
  const { store, group, left, copy, strips, settle, layout } = await decoded({ channels: [channel("m", "Mid", 6, 0, { sends: [1] }), channel("s", "Side", 7, 1, { sends: [1] })] });
  const cue = store.mixer(Q, 1);
  cue.setPan(6, 12);
  cue.setLevel(8, 15);
  await settle();
  assert.equal(strips(1, copy.slot).level, 15, "the link holds in every mix, so the copy's level there already matches");
  assert.deepEqual(texts(store, group, 1), [], "nothing is said about a mix it does not play in");
  const plan = ok(addToMixPlan(store, Q, group, 1));
  assert.deepEqual(plan.lines, [
    "Send Side L to Cue too",
    "Send Side Ø to Cue too",
    "Pan Mid to the centre in Cue (it is at L 67%)",
    "Pan Side L hard left in Cue (it is at C)",
    "Pan Side Ø hard right in Cue (it is at C)",
    "Mute Side in Cue while the decode plays there: Side L carries the side signal instead",
    "Remember Cue's pans and mutes, so removing the decode puts them back",
  ]);
  await plan.apply();
  await settle();
  assert.deepEqual([strips(1, 6).pan, strips(1, 7).mute, strips(1, left.slot), strips(1, copy.slot)], [32, true, { level: 15, pan: 2, mute: false, solo: false }, { level: 15, pan: 62, mute: false, solo: false }]);
  const saved = layout().groups[0]?.mid_side;
  assert.deepEqual([saved?.pans, saved?.side_muted], [{ "0": { mid: 32, side: 32 }, "1": { mid: 12, side: 32 } }, [0, 1]]);
  assert.deepEqual(texts(store, group, 1), []);
  assert.equal(addToMixPlan(store, Q, group, 1).ok, false, "it already plays there");
  assert.equal(width(store, Q, group, 1), -15);

  await ok(removePlan(store, Q, group)).apply();
  await settle();
  assert.deepEqual([strips(0, 6).pan, strips(1, 6).pan, strips(0, 7).mute, strips(1, 7).mute], [32, 12, false, false]);
});

test("without two free chains, or two free instances of one inverting effect, or before the chains are read, it is refused with why and what to free up", async () => {
  const full = await bench({ chains: Object.fromEntries([0, 1, 2, 3, 4, 5].map((k) => [k, [[9, k]] as [number, number][]])) });
  const refused = setupPlan(full.store, Q, 0, "m", "s");
  assert.equal(
    refused.ok ? "" : refused.why,
    "The side signal goes through two effect chains, one for each side strip, and none is free: the others hold an effect, are linked, are fed by another input or have a channel on their output. Free two first: on the Effects page take every effect out of a chain and unlink it, route Mute into it, and take any mixer channel off its AFX Out.",
  );

  // One free chain is not enough; nor is a chain with a channel on its output.
  const one = await bench({ chains: Object.fromEntries([1, 2, 3, 4, 5].map((k) => [k, [[9, k]] as [number, number][]])) });
  const why = setupPlan(one.store, Q, 0, "m", "s");
  assert.match(why.ok ? "" : why.why, /and only AFX In 1 is free: .* Free one more first:/);
  const used = await bench({ chains: Object.fromEntries([2, 3, 4, 5].map((k) => [k, [[9, k]] as [number, number][]])), channels: [channel("m", "Mid", 6, 0), channel("s", "Side", 7, 1), channel("x", "Return", 9, 0, { source: { group: AFX_OUT, channel: 0 } })] });
  const taken = setupPlan(used.store, Q, 0, "m", "s");
  assert.match(taken.ok ? "" : taken.why, /only AFX In 2 is free/, "AFX Out 1 already has a channel on it");

  // The most transparent with two instances free is taken: the BAE-1084, else the BAE-1023, else the BAE-1073.
  const short = await bench({ instances: { 7: 16, 24: 2, 25: 1 } });
  await short.store.effects(Q).load();
  const fallback = freeChains(short.store, Q, short.layout().channels[1] as MixerChannel);
  assert.equal("label" in fallback ? fallback.label : fallback.why, "BAE-1023 in AFX In 1 and AFX In 2");

  const spent = await bench({ instances: { 7: 1, 24: 0, 25: 1, 9: 16 } });
  await spent.store.effects(Q).load();
  const none = setupPlan(spent.store, Q, 0, "m", "s");
  assert.equal(none.ok ? "" : none.why, "AFX In 1 and AFX In 2 are free, but two instances of one of BAE-1084, BAE-1023 or BAE-1073 are needed, one for each side strip, and no two of one are left. Take some out of other chains on the Effects page first.");

  const dry = await bench({ dryRun: true });
  const unread = setupPlan(dry.store, Q, 0, "m", "s");
  assert.match(unread.ok ? "" : unread.why, /have not been read/);
});

test("an effect whose settings cannot be read is taken out again, with the other, and nothing else is changed", async () => {
  const { client, store, chains, routes, strips, settle, layout } = await bench({ strips: { 6: { pan: 20 } } });
  const respond = client.respond;
  // The first effect reads; the second does not.
  client.respond = async (call) => ((call.args as Record<string, unknown> | undefined)?.["id"] === 1 && call.command === "get_neve_1084_conf" ? { device_id: call.deviceId, command: call.command, sent_hex: "74", sent_len: 1, dry_run: false, response: null, response_error: "timeout" } : respond(call));
  const plan = ok(setupPlan(store, Q, 0, "m", "s"));
  await assert.rejects(plan.apply(), (error: unknown) => {
    assert.ok(error instanceof UndoneError, "it says nothing was left behind");
    assert.equal(error.message, "BAE-1084's settings in AFX In 2 could not be read, so they could not be set. The chains are as they were, and nothing else was changed.");
    return true;
  });
  await settle();
  assert.deepEqual([chains[0], chains[1]], [[], []], "a chain is never left holding half a decode");
  assert.deepEqual([routes[AFX_IN]?.[0], routes[AFX_IN]?.[1]], [{ source: MUTE, channel: 0 }, { source: MUTE, channel: 0 }], "and the side input is routed out of both again");
  assert.deepEqual([strips(0, 6).pan, strips(0, 7).mute], [20, false], "the pans and mutes come after it, so they were not changed");
  assert.deepEqual(layout().groups, []);
  assert.deepEqual(layout().channels.map((c) => c.id), ["m", "s"]);
});

test("two channels that cannot be a mid and a side are refused, with the reason", async () => {
  const { store, group } = await decoded({ channels: [channel("m", "Mid", 6, 0), channel("s", "Side", 7, 1), channel("x", "Other", 10, 0), channel("y", "Cue only", 11, 3, { main_mix: 1 })] });
  const why = (mid: string, side: string) => {
    const planned = setupPlan(store, Q, 0, mid, side);
    return planned.ok ? "planned" : planned.why;
  };
  assert.match(why("m", "x"), /^M\/S: Mid already uses one of these channels/);
  assert.match(why("x", "x"), /^Select two channels/);
  assert.match(why("x", "y"), /need an input and a place in this mix/);
  await store.channels(Q).setMainMix("y", 0);
  assert.equal(why("x", "y"), "planned", "AFX In 1 and 2 carry the first decode, so the next two chains are taken");
  await store.channels(Q).setSource("y", { group: PREAMP, channel: 0 });
  assert.match(why("x", "y"), /same input/);
  assert.deepEqual(texts(store, group), []);
});

test("decodes saved by earlier ways still load, say they are no longer supported, and can only be removed", async () => {
  const preampLink = { id: "p0", kind: "preamp" as const, mode: "absolute" as const, members: [{ device_id: Q, channel: 1 }, { device_id: Q, channel: 2 }] };
  const sideLink = { id: "l1", kind: "mixer" as const, mode: "absolute" as const, members: [{ device_id: Q, channel: 7 }, { device_id: Q, channel: 8 }] };
  const old: MidSide = { mid: "m", side: "s", inverted: "c", via: "preamp", preamp: 2, mid_source: { group: PREAMP, channel: 0 }, side_source: { group: PREAMP, channel: 1 }, pans: { "0": { mid: 20, side: 44 } }, phase_invert: false };
  const { store, sent, settle, layout } = await bench({
    strips: { 6: { pan: 32 }, 7: { pan: 2 }, 8: { pan: 62 } },
    links: [preampLink, sideLink],
    groups: [{ id: "ms", name: "M/S: Mid", collapsed: false, mid_side: old }],
    channels: [channel("m", "Mid", 6, 0, { group: "ms" }), channel("s", "Side", 7, 1, { group: "ms" }), channel("c", "Side Ø", 8, 2, { group: "ms" })],
  });
  const retiredText = "This decode makes its inverted copy with a second preamp, which Gazelle no longer supports, so it is not checked any more. Remove it to put the pans, links and channels back.";
  assert.deepEqual(texts(store, "ms"), [retiredText]);
  assert.deepEqual([midSideGuard(store).strip(Q, 0, 7)?.role, midSideGuard(store).strip(Q, 0, 8)?.role], ["side", "inverted"], "its strips are still marked");
  assert.equal(midSideGuard(store).strip(Q, 0, 7)?.warning, retiredText);
  assert.equal(repairPlan(store, Q, "ms", 0).ok, false, "nothing to put back");
  const elsewhere = addToMixPlan(store, Q, "ms", 1);
  assert.equal(elsewhere.ok ? "" : elsewhere.why, retiredText);
  assert.equal(width(store, Q, "ms", 0), undefined, "and no Width");

  const plan = ok(removePlan(store, Q, "ms"));
  assert.deepEqual(plan.lines, [
    "Pan Mid back to L 40% in Tracking",
    "Pan Side back to R 40% in Tracking",
    "Unlink Side and Side Ø",
    "Remove the channel Side Ø, the inverted copy",
    'Remove the group "M/S: Mid"; its channels stay',
  ]);
  assert.match(plan.notes[1] ?? "", /^Preamp 3 is left as it is, with its polarity \(Ø\) and any link to the side preamp: check it on the Inputs page/);
  await plan.apply();
  await settle();
  assert.equal(sent().some((c) => c.command.startsWith("set_pre")), false, "the preamp is not touched");
  assert.deepEqual(layout().groups, []);
  assert.deepEqual(layout().channels.map((c) => c.id), ["m", "s"]);
  assert.deepEqual(store.links.links.peek().map((l) => l.kind), ["preamp"], "the preamp link is left for the person to undo");

  // One chain for the copy alone, against the dry side: its chain is put back on removal.
  const single: MidSide = { mid: "m", side: "s", inverted: "c", via: "effect", chain: 1, effect_type: BAE_1073, effect_inst: 0, mid_source: { group: PREAMP, channel: 0 }, side_source: { group: PREAMP, channel: 1 }, pans: { "0": { mid: 32, side: 32 } }, returns_muted: [0] };
  const one = await bench({
    chains: { 1: [[BAE_1073, 0]] },
    groups: [{ id: "ms", name: "M/S: Mid", collapsed: false, mid_side: single }],
    channels: [channel("m", "Mid", 6, 0, { group: "ms" }), channel("s", "Side", 7, 1, { group: "ms" }), channel("c", "Side Ø", 8, 0, { source: { group: AFX_OUT, channel: 1 }, group: "ms" })],
  });
  await one.store.routing(Q).route(AFX_IN, 1, { source: PREAMP, channel: 1 });
  assert.match(texts(one.store, "ms")[0] ?? "", /^This decode sends only the inverted copy through an effect chain, against the dry side/);
  assert.deepEqual(ok(removePlan(one.store, Q, "ms")).lines, [
    "Remove the channel Side Ø, the inverted copy",
    "Remove BAE-1073 from AFX In 2",
    "Route Mute into AFX In 2 again",
    "Unmute the effect return AFX Out 2 in Tracking",
    'Remove the group "M/S: Mid"; its channels stay',
  ]);
});

test("a saved layout carries the group but not the decode, which is this device's own setup", async () => {
  const { store } = await decoded();
  const channels = store.channels(Q);
  const id = channels.saveLayout("With M/S");
  const saved = channels.savedLayouts().find((l) => l.id === id);
  assert.deepEqual(saved?.mixer.groups.map((g) => [g.name, g.mid_side]), [["M/S: Mid", undefined]]);
  assert.notEqual(channels.layout.peek().groups[0]?.mid_side, undefined, "the live layout keeps it");
});
