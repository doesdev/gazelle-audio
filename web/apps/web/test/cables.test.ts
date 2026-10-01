import { test } from "node:test";
import assert from "node:assert/strict";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { channelSpan } from "../src/store/cables.ts";
import { Store } from "../src/store/store.ts";
import { builtInThemes, device, FakeClient, flush, MemoryStorage, type Invocation } from "./fake-client.ts";

// Topology positions used below. Quadro inputs: PREAMP 0, COM_PLAY (USB 1 PLAY) 1, ADAT_IN 3,
// SPDIF_IN 4, MIXER_OUT 6..9, MUTE 10; outputs: SPDIF_OUT 6. Studio+ inputs: PREAMP 0, ADAT_IN 4,
// SPDIF_IN 5, MIXER_OUT 7..10, MUTE 11; outputs: ADAT_OUT 7, SPDIF_OUT 8.
const QUADRO = "loopback-0";
const STUDIO = "loopback-1";

function setup() {
  const client = new FakeClient(device(STUDIO, "studio", "Zen Studio+"), device(QUADRO, "quadro", "Zen Quadro"));
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), requestFrame: (callback) => callback(), themeSources: builtInThemes });
  return { client, store };
}

/** Answers `get_routing` for the given groups (by device and destination), as a live device would. */
function answerRouting(client: FakeClient, routes: Record<string, Record<number, [number, number][]>>): void {
  client.respond = async (call: Invocation) => {
    const slots = call.command === "get_routing" ? routes[call.deviceId]?.[Number(call.options?.["ext3"])] : undefined;
    const response = slots === undefined ? null : { bank_configs: Array.from({ length: 32 }, (_, i) => ({ in_periph_id: slots[i]?.[0] ?? (call.deviceId === STUDIO ? 11 : 10), in_chann: slots[i]?.[1] ?? 0 })) };
    return { device_id: call.deviceId, command: call.command, sent_hex: "70", sent_len: 1, dry_run: false, response, response_error: null };
  };
}

const adat = { from: { device_id: STUDIO, port: "ADAT_OUT", first: 0 }, to: { device_id: QUADRO, port: "ADAT_IN", first: 0 }, channels: 8 } as const;
const spdif = { from: { device_id: QUADRO, port: "SPDIF_OUT", first: 0 }, to: { device_id: STUDIO, port: "SPDIF_IN", first: 0 }, channels: 2 } as const;

test("a run of channels reads as words, never with a dash: one, a pair, or a range", () => {
  assert.equal(channelSpan(3, 3), "3");
  assert.equal(channelSpan(1, 2), "1 and 2");
  assert.equal(channelSpan(9, 16), "9 to 16");
});

