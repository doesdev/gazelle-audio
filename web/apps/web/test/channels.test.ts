import { test } from "node:test";
import assert from "node:assert/strict";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import type { MixerChannel } from "gazelle-audio-client";
import { channelColor } from "../src/store/channels.ts";
import { PROFILES } from "../src/store/profiles.ts";
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

test("what feeds an output: unread until its routing is read, then the mixes whose left and right it takes and where else they play, or the sources it takes instead", async () => {
  const { client, store, set } = await setup();
  const channels = store.channels(Q);
  // Quadro destinations: LINE OUT 0, HP1 1, HP2 2, MONITOR 3, USB A REC 4; mix 1 plays from source 6, mix 2 from 7.
  const monitor = channels.outputFeed(3);
  assert.deepEqual(monitor.value, { state: "unread" });
  set(Q, 3, 0, [6, 0]);
  set(Q, 3, 1, [6, 1]);
  set(Q, 1, 0, [6, 0]);
  set(Q, 1, 1, [6, 1]);
  set(Q, 4, 2, [6, 0]);
  set(Q, 4, 3, [6, 1]);
  set(Q, 2, 0, [USB1, 0]);
  set(Q, 2, 1, [USB1, 1]);
  set(Q, 0, 0, [PREAMP, 2]);
  set(Q, 0, 1, [PREAMP, 2]);
  set(Q, 6, 0, [6, 0]); // S/PDIF OUT left only: not a whole pair, so mix 1 does not play there
  await store.readRoutes(Q, [3]);
  assert.deepEqual(monitor.value, { state: "mixes", mixes: [0], others: [] }, "only what has been read is named");
  await channels.loadOutputs();
  assert.deepEqual(monitor.value, { state: "mixes", mixes: [0], others: ["HP1", "USB A REC 3/4"] });
  assert.deepEqual(channels.outputFeed(1).value, { state: "mixes", mixes: [0], others: ["MONITOR", "USB A REC 3/4"] });
  assert.deepEqual(channels.outputFeed(2).value, { state: "none", sources: ["USB 1 PLAY 1", "USB 1 PLAY 2"] }, "played straight from USB");
  assert.deepEqual(channels.outputFeed(0).value, { state: "none", sources: ["PREAMP 3"] }, "one source on both sides is named once");
  assert.deepEqual(channels.outputFeed(5).value, { state: "none", sources: [] }, "muted in routing");
  assert.deepEqual(channels.outputFeed(6).value, { state: "none", sources: ["LOOPBACK HP1 1"] }, "half a mix is not a mix");

  // It follows routing changes.
  await channels.setMixOutput(1, { destination: 3, channel: 0 }, true);
  assert.deepEqual(monitor.value, { state: "mixes", mixes: [1], others: [] });
  assert.throws(() => channels.outputFeed(8), RangeError, "a mixer input is not an output");

  // The Studio+ Line out has four pairs, so more than one mix can feed it (mixes 1 and 2 play from sources 7 and 8).
  const studio = store.channels(S);
  set(S, 0, 0, [7, 0]);
  set(S, 0, 1, [7, 1]);
  set(S, 0, 4, [8, 0]);
  set(S, 0, 5, [8, 1]);
  set(S, 0, 6, [3, 5]);
  await store.readRoutes(S, [0]);
  assert.deepEqual(studio.outputFeed(0).value, { state: "mixes", mixes: [0, 1], others: [] });

  // A read with no reply (a dry run) leaves the feed unknown rather than unread.
  client.respond = async (call) => ({ device_id: call.deviceId, command: call.command, sent_hex: "70", sent_len: 16, dry_run: true, response: null, response_error: null });
  await store.readRoutes(S, [3]);
  assert.deepEqual(studio.outputFeed(3).value, { state: "unknown" });
});

