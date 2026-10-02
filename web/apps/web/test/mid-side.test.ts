// Monitoring a mid and a side microphone as stereo (store/mid-side.ts): the plan lists every change
// and sends nothing until it is applied, both ways of making the inverted copy, the Width, what the
// guard notices and how it is mended, and removal putting everything back. Tested against a fake
// Quadro that keeps routing, strips, preamps, chains and effect settings, so what is read back
// after a change is the device's.

import { test } from "node:test";
import assert from "node:assert/strict";

import type { MidSide, MixerChannel, Workspace } from "gazelle-audio-client";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { loadCatalogue } from "../src/store/effect-parameters.ts";
import { addToMixPlan, decodeOf, decodes, midSideGuard, problems, removePlan, repairPlan, setupPlan, setWidth, UndoneError, ways, width, type Planned, type Plan } from "../src/store/mid-side.ts";
import type { RouteSlot } from "../src/store/routing.ts";
import { Store } from "../src/store/store.ts";
import { builtInThemes, device, FakeClient, flush, MemoryStorage } from "./fake-client.ts";

// The effect way reads the inverting effect's settings, which needs the catalogue the Effects page fetches.
await loadCatalogue();

const Q = "loopback-0";
/** Quadro topology positions, as `refs/schemas/quadro_topology.json` lists them. */
const PREAMP = 0;
const USB1 = 1;
const AFX_OUT = 5;
const MUTE = 10;
const AFX_IN = 7;
const MIX_IN = [8, 9, 10, 11];
/** The BAE-1073: the first effect with a polarity switch. */
const BAE = 7;

interface Bench {
  /** Each chain's effects as `[type, inst]`; a chain not named is empty. */
  chains?: Record<number, [number, number][]>;
  /** Mix 1's pans by slot, and levels, as the device holds them. */
  strips?: Record<string, { level?: number; pan?: number; mute?: boolean }>;
  /** Preamps as reported: type, gain and polarity. */
  preamps?: { type: number; gain: number; phase: number }[];
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
  const preamps = options.preamps ?? [0, 1, 2, 3].map(() => ({ type: 0, gain: 30, phase: 0 }));
  const channels = options.channels ?? [channel("m", "Mid", 6, 0), channel("s", "Side", 7, 1)];
  // The device routes what the layout says: each channel's input on its slot, in its mixes.
  for (const c of channels) for (const mix of [c.main_mix, ...c.sends]) if (mix !== undefined && c.source !== undefined) group(MIX_IN[mix] as number)[c.slot] = { source: c.source.group, channel: c.source.channel };
  if (options.returns === true) for (let k = 0; k < 6; k++) group(MIX_IN[0] as number)[k] = { source: AFX_OUT, channel: k };
  const report = () => client.cyclic.get(`${Q}|0x73`)?.({ preamps: preamps.map((p) => ({ type: p.type, phantom: 0, hpf: 0, phase_inv: p.phase, zero_cross: 0 })), preamp_gains: new Uint8Array(preamps.map((p) => p.gain & 0xff)) });
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
        case "get_neve_1073_conf":
          response = { entries: [{ enabled: 1, gain: 0, high_shelf: 8, peak_freq: 0, peak_gain: 8, low_freq: 0, low_gain: 8, high_pass: 0, phase_inv: 0, ...settings[`${BAE}:${Number(args["id"])}`] }] };
          break;
        case "set_neve_1073_conf": {
          const { type_id: _type, inst_id, ...values } = args as Record<string, number>;
          settings[`${BAE}:${inst_id}`] = values;
          break;
        }
        case "set_pre_phase_inv":
          (preamps[Number(args["id"])] as { phase: number }).phase = Number(args["phase_inv"]);
          break;
        case "set_pre_gain":
          (preamps[Number(args["id"])] as { gain: number }).gain = Number(args["gain"]);
          break;
        case "set_pre_type":
          (preamps[Number(args["id"])] as { type: number }).type = Number(args["pretype"]);
          break;
        default:
      }
    }
    return { device_id: call.deviceId, command: call.command, sent_hex: "70", sent_len: 1, dry_run: dryRun, response, response_error: null };
  };
  client.stored = { version: 1, groups: [], links: options.links ?? [], aliases: {}, mixers: { [Q]: { mixes: [{ name: "Tracking" }, { name: "Cue" }], groups: options.groups ?? [], channels } } };
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), requestFrame: (callback) => callback(), themeSources: builtInThemes });
  await store.start();
  // What the Mixer page reads as it opens: the mixes, the routing and the chains; and it follows the status report.
  store.inputs(Q).activate();
  if (!dryRun) report();
  await store.readMixes(Q);
  await store.readRoutes(Q);
  await store.effects(Q).readChainsOnce();
  client.invocations.length = 0;
  client.puts.length = 0;
  /** Every command that changes the device, in order, since the last call. */
  const sent = () => client.invocations.splice(0).filter((c) => c.command.startsWith("set_"));
  const settle = async () => {
    for (let i = 0; i < 5; i++) await flush();
    if (!dryRun) report();
  };
  const layout = () => store.channels(Q).layout.peek();
  return { client, store, routes, strips: stripOf, chains, settings, preamps, sent, settle, layout, report };
}

