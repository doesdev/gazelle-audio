// The aggregate store: how often it asks the server, what it does when the server does not offer
// the routes at all, how a reason's `fix` becomes exactly one request, and what a gap, a stall and
// a buffer match read as. No server is started here and nothing reaches a device.

import { test } from "node:test";
import assert from "node:assert/strict";

import { GazelleError, type AggregateAnswer, type AggregateFix, type AggregateMatchBuffers, type AggregateRegistrationRun, type AggregateStatusReading } from "gazelle-audio-client";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import {
  interfaceChannels,
  AGGREGATE_LIVE_POLL_MS,
  AGGREGATE_OPEN_POLL_MS,
  AGGREGATE_POLL_MS,
  AggregateModel,
  autoChannelName,
  appliedTrimsText,
  buffersMatch,
  cablePort,
  cablingSteps,
  cardPhaseView,
  CHECK_TOLERANCE_SAMPLES,
  checkVerdicts,
  choicesWith,
  eventView,
  GAZELLE_MEASUREMENT,
  livePhaseView,
  masterIndex,
  phaseChoices,
  phaseFromPicks,
  phasePicks,
  phaseReference,
  phaseRefusedText,
  phaseRoutingNote,
  phaseSetting,
  phaseSetupView,
  reasonCard,
  reasonHint,
  referenceChanges,
  referenceText,
  runCleanText,
  runPhaseViews,
  withPass,
  withPhase,
  witnessViews,
  CALIBRATE_POLL_MS,
  calibrateDevices,
  calibrateProblem,
  calibrateProgress,
  calibrateRequest,
  calibrateRunning,
  calibrateStepText,
  channelCounts,
  channelLabel,
  channelSummary,
  CHANNEL_LABEL_MAX,
  defaultPicks,
  deviceName,
  deviceViews,
  driftFound,
  fixNeedsConfirming,
  fixRequest,
  gapView,
  isExposed,
  matchedBy,
  matchNote,
  matchBuffersText,
  matchTarget,
  namedCount,
  outcomeSummary,
  pollDelayMs,
  readingView,
  reconcilePicks,
  registrationText,
  resolvedDeviceId,
  slotDevice,
  trimRows,
  trimsToApply,
  statusLine,
  suggestedChannelName,
  viewFor,
  withChannelExposed,
  withChannelName,
  withMeasuredTrims,
  type AggregateCalibrateOutcome,
  type AggregateCalibrateReading,
  type AggregateCalibration,
  type AggregateContext,
  type AggregateDevice,
  type CalibratePicks,
} from "../src/store/aggregate.ts";
import { Store } from "../src/store/store.ts";
import { device, ends, FakeClient, flush, MemoryStorage } from "./fake-client.ts";

/** An en dash and an em dash, by code point, so this file carries neither. */
const DASHES = new RegExp(`[${String.fromCodePoint(0x2013, 0x2014)}]`, "u");

const silent: AggregateStatusReading = { state: "silent", message: "The aggregate driver is not loaded in any program." };

const live = (parts: Partial<Extract<AggregateStatusReading, { state: "read" }>> = {}): AggregateStatusReading => ({
  state: "read",
  open: true,
  streaming: true,
  generation: 3,
  generation_in_force: 3,
  up_to_date: true,
  plan: { master: "Quadro", rate: 96000, buffer_size: 256, inputs: 40, outputs: 40, alignment: "aligned", input_latency: 600, output_latency: 700 },
  devices: [],
  ...parts,
});

const liveDevice = (name: string, parts: Partial<Extract<AggregateStatusReading, { state: "read" }>["devices"][number]> = {}) => ({
  name,
  streaming: true,
  stalled: false,
  is_master: false,
  sample_gap: 0,
  callbacks: 100,
  dropped: 0,
  starved: 0,
  ...parts,
});

const answer = (parts: Partial<AggregateAnswer> = {}): AggregateAnswer => ({
  read_at_ms: 1_789_700_000_000,
  configured: true,
  export_path: "C:\\Users\\someone\\AppData\\Roaming\\gazelle\\aggregate.json",
  drivers: [],
  registration: {
    registered: true,
    clsid: "{0000}",
    name: "Gazelle Aggregate",
    dll: "C:\\Gazelle\\gazelle_aggregate.dll",
    dll_present: true,
    message: "Gazelle Aggregate is registered.",
    dll_search: { state: "found", dll: "C:\\Gazelle\\gazelle_aggregate.dll" },
  },
  devices: [],
  ready: true,
  reasons: [],
  status: silent,
  events: [],
  ...parts,
});

const report = (name: string, parts: Partial<AggregateAnswer["devices"][number]> = {}): AggregateAnswer["devices"][number] => ({
  name,
  registered: true,
  attached: true,
  driver: {},
  is_master: false,
  ...parts,
});

// ---------------------------------------------------------------------------------------------
// How often it asks
// ---------------------------------------------------------------------------------------------

test("the page asks every second while audio is running, and slowly while nothing has the driver", () => {
  assert.equal(pollDelayMs(live()), AGGREGATE_LIVE_POLL_MS);
  assert.equal(pollDelayMs(live({ streaming: false })), AGGREGATE_OPEN_POLL_MS, "open but not streaming is in between");
  assert.equal(pollDelayMs(live({ streaming: false, open: false })), AGGREGATE_POLL_MS);
  assert.equal(pollDelayMs(silent), AGGREGATE_POLL_MS, "the ordinary case: nothing published at all");
  assert.equal(pollDelayMs(undefined), AGGREGATE_POLL_MS);
  assert.equal(AGGREGATE_LIVE_POLL_MS < AGGREGATE_OPEN_POLL_MS && AGGREGATE_OPEN_POLL_MS < AGGREGATE_POLL_MS, true);
});

// ---------------------------------------------------------------------------------------------
// A reason becomes a request
// ---------------------------------------------------------------------------------------------

const fix = (parts: Partial<AggregateFix>): AggregateFix => ({ kind: "register", method: "POST", route: "aggregate/register", body: {}, label: "Register the driver", ...parts });

test("each fix the server prepares becomes exactly the call that sends it", () => {
  assert.deepEqual(fixRequest(fix({})), { call: "register" });
  assert.deepEqual(fixRequest(fix({ route: "aggregate/unregister" })), { call: "unregister" });
  assert.deepEqual(fixRequest(fix({ kind: "match_buffers", route: "aggregate/match-buffers", body: { buffer_size: 512 } })), { call: "match-buffers", bufferSize: 512 });
  assert.deepEqual(fixRequest(fix({ kind: "set_sample_rate", route: "devices/serial:S/command/set_samp_rate", body: { srate_idx: 4 } })), {
    call: "command",
    deviceId: "serial:S",
    command: "set_samp_rate",
    args: { srate_idx: 4 },
  });
  assert.deepEqual(fixRequest(fix({ kind: "set_clock_source", route: "devices/serial:S/command/set_sync_source", body: { src_index: 5 } })), {
    call: "command",
    deviceId: "serial:S",
    command: "set_sync_source",
    args: { src_index: 5 },
  });
});

test("a device id written for a URL comes back as the id itself", () => {
  const request = fixRequest(fix({ route: "devices/serial%3AS%201/command/set_samp_rate", body: { srate_idx: 2 } }));
  assert.equal(request?.call === "command" && request.deviceId, "serial:S 1");
});

test("a route or a body this page does not know is no button at all, rather than a guess", () => {
  assert.equal(fixRequest(fix({ route: "aggregate/match-buffers", body: {} })), undefined, "a match with no size");
  assert.equal(fixRequest(fix({ route: "aggregate/reboot-the-pc" })), undefined);
  assert.equal(fixRequest(fix({ route: "devices/x/command/SET_SAMP_RATE" })), undefined, "a command name is lower case");
  assert.equal(fixRequest(fix({ method: "GET" as AggregateFix["method"] })), undefined);
});

test("only the fix that interrupts a DAW asks for a second click", () => {
  assert.equal(fixNeedsConfirming(fix({ kind: "match_buffers" })), true);
  for (const kind of ["register", "set_clock_source", "set_sample_rate"] as AggregateFix["kind"][]) assert.equal(fixNeedsConfirming(fix({ kind })), false, kind);
});

// ---------------------------------------------------------------------------------------------
// What a gap reads as
// ---------------------------------------------------------------------------------------------

test("zero is the good reading, a direction is named, and a stall is said before anything else", () => {
  assert.deepEqual(gapView(liveDevice("A")), { text: "In step", tone: "good" });
  assert.deepEqual(gapView(liveDevice("A", { is_master: true })), { text: "Master", tone: "good" });
  assert.deepEqual(gapView(liveDevice("A", { sample_gap: -128 })), { text: "128 samples behind", tone: "off" });
  assert.deepEqual(gapView(liveDevice("A", { sample_gap: 1 })), { text: "1 sample ahead", tone: "off" });
  assert.deepEqual(gapView(liveDevice("A", { streaming: false })), { text: "Not streaming", tone: "idle" });
  // A stalled master with a gap of zero would otherwise read as the healthiest device there is.
  assert.deepEqual(gapView(liveDevice("A", { stalled: true, is_master: true })), { text: "Stalled", tone: "stalled" });
});

test("the line at the top says what the driver is doing, and silence is not a failure", () => {
  assert.match(statusLine(live()), /audio is running/);
  assert.match(statusLine(live({ streaming: false })), /audio is not running/);
  assert.match(statusLine(live({ streaming: false, open: false })), /nothing is using it/);
  assert.equal(statusLine(silent), silent.state === "silent" ? silent.message : "", "the server's own words for a driver that is not loaded");
});

// ---------------------------------------------------------------------------------------------
// Joining what Gazelle read to what the driver published
// ---------------------------------------------------------------------------------------------

test("a configured interface is joined to its live figures by name, and a device only the driver knows is kept", () => {
  const joined = deviceViews(answer({ devices: [report("Quadro"), report("Studio+")], status: live({ devices: [liveDevice("Quadro", { is_master: true }), liveDevice("Ghost")] }) }));
  assert.deepEqual(joined.map((view) => [view.name, view.report !== undefined, view.live !== undefined]), [
    ["Quadro", true, true],
    ["Studio+", true, false],
    ["Ghost", false, true],
  ]);
});

