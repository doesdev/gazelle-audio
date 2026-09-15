import { test } from "node:test";
import assert from "node:assert/strict";

import { GazelleError } from "gazelle-audio-client";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { ROUTING_SLOTS, type RouteSlot } from "../src/store/routing.ts";
import { Store } from "../src/store/store.ts";
import { builtInThemes, device, FakeClient, MemoryStorage, type Invocation } from "./fake-client.ts";

// Quadro topology positions (P38): sources PREAMP 0, LOOPBACK HP1 6, MUTE 10; destinations HP1 1, MIX CH4 11.
const PREAMP = 0;
const LOOPBACK_HP1 = 6;
const MUTE = 10;
const HP1 = 1;
const MIX_CH4 = 11;

type Pair = [number, number];

/** A store over a fake Quadro that keeps routing like the device: one 32-slot group per destination. */
function setup({ dryRun = false } = {}) {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"));
  const groups = new Map<number, Pair[]>();
  const slotsOf = (group: number): Pair[] => groups.get(group) ?? Array.from({ length: ROUTING_SLOTS }, (): Pair => [MUTE, 0]);
  const envelope = (call: Invocation, response: unknown) => ({ device_id: call.deviceId, command: call.command, sent_hex: "74", sent_len: 16, dry_run: dryRun, response, response_error: null });
  const device_ = async (call: Invocation): Promise<unknown> => {
    if (call.command === "get_routing") {
      const group = call.options?.["ext3"] as number;
      // The Quadro reply declares 64 pairs; the model keeps one per channel.
      const pairs = [...slotsOf(group), ...Array.from({ length: ROUTING_SLOTS }, (): Pair => [MUTE, 0])];
      return envelope(call, dryRun ? null : { bank_idx: group, bank_configs: pairs.map(([p, c]) => ({ in_periph_id: p, in_chann: c })) });
    }
    if (call.command === "set_routing" && !dryRun) {
      const args = call.args as { bank_idx: number; bank_configs: Uint8Array[] };
      groups.set(args.bank_idx, args.bank_configs.map((b): Pair => [b[0] as number, b[1] as number]));
    }
    return envelope(call, null);
  };
  client.respond = device_;
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), themeSources: builtInThemes });
  const writes = () => client.invocations.filter((c) => c.command === "set_routing").map((c) => c.args as { bank_idx: number; bank_configs: Uint8Array[] });
  const pairsOf = (configs: Uint8Array[]) => configs.map((b): Pair => [b[0] as number, b[1] as number]);
  return { client, groups, store, routing: store.routing("loopback-0"), writes, pairsOf, device: device_ };
}

const muted = (n: number): Pair[] => Array.from({ length: n }, (): Pair => [MUTE, 0]);
const slot = (source: number, channel: number): RouteSlot => ({ source, channel });
const noticeText = (store: Store) => JSON.stringify(store.notices.value);

test("a destination group is read by its position in ext3, one slot per channel", async () => {
  const { client, routing } = setup();
  assert.equal(routing.mute, MUTE);
  assert.equal(routing.destination(MIX_CH4).value, undefined, "unknown until read");

  assert.equal(await routing.load(MIX_CH4), true);
  assert.deepEqual(client.invocations.map((c) => [c.command, c.options?.["ext3"]]), [["get_routing", MIX_CH4]]);
  assert.equal(routing.destination(MIX_CH4).value?.length, 32);

  await routing.load(HP1);
  assert.deepEqual(routing.destination(HP1).value, [slot(MUTE, 0), slot(MUTE, 0)], "HP1 has two channels");
});

test("a route reads its group fresh and changes only its own slot, keeping routes made elsewhere", async () => {
  const { client, groups, routing, writes, pairsOf } = setup();
  await routing.load(MIX_CH4);
  // Another panel routes PREAMP 2 to slot 3 after this browser read the group.
  groups.set(MIX_CH4, muted(32).map((p, i): Pair => (i === 3 ? [PREAMP, 1] : p)));

  assert.equal(await routing.route(MIX_CH4, 7, slot(PREAMP, 2)), true);
  const commands = client.invocations.map((c) => c.command);
  assert.deepEqual(commands.slice(-2), ["get_routing", "set_routing"], "read immediately before the write");
  const [write] = writes();
  assert.equal(write?.bank_idx, MIX_CH4);
  const expected = muted(32);
  expected[3] = [PREAMP, 1];
  expected[7] = [PREAMP, 2];
  assert.deepEqual(pairsOf(write?.bank_configs ?? []), expected);
  assert.deepEqual(routing.destination(MIX_CH4).value?.[3], slot(PREAMP, 1));
  assert.deepEqual(routing.destination(MIX_CH4).value?.[7], slot(PREAMP, 2));
});