function ok(planned: Planned): Plan {
  if (!planned.ok) assert.fail(`refused: ${planned.why}`);
  return planned.plan;
}

const texts = (store: Store, group: string, mix = 0) => problems(store, Q, group, mix).map((p) => p.text);

/** A decode through the second preamp, set up on a fresh bench: Mid on Preamp 1, Side on Preamp 2, the copy on Preamp 3. */
async function decoded(options: Bench = {}) {
  const made = await bench(options);
  await ok(setupPlan(made.store, Q, 0, "m", "s", { via: "preamp", preamp: 2 })).apply();
  await made.settle();
  made.sent();
  const found = decodes(made.store, Q)[0];
  assert.ok(found, "the decode is in the layout");
  return { ...made, group: found.group.id, copy: found.inverted as MixerChannel };
}

test("the second preamp way lists every change in order, and sends nothing until it is applied", async () => {
  const pair = { id: "l0", kind: "mixer" as const, mode: "absolute" as const, members: [{ device_id: Q, channel: 6 }, { device_id: Q, channel: 7 }] };
  const { store, strips, sent, settle, layout } = await bench({
    strips: { 6: { pan: 20 }, 7: { pan: 44, level: 6 } },
    preamps: [{ type: 0, gain: 30, phase: 0 }, { type: 0, gain: 34, phase: 0 }, { type: 1, gain: 10, phase: 0 }, { type: 0, gain: 0, phase: 0 }],
    links: [pair],
  });
  const plan = ok(setupPlan(store, Q, 0, "m", "s", { via: "preamp", preamp: 2 }));
  assert.deepEqual(plan.lines, [
    "Set Preamp 3 to Mic, as Preamp 2 is",
    "Set Preamp 3's gain to 34 dB, as Preamp 2's is",
    "Link Preamp 3 to Preamp 2, so its gain, type and 48V follow",
    "Switch Preamp 3's polarity (Ø) on",
    "Pan Mid to the centre in Tracking (it is at L 40%)",
    "Pan Side hard left in Tracking (it is at R 40%)",
    "Add a channel, Side Ø, on Preamp 3 to Tracking: the inverted copy, panned hard right at Side's level, -6 dB",
    "Link Side and Side Ø, so their levels, mutes and solos always match; Side leaves its link with Mid",
    'Group the three as "M/S: Mid"',
  ]);
  assert.deepEqual(plan.notes, [
    "Feed Preamp 3 from a split (a Y cable) of the side microphone.",
    "What is recorded does not change: Preamp 1 and Preamp 2 reach your DAW raw, as they do now, for decoding later. Only the mix hears the decoded stereo.",
    "Preamp 3 is one more input your DAW can record: the side microphone again, inverted. It can be ignored.",
  ]);
  await settle();
  assert.deepEqual(sent(), [], "working the plan out sends nothing");
  assert.deepEqual([layout().groups, layout().channels.length, store.links.links.peek()], [[], 2, [pair]], "and changes nothing in the workspace");

  await plan.apply();
  await settle();
  const commands = sent();
  assert.deepEqual(commands.filter((c) => c.command.startsWith("set_pre")).map((c) => [c.command, c.args]), [
    ["set_pre_type", { id: 2, pretype: 0 }],
    ["set_pre_gain", { id: 2, gain: 34 }],
    ["set_pre_phase_inv", { id: 2, phase_inv: 1 }],
  ], "only the second preamp is touched: the side preamp is recorded as it was");
  assert.deepEqual([strips(0, 6).pan, strips(0, 7).pan, strips(0, 8)], [32, 2, { level: 6, pan: 62, mute: false, solo: false }], "centre, hard left, and the copy hard right at the side's level");

  const { groups, channels } = layout();
  const copy = channels.find((c) => c.name === "Side Ø");
  assert.deepEqual({ slot: copy?.slot, source: copy?.source, main_mix: copy?.main_mix }, { slot: 8, source: { group: PREAMP, channel: 2 }, main_mix: 0 });
  assert.deepEqual(channels.map((c) => [c.id, c.group]), [["m", groups[0]?.id], ["s", groups[0]?.id], [copy?.id, groups[0]?.id]], "the three sit together in the group, in order");
  assert.equal(groups[0]?.name, "M/S: Mid");
  const expected: MidSide = { mid: "m", side: "s", inverted: copy?.id ?? "", via: "preamp", preamp: 2, mid_source: { group: PREAMP, channel: 0 }, side_source: { group: PREAMP, channel: 1 }, pans: { "0": { mid: 20, side: 44 } }, phase_invert: false, displaced_links: [pair] };
  assert.deepEqual(groups[0]?.mid_side, expected);
  const links = store.links.links.peek().map((l) => [l.kind, l.mode, l.members.map((m) => m.channel)]);
  assert.deepEqual(links, [["preamp", "absolute", [1, 2]], ["mixer", "absolute", [7, 8]]], "the side strips are tied, the second preamp follows the side's, and Mid is out of Side's old link");
  assert.deepEqual(texts(store, groups[0]?.id ?? ""), [], "a decode just set up is sound");
});