test("with nothing published there is a row per configured interface and no live half", () => {
  const joined = deviceViews(answer({ devices: [report("Quadro")] }));
  assert.equal(joined.length, 1);
  assert.equal(joined[0]?.live, undefined);
  assert.deepEqual(deviceViews(undefined), []);
});

test("the size to match to is the master's, then the setup's, then whatever could be read", () => {
  assert.equal(matchTarget(answer({ devices: [report("A", { driver: { buffer_size: 128 } }), report("B", { is_master: true, driver: { buffer_size: 512 } })] })), 512);
  assert.equal(matchTarget(answer({ config: { buffer_size: 256 }, devices: [report("A", { driver: {} })] })), 256);
  assert.equal(matchTarget(answer({ devices: [report("A", { driver: {} }), report("B", { driver: { buffer_size: 64 } })] })), 64);
  assert.equal(matchTarget(answer({ devices: [report("A", { driver: {} })] })), undefined, "nothing to match to is no target");
  assert.equal(matchTarget(undefined), undefined);
});

test("buffers match only when every size that could be read is the same one", () => {
  assert.equal(buffersMatch(answer({ devices: [report("A", { driver: { buffer_size: 256 } }), report("B", { driver: { buffer_size: 256 } })] })), true);
  assert.equal(buffersMatch(answer({ devices: [report("A", { driver: { buffer_size: 256 } }), report("B", { driver: { buffer_size: 128 } })] })), false);
  assert.equal(buffersMatch(answer({ devices: [report("A", { driver: {} })] })), false, "nothing read is not a match");
});

// ---------------------------------------------------------------------------------------------
// Which Gazelle device an entry is
// ---------------------------------------------------------------------------------------------

/** The one view of a one-device answer, which is what the page reads a card from. */
const viewOf = (parts: Partial<AggregateAnswer["devices"][number]> = {}, status?: AggregateStatusReading) =>
  deviceViews(answer({ devices: [report("Quadro", parts)], ...(status === undefined ? {} : { status }) }))[0];

test("the device an entry is comes from the answer, and the workspace's own choice is the fallback", () => {
  assert.equal(resolvedDeviceId(viewOf({ device_id: "serial:Q", matched_by: "worked_out" }), { key: "Quadro" }), "serial:Q");
  assert.equal(resolvedDeviceId(viewOf({ device_id: "serial:Q", matched_by: "chosen" }), { device_id: "serial:Q" }), "serial:Q");
  // A server that answered nothing at all leaves whatever the workspace pinned, so the card still works.
  assert.equal(resolvedDeviceId(undefined, { device_id: "serial:Q" }), "serial:Q");
  assert.equal(resolvedDeviceId(viewOf({ matched_by: "none" }), { key: "Quadro" }), undefined);
});

test("how the device was arrived at is what the answer says, and an older server is read plainly", () => {
  assert.equal(matchedBy(viewOf({ device_id: "serial:Q", matched_by: "worked_out" })), "worked_out");
  assert.equal(matchedBy(viewOf({ device_id: "serial:Q", matched_by: "chosen" })), "chosen");
  assert.equal(matchedBy(viewOf({ matched_by: "none" })), "none");
  assert.equal(matchedBy(viewOf({ device_id: "serial:Q" })), "chosen", "a server too old to say, with a device");
  assert.equal(matchedBy(viewOf({})), "none", "and with none");
});

test("the note saying why a device could not be told is shown only when it could not", () => {
  const why = "Two Zen Quadro Synergy Core are connected. Choose which one this is.";
  assert.equal(matchNote(viewOf({ matched_by: "none", match_note: why })), why);
  assert.equal(matchNote(viewOf({ device_id: "serial:Q", matched_by: "worked_out", match_note: why })), undefined, "a match that worked has nothing to explain");
});

// ---------------------------------------------------------------------------------------------
// The channels of one interface
// ---------------------------------------------------------------------------------------------

const gazelleNames = { inputs: ["Mic 1", "Mic 2", "Mic 3"], outputs: ["Monitor L", "Monitor R"], source: "gazelle" as const };

test("how many channels to list is the count the driver published, then Gazelle's names, then nothing", () => {
  const published = live({ devices: [liveDevice("Quadro", { inputs: 16, outputs: 24 })] });
  assert.deepEqual(channelCounts(viewOf({ channels: gazelleNames }, published)), { inputs: 16, outputs: 24 }, "what the driver itself said wins");
  assert.deepEqual(channelCounts(viewOf({ channels: gazelleNames })), { inputs: 3, outputs: 2 });
  assert.deepEqual(channelCounts(viewOf({})), { inputs: undefined, outputs: undefined }, "nothing known is not a guess");
  assert.deepEqual(channelCounts(viewOf({ channels: { inputs: [], outputs: [], source: "none" } })), { inputs: undefined, outputs: undefined });
  assert.deepEqual(channelCounts(undefined), { inputs: undefined, outputs: undefined });
});

test("a channel with no name of its own is the interface's name and its number from one", () => {
  assert.equal(autoChannelName("Quadro", 0), "Quadro 1");
  assert.equal(autoChannelName("Zen Studio+", 15), "Zen Studio+ 16");
});

test("Gazelle's own name for a channel is offered only when Gazelle has one", () => {
  assert.equal(suggestedChannelName(gazelleNames, true, 1), "Mic 2");
  assert.equal(suggestedChannelName(gazelleNames, false, 0), "Monitor L");
  assert.equal(suggestedChannelName(gazelleNames, true, 9), undefined, "past the end of what it knows");
  assert.equal(suggestedChannelName({ inputs: ["Mic 1"], outputs: [], source: "none" }, true, 0), undefined, "a source of none is no suggestion");
  assert.equal(suggestedChannelName(undefined, true, 0), undefined);
});

test("nothing chosen means every channel is exposed, which is what the field being absent means", () => {
  assert.equal(isExposed(undefined, 7), true);
  assert.equal(isExposed([0, 1], 1), true);
  assert.equal(isExposed([0, 1], 2), false);
});

test("the exposed channels appear as a field only while something is left out, and go away again", () => {
  // Every channel exposed and one turned off: the field appears, with the rest in it.
  assert.deepEqual(withChannelExposed(undefined, 4, 2, false), [0, 1, 3]);
  // The last one put back takes the field away, because absent is what all of them means.
  assert.equal(withChannelExposed([0, 1, 3], 4, 2, true), undefined);
  assert.deepEqual(withChannelExposed([0, 1, 3], 4, 3, false), [0, 1]);
  assert.deepEqual(withChannelExposed([2, 0], 4, 1, true), [0, 1, 2], "and what comes back is in order");
  assert.deepEqual(withChannelExposed([0, 1], 4, 5, false), [0, 1], "a channel the interface does not have changes nothing");
  assert.equal(withChannelExposed([0, 1], 2, 1, true), undefined, "a list that is already all of them is no list");
});

test("naming a channel writes it, and clearing one takes the entry out rather than writing nothing", () => {
  assert.deepEqual(withChannelName(undefined, 0, "Vocal mic"), { "0": "Vocal mic" });
  assert.deepEqual(withChannelName({ "0": "Vocal mic" }, 1, "Room"), { "0": "Vocal mic", "1": "Room" });
  assert.deepEqual(withChannelName({ "0": "Vocal mic", "1": "Room" }, 1, ""), { "0": "Vocal mic" });
  assert.equal(withChannelName({ "0": "Vocal mic" }, 0, "   "), undefined, "only spaces is not a name, and the last one out takes the map with it");
  assert.deepEqual(withChannelName(undefined, 0, "  Vocal mic  "), { "0": "Vocal mic" }, "the spaces around a name are not part of it");
  const long = "x".repeat(CHANNEL_LABEL_MAX + 9);
  assert.equal(withChannelName(undefined, 0, long)?.["0"]?.length, CHANNEL_LABEL_MAX);
});

test("the names counted are the ones on channels the interface actually has", () => {
  assert.equal(namedCount({ "0": "Vocal mic", "9": "Talkback" }, 4), 1);
  assert.equal(namedCount({ "0": "Vocal mic", "9": "Talkback" }, undefined), 2, "with no count, every name counts");
  assert.equal(namedCount({ "0": "  " }, 4), 0);
  assert.equal(namedCount(undefined, 4), 0);
});

test("the Channels line says what is exposed and how many are named", () => {
  const device = (parts: Partial<AggregateDevice>): AggregateDevice => ({ key: "Quadro", ...parts });
  assert.equal(channelSummary(device({}), { inputs: 16, outputs: 24 }), "All in, All out");
  assert.equal(channelSummary(device({ inputs: [0, 1] }), { inputs: 16, outputs: 24 }), "2 in, All out");
  assert.equal(
    channelSummary(device({ inputs: Array.from({ length: 16 }, (_, at) => at), outputs: Array.from({ length: 24 }, (_, at) => at), input_names: { "0": "Vocal mic", "1": "Room" }, output_names: { "0": "Main L" } }), { inputs: 16, outputs: 24 }),
    "16 in, 24 out, 3 named",
  );
  assert.equal(channelSummary(device({ input_names: { "0": "Vocal mic" } }), { inputs: undefined, outputs: undefined }), "All in, All out, 1 named");
});

test("the label a channel shows is the workspace's, and a channel with none shows nothing", () => {
  assert.equal(channelLabel({ "3": "Vocal mic" }, 3), "Vocal mic");
  assert.equal(channelLabel({ "3": "Vocal mic" }, 4), "");
  assert.equal(channelLabel(undefined, 0), "");
});

// ---------------------------------------------------------------------------------------------
// Lining the interfaces up: the channels, the pickers, and what to plug in
// ---------------------------------------------------------------------------------------------

/** Two interfaces, the first with four channels each way and the second with two. */
const twoInterfaces = (parts: Partial<AggregateDevice>[] = [{}, {}]) => ({
  config: { devices: [{ key: "Q", name: "Quadro", ...parts[0] }, { key: "S", name: "Studio+", ...parts[1] }] },
  answer: answer({
    devices: [
      report("Quadro", { channels: { inputs: ["Mic 1", "Mic 2", "Mic 3", "Mic 4"], outputs: ["Main L", "Main R", "Cue L", "Cue R"], source: "gazelle" } }),
      report("Studio+", { channels: { inputs: ["Line 1", "Line 2"], outputs: ["Out 1", "Out 2"], source: "gazelle" } }),
    ],
  }),
});

