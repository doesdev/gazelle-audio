import { test } from "node:test";
import assert from "node:assert/strict";

import type { AggregateFix, Cable, Workspace } from "gazelle-audio-client";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { fixRequest } from "../src/store/aggregate.ts";
import {
  applyDedication,
  breaks,
  clockAdvice,
  dedicatedPaths,
  eligibility,
  pathMarks,
  phaseGuard,
  phasePathContext,
  planDedication,
  reasonPage,
  restorePath,
  restoreWrites,
  routingGroups,
  staleness,
  storeRouteWriter,
  withoutDedication,
  type DedicationPlan,
} from "../src/store/phase-path.ts";
import { ROUTING_SLOTS } from "../src/store/routing.ts";
import { Store } from "../src/store/store.ts";
import { builtInThemes, device, FakeClient, MemoryStorage, type Invocation } from "./fake-client.ts";

// Quadro: sources Preamp 0, USB 1 Play 1, SPDIF IN 4, MIX1 OUT 6, MUTE 10; destinations HP1 1,
// MONITOR 3, SPDIF_OUT 6, MIX CH1..4 8..11. Studio+: sources Preamp 0, SPDIF IN 5, MUTE 11;
// destinations USB Rec 6, SPDIF_OUT 8.
const QUADRO = "loopback-0";
const STUDIO = "loopback-1";
const COM_PLAY = 1;
const MIX1_OUT = 6;
const Q_MUTE = 10;
const HP1 = 1;
const MONITOR = 3;
const SPDIF_OUT = 6;
const MIX_IN = [8, 9, 10, 11] as const;
const S_SPDIF_IN = 5;
const S_MUTE = 11;
const S_USB_REC = 6;

type Pair = [number, number];

const spdif: Cable = { id: "c1", from: { device_id: QUADRO, port: "SPDIF_OUT", first: 0 }, to: { device_id: STUDIO, port: "SPDIF_IN", first: 0 }, channels: 2 };

/** The owner's setup: the Quadro drives the callback, the Studio+'s phase is measured over USB 1 Play 3 and USB Rec 21. */
function ownersWorkspace(parts: Partial<Workspace> = {}): Workspace {
  return {
    version: 1,
    groups: [],
    links: [],
    aliases: { [QUADRO]: "Quadro", [STUDIO]: "Studio+" },
    mixers: {},
    cables: [spdif],
    aggregate: {
      devices: [
        { key: "Zen Quadro Synergy Core", device_id: QUADRO },
        { key: "Zen Studio+", device_id: STUDIO, phase: { master_output: 2, input: 20, reference: -37 } },
      ],
    },
    ...parts,
  };
}

/**
 * Two fake interfaces that keep routing as a device does, one 32 slot group per destination, and a
 * store over them. The Quadro plays USB 1 Play 1 and 2 into Mix 1, USB 1 Play 3 into Mix 1 and Mix 4
 * for a headphone amp, Mix 1 to HP1, and its S/PDIF output is muted; the Studio+ records S/PDIF In L
 * on USB Rec 21.
 */