test("a preamp already matched and inverted needs no change, and a side preamp with its own polarity on gets the opposite", async () => {
  const same = await bench({ preamps: [{ type: 0, gain: 30, phase: 0 }, { type: 0, gain: 34, phase: 0 }, { type: 0, gain: 34, phase: 1 }, { type: 0, gain: 0, phase: 0 }] });
  const lines = ok(setupPlan(same.store, Q, 0, "m", "s", { via: "preamp", preamp: 2 })).lines;
  assert.equal(lines.some((line) => /Set Preamp 3|polarity/.test(line)), false, `nothing to set on a preamp that already matches: ${lines.join(" | ")}`);

  const flipped = await bench({ preamps: [{ type: 0, gain: 30, phase: 0 }, { type: 0, gain: 30, phase: 1 }, { type: 0, gain: 30, phase: 1 }, { type: 0, gain: 0, phase: 0 }] });
  const plan = ok(setupPlan(flipped.store, Q, 0, "m", "s", { via: "preamp", preamp: 2 }));
  assert.ok(plan.lines.includes("Switch Preamp 3's polarity (Ø) off, the opposite of Preamp 2's"));
  await plan.apply();
  await flipped.settle();
  assert.deepEqual(flipped.preamps.map((p) => p.phase), [0, 1, 0, 0], "inverted against the side preamp, whichever way that is set");
  assert.deepEqual(texts(flipped.store, decodes(flipped.store, Q)[0]?.group.id ?? ""), []);
});

test("the Width moves both side strips through their link, and never the mid", async () => {
  const { store, group, strips, sent, settle } = await decoded({ strips: { 6: { level: 4 } } });
  assert.equal(width(store, Q, group, 0), 4, "the side strips at 0 dB sit 4 dB above a mid at -4 dB");
  setWidth(store, Q, group, 0, -5);
  await settle();
  assert.deepEqual(sent().map((c) => [c.args?.["channel"], c.args?.["level"]]), [[8, 9], [9, 9]], "one command each for the side strip and its inverted copy");
  assert.deepEqual([strips(0, 6).level, strips(0, 7).level, strips(0, 8).level], [4, 9, 9]);
  assert.equal(width(store, Q, group, 0), -5);
  setWidth(store, Q, group, 0, 30);
  await settle();
  assert.deepEqual([strips(0, 7).level, strips(0, 8).level], [0, 0], "held at the top of the fader");
  assert.equal(width(store, Q, group, 0), 4);
  assert.equal(width(store, Q, group, 1), undefined, "it does not play in the Cue mix");
  assert.deepEqual(texts(store, group), []);
});