test("cables are declared between two devices' digital ports of one kind, checked as the server checks them, and removed", async () => {
  const { client, store } = setup();
  await store.start();
  const cables = store.cables;
  const id = cables.declare(adat.from, adat.to, adat.channels) as string;
  const other = cables.declare(spdif.from, spdif.to, spdif.channels) as string;
  assert.notEqual(id, other);
  assert.deepEqual(store.workspace.value?.cables, [{ id, ...adat }, { id: other, ...spdif }]);
  assert.equal(cables.label(cables.list.value[0]!), "Zen Studio+ ADAT Out 1 to 8 → Zen Quadro ADAT In 1 to 8");
  assert.equal(cables.label(cables.list.value[1]!), "Zen Quadro S/PDIF Out 1 and 2 → Zen Studio+ S/PDIF In 1 and 2");

  assert.throws(() => cables.declare({ device_id: STUDIO, port: "ADAT_IN", first: 0 }, adat.to, 8), /from must be SPDIF_OUT or ADAT_OUT/);
  assert.throws(() => cables.declare(adat.from, { device_id: QUADRO, port: "SPDIF_OUT", first: 0 }, 2), /to must be SPDIF_IN or ADAT_IN/);
  assert.throws(() => cables.declare(spdif.from, { device_id: STUDIO, port: "ADAT_IN", first: 0 }, 2), /SPDIF_OUT cannot feed ADAT_IN/);
  assert.throws(() => cables.declare({ device_id: STUDIO, port: "SPDIF_OUT", first: 0 }, { device_id: STUDIO, port: "SPDIF_IN", first: 0 }, 2), /joins two devices/);
  assert.throws(() => cables.declare(spdif.from, spdif.to, 3), /1\.\.2 channels, not 3/);
  assert.throws(() => cables.declare(adat.from, adat.to, 0), /1\.\.8 channels, not 0/);
  assert.throws(() => cables.declare({ device_id: QUADRO, port: "ADAT_OUT", first: 0 }, { device_id: STUDIO, port: "ADAT_IN", first: 0 }, 8), /the quadro has no ADAT_OUT/);
  assert.throws(() => cables.declare({ device_id: STUDIO, port: "ADAT_OUT", first: 8 }, { device_id: QUADRO, port: "ADAT_IN", first: 4 }, 8), /the quadro's ADAT_IN has channels 0\.\.7, not 4\.\.11/);
  assert.throws(() => cables.declare({ device_id: STUDIO, port: "ADAT_OUT", first: 8 }, { device_id: QUADRO, port: "ADAT_IN", first: 1 }, 8), /not 1\.\.8/);
  // A device that is not attached keeps what it names, as on the server.
  assert.ok(cables.declare({ device_id: STUDIO, port: "ADAT_OUT", first: 8 }, { device_id: "usb:gone", port: "ADAT_IN", first: 8 }, 8));

  assert.deepEqual(cables.portsOf(QUADRO, "out"), ["SPDIF_OUT"]);
  assert.deepEqual(cables.portsOf(STUDIO, "out"), ["SPDIF_OUT", "ADAT_OUT"]);
  assert.deepEqual(cables.portsOf(QUADRO, "in"), ["SPDIF_IN", "ADAT_IN"]);

  assert.equal(cables.remove(other), true);
  assert.equal(cables.remove(other), false);
  assert.equal(cables.list.value.length, 2);
  assert.deepEqual(client.invocations, [], "declaring a cable sends nothing to a device");
});

test("an output port shows what feeds it, pair by pair: a mix, a source played bit for bit, or not read", async () => {
  const { client, store } = setup();
  await store.start();
  answerRouting(client, {
    [QUADRO]: { 6: [[9, 0], [9, 1]] },
    [STUDIO]: { 7: [[0, 0], [0, 1], [0, 2], [11, 0], [9, 1], [9, 0]] },
  });
  const cables = store.cables;
  assert.deepEqual(cables.feed(QUADRO, "SPDIF_OUT", 0).map((p) => p.text), ["Not read"]);
  await store.readRoutes(QUADRO, [6]);
  await store.readRoutes(STUDIO, [7]);
  store.channels(QUADRO).renameMix(3, "Music");

  assert.deepEqual(cables.feed(QUADRO, "SPDIF_OUT", 0), [{ channel: 0, label: "1/2", text: "Music", mix: 3, direct: false, known: true }]);
  const studio = cables.feed(STUDIO, "ADAT_OUT", 0);
  assert.equal(studio.length, 4, "an ADAT port is four pairs");
  assert.deepEqual(studio.map((p) => [p.label, p.text, p.mix, p.direct]), [
    ["1/2", "Preamp 1/2", undefined, true],
    ["3/4", "Preamp 3 + Mute", undefined, true],
    ["5/6", "Mix 3 R + Mix 3 L", undefined, true],
    ["7/8", "Muted", undefined, false],
  ]);
  assert.deepEqual(cables.feed(STUDIO, "ADAT_OUT", 8).map((p) => p.label), ["9/10", "11/12", "13/14", "15/16"]);
});

test("routing a mix, a source or nothing to a port's pair is one read-before-write set_routing on the device that owns the port", async () => {
  const { client, store } = setup();
  await store.start();
  const cables = store.cables;
  const choices = cables.routeChoices(QUADRO);
  assert.deepEqual(choices.mixes.map((m) => m.label).slice(0, 2), ["Mix 1", "Mix 2"]);
  assert.ok(choices.sources.some((s) => s.label === "USB 1 Play 1/2" && s.group === 1 && s.channel === 0));
  assert.ok(!choices.sources.some((s) => s.label.startsWith("Mix") || s.label.startsWith("Loopback") || s.label.startsWith("Mute")), "mixes are offered by name, and mute on its own");
  assert.ok(choices.sources.some((s) => s.label === "S/PDIF In 1/2"));

  const sets = () => client.invocations.filter((call) => call.command === "set_routing");
  assert.equal(await cables.routePair(QUADRO, "SPDIF_OUT", 0, { mix: 3 }), true);
  const [read, write] = client.invocations.slice(-2);
  assert.equal(read?.command, "get_routing", "the group is read first");
  assert.equal(read?.options?.["ext3"], 6);
  assert.equal(write?.command, "set_routing");
  assert.equal(write?.deviceId, QUADRO);
  assert.equal(write?.args?.["bank_idx"], 6);
  const slots = (write?.args?.["bank_configs"] as Uint8Array[]).map((s) => [...s]);
  assert.deepEqual(slots.slice(0, 3), [[9, 0], [9, 1], [10, 0]]);
  assert.equal(slots.length, 32);
  assert.deepEqual(cables.feed(QUADRO, "SPDIF_OUT", 0).map((p) => [p.text, p.mix]), [["Mix 4", 3]]);

  await cables.routePair(STUDIO, "ADAT_OUT", 2, { group: 0, channel: 2 });
  assert.equal(sets().at(-1)?.deviceId, STUDIO);
  assert.equal(sets().at(-1)?.args?.["bank_idx"], 7);
  assert.deepEqual((sets().at(-1)?.args?.["bank_configs"] as Uint8Array[]).slice(0, 4).map((s) => [...s]), [[11, 0], [11, 0], [0, 2], [0, 3]]);
  assert.deepEqual(cables.feed(STUDIO, "ADAT_OUT", 0)[1]?.text, "Preamp 3/4");

  await cables.routePair(QUADRO, "SPDIF_OUT", 0, { mute: true });
  assert.deepEqual((sets().at(-1)?.args?.["bank_configs"] as Uint8Array[]).slice(0, 2).map((s) => [...s]), [[10, 0], [10, 0]]);
  assert.equal(cables.feed(QUADRO, "SPDIF_OUT", 0)[0]?.text, "Muted");
  // A source with no channel after the one chosen plays that channel on both sides.
  await cables.routePair(QUADRO, "SPDIF_OUT", 0, { group: 1, channel: 15 });
  assert.deepEqual((sets().at(-1)?.args?.["bank_configs"] as Uint8Array[]).slice(0, 2).map((s) => [...s]), [[1, 15], [1, 15]]);

  assert.throws(() => cables.routePair(QUADRO, "SPDIF_OUT", 2, { mute: true }), RangeError, "the Quadro's S/PDIF Out has one pair");
  assert.throws(() => cables.routePair(STUDIO, "ADAT_OUT", 1, { mute: true }), RangeError, "a pair starts on an even channel");
  assert.throws(() => cables.routePair(QUADRO, "ADAT_OUT" as never, 0, { mute: true }), RangeError);
});

test("a receiving input says where its signal comes from, through the cable and the sender's routing", async () => {
  const { client, store } = setup();
  await store.start();
  answerRouting(client, { [STUDIO]: { 7: [[0, 0], [0, 1], [0, 2]] } });
  const cables = store.cables;
  store.renameDevice(STUDIO, "Drum rack");
  cables.declare({ device_id: STUDIO, port: "ADAT_OUT", first: 0 }, { device_id: QUADRO, port: "ADAT_IN", first: 2 }, 6);
  store.channels(STUDIO).add();
  const snare = store.channels(STUDIO).layout.value.channels[0]!.id;
  store.channels(STUDIO).rename(snare, "Snare");
  await store.channels(STUDIO).setSource(snare, { group: 0, channel: 1 });

  assert.equal(cables.provenance(QUADRO, "ADAT_IN", 1), undefined, "ADAT In 2 is before the cable's first channel");
  assert.equal(cables.provenance(QUADRO, "SPDIF_IN", 2), undefined);
  assert.equal(cables.provenance(QUADRO, "ADAT_IN", 3)?.text, "from Drum rack ADAT Out 2", "the sender's routing is not read yet");
  assert.deepEqual(cables.sendersToRead(QUADRO, "ADAT_IN", 3), { deviceId: STUDIO, destination: 7 });
  await store.readRoutes(STUDIO, [7]);
  assert.equal(cables.provenance(QUADRO, "ADAT_IN", 2)?.text, "from Drum rack ADAT Out 1 ← Preamp 1");
  assert.equal(cables.provenance(QUADRO, "ADAT_IN", 3)?.text, "from Drum rack ADAT Out 2 ← Preamp 2 (Snare)");
  assert.equal(cables.provenance(QUADRO, "ADAT_IN", 5)?.text, "from Drum rack ADAT Out 4 ← Mute");
  assert.equal(cables.provenance(QUADRO, "ADAT_IN", 7)?.text, "from Drum rack ADAT Out 6 ← Mute");
  assert.equal(cables.provenance(QUADRO, "ADAT_IN", 7)?.cable.to.first, 2);
  assert.equal(cables.provenance(QUADRO, "ADAT_IN", 8), undefined, "ADAT In 9 is past the cable's last channel");
  await flush();
});

test("a cable warns when the two clocks disagree, the receiver is not locked, or the sender has signal the receiver does not", async () => {
  const { client, store } = setup();
  await store.start();
  answerRouting(client, { [STUDIO]: { 7: [[0, 0], [0, 1]] } });
  const cables = store.cables;
  const id = cables.declare({ device_id: STUDIO, port: "ADAT_OUT", first: 0 }, { device_id: QUADRO, port: "ADAT_IN", first: 0 }, 2) as string;
  const cable = () => cables.list.value.find((c) => c.id === id)!;
  const offs = [store.watchReport(STUDIO, "0x73"), store.watchReport(QUADRO, "0x73")];
  const report = (deviceId: string, fields: Record<string, unknown>) => client.cyclic.get(`${deviceId}|0x73`)?.(fields);

  assert.deepEqual(cables.health(cable()), [], "nothing is said before either device reports");
  report(STUDIO, { power_on: 1, base_index: 4, locked_wc: 1, peaks_preamp: Uint8Array.of(20, 90, 90, 90) });
  report(QUADRO, { power_on: 1, base_index: 2, locked: 0, peaks_adat: Uint8Array.of(90, 90, 90, 90, 90, 90, 90, 90) });
  assert.deepEqual(cables.health(cable()), ["The sample rates differ: 96 kHz on Zen Studio+, 48 kHz on Zen Quadro.", "Zen Quadro is not locked to its clock."]);

  report(QUADRO, { base_index: 4, locked: 1 });
  assert.deepEqual(cables.health(cable()), [], "the sender's routing is not read, so its signal is not known");
  await store.readRoutes(STUDIO, [7]);
  assert.deepEqual(cables.health(cable()), ["Signal leaves Zen Studio+ ADAT Out 1 but none arrives at Zen Quadro ADAT In 1: check the cable, the routing and the clock."]);
  report(QUADRO, { peaks_adat: Uint8Array.of(24, 90, 90, 90, 90, 90, 90, 90) });
  assert.deepEqual(cables.health(cable()), [], "signal arrives");
  for (const off of offs) off();
});

test("the S/PDIF converter answers the clock warnings: with it on, rates need not match and the receiver need not lock", async () => {
  const { client, store } = setup();
  await store.start();
  const cables = store.cables;
  // The Quadro's S/PDIF Out into the Studio+, whose converter can take another rate (the user, 2026-09-19).
  const id = cables.declare(spdif.from, spdif.to, spdif.channels) as string;
  const cable = () => cables.list.value.find((c) => c.id === id)!;
  const offs = [store.watchReport(STUDIO, "0x73"), store.watchReport(QUADRO, "0x73")];
  const report = (deviceId: string, fields: Record<string, unknown>) => client.cyclic.get(`${deviceId}|0x73`)?.(fields);

  report(QUADRO, { power_on: 1, base_index: 4, locked: 1 });
  report(STUDIO, { power_on: 1, base_index: 2, locked_wc: 0, spdif_src: 0 });
  assert.deepEqual(
    cables.health(cable()),
    ["The sample rates differ: 96 kHz on Zen Quadro, 48 kHz on Zen Studio+.", "Zen Studio+ is not locked to its clock."],
    "with the converter off, both are worth saying",
  );

  report(STUDIO, { spdif_src: 1 });
  assert.deepEqual(cables.health(cable()), [], "with the converter on, the rate is converted and the clock need not follow");

  // It only answers for the S/PDIF input it converts: an ADAT cable into the same device still warns.
  const adatId = cables.declare(adat.from, adat.to, adat.channels) as string;
  const adatCable = () => cables.list.value.find((c) => c.id === adatId)!;
  report(QUADRO, { locked: 0 });
  assert.deepEqual(
    cables.health(adatCable()),
    ["The sample rates differ: 48 kHz on Zen Studio+, 96 kHz on Zen Quadro.", "Zen Quadro is not locked to its clock."],
    "the converter is on the Studio+'s S/PDIF input, not on the Quadro's ADAT input",
  );
  for (const off of offs) off();
});

test("a channel on a cable's input is named through it: the sender's channel, a mix, or the source, and plain while unread or muted", async () => {
  const { client, store } = setup();
  await store.start();
  // Studio+ ADAT Out 1..5: Preamp 1, USB Play 3, Mix 2 L, Mute, Preamp 1 again.
  answerRouting(client, { [STUDIO]: { 7: [[0, 0], [3, 2], [8, 0], [11, 0], [0, 0]] } });
  store.renameDevice(STUDIO, "Live room");
  store.cables.declare(adat.from, adat.to, 6);
  const studio = store.channels(STUDIO);
  const kick = studio.add() as string;
  studio.rename(kick, "Kick");
  studio.setChannelColor(kick, "#aa3311");
  await studio.setSource(kick, { group: 0, channel: 0 });
  studio.renameMix(1, "Cue");

  const quadro = store.channels(QUADRO);
  const ids: string[] = [];
  for (let k = 0; k < 6; k++) {
    const id = quadro.add() as string;
    await quadro.setSource(id, { group: 3, channel: k });
    ids.push(id);
  }
  const channel = (k: number) => quadro.channel(ids[k]!)!;
  const names = () => ids.map((_, k) => quadro.displayName(channel(k)));

  // Not read yet: the plain input labels, with where they come from all the same.
  assert.deepEqual(names(), ["ADAT In 1", "ADAT In 2", "ADAT In 3", "ADAT In 4", "ADAT In 5", "ADAT In 6"]);
  assert.equal(quadro.detail(channel(0)), "from Live room, ADAT In 1");
  assert.equal(store.cables.band(store.cables.list.value[0]!), "From Live room (ADAT)");
  assert.deepEqual(store.cables.sendersInto(QUADRO), [{ deviceId: STUDIO, destination: 7 }]);
  assert.deepEqual(store.cables.sendersInto(STUDIO), []);

  await store.readRoutes(STUDIO, [7]);
  // The sender's channel by name, a playback channel and a mix by their labels, Mute and silence plain.
  assert.deepEqual(names(), ["Kick", "USB Play 3", "Cue L", "ADAT In 4", "Kick", "ADAT In 6"]);
  assert.equal(quadro.through({ group: 3, channel: 3 })?.name, undefined, "Mute names nothing");
  assert.equal(quadro.through({ group: 3, channel: 6 }), undefined, "ADAT In 7 is past the cable");
  assert.equal(quadro.through({ group: 4, channel: 0 }), undefined, "no cable brings S/PDIF In");
  assert.equal(quadro.through({ group: 0, channel: 0 }), undefined, "a preamp is no cable's");
  // The sender's channel colour, under the receiver's own.
  assert.deepEqual(quadro.strip(channel(0), 0), { label: "Kick", color: "#aa3311", source: { group: 3, channel: 0 }, inMix: false, detail: "from Live room, ADAT In 1" });
  assert.equal(quadro.color(channel(1), []).from, "input", "no sender channel, no sender colour: its own input's");

  // It follows the sender's layout and name as they change.
  studio.rename(kick, "Kick In");
  store.renameDevice(STUDIO, "Drum room");
  assert.equal(quadro.displayName(channel(0)), "Kick In");
  assert.equal(quadro.detail(channel(0)), "from Drum room, ADAT In 1");

  // A name the user gave wins, and the through name moves to the tooltip. So do their colours.
  quadro.rename(ids[0]!, "Bass drum");
  quadro.setChannelColor(ids[0]!, "#00ff00");
  assert.equal(quadro.displayName(channel(0)), "Bass drum");
  assert.equal(quadro.detail(channel(0)), "Kick In, from Drum room, ADAT In 1");
  assert.deepEqual(quadro.color(channel(0), []), { color: "#00ff00", from: "custom" });
  quadro.setChannelColor(ids[0]!, undefined);
  assert.deepEqual(quadro.color(channel(0), []), { color: "#aa3311", from: "cable" });
  const group = quadro.addGroup("Drums", [ids[0]!]) as string;
  quadro.setGroupColor(group, "#0000ff");
  assert.deepEqual(quadro.color(channel(0), []), { color: "#0000ff", from: "group" });

  // A cable dedicated to the phase measurement names nothing: the Mixer guards it instead.
  store.editWorkspace((workspace) => ({ ...workspace, cables: (workspace.cables ?? []).map((cable) => ({ ...cable, dedicated: { phase_output: 0, phase_input: 0 } })) }));
  assert.deepEqual(names(), ["Bass drum", "ADAT In 2", "ADAT In 3", "ADAT In 4", "ADAT In 5", "ADAT In 6"]);
  assert.equal(quadro.detail(channel(1)), undefined);
  assert.deepEqual(store.cables.sendersInto(QUADRO), []);
  await flush();
});

test("naming through works the other way too: the Quadro's S/PDIF Out names the Studio+'s S/PDIF In channels", async () => {
  const { client, store } = setup();
  await store.start();
  // Quadro S/PDIF Out: Preamp 2, then Mix 4's right side.
  answerRouting(client, { [QUADRO]: { 6: [[0, 1], [9, 1]] } });
  store.renameDevice(QUADRO, "Desk");
  store.cables.declare(spdif.from, spdif.to, 2);
  const vox = store.channels(QUADRO).add() as string;
  store.channels(QUADRO).rename(vox, "Vox");
  await store.channels(QUADRO).setSource(vox, { group: 0, channel: 1 });
  store.channels(QUADRO).renameMix(3, "Music");
  await store.readRoutes(QUADRO, [6]);

  const studio = store.channels(STUDIO);
  assert.equal(studio.through({ group: 5, channel: 0 })?.name, "Vox");
  assert.equal(studio.through({ group: 5, channel: 0 })?.sender, "Desk");
  assert.equal(studio.through({ group: 5, channel: 1 })?.name, "Music R");
  const id = studio.add() as string;
  await studio.setSource(id, { group: 5, channel: 0 });
  assert.equal(studio.displayName(studio.channel(id)!), "Vox");
  assert.equal(studio.detail(studio.channel(id)!), "from Desk, S/PDIF In 1");
  assert.equal(store.cables.band(store.cables.list.value[0]!), "From Desk (S/PDIF)");
  await flush();
});