async function setup(workspace = ownersWorkspace()) {
  const client = new FakeClient(device(QUADRO, "quadro", "Zen Quadro"), device(STUDIO, "studio", "Zen Studio+"));
  const groups = new Map<string, Pair[]>();
  const mute = (id: string) => (id === QUADRO ? Q_MUTE : S_MUTE);
  const slotsOf = (id: string, group: number): Pair[] => groups.get(`${id}|${group}`) ?? Array.from({ length: ROUTING_SLOTS }, (): Pair => [mute(id), 0]);
  const set = (id: string, group: number, slots: Record<number, Pair>) => groups.set(`${id}|${group}`, slotsOf(id, group).map((pair, at) => slots[at] ?? pair));
  set(QUADRO, MIX_IN[0], { 16: [COM_PLAY, 0], 17: [COM_PLAY, 1], 4: [COM_PLAY, 2] });
  set(QUADRO, MIX_IN[3], { 9: [COM_PLAY, 2] });
  set(QUADRO, HP1, { 0: [MIX1_OUT, 0], 1: [MIX1_OUT, 1] });
  set(STUDIO, S_USB_REC, { 20: [S_SPDIF_IN, 0] });
  client.respond = async (call: Invocation) => {
    const envelope = (response: unknown) => ({ device_id: call.deviceId, command: call.command, sent_hex: "74", sent_len: 16, dry_run: false, response, response_error: null });
    if (call.command === "get_routing") {
      const group = call.options?.["ext3"] as number;
      return envelope({ bank_idx: group, bank_configs: slotsOf(call.deviceId, group).map(([p, c]) => ({ in_periph_id: p, in_chann: c })) });
    }
    if (call.command === "set_routing") {
      const args = call.args as { bank_idx: number; bank_configs: Uint8Array[] };
      groups.set(`${call.deviceId}|${args.bank_idx}`, args.bank_configs.map((b): Pair => [b[0] as number, b[1] as number]));
    }
    return envelope(null);
  };
  client.stored = workspace;
  client.server = { ...client.server, dry_run: false };
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), themeSources: builtInThemes });
  await store.start();
  await store.readRoutes(QUADRO);
  await store.readRoutes(STUDIO);
  client.invocations.length = 0;
  const writes = () =>
    client.invocations.filter((c) => c.command === "set_routing").map((c) => {
      const args = c.args as { bank_idx: number; bank_configs: Uint8Array[] };
      return { device: c.deviceId, group: args.bank_idx, slots: args.bank_configs.map((b): Pair => [b[0] as number, b[1] as number]) };
    });
  return { client, store, set, slotsOf, writes };
}

const planned = (result: ReturnType<typeof planDedication>): DedicationPlan => {
  assert.ok(result.ok, result.ok ? "" : result.why);
  return result.plan;
};

const pairs = (length: number, fill: Pair, at: Record<number, Pair>): Pair[] => Array.from({ length }, (_, i): Pair => at[i] ?? fill);

test("only a digital cable from the callback master to a follower, both in the aggregate and connected, can be dedicated", async () => {
  const { store } = await setup();
  const context = () => phasePathContext(store);
  assert.deepEqual(eligibility(spdif, context()), { ok: true, roles: { masterId: QUADRO, followerId: STUDIO, masterIndex: 0, followerIndex: 1 } });

  const back: Cable = { id: "c2", from: { device_id: STUDIO, port: "SPDIF_OUT", first: 0 }, to: { device_id: QUADRO, port: "SPDIF_IN", first: 0 }, channels: 2 };
  const refused = eligibility(back, context());
  assert.ok(!refused.ok);
  assert.equal(refused.why, "This cable runs into Quadro, which drives the callback. A phase is measured from the callback master to another interface, so only a cable leaving Quadro can carry it.");

  store.editAggregate((aggregate) => ({ ...aggregate, callback_master: "Zen Studio+" }));
  const notMaster = eligibility(spdif, context());
  assert.ok(!notMaster.ok);
  assert.equal(notMaster.why, "This cable runs into Studio+, which drives the callback. A phase is measured from the callback master to another interface, so only a cable leaving Studio+ can carry it.");

  store.editAggregate((aggregate) => ({ devices: [aggregate.devices?.[0] ?? {}] }));
  const alone = eligibility(spdif, context());
  assert.ok(!alone.ok);
  assert.match(alone.why, /^Only a cable between two interfaces of the aggregate/);

  store.editAggregate((aggregate) => ({ devices: [...(aggregate.devices ?? []), { key: "Other", device_id: "usb:gone" }] }));
  const outside = eligibility(spdif, context());
  assert.ok(!outside.ok);
  assert.equal(outside.why, "Studio+ is not in the aggregate, so this cable carries nothing the aggregate measures.");
});