test("the guard: side levels and mutes apart, a pan off its side, and the link gone are each named and mended", async () => {
  const { store, group, strips, settle } = await decoded();
  const mixer = store.mixer(Q, 0);
  // Linked, the copy follows the side strip: no drift.
  mixer.setLevel(7, 12, true);
  mixer.toggleMute(7, true);
  await settle();
  assert.deepEqual([strips(0, 8).level, strips(0, 8).mute], [12, true]);
  assert.deepEqual(texts(store, group), []);
  mixer.toggleMute(7, true);

  // The link removed by hand, then one side moved and muted alone, and both pans touched.
  store.links.removeMember("mixer", Q, 8);
  mixer.setLevel(8, 20, true);
  await mixer.setAlone(8, { mute: true });
  mixer.setPan(7, 10);
  mixer.setPan(8, 2);
  mixer.setPan(6, 40);
  assert.deepEqual(texts(store, group), [
    "Side and Side Ø are no longer linked, so their levels and mutes can drift apart.",
    "Side is panned L 73% in Tracking, not hard left, so left and right no longer decode.",
    "Side Ø is panned hard left in Tracking, not hard right, so left and right no longer decode.",
    "Mid is panned R 27% in Tracking, not centre, so the image leans to one side.",
    "Side is at -12 dB and Side Ø at -20 dB in Tracking: the two must match.",
    "Side Ø is muted in Tracking and Side is not, so only one side plays.",
  ]);
  assert.deepEqual(midSideGuard(store).strip(Q, 0, 8)?.role, "inverted");
  assert.match(midSideGuard(store).strip(Q, 0, 6)?.warning ?? "", /no longer linked/);
  assert.match(midSideGuard(store).group(Q, 0, group) ?? "", /the two must match/);

  const plan = ok(repairPlan(store, Q, group, 0));
  assert.deepEqual(plan.lines, [
    "Link Side and Side Ø again, each taking the same value",
    "Pan Side hard left in Tracking",
    "Pan Side Ø hard right in Tracking",
    "Pan Mid to the centre in Tracking",
    "Set Side Ø to -12 dB in Tracking",
    "Unmute Side Ø in Tracking",
  ]);
  await plan.apply();
  await settle();
  assert.deepEqual([strips(0, 6).pan, strips(0, 7), strips(0, 8)], [32, { level: 12, pan: 2, mute: false, solo: false }, { level: 12, pan: 62, mute: false, solo: false }]);
  assert.deepEqual(texts(store, group), []);
  assert.equal(midSideGuard(store).strip(Q, 0, 7)?.warning, undefined);
  assert.equal(repairPlan(store, Q, group, 0).ok, false, "nothing left to put back");
});

test("the guard: the copy losing its inversion, by its own switch or through the preamp link, and a gain apart", async () => {
  const { store, group, preamps, sent, settle } = await decoded();
  const inputs = store.inputs(Q);
  assert.deepEqual(preamps.map((p) => p.phase), [0, 0, 1, 0]);

  // The second preamp's own switch turned off.
  inputs.setPhaseInvert(2, false, false);
  await settle();
  assert.deepEqual(texts(store, group), ["Preamp 3's polarity (Ø) is off, the same as Preamp 2's, so the copy is not inverted: left and right play the same thing."]);
  await ok(repairPlan(store, Q, group, 0)).apply();
  await settle();
  assert.deepEqual(preamps.map((p) => p.phase), [0, 0, 1, 0]);
  assert.deepEqual(texts(store, group), []);

  // The side preamp's switch, as its channel head sends it: the link copies it to the second preamp.
  sent();
  inputs.setPhaseInvert(1, true);
  await settle();
  assert.deepEqual(preamps.map((p) => p.phase), [0, 1, 1, 0], "the link made them the same");
  assert.deepEqual(texts(store, group), ["Preamp 3's polarity (Ø) is on, the same as Preamp 2's, so the copy is not inverted: left and right play the same thing."]);
  const plan = ok(repairPlan(store, Q, group, 0));
  assert.deepEqual(plan.lines, ["Switch Preamp 3's polarity (Ø) off"]);
  sent();
  await plan.apply();
  await settle();
  assert.deepEqual(sent().map((c) => [c.command, c.args]), [["set_pre_phase_inv", { id: 2, phase_inv: 0 }]], "the side preamp, which is recorded, is left as the person set it");
  assert.deepEqual(texts(store, group), []);

  // The second preamp's gain moved alone, and its link to the side preamp removed.
  store.links.removeMember("preamp", Q, 2);
  inputs.setGain(2, 12, false);
  await settle();
  assert.deepEqual(texts(store, group), ["Preamp 3 is at 12 dB and Preamp 2 at 30 dB, so the two side copies do not match.", "Preamp 3's gain no longer follows Preamp 2's."]);
  await ok(repairPlan(store, Q, group, 0)).apply();
  await settle();
  assert.equal(preamps[2]?.gain, 30);
  assert.deepEqual(store.links.linkOf("preamp", Q, 1)?.members.map((m) => m.channel), [1, 2]);
  assert.deepEqual(texts(store, group), []);
});