test("an interface is named the way every answer joins it, and found in the answer by that name", () => {
  assert.equal(deviceName({ key: "Zen Quadro" }, 0), "Zen Quadro");
  assert.equal(deviceName({ key: "Zen Quadro", name: "Quadro" }, 0), "Quadro");
  assert.equal(deviceName({}, 2), "Interface 3");
  const it = twoInterfaces();
  assert.equal(viewFor(it.answer, it.config.devices[1] as AggregateDevice, 1)?.name, "Studio+");
  assert.deepEqual(calibrateDevices(it.config), ["Quadro", "Studio+"]);
});

test("each interface's channels are listed by that interface and its own numbering, never the aggregate's", () => {
  const it = twoInterfaces();
  const inputs = interfaceChannels(it.config, it.answer, true);
  assert.deepEqual(inputs.map((one) => [one.device, one.index, one.channel, one.auto]), [
    ["Quadro", 0, 0, "Quadro 1"],
    ["Quadro", 0, 1, "Quadro 2"],
    ["Quadro", 0, 2, "Quadro 3"],
    ["Quadro", 0, 3, "Quadro 4"],
    ["Studio+", 1, 0, "Studio+ 1"],
    ["Studio+", 1, 1, "Studio+ 2"],
  ]);
  assert.ok(inputs.every((one) => !Object.hasOwn(one, "number")), "nothing here is a place in the aggregate's list");
  assert.equal(interfaceChannels(it.config, it.answer, false).length, 6);
  assert.deepEqual(interfaceChannels(undefined, undefined, true), [], "nothing set up is no list");
});

test("a channel kept out of the aggregate is not offered, and every other channel keeps its own number", () => {
  const it = twoInterfaces([{ inputs: [0, 3] }, {}]);
  assert.deepEqual(interfaceChannels(it.config, it.answer, true).map((one) => [one.device, one.channel, one.auto]), [
    ["Quadro", 0, "Quadro 1"],
    ["Quadro", 3, "Quadro 4"],
    ["Studio+", 0, "Studio+ 1"],
    ["Studio+", 1, "Studio+ 2"],
  ]);
});

test("the channels the phase measurement runs over are not offered, because the driver keeps them", () => {
  // The Studio+'s phase arrives on its second input, over the Quadro's fourth output.
  const it = twoInterfaces([{}, { phase: { master_output: 3, input: 1 } }]);
  assert.deepEqual(interfaceChannels(it.config, it.answer, true).map((one) => one.auto), ["Quadro 1", "Quadro 2", "Quadro 3", "Quadro 4", "Studio+ 1"]);
  assert.deepEqual(interfaceChannels(it.config, it.answer, false).map((one) => one.auto), ["Quadro 1", "Quadro 2", "Quadro 3", "Studio+ 1", "Studio+ 2"]);
});

test("a channel with a name of its own says both, so the cable can still be found on the box", () => {
  const it = twoInterfaces([{ input_names: { "0": "Vocal mic" } }, {}]);
  const inputs = interfaceChannels(it.config, it.answer, true);
  assert.equal(inputs[0]?.text, "Vocal mic (Quadro 1)");
  assert.equal(inputs[1]?.text, "Quadro 2", "an unnamed one is just itself");
});

test("an interface whose channels are not known yet offers none of them rather than guessing", () => {
  const config = { devices: [{ key: "Q", name: "Quadro" }, { key: "S", name: "Studio+" }] };
  assert.deepEqual(interfaceChannels(config, answer({ devices: [report("Quadro"), report("Studio+")] }), true), []);
});

test("the pass decides which side is all on one interface, and which picker offers what", () => {
  const devices = ["Quadro", "Studio+"];
  // The input pass plays every click from the reference and records on each interface in turn.
  assert.equal(slotDevice("inputs", "Quadro", "outputs", 1, devices), "Quadro");
  assert.equal(slotDevice("inputs", "Quadro", "inputs", 1, devices), "Studio+");
  // The output pass is the same thing the other way round.
  assert.equal(slotDevice("outputs", "Quadro", "outputs", 1, devices), "Studio+");
  assert.equal(slotDevice("outputs", "Quadro", "inputs", 1, devices), "Quadro");
});

test("the input pass starts with two outputs of the reference and one input on each interface", () => {
  const it = twoInterfaces();
  const picks = defaultPicks(it.config, it.answer, "inputs");
  assert.equal(picks.reference, "Quadro", "the first interface, until somebody says otherwise");
  assert.deepEqual(picks.outputs, [0, 1], "two outputs of one interface");
  assert.deepEqual(picks.inputs, [0, 0], "Quadro 1 in and Studio+ 1 in, each by its own interface's numbering");
  assert.deepEqual(cablingSteps(picks, it.config, it.answer).map((cable) => cable.text), ["Quadro 1 into Quadro 1", "Quadro 2 into Studio+ 1"]);
});

test("the output pass plays one output on each interface into two inputs of the reference", () => {
  const it = twoInterfaces();
  const picks = defaultPicks(it.config, it.answer, "outputs", "Studio+");
  assert.equal(picks.reference, "Studio+");
  assert.deepEqual(picks.outputs, [0, 0], "one output on each interface");
  assert.deepEqual(picks.inputs, [0, 1], "both into the Studio+");
  assert.deepEqual(cablingSteps(picks, it.config, it.answer).map((cable) => cable.text), ["Quadro 1 into Studio+ 1", "Studio+ 1 into Studio+ 2"]);
});

test("a reference nothing in the setup names falls back to the first interface", () => {
  const it = twoInterfaces();
  assert.equal(defaultPicks(it.config, it.answer, "inputs", "A device that has gone").reference, "Quadro");
  assert.equal(defaultPicks(undefined, undefined).reference, "");
});

test("what was chosen is kept across a poll, and only a choice that no longer fits falls back", () => {
  const it = twoInterfaces();
  const stored: CalibratePicks = { direction: "inputs", reference: "Quadro", outputs: [2, 3], inputs: [1, 1], clicks: 16, level_dbfs: -12 };
  assert.deepEqual(reconcilePicks(stored, it.config, it.answer), stored, "every choice still names a channel its picker offers");

  // An input the Studio+ is not offering: it has two, so its third is not a choice.
  assert.deepEqual(reconcilePicks({ ...stored, inputs: [1, 2] }, it.config, it.answer).inputs, [1, 0], "the one that does not fit, and only that one");
  // A channel that is not there any more, and a number of clicks nothing offers.
  const gone: CalibratePicks = { ...stored, outputs: [99, 3], clicks: 7 };
  assert.deepEqual(reconcilePicks(gone, it.config, it.answer).outputs, [0, 3]);
  assert.equal(reconcilePicks(gone, it.config, it.answer).clicks, 8);
  // Nothing chosen at all is simply where the defaults would have put it.
  assert.deepEqual(reconcilePicks(undefined, it.config, it.answer), defaultPicks(it.config, it.answer));
});

test("changing the pass takes the choices the other way round with it", () => {
  const it = twoInterfaces();
  const chosen: CalibratePicks = { ...defaultPicks(it.config, it.answer, "inputs"), outputs: [2, 3], clicks: 16, level_dbfs: -12 };
  const flipped = withPass(chosen, it.config, it.answer, "outputs", "Quadro");
  assert.deepEqual(flipped.outputs, [0, 0], "one output on each interface");
  assert.deepEqual(flipped.inputs, [0, 1], "both into the reference");
  assert.deepEqual([flipped.clicks, flipped.level_dbfs], [16, -12], "how many clicks and how loud stay as they were");
  const moved = withPass(chosen, it.config, it.answer, "inputs", "Studio+");
  assert.deepEqual([moved.reference, moved.outputs, moved.inputs], ["Studio+", [0, 1], [0, 0]], "another reference is other cabling too");
});

test("a run that cannot be made says why, and makes no request", () => {
  const it = twoInterfaces();
  const good = defaultPicks(it.config, it.answer);
  assert.equal(calibrateProblem(good, it.config, it.answer), undefined);
  assert.deepEqual(calibrateRequest(good, it.config, it.answer), {
    direction: "inputs",
    outputs: [{ device: 0, channel: 0 }, { device: 0, channel: 1 }],
    inputs: [{ device: 0, channel: 0 }, { device: 1, channel: 0 }],
    clicks: 8,
    level_dbfs: -20,
  });

  const one = { devices: [{ key: "Q", name: "Quadro" }] };
  assert.match(String(calibrateProblem(defaultPicks(one, it.answer), one, it.answer)), /at least two interfaces/);
  assert.equal(calibrateRequest(defaultPicks(one, it.answer), one, it.answer), undefined);

  assert.match(String(calibrateProblem({ ...good, outputs: [0, undefined] }, it.config, it.answer)), /one output and one input/);
  assert.match(String(calibrateProblem({ ...good, outputs: [1, 1] }, it.config, it.answer)), /cannot take their click from one output/);
  // The same number on two interfaces is two different inputs, and a perfectly good run.
  assert.equal(calibrateProblem({ ...good, inputs: [0, 0] }, it.config, it.answer), undefined);
  const outputPass = defaultPicks(it.config, it.answer, "outputs");
  assert.match(String(calibrateProblem({ ...outputPass, inputs: [2, 2] }, it.config, it.answer)), /cannot record on one input/);
  // A channel the interface its picker is about is not offering.
  assert.match(String(calibrateProblem({ ...good, outputs: [0, 4] }, it.config, it.answer)), /one output and one input/);
  assert.match(String(calibrateProblem({ ...good, inputs: [0, 2] }, it.config, it.answer)), /one output and one input/);
  assert.match(String(calibrateProblem({ ...good, clicks: 3 }, it.config, it.answer)), /how many clicks/);
});

/**
 * **The Check the owner pressed on 2026-09-21.** Gazelle's list has fourteen inputs for the Quadro,
 * and its driver has sixteen, so counting the aggregate's channels here put the Studio+'s first input
 * at 14, which is the Quadro's fifteenth, and the run rightly refused it. Asked for by interface, the
 * Studio+'s first input is the Studio+'s first input whatever the Quadro is counted as.
 */
