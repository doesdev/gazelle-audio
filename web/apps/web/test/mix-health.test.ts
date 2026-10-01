// What a mix plays outside its channels (store/mix-health.ts) and tidying it (store/mix-tidy.ts):
// the owner's Quadro had soloed effect returns silencing Mix 1 and a leftover USB pair playing in
// HP2's mix, neither shown by the Mixer page. Tested against a fake device that keeps routing and
// strips, so the tidy's writes and the read after them are the device's.

import { test } from "node:test";
import assert from "node:assert/strict";

import { topologies, type MixerChannel } from "gazelle-audio-client";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { mixHealth, mixWarning } from "../src/store/mix-health.ts";
import { applyTidy, planLines, tidyPlan, type Naming } from "../src/store/mix-tidy.ts";
import { LEVEL_MAX, PAN_CENTRE, type StripState } from "../src/store/mixer.ts";
import type { RouteSlot } from "../src/store/routing.ts";
import { Store } from "../src/store/store.ts";
import { builtInThemes, device, FakeClient, MemoryStorage } from "./fake-client.ts";

/** Quadro topology positions, as `refs/schemas/quadro_topology.json` lists them. */
const PREAMP = 0;
const USB1 = 1;
const AFX_OUT = 5;
const MUTE = 10;
const MIX_IN = [8, 9, 10, 11];
/** The Studio+'s AFX OUT and MUTE sources. */
const STUDIO_AFX_OUT = 6;
const STUDIO_MUTE = 11;

const quadro = topologies.quadro;
const unity: StripState = { level: 0, pan: PAN_CENTRE, mute: false, solo: false, send: 0, linked: false };

/** 32 slots, MUTE except those given. */
function slots(routed: Record<number, [number, number]>, mute = MUTE): RouteSlot[] {
  return Array.from({ length: 32 }, (_, i) => ({ source: routed[i]?.[0] ?? mute, channel: routed[i]?.[1] ?? 0 }));
}

const channel = (id: string, slot: number, group: number, ch: number, mainMix = 0, sends: number[] = []): MixerChannel => ({ id, name: "", slot, source: { group, channel: ch }, main_mix: mainMix, sends });

test("a routed, audible slot no channel of the mix uses is a stray; a channel on the wrong input is one too, naming the channel", () => {
  const health = mixHealth(quadro, slots({ 6: [PREAMP, 1], 9: [USB1, 2] }), () => unity, [channel("vox", 6, PREAMP, 0)]);
  assert.deepEqual(
    health.strays.map((s) => [s.slot, s.source.source, s.source.channel, s.channel?.id]),
    [
      [6, PREAMP, 1, "vox"],
      [9, USB1, 2, undefined],
    ],
  );
  assert.deepEqual(health.solos, []);
  assert.deepEqual(health.returns, []);
});

test("a muted strip, one at the floor, and a MUTE slot play nothing and are not strays", () => {
  const strip = (slot: number): StripState => (slot === 10 ? { ...unity, mute: true } : slot === 11 ? { ...unity, level: LEVEL_MAX } : unity);
  const health = mixHealth(quadro, slots({ 10: [USB1, 0], 11: [USB1, 1] }), strip, []);
  assert.deepEqual(health.strays, [], "nothing is heard from any of them");
  const just = mixHealth(quadro, slots({ 11: [USB1, 1] }), (slot) => (slot === 11 ? { ...unity, level: LEVEL_MAX - 1 } : unity), []);
  assert.deepEqual(just.strays.map((s) => s.slot), [11], "one dB above the floor is still heard");
});

test("a solo on a Quadro effect return counts even when it is muted, and is not shown by any channel", () => {
  // The owner's Mix 1: AFX Out 5 and 6 soloed, muted, at -2 dB.
  const afx = Object.fromEntries(Array.from({ length: 6 }, (_, k) => [k, [AFX_OUT, k] as [number, number]]));
  const strip = (slot: number): StripState => (slot === 4 || slot === 5 ? { ...unity, level: 2, mute: true, solo: true } : slot < 4 ? { ...unity, level: LEVEL_MAX } : unity);
  const health = mixHealth(quadro, slots({ ...afx, 6: [PREAMP, 0], 7: [PREAMP, 1] }), strip, [channel("a", 6, PREAMP, 0), channel("b", 7, PREAMP, 1)]);
  assert.deepEqual(health.solos.map((s) => [s.slot, s.shown, s.mute]), [
    [4, false, true],
    [5, false, true],
  ]);
  assert.deepEqual(health.strays, [], "the channels match, and the soloed returns are muted");
  assert.deepEqual(health.returns, [], "the other returns sit at the floor");
  assert.equal(mixWarning(health), "2 soloed strips");
});