test("the guard: swapped inputs, a channel taken out of the mix, a channel removed, and a mono mix left alone", async () => {
  const { store, group, copy, routes, settle } = await decoded();
  const channels = store.channels(Q);

  await channels.setSource("m", { group: PREAMP, channel: 1 });
  await channels.setSource("s", { group: PREAMP, channel: 0 });
  assert.deepEqual(texts(store, group), ["Mid and Side have swapped inputs, so the side strips carry the mid microphone."]);
  const plan = ok(repairPlan(store, Q, group, 0));
  assert.deepEqual(plan.lines, ["Put Mid back on Preamp 1 and Side back on Preamp 2"]);
  await plan.apply();
  await settle();
  assert.deepEqual([routes[MIX_IN[0] as number]?.[6], routes[MIX_IN[0] as number]?.[7]], [{ source: PREAMP, channel: 0 }, { source: PREAMP, channel: 1 }]);
  assert.deepEqual(texts(store, group), []);

  await channels.setSource("s", { group: USB1, channel: 4 });
  assert.deepEqual(texts(store, group), ["Side is on USB 1 Play 5, not Preamp 2, which its inverted copy is made from."]);
  await ok(repairPlan(store, Q, group, 0)).apply();

  // Mono centres every pan and remembers where each returns to: the decode is still sound.
  channels.setMono(0, true);
  await settle();
  assert.deepEqual(texts(store, group), [], "a mono mix is not a broken decode");
  channels.setMono(0, false);
  await settle();
  assert.deepEqual(texts(store, group), []);

  await channels.setMainMix(copy.id, 1);
  assert.deepEqual(texts(store, group), ["Side Ø is not in Tracking, so the decode is incomplete here."]);
  await ok(repairPlan(store, Q, group, 0)).apply();
  await settle();
  assert.deepEqual(decodeOf(store, Q, group)?.inverted?.sends, [0]);

  await channels.remove("m");
  assert.deepEqual(texts(store, group), ["The mid channel has been removed, so nothing is decoded any more. Remove this decode to put the rest back."]);
  assert.equal(repairPlan(store, Q, group, 0).ok, false, "only removing it is left");
  assert.equal(removePlan(store, Q, group).ok, true);
});

test("removing a decode puts back the pans, the links, the second preamp's polarity and the groups, and takes the extra channel out", async () => {
  const pair = { id: "l0", kind: "mixer" as const, mode: "absolute" as const, members: [{ device_id: Q, channel: 6 }, { device_id: Q, channel: 7 }] };
  const { store, group, copy, strips, preamps, routes, sent, settle, layout } = await decoded({
    strips: { 6: { pan: 20 }, 7: { pan: 44 } },
    links: [pair],
    groups: [{ id: "g0", name: "Room", collapsed: false }],
    channels: [channel("m", "Mid", 6, 0, { group: "g0" }), channel("s", "Side", 7, 1, { group: "g0" })],
  });
  const plan = ok(removePlan(store, Q, group));
  assert.deepEqual(plan.lines, [
    "Pan Mid back to L 40% in Tracking",
    "Pan Side back to R 40% in Tracking",
    "Unlink Side and Side Ø",
    "Remove the channel Side Ø, the inverted copy",
    "Unlink Preamp 3 from Preamp 2",
    "Switch Preamp 3's polarity (Ø) back off",
    "Link Mid and Side again",
    'Remove the group "M/S: Mid"; its channels stay, and Mid goes back to the group Room, and Side goes back to the group Room',
  ]);
  await settle();
  assert.deepEqual(sent(), [], "nothing is sent until it is confirmed");
  assert.equal(layout().channels.length, 3, "and nothing is taken out");
  await plan.apply();
  await settle();
  assert.deepEqual([strips(0, 6).pan, strips(0, 7).pan], [20, 44]);
  assert.equal(preamps[2]?.phase, 0);
  assert.deepEqual(routes[MIX_IN[0] as number]?.[copy.slot], { source: MUTE, channel: 0 }, "the copy's slot is muted, as removing any channel leaves it");
  assert.deepEqual(layout().channels.map((c) => [c.id, c.group]), [["m", "g0"], ["s", "g0"]]);
  assert.deepEqual(layout().groups.map((g) => g.id), ["g0"]);
  assert.deepEqual(store.links.links.peek().map((l) => [l.kind, l.members.map((m) => m.channel)]), [["mixer", [6, 7]]], "Mid and Side are linked as they were, and nothing else is");
  assert.deepEqual(decodes(store, Q), []);
});

