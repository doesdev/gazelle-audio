import { test } from "node:test";
import assert from "node:assert/strict";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { Store } from "../src/store/store.ts";
import { builtInThemes, device, FakeClient, flush, MemoryStorage, type Invocation } from "./fake-client.ts";

const Q = "loopback-0";
const S = "loopback-1";
// Quadro topology positions (P38): sources PREAMP 0, USB 1 PLAY 1, AFX OUT 5, MUTE 10; MIX CH1-4 are destinations 8-11.
const PREAMP = 0;
const USB1 = 1;
const AFX_OUT = 5;
const MUTE = 10;
const MIX = [8, 9, 10, 11];

type Pair = [number, number];

/** A started store over fake devices that keep routing like the hardware. */
async function setup() {
  const client = new FakeClient(device(Q, "quadro", "Zen Quadro"), device(S, "studio", "Zen Studio+"));
  const muteOf = (deviceId: string) => (deviceId === Q ? MUTE : 11);
  const routes = new Map<string, Pair[]>();
  const group = (deviceId: string, destination: number): Pair[] => {
    const key = `${deviceId}:${destination}`;
    let slots = routes.get(key);
    if (slots === undefined) {
      slots = Array.from({ length: 32 }, (): Pair => [muteOf(deviceId), 0]);
      routes.set(key, slots);
    }
    return slots;
  };
  client.respond = async (call: Invocation) => {
    const envelope = { device_id: call.deviceId, command: call.command, sent_hex: "70", sent_len: 16, dry_run: false, response: null as unknown, response_error: null };
    if (call.command === "get_routing") {
      const destination = call.options?.["ext3"] as number;
      envelope.response = { bank_idx: destination, bank_configs: group(call.deviceId, destination).map(([p, c]) => ({ in_periph_id: p, in_chann: c })) };
    } else if (call.command === "set_routing") {
      const args = call.args as { bank_idx: number; bank_configs: Uint8Array[] };
      routes.set(`${call.deviceId}:${args.bank_idx}`, args.bank_configs.map((b): Pair => [b[0] as number, b[1] as number]));
    }
    return envelope;
  };
  const timers = new ManualTimers();
  const store = new Store(client, { timers, storage: new MemoryStorage(), themeSources: builtInThemes });
  await store.start();
  const set = (deviceId: string, destination: number, slot: number, pair: Pair) => {
    group(deviceId, destination)[slot] = pair;
  };
  const at = (deviceId: string, destination: number, slot: number) => group(deviceId, destination)[slot];
  const writes = () => client.invocations.filter((c) => c.command === "set_routing").length;
  return { client, store, timers, set, at, writes };
}

test("a device without a layout imports its channels from the mixer routing, skipping the Quadro's effect returns", async () => {
  const { store, set } = await setup();
  for (let slot = 0; slot < 6; slot++) set(Q, MIX[0] as number, slot, [AFX_OUT, slot]);
  set(Q, MIX[0] as number, 6, [PREAMP, 1]);
  set(Q, MIX[1] as number, 6, [PREAMP, 1]);
  set(Q, MIX[3] as number, 6, [USB1, 0]); // another source on the same slot stays on the device
  set(Q, MIX[2] as number, 9, [USB1, 3]);

  const channels = store.channels(Q);
  assert.equal(channels.configured, false);
  assert.equal(await channels.importFromDevice(), true);
  assert.deepEqual(
    channels.layout.value.channels.map(({ id: _, ...rest }) => rest),
    [
      { name: "PREAMP 2", slot: 6, source: { group: PREAMP, channel: 1 }, main_mix: 0, sends: [1] },
      { name: "USB 1 PLAY 4", slot: 9, source: { group: USB1, channel: 3 }, main_mix: 2, sends: [] },
    ],
  );
  assert.equal(channels.configured, true);
  assert.equal(await channels.importFromDevice(), false, "an existing layout is never replaced");
});