test("a check names each channel by interface, so a wrong count cannot put a cable on the wrong interface", () => {
  const fourteen = Array.from({ length: 14 }, (_, at) => `Mic ${at + 1}`);
  const config = { devices: [{ key: "Zen Quadro Synergy Core", name: "Zen Quadro Synergy Core" }, { key: "ZenStudioTB ASIO Driver", name: "ZenStudioTB ASIO Driver" }] };
  const real = answer({
    devices: [
      report("Zen Quadro Synergy Core", { channels: { inputs: fourteen, outputs: fourteen, source: "gazelle" } }),
      report("ZenStudioTB ASIO Driver", { channels: { inputs: fourteen.slice(0, 8), outputs: fourteen.slice(0, 8), source: "gazelle" } }),
    ],
  });
  const request = calibrateRequest(defaultPicks(config, real), config, real, { check: true });
  assert.deepEqual(request, {
    direction: "inputs",
    outputs: [{ device: 0, channel: 0 }, { device: 0, channel: 1 }],
    inputs: [{ device: 0, channel: 0 }, { device: 1, channel: 0 }],
    clicks: 8,
    level_dbfs: -20,
    check: true,
  });
  const json = JSON.stringify(request);
  assert.doesNotMatch(json, /14/, "no number in it depends on how many channels the Quadro was counted as");

  // The output pass the other way round: one output on each interface, both into the reference.
  const outputs = calibrateRequest(defaultPicks(config, real, "outputs", "ZenStudioTB ASIO Driver"), config, real);
  assert.deepEqual(outputs?.outputs, [{ device: 0, channel: 0 }, { device: 1, channel: 0 }]);
  assert.deepEqual(outputs?.inputs, [{ device: 1, channel: 0 }, { device: 1, channel: 1 }]);
});

// ---------------------------------------------------------------------------------------------
// What the measurement says
// ---------------------------------------------------------------------------------------------

const heard = (parts: Partial<AggregateCalibrateReading> = {}): AggregateCalibrateReading => ({
  device: "Studio+",
  is_reference: false,
  lag_samples: 27.8,
  spread_samples: 0.3,
  clicks_found: 8,
  ...parts,
});

const measured = (parts: Partial<AggregateCalibrateOutcome> = {}): AggregateCalibrateOutcome => ({
  direction: "inputs",
  rate: 96000,
  buffer_size: 512,
  reference: "Quadro",
  readings: [heard({ device: "Quadro", is_reference: true, lag_samples: 0, spread_samples: 0 }), heard()],
  trims: [{ device: "Studio+", direction: "inputs", was: 0, measured: 28, now: 28 }],
  warnings: [],
  ...parts,
});

test("a run is asked about only while it is going", () => {
  assert.equal(calibrateRunning({ state: "running" }), true);
  for (const state of ["idle", "done", "failed"] as const) assert.equal(calibrateRunning({ state }), false, state);
  assert.equal(calibrateRunning(undefined), false);
});

test("the line while it runs is the server's own step with how far along it is", () => {
  assert.equal(calibrateStepText({ state: "running", step: "Playing the clicks", progress: 0.42 }), "Playing the clicks (42%)");
  assert.equal(calibrateStepText({ state: "running", step: "Opening the drivers" }), "Opening the drivers", "a run with no progress still says what it is doing");
  assert.equal(calibrateStepText({ state: "running" }), "Measuring");
  assert.equal(calibrateStepText({ state: "done", step: "Playing the clicks", progress: 1 }), "", "nothing is running, so there is no line");
  assert.equal(calibrateProgress({ state: "running", progress: 0.42 }), 0.42);
  assert.equal(calibrateProgress({ state: "running", progress: 7 }), 1, "a figure past the end is the end");
  assert.equal(calibrateProgress({ state: "running" }), 0);
  assert.equal(calibrateProgress({ state: "done" }), 0);
});

test("a reading says how far out, how steady it was, and how much it had to go on", () => {
  const late = readingView(heard());
  assert.equal(late.lag, "27.8 samples late");
  assert.equal(late.spread, "Clicks agreed to 0.3 samples");
  assert.equal(late.clicks, "8 clicks found");
  assert.equal(late.tone, "off");
  assert.equal(readingView(heard({ lag_samples: -1 })).lag, "1 sample early");
  assert.equal(readingView(heard({ lag_samples: 0 })).lag, "In step");
  assert.equal(readingView(heard({ lag_samples: 0 })).tone, "good");
  assert.match(readingView(heard({ device: "Quadro", is_reference: true, lag_samples: 0 })).lag, /measured against/);
  assert.equal(readingView(heard({ clicks_found: 0, spread_samples: 0 })).spread, "Nothing to measure");
  // What the server says it played, and its own sentence about this interface, where it sends them.
  assert.equal(readingView(heard({ clicks_found: 7, clicks_expected: 8 })).clicks, "7 of 8 clicks found");
  assert.equal(readingView(heard({ note: "Studio+ recorded 27.8 samples after the Quadro." })).note, "Studio+ recorded 27.8 samples after the Quadro.");
  assert.equal(readingView(heard({ note: "  " })).note, undefined, "an empty sentence is no sentence");
});

test("a drift finding reads as the serious one, and says that no trim can answer it", () => {
  const drifting = readingView(heard({ drift: { samples_per_second: 0.6, ppm: 6.25, real: true } }));
  assert.equal(drifting.tone, "drift");
  assert.match(String(drifting.drift), /not sharing one clock/);
  assert.match(String(drifting.drift), /no trim can put that right/);
  assert.match(String(drifting.drift), /0\.6 samples a second, 6\.25 ppm/);
  // One the server measured and does not believe in is said quietly, and changes nothing.
  const steady = readingView(heard({ drift: { samples_per_second: 0.02, ppm: 0.2, real: false } }));
  assert.equal(steady.tone, "off");
  assert.match(String(steady.drift), /holding together/);
  assert.equal(readingView(heard()).drift, undefined, "and a run that did not measure it says nothing");

  assert.equal(driftFound(measured({ readings: [heard({ drift: { samples_per_second: 1, ppm: 10, real: true } })] })), true);
  assert.equal(driftFound(measured()), false);
  assert.equal(driftFound(undefined), false);
});

test("what a run came to names the rate and the buffer size a trim is only true for", () => {
  assert.match(outcomeSummary(measured()), /Measured what the interfaces record at 96 kHz, 512 samples, against Quadro/);
  assert.match(outcomeSummary(measured()), /only true for this rate and this buffer size/);
  assert.match(outcomeSummary(measured({ direction: "outputs" })), /what the interfaces play/);
});

test("a trim row shows what it is now, what was measured and what it would become", () => {
  assert.deepEqual(trimRows(measured()), [{ device: "Studio+", what: "Input trim", was: "0", measured: "28", now: "28", changed: true }]);
  const already = measured({ trims: [{ device: "Studio+", direction: "outputs", was: 28, measured: 28, now: 28 }] });
  assert.equal(trimRows(already)[0]?.changed, false);
  assert.equal(trimRows(already)[0]?.what, "Output trim");
  assert.deepEqual(trimsToApply(already), [], "nothing to write is no button");
  assert.equal(trimsToApply(measured()).length, 1);
  assert.deepEqual(trimRows(undefined), []);

  // One the server says it is not offering is shown with its reason and passed over by the button.
  const held = measured({ trims: [{ device: "Quadro", direction: "inputs", field: "input_trim", was: 0, measured: 0, now: 0, is_reference: true, not_applied: "The reference has nothing to correct against itself." }, { device: "Studio+", direction: "inputs", field: "input_trim", was: 0, measured: 28, now: 28, not_applied: "Only 2 of 8 clicks were found, which is not a measurement to write." }] });
  assert.equal(trimRows(held)[1]?.changed, false);
  assert.match(String(trimRows(held)[1]?.notApplied), /not a measurement to write/);
  assert.deepEqual(trimsToApply(held), []);
  assert.deepEqual(withMeasuredTrims({ devices: [{ key: "S", name: "Studio+" }] }, held).devices, [{ key: "S", name: "Studio+" }], "and nothing of it is written");
  // The field the server names decides where it is written, whatever the pass was called.
  assert.equal(trimRows(measured({ trims: [{ device: "Studio+", direction: "inputs", field: "output_trim", was: 0, measured: 5, now: 5 }] }))[0]?.what, "Output trim");
});

test("applying the trims writes them into the setup, by the name the setup gives each interface", () => {
  const config = { devices: [{ key: "Q", name: "Quadro" }, { key: "S", name: "Studio+", input_trim: 4 }] };
  const written = withMeasuredTrims(config, measured());
  assert.deepEqual(written.devices, [{ key: "Q", name: "Quadro" }, { key: "S", name: "Studio+", input_trim: 28 }]);
  // Zero is the field being absent, as every other trim on this page is written.
  const back = withMeasuredTrims(written, measured({ trims: [{ device: "Studio+", direction: "inputs", was: 28, measured: 0, now: 0 }] }));
  assert.deepEqual(back.devices, [{ key: "Q", name: "Quadro" }, { key: "S", name: "Studio+" }]);
  // An interface the measurement names and the setup no longer has changes nothing.
  assert.deepEqual(withMeasuredTrims({ devices: [{ key: "Q", name: "Quadro" }] }, measured()).devices, [{ key: "Q", name: "Quadro" }]);
  assert.deepEqual(withMeasuredTrims(config, undefined), config);
  assert.match(appliedTrimsText(trimsToApply(measured())), /1 trim written: Studio\+ in 28\./);
  assert.match(appliedTrimsText([]), /Nothing to change/);
});

// ---------------------------------------------------------------------------------------------
// The phase reference, written with the trim
// ---------------------------------------------------------------------------------------------

/** A run whose Studio+ trim comes with a phase reference, as the server offers one. */
const withReference = (reference: { was: number | null; now: number | null }, trim: Partial<AggregateCalibrateOutcome["trims"][number]> = {}) =>
  measured({ trims: [{ device: "Studio+", direction: "inputs", field: "input_trim", was: 0, measured: 28, now: 28, phase_reference: reference, ...trim }] });

const phased = { key: "S", name: "Studio+", input_trim: 28, phase: { master_output: 15, input: 8, reference: -84 } };

test("writing a trim writes the phase reference measured beside it", () => {
  const written = withMeasuredTrims({ devices: [{ key: "Q", name: "Quadro" }, { key: "S", name: "Studio+", phase: { master_output: 15, input: 8 } }] }, withReference({ was: null, now: -84 }));
  assert.deepEqual(written.devices?.[1], { key: "S", name: "Studio+", input_trim: 28, phase: { master_output: 15, input: 8, reference: -84 } });
  // A new reference replaces the old one, and the path it was measured on is left as it was.
  const again = withMeasuredTrims({ devices: [phased] }, withReference({ was: -84, now: -148 }, { was: 28, measured: 30, now: 30 }));
  assert.deepEqual(again.devices?.[0], { key: "S", name: "Studio+", input_trim: 30, phase: { master_output: 15, input: 8, reference: -148 } });
});