test("placing a channel (a drop) sets its order and group together: between two members of a group it joins, anywhere else it has none", async () => {
  const { store } = await setup();
  const channels = store.channels(Q);
  const [a, b, c, d] = [channels.add(), channels.add(), channels.add(), channels.add()] as string[];
  const drums = channels.addGroup("Drums", [a as string, b as string]) as string;
  const view = () => channels.layout.value.channels.map((x) => `${x.id}${x.group === drums ? "*" : ""}`);
  const [A, B, C, D] = [a, b, c, d] as [string, string, string, string];

  assert.equal(channels.place(D, 1), true);
  assert.deepEqual(view(), [`${A}*`, `${D}*`, `${B}*`, C], "dropped between two drums, d joins the group");
  channels.place(A, 4);
  assert.deepEqual(view(), [`${D}*`, `${B}*`, C, A], "dropped after an ungrouped channel, a leaves the group");
  channels.place(C, 0);
  assert.deepEqual(view(), [C, `${D}*`, `${B}*`, A], "dropped at the edge of a group, c does not join it");
  channels.place(B, 0);
  assert.deepEqual(view(), [B, C, `${D}*`, A], "dragged out of its group, b leaves it");
  assert.throws(() => channels.place("nope", 0), RangeError);
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

test("a starting layout makes its channels from the first free slots, names its mixes and routes the device", async () => {
  const { store, at } = await setup();
  const channels = store.channels(Q);
  const tracking = PROFILES.quadro.find((p) => p.id === "tracking");
  assert.ok(tracking, "the Quadro has a tracking layout");
  assert.equal(await channels.applyProfile("tracking"), true);
  assert.deepEqual(
    channels.layout.value.channels.map((c) => [c.name, c.slot, c.source?.group, c.source?.channel, c.main_mix, c.sends]),
    [
      ["Preamp 1", 6, PREAMP, 0, 0, [1]],
      ["Preamp 2", 7, PREAMP, 1, 0, [1]],
      ["Preamp 3", 8, PREAMP, 2, 0, [1]],
      ["Preamp 4", 9, PREAMP, 3, 0, [1]],
      ["DAW L", 10, USB1, 0, 0, [1]],
      ["DAW R", 11, USB1, 1, 0, [1]],
    ],
  );
  assert.deepEqual(channels.layout.value.mixes.map((m) => m.name), ["Monitors", "Cue"]);
  assert.deepEqual([at(Q, MIX[0] as number, 6), at(Q, MIX[1] as number, 11), at(Q, MIX[2] as number, 6)], [[PREAMP, 0], [USB1, 1], [MUTE, 0]], "main mix and send routed, other mixes left muted");
});

test("a starting layout replaces only a mixer with no channel set up, and every layout's inputs exist on its device", async () => {
  const { store } = await setup();
  const channels = store.channels(Q);
  await channels.applyProfile("tracking");
  await assert.rejects(channels.applyProfile("playback"), /set up/);
  await assert.rejects(store.channels(S).applyProfile("no-such-layout"), RangeError);

  for (const [family, deviceId] of [["quadro", Q], ["studio", S]] as const) {
    const topology = store.topology(deviceId);
    assert.ok(topology);
    assert.ok(PROFILES[family].length >= 2, `${family} has layouts to choose from`);
    for (const profile of PROFILES[family]) {
      for (const channel of profile.channels) {
        const group = topology.inputs.find((g) => g.type === channel.input);
        assert.ok(group !== undefined && channel.channel < group.channels, `${family} ${profile.id}: ${channel.name} is on a real input`);
        assert.ok(channel.main_mix < topology.mixers.count && channel.sends.every((m) => m < topology.mixers.count && m !== channel.main_mix));
      }
      assert.ok(profile.channels.length <= 32 - (family === "quadro" ? 6 : 0), `${family} ${profile.id} fits the free slots`);
    }
  }
});

test("a mixer can be saved as a layout for its model, applied later like a starting layout, and removed", async () => {
  const { store, at } = await setup();
  const channels = store.channels(Q);
  await channels.applyProfile("tracking");
  const first = channels.layout.value.channels[0];
  assert.ok(first);
  channels.rename(first.id, "Vox");
  assert.throws(() => channels.saveLayout("  "), RangeError, "a layout needs a name");
  const id = channels.saveLayout("My session");
  assert.ok(id);
  assert.deepEqual(channels.savedLayouts().map((l) => [l.name, l.family, l.mixer.channels.length]), [["My session", "quadro", 6]]);
  assert.deepEqual(store.channels(S).savedLayouts(), [], "saved per model: the Studio+ does not offer a Quadro layout");

  await assert.rejects(channels.applySavedLayout(id), /set up/, "like starting layouts, it never replaces a working mixer");
  for (const c of [...channels.layout.value.channels]) await channels.remove(c.id);
  assert.equal(await channels.applySavedLayout(id), true);
  assert.deepEqual(channels.layout.value.channels.map((c) => [c.name, c.slot]).slice(0, 2), [["Vox", 6], ["Preamp 2", 7]]);
  assert.deepEqual(channels.layout.value.mixes.map((m) => m.name), ["Monitors", "Cue"]);
  assert.deepEqual(at(Q, MIX[0] as number, 6), [PREAMP, 0], "applying routes the channels");
  assert.ok(channels.layout.value.channels.every((c) => !channels.savedLayouts()[0]?.mixer.channels.some((s) => s.id === c.id)), "applied channels get fresh ids");

  assert.equal(channels.removeSavedLayout(id), true);
  assert.deepEqual(channels.savedLayouts(), []);
  await assert.rejects(channels.applySavedLayout(id), RangeError);
});

test("a channel with no name of its own is called by its input's name, and a typed name replaces it", async () => {
  const { store } = await setup();
  const channels = store.channels(Q);
  const id = channels.add() as string;
  const name = () => channels.displayName(channels.channel(id) as NonNullable<ReturnType<typeof channels.channel>>);

  // With no input yet there is nothing to borrow, so it is known by its mixer input.
  assert.equal(name(), "Ch 7");
  await channels.setSource(id, { group: PREAMP, channel: 2 });
  assert.equal(name(), channels.sourceLabel({ group: PREAMP, channel: 2 }));
  // It follows the input while the name is not set.
  await channels.setSource(id, { group: USB1, channel: 5 });
  assert.equal(name(), channels.sourceLabel({ group: USB1, channel: 5 }));

  channels.rename(id, "Kick");
  assert.equal(name(), "Kick");
  // Clearing the typed name hands the channel back to its input's name.
  channels.rename(id, "");
  assert.equal(name(), channels.sourceLabel({ group: USB1, channel: 5 }));
});

test("a mix's strips are the channels set up in it, main or send, in the Mixer page's order, each with its name, colour and input", async () => {
  const { store } = await setup();
  const channels = store.channels(Q);
  const [a, b, c, d, e] = [channels.add(), channels.add(), channels.add(), channels.add(), channels.add()] as string[];
  await channels.setSource(a as string, { group: PREAMP, channel: 0 });
  await channels.setMainMix(a as string, 0);
  await channels.setSource(b as string, { group: PREAMP, channel: 1 });
  await channels.setMainMix(b as string, 1);
  await channels.setSend(b as string, 0, true);
  await channels.setSource(c as string, { group: USB1, channel: 0 });
  await channels.setMainMix(c as string, 1);
  // d has an input but no main mix, and e a main mix but no input: neither feeds anything.
  await channels.setSource(d as string, { group: USB1, channel: 1 });
  await channels.setMainMix(e as string, 0);
  channels.move(b as string, 0);
  channels.rename(a as string, "Vox");
  const drums = channels.addGroup("Drums") as string;
  channels.setGroupColor(drums, "#b5473a");
  channels.setGroup(c as string, drums);

  const ids = (mix: number) => channels.inMix(mix).map((x) => x.id);
  assert.deepEqual(ids(0), [b, a], "a send counts, in layout order rather than slot order");
  assert.deepEqual(ids(1), [b, c]);
  assert.deepEqual(ids(2), []);
  assert.throws(() => channels.inMix(4), RangeError);

  const inputColour = (group: number) => store.topology(Q)?.inputs[group]?.color;
  const strip = (id: string, mix: number) => channels.strip(channels.channel(id) as NonNullable<ReturnType<typeof channels.channel>>, mix);
  assert.deepEqual(strip(a as string, 0), { label: "Vox", color: inputColour(PREAMP), source: { group: PREAMP, channel: 0 }, inMix: true });
  assert.deepEqual(strip(a as string, 1), { label: "Vox", color: inputColour(PREAMP), source: { group: PREAMP, channel: 0 }, inMix: false });
  assert.deepEqual(strip(b as string, 0), { label: channels.sourceLabel({ group: PREAMP, channel: 1 }), color: inputColour(PREAMP), source: { group: PREAMP, channel: 1 }, inMix: true }, "a send is in the mix");
  assert.deepEqual(strip(c as string, 1), { label: channels.sourceLabel({ group: USB1, channel: 0 }), color: "#b5473a", source: { group: USB1, channel: 0 }, inMix: true }, "a group's colour");
  assert.deepEqual(strip(d as string, 0), { label: channels.sourceLabel({ group: USB1, channel: 1 }), color: inputColour(USB1), source: { group: USB1, channel: 1 }, inMix: false }, "an inactive channel is in no mix");
  assert.equal(strip(e as string, 0).inMix, false, "not even its main mix, until it has an input");
});

test("a channel's own colour is set and cleared, and saved in the workspace; an unset colour is omitted", async () => {
  const { client, store, timers } = await setup();
  const channels = store.channels(Q);
  const a = channels.add() as string;
  const b = channels.add() as string;

  assert.equal(channels.setChannelColor(a, "#3fae6a"), true);
  assert.equal(channels.channel(a)?.color, "#3fae6a");
  assert.equal("color" in (channels.channel(b) ?? {}), false, "other channels keep no colour");
  timers.advance(1000);
  await flush();
  assert.equal(client.stored.mixers[Q]?.channels.find((c) => c.id === a)?.color, "#3fae6a");

  assert.equal(channels.setChannelColor(a, undefined), true);
  assert.equal("color" in (channels.channel(a) ?? {}), false, "a cleared colour is omitted, as the server omits it");
  timers.advance(1000);
  await flush();
  assert.equal("color" in (client.stored.mixers[Q]?.channels.find((c) => c.id === a) ?? {}), false);

  for (const bad of ["red", "#3fae6", "3fae6a0", "#3fae6g", "#3fae6a0", " #3fae6a"]) assert.throws(() => channels.setChannelColor(a, bad), RangeError, bad);
  assert.throws(() => channels.setChannelColor("nope", "#3fae6a"), RangeError);
});

test("a strip's colour is its group's, else the channel's own, else its input's routing colour, else the theme palette's", async () => {
  const { store } = await setup();
  const channels = store.channels(Q);
  const topology = store.topology(Q);
  assert.ok(topology !== undefined);
  const palette = store.theme.value.palette;
  const id = channels.add() as string; // slot 6
  const colour = () => channelColor(channels.channel(id) as MixerChannel, { groups: channels.layout.value.groups, inputs: topology.inputs, palette });

  // No input: the palette, by mixer input pair, as strips always were.
  assert.deepEqual(colour(), { color: palette[3 % palette.length], from: "palette" });
  await channels.setSource(id, { group: USB1, channel: 0 });
  const input = topology.inputs[USB1]?.color;
  assert.ok(input !== undefined && input !== palette[3 % palette.length], "the test needs an input colour unlike the palette's");
  assert.deepEqual(colour(), { color: input, from: "input" });

  channels.setChannelColor(id, "#123456");
  assert.deepEqual(colour(), { color: "#123456", from: "custom" });

  const drums = channels.addGroup("Drums", [id]) as string;
  assert.deepEqual(colour(), { color: "#123456", from: "custom" }, "a group without a colour leaves the channel's own");
  channels.setGroupColor(drums, "#b5473a");
  assert.deepEqual(colour(), { color: "#b5473a", from: "group" });
  channels.setChannelColor(id, undefined);
  assert.deepEqual(colour(), { color: "#b5473a", from: "group" });
  channels.setGroup(id, undefined);
  assert.deepEqual(colour(), { color: input, from: "input" }, "out of the group, with no colour of its own, it takes its input's again");

  // A source the topology does not have falls through to the palette; an empty palette gives no colour.
  const stray = { ...(channels.channel(id) as MixerChannel), source: { group: 99, channel: 0 } };
  assert.deepEqual(channelColor(stray, { groups: [], inputs: topology.inputs, palette }), { color: palette[3 % palette.length], from: "palette" });
  assert.deepEqual(channelColor({ ...stray, slot: 9 }, { groups: [], inputs: topology.inputs, palette: ["#000001", "#000002", "#000003"] }), { color: "#000002", from: "palette" }, "slots 8 and 9 are the fifth pair, and the palette wraps");
  assert.deepEqual(channelColor(stray, { groups: [], inputs: topology.inputs, palette: [] }), { color: undefined, from: "palette" });
});