test("a decode can be played in another mix the same way, and removing it puts both mixes' pans back", async () => {
  const { store, group, copy, strips, settle, layout } = await decoded({ channels: [channel("m", "Mid", 6, 0, { sends: [1] }), channel("s", "Side", 7, 1)] });
  const cue = store.mixer(Q, 1);
  cue.setPan(6, 12);
  cue.setLevel(7, 15);
  await settle();
  assert.equal(strips(1, copy.slot).level, 15, "the link holds in every mix, so the copy's level there already matches");
  assert.deepEqual(texts(store, group, 1), [], "nothing is said about a mix it does not play in");
  const plan = ok(addToMixPlan(store, Q, group, 1));
  assert.deepEqual(plan.lines, [
    "Send Side to Cue too",
    "Send Side Ø to Cue too",
    "Pan Mid to the centre in Cue (it is at L 67%)",
    "Pan Side hard left in Cue (it is at C)",
    "Pan Side Ø hard right in Cue (it is at C)",
    "Remember Cue's pans, so removing the decode puts them back",
  ]);
  await plan.apply();
  await settle();
  assert.deepEqual([strips(1, 6).pan, strips(1, 7), strips(1, copy.slot)], [32, { level: 15, pan: 2, mute: false, solo: false }, { level: 15, pan: 62, mute: false, solo: false }]);
  assert.deepEqual(layout().groups[0]?.mid_side?.pans, { "0": { mid: 32, side: 32 }, "1": { mid: 12, side: 32 } });
  assert.deepEqual(texts(store, group, 1), []);
  assert.equal(addToMixPlan(store, Q, group, 1).ok, false, "it already plays there");
  assert.equal(width(store, Q, group, 1), -15);

  await ok(removePlan(store, Q, group)).apply();
  await settle();
  assert.deepEqual([strips(0, 6).pan, strips(0, 7).pan, strips(1, 6).pan, strips(1, 7).pan], [32, 32, 12, 32]);
});