test("slots past a group's channels are MUTE, and unrouting writes MUTE", async () => {
  const { routing, writes, pairsOf } = setup();
  await routing.route(HP1, 1, slot(LOOPBACK_HP1, 1));
  const first = muted(32);
  first[1] = [LOOPBACK_HP1, 1];
  assert.deepEqual(pairsOf(writes()[0]?.bank_configs ?? []), first);

  await routing.route(HP1, 1, null);
  assert.deepEqual(pairsOf(writes()[1]?.bank_configs ?? []), muted(32));
  assert.deepEqual(routing.destination(HP1).value, [slot(MUTE, 0), slot(MUTE, 0)]);
});

test("changes to one group run one at a time, so neither overwrites the other", async () => {
  const { groups, routing } = setup();
  const results = await Promise.all([routing.route(MIX_CH4, 7, slot(PREAMP, 0)), routing.route(MIX_CH4, 8, slot(PREAMP, 1))]);
  assert.deepEqual(results, [true, true]);
  assert.deepEqual([groups.get(MIX_CH4)?.[7], groups.get(MIX_CH4)?.[8]], [[PREAMP, 0], [PREAMP, 1]]);
});

test("a failed write restores what the device reported and says why", async () => {
  const { client, routing, store, device } = setup();
  client.respond = async (call) => {
    if (call.command === "set_routing") throw new GazelleError("device_gone", "the device went away");
    return device(call);
  };
  assert.equal(await routing.route(MIX_CH4, 7, slot(PREAMP, 2)), false);
  assert.deepEqual(routing.destination(MIX_CH4).value?.[7], slot(MUTE, 0));
  assert.match(noticeText(store), /set_routing failed: the device went away/);
});

test("nothing is written when the device does not report the group's routing", async () => {
  const { client, routing, store, writes, device } = setup();
  client.respond = async (call) => (call.command === "get_routing" ? { device_id: call.deviceId, command: call.command, sent_hex: "74", sent_len: 16, dry_run: false, response: null, response_error: "could not decode 0 bytes" } : device(call));
  assert.equal(await routing.route(MIX_CH4, 7, slot(PREAMP, 2)), false);
  assert.match(noticeText(store), /Could not read the routing to MIX CH4: could not decode 0 bytes/);

  client.respond = async (call) => (call.command === "get_routing" ? { device_id: call.deviceId, command: call.command, sent_hex: "74", sent_len: 16, dry_run: false, response: null, response_error: null } : device(call));
  assert.equal(await routing.route(MIX_CH4, 7, slot(PREAMP, 2)), false);
  assert.match(noticeText(store), /Routing to MIX CH4 was not changed: the device did not report its current routing/);
  assert.deepEqual(writes(), []);
});

test("in dry run a route builds on what is already known", async () => {
  const { routing, writes, pairsOf } = setup({ dryRun: true });
  assert.equal(await routing.route(MIX_CH4, 7, slot(PREAMP, 0)), true);
  assert.equal(await routing.route(MIX_CH4, 8, slot(PREAMP, 1)), true);
  const expected = muted(32);
  expected[7] = [PREAMP, 0];
  expected[8] = [PREAMP, 1];
  assert.deepEqual(pairsOf(writes()[1]?.bank_configs ?? []), expected, "the second write keeps the first route");
});

test("routes outside the topology are refused before anything is sent", () => {
  const { client, routing } = setup();
  assert.throws(() => routing.route(99, 0, null), RangeError);
  assert.throws(() => routing.route(HP1, 2, null), RangeError, "HP1 has channels 0..1");
  assert.throws(() => routing.route(MIX_CH4, 0, slot(PREAMP, 4)), RangeError, "PREAMP has channels 0..3");
  assert.throws(() => routing.route(MIX_CH4, 0, slot(42, 0)), RangeError);
  assert.deepEqual(client.invocations, []);
});