test("the Quadro's audible effect returns are their own category; another input on slots 1 to 6 is a stray", () => {
  const health = mixHealth(quadro, slots({ 0: [AFX_OUT, 0], 1: [AFX_OUT, 1], 2: [AFX_OUT, 2], 5: [USB1, 0] }), (slot) => (slot === 2 ? { ...unity, level: 10 } : unity), []);
  assert.deepEqual(health.returns.map((s) => [s.slot, s.level]), [
    [0, 0],
    [1, 0],
    [2, 10],
  ]);
  assert.deepEqual(health.strays.map((s) => s.slot), [5]);
  assert.equal(mixWarning(health), "1 strip plays outside this mix's channels", "effect returns are not warned about");
  assert.equal(mixWarning(mixHealth(quadro, slots({ 0: [AFX_OUT, 0] }), () => unity, [])), undefined);
});

test("the Studio+ has no fixed effect returns: AFX OUT on a slot no channel uses is a stray", () => {
  const studio = topologies.studio;
  const health = mixHealth(studio, slots({ 0: [STUDIO_AFX_OUT, 0] }, STUDIO_MUTE), () => unity, []);
  assert.deepEqual(health.returns, []);
  assert.deepEqual(health.strays.map((s) => [s.slot, s.source.source]), [[0, STUDIO_AFX_OUT]]);
});

/** A Quadro that keeps routing and mixer strips as the loopback does, recording every command. */
function liveQuadro(routes: Record<number, RouteSlot[]>, strips: Record<string, Partial<StripState>>) {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"));
  client.server = { ...client.server, dry_run: false };
  const stripOf = (mix: number, channel: number) => ({ ...unity, ...strips[`${mix}:${channel}`] });
  client.respond = async (call) => {
    const args = call.args ?? {};
    let response: Record<string, unknown> | null = null;
    if (call.command === "set_routing") {
      routes[Number(args["bank_idx"])] = (args["bank_configs"] as Uint8Array[]).map((pair) => ({ source: pair[0] as number, channel: pair[1] as number }));
    } else if (call.command === "get_routing") {
      const group = routes[Number(call.options?.["ext3"])] ?? slots({});
      response = { bank_idx: call.options?.["ext3"], bank_configs: group.map((s) => ({ in_periph_id: s.source, in_chann: s.channel })) };
    } else if (call.command === "set_mixer") {
      const { mixer_id, channel, ...values } = args as Record<string, number>;
      strips[`${mixer_id}:${(channel as number) - 1}`] = { level: values["level"] as number, pan: values["pan"] as number, mute: values["mute"] === 1, solo: values["solo"] === 1 };
    } else if (call.command === "get_mixer") {
      const mix = Number(call.options?.["ext3"]);
      response = { entries: [{ level: 0, pan: 32, mute: 0, solo: 0 }, ...Array.from({ length: 32 }, (_, i) => stripOf(mix, i)).map((s) => ({ level: s.level, pan: s.pan, mute: s.mute ? 1 : 0, solo: s.solo ? 1 : 0 }))] };
    } else if (call.command === "get_mixer_links") {
      response = { entries: Array.from({ length: 64 }, () => ({ linked: 0 })) };
    }
    return { device_id: call.deviceId, command: call.command, sent_hex: "74", sent_len: 1, dry_run: false, response, response_error: null };
  };
  return client;
}