test("the effect way: an empty chain fed by the side input, holding one effect with its polarity switch on, and said not to be checked", async () => {
  const { client, store, routes, strips, chains, settings, sent, settle, layout } = await bench({ chains: { 0: [[9, 0]] }, returns: true });
  const [mid, side] = layout().channels as [MixerChannel, MixerChannel];
  const offered = ways(store, Q, mid, side);
  assert.deepEqual(offered.preamps.map((p) => p.label), ["Preamp 3", "Preamp 4"], "every preamp but the mid's and the side's own");
  assert.deepEqual(offered.effect, { chain: 1, type: BAE, label: "BAE-1073 in AFX In 2" }, "AFX In 1 holds an effect, so the first empty chain is taken");

  const plan = ok(setupPlan(store, Q, 0, "m", "s", { via: "effect", chain: 1, type: BAE }));
  assert.deepEqual(plan.lines, [
    "Route Preamp 2 into AFX In 2 as well, in place of Mute",
    "Add BAE-1073 to AFX In 2, which is empty, with its polarity switch on and every other setting at its starting value",
    "Mute the effect return AFX Out 2 on slot 2 of Tracking, which would play the inverted copy a second time",
    "Pan Side hard left in Tracking (it is at C)",
    "Add a channel, Side Ø, on AFX Out 2 to Tracking: the inverted copy, panned hard right at Side's level, 0 dB",
    "Link Side and Side Ø, so their levels, mutes and solos always match",
    'Group the three as "M/S: Mid"',
  ]);
  assert.match(plan.notes[0] ?? "", /^Not checked on a device: whether an effect chain delays or colours what passes through it\./);
  assert.equal(plan.notes.some((note) => /one more input/.test(note)), false, "no extra recorded input this way");
  await settle();
  assert.deepEqual(sent(), [], "nothing is sent until it is confirmed");

  await plan.apply();
  await settle();
  assert.deepEqual(chains[1], [[BAE, 0]]);
  assert.deepEqual(settings[`${BAE}:0`], { gain: 0, high_shelf: 8, peak_freq: 0, peak_gain: 8, low_freq: 0, low_gain: 8, high_pass: 0, phase_inv: 1 }, "flat, with the polarity switch on");
  assert.deepEqual(routes[AFX_IN]?.[1], { source: PREAMP, channel: 1 });
  assert.equal(strips(0, 1).mute, true, "its effect return is muted in this mix");
  const decode = layout().groups[0]?.mid_side;
  assert.deepEqual({ via: decode?.via, chain: decode?.chain, type: decode?.effect_type, inst: decode?.effect_inst, input: decode?.chain_input, returns: decode?.returns_muted }, { via: "effect", chain: 1, type: BAE, inst: 0, input: undefined, returns: [0] });
  const copy = layout().channels.find((c) => c.name === "Side Ø");
  assert.deepEqual(copy?.source, { group: AFX_OUT, channel: 1 });
  const group = layout().groups[0]?.id ?? "";
  assert.deepEqual(texts(store, group), []);
  assert.equal(client.invocations.some((c) => c.command.startsWith("set_pre")), false, "no preamp is touched");
  sent();

  // The guard: the effect bypassed, its polarity switch off, the chain rerouted, its return unmuted.
  const effects = store.effects(Q);
  effects.setBypass(1, 0, true);
  effects.setParameter(1, 0, "phase_inv", 0);
  await store.routing(Q).route(AFX_IN, 1, { source: PREAMP, channel: 3 });
  await store.mixer(Q, 0).setAlone(1, { mute: false });
  await settle();
  assert.deepEqual(texts(store, group), [
    "BAE-1073 in AFX In 2 is bypassed, so the copy is not inverted.",
    "BAE-1073's polarity switch in AFX In 2 is off, so the copy is not inverted.",
    "AFX In 2 takes Preamp 4, not Preamp 2, so the copy is not the side signal.",
    "The effect return AFX Out 2 also plays in Tracking, so the inverted copy is heard twice.",
  ]);
  await ok(repairPlan(store, Q, group, 0)).apply();
  await settle();
  assert.deepEqual(texts(store, group), []);
  assert.equal(settings[`${BAE}:0`]?.["phase_inv"], 1);

  // The effect taken out of the chain: an empty chain passes the side signal straight through.
  effects.removeEffect(1, 0);
  await settle();
  assert.deepEqual(texts(store, group), ["AFX In 2 is empty, not the BAE-1073 that inverts the copy, so the copy is not inverted."]);
  const mend = ok(repairPlan(store, Q, group, 0));
  assert.deepEqual(mend.lines, ["Add BAE-1073 to AFX In 2 again with its polarity switch on"]);
  await mend.apply();
  await settle();
  assert.deepEqual(chains[1], [[BAE, 0]]);
  assert.deepEqual(texts(store, group), []);

  const remove = ok(removePlan(store, Q, group));
  assert.deepEqual(remove.lines, [
    "Pan Side back to centre in Tracking",
    "Unlink Side and Side Ø",
    "Remove the channel Side Ø, the inverted copy",
    "Remove BAE-1073 from AFX In 2",
    "Route Mute into AFX In 2 again",
    "Unmute the effect return AFX Out 2 in Tracking",
    'Remove the group "M/S: Mid"; its channels stay',
  ]);
  await remove.apply();
  await settle();
  assert.deepEqual(chains[1], []);
  assert.deepEqual(routes[AFX_IN]?.[1], { source: MUTE, channel: 0 });
  assert.equal(strips(0, 1).mute, false);
  assert.deepEqual(layout().groups, []);
  assert.deepEqual(layout().channels.map((c) => c.id), ["m", "s"]);
});