test("a trim offered with nothing heard on the cable takes the old reference out", () => {
  const written = withMeasuredTrims({ devices: [phased] }, withReference({ was: -84, now: null }, { was: 28, measured: 31, now: 31 }));
  assert.deepEqual(written.devices?.[0], { key: "S", name: "Studio+", input_trim: 31, phase: { master_output: 15, input: 8 } });
  // Even when the trim itself comes out the same, because the old reference no longer belongs beside it.
  const same = withReference({ was: -84, now: null }, { was: 28, measured: 28, now: 28 });
  assert.equal(trimsToApply(same).length, 1);
  assert.deepEqual(withMeasuredTrims({ devices: [phased] }, same).devices?.[0], { key: "S", name: "Studio+", input_trim: 28, phase: { master_output: 15, input: 8 } });
});

test("a first run whose trim is unchanged but whose reference is new is still there to write", () => {
  // The trim was already 28 and was measured at 28; the reference was nothing and is now -84.
  const first = withReference({ was: null, now: -84 }, { was: 28, measured: 28, now: 28 });
  assert.equal(referenceChanges(first.trims[0] as AggregateCalibrateOutcome["trims"][number]), true);
  assert.equal(trimsToApply(first).length, 1, "the button is offered");
  assert.equal(trimRows(first)[0]?.changed, true);
  const written = withMeasuredTrims({ devices: [{ key: "S", name: "Studio+", input_trim: 28, phase: { master_output: 15, input: 8 } }] }, first);
  assert.deepEqual(written.devices?.[0], { key: "S", name: "Studio+", input_trim: 28, phase: { master_output: 15, input: 8, reference: -84 } });
  assert.equal(appliedTrimsText(trimsToApply(first)), "1 trim written: Studio+ in 28 (phase reference -84).");
  // And nothing changing on either count is still nothing to write.
  const settled = withReference({ was: -84, now: -84 }, { was: 28, measured: 28, now: 28 });
  assert.equal(referenceChanges(settled.trims[0] as AggregateCalibrateOutcome["trims"][number]), false);
  assert.deepEqual(trimsToApply(settled), []);
});

test("a reference is only written where there is a phase path to go with it", () => {
  // The phase setting was cleared after the run: the trim is written, and no half setting is made up.
  const written = withMeasuredTrims({ devices: [{ key: "S", name: "Studio+" }] }, withReference({ was: null, now: -84 }));
  assert.deepEqual(written.devices?.[0], { key: "S", name: "Studio+", input_trim: 28 });
  // An output trim never carries one, whatever arrives beside it.
  const out = withReference({ was: null, now: -84 }, { direction: "outputs", field: "output_trim" });
  assert.deepEqual(withMeasuredTrims({ devices: [phased] }, out).devices?.[0], { ...phased, output_trim: 28 });
  // And one the server is not offering writes neither.
  const held = withReference({ was: null, now: -84 }, { not_applied: "Only 2 of 8 clicks were found." });
  assert.deepEqual(trimsToApply(held), []);
});

test("a check offers no trims, whatever its outcome carries", () => {
  const checked = { ...withReference({ was: null, now: -84 }), checking: true };
  assert.deepEqual(trimsToApply(checked), []);
  assert.deepEqual(withMeasuredTrims({ devices: [phased] }, checked).devices, [phased]);
});

test("the reference beside a trim is said as a reference, and never as a trim", () => {
  assert.equal(referenceText(undefined), undefined);
  assert.equal(referenceText({ was: null, now: -84 }), "Phase reference: none yet, becomes -84 samples");
  assert.equal(referenceText({ was: -84, now: -148 }), "Phase reference: -84 samples becomes -148 samples");
  assert.equal(referenceText({ was: -84, now: -84 }), "Phase reference: stays at -84 samples");
  assert.match(String(referenceText({ was: -84, now: null })), /-84 samples is taken out, because nothing was heard on the cable/);
  assert.match(String(referenceText({ was: null, now: null })), /none, and nothing was heard/);
  assert.equal(trimRows(withReference({ was: null, now: -84 }))[0]?.reference, "Phase reference: none yet, becomes -84 samples");
  assert.equal(trimRows(measured())[0]?.reference, undefined, "a trim with no phase setting says nothing about one");
  for (const text of [referenceText({ was: null, now: -84 }), referenceText({ was: -84, now: null }), referenceText({ was: 1, now: 2 })]) assert.doesNotMatch(String(text), /trim/i);
  assert.equal(appliedTrimsText(trimsToApply(withReference({ was: -84, now: null }, { was: 28, now: 30, measured: 30 }))), "1 trim written: Studio+ in 30 (phase reference taken out).");
});

test("a check is asked for with the same cabling, and a measurement leaves the field out", () => {
  const it = twoInterfaces();
  const picks = defaultPicks(it.config, it.answer);
  assert.equal(Object.hasOwn(calibrateRequest(picks, it.config, it.answer) ?? {}, "check"), false);
  assert.deepEqual(calibrateRequest(picks, it.config, it.answer, { check: true }), { ...calibrateRequest(picks, it.config, it.answer), check: true });
  assert.equal(calibrateRequest({ ...picks, outputs: [1, 1] }, it.config, it.answer, { check: true }), undefined, "a check that cannot be made is no request either");
});

// ---------------------------------------------------------------------------------------------
// The phase setup on a card
// ---------------------------------------------------------------------------------------------

test("a phase setting counts only when both channels are there", () => {
  assert.deepEqual(phaseSetting({ phase: { master_output: 15, input: 8 } }), { master_output: 15, input: 8 });
  assert.equal(phaseSetting({ phase: { master_output: 15 } as never }), undefined);
  assert.equal(phaseSetting({ phase: { master_output: -1, input: 8 } }), undefined);
  assert.equal(phaseSetting({}), undefined);
  assert.equal(phaseReference({ phase: { master_output: 15, input: 8, reference: -84 } }), -84);
  assert.equal(phaseReference({ phase: { master_output: 15, input: 8 } }), undefined);
});

test("the callback master is the one the setup names, and the first when it names none", () => {
  const it = twoInterfaces();
  assert.equal(masterIndex(it.config, it.answer), 0);
  assert.equal(masterIndex({ ...it.config, callback_master: "Studio+" }, it.answer), 1);
  assert.equal(masterIndex({ ...it.config, callback_master: "s" }, it.answer), 1, "by registry key, without case");
  assert.equal(masterIndex({ devices: [] }, it.answer), undefined);
});

test("the pickers offer the master's own outputs and this interface's own inputs, counted from one", () => {
  const it = twoInterfaces([{ output_names: { "3": "To Studio+" } }, {}]);
  const choices = phaseChoices(it.config, it.answer, 1);
  assert.equal(choices?.master, "Quadro");
  assert.deepEqual(choices?.outputs?.map((one) => [one.value, one.text]), [
    [0, "Quadro 1 (Main L)"],
    [1, "Quadro 2 (Main R)"],
    [2, "Quadro 3 (Cue L)"],
    [3, "Quadro 4 (To Studio+)"],
  ]);
  assert.deepEqual(choices?.inputs?.map((one) => one.text), ["Studio+ 1 (Line 1)", "Studio+ 2 (Line 2)"]);
  // Not on the callback master's card, which the others are measured against.
  assert.equal(phaseChoices(it.config, it.answer, 0), undefined);
  // A channel kept out of what a DAW sees is still offered: the driver opens these two itself.
  const hidden = twoInterfaces([{}, { inputs: [0] }]);
  assert.equal(phaseChoices(hidden.config, hidden.answer, 1)?.inputs?.length, 2);
  // With no count to go on, nothing is guessed at.
  assert.equal(phaseChoices(it.config, answer(), 1)?.inputs, undefined);
  // And a setting made while more channels were known is kept on the list.
  assert.deepEqual(choicesWith([{ value: 0, text: "Studio+ 1" }], 9, "Studio+").map((one) => one.text), ["Studio+ 1", "Studio+ 10 (not listed now)"]);
  assert.equal(choicesWith(undefined, undefined, "Studio+").length, 0);
});

test("nothing is written until both channels are chosen, and a new path loses the old reference", () => {
  assert.equal(phaseFromPicks(undefined, { master_output: 15 }), undefined, "half a path is refused by the driver");
  assert.deepEqual(phaseFromPicks(undefined, { master_output: 15, input: 8 }), { master_output: 15, input: 8 });
  const current = { master_output: 15, input: 8, reference: -84 };
  assert.equal(phaseFromPicks(current, { master_output: 15, input: 8 }), current, "the same path is the same setting");
  assert.deepEqual(phaseFromPicks(current, { master_output: 14, input: 8 }), { master_output: 14, input: 8 });
  // What the pickers show: this tab's pick, and otherwise the setting.
  assert.deepEqual(phasePicks(current, undefined), { master_output: 15, input: 8 });
  assert.deepEqual(phasePicks(current, { input: 9 }), { master_output: 15, input: 9 });
  assert.deepEqual(phasePicks(undefined, undefined), {});
  assert.deepEqual(withPhase({ key: "S" }, { master_output: 1, input: 2 }), { key: "S", phase: { master_output: 1, input: 2 } });
  assert.deepEqual(withPhase({ key: "S", phase: { master_output: 1, input: 2 } }, undefined), { key: "S" });
});

test("the phase setup says whether it is set up and whether it has a reference yet", () => {
  assert.equal(phaseSetupView({}, false, "Quadro").summary, "Not set up");
  assert.match(phaseSetupView({}, false, "Quadro").note, /leaves Quadro on/);
  const waiting = phaseSetupView({ phase: { master_output: 15, input: 8 } }, false, "Quadro");
  assert.equal(waiting.summary, "Set up, no reference yet");
  assert.match(waiting.note, /One measurement under Line the interfaces up gives it one/);
  assert.equal(phaseSetupView(phased, false, "Quadro").summary, "Set up, reference -84 samples");
  assert.equal(phaseSetupView(phased, false, "Quadro").tone, "good");
  assert.equal(phaseSetupView(phased, true, "Quadro").summary, "Refused on the callback master");
  for (const view of [waiting, phaseSetupView(phased, false, "Quadro")]) assert.doesNotMatch(view.summary, /trim/i);
});