/** The owner's Quadro as read on 2026-09-30 (Mix 1 and HP2's Mix 4), with the layout's channels. */
async function ownersQuadro() {
  const afx = Object.fromEntries(Array.from({ length: 6 }, (_, k) => [k, [AFX_OUT, k] as [number, number]]));
  const routes: Record<number, RouteSlot[]> = {
    [MIX_IN[0] as number]: slots({ ...afx, 6: [PREAMP, 0], 7: [PREAMP, 1] }),
    [MIX_IN[3] as number]: slots({ ...afx, 6: [USB1, 2], 7: [USB1, 3], 8: [USB1, 0] }),
  };
  const strips: Record<string, Partial<StripState>> = { "0:4": { level: 2, mute: true, solo: true }, "0:5": { level: 2, mute: true, solo: true } };
  for (const k of [0, 1, 2, 3]) strips[`0:${k}`] = { level: LEVEL_MAX };
  for (const k of [0, 1, 2, 3]) strips[`3:${k}`] = { level: LEVEL_MAX };
  const client = liveQuadro(routes, strips);
  client.stored = {
    version: 1,
    groups: [],
    links: [],
    aliases: {},
    mixers: { "loopback-0": { mixes: [{ name: "HP Amp Tracking" }, {}, {}, { name: "HP2 Tracking" }], groups: [], channels: [channel("a", 6, PREAMP, 0), channel("b", 7, PREAMP, 1), channel("c", 8, USB1, 0, 3)] } },
  };
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), requestFrame: () => {}, themeSources: builtInThemes });
  await store.start();
  await store.readMixes("loopback-0");
  await store.readRoutes("loopback-0", MIX_IN);
  return { client, store, routes, strips };
}

const naming = (store: Store): Naming => ({ source: (s) => store.channels("loopback-0").sourceLabel(s), mute: MUTE });

test("the owner's Quadro: Mix 1's hidden solos and Mix 4's leftover USB pair are found, and nothing else", async () => {
  const { store } = await ownersQuadro();
  assert.equal(store.mixWarning("loopback-0", 0), "2 soloed strips");
  assert.equal(store.mixWarning("loopback-0", 1), undefined);
  assert.equal(store.mixWarning("loopback-0", 2), undefined);
  assert.equal(store.mixWarning("loopback-0", 3), "2 strips play outside this mix's channels");
  const hp2 = store.mixHealth("loopback-0", 3).value;
  assert.deepEqual(hp2?.strays.map((s) => [s.slot, s.source.source, s.source.channel]), [
    [6, USB1, 2],
    [7, USB1, 3],
  ]);
  assert.deepEqual(hp2?.returns.map((s) => s.slot), [4, 5], "AFX Out 5 and 6 play in HP2 at unity");
});

test("tidying Mix 4 writes one set_routing muting only the leftover slots, leaves the effect returns alone, and reads the mix again", async () => {
  const { client, store, routes } = await ownersQuadro();
  const health = store.mixHealth("loopback-0", 3).value;
  assert.ok(health);
  const plan = tidyPlan(health, false);
  assert.deepEqual(planLines(plan, naming(store), () => ""), [
    "Slot 7: route Mute in place of USB 1 Play 3, which is not one of this mix's channels",
    "Slot 8: route Mute in place of USB 1 Play 4, which is not one of this mix's channels",
  ]);
  client.invocations.length = 0;
  assert.equal(await applyTidy(store, "loopback-0", 3, plan), 2);
  const commands = client.invocations.map((c) => [c.command, c.options?.["ext3"]]);
  assert.deepEqual(commands, [
    ["get_routing", MIX_IN[3]],
    ["set_routing", undefined],
    ["get_routing", MIX_IN[3]],
    ["get_mixer", 3],
    ["get_mixer_links", undefined],
  ], "read first, one write, then the mix read back");
  const written = client.invocations.find((c) => c.command === "set_routing")?.args;
  assert.equal(written?.["bank_idx"], MIX_IN[3]);
  const pairs = (written?.["bank_configs"] as Uint8Array[]).map((p) => [p[0], p[1]]);
  const expected = slots({ 0: [AFX_OUT, 0], 1: [AFX_OUT, 1], 2: [AFX_OUT, 2], 3: [AFX_OUT, 3], 4: [AFX_OUT, 4], 5: [AFX_OUT, 5], 8: [USB1, 0] }).map((s) => [s.source, s.channel]);
  assert.deepEqual(pairs, expected, "slots 7 and 8 MUTE, the effect returns and the layout's channel kept");
  assert.deepEqual(routes[MIX_IN[3] as number]?.slice(6, 9), [{ source: MUTE, channel: 0 }, { source: MUTE, channel: 0 }, { source: USB1, channel: 0 }]);
  assert.equal(store.mixWarning("loopback-0", 3), undefined, "clean once read back");
  assert.deepEqual(store.mixHealth("loopback-0", 3).value?.returns.map((s) => s.slot), [4, 5], "the returns still play");
});