test("the effect way is not offered without an empty, unused chain, or before the chains are read, and says why", async () => {
  const full = await bench({ chains: Object.fromEntries([0, 1, 2, 3, 4, 5].map((k) => [k, [[9, k]] as [number, number][]])) });
  const [mid, side] = full.layout().channels as [MixerChannel, MixerChannel];
  const offered = ways(full.store, Q, mid, side).effect;
  assert.match("why" in offered ? offered.why : "", /^Every effect chain holds an effect/);
  const refused = setupPlan(full.store, Q, 0, "m", "s", { via: "effect", chain: 1, type: BAE });
  assert.equal(refused.ok, false, "and a plan through one is refused rather than made on a guess");

  const dry = await bench({ dryRun: true });
  const [dryMid, drySide] = dry.layout().channels as [MixerChannel, MixerChannel];
  const unread = ways(dry.store, Q, dryMid, drySide).effect;
  assert.match("why" in unread ? unread.why : "", /have not been read/);
  // The second preamp still works there, and says the gain could not be matched.
  const plan = ok(setupPlan(dry.store, Q, 0, "m", "s", { via: "preamp", preamp: 3 }));
  assert.match(plan.notes[0] ?? "", /have not been reported yet, so Preamp 4's gain is not matched here/);
  assert.ok(plan.lines.includes("Switch Preamp 4's polarity (Ø) on"));
});

test("an effect whose settings cannot be read is taken out again, and nothing else is changed", async () => {
  const { client, store, chains, routes, strips, settle, layout } = await bench({ strips: { 6: { pan: 20 } } });
  const respond = client.respond;
  client.respond = async (call) => (call.command === "get_neve_1073_conf" ? { device_id: call.deviceId, command: call.command, sent_hex: "74", sent_len: 1, dry_run: false, response: null, response_error: "timeout" } : respond(call));
  const plan = ok(setupPlan(store, Q, 0, "m", "s", { via: "effect", chain: 0, type: BAE }));
  await assert.rejects(plan.apply(), (error: unknown) => {
    assert.ok(error instanceof UndoneError, "it says nothing was left behind");
    assert.equal(error.message, "BAE-1073's settings could not be read, so its polarity switch could not be set. The chain is as it was, and nothing else was changed.");
    return true;
  });
  await settle();
  assert.deepEqual(chains[0], [], "a chain is never left holding an effect that does not invert");
  assert.deepEqual(routes[AFX_IN]?.[0], { source: MUTE, channel: 0 }, "and the side input is routed out of it again");
  assert.equal(strips(0, 6).pan, 20, "the pans come after it, so they were not moved");
  assert.deepEqual(layout().groups, []);
  assert.deepEqual(layout().channels.map((c) => c.id), ["m", "s"]);
});

test("two channels that cannot be a mid and a side are refused, with the reason", async () => {
  const { store, group } = await decoded({ channels: [channel("m", "Mid", 6, 0), channel("s", "Side", 7, 1), channel("x", "Other", 9, 0), channel("y", "Cue only", 10, 3, { main_mix: 1 })] });
  const why = (mid: string, side: string, preamp = 3) => {
    const planned = setupPlan(store, Q, 0, mid, side, { via: "preamp", preamp });
    return planned.ok ? "planned" : planned.why;
  };
  assert.match(why("m", "x"), /^M\/S: Mid already uses one of these channels/);
  assert.match(why("x", "x"), /^Select two channels/);
  assert.match(why("x", "y"), /need an input and a place in this mix/);
  await store.channels(Q).setMainMix("y", 0);
  assert.match(why("x", "y", 0), /Choose a preamp other than the mid's and the side's own/);
  assert.equal(why("x", "y", 1), "planned");
  await store.channels(Q).setSource("y", { group: PREAMP, channel: 0 });
  assert.match(why("x", "y", 1), /same input/);
  assert.deepEqual(texts(store, group), []);
});

test("a saved layout carries the group but not the decode, which is this device's own setup", async () => {
  const { store } = await decoded();
  const channels = store.channels(Q);
  const id = channels.saveLayout("With M/S");
  const saved = channels.savedLayouts().find((l) => l.id === id);
  assert.deepEqual(saved?.mixer.groups.map((g) => [g.name, g.mid_side]), [["M/S: Mid", undefined]]);
  assert.notEqual(channels.layout.peek().groups[0]?.mid_side, undefined, "the live layout keeps it");
});