test("the routing the phase needs names both ends and the kind of socket the cable is", () => {
  const cables = [{ id: "c", from: { device_id: "q", port: "SPDIF_OUT" as const, first: 0 }, to: { device_id: "s", port: "SPDIF_IN" as const, first: 0 }, channels: 2 }];
  assert.equal(cablePort(cables, "q", "s"), "S/PDIF");
  assert.equal(cablePort([{ ...cables[0]!, from: { device_id: "q", port: "ADAT_OUT", first: 0 } }], "q", "s"), "ADAT");
  assert.equal(cablePort(cables, "s", "q"), undefined);
  assert.equal(cablePort(cables, undefined, "s"), undefined);
  const note = phaseRoutingNote("Quadro", "Studio+", "S/PDIF");
  assert.match(note, /on Quadro, route the playback channel chosen under Leaves the callback master on to its S\/PDIF output/);
  assert.match(note, /on Studio\+, route its S\/PDIF input, where the cable arrives, to the record channel chosen under Arrives on/);
  assert.match(note, /reads as nothing heard/);
});

// ---------------------------------------------------------------------------------------------
// The phase, live and in a run
// ---------------------------------------------------------------------------------------------

test("a session's phase is said in words, with what was measured and what was applied", () => {
  const applied = livePhaseView(liveDevice("Studio+", { phase: "applied", phase_measured: -148, phase_applied: -64 }));
  assert.deepEqual(applied, { text: "Lined up to its reference", tone: "good", refused: false, figures: "Measured -148 samples, applied -64 samples" });
  assert.equal(livePhaseView(liveDevice("Studio+", { phase: "no_reference", phase_measured: -148, phase_applied: 0 }))?.tone, "warn");
  const unheard = livePhaseView(liveDevice("Studio+", { phase: "not_heard" }));
  assert.equal(unheard?.refused, true);
  assert.equal(unheard?.figures, undefined, "nothing heard is nothing measured");
  assert.equal(livePhaseView(liveDevice("Studio+", { phase: "off_the_grid", phase_measured: -100, phase_applied: 0 }))?.text, "Refused: not a whole number of 32 sample steps from its reference");
  assert.equal(livePhaseView(liveDevice("Studio+", { phase: "something_new" }))?.text, "something_new", "a word this page does not know is shown as it came");
  assert.equal(livePhaseView(liveDevice("Quadro", { is_master: true, phase: "not_configured" })), undefined);
  assert.equal(livePhaseView(liveDevice("Studio+")), undefined, "an older driver says nothing about it");
  // The card's own line.
  assert.equal(cardPhaseView(undefined, false).text, "No DAW has it open");
  assert.equal(cardPhaseView(undefined, true).text, "The others are measured against it");
  assert.equal(cardPhaseView(liveDevice("Studio+", { phase: "applied", phase_measured: -148, phase_applied: -64 }), false).text, "Lined up to its reference. Measured -148 samples, applied -64 samples");
  assert.equal(cardPhaseView(liveDevice("Studio+"), false).text, "Not reported");
});

test("a run says whether it was clean, and how many blocks it lost when it was not", () => {
  assert.equal(runCleanText(measured()), undefined, "an older server does not say");
  assert.deepEqual(runCleanText(measured({ clean: true, blocks_lost: 0 })), { text: "Clean: no interface lost a block while it ran.", problem: false });
  const lost = runCleanText(measured({ clean: false, blocks_lost: 4 }));
  assert.equal(lost?.problem, true);
  assert.match(String(lost?.text), /^Not clean: 4 blocks lost while it ran\./);
  assert.match(String(runCleanText(measured({ clean: false, blocks_lost: 1 }))?.text), /1 block lost/);
});