/** The owner's PC: USB 1 Play 16 is the highest nothing uses, and the Studio+ already records the cable. */
test("dedicating picks the highest free playback channel, keeps the record channel that already records the cable, and says every change", async () => {
  const { store } = await setup();
  const plan = planned(planDedication(spdif, phasePathContext(store)));
  assert.equal(plan.output, 15, "USB 1 Play 16: 1 to 3 are in Mix 1 and Mix 4");
  assert.equal(plan.input, 20, "USB Rec 21 records S/PDIF In L already");
  assert.deepEqual(plan.writes, [{ deviceId: QUADRO, destination: SPDIF_OUT, changes: [{ channel: 0, source: { source: COM_PLAY, channel: 15 } }] }], "the S/PDIF output's right side is muted already, so only its left changes");
  assert.deepEqual(plan.phase, { master_output: 15, input: 20 }, "a new path takes no reference from the old one");
  assert.equal(plan.referenceCleared, -37);
  assert.deepEqual(plan.lines, [
    "On Quadro, S/PDIF Out L plays USB 1 Play 16 directly, with no mix between (it is muted now).",
    "On Studio+, USB Rec 21 already records S/PDIF In L, so it stays as it is.",
    "In the aggregate's setup, Studio+'s phase is measured from Quadro's USB 1 Play 16 to its USB Rec 21 (it was USB 1 Play 3 to USB Rec 21).",
    "Its phase reference, -37 samples, is taken out, because it was measured over the old path: measure the interfaces again under Line the interfaces up on the Aggregate page to give it one.",
    "USB 1 Play 16 and USB Rec 21 are kept for the phase measurement and hidden from your DAW. USB 1 Play 3 is given back to it.",
  ]);
});

test("a playback channel a Mixer channel is laid out on, or another interface's phase uses, is not free", async () => {
  const workspace = ownersWorkspace({ mixers: { [QUADRO]: { mixes: [], groups: [], channels: [{ id: "x", name: "", slot: 3, source: { group: COM_PLAY, channel: 15 }, main_mix: 0, sends: [] }] } } });
  workspace.aggregate?.devices?.push({ key: "Third", device_id: "usb:third", phase: { master_output: 14, input: 0 } });
  const { store } = await setup(workspace);
  assert.equal(planned(planDedication(spdif, phasePathContext(store))).output, 13, "16 is laid out on the Mixer and 15 is the third's");
});

test("the S/PDIF output's other side is muted, and a follower that records nothing of the cable gets its highest free record channel", async () => {
  const { store, set } = await setup();
  set(QUADRO, SPDIF_OUT, { 0: [MIX1_OUT, 0], 1: [MIX1_OUT, 1] });
  set(STUDIO, S_USB_REC, { 20: [0, 0], 23: [0, 1] });
  await store.routing(QUADRO).load(SPDIF_OUT);
  await store.routing(STUDIO).load(S_USB_REC);
  const plan = planned(planDedication(spdif, phasePathContext(store)));
  assert.equal(plan.input, 22, "USB Rec 24 records Preamp 2, so USB Rec 23 is the highest free");
  assert.deepEqual(plan.writes, [
    { deviceId: QUADRO, destination: SPDIF_OUT, changes: [{ channel: 0, source: { source: COM_PLAY, channel: 15 } }, { channel: 1, source: null }] },
    { deviceId: STUDIO, destination: S_USB_REC, changes: [{ channel: 22, source: { source: S_SPDIF_IN, channel: 0 } }] },
  ]);
  assert.deepEqual(plan.lines.slice(0, 3), [
    "On Quadro, S/PDIF Out L plays USB 1 Play 16 directly, with no mix between (it plays Mix 1 L now).",
    "On Quadro, S/PDIF Out R is muted (it plays Mix 1 R now), so the cable carries the measurement and its clock and nothing else. S/PDIF carries its clock whatever it plays.",
    "On Studio+, USB Rec 23 records S/PDIF In L (it records nothing now).",
  ]);
});