test("with nothing routed the layout starts with one inactive channel", async () => {
  const { store, writes } = await setup();
  const channels = store.channels(Q);
  await channels.importFromDevice();
  const [only, ...rest] = channels.layout.value.channels;
  assert.deepEqual([only?.name, only?.slot, only?.source, only?.main_mix, only?.sends, rest], ["", 6, undefined, undefined, [], []]);
  assert.equal(channels.isActive(only as NonNullable<typeof only>), false);
  assert.equal(writes(), 0, "importing writes nothing to the device");
});

test("channels take the lowest free slot, from input 7 on the Quadro and 1 on the Studio+, until the mixer is full", async () => {
  const { store } = await setup();
  const quadro = store.channels(Q);
  const first = quadro.add();
  const second = quadro.add();
  assert.deepEqual(quadro.layout.value.channels.map((c) => c.slot), [6, 7]);
  assert.notEqual(first, second);
  for (let i = 0; i < 24; i++) assert.notEqual(quadro.add(), undefined);
  assert.equal(quadro.add(), undefined, "26 inputs are free on the Quadro");
  assert.match(JSON.stringify(store.notices.value), /All 26 mixer channels are in use \(inputs 1–6 carry the effect returns\)/);

  await quadro.remove(first as string);
  assert.equal(quadro.channel(quadro.add() as string)?.slot, 6, "a freed slot is reused");

  const studio = store.channels(S);
  studio.add();
  assert.deepEqual(studio.layout.value.channels.map((c) => c.slot), [0]);
});

test("a channel routes only once it has an input and a main mix, then feeds its main mix and sends", async () => {
  const { store, at, writes } = await setup();
  const channels = store.channels(Q);
  const id = channels.add() as string;

  assert.equal(await channels.setSource(id, { group: PREAMP, channel: 2 }), true);
  await channels.setSend(id, 3, true);
  assert.equal(writes(), 0, "without a main mix nothing is routed");

  await channels.setMainMix(id, 1);
  assert.deepEqual([at(Q, MIX[1] as number, 6), at(Q, MIX[3] as number, 6), at(Q, MIX[0] as number, 6)], [[PREAMP, 2], [PREAMP, 2], [MUTE, 0]]);
  assert.equal(channels.isActive(channels.channel(id) as NonNullable<ReturnType<typeof channels.channel>>), true);
  assert.equal(await channels.setSend(id, 1, true), false, "a channel does not send to its own main mix");

  await channels.setMainMix(id, 3);
  assert.deepEqual(channels.channel(id)?.sends, [], "the new main mix is no longer a send");
  assert.deepEqual([at(Q, MIX[1] as number, 6), at(Q, MIX[3] as number, 6)], [[MUTE, 0], [PREAMP, 2]]);

  await channels.setSource(id, { group: USB1, channel: 5 });
  assert.deepEqual(at(Q, MIX[3] as number, 6), [USB1, 5]);
  await channels.setSource(id, undefined);
  assert.deepEqual(at(Q, MIX[3] as number, 6), [MUTE, 0], "clearing the input mutes what the channel fed");
});

test("a group's channels stay together: joining a group moves a channel after its last member; groups have a colour", async () => {
  const { store } = await setup();
  const channels = store.channels(Q);
  const [a, b, c, d] = [channels.add(), channels.add(), channels.add(), channels.add()] as string[];
  const order = () => channels.layout.value.channels.map((x) => x.id);

  const drums = channels.addGroup("Drums", [a as string]) as string;
  assert.deepEqual(channels.channel(a as string)?.group, drums, "a new group can start with channels");
  channels.setGroup(d as string, drums);
  assert.deepEqual(order(), [a, d, b, c], "d joins right after the group's last member");
  channels.setGroup(c as string, drums);
  assert.deepEqual(order(), [a, d, c, b]);
  channels.setGroup(d as string, undefined);
  assert.deepEqual(order(), [a, c, d, b], "leaving a group moves a channel out after it, so the group stays contiguous");

  assert.equal(channels.setGroupColor(drums, "#b5473a"), true);
  assert.equal(channels.layout.value.groups[0]?.color, "#b5473a");
  channels.setGroupColor(drums, undefined);
  assert.equal("color" in (channels.layout.value.groups[0] ?? {}), false, "an unset colour is omitted");
  assert.throws(() => channels.setGroupColor(drums, "red"), RangeError);

  const keys = channels.addGroup("Keys") as string;
  channels.setGroup(b as string, keys);
  channels.move(b as string, 0);
  assert.deepEqual(order(), [b, a, c, d], "moving a grouped channel alone is allowed; groups render by runs");
  assert.equal(channels.groupOf(b as string)?.name, "Keys");
});