test("tidying Mix 1 clears both solos with set_mixer, keeping level and mute; asked to, a tidy also mutes the effect returns", async () => {
  const { client, store, strips } = await ownersQuadro();
  const plan = tidyPlan(store.mixHealth("loopback-0", 0).value!, false);
  assert.deepEqual(planLines(plan, naming(store), () => ""), ["AFX Out 5 (slot 5): clear its solo", "AFX Out 6 (slot 6): clear its solo"]);
  client.invocations.length = 0;
  assert.equal(await applyTidy(store, "loopback-0", 0, plan), 2);
  assert.deepEqual(
    client.invocations.filter((c) => c.command === "set_mixer").map((c) => c.args),
    [
      { mixer_id: 0, channel: 5, level: 2, pan: 32, mute: 1, solo: 0 },
      { mixer_id: 0, channel: 6, level: 2, pan: 32, mute: 1, solo: 0 },
    ],
  );
  assert.equal(client.invocations.some((c) => c.command === "set_routing"), false, "nothing to route");
  assert.equal(strips["0:4"]?.solo, false);
  assert.equal(store.mixWarning("loopback-0", 0), undefined);

  // Mix 4 with "also mute the effect returns": the two strays and the two playing returns.
  const hp2 = tidyPlan(store.mixHealth("loopback-0", 3).value!, true);
  assert.deepEqual(planLines(hp2, naming(store), () => "").slice(2), ["AFX Out 5 (slot 5): mute this effect return", "AFX Out 6 (slot 6): mute this effect return"]);
  client.invocations.length = 0;
  assert.equal(await applyTidy(store, "loopback-0", 3, hp2), 4);
  assert.deepEqual(
    client.invocations.filter((c) => c.command === "set_mixer").map((c) => c.args),
    [
      { mixer_id: 3, channel: 5, level: 0, pan: 32, mute: 1, solo: 0 },
      { mixer_id: 3, channel: 6, level: 0, pan: 32, mute: 1, solo: 0 },
    ],
  );
  assert.deepEqual(store.mixHealth("loopback-0", 3).value, { strays: [], solos: [], returns: [] });
});

test("a channel on the wrong input is routed back to its layout input, not muted", async () => {
  const routes: Record<number, RouteSlot[]> = { [MIX_IN[0] as number]: slots({ 6: [PREAMP, 3] }) };
  const client = liveQuadro(routes, {});
  client.stored = { version: 1, groups: [], links: [], aliases: {}, mixers: { "loopback-0": { mixes: [], groups: [], channels: [{ ...channel("vox", 6, PREAMP, 0), name: "Vox" }] } } };
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), requestFrame: () => {}, themeSources: builtInThemes });
  await store.start();
  await store.readMixes("loopback-0");
  await store.readRoutes("loopback-0", MIX_IN);
  const plan = tidyPlan(store.mixHealth("loopback-0", 0).value!, false);
  assert.deepEqual(planLines(plan, naming(store), (id) => store.channels("loopback-0").displayName(store.channels("loopback-0").channel(id)!)), [
    "Slot 7: route Vox's input Preamp 1 again, in place of Preamp 4, as applying the layout would",
  ]);
  assert.equal(await applyTidy(store, "loopback-0", 0, plan), 1);
  assert.deepEqual(routes[MIX_IN[0] as number]?.[6], { source: PREAMP, channel: 0 });
  assert.equal(store.mixWarning("loopback-0", 0), undefined);
});

test("nothing is known, and nothing is warned about, until the mix's routing and strips are read", async () => {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"));
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), requestFrame: () => {}, themeSources: builtInThemes });
  await store.start();
  await store.readMixes("loopback-0");
  await store.readRoutes("loopback-0", MIX_IN);
  assert.equal(store.mixHealth("loopback-0", 0).value, undefined, "a dry run reads nothing");
  assert.equal(store.mixWarning("loopback-0", 0), undefined);
});