test("nothing is planned before the routing is read, and a master with no free playback channel says so", async () => {
  const { store } = await setup();
  const context = phasePathContext(store);
  const unread = { ...context, routing: (id: string, destination: number) => (id === QUADRO && destination === MONITOR ? undefined : context.routing(id, destination)) };
  const result = planDedication(spdif, unread);
  assert.ok(!result.ok);
  assert.equal(result.why, "The routing of Quadro has not been read, so which channels are free is not known. Read it on the Routing page, and try again.");

  const everyChannelUsed = { ...context, routing: (id: string, destination: number) => (id === QUADRO && destination === MIX_IN[2] ? Array.from({ length: 32 }, (_, at) => ({ source: COM_PLAY, channel: at % 16 })) : context.routing(id, destination)) };
  const full = planDedication(spdif, everyChannelUsed);
  assert.ok(!full.ok);
  assert.equal(full.why, "Every USB playback channel of Quadro goes somewhere already, so none is free to keep for the phase measurement.");
});

/** The bytes: one read and one write of the S/PDIF group, every other slot as the device had it, and the workspace written once. */
test("dedicating writes exactly the planned routing, one set_routing per group read first, then the dedication and the phase setup together", async () => {
  const { client, store, set, writes } = await setup();
  set(QUADRO, SPDIF_OUT, { 1: [MIX1_OUT, 1] });
  set(STUDIO, S_USB_REC, { 20: [S_MUTE, 0], 3: [0, 3] });
  await store.routing(QUADRO).load(SPDIF_OUT);
  await store.routing(STUDIO).load(S_USB_REC);
  client.invocations.length = 0;
  const plan = planned(planDedication(spdif, phasePathContext(store)));
  const done = await applyDedication(plan, { route: storeRouteWriter(store), editWorkspace: (update) => store.editWorkspace(update), deviceName: (id) => id });
  assert.deepEqual(done, { ok: true, text: "Dedicated to the phase measurement and the clock. Measure the interfaces again on the Aggregate page to give the phase a reference." });
  assert.deepEqual(
    client.invocations.map((call) => [call.deviceId, call.command, call.options?.["ext3"] ?? (call.args as { bank_idx?: number } | undefined)?.bank_idx]),
    [
      [QUADRO, "get_routing", SPDIF_OUT],
      [QUADRO, "set_routing", SPDIF_OUT],
      [STUDIO, "get_routing", S_USB_REC],
      [STUDIO, "set_routing", S_USB_REC],
    ],
  );
  assert.deepEqual(writes(), [
    { device: QUADRO, group: SPDIF_OUT, slots: pairs(32, [Q_MUTE, 0], { 0: [COM_PLAY, 15] }) },
    { device: STUDIO, group: S_USB_REC, slots: pairs(32, [S_MUTE, 0], { 3: [0, 3], 23: [S_SPDIF_IN, 0] }) },
  ]);
  const saved = store.workspace.value;
  assert.deepEqual(saved?.cables?.[0]?.dedicated, { phase_output: 15, phase_input: 23 });
  assert.deepEqual(saved?.aggregate?.devices?.[1]?.phase, { master_output: 15, input: 23 }, "the reference is gone with the old path");
  assert.equal(staleness(saved?.cables?.[0] as Cable, saved, (id) => id), undefined, "and it means something");

  // Dedicating again changes nothing: the path is there and the channel goes nowhere else.
  client.invocations.length = 0;
  const again = planned(planDedication(saved?.cables?.[0] as Cable, phasePathContext(store)));
  assert.equal(again.output, 15);
  assert.equal(again.input, 23);
  assert.deepEqual(again.writes, []);
  assert.equal(again.referenceCleared, undefined);
  assert.equal(again.lines[0], "On Quadro, S/PDIF Out L already plays USB 1 Play 16 and nothing else does, so it stays as it is.");
});

test("a write that is not sent stops it before the workspace says anything", async () => {
  const { client, store } = await setup();
  const plan = planned(planDedication(spdif, phasePathContext(store)));
  client.respond = async (call) => {
    throw new Error(`${call.command} refused`);
  };
  const done = await applyDedication(plan, { route: storeRouteWriter(store), editWorkspace: (update) => store.editWorkspace(update), deviceName: (id) => (id === QUADRO ? "Quadro" : id) });
  assert.equal(done.ok, false);
  assert.equal(done.text, "The cable was not dedicated: the routing on Quadro could not be written. Nothing was changed.");
  assert.equal(store.workspace.value?.cables?.[0]?.dedicated, undefined);
  assert.deepEqual(store.workspace.value?.aggregate?.devices?.[1]?.phase, { master_output: 2, input: 20, reference: -37 });
});