test("a mix's outputs are the destination pairs its mix output feeds; turning one on routes left and right, off mutes them", async () => {
  const { store, set, at } = await setup();
  const channels = store.channels(Q);
  // Quadro: mix 1's output is source 6 (LOOPBACK HP1). Destinations: LINE OUT 0, HP1 1, HP2 2, MONITOR 3, USB A REC 4 (16 channels), ...
  const pairs = channels.outputPairs();
  assert.deepEqual(pairs.slice(0, 6).map((p) => p.label), ["LINE OUT", "HP1", "HP2", "MONITOR", "USB A REC 1/2", "USB A REC 3/4"]);
  assert.equal(pairs.some((p) => p.label.startsWith("MIX CH")), false, "mixer inputs are channels' business, not a mix's outputs");

  set(Q, 3, 0, [6, 0]);
  set(Q, 3, 1, [6, 1]);
  set(Q, 4, 2, [6, 0]);
  set(Q, 4, 3, [6, 1]);
  set(Q, 1, 0, [6, 0]); // HP1 left only: not a whole pair
  await channels.loadOutputs();
  assert.deepEqual(channels.mixOutputs(0).value.map((p) => p.label), ["MONITOR", "USB A REC 3/4"]);

  assert.equal(await channels.setMixOutput(0, { destination: 2, channel: 0 }, true), true);
  assert.deepEqual([at(Q, 2, 0), at(Q, 2, 1)], [[6, 0], [6, 1]]);
  assert.deepEqual(channels.mixOutputs(0).value.map((p) => p.label), ["HP2", "MONITOR", "USB A REC 3/4"]);
  await channels.setMixOutput(0, { destination: 3, channel: 0 }, false);
  assert.deepEqual([at(Q, 3, 0), at(Q, 3, 1)], [[MUTE, 0], [MUTE, 0]]);
  assert.deepEqual(channels.mixOutputs(1).value, [], "mix 2 feeds nothing");
  assert.throws(() => channels.setMixOutput(0, { destination: 8, channel: 0 }, true), RangeError, "a mixer input is not an output");
});

test("removing mutes every mix the channel fed; order, names, groups and mix names are saved in the workspace", async () => {
  const { client, store, at, timers } = await setup();
  const channels = store.channels(Q);
  const a = channels.add() as string;
  const b = channels.add() as string;
  await channels.setSource(a, { group: PREAMP, channel: 0 });
  await channels.setMainMix(a, 0);
  await channels.setSend(a, 2, true);
  assert.deepEqual([at(Q, MIX[0] as number, 6), at(Q, MIX[2] as number, 6)], [[PREAMP, 0], [PREAMP, 0]]);
  await channels.remove(a);
  assert.deepEqual([at(Q, MIX[0] as number, 6), at(Q, MIX[2] as number, 6)], [[MUTE, 0], [MUTE, 0]]);

  const c = channels.add() as string;
  channels.move(c, 0);
  channels.rename(c, "Vox");
  const drums = channels.addGroup("Drums") as string;
  channels.setGroup(b, drums);
  channels.renameMix(1, "Cue A");
  assert.deepEqual(channels.layout.value.channels.map((x) => [x.id, x.name, x.group]), [[c, "Vox", undefined], [b, "", drums]]);
  assert.deepEqual([channels.mixName(1), channels.mixName(2)], ["Cue A", "Mix 3"]);
  channels.removeGroup(drums);
  assert.equal(channels.channel(b)?.group, undefined);
  assert.throws(() => channels.setGroup(b, "nope"), RangeError);

  timers.advance(1000);
  await flush();
  assert.deepEqual(client.stored.mixers[Q]?.channels.map((x) => x.id), [c, b]);
  assert.deepEqual(client.stored.mixers[Q]?.mixes, [{}, { name: "Cue A" }]);
});
