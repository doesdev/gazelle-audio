import { test } from "node:test";
import assert from "node:assert/strict";

import type { MixerChannel } from "gazelle-audio-client";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { LEVEL_MAX, PAN_MAX } from "../src/store/mixer.ts";
import { SoftLinkModel } from "../src/store/soft-link.ts";
import { Store } from "../src/store/store.ts";
import { builtInThemes, device, FakeClient, flush, MemoryStorage } from "./fake-client.ts";

const Q = "loopback-0";
const S = "loopback-1";

/** A started store whose Quadro has channels on slots 6 to 10, all in mix 1 (index 0); slot 10 is in mix 2 only. */
async function setup() {
  const client = new FakeClient(device(Q, "quadro", "Zen Quadro"), device(S, "studio", "Zen Studio+"));
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), themeSources: builtInThemes });
  await store.start();
  const channel = (slot: number, mix = 0): MixerChannel => ({ id: `c${slot}`, name: "", slot, source: { group: 0, channel: slot - 6 }, main_mix: mix, sends: [] });
  assert.ok(store.editWorkspace((w) => ({ ...w, mixers: { [Q]: { mixes: [], groups: [], channels: [channel(6), channel(7), channel(8), channel(9), channel(10, 1)] } } })));
  const mixer = store.mixer(Q, 0);
  /** set_mixer as [channel, level, pan, mute, solo], device channel = slot + 1. */
  const sent = () => client.invocations.filter((c) => c.command === "set_mixer").map((c) => [c.args?.["channel"], c.args?.["level"], c.args?.["pan"], c.args?.["mute"], c.args?.["solo"]]);
  const reset = async () => {
    await flush();
    client.invocations.length = 0;
  };
  return { client, store, mixer, sent, reset };
}

test("selecting: a click selects one, Ctrl toggles, Shift takes the range in the order shown; one channel alone links nothing", () => {
  const soft = new SoftLinkModel();
  const order = [6, 7, 8, 9];
  soft.select(Q, 7, "only", order);
  assert.deepEqual([soft.selection.value?.slots, soft.linked.value, soft.peers(Q, 7)], [[7], 0, []]);
  soft.select(Q, 9, "range", order);
  assert.deepEqual(soft.selection.value?.slots, [7, 8, 9]);
  assert.equal(soft.linked.value, 3);
  assert.deepEqual(soft.peers(Q, 8), [7, 9]);
  assert.deepEqual(soft.peers(Q, 6), [], "a change to a channel outside the selection is its own");
  soft.select(Q, 8, "toggle", order);
  assert.deepEqual(soft.selection.value?.slots, [7, 9]);
  soft.select(Q, 6, "only", order);
  assert.deepEqual(soft.selection.value?.slots, [6]);
  soft.select(Q, 6, "only", order);
  assert.equal(soft.selection.value, undefined, "a click on the only one selected clears it");
  soft.select(Q, 6, "toggle", order);
  soft.select(S, 1, "toggle", [0, 1]);
  assert.deepEqual(soft.selection.value, { deviceId: S, slots: [1] }, "one device at a time: another device's selection starts over");
  assert.equal(soft.selected(Q, 6), false);
  soft.clear();
  assert.equal(soft.selection.value, undefined);
});

test("a fader moved on one soft-linked channel moves the others by the same step, each held inside its range, with the commands their own faders send", async () => {
  const { store, mixer, sent, reset } = await setup();
  mixer.setLevel(6, 10);
  mixer.setLevel(7, 20);
  mixer.setLevel(8, 86);
  mixer.setLevel(9, 40);
  await reset();
  store.softLink.select(Q, 6, "only", [6, 7, 8]);
  store.softLink.select(Q, 8, "range", [6, 7, 8]);

  mixer.setLevel(6, 16, true);
  await flush();
  assert.deepEqual(sent(), [
    [7, 16, 32, 0, 0],
    [8, 26, 32, 0, 0],
    [9, LEVEL_MAX, 32, 0, 0],
  ], "6 dB down each; slot 8 stops at the bottom of its range");
  assert.equal(mixer.strip(9).peek().level, 40, "slot 9 is not selected");

  await reset();
  mixer.setLevel(7, 0, true);
  await flush();
  assert.deepEqual(sent().map(([channel, level]) => [channel, level]), [[8, 0], [7, 0], [9, 64]], "26 dB up from 26: the others too, held at 0 dB");

  await reset();
  mixer.setLevel(6, 30);
  await flush();
  assert.deepEqual(sent().length, 1, "a control that does not ask for it (the Remote page's) moves only its own strip");
});

