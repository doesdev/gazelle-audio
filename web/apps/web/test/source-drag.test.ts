// Dragging Routing page sources onto the mixer dock: what the drag carries, and what a drop does to
// the layout and the device, which must be what "+" and a channel's Input and Main mix menus do.

import { test } from "node:test";
import assert from "node:assert/strict";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { addDroppedSources, carriesSources, decodeSourceDrag, doublingsOf, dropHint, encodeSourceDrag, layoutForDrop, SOURCE_MIME } from "../src/elements/source-drag.ts";
import { Store } from "../src/store/store.ts";
import { builtInThemes, device, FakeClient, MemoryStorage, type Invocation } from "./fake-client.ts";

const Q = "loopback-0";
// Quadro topology positions: sources PREAMP 0, USB 1 PLAY 1, MUTE 10; MIX CH1-4 are destinations 8-11.
const PREAMP = 0;
const USB1 = 1;
const MUTE = 10;
const MIX = [8, 9, 10, 11];

type Pair = [number, number];

/** A started store over a fake Quadro that keeps routing like the hardware. */
async function setup() {
  const client = new FakeClient(device(Q, "quadro", "Zen Quadro"));
  const routes = new Map<number, Pair[]>();
  const group = (destination: number): Pair[] => {
    let slots = routes.get(destination);
    if (slots === undefined) {
      slots = Array.from({ length: 32 }, (): Pair => [MUTE, 0]);
      routes.set(destination, slots);
    }
    return slots;
  };
  client.respond = async (call: Invocation) => {
    const envelope = { device_id: call.deviceId, command: call.command, sent_hex: "70", sent_len: 16, dry_run: false, response: null as unknown, response_error: null };
    if (call.command === "get_routing") {
      const destination = call.options?.["ext3"] as number;
      envelope.response = { bank_idx: destination, bank_configs: group(destination).map(([p, c]) => ({ in_periph_id: p, in_chann: c })) };
    } else if (call.command === "set_routing") {
      const args = call.args as { bank_idx: number; bank_configs: Uint8Array[] };
      routes.set(args.bank_idx, args.bank_configs.map((b): Pair => [b[0] as number, b[1] as number]));
    }
    return envelope;
  };
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), themeSources: builtInThemes });
  await store.start();
  const at = (destination: number, slot: number) => group(destination)[slot];
  const set = (destination: number, slot: number, pair: Pair) => {
    group(destination)[slot] = pair;
  };
  return { store, at, set };
}

test("a drag carries its device and sources under its own type, and only well-formed data reads back", () => {
  const drag = { deviceId: Q, sources: [{ group: PREAMP, channel: 0 }, { group: PREAMP, channel: 1 }] };
  assert.deepEqual(decodeSourceDrag(encodeSourceDrag(drag)), drag);
  for (const bad of ["", "not json", "null", "[]", '{"deviceId":"","sources":[{"group":0,"channel":0}]}', `{"deviceId":"${Q}","sources":[]}`, `{"deviceId":"${Q}","sources":[{"group":-1,"channel":0}]}`, `{"deviceId":"${Q}","sources":[{"group":0,"channel":1.5}]}`, `{"deviceId":"${Q}","sources":[null]}`]) {
    assert.equal(decodeSourceDrag(bad), undefined, bad);
  }
  assert.equal(carriesSources([SOURCE_MIME, "text/plain"]), true);
  assert.equal(carriesSources(["text/plain", "Files"]), false, "another drag, a file say, is not ours");
  assert.equal(carriesSources(undefined), false);
  assert.equal(dropHint(1, "Monitors"), "Drop to add a channel to Monitors");
  assert.equal(dropHint(3, "Cue"), "Drop to add 3 channels to Cue");
});

test("a drop adds one channel per source, each fed by it with the dock's mix as its main mix, routed as the Mixer page routes it", async () => {
  const { store, at } = await setup();
  const channels = store.channels(Q);
  const ids = await addDroppedSources(store, Q, 1, [{ group: PREAMP, channel: 2 }, { group: USB1, channel: 0 }]);
  assert.equal(ids.length, 2);
  assert.deepEqual(
    channels.layout.value.channels.map(({ id: _, ...rest }) => rest),
    [
      { name: "", slot: 6, source: { group: PREAMP, channel: 2 }, main_mix: 1, sends: [] },
      { name: "", slot: 7, source: { group: USB1, channel: 0 }, main_mix: 1, sends: [] },
    ],
  );
  assert.deepEqual([at(MIX[1] as number, 6), at(MIX[1] as number, 7)], [[PREAMP, 2], [USB1, 0]], "routed into mix 2");
  assert.deepEqual([at(MIX[0] as number, 6), at(MIX[2] as number, 7)], [[MUTE, 0], [MUTE, 0]], "and nowhere else");
  assert.deepEqual(
    channels.inMix(1).map((c) => channels.displayName(c)),
    ["PREAMP 3", "USB 1 PLAY 1"],
    "the dock's strips, named by their inputs",
  );
});

test("a drop stops at a full mixer, and a source the device does not have is refused", async () => {
  const { store } = await setup();
  const channels = store.channels(Q);
  for (let i = 0; i < 25; i++) channels.add();
  const ids = await addDroppedSources(store, Q, 0, [{ group: PREAMP, channel: 0 }, { group: PREAMP, channel: 1 }]);
  assert.equal(ids.length, 1, "one slot was left");
  assert.match(JSON.stringify(store.notices.value), /All 26 mixer channels are in use/);
  await assert.rejects(channels.addFed({ group: MUTE, channel: 0 }, 0), RangeError);
  await assert.rejects(channels.addFed({ group: PREAMP, channel: 4 }, 0), RangeError, "the Quadro has four preamps");
  await assert.rejects(channels.addFed({ group: PREAMP, channel: 0 }, 4), RangeError, "and four mixes");
});

test("a drop asks first where it would put an input into the mix twice, in the Input menu's words", async () => {
  const { store } = await setup();
  await addDroppedSources(store, Q, 0, [{ group: PREAMP, channel: 0 }]);
  const both = [{ group: PREAMP, channel: 0 }, { group: PREAMP, channel: 1 }];
  assert.deepEqual(doublingsOf(store, Q, 0, both), ["PREAMP 1 is in this mix twice, so it is summed twice (about +6 dB)"]);
  assert.deepEqual(doublingsOf(store, Q, 1, both), [], "another mix has neither");
  assert.deepEqual(doublingsOf(store, Q, 0, [{ group: USB1, channel: 3 }]), []);
});

test("a device with no layout first takes the one its routing gives, so a drop cannot double what is already there", async () => {
  const { store, set } = await setup();
  set(MIX[0] as number, 6, [PREAMP, 1]);
  const channels = store.channels(Q);
  assert.equal(channels.configured, false);
  assert.deepEqual(doublingsOf(store, Q, 0, [{ group: PREAMP, channel: 1 }]), [], "without a layout nothing is known");
  await layoutForDrop(store, Q);
  assert.equal(channels.configured, true);
  assert.equal(doublingsOf(store, Q, 0, [{ group: PREAMP, channel: 1 }]).length, 1, "the imported channel is seen");
  const before = channels.layout.value.channels.length;
  await layoutForDrop(store, Q);
  assert.equal(channels.layout.value.channels.length, before, "a layout is never taken twice");
});