test("turning a dedication off keeps the routing and the phase setup, or clears the phase setup when asked", () => {
  const workspace = ownersWorkspace({ cables: [{ ...spdif, dedicated: { phase_output: 15, phase_input: 20 } }] });
  const kept = withoutDedication(workspace, "c1", false);
  assert.equal(kept.cables?.[0]?.dedicated, undefined);
  assert.equal("dedicated" in (kept.cables?.[0] ?? {}), false);
  assert.deepEqual(kept.aggregate?.devices?.[1]?.phase, { master_output: 2, input: 20, reference: -37 });
  const cleared = withoutDedication(workspace, "c1", true);
  assert.equal(cleared.aggregate?.devices?.[1]?.phase, undefined);
  assert.equal(withoutDedication(workspace, "nothing", true), workspace);
});

test("a dedication means nothing once its sender stops driving the callback, a device leaves, or the phase setup moves", () => {
  const dedicated = { ...spdif, dedicated: { phase_output: 15, phase_input: 20 } };
  const workspace = ownersWorkspace({ cables: [dedicated] });
  (workspace.aggregate?.devices?.[1] as { phase?: unknown }).phase = { master_output: 15, input: 20 };
  const names = (id: string) => (id === QUADRO ? "Quadro" : "Studio+");
  assert.equal(staleness(dedicated, workspace, names), undefined);
  assert.equal(
    staleness(dedicated, { ...workspace, aggregate: { ...workspace.aggregate, callback_master: "Zen Studio+" } }, names),
    "Quadro does not drive the callback now, and a phase is measured from the interface that does, so the dedication means nothing now: its routing stays as it is and nothing guards it. Turn it off to release its channels, or dedicate it again.",
  );
  assert.match(staleness(dedicated, { ...workspace, aggregate: { devices: [workspace.aggregate?.devices?.[0] ?? {}] } }, names) ?? "", /^Studio\+ is not in the aggregate now/);
  const moved = structuredClone(workspace);
  (moved.aggregate?.devices?.[1] as { phase?: unknown }).phase = { master_output: 2, input: 20 };
  assert.match(staleness(dedicated, moved, names) ?? "", /^Studio\+'s phase setup names other channels now/);
});

test("the Routing page's guard names the cable for each change that would break the path, and nothing else", async () => {
  const { store } = await setup();
  const topology = (id: string) => store.topology(id);
  const paths = dedicatedPaths([{ ...spdif, dedicated: { phase_output: 15, phase_input: 20 } }], { topology, deviceName: (id) => (id === QUADRO ? "Quadro" : "Studio+") });
  assert.equal(paths.length, 1);
  const quadro = store.topology(QUADRO)!;
  const studio = store.topology(STUDIO)!;
  const cable = "the dedicated S/PDIF cable, Quadro S/PDIF Out 1 and 2 → Studio+ S/PDIF In 1 and 2";
  assert.deepEqual(breaks(paths, QUADRO, SPDIF_OUT, [{ channel: 0, source: { source: MIX1_OUT, channel: 0 } }], Q_MUTE, quadro), [`S/PDIF Out L would stop playing USB 1 Play 16, so the phase measurement over ${cable} would hear nothing.`]);
  assert.deepEqual(breaks(paths, QUADRO, SPDIF_OUT, [{ channel: 0, source: null }, { channel: 1, source: null }], Q_MUTE, quadro), [`S/PDIF Out L would stop playing USB 1 Play 16, so the phase measurement over ${cable} would hear nothing.`], "muting the row");
  assert.deepEqual(breaks(paths, QUADRO, MONITOR, [{ channel: 0, source: { source: COM_PLAY, channel: 15 } }], Q_MUTE, quadro), [
    `USB 1 Play 16 would play to Monitor L as well, and the burst the driver plays into it at the start of every session with it. It is kept for the phase measurement over ${cable}.`,
  ]);
  assert.deepEqual(breaks(paths, STUDIO, S_USB_REC, [{ channel: 20, source: { source: 0, channel: 0 } }], S_MUTE, studio), [`USB Rec 21 would stop recording S/PDIF In L, so the phase measurement over ${cable} would hear nothing.`]);
  assert.deepEqual(breaks(paths, QUADRO, SPDIF_OUT, [{ channel: 1, source: { source: MIX1_OUT, channel: 1 } }], Q_MUTE, quadro), [], "the right side is not the path");
  assert.deepEqual(breaks(paths, QUADRO, SPDIF_OUT, [{ channel: 0, source: { source: COM_PLAY, channel: 15 } }], Q_MUTE, quadro), [], "putting it back is not breaking it");
  assert.deepEqual(breaks(paths, STUDIO, S_USB_REC, [{ channel: 19, source: null }], S_MUTE, studio), []);

  const marks = pathMarks(paths, QUADRO);
  assert.deepEqual([...marks.destinations.keys()], [`${SPDIF_OUT}:0`]);
  assert.deepEqual([...marks.sources.keys()], [`${COM_PLAY}:15`]);
  assert.match(marks.sources.get(`${COM_PLAY}:15`) ?? "", /hidden from your DAW\.$/);
  assert.deepEqual([...pathMarks(paths, STUDIO).destinations.keys()], [`${S_USB_REC}:20`]);
});

test("the Mixer's guard marks the kept channels and outputs, and says what a Mixer change would break, in the Routing page's words", async () => {
  const dedicated = { ...spdif, dedicated: { phase_output: 15, phase_input: 20 } };
  const { store } = await setup(ownersWorkspace({ cables: [dedicated], mixers: { [QUADRO]: { mixes: [{ name: "Monitors" }, { name: "Cue" }], groups: [], channels: [] } } }));
  const guard = phaseGuard(store);
  const cable = "the dedicated S/PDIF cable, Quadro S/PDIF Out 1 and 2 → Studio+ S/PDIF In 1 and 2";

  // The marks: the kept playback channel as a channel's input, the cable's output and the record channel as output pairs.
  assert.match(guard.source(QUADRO, { group: COM_PLAY, channel: 15 }) ?? "", /^Kept for the phase measurement over the dedicated S\/PDIF cable/);
  assert.equal(guard.source(QUADRO, { group: COM_PLAY, channel: 14 }), undefined);
  assert.equal(guard.source(QUADRO, undefined), undefined);
  assert.equal(guard.source(STUDIO, { group: S_SPDIF_IN, channel: 0 }), undefined, "the arriving signal is not kept: anything may record it as well");
  assert.match(guard.output(QUADRO, SPDIF_OUT, 0) ?? "", /it plays USB 1 Play 16 and nothing else\.$/);
  assert.match(guard.output(STUDIO, S_USB_REC, 20) ?? "", /it records S\/PDIF In L, and is hidden from your DAW\.$/, "the pair holding the record channel");
  assert.equal(guard.output(STUDIO, S_USB_REC, 18), undefined);
  assert.equal(guard.output(QUADRO, MONITOR, 0), undefined);

  // A channel on the kept playback channel would route it into its mixes, named as the Mixer names them.
  const playsToo = (mix: string) => `USB 1 Play 16 would play to ${mix} as well, and the burst the driver plays into it at the start of every session with it. It is kept for the phase measurement over ${cable}.`;
  assert.deepEqual(guard.feeding(QUADRO, { group: COM_PLAY, channel: 15 }, 6, [0, 1]), [playsToo("Monitors"), playsToo("Cue")]);
  assert.deepEqual(guard.feeding(QUADRO, { group: COM_PLAY, channel: 14 }, 6, [0, 1]), []);
  assert.deepEqual(guard.feeding(QUADRO, { group: COM_PLAY, channel: 15 }, 6, []), [], "a channel in no mix routes nothing");
  assert.deepEqual(guard.feeding(QUADRO, undefined, 6, [0]), []);

  // A mix sent to the cable's output, or to the pair holding the record channel, takes it from the path.
  assert.deepEqual(guard.mixOutput(QUADRO, 0, { destination: SPDIF_OUT, channel: 0 }), [`S/PDIF Out L would stop playing USB 1 Play 16, so the phase measurement over ${cable} would hear nothing.`]);
  assert.deepEqual(guard.mixOutput(STUDIO, 0, { destination: S_USB_REC, channel: 20 }), [`USB Rec 21 would stop recording S/PDIF In L, so the phase measurement over ${cable} would hear nothing.`]);
  assert.deepEqual(guard.mixOutput(QUADRO, 0, { destination: MONITOR, channel: 0 }), []);
  assert.deepEqual(guard.mixOutput(STUDIO, 0, { destination: S_USB_REC, channel: 18 }), []);

  // Tidy's routes: putting the kept channel back on a mix slot breaks it, muting the slot does not.
  assert.deepEqual(guard.routes(QUADRO, MIX_IN[0], [{ channel: 4, source: { source: COM_PLAY, channel: 15 } }]), [playsToo("Monitors")]);
  assert.deepEqual(guard.routes(QUADRO, MIX_IN[0], [{ channel: 4, source: null }]), []);

  // Starting from a layout: a saved one with a channel on the kept playback channel, and a starting
  // layout that does not use it.
  store.editWorkspace((workspace) => ({
    ...workspace,
    layouts: [{ id: "kept", name: "Kept", family: "quadro", mixer: { mixes: [], groups: [], channels: [{ id: "a", name: "", slot: 9, source: { group: COM_PLAY, channel: 15 }, main_mix: 1, sends: [] }] } }],
  }));
  assert.deepEqual(guard.start(QUADRO, "saved:kept"), [playsToo("Cue")]);
  assert.deepEqual(guard.start(QUADRO, "tracking"), [], "the Tracking layout plays USB 1 Play 1 and 2");
  assert.deepEqual(guard.start(QUADRO, "saved:gone"), []);

  // With USB 1 Play 1 kept instead, the Tracking layout's DAW L would route it into both its mixes.
  store.editWorkspace((workspace) => ({ ...workspace, cables: [{ ...spdif, dedicated: { phase_output: 0, phase_input: 20 } }] }));
  assert.equal(guard.start(QUADRO, "tracking").length, 2);
  assert.match(guard.start(QUADRO, "tracking")[0] ?? "", /^USB 1 Play 1 would play to Monitors as well/);

  // Without a dedicated cable there is nothing to mark or ask.
  store.editWorkspace((workspace) => ({ ...workspace, cables: [spdif] }));
  assert.equal(guard.source(QUADRO, { group: COM_PLAY, channel: 0 }), undefined);
  assert.equal(guard.output(QUADRO, SPDIF_OUT, 0), undefined);
  assert.deepEqual(guard.mixOutput(QUADRO, 0, { destination: SPDIF_OUT, channel: 0 }), []);
  assert.deepEqual(guard.start(QUADRO, "tracking"), []);
});

test("the receiver's clock is offered its fix only while it does not follow the cable", () => {
  const context = (source: number) => ({
    workspace: undefined,
    topology: () => undefined,
    routing: () => undefined,
    deviceName: () => "Studio+",
    clock: () => ({ source, sources: ["Oven", "Word clock", "ADAT", "ADAT x2", "ADAT x4", "S/PDIF", "USB"] }),
  });
  assert.deepEqual(clockAdvice(spdif, context(0)), { deviceId: STUDIO, index: 5, name: "S/PDIF", now: "Oven", label: "Put Studio+ on S/PDIF" });
  assert.equal(clockAdvice(spdif, context(5)), undefined);
});

/** The readiness fix the server offers for a broken dedicated path, made through the routing model. */
test("the fix that puts a dedicated path back writes each group once, read first, through the routing model", async () => {
  const { store, set, writes } = await setup();
  // Someone routed Mix 1 to S/PDIF Out L, USB 1 Play 16 to the monitors, and a preamp to USB Rec 21.
  set(QUADRO, SPDIF_OUT, { 0: [MIX1_OUT, 0], 1: [MIX1_OUT, 1] });
  set(QUADRO, MONITOR, { 0: [MIX1_OUT, 0], 1: [COM_PLAY, 15] });
  set(STUDIO, S_USB_REC, { 20: [0, 0] });
  const fix: AggregateFix = {
    kind: "restore_phase_path",
    method: "PUT",
    route: "routing",
    label: "Put the phase path back",
    body: {
      writes: [
        { device_id: QUADRO, destination: SPDIF_OUT, channel: 0, source: [COM_PLAY, 15] },
        { device_id: QUADRO, destination: MONITOR, channel: 1, source: null },
        { device_id: STUDIO, destination: S_USB_REC, channel: 20, source: [S_SPDIF_IN, 0] },
      ],
    },
  };
  const request = restoreWrites(fix);
  assert.ok(request !== undefined);
  assert.equal(fixRequest(fix), undefined, "the model sends nothing for it: it is routing, which the page writes");
  assert.deepEqual(routingGroups(request).map((group) => [group.deviceId, group.destination, group.changes.length]), [
    [QUADRO, SPDIF_OUT, 1],
    [QUADRO, MONITOR, 1],
    [STUDIO, S_USB_REC, 1],
  ]);
  assert.equal(restoreWrites({ ...fix, body: { writes: [{ device_id: QUADRO, destination: -1, channel: 0, source: null }] } }), undefined, "a write that is not one is no fix");
  assert.equal(restoreWrites({ ...fix, kind: "set_setup_rate" }), undefined);
  assert.equal(reasonPage({ code: "phase_path_broken", severity: "warning", message: "", fix }), undefined, "a dedicated path is put back here");
  assert.equal(reasonPage({ code: "phase_path_broken", severity: "warning", message: "" }), "workspace", "one that is not is dedicated there");
  assert.equal(reasonPage({ code: "phase_dedication_stale", severity: "warning", message: "" }), "workspace");

  await store.aggregate.perform(() => restorePath(request, storeRouteWriter(store)));
  assert.deepEqual(writes(), [
    { device: QUADRO, group: SPDIF_OUT, slots: pairs(32, [Q_MUTE, 0], { 0: [COM_PLAY, 15], 1: [MIX1_OUT, 1] }) },
    { device: QUADRO, group: MONITOR, slots: pairs(32, [Q_MUTE, 0], { 0: [MIX1_OUT, 0] }) },
    { device: STUDIO, group: S_USB_REC, slots: pairs(32, [S_MUTE, 0], { 20: [S_SPDIF_IN, 0] }) },
  ]);
  assert.deepEqual(store.aggregate.outcome.value, { text: "The phase path is back: 3 routing groups were sent.", problem: false });
});

/** Dry run: nothing reaches an interface, and a fix's bytes are built on the routing already known. */
test("in dry run a dedication is not planned, and a fix's bytes build on what is known and reach nothing", async () => {
  const { client, store, writes } = await setup();
  const dry = { ...phasePathContext(store), dryRun: true, routing: () => undefined };
  const refused = planDedication(spdif, dry);
  assert.ok(!refused.ok);
  assert.equal(refused.why, "The server is in dry run, where the interfaces' routing cannot be read, so which channels are free is not known. A cable is dedicated only with the routing read.");

  // The server goes into dry run: a read answers nothing, and a write is built and not sent.
  client.respond = async (call: Invocation) => ({ device_id: call.deviceId, command: call.command, sent_hex: "74", sent_len: 16, dry_run: true, response: null, response_error: null });
  const done = await restorePath([{ deviceId: QUADRO, destination: HP1, channel: 1, source: { source: COM_PLAY, channel: 15 } }], storeRouteWriter(store));
  assert.deepEqual(done, { text: "The phase path is back: one routing group was sent.", problem: false });
  assert.deepEqual(writes(), [{ device: QUADRO, group: HP1, slots: pairs(32, [Q_MUTE, 0], { 0: [MIX1_OUT, 0], 1: [COM_PLAY, 15] }) }], "HP1 L keeps Mix 1 L, as last read");
});