test("pan moves by the same step, held per channel; mute and solo take the clicked channel's new state", async () => {
  const { store, mixer, sent, reset } = await setup();
  mixer.setPan(6, 40);
  mixer.setPan(7, 58);
  mixer.toggleMute(8);
  await reset();
  for (const slot of [6, 7, 8]) store.softLink.select(Q, slot, "toggle");

  mixer.setPan(6, 46, true);
  await flush();
  assert.deepEqual(sent().map(([channel, , pan]) => [channel, pan]), [[7, 46], [8, PAN_MAX], [9, 38]]);

  await reset();
  mixer.toggleMute(6, true);
  await flush();
  assert.deepEqual(sent().map(([channel, , , mute]) => [channel, mute]), [[7, 1], [8, 1], [9, 1]], "slot 8 was muted already and stays muted");
  await reset();
  mixer.toggleSolo(7, true);
  mixer.toggleSolo(7, true);
  await flush();
  assert.deepEqual(sent().map(([channel, , , , solo]) => [channel, solo]), [[8, 1], [7, 1], [9, 1], [8, 0], [7, 0], [9, 0]]);
});

test("only channels in the mix being changed follow, and a mono mix keeps the soft-linked pans for later", async () => {
  const { store, mixer, sent, reset } = await setup();
  for (const slot of [6, 10]) store.softLink.select(Q, slot, "toggle");
  mixer.setLevel(6, 5, true);
  await flush();
  assert.deepEqual(sent().map(([channel]) => channel), [7], "slot 10 is in another mix");

  store.softLink.select(Q, 7, "toggle");
  mixer.setPan(7, 40);
  assert.ok(store.channels(Q).setMono(0, true));
  await reset();
  mixer.setPan(6, 36, true);
  await flush();
  assert.deepEqual(sent(), [], "nothing is sent while the mix is mono");
  assert.deepEqual([mixer.monoPan(6), mixer.monoPan(7)], [36, 44], "both are kept for when mono ends, 4 steps right");
});

test("a channel in a saved link and the soft link moves once; the saved link's other members follow the channels that moved", async () => {
  const { store, mixer, sent, reset } = await setup();
  // 6 and 7 are linked relative, 8 and 9 absolute; 6, 7 and 8 are soft-linked, 9 is not.
  mixer.setLevel(7, 10);
  await reset();
  store.links.create("mixer", [{ device_id: Q, channel: 6 }, { device_id: Q, channel: 7 }], "relative");
  store.links.create("mixer", [{ device_id: Q, channel: 8 }, { device_id: Q, channel: 9 }], "absolute");
  await reset();
  for (const slot of [6, 7, 8]) store.softLink.select(Q, slot, "toggle");

  mixer.setLevel(6, 3, true);
  await flush();
  assert.deepEqual(sent().map(([channel, level]) => [channel, level]), [[7, 3], [8, 13], [9, 3], [10, 3]], "slot 7 moves 3 dB once, not 6; slot 9 follows slot 8 by its saved link");

  await reset();
  mixer.setLevel(9, 20, true);
  await flush();
  assert.deepEqual(sent().map(([channel, level]) => [channel, level]), [[10, 20], [9, 20]], "from outside the selection only the saved link follows");

  await reset();
  store.softLink.clear();
  mixer.setLevel(6, 0, true);
  await flush();
  assert.deepEqual(sent().map(([channel, level]) => [channel, level]), [[7, 0], [8, 10]], "cleared: the saved link alone");
});

test("the soft link is never saved in the workspace and never sets a device link flag", async () => {
  const { client, store, mixer } = await setup();
  const before = JSON.stringify(store.workspace.value);
  for (const slot of [6, 7]) store.softLink.select(Q, slot, "toggle");
  mixer.setLevel(6, 4, true);
  await flush();
  assert.equal(JSON.stringify({ ...store.workspace.value }), before);
  assert.deepEqual(client.invocations.filter((c) => c.command === "set_stereo_link"), []);
});