test("a run's phases read as measured and not applied, on purpose, and a refused one reads as refused", () => {
  const run = measured({
    phases: [
      { device: "Quadro", state: "not_configured", measured_samples: 0, applied_samples: 0, note: "" },
      { device: "Studio+", state: "measured_only", measured_samples: -148, applied_samples: 0, note: "Studio+ was measured at -148 samples." },
    ],
  });
  const views = runPhaseViews(run);
  assert.match(views[1]?.text ?? "", /not applied, on purpose/);
  assert.equal(views[1]?.figures, "Measured -148 samples, applied 0 samples");
  assert.equal(views[1]?.note, "Studio+ was measured at -148 samples.");
  assert.equal(views[0]?.note, undefined);
  assert.equal(phaseRefusedText(run), undefined);
  const refused = measured({ phases: [{ device: "Studio+", state: "not_heard", measured_samples: 0, applied_samples: 0, note: "Nothing arrived on its measurement channel." }] });
  assert.equal(runPhaseViews(refused)[0]?.refused, true);
  assert.match(String(phaseRefusedText(refused)), /not measured on Studio\+, so writing the trims takes that interface's old reference out/);
  assert.match(String(phaseRefusedText({ ...refused, checking: true })), /this check ran on the drivers' own figures/);
  // In a check, lined up is lined up.
  assert.equal(runPhaseViews(measured({ checking: true, phases: [{ device: "Studio+", state: "applied", measured_samples: -148, applied_samples: -64 }] }))[0]?.text, "Lined up to its reference");
});

test("a check is a verdict per interface: how far apart a recording would land now", () => {
  const check = measured({
    checking: true,
    trims: [],
    readings: [heard({ device: "Quadro", is_reference: true, lag_samples: 0 }), heard({ lag_samples: 0.02 })],
  });
  assert.match(outcomeSummary(check), /^Checked what the interfaces record at 96 kHz, 512 samples, against Quadro/);
  assert.match(outcomeSummary(check), /writes nothing/);
  const verdicts = checkVerdicts(check);
  assert.equal(verdicts.length, 1, "the reference has no verdict of its own");
  assert.deepEqual(verdicts[0], { device: "Studio+", text: "Lined up: a recording would land 0.02 samples late against Quadro.", tone: "good" });
  const out = checkVerdicts({ ...check, readings: [heard({ lag_samples: -32 })] })[0];
  assert.equal(out?.tone, "off");
  assert.match(String(out?.text), /^Out: a recording would land 32 samples early against Quadro\. Measure again, then check\./);
  assert.equal(checkVerdicts({ ...check, readings: [heard({ lag_samples: 0 })] })[0]?.text, "Lined up: a recording would land in step with Quadro.");
  assert.match(String(checkVerdicts({ ...check, readings: [heard({ clicks_found: 0 })] })[0]?.text), /Nothing was heard/);
  assert.equal(checkVerdicts({ ...check, readings: [heard({ drift: { samples_per_second: 1, ppm: 10, real: true } })] })[0]?.tone, "drift");
  assert.equal(checkVerdicts({ ...check, readings: [heard({ lag_samples: CHECK_TOLERANCE_SAMPLES })] })[0]?.tone, "off", "a whole sample out is out");
  assert.deepEqual(checkVerdicts(measured()), [], "a measurement is not a verdict");
});

test("a reading says what its interface lost, and how steady it was against what is allowed", () => {
  const lost = readingView(heard({ blocks_dropped: 3, blocks_starved: 1 }));
  assert.equal(lost.lost, "Its audio dropped 3 blocks and missed 1 block while this ran, so the clicks were measured across a fault.");
  assert.equal(readingView(heard({ blocks_dropped: 0, blocks_starved: 0 })).lost, undefined);
  assert.equal(readingView(heard({ device: "Quadro", is_reference: true, lag_samples: 0, blocks_starved: 2 })).tone, "off", "a reference that lost blocks is not a good reading");
  assert.equal(readingView(heard({ spread_limit_samples: 7 })).spread, "Clicks agreed to 0.3 samples, within the 7 allowed");
  const wide = readingView(heard({ lag_samples: 0, spread_samples: 12.4, spread_limit_samples: 7 }));
  assert.equal(wide.spread, "Clicks disagreed by 12.4 samples, past the 7 allowed");
  assert.equal(wide.tone, "off");
});

test("a channel the run listened in on is named by its interface and that interface's number, and changes no trim", () => {
  const it = twoInterfaces([{}, { input_names: { "1": "SPDIF L" } }]);
  const inputs = interfaceChannels(it.config, it.answer, true);
  const run = measured({ witnesses: [{ channel: 1, device: "Studio+", lag_samples: 3.2, spread_samples: 0.1, clicks_found: 8, clicks_expected: 8, note: "Studio+ 2 recorded it 3.2 samples late." }] });
  const views = witnessViews(run, inputs);
  assert.equal(views[0]?.channel, "SPDIF L (Studio+ 2)");
  const unlisted = measured({ witnesses: [{ channel: 8, device: "Studio+", lag_samples: 3.2, spread_samples: 0.1, clicks_found: 8 }] });
  assert.equal(witnessViews(unlisted, inputs)[0]?.channel, "Studio+ 9", "one the page does not list is still named, from one");
  assert.equal(views[0]?.lag, "3.2 samples late");
  assert.equal(views[0]?.clicks, "8 of 8 clicks found");
  assert.deepEqual(witnessViews(measured(), inputs), []);
  assert.deepEqual(trimsToApply(run), trimsToApply(measured()));
});

// ---------------------------------------------------------------------------------------------
// The log, and the reason that points at the phase setup
// ---------------------------------------------------------------------------------------------

test("the log's lines are in words, and a Gazelle measurement's lines say they are Gazelle's", () => {
  assert.deepEqual(eventView({ at: "2026-09-21 21:14:09", kind: "phase", message: "Studio+ was measured at -148 samples, and held back by 64 samples." }), {
    at: "2026-09-21 21:14:09",
    kind: "Phase measured",
    message: "Studio+ was measured at -148 samples, and held back by 64 samples.",
    gazelle: false,
    problem: false,
  });
  assert.equal(eventView({ at: "x", kind: "glitched", message: "Studio+ dropped a block" }).kind, "Lost a block");
  assert.equal(eventView({ at: "x", kind: "glitched", message: "Studio+ dropped a block" }).problem, true);
  assert.equal(eventView({ at: "x", kind: "phase", message: "Studio+ was not lined up: nothing arrived." }).problem, true);
  assert.equal(eventView({ at: "x", kind: "session-ended", message: "ran for 47 minutes" }).kind, "Session ended");
  assert.equal(eventView({ at: "x", kind: "brand-new", message: "" }).kind, "brand-new");
  const ours = eventView({ at: "x", kind: "phase", message: `${GAZELLE_MEASUREMENT} Studio+ was measured at -148 samples.` });
  assert.equal(ours.gazelle, true);
  assert.equal(ours.message, "Studio+ was measured at -148 samples.");
});

test("a phase not measured says where on the page to set it up", () => {
  const it = twoInterfaces();
  const reason = { code: "phase_not_measured" as const, severity: "warning" as const, message: "Studio+ has a S/PDIF cable from Quadro and has not been set up for phase measurement.", device: "Studio+", device_id: "loopback-1" };
  assert.match(String(reasonHint(reason)), /Phase on Studio\+'s card/);
  assert.match(String(reasonHint(reason)), /A trim does not answer this/);
  assert.equal(reasonCard(reason, it.config), 1);
  assert.equal(reasonCard({ ...reason, device: "Gone" }, it.config), undefined);
  assert.equal(reasonHint({ code: "no_cable", severity: "blocking", message: "" }), undefined);
  assert.equal(reasonCard({ code: "no_cable", severity: "blocking", message: "", device: "Studio+" }, it.config), undefined);
});

test("nothing the phase writes carries an en or em dash", () => {
  const texts = [
    referenceText({ was: null, now: -84 }),
    referenceText({ was: -84, now: null }),
    phaseSetupView({}, false, "Quadro").note,
    phaseSetupView(phased, false, "Quadro").note,
    phaseSetupView(phased, true, "Quadro").note,
    phaseRoutingNote("Quadro", "Studio+", undefined),
    livePhaseView(liveDevice("Studio+", { phase: "too_far", phase_measured: 600, phase_applied: 0 }))?.text,
    runCleanText(measured({ clean: false, blocks_lost: 2 }))?.text,
    phaseRefusedText(measured({ phases: [{ device: "Studio+", state: "not_heard", measured_samples: 0, applied_samples: 0 }] })),
    checkVerdicts(measured({ checking: true, readings: [heard({ lag_samples: 40 })] }))[0]?.text,
    outcomeSummary(measured({ checking: true })),
    readingView(heard({ blocks_dropped: 1 })).lost,
    reasonHint({ code: "phase_not_measured", severity: "warning", message: "", device: "Studio+" }),
  ];
  for (const text of texts) assert.doesNotMatch(String(text), DASHES, String(text));
});

// ---------------------------------------------------------------------------------------------
// What a press came to
// ---------------------------------------------------------------------------------------------

test("a buffer match says how many took it, and names every one that refused", () => {
  const all: AggregateMatchBuffers = { buffer_size: 256, changed: 2, refused: 0, devices: [] };
  assert.deepEqual(matchBuffersText(all), { text: "2 devices put on 256 samples.", problem: false });
  const some: AggregateMatchBuffers = {
    buffer_size: 256,
    changed: 1,
    refused: 1,
    devices: [{ device: "Studio+", error: { code: "asio_in_use", message: "A program is using its ASIO interface." } }],
  };
  const said = matchBuffersText(some);
  assert.equal(said.problem, true);
  assert.match(said.text, /1 device put on 256 samples, 1 refused\. Studio\+: A program is using its ASIO interface\./);
});

test("a declined administrator prompt is said plainly and is not a failure", () => {
  const run = (parts: Partial<AggregateRegistrationRun["run"]>): AggregateRegistrationRun => ({
    dll: "C:\\Gazelle\\gazelle_aggregate.dll",
    command: "regsvr32 C:\\Gazelle\\gazelle_aggregate.dll",
    run: { started: true, exit_code: 0, message: "The driver was registered.", ...parts },
  });
  assert.deepEqual(registrationText(run({}), false), { text: "Registering finished. The driver was registered.", problem: false });
  assert.equal(registrationText(run({ started: false, message: "The prompt was declined." }), false).problem, false);
  assert.match(registrationText(run({ started: false }), false).text, /declined/);
  assert.equal(registrationText(run({ exit_code: 5, message: "Access is denied." }), true).problem, true);
  assert.match(registrationText(run({ exit_code: 5, message: "Access is denied." }), true).text, /^Unregistering failed \(code 5\)/);
});

test("nothing this module writes carries an en or em dash", () => {
  const texts = [
    statusLine(silent),
    statusLine(live()),
    gapView(liveDevice("A", { sample_gap: 5 })).text,
    matchBuffersText({ buffer_size: 64, changed: 1, refused: 0, devices: [] }).text,
    registrationText({ dll: "x", command: "y", run: { started: false, message: "" } }, false).text,
    calibrateStepText({ state: "running", step: "Playing the clicks", progress: 0.5 }),
    outcomeSummary(measured()),
    readingView(heard({ drift: { samples_per_second: 0.6, ppm: 6.25, real: true } })).drift ?? "",
    readingView(heard()).lag,
    appliedTrimsText(trimsToApply(measured())),
    String(calibrateProblem({ ...defaultPicks(twoInterfaces().config, twoInterfaces().answer), outputs: [1, 1] }, twoInterfaces().config, twoInterfaces().answer)),
    cablingSteps(defaultPicks(twoInterfaces().config, twoInterfaces().answer), twoInterfaces().config, twoInterfaces().answer)[0]?.text ?? "",
  ];
  for (const text of texts) assert.doesNotMatch(text, DASHES, text);
});

// ---------------------------------------------------------------------------------------------
// The model: polling, stopping, and the calls a button makes
// ---------------------------------------------------------------------------------------------

interface Recorded {
  calls: string[];
  timers: ManualTimers;
  model: AggregateModel;
  /** What the next read answers, or throws. */
  next: { answer?: AggregateAnswer; error?: unknown };
  /** What the measurement route answers, or throws, and what starting one throws. */
  calibration: { state: AggregateCalibration; error?: unknown; refuse?: unknown };
}

function model(first: AggregateAnswer = answer()): Recorded {
  const timers = new ManualTimers();
  const recorded: Recorded = { calls: [], timers, model: undefined as unknown as AggregateModel, next: { answer: first }, calibration: { state: { state: "idle" } } };
  const context: AggregateContext = {
    read: async () => {
      recorded.calls.push("read");
      if (recorded.next.error !== undefined) throw recorded.next.error;
      return recorded.next.answer as AggregateAnswer;
    },
    matchBuffers: async (size, options) => {
      recorded.calls.push(`match:${size}${options?.force === true ? ":force" : ""}`);
      return { buffer_size: size, changed: 1, refused: 0, devices: [] };
    },
    register: async () => {
      recorded.calls.push("register");
      return { dll: "d", command: "c", run: { started: true, exit_code: 0, message: "Registered." } };
    },
    unregister: async () => {
      recorded.calls.push("unregister");
      return { dll: "d", command: "c", run: { started: true, exit_code: 0, message: "Unregistered." } };
    },
    command: async (deviceId, name, args) => {
      recorded.calls.push(`command:${deviceId}:${name}:${JSON.stringify(args)}`);
      return true;
    },
    calibration: async () => {
      recorded.calls.push("calibration");
      if (recorded.calibration.error !== undefined) throw recorded.calibration.error;
      return recorded.calibration.state;
    },
    calibrate: async (request) => {
      recorded.calls.push(`calibrate:${request.direction}:${ends(request.outputs)}:${ends(request.inputs)}:${request.clicks}:${request.level_dbfs}`);
      if (recorded.calibration.refuse !== undefined) throw recorded.calibration.refuse;
      recorded.calibration.state = { state: "running", step: "Playing the clicks", progress: 0.25 };
      return { started: true };
    },
    stopCalibrate: async () => {
      recorded.calls.push("stop-calibrate");
      const wasRunning = recorded.calibration.state.state === "running";
      recorded.calibration.state = { state: "idle" };
      return { stopped: wasRunning };
    },
    timers,
  };
  recorded.model = new AggregateModel(context);
  return recorded;
}

test("nothing is asked for until a page is watching, and nothing more once it has gone", async () => {
  const it = model();
  assert.deepEqual(it.calls, [], "a model nobody is looking at asks nothing");
  const stop = it.model.activate();
  await flush();
  const reads = () => it.calls.filter((call) => call === "read").length;
  assert.deepEqual(it.calls, ["read", "calibration"], "the answer, and once whether a measurement is going");
  assert.deepEqual(it.timers.pending(), [AGGREGATE_POLL_MS], "and asks again at the silent rate");

  it.timers.advance(AGGREGATE_POLL_MS);
  await flush();
  assert.equal(reads(), 2);

  stop();
  assert.deepEqual(it.timers.pending(), [], "the page going takes the next read with it");
  it.timers.advance(AGGREGATE_POLL_MS * 4);
  await flush();
  assert.equal(reads(), 2, "and no more are made");
});

test("a driver that is streaming is read every second, and stopping goes back to the slow rate", async () => {
  const it = model(answer({ status: live() }));
  const stop = it.model.activate();
  await flush();
  assert.deepEqual(it.timers.pending(), [AGGREGATE_LIVE_POLL_MS]);

  it.next = { answer: answer({ status: silent }) };
  it.timers.advance(AGGREGATE_LIVE_POLL_MS);
  await flush();
  assert.deepEqual(it.timers.pending(), [AGGREGATE_POLL_MS]);
  stop();
});

test("a server that does not offer the routes is said once and never asked again", async () => {
  const it = model();
  it.next = { error: new GazelleError("http_404", "GET /api/v1/aggregate returned HTTP 404") };
  const stop = it.model.activate();
  await flush();
  assert.equal(it.model.offered.value, false);
  assert.equal(it.model.problem.value, undefined, "not offering it is not a failure to report");
  assert.deepEqual(it.timers.pending(), [], "and there is nothing to keep asking for");
  it.timers.advance(AGGREGATE_POLL_MS * 10);
  await flush();
  assert.equal(it.calls.filter((call) => call === "read").length, 1);
  stop();
});

test("a read that failed for another reason says so and keeps trying", async () => {
  const it = model();
  it.next = { error: new Error("the server went away") };
  const stop = it.model.activate();
  await flush();
  assert.equal(it.model.offered.value, true);
  assert.match(String(it.model.problem.value), /could not be read: the server went away/);
  assert.deepEqual(it.timers.pending(), [AGGREGATE_POLL_MS]);

  it.next = { answer: answer() };
  it.timers.advance(AGGREGATE_POLL_MS);
  await flush();
  assert.equal(it.model.problem.value, undefined, "an answer clears it");
  stop();
});

test("each fix sends its own request and nothing else, and then the answer is read again", async () => {
  const it = model();
  await it.model.applyFix(fix({ kind: "set_sample_rate", route: "devices/serial:S/command/set_samp_rate", body: { srate_idx: 4 } }));
  assert.deepEqual(it.calls, ['command:serial:S:set_samp_rate:{"srate_idx":4}', "read"]);

  it.calls.length = 0;
  await it.model.applyFix(fix({ kind: "match_buffers", route: "aggregate/match-buffers", body: { buffer_size: 512 } }));
  assert.deepEqual(it.calls, ["match:512", "read"]);
  assert.deepEqual(it.model.outcome.value, { text: "1 device put on 512 samples.", problem: false });

  it.calls.length = 0;
  await it.model.applyFix(fix({}));
  assert.deepEqual(it.calls, ["register", "read"]);

  it.calls.length = 0;
  await it.model.applyFix(fix({ route: "aggregate/unregister" }));
  assert.deepEqual(it.calls, ["unregister", "read"]);
});

test("a buffer match a program refused can be sent again forced", async () => {
  const it = model();
  await it.model.matchBuffers(256, { force: true });
  assert.deepEqual(it.calls, ["match:256:force", "read"]);
});

test("a fix this page does not know sends nothing and says why", async () => {
  const it = model();
  await it.model.applyFix(fix({ route: "aggregate/do-something-new" }));
  assert.deepEqual(it.calls, [], "nothing was sent");
  assert.equal(it.model.outcome.value?.problem, true);
  assert.match(String(it.model.outcome.value?.text), /newer than this page/);
});

test("a call that threw leaves the page saying so rather than silently doing nothing", async () => {
  const it = model();
  const failing = new AggregateModel({
    read: async () => answer(),
    matchBuffers: async () => {
      throw new GazelleError("asio_in_use", "A program is using the driver's ASIO interface.");
    },
    register: async () => {
      throw new GazelleError("dll_not_found", "gazelle_aggregate.dll was not found.");
    },
    unregister: async () => {
      throw new Error("no");
    },
    command: async () => false,
    calibration: async () => ({ state: "idle" }) as AggregateCalibration,
    calibrate: async () => {
      throw new GazelleError("asio_in_use", "A DAW has the drivers open.");
    },
    stopCalibrate: async () => ({ stopped: false }),
    timers: it.timers,
  });
  await failing.matchBuffers(256);
  assert.equal(failing.outcome.value?.problem, true);
  assert.match(String(failing.outcome.value?.text), /were not changed: A program is using/);
  await failing.setRegistered(true);
  assert.match(String(failing.outcome.value?.text), /not registered: gazelle_aggregate\.dll was not found/);
});

// ---------------------------------------------------------------------------------------------
// The measurement, through the model
// ---------------------------------------------------------------------------------------------

/** What the calls were, with the whole-answer reads left out: only the measurement's own. */
const calibrateCalls = (it: Recorded) => it.calls.filter((call) => call !== "read");

test("a page opening asks once whether a measurement is going, and nothing more while none is", async () => {
  const it = model();
  const stop = it.model.activate();
  await flush();
  assert.deepEqual(calibrateCalls(it), ["calibration"]);
  assert.equal(it.model.calibration.value?.state, "idle");
  // Only the whole answer is on a timer: an idle measurement is nothing to keep asking about.
  assert.deepEqual(it.timers.pending(), [AGGREGATE_POLL_MS]);
  it.timers.advance(AGGREGATE_POLL_MS * 4);
  await flush();
  assert.deepEqual(calibrateCalls(it), ["calibration"]);
  stop();
});

test("a run that is going is asked about twice a second, and the asking stops when it does", async () => {
  const it = model();
  it.calibration.state = { state: "running", step: "Playing the clicks", progress: 0.2 };
  const stop = it.model.activate();
  await flush();
  assert.equal(it.timers.pending().includes(CALIBRATE_POLL_MS), true);
  assert.equal(calibrateStepText(it.model.calibration.value), "Playing the clicks (20%)");

  it.calibration.state = { state: "done", outcome: measured() };
  it.timers.advance(CALIBRATE_POLL_MS);
  await flush();
  assert.equal(it.model.calibration.value?.state, "done");
  assert.equal(it.timers.pending().includes(CALIBRATE_POLL_MS), false, "a finished run is nothing to keep asking about");
  const before = calibrateCalls(it).length;
  it.timers.advance(CALIBRATE_POLL_MS * 10);
  await flush();
  assert.equal(calibrateCalls(it).length, before);
  stop();
});

test("starting a run sends it and then follows it, and a page that has gone follows nothing", async () => {
  const it = model();
  const stop = it.model.activate();
  await flush();
  it.calls.length = 0;
  await it.model.startCalibration({ direction: "inputs", outputs: [{ device: 0, channel: 0 }, { device: 0, channel: 1 }], inputs: [{ device: 0, channel: 0 }, { device: 1, channel: 0 }], clicks: 8, level_dbfs: -20 });
  await flush();
  assert.deepEqual(calibrateCalls(it), ["calibrate:inputs:0.0/0.1:0.0/1.0:8:-20", "calibration"]);
  assert.equal(it.model.calibration.value?.state, "running");
  assert.equal(it.model.calibrationProblem.value, undefined);

  stop();
  const after = calibrateCalls(it).length;
  it.timers.advance(CALIBRATE_POLL_MS * 6);
  await flush();
  assert.equal(calibrateCalls(it).length, after, "the page going takes the next read with it");
});

test("a run the server will not start says why, and nothing is left saying it is running", async () => {
  const it = model();
  it.calibration.refuse = new GazelleError("asio_in_use", "A DAW has the drivers open. Close it and try again.");
  await it.model.startCalibration({ direction: "inputs", outputs: [{ device: 0, channel: 0 }, { device: 0, channel: 1 }], inputs: [{ device: 0, channel: 0 }, { device: 1, channel: 0 }], clicks: 8, level_dbfs: -20 });
  await flush();
  assert.match(String(it.model.calibrationProblem.value), /did not start: A DAW has the drivers open/);
  assert.equal(it.model.calibration.value?.state, "idle");
});

test("a run that failed carries the server's own refusal, and stopping one puts it back to idle", async () => {
  const it = model();
  it.calibration.state = { state: "failed", refusal: "Nothing arrived on Studio+ 1. Check the cable from Quadro 2 into it." };
  const stop = it.model.activate();
  await flush();
  assert.equal(it.model.calibration.value?.state, "failed");
  assert.match(String(it.model.calibration.value?.refusal), /Check the cable from Quadro 2/);

  it.calibration.state = { state: "running", step: "Playing the clicks" };
  await it.model.stopCalibration();
  await flush();
  assert.equal(it.calls.includes("stop-calibrate"), true);
  assert.equal(it.model.calibration.value?.state, "idle");
  stop();
});

test("a server too old to measure is said once, and never asked again", async () => {
  const it = model();
  it.calibration.error = new GazelleError("http_404", "GET /api/v1/aggregate/calibrate returned HTTP 404");
  const stop = it.model.activate();
  await flush();
  assert.equal(it.model.calibrationOffered.value, false);
  assert.equal(it.model.calibrationProblem.value, undefined, "not offering it is not a failure to report");
  assert.deepEqual(calibrateCalls(it), ["calibration"]);
  stop();
});

// ---------------------------------------------------------------------------------------------
// Through the store, which is how the page reaches it
// ---------------------------------------------------------------------------------------------

function store() {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"));
  const timers = new ManualTimers();
  const built = new Store(client, { timers, storage: new MemoryStorage(), requestFrame: () => {}, themeSources: [] });
  return { client, timers, store: built };
}

test("the store's aggregate asks the server only while a page is watching it", async () => {
  const it = store();
  it.client.aggregateAnswer = answer();
  await it.store.start();
  await flush();
  assert.deepEqual(it.client.aggregateCalls, [], "starting the app does not read the aggregate");

  const stop = it.store.aggregate.activate();
  await flush();
  // The whole answer, and once whether a measurement is going. This server does not measure.
  assert.deepEqual(it.client.aggregateCalls, ["read", "calibration"]);
  assert.equal(it.store.aggregate.answer.value?.ready, true);
  assert.equal(it.store.aggregate.calibrationOffered.value, false);
  stop();
});

test("a measurement started through the store sends exactly what was asked for", async () => {
  const it = store();
  it.client.aggregateAnswer = answer();
  it.client.calibration = { state: "idle" };
  await it.store.start();
  await flush();
  await it.store.aggregate.startCalibration({ direction: "inputs", outputs: [{ device: 0, channel: 0 }, { device: 0, channel: 1 }], inputs: [{ device: 0, channel: 0 }, { device: 1, channel: 0 }], clicks: 8, level_dbfs: -20 });
  await flush();
  assert.equal(it.client.aggregateCalls.includes("calibrate:inputs:0.0/0.1:0.0/1.0"), true);
  assert.equal(it.store.aggregate.calibration.value?.state, "running");
});

test("the setup is edited in the workspace, which is what exports the driver's file", async () => {
  const it = store();
  await it.store.start();
  await flush();
  assert.equal(it.store.editAggregate((current) => ({ ...current, devices: [{ key: "Zen Quadro", name: "Quadro" }] })), true);
  assert.deepEqual(it.store.workspace.value?.aggregate?.devices, [{ key: "Zen Quadro", name: "Quadro" }]);
  // The rest of the workspace is untouched by an aggregate edit.
  assert.deepEqual(it.store.workspace.value?.groups, []);
});

test("a check started through the store is sent as a check", async () => {
  const it = store();
  it.client.aggregateAnswer = answer();
  it.client.calibration = { state: "idle" };
  await it.store.start();
  await flush();
  await it.store.aggregate.startCalibration({ direction: "inputs", outputs: [{ device: 0, channel: 0 }, { device: 0, channel: 1 }], inputs: [{ device: 0, channel: 0 }, { device: 1, channel: 0 }], clicks: 8, level_dbfs: -20, check: true });
  await flush();
  assert.equal(it.client.aggregateCalls.includes("calibrate:inputs:0.0/0.1:0.0/1.0:check"), true);
});
