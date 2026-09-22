// The aggregate store: how often it asks the server, what it does when the server does not offer
// the routes at all, how a reason's `fix` becomes exactly one request, and what a gap, a stall and
// a buffer match read as. No server is started here and nothing reaches a device.

import { readFileSync } from "node:fs";
import { test } from "node:test";
import assert from "node:assert/strict";

import { GazelleError, topologies, type Aggregate, type AggregateAnswer, type AggregateFix, type AggregateMatchBuffers, type AggregateRegistrationRun, type AggregateStatusReading, type DeviceMixer } from "gazelle-audio-client";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import {
  interfaceChannels,
  AGGREGATE_LIVE_POLL_MS,
  AGGREGATE_OPEN_POLL_MS,
  AGGREGATE_POLL_MS,
  AggregateModel,
  aggregateNaming,
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
  masterReference,
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
  channelName,
  channelRuns,
  channelSummary,
  inputSocketName,
  playbackOutputs,
  readGroups,
  recordingInputs,
  countedNames,
  dawLine,
  interfaceNames,
  namingGroups,
  pageNameOf,
  withPageNames,
  CHANNEL_LABEL_MAX,
  countCheck,
  dawChannelName,
  defaultPicks,
  deviceViews,
  distinctNames,
  driftFound,
  driverOfferText,
  fitLabel,
  fixNeedsConfirming,
  fixRequest,
  gapView,
  interfaceName,
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
  sourceName,
  trimRows,
  trimsToApply,
  statusLine,
  usbGroups,
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
  type NamingSources,
} from "../src/store/aggregate.ts";
import { RoutingModel, type RouteSlot } from "../src/store/routing.ts";
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
// What the aggregate calls things: Gazelle's names
// ---------------------------------------------------------------------------------------------

/** The two devices Gazelle has, as the store lists them. */
const attached = [
  { id: "serial:Q", model: "Zen Quadro Synergy Core", family: "quadro" as const, short_model: "Quadro" },
  { id: "serial:S", model: "Zen Studio+", family: "studio" as const, short_model: "Studio+" },
];

/** The person's own names for them, as the sidebar shows them. */
const ownNames = { "serial:Q": "Quadro", "serial:S": "Studio+" };

/**
 * Two interfaces, the Quadro and the Studio+, as the page has them: the setup, the answer that
 * resolves each to one of Gazelle's devices, and the naming the page works out from both.
 */
const twoInterfaces = (parts: Partial<AggregateDevice>[] = [{}, {}], sources: Partial<NamingSources> = {}) => {
  const config: Aggregate = { devices: [{ key: "Q", device_id: "serial:Q", ...parts[0] }, { key: "S", device_id: "serial:S", ...parts[1] }] };
  const read = answer({ devices: [report("Quadro", { index: 0, device_id: "serial:Q" }), report("Studio+", { index: 1, device_id: "serial:S" })] });
  const naming = aggregateNaming(config, read, { devices: attached, aliases: ownNames, ...sources });
  return { config, answer: read, naming };
};

/** The Quadro's topology position of a source group, by id, which is how a routing slot names it. */
const quadroSource = (id: string) => topologies.quadro.inputs.findIndex((group) => group.id === id);

test("an interface's aggregate channels are its USB record and playback groups, counted from the topology", () => {
  const quadro = usbGroups(topologies.quadro);
  assert.deepEqual([quadro?.record.id, quadro?.record.name, quadro?.record.channels], ["COM_REC0", "USB A REC", 16]);
  assert.deepEqual([quadro?.playback.id, quadro?.playback.name, quadro?.playback.channels], ["COM_PLAY0", "USB 1 PLAY", 16]);
  const studio = usbGroups(topologies.studio);
  assert.deepEqual([studio?.record.id, studio?.record.name, studio?.record.channels], ["USB_REC0", "USB REC", 24]);
  assert.deepEqual([studio?.playback.id, studio?.playback.name, studio?.playback.channels], ["USB_PLAY0", "USB PLAY", 24]);
  const it = twoInterfaces();
  assert.deepEqual(channelCounts(it.naming[0]), { inputs: 16, outputs: 16 }, "not fourteen, which counting its inputs gave");
  assert.deepEqual(channelCounts(it.naming[1]), { inputs: 24, outputs: 24 });
  assert.deepEqual(channelCounts(undefined), { inputs: undefined, outputs: undefined }, "nothing known is not a guess");
});

test("the topology's count is the truth, and a driver that published another is said as a check", () => {
  const published = (inputs: number, outputs: number) =>
    aggregateNaming({ devices: [{ key: "Q", device_id: "serial:Q" }] }, answer({ devices: [report("Quadro", { index: 0, device_id: "serial:Q" })], status: live({ devices: [liveDevice("Quadro", { inputs, outputs })] }) }), { devices: attached, aliases: ownNames })[0];
  assert.deepEqual(channelCounts(published(14, 8)), { inputs: 16, outputs: 16 }, "what the topology says wins over what a driver published");
  assert.match(String(countCheck(published(14, 8))), /reports 14 inputs and 8 outputs for this interface, and it has 16 USB record and 16 USB playback channels/);
  assert.equal(countCheck(published(16, 16)), undefined, "a driver that agrees is nothing to say");
  assert.equal(countCheck(twoInterfaces().naming[0]), undefined, "and so is one that published nothing");
  // A device of a model Gazelle does not know is counted by what its driver published, if anything.
  const unknown = aggregateNaming({ devices: [{ key: "Other" }] }, answer({ devices: [report("Other", { index: 0 })], status: live({ devices: [liveDevice("Other", { inputs: 8, outputs: 8 })] }) }), { devices: [] })[0];
  assert.deepEqual(channelCounts(unknown), { inputs: 8, outputs: 8 });
  assert.equal(countCheck(unknown), undefined);
});

test("an interface is called by the person's name for the device, else its model, and told apart from its twin", () => {
  assert.deepEqual(twoInterfaces().naming.map((one) => one.name), ["Quadro", "Studio+"]);
  assert.deepEqual(twoInterfaces([{}, {}], { aliases: {} }).naming.map((one) => one.name), ["Zen Quadro Synergy Core", "Zen Studio+"], "its model, where the person gave it no name");
  // An older setup's own name for an interface is not a name anything is called by any more.
  assert.equal(twoInterfaces([{ name: "Old name" }, {}]).naming[0]?.name, "Quadro");
  // Two of one model that nobody has named are told apart by their place, as the server does it.
  const twins = aggregateNaming(
    { devices: [{ key: "A", device_id: "serial:Q" }, { key: "B", device_id: "serial:Q2" }] },
    answer({ devices: [report("x", { index: 0, device_id: "serial:Q" }), report("y", { index: 1, device_id: "serial:Q2" })] }),
    { devices: [...attached, { id: "serial:Q2", model: "Zen Quadro Synergy Core", family: "quadro", short_model: "Quadro" }] },
  );
  assert.deepEqual(twins.map((one) => one.name), ["Zen Quadro Synergy Core", "Zen Quadro Synergy Core (2)"]);
  assert.deepEqual(distinctNames(["A", "a", "B"]), ["A", "a (2)", "B"]);
  // A device that is not connected is named from what the server last knew of it, then its own name for it.
  const away = aggregateNaming({ devices: [{ key: "ZenStudioTB", known: { device_id: "serial:S", family: "studio", model: "Zen Studio+" } }] }, undefined, { devices: [] });
  assert.equal(away[0]?.name, "Zen Studio+");
  const awayNamed = aggregateNaming({ devices: [{ key: "ZenStudioTB", known: { device_id: "serial:S", family: "studio", model: "Zen Studio+" } }] }, undefined, { devices: [], aliases: ownNames });
  assert.equal(awayNamed[0]?.name, "Studio+", "and by the person's name for the device it was, as the server names it");
  assert.equal(aggregateNaming({ devices: [{ key: "ZenStudioTB" }] }, answer({ devices: [report("Drum room", { index: 0 })] }), { devices: [] })[0]?.name, "Drum room");
  assert.equal(aggregateNaming({ devices: [{ key: "ZenStudioTB" }] }, undefined, { devices: [] })[0]?.name, "ZenStudioTB", "and only then the vendor driver's key");
  assert.equal(interfaceName(undefined, 2, {}), "Interface 3");
});

test("a card finds its part of the answer by its place in the setup, whatever the interface is called", () => {
  const it = twoInterfaces();
  assert.equal(viewFor(it.answer, 1)?.report?.name, "Studio+");
  assert.equal(viewFor(answer({ devices: [report("A"), report("B")] }), 1)?.report?.name, "B", "an older server's answer, by position");
  assert.deepEqual(calibrateDevices(it.config, it.naming), ["Quadro", "Studio+"]);
});

test("a source is named as the Routing page names it, with the person's own names first", () => {
  const preamp = quadroSource("PREAMP0");
  const afx = quadroSource("AFX_OUT0");
  const mix = quadroSource("MIXER_OUT0");
  const mute = quadroSource("MUTE0");
  assert.equal(sourceName(topologies.quadro, { source: preamp, channel: 0 }), "PREAMP 1");
  assert.equal(sourceName(topologies.quadro, { source: afx, channel: 2 }), "AFX OUT 3");
  assert.equal(sourceName(topologies.quadro, { source: mute, channel: 0 }), undefined, "MUTE is nothing");
  const layout: DeviceMixer = {
    mixes: [{ name: "Cue" }],
    groups: [],
    channels: [
      { id: "a", name: "", slot: 6, source: { group: preamp, channel: 1 }, sends: [] },
      { id: "b", name: "Vocal mic", slot: 7, source: { group: preamp, channel: 0 }, sends: [] },
    ],
  };
  assert.equal(sourceName(topologies.quadro, { source: preamp, channel: 0 }, layout), "Vocal mic", "a Mixer channel the person named");
  assert.equal(sourceName(topologies.quadro, { source: preamp, channel: 1 }, layout), "PREAMP 2", "one they did not name is the source itself");
  assert.equal(sourceName(topologies.quadro, { source: mix, channel: 1 }, layout), "Cue R", "a mix they named");
});

test("an input is named for what routing sends its record channel, and a typed name wins over both", () => {
  const preamp = quadroSource("PREAMP0");
  const mute = quadroSource("MUTE0");
  const layout: DeviceMixer = { mixes: [], groups: [], channels: [{ id: "b", name: "Vocal mic", slot: 7, source: { group: preamp, channel: 0 }, sends: [] }] };
  const record = [
    { source: preamp, channel: 0 },
    { source: mute, channel: 0 },
    { source: preamp, channel: 2 },
  ];
  const recordAt = usbGroups(topologies.quadro)?.recordPosition;
  const it = twoInterfaces([{}, {}], { routing: (deviceId, at) => (deviceId === "serial:Q" && at === recordAt ? record : undefined), layouts: { "serial:Q": layout } });
  const device = it.config.devices?.[0];
  const first = channelName(device, it.naming[0], true, 0);
  assert.deepEqual(first, { usb: "USB A REC 1", carries: "Vocal mic", text: "Vocal mic, USB A REC 1", automatic: "Vocal mic" }, "the person's name for the Mixer channel that takes its source");
  assert.equal(channelName(device, it.naming[0], true, 1).text, "USB A REC 2", "nothing routed is the record channel alone");
  assert.equal(channelName(device, it.naming[0], true, 1).automatic, "USB A REC 2");
  assert.equal(channelName(device, it.naming[0], true, 2).text, "PREAMP 3, USB A REC 3", "the source as the Routing page names it");
  assert.equal(channelName(it.config.devices?.[1], it.naming[1], true, 0).text, "USB REC 1", "a group not read yet is not known");
  // A name the person typed wins, on the page and for the DAW, and the automatic one stays beside it.
  const typed = channelName({ ...device, input_names: { "0": "Lead vocal" } }, it.naming[0], true, 0);
  assert.equal(typed.text, "Lead vocal, USB A REC 1");
  assert.equal(typed.typed, "Lead vocal");
  assert.equal(typed.automatic, "Vocal mic", "what comes back when it is cleared");
});

test("a re-route changes the name the page shows, and leaves a typed name alone", () => {
  const preamp = quadroSource("PREAMP0");
  const afx = quadroSource("AFX_OUT0");
  let record = [{ source: preamp, channel: 0 }, { source: preamp, channel: 1 }];
  const recordAt = usbGroups(topologies.quadro)?.recordPosition;
  const naming = () => twoInterfaces([{ input_names: { "1": "Talkback" } }, {}], { routing: (_, at) => (at === recordAt ? record : undefined) }).naming[0];
  const device: AggregateDevice = { key: "Q", input_names: { "1": "Talkback" } };
  assert.equal(channelName(device, naming(), true, 0).text, "PREAMP 1, USB A REC 1");
  record = [{ source: afx, channel: 2 }, { source: afx, channel: 3 }];
  assert.equal(channelName(device, naming(), true, 0).text, "AFX OUT 3, USB A REC 1", "it says what would be recorded now");
  assert.equal(channelName(device, naming(), true, 1).text, "Talkback, USB A REC 2", "the typed one is untouched");
});

/** The Quadro's destination groups by id, and a routing where every group an output could reach is read. */
const quadroDestination = (id: string) => topologies.quadro.outputs.findIndex((group) => group.id === id);

/** Every group the Quadro's names come from, read, and routed to MUTE unless `routes` says otherwise. */
function quadroRouting(routes: [destination: string, channel: number, source: string, from: number][]): (deviceId: string, at: number) => RouteSlot[] | undefined {
  const mute = quadroSource("MUTE0");
  const groups = new Map(namingGroups(topologies.quadro).map((at) => [at, Array.from({ length: topologies.quadro.outputs[at]?.channels ?? 0 }, () => ({ source: mute, channel: 0 }))]));
  for (const [destination, channel, source, from] of routes) {
    const slots = groups.get(quadroDestination(destination));
    if (slots !== undefined) slots[channel] = { source: quadroSource(source), channel: from };
  }
  return (deviceId, at) => (deviceId === "serial:Q" ? groups.get(at) : undefined);
}

const quadroOutput = (routing: ReturnType<typeof quadroRouting>, channel: number, layout?: DeviceMixer) => {
  const it = twoInterfaces([{}, {}], { routing, ...(layout === undefined ? {} : { layouts: { "serial:Q": layout } }) });
  return channelName(it.config.devices?.[0], it.naming[0], false, channel);
};

test("the groups an interface's names come from are its record group, its outputs and its mix inputs", () => {
  assert.deepEqual(namingGroups(topologies.quadro).map((at) => topologies.quadro.outputs[at]?.id), ["LINE_OUT0", "HEADPHONES0", "HEADPHONES1", "MONITOR0", "COM_REC0", "SPDIF_OUT0", "MIXER_IN0", "MIXER_IN1", "MIXER_IN2", "MIXER_IN3"]);
  assert.ok(!namingGroups(topologies.studio).map((at) => topologies.studio.outputs[at]?.id).includes("TB_REC0"));
  assert.deepEqual(namingGroups(undefined), []);
});

test("an output reaching Monitor L through Mix 1 is called Monitor L, with its USB channel after it", () => {
  const routing = quadroRouting([
    ["MIXER_IN0", 6, "COM_PLAY0", 0],
    ["MIXER_IN0", 7, "COM_PLAY0", 1],
    ["MONITOR0", 0, "MIXER_OUT0", 0],
    ["MONITOR0", 1, "MIXER_OUT0", 1],
  ]);
  assert.deepEqual(quadroOutput(routing, 0), { usb: "USB 1 PLAY 1", carries: "Monitor L", text: "Monitor L, USB 1 PLAY 1", automatic: "Monitor L" });
  assert.equal(quadroOutput(routing, 1).text, "Monitor R, USB 1 PLAY 2", "the second of a pair is the right side");
  // Straight to a socket, with no mix between.
  assert.equal(quadroOutput(quadroRouting([["LINE_OUT0", 1, "COM_PLAY0", 4]]), 4).text, "Line out R, USB 1 PLAY 5");
});

test("an output landing only in a mix is named for the mix channel, in the person's words where they gave them", () => {
  const routing = quadroRouting([["MIXER_IN1", 9, "COM_PLAY0", 2]]);
  assert.equal(quadroOutput(routing, 2).text, "Ch 10 in Mix 2, USB 1 PLAY 3");
  const layout: DeviceMixer = { mixes: [{}, { name: "Cue" }], groups: [], channels: [{ id: "k", name: "Click", slot: 9, source: { group: quadroSource("COM_PLAY0"), channel: 2 }, sends: [] }] };
  assert.equal(quadroOutput(routing, 2, layout).text, "Click in Cue, USB 1 PLAY 3");
});

test("an output routed nowhere says so, once everything it could reach has been read", () => {
  const nowhere = quadroOutput(quadroRouting([]), 4);
  assert.equal(nowhere.text, "USB 1 PLAY 5, not routed");
  assert.equal(nowhere.automatic, "Not routed");
  // A group not read yet leaves where it goes unknown, which is the USB channel alone.
  const full = quadroRouting([]);
  const partial = (deviceId: string, at: number) => (at === quadroDestination("MIXER_IN3") ? undefined : full(deviceId, at));
  assert.equal(quadroOutput(partial, 4).text, "USB 1 PLAY 5");
});

test("an output reaching several places is the highest ranked hardware output and a count of the rest", () => {
  const routing = quadroRouting([
    ["HEADPHONES0", 0, "COM_PLAY0", 0],
    ["MONITOR0", 0, "COM_PLAY0", 0],
  ]);
  assert.equal(quadroOutput(routing, 0).text, "Monitor L +1, USB 1 PLAY 1", "the monitors outrank the headphones");
  // A typed name still wins.
  const it = twoInterfaces([{ output_names: { "0": "Talkback" } }, {}], { routing });
  assert.equal(channelName(it.config.devices?.[0], it.naming[0], false, 0).text, "Talkback, USB 1 PLAY 1");
});

test("a re-route changes an output's name", () => {
  const before = quadroRouting([["MIXER_IN0", 6, "COM_PLAY0", 0], ["MONITOR0", 0, "MIXER_OUT0", 0]]);
  const after = quadroRouting([["MIXER_IN0", 6, "COM_PLAY0", 0], ["HEADPHONES1", 0, "MIXER_OUT0", 0]]);
  assert.equal(quadroOutput(before, 0).text, "Monitor L, USB 1 PLAY 1");
  assert.equal(quadroOutput(after, 0).text, "HP2 L, USB 1 PLAY 1");
});

test("a row says each name once: the DAW line only when it says something the name does not", () => {
  const routing = quadroRouting([]);
  const it = twoInterfaces([{}, {}], { routing, aliases: {} });
  const unrouted = channelName(it.config.devices?.[0], it.naming[0], false, 0);
  assert.equal(it.naming[0]?.dawName, "Quadro", "unnamed, the DAW's reference is the model's short form");
  assert.equal(dawLine("Quadro", 0, unrouted), "Not routed (Quadro 1)", "a label the name does not carry is said whole");
  // A label the name carries is not said again: only the reference the DAW adds.
  const routed = channelName(it.config.devices?.[0], twoInterfaces([{}, {}], { routing: quadroRouting([["MONITOR0", 0, "COM_PLAY0", 2]]), aliases: {} }).naming[0], false, 2);
  assert.equal(routed.text, "Monitor L, USB 1 PLAY 3");
  assert.equal(dawLine("Quadro", 2, routed), "... (Quadro 3)");
  // A long reference that does not fit leaves the label alone, which is the name again: no line.
  const plainUsb = channelName(it.config.devices?.[0], twoInterfaces([{}, {}], { aliases: {} }).naming[0], false, 4);
  assert.equal(plainUsb.text, "USB 1 PLAY 5");
  assert.equal(dawLine("A device name far too long to leave room", 4, plainUsb), undefined);
});

test("a device nobody named goes by its model's short form in a DAW, and a name of their own replaces it", () => {
  const shorts = [
    { id: "serial:Q", model: "Zen Quadro Synergy Core", family: "quadro" as const, short_model: "Quadro" },
    { id: "serial:S", model: "Zen Studio+", family: "studio" as const, short_model: "Studio+" },
  ];
  const unnamed = twoInterfaces([{}, {}], { devices: shorts, aliases: {} }).naming;
  assert.deepEqual(unnamed.map((one) => [one.name, one.dawName]), [["Zen Quadro Synergy Core", "Quadro"], ["Zen Studio+", "Studio+"]]);
  assert.deepEqual(twoInterfaces([{}, {}], { devices: shorts }).naming.map((one) => one.dawName), ["Quadro", "Studio+"], "the person's names replace the short forms");
  assert.deepEqual(countedNames(["Quadro", "quadro", "Studio+"]), ["Quadro", "quadro 2", "Studio+"]);
  // Two of one model nobody named: the tags differ.
  const twins = aggregateNaming(
    { devices: [{ key: "A", device_id: "serial:Q" }, { key: "B", device_id: "serial:Q2" }] },
    answer({ devices: [report("x", { index: 0, device_id: "serial:Q" }), report("y", { index: 1, device_id: "serial:Q2" })] }),
    { devices: [shorts[0] as (typeof shorts)[number], { id: "serial:Q2", model: "Zen Quadro Synergy Core", family: "quadro", short_model: "Quadro" }] },
  );
  assert.deepEqual(twins.map((one) => one.dawName), ["Quadro", "Quadro 2"]);
});

test("a run's outcome is put in the page's names, so its trims reach the right card", () => {
  const it = twoInterfaces([{}, {}], { aliases: {}, devices: attached.map((one) => ({ ...one, short_model: one.family === "quadro" ? "Quadro" : "Studio+" })) });
  const run = withPageNames(measured({ reference: "Quadro", readings: [heard({ device: "Quadro", is_reference: true }), heard({ device: "Studio+" })], trims: [{ device: "Studio+", direction: "inputs", was: 0, measured: 28, now: 28 }] }), it.naming);
  assert.equal(run?.reference, "Zen Quadro Synergy Core");
  assert.deepEqual(run?.readings.map((one) => one.device), ["Zen Quadro Synergy Core", "Zen Studio+"]);
  assert.deepEqual(withMeasuredTrims(it.config, run, interfaceNames(it.config, it.naming)).devices?.[1]?.input_trim, 28);
  assert.equal(pageNameOf("Studio+", it.naming), "Zen Studio+");
  assert.equal(pageNameOf("Someone else", it.naming), "Someone else");
});

test("the name a DAW shows is built as the driver builds it, and a long device name leaves the label alone", () => {
  assert.equal(dawChannelName("Quadro", 0, "Vocal mic"), "Vocal mic (Quadro 1)");
  assert.equal(dawChannelName("Zen Quadro Synergy Core", 0, "Vocal mic"), "Vocal mic", "no room for the reference, so the label alone");
  assert.equal(dawChannelName("Zen Quadro Synergy Core", 15, undefined), "Zen Quadro Synergy Core 16", "no label is the reference alone");
  assert.equal(dawChannelName("Studio+", 3, "USB REC 4"), "USB REC 4 (Studio+ 4)");
  for (const name of [dawChannelName("Zen Quadro Synergy Core", 9, "A label that is exactly thirty1"), dawChannelName("x".repeat(40), 99, undefined)]) assert.ok([...name].length <= CHANNEL_LABEL_MAX, name);
  assert.equal(fitLabel("x".repeat(40)).length, CHANNEL_LABEL_MAX);
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

test("naming a channel writes only what was typed, and clearing one takes the entry out rather than writing nothing", () => {
  assert.deepEqual(withChannelName(undefined, 0, "Vocal mic"), { "0": "Vocal mic" });
  assert.deepEqual(withChannelName({ "0": "Vocal mic" }, 1, "Room"), { "0": "Vocal mic", "1": "Room" });
  assert.deepEqual(withChannelName({ "0": "Vocal mic", "1": "Room" }, 1, ""), { "0": "Vocal mic" });
  assert.equal(withChannelName({ "0": "Vocal mic" }, 0, "   "), undefined, "only spaces is not a name, and the last one out takes the map with it");
  assert.deepEqual(withChannelName(undefined, 0, "  Vocal mic  "), { "0": "Vocal mic" }, "the spaces around a name are not part of it");
  const long = "x".repeat(CHANNEL_LABEL_MAX + 9);
  assert.equal(withChannelName(undefined, 0, long)?.["0"]?.length, CHANNEL_LABEL_MAX);
});

test("the names counted are the ones typed on channels the interface actually has", () => {
  assert.equal(namedCount({ "0": "Vocal mic", "19": "Talkback" }, 16), 1);
  assert.equal(namedCount({ "0": "Vocal mic", "9": "Talkback" }, undefined), 2, "with no count, every name counts");
  assert.equal(namedCount({ "0": "  " }, 4), 0);
  assert.equal(namedCount(undefined, 4), 0);
});

test("the Channels line says how many there are, what is exposed and how many are named", () => {
  const device = (parts: Partial<AggregateDevice>): AggregateDevice => ({ key: "Quadro", ...parts });
  assert.equal(channelSummary(device({}), { inputs: 16, outputs: 16 }), "All 16 in, all 16 out");
  assert.equal(channelSummary(device({ inputs: [0, 1] }), { inputs: 16, outputs: 16 }), "2 of 16 in, all 16 out");
  assert.equal(channelSummary(device({ input_names: { "0": "Vocal mic", "1": "Room" }, output_names: { "0": "Main L" } }), { inputs: 24, outputs: 24 }), "All 24 in, all 24 out, 3 named");
  assert.equal(channelSummary(device({ input_names: { "0": "Vocal mic" } }), { inputs: undefined, outputs: undefined }), "All in, all out, 1 named");
});

test("the label a channel shows is the workspace's, and a channel with none shows nothing", () => {
  assert.equal(channelLabel({ "3": "Vocal mic" }, 3), "Vocal mic");
  assert.equal(channelLabel({ "3": "Vocal mic" }, 4), "");
  assert.equal(channelLabel(undefined, 0), "");
});

test("the master is written by its registry key, which a rename cannot change", () => {
  assert.equal(masterReference({ key: "Zen Quadro Synergy Core", name: "Quadro" }), "Zen Quadro Synergy Core");
  assert.equal(masterReference({ clsid: "{AE4A4452-A316-11E5-A113-080027F6C1F4}" }), "{AE4A4452-A316-11E5-A113-080027F6C1F4}");
  assert.equal(masterReference({}), undefined);
});

test("a driver offered for adding is named for the device it would be, with its own name as a detail", () => {
  const usb = [{ id: "serial:Q", model: "Zen Quadro Synergy Core", family: "quadro" as const, backend: "usb" }, { id: "serial:S", model: "Zen Studio+", family: "studio" as const, backend: "usb" }];
  assert.equal(driverOfferText({ key: "ZenStudioTB ASIO Driver" }, usb, { "serial:S": "Drum room" }), "Drum room (ZenStudioTB ASIO Driver)");
  assert.equal(driverOfferText({ key: "Zen Quadro Synergy Core", description: "Zen Quadro Synergy Core" }, usb, {}), "Zen Quadro Synergy Core", "the same words once, not twice");
  assert.equal(driverOfferText({ key: "Focusrite USB" }, usb, {}), "Focusrite USB", "a driver naming no model is its own name");
  const twoQuadros = [...usb, { id: "serial:Q2", model: "Zen Quadro Synergy Core", family: "quadro" as const, backend: "usb" }];
  assert.equal(driverOfferText({ key: "ZenQuadro ASIO" }, twoQuadros, { "serial:Q": "Desk" }), "ZenQuadro ASIO", "two of the model is not one to name it after");
});

// ---------------------------------------------------------------------------------------------
// Lining the interfaces up: the channels, the pickers, and what to plug in
// ---------------------------------------------------------------------------------------------

test("each interface's channels are listed by that interface and its own numbering, never the aggregate's", () => {
  const it = twoInterfaces();
  const inputs = interfaceChannels(it.config, it.naming, true);
  assert.equal(inputs.length, 16 + 24, "every USB record channel of both");
  assert.deepEqual(inputs.slice(0, 2).map((one) => [one.device, one.index, one.channel, one.text]), [
    ["Quadro", 0, 0, "USB A REC 1"],
    ["Quadro", 0, 1, "USB A REC 2"],
  ]);
  assert.deepEqual(inputs[16] && [inputs[16].device, inputs[16].index, inputs[16].channel, inputs[16].text], ["Studio+", 1, 0, "USB REC 1"]);
  assert.ok(inputs.every((one) => !Object.hasOwn(one, "number")), "nothing here is a place in the aggregate's list");
  assert.equal(interfaceChannels(it.config, it.naming, false).length, 16 + 24);
  assert.deepEqual(interfaceChannels(undefined, undefined, true), [], "nothing set up is no list");
});

test("a channel kept out of the aggregate is not offered, and every other channel keeps its own number", () => {
  const it = twoInterfaces([{ inputs: [0, 3] }, { inputs: [1] }]);
  assert.deepEqual(interfaceChannels(it.config, it.naming, true).map((one) => [one.device, one.channel, one.usb]), [
    ["Quadro", 0, "USB A REC 1"],
    ["Quadro", 3, "USB A REC 4"],
    ["Studio+", 1, "USB REC 2"],
  ]);
});

test("the channels the phase measurement runs over are not offered, because the driver keeps them", () => {
  // The Studio+'s phase arrives on its second input, over the Quadro's fourth output.
  const it = twoInterfaces([{ inputs: [0, 1], outputs: [2, 3] }, { inputs: [0, 1], outputs: [0], phase: { master_output: 3, input: 1 } }]);
  assert.deepEqual(interfaceChannels(it.config, it.naming, true).map((one) => one.text), ["USB A REC 1", "USB A REC 2", "USB REC 1"]);
  assert.deepEqual(interfaceChannels(it.config, it.naming, false).map((one) => one.text), ["USB 1 PLAY 3", "USB PLAY 1"]);
});

test("a channel with a name of its own is named by it, then by its USB channel", () => {
  const it = twoInterfaces([{ input_names: { "0": "Vocal mic" } }, {}]);
  const inputs = interfaceChannels(it.config, it.naming, true);
  assert.equal(inputs[0]?.text, "Vocal mic, USB A REC 1");
  assert.equal(inputs[1]?.text, "USB A REC 2", "an unnamed one is just itself");
});

test("an interface whose channels are not known yet offers none of them rather than guessing", () => {
  const config = { devices: [{ key: "Other A" }, { key: "Other B" }] };
  const naming = aggregateNaming(config, answer({ devices: [report("A", { index: 0 }), report("B", { index: 1 })] }), { devices: [] });
  assert.deepEqual(interfaceChannels(config, naming, true), []);
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

test("the input pass starts with two outputs of the reference and one input on each interface, named as everywhere else", () => {
  const it = twoInterfaces();
  const picks = defaultPicks(it.config, it.naming, "inputs");
  assert.equal(picks.reference, "Quadro", "the first interface, until somebody says otherwise");
  assert.deepEqual(picks.outputs, [0, 1], "two outputs of one interface");
  assert.deepEqual(picks.inputs, [0, 0], "each interface's first input, by its own numbering");
  assert.deepEqual(cablingSteps(picks, it.config, it.naming).map((cable) => cable.text), ["USB 1 PLAY 1 on Quadro into USB A REC 1 on Quadro", "USB 1 PLAY 2 on Quadro into USB REC 1 on Studio+"]);
});

test("the output pass plays one output on each interface into two inputs of the reference", () => {
  const it = twoInterfaces();
  const picks = defaultPicks(it.config, it.naming, "outputs", "Studio+");
  assert.equal(picks.reference, "Studio+");
  assert.deepEqual(picks.outputs, [0, 0], "one output on each interface");
  assert.deepEqual(picks.inputs, [0, 1], "both into the Studio+");
  assert.deepEqual(cablingSteps(picks, it.config, it.naming).map((cable) => cable.text), ["USB 1 PLAY 1 on Quadro into USB REC 1 on Studio+", "USB PLAY 1 on Studio+ into USB REC 2 on Studio+"]);
});

test("a reference nothing in the setup names falls back to the first interface", () => {
  const it = twoInterfaces();
  assert.equal(defaultPicks(it.config, it.naming, "inputs", "A device that has gone").reference, "Quadro");
  assert.equal(defaultPicks(undefined, undefined).reference, "");
});

test("what was chosen is kept across a poll, and only a choice that no longer fits falls back", () => {
  const it = twoInterfaces();
  const stored: CalibratePicks = { direction: "inputs", reference: "Quadro", outputs: [2, 3], inputs: [1, 1], clicks: 16, level_dbfs: -12 };
  assert.deepEqual(reconcilePicks(stored, it.config, it.naming), stored, "every choice still names a channel its picker offers");

  // An input the Studio+ does not have: it has twenty four.
  assert.deepEqual(reconcilePicks({ ...stored, inputs: [1, 24] }, it.config, it.naming).inputs, [1, 0], "the one that does not fit, and only that one");
  // A channel that is not there any more, and a number of clicks nothing offers.
  const gone: CalibratePicks = { ...stored, outputs: [99, 3], clicks: 7 };
  assert.deepEqual(reconcilePicks(gone, it.config, it.naming).outputs, [0, 3]);
  assert.equal(reconcilePicks(gone, it.config, it.naming).clicks, 8);
  // Nothing chosen at all is simply where the defaults would have put it.
  assert.deepEqual(reconcilePicks(undefined, it.config, it.naming), defaultPicks(it.config, it.naming));
});

test("changing the pass takes the choices the other way round with it", () => {
  const it = twoInterfaces();
  const chosen: CalibratePicks = { ...defaultPicks(it.config, it.naming, "inputs"), outputs: [2, 3], clicks: 16, level_dbfs: -12 };
  const flipped = withPass(chosen, it.config, it.naming, "outputs", "Quadro");
  assert.deepEqual(flipped.outputs, [0, 0], "one output on each interface");
  assert.deepEqual(flipped.inputs, [0, 1], "both into the reference");
  assert.deepEqual([flipped.clicks, flipped.level_dbfs], [16, -12], "how many clicks and how loud stay as they were");
  const moved = withPass(chosen, it.config, it.naming, "inputs", "Studio+");
  assert.deepEqual([moved.reference, moved.outputs, moved.inputs], ["Studio+", [0, 1], [0, 0]], "another reference is other cabling too");
});

test("a run that cannot be made says why, and makes no request", () => {
  const it = twoInterfaces();
  const good = defaultPicks(it.config, it.naming);
  assert.equal(calibrateProblem(good, it.config, it.naming), undefined);
  assert.deepEqual(calibrateRequest(good, it.config, it.naming), {
    direction: "inputs",
    outputs: [{ device: 0, channel: 0 }, { device: 0, channel: 1 }],
    inputs: [{ device: 0, channel: 0 }, { device: 1, channel: 0 }],
    clicks: 8,
    level_dbfs: -20,
  });

  const one = { devices: [{ key: "Q", device_id: "serial:Q" }] };
  const alone = aggregateNaming(one, it.answer, { devices: attached, aliases: ownNames });
  assert.match(String(calibrateProblem(defaultPicks(one, alone), one, alone)), /at least two interfaces/);
  assert.equal(calibrateRequest(defaultPicks(one, alone), one, alone), undefined);

  assert.match(String(calibrateProblem({ ...good, outputs: [0, undefined] }, it.config, it.naming)), /one output and one input/);
  assert.match(String(calibrateProblem({ ...good, outputs: [1, 1] }, it.config, it.naming)), /cannot take their click from one output/);
  // The same number on two interfaces is two different inputs, and a perfectly good run.
  assert.equal(calibrateProblem({ ...good, inputs: [0, 0] }, it.config, it.naming), undefined);
  const outputPass = defaultPicks(it.config, it.naming, "outputs");
  assert.match(String(calibrateProblem({ ...outputPass, inputs: [2, 2] }, it.config, it.naming)), /cannot record on one input/);
  // A channel the interface its picker is about is not offering.
  assert.match(String(calibrateProblem({ ...good, outputs: [0, 16] }, it.config, it.naming)), /one output and one input/);
  assert.match(String(calibrateProblem({ ...good, inputs: [0, 24] }, it.config, it.naming)), /one output and one input/);
  assert.match(String(calibrateProblem({ ...good, clicks: 3 }, it.config, it.naming)), /how many clicks/);
});

/**
 * **The Check the owner pressed on 2026-09-21.** Gazelle's list had fourteen inputs for the Quadro,
 * and its driver has sixteen, so counting the aggregate's channels put the Studio+'s first input at
 * 14, which is the Quadro's fifteenth. The count is the USB group's own now, sixteen, and the run is
 * asked for by interface anyway, so no count can put a cable on the wrong interface.
 */
test("a check names each channel by interface, so no count can put a cable on the wrong interface", () => {
  const it = twoInterfaces();
  const request = calibrateRequest(defaultPicks(it.config, it.naming), it.config, it.naming, { check: true });
  assert.deepEqual(request, {
    direction: "inputs",
    outputs: [{ device: 0, channel: 0 }, { device: 0, channel: 1 }],
    inputs: [{ device: 0, channel: 0 }, { device: 1, channel: 0 }],
    clicks: 8,
    level_dbfs: -20,
    check: true,
  });
  assert.doesNotMatch(JSON.stringify(request), /1[46]/, "no number in it depends on how many channels the Quadro has");

  // The output pass the other way round: one output on each interface, both into the reference.
  const outputs = calibrateRequest(defaultPicks(it.config, it.naming, "outputs", "Studio+"), it.config, it.naming);
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

/** The trims written into a setup whose interfaces the page calls by these fixtures' own names. */
const writeTrims = (config: Aggregate, outcome: AggregateCalibrateOutcome | undefined) =>
  withMeasuredTrims(config, outcome, (config.devices ?? []).map((device) => String(device.name ?? device.key)));

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
  assert.deepEqual(writeTrims({ devices: [{ key: "S", name: "Studio+" }] }, held).devices, [{ key: "S", name: "Studio+" }], "and nothing of it is written");
  // The field the server names decides where it is written, whatever the pass was called.
  assert.equal(trimRows(measured({ trims: [{ device: "Studio+", direction: "inputs", field: "output_trim", was: 0, measured: 5, now: 5 }] }))[0]?.what, "Output trim");
});

test("applying the trims writes them into the setup, by the name the run gives each interface", () => {
  const config = { devices: [{ key: "Q", name: "Quadro" }, { key: "S", name: "Studio+", input_trim: 4 }] };
  const written = writeTrims(config, measured());
  assert.deepEqual(written.devices, [{ key: "Q", name: "Quadro" }, { key: "S", name: "Studio+", input_trim: 28 }]);
  // Zero is the field being absent, as every other trim on this page is written.
  const back = writeTrims(written, measured({ trims: [{ device: "Studio+", direction: "inputs", was: 28, measured: 0, now: 0 }] }));
  assert.deepEqual(back.devices, [{ key: "Q", name: "Quadro" }, { key: "S", name: "Studio+" }]);
  // An interface the measurement names and the setup no longer has changes nothing.
  assert.deepEqual(writeTrims({ devices: [{ key: "Q", name: "Quadro" }] }, measured()).devices, [{ key: "Q", name: "Quadro" }]);
  assert.deepEqual(writeTrims(config, undefined), config);
  // The names are the page's, which are Gazelle's: a run names the device by its name in Gazelle.
  assert.deepEqual(withMeasuredTrims({ devices: [{ key: "Q" }, { key: "S" }] }, measured(), ["Quadro", "Studio+"]).devices, [{ key: "Q" }, { key: "S", input_trim: 28 }]);
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
  const written = writeTrims({ devices: [{ key: "Q", name: "Quadro" }, { key: "S", name: "Studio+", phase: { master_output: 15, input: 8 } }] }, withReference({ was: null, now: -84 }));
  assert.deepEqual(written.devices?.[1], { key: "S", name: "Studio+", input_trim: 28, phase: { master_output: 15, input: 8, reference: -84 } });
  // A new reference replaces the old one, and the path it was measured on is left as it was.
  const again = writeTrims({ devices: [phased] }, withReference({ was: -84, now: -148 }, { was: 28, measured: 30, now: 30 }));
  assert.deepEqual(again.devices?.[0], { key: "S", name: "Studio+", input_trim: 30, phase: { master_output: 15, input: 8, reference: -148 } });
});

test("a trim offered with nothing heard on the cable takes the old reference out", () => {
  const written = writeTrims({ devices: [phased] }, withReference({ was: -84, now: null }, { was: 28, measured: 31, now: 31 }));
  assert.deepEqual(written.devices?.[0], { key: "S", name: "Studio+", input_trim: 31, phase: { master_output: 15, input: 8 } });
  // Even when the trim itself comes out the same, because the old reference no longer belongs beside it.
  const same = withReference({ was: -84, now: null }, { was: 28, measured: 28, now: 28 });
  assert.equal(trimsToApply(same).length, 1);
  assert.deepEqual(writeTrims({ devices: [phased] }, same).devices?.[0], { key: "S", name: "Studio+", input_trim: 28, phase: { master_output: 15, input: 8 } });
});

test("a first run whose trim is unchanged but whose reference is new is still there to write", () => {
  // The trim was already 28 and was measured at 28; the reference was nothing and is now -84.
  const first = withReference({ was: null, now: -84 }, { was: 28, measured: 28, now: 28 });
  assert.equal(referenceChanges(first.trims[0] as AggregateCalibrateOutcome["trims"][number]), true);
  assert.equal(trimsToApply(first).length, 1, "the button is offered");
  assert.equal(trimRows(first)[0]?.changed, true);
  const written = writeTrims({ devices: [{ key: "S", name: "Studio+", input_trim: 28, phase: { master_output: 15, input: 8 } }] }, first);
  assert.deepEqual(written.devices?.[0], { key: "S", name: "Studio+", input_trim: 28, phase: { master_output: 15, input: 8, reference: -84 } });
  assert.equal(appliedTrimsText(trimsToApply(first)), "1 trim written: Studio+ in 28 (phase reference -84).");
  // And nothing changing on either count is still nothing to write.
  const settled = withReference({ was: -84, now: -84 }, { was: 28, measured: 28, now: 28 });
  assert.equal(referenceChanges(settled.trims[0] as AggregateCalibrateOutcome["trims"][number]), false);
  assert.deepEqual(trimsToApply(settled), []);
});

test("a reference is only written where there is a phase path to go with it", () => {
  // The phase setting was cleared after the run: the trim is written, and no half setting is made up.
  const written = writeTrims({ devices: [{ key: "S", name: "Studio+" }] }, withReference({ was: null, now: -84 }));
  assert.deepEqual(written.devices?.[0], { key: "S", name: "Studio+", input_trim: 28 });
  // An output trim never carries one, whatever arrives beside it.
  const out = withReference({ was: null, now: -84 }, { direction: "outputs", field: "output_trim" });
  assert.deepEqual(writeTrims({ devices: [phased] }, out).devices?.[0], { ...phased, output_trim: 28 });
  // And one the server is not offering writes neither.
  const held = withReference({ was: null, now: -84 }, { not_applied: "Only 2 of 8 clicks were found." });
  assert.deepEqual(trimsToApply(held), []);
});

test("a check offers no trims, whatever its outcome carries", () => {
  const checked = { ...withReference({ was: null, now: -84 }), checking: true };
  assert.deepEqual(trimsToApply(checked), []);
  assert.deepEqual(writeTrims({ devices: [phased] }, checked).devices, [phased]);
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
  const picks = defaultPicks(it.config, it.naming);
  assert.equal(Object.hasOwn(calibrateRequest(picks, it.config, it.naming) ?? {}, "check"), false);
  assert.deepEqual(calibrateRequest(picks, it.config, it.naming, { check: true }), { ...calibrateRequest(picks, it.config, it.naming), check: true });
  assert.equal(calibrateRequest({ ...picks, outputs: [1, 1] }, it.config, it.naming, { check: true }), undefined, "a check that cannot be made is no request either");
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

test("the callback master is the one the setup names by key, and the first when it names none", () => {
  const it = twoInterfaces();
  assert.equal(masterIndex(it.config, it.answer), 0);
  assert.equal(masterIndex({ ...it.config, callback_master: "s" }, it.answer), 1, "by registry key, without case");
  // An older setup named it by the name it gave the device, and that still finds it.
  assert.equal(masterIndex({ devices: [{ key: "Q" }, { key: "S", name: "Old Studio" }], callback_master: "Old Studio" }, it.answer), 1);
  // One that names nothing falls back to the one the answer says is the master.
  const reported = answer({ devices: [report("Quadro", { index: 0 }), report("Studio+", { index: 1, is_master: true })] });
  assert.equal(masterIndex({ ...it.config, callback_master: "Gone" }, reported), 1);
  assert.equal(masterIndex({ devices: [] }, it.answer), undefined);
});

test("the phase pickers offer the master's own USB playback channels and this interface's own USB record channels", () => {
  const it = twoInterfaces([{ output_names: { "15": "To Studio+" } }, {}]);
  const choices = phaseChoices(it.config, it.answer, it.naming, 1);
  assert.equal(choices?.master, "Quadro");
  assert.equal(choices?.own, "Studio+");
  assert.equal(choices?.outputs?.length, 16);
  assert.deepEqual(choices?.outputs?.[0], { value: 0, text: "USB 1 PLAY 1" });
  assert.deepEqual(choices?.outputs?.[15], { value: 15, text: "To Studio+, USB 1 PLAY 16" }, "a name the person gave it first");
  assert.equal(choices?.inputs?.length, 24);
  assert.equal(choices?.inputs?.[7]?.text, "USB REC 8");
  // Not on the callback master's card, which the others are measured against.
  assert.equal(phaseChoices(it.config, it.answer, it.naming, 0), undefined);
  // A channel kept out of what a DAW sees is still offered: the driver opens these two itself.
  const hidden = twoInterfaces([{}, { inputs: [0] }]);
  assert.equal(phaseChoices(hidden.config, hidden.answer, hidden.naming, 1)?.inputs?.length, 24);
  // With no count to go on, nothing is guessed at.
  const unknown = aggregateNaming({ devices: [{ key: "Q" }, { key: "Other" }] }, answer(), { devices: [] });
  assert.equal(phaseChoices({ devices: [{ key: "Q" }, { key: "Other" }] }, answer(), unknown, 1)?.inputs, undefined);
  // And a setting made while more channels were known is kept on the list.
  assert.deepEqual(choicesWith([{ value: 0, text: "USB REC 1" }], 30, (channel) => `USB REC ${channel + 1}`).map((one) => one.text), ["USB REC 1", "USB REC 31 (not listed now)"]);
  assert.equal(choicesWith(undefined, undefined, String).length, 0);
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

test("a channel the run listened in on is named as every other channel is, and changes no trim", () => {
  const it = twoInterfaces([{}, { input_names: { "1": "SPDIF L" }, inputs: [0, 1] }]);
  const inputs = interfaceChannels(it.config, it.naming, true);
  const run = measured({ witnesses: [{ channel: 1, device: "Studio+", lag_samples: 3.2, spread_samples: 0.1, clicks_found: 8, clicks_expected: 8, note: "Studio+ 2 recorded it 3.2 samples late." }] });
  const views = witnessViews(run, inputs, it.config, it.naming);
  assert.equal(views[0]?.channel, "SPDIF L, USB REC 2");
  const unlisted = measured({ witnesses: [{ channel: 8, device: "Studio+", lag_samples: 3.2, spread_samples: 0.1, clicks_found: 8 }] });
  assert.equal(witnessViews(unlisted, inputs, it.config, it.naming)[0]?.channel, "USB REC 9", "one the page does not list is still named");
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
  assert.equal(reasonCard(reason, it.config, it.naming), 1, "by Gazelle's name for it");
  assert.equal(reasonCard({ ...reason, device: "Gone" }, it.config, it.naming), undefined);
  assert.equal(reasonCard({ ...reason, device: "Called anything", device_index: 1 }, it.config, it.naming), 1, "and by its place, where the server gives it");
  assert.equal(reasonCard({ ...reason, device_index: 7 }, it.config, it.naming), undefined, "a place the setup does not have is no card");
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
    String(calibrateProblem({ ...defaultPicks(twoInterfaces().config, twoInterfaces().naming), outputs: [1, 1] }, twoInterfaces().config, twoInterfaces().naming)),
    cablingSteps(defaultPicks(twoInterfaces().config, twoInterfaces().naming), twoInterfaces().config, twoInterfaces().naming)[0]?.text ?? "",
    String(countCheck(aggregateNaming({ devices: [{ key: "Q", device_id: "serial:Q" }] }, answer({ devices: [report("Quadro", { index: 0, device_id: "serial:Q" })], status: live({ devices: [liveDevice("Quadro", { inputs: 14, outputs: 8 })] }) }), { devices: attached })[0])),
    dawChannelName("Zen Quadro Synergy Core", 0, "Vocal mic"),
    channelName({ key: "Q" }, twoInterfaces().naming[0], true, 0).text,
    driverOfferText({ key: "ZenStudioTB" }, [{ id: "serial:S", model: "Zen Studio+", family: "studio", backend: "usb" }], {}),
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
  assert.equal(it.store.editAggregate((current) => ({ ...current, devices: [{ key: "Zen Quadro" }] })), true);
  assert.deepEqual(it.store.workspace.value?.aggregate?.devices, [{ key: "Zen Quadro" }]);
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

// ---------------------------------------------------------------------------------------------
// Outputs ranked, and where the DAW can play
// ---------------------------------------------------------------------------------------------

/** The cases the server's naming is held to as well, from the one file both sides read. */
const SHARED_CASES = JSON.parse(readFileSync(new URL("../../../../refs/fixtures/aggregate_output_names.json", import.meta.url), "utf8")) as {
  cases: { what: string; family: "quadro" | "studio"; routes: [string, number, string, number][]; names: Record<string, string> }[];
};

/** A device of either model with every group its names come from read, MUTE unless `routes` says otherwise. */
function namingWith(family: "quadro" | "studio", routes: [string, number, string, number][], layout?: DeviceMixer) {
  const topology = topologies[family];
  const mute = topology.inputs.findIndex((group) => group.id === "MUTE0");
  const groups = new Map(readGroups(topology).map((at) => [at, Array.from({ length: topology.outputs[at]?.channels ?? 0 }, () => ({ source: mute, channel: 0 }))]));
  for (const [to, channel, from, fromChannel] of routes) {
    const slots = groups.get(topology.outputs.findIndex((group) => group.id === to));
    if (slots !== undefined) slots[channel] = { source: topology.inputs.findIndex((group) => group.id === from), channel: fromChannel };
  }
  const id = family === "quadro" ? "serial:Q" : "serial:S";
  const config = { devices: [{ key: family, device_id: id }] };
  const naming = aggregateNaming(config, answer({ devices: [report(family, { index: 0, device_id: id })] }), {
    devices: attached,
    aliases: {},
    routing: (_, at) => groups.get(at),
    ...(layout === undefined ? {} : { layouts: { [id]: layout } }),
  });
  return { config, naming: naming[0], groups };
}

test("outputs are named by the same cases the server is held to, the monitors first", () => {
  assert.ok(SHARED_CASES.cases.length >= 4);
  for (const one of SHARED_CASES.cases) {
    const { config, naming } = namingWith(one.family, one.routes);
    for (const [channel, expected] of Object.entries(one.names)) assert.equal(channelName(config.devices[0], naming, false, Number(channel)).text, expected, `${one.what}: output ${channel}`);
  }
});

test("channels are listed as runs", () => {
  assert.equal(channelRuns([0, 1]), "1 to 2");
  assert.equal(channelRuns([6]), "7");
  assert.equal(channelRuns([3, 0, 1, 2, 6, 7, 9]), "1 to 4 and 7 to 8 and 10");
  assert.equal(channelRuns([1, 1, 0]), "1 to 2");
});

/** The owner's Quadro: USB 1 PLAY 1 and 2 through Mix 1 to the monitors and HP1, and the line outs fed by Mix 3, which no USB channel enters. */
const OWNER: [string, number, string, number][] = [
  ["MIXER_IN0", 16, "COM_PLAY0", 0],
  ["MIXER_IN0", 17, "COM_PLAY0", 1],
  ["MONITOR0", 0, "MIXER_OUT0", 0],
  ["MONITOR0", 1, "MIXER_OUT0", 1],
  ["HEADPHONES0", 0, "MIXER_OUT0", 0],
  ["HEADPHONES0", 1, "MIXER_OUT0", 1],
  ["LINE_OUT0", 0, "MIXER_OUT2", 0],
  ["LINE_OUT0", 1, "MIXER_OUT2", 1],
];

test("where the DAW can play: each output in rank, through a mix, directly, or nothing", () => {
  const lines = playbackOutputs(namingWith("quadro", [...OWNER, ["SPDIF_OUT0", 0, "COM_PLAY0", 4], ["HEADPHONES1", 1, "COM_PLAY0", 5], ["HEADPHONES1", 0, "MIXER_OUT0", 0]]).naming);
  assert.deepEqual(lines.map((line) => [line.label, line.state, line.text]), [
    ["Monitor", "reached", "USB 1 PLAY 1 to 2, through Mix 1"],
    ["Line out", "nothing", "nothing from the DAW reaches it"],
    ["HP1", "reached", "USB 1 PLAY 1 to 2, through Mix 1"],
    ["HP2", "reached", "USB 1 PLAY 1 to 2, through Mix 1; USB 1 PLAY 6, directly"],
    ["S/PDIF out", "reached", "USB 1 PLAY 5, directly"],
  ]);
  // The person's name for the mix.
  const named = playbackOutputs(namingWith("quadro", OWNER, { mixes: [{ name: "Cue" }], groups: [], channels: [] }).naming);
  assert.equal(named[0]?.text, "USB 1 PLAY 1 to 2, through Cue");
  // A group larger than a pair is a line per channel, as the Studio+'s line outs are.
  const studio = playbackOutputs(namingWith("studio", [["LINE_OUT0", 2, "USB_PLAY0", 2], ["LINE_OUT0", 3, "USB_PLAY0", 3]]).naming);
  assert.deepEqual(studio.slice(0, 5).map((line) => [line.label, line.text]), [
    ["Monitor", "nothing from the DAW reaches it"],
    ["Line out 1", "nothing from the DAW reaches it"],
    ["Line out 2", "nothing from the DAW reaches it"],
    ["Line out 3", "USB PLAY 3, directly"],
    ["Line out 4", "USB PLAY 4, directly"],
  ]);
  assert.equal(studio.filter((line) => line.label.startsWith("Line out ")).length, 8, "all eight line outs");
  assert.equal(studio.filter((line) => line.label.startsWith("ADAT out ")).length, 16, "and all sixteen ADAT outs");
  assert.deepEqual(studio.filter((line) => line.channels.length === 2).map((line) => line.label), ["Monitor", "HP1", "HP2", "S/PDIF out", "Reamp"], "pairs stay one line each");
  assert.equal(studio[studio.length - 1]?.label, "Reamp", "reamp comes last");
});

test("nothing reaching an output is said only once the groups behind it have been read", () => {
  const { config, groups } = namingWith("quadro", OWNER);
  const without = (missing: string) => {
    const at = topologies.quadro.outputs.findIndex((group) => group.id === missing);
    return aggregateNaming(config, answer({ devices: [report("quadro", { index: 0, device_id: "serial:Q" })] }), { devices: attached, routing: (_, g) => (g === at ? undefined : groups.get(g)) })[0];
  };
  const lineOut = (naming: ReturnType<typeof without>) => playbackOutputs(naming).find((line) => line.label === "Line out");
  assert.deepEqual([lineOut(without("MIXER_IN2"))?.state, lineOut(without("MIXER_IN2"))?.text], ["unread", "not read yet"], "the mix behind it is not read");
  assert.equal(lineOut(without("LINE_OUT0"))?.state, "unread", "its own group is not read");
  // Read, but the free channels are not all known: no button, and it says why.
  const notAll = lineOut(without("MIXER_IN3"));
  assert.equal(notAll?.state, "nothing");
  assert.equal(notAll?.send, undefined);
  assert.match(String(notAll?.noSend), /not known until the routing has been read/);
});

test("an output nothing reaches offers the first free run of its width, and says what it would stop playing", () => {
  const line = playbackOutputs(namingWith("quadro", OWNER).naming).find((one) => one.label === "Line out");
  const playback = quadroSource("COM_PLAY0");
  assert.deepEqual(line?.send?.run, [2, 3], "USB 1 PLAY 1 and 2 are in Mix 1, so the first free pair is 3 and 4");
  assert.equal(line?.send?.label, "Send USB 1 PLAY 3 to 4 here");
  assert.equal(line?.send?.title, "Line out stops playing Mix 3, and plays USB 1 PLAY 3 to 4 instead");
  assert.equal(line?.send?.destination, quadroDestination("LINE_OUT0"));
  assert.deepEqual(line?.send?.changes, [
    { channel: 0, source: { source: playback, channel: 2 } },
    { channel: 1, source: { source: playback, channel: 3 } },
  ]);
  // A single socket takes a single free channel, not a pair, and writes only its own slot.
  const studio = playbackOutputs(namingWith("studio", [["MONITOR0", 0, "USB_PLAY0", 0], ["MONITOR0", 1, "USB_PLAY0", 1], ["LINE_OUT0", 1, "USB_PLAY0", 2]]).naming);
  const single = studio.find((one) => one.label === "Line out 1");
  const studioPlay = topologies.studio.inputs.findIndex((group) => group.id === "USB_PLAY0");
  assert.deepEqual(single?.send?.run, [3], "USB PLAY 1 to 3 are in use, so the first free one is 4");
  assert.equal(single?.send?.label, "Send USB PLAY 4 here");
  assert.deepEqual(single?.send?.changes, [{ channel: 0, source: { source: studioPlay, channel: 3 } }]);
  // A pair still takes a pair that starts on an odd channel from one.
  assert.deepEqual(studio.find((one) => one.label === "HP1")?.send?.run, [4, 5]);
  // Every pair in use: no button, and it says so.
  const busy: [string, number, string, number][] = Array.from({ length: 16 }, (_, channel): [string, number, string, number] => ["MIXER_IN3", channel, "COM_PLAY0", channel]);
  const full = playbackOutputs(namingWith("quadro", [...OWNER, ...busy]).naming).find((one) => one.label === "Line out");
  assert.equal(full?.send, undefined);
  assert.match(String(full?.noSend), /none is free/);
});

test("pressing send is one routing write of the output's group, changing only its own slots", async () => {
  const line = playbackOutputs(namingWith("quadro", OWNER).naming).find((one) => one.label === "Line out");
  const send = line?.send;
  assert.ok(send !== undefined);
  const mix3 = quadroSource("MIXER_OUT2");
  const written: { destination: number; slots: readonly RouteSlot[] }[] = [];
  const routing = new RoutingModel({
    deviceId: "serial:Q",
    topology: topologies.quadro,
    read: async () => ({ slots: [{ source: mix3, channel: 0 }, { source: mix3, channel: 1 }], dryRun: false }),
    write: async (destination, slots) => {
      written.push({ destination, slots });
      return true;
    },
    notify: () => {},
  });
  assert.equal(await routing.routeMany(send.destination, send.changes), true);
  assert.equal(written.length, 1, "one write");
  const mute = quadroSource("MUTE0");
  assert.equal(written[0]?.destination, quadroDestination("LINE_OUT0"));
  assert.deepEqual(written[0]?.slots, [
    { source: quadroSource("COM_PLAY0"), channel: 2 },
    { source: quadroSource("COM_PLAY0"), channel: 3 },
    ...Array.from({ length: 30 }, () => ({ source: mute, channel: 0 })),
  ]);
});

test("nothing the playback list writes carries an en or em dash", () => {
  for (const line of playbackOutputs(namingWith("quadro", OWNER).naming)) {
    for (const text of [line.label, line.text, line.send?.label ?? "", line.send?.title ?? "", line.noSend ?? ""]) assert.doesNotMatch(text, DASHES, text);
  }
});

// ---------------------------------------------------------------------------------------------
// Where the DAW can record
// ---------------------------------------------------------------------------------------------

test("the sockets a person plugs into are the preamps, line ins, ADAT and S/PDIF, and nothing else", () => {
  const kept = (family: "quadro" | "studio") => topologies[family].inputs.filter((group) => inputSocketName(group) !== undefined).map((group) => group.id);
  assert.deepEqual(kept("quadro"), ["PREAMP0", "ADAT_IN0", "SPDIF_IN0"], "not USB PLAY, the mixes, AFX, the oscillator, MUTE or the emulated preamps");
  assert.deepEqual(kept("studio"), ["PREAMP0", "LINE_IN0", "ADAT_IN0", "SPDIF_IN0"], "and not Thunderbolt playback either");
  assert.deepEqual(readGroups(topologies.quadro).map((at) => topologies.quadro.outputs[at]?.id), ["LINE_OUT0", "HEADPHONES0", "HEADPHONES1", "MONITOR0", "COM_REC0", "SPDIF_OUT0", "AFX_IN0", "MIXER_IN0", "MIXER_IN1", "MIXER_IN2", "MIXER_IN3"], "the effect inputs are read as well");
});

test("an input is a line per socket in a larger group, and a pair is one line, in the device's own order", () => {
  const quadro = recordingInputs(namingWith("quadro", []).naming);
  assert.deepEqual(quadro.map((line) => line.label), ["Preamp 1", "Preamp 2", "Preamp 3", "Preamp 4", ...Array.from({ length: 8 }, (_, at) => `ADAT in ${at + 1}`), "S/PDIF in"]);
  assert.deepEqual(quadro[quadro.length - 1]?.channels, [0, 1], "S/PDIF in is a pair");
  const studio = recordingInputs(namingWith("studio", []).naming);
  assert.equal(studio.length, 12 + 8 + 16 + 1);
  assert.deepEqual([studio[12]?.label, studio[20]?.label, studio[36]?.label], ["Line in 1", "ADAT in 1", "S/PDIF in"]);
});

test("an input says which USB record channels carry it: directly, through a mix, through an effect, or several", () => {
  const routes: [string, number, string, number][] = [
    ["COM_REC0", 0, "PREAMP0", 0],
    ["MIXER_IN1", 6, "PREAMP0", 0],
    ["MIXER_IN1", 7, "PREAMP0", 1],
    ["COM_REC0", 2, "MIXER_OUT1", 0],
    ["COM_REC0", 3, "MIXER_OUT1", 1],
    ["AFX_IN0", 2, "ADAT_IN0", 4],
    ["COM_REC0", 5, "AFX_OUT0", 2],
  ];
  const lines = recordingInputs(namingWith("quadro", routes).naming);
  const say = (label: string) => lines.find((line) => line.label === label);
  assert.equal(say("Preamp 1")?.text, "USB A REC 1, directly; USB A REC 3 to 4, through Mix 2");
  assert.equal(say("Preamp 2")?.text, "USB A REC 3 to 4, through Mix 2");
  assert.equal(say("Preamp 2")?.state, "recorded");
  assert.equal(say("ADAT in 5")?.text, "USB A REC 6, through AFX 3");
  const named = recordingInputs(namingWith("quadro", routes, { mixes: [{}, { name: "Cue" }], groups: [], channels: [] }).naming);
  assert.equal(named.find((line) => line.label === "Preamp 2")?.text, "USB A REC 3 to 4, through Cue", "the person's name for the mix");
  // The Studio+ in the same words.
  const studio = recordingInputs(namingWith("studio", [["USB_REC0", 7, "LINE_IN0", 2]]).naming);
  assert.equal(studio.find((line) => line.label === "Line in 3")?.text, "USB REC 8, directly");
});

test("nothing records an input only once the record group, the mix inputs and the effect inputs are read", () => {
  const { config, groups } = namingWith("quadro", []);
  const without = (missing: string) => {
    const at = topologies.quadro.outputs.findIndex((group) => group.id === missing);
    return aggregateNaming(config, answer({ devices: [report("quadro", { index: 0, device_id: "serial:Q" })] }), { devices: attached, routing: (_, g) => (g === at ? undefined : groups.get(g)) })[0];
  };
  assert.deepEqual([recordingInputs(namingWith("quadro", []).naming)[0]?.state, recordingInputs(namingWith("quadro", []).naming)[0]?.text], ["nothing", "nothing records it"]);
  for (const missing of ["COM_REC0", "MIXER_IN3", "AFX_IN0"]) {
    const first = recordingInputs(without(missing))[0];
    assert.deepEqual([first?.state, first?.text, first?.send], ["unread", "not read yet", undefined], missing);
  }
});

test("an input nothing records offers the first free record channels of its width, free being routed from MUTE", () => {
  const preamp = quadroSource("PREAMP0");
  const lines = recordingInputs(namingWith("quadro", [["COM_REC0", 0, "PREAMP0", 0]]).naming);
  const second = lines.find((line) => line.label === "Preamp 2");
  assert.deepEqual(second?.send?.run, [1], "a single socket takes a single channel");
  assert.equal(second?.send?.label, "Record it on USB A REC 2");
  assert.equal(second?.send?.title, "USB A REC 2 records nothing now, and records Preamp 2 instead");
  assert.equal(second?.send?.destination, usbGroups(topologies.quadro)?.recordPosition);
  assert.deepEqual(second?.send?.changes, [{ channel: 1, source: { source: preamp, channel: 1 } }]);
  const spdif = lines.find((line) => line.label === "S/PDIF in");
  assert.deepEqual(spdif?.send?.run, [2, 3], "a pair takes a pair that starts on an odd channel from one");
  assert.equal(spdif?.send?.label, "Record it on USB A REC 3 to 4");
  assert.deepEqual(spdif?.send?.changes, [
    { channel: 2, source: { source: quadroSource("SPDIF_IN0"), channel: 0 } },
    { channel: 3, source: { source: quadroSource("SPDIF_IN0"), channel: 1 } },
  ]);
});

/**
 * The Quadro fills a USB record slot nobody uses with (0, 0), which is PREAMP 1, not MUTE: such a
 * slot is recording something and is not free.
 */
test("a slot the Quadro filled with PREAMP 1 is not free, and with none free there is no button", () => {
  const padded: [string, number, string, number][] = Array.from({ length: 16 }, (_, at): [string, number, string, number] => ["COM_REC0", at, "PREAMP0", 0]);
  const someFree = padded.filter(([, at]) => at !== 8 && at !== 9);
  const lines = recordingInputs(namingWith("quadro", someFree).naming);
  assert.equal(lines.find((line) => line.label === "Preamp 1")?.text, "USB A REC 1 to 8 and 11 to 16, directly");
  assert.deepEqual(lines.find((line) => line.label === "S/PDIF in")?.send?.run, [8, 9], "the only slots routed from MUTE");
  assert.deepEqual(lines.find((line) => line.label === "ADAT in 1")?.send?.run, [8]);
  const full = recordingInputs(namingWith("quadro", padded).naming).find((line) => line.label === "S/PDIF in");
  assert.equal(full?.state, "nothing");
  assert.equal(full?.send, undefined);
  assert.match(String(full?.noSend), /Every USB record channel is in use/);
});

test("pressing record is one routing write of the record group, changing only the chosen slots", async () => {
  const padded: [string, number, string, number][] = Array.from({ length: 16 }, (_, at): [string, number, string, number] => ["COM_REC0", at, "PREAMP0", 0]);
  const routes = padded.filter(([, at]) => at !== 8 && at !== 9);
  const send = recordingInputs(namingWith("quadro", routes).naming).find((line) => line.label === "S/PDIF in")?.send;
  assert.ok(send !== undefined);
  const mute = quadroSource("MUTE0");
  const read = Array.from({ length: 64 }, (_, at) => (at === 8 || at === 9 || at >= 16 ? { source: mute, channel: 0 } : { source: 0, channel: 0 }));
  const written: { destination: number; slots: readonly RouteSlot[] }[] = [];
  const routing = new RoutingModel({
    deviceId: "serial:Q",
    topology: topologies.quadro,
    read: async () => ({ slots: read, dryRun: false }),
    write: async (destination, slots) => {
      written.push({ destination, slots });
      return true;
    },
    notify: () => {},
  });
  assert.equal(await routing.routeMany(send.destination, send.changes), true);
  assert.equal(written.length, 1, "one write");
  const spdif = quadroSource("SPDIF_IN0");
  const expected = read.slice(0, 32).map((slot, at) => (at === 8 ? { source: spdif, channel: 0 } : at === 9 ? { source: spdif, channel: 1 } : at >= 16 ? { source: mute, channel: 0 } : slot));
  assert.equal(written[0]?.destination, quadroDestination("COM_REC0"));
  assert.deepEqual(written[0]?.slots, expected, "every other slot as it was read, PREAMP 1 padding included");
});

test("the recording list on the Studio+ offers single channels for its single sockets", () => {
  const lines = recordingInputs(namingWith("studio", [["USB_REC0", 0, "PREAMP0", 0]]).naming);
  assert.equal(lines[0]?.text, "USB REC 1, directly");
  assert.equal(lines[1]?.send?.label, "Record it on USB REC 2");
  assert.equal(lines[lines.length - 1]?.send?.label, "Record it on USB REC 3 to 4", "S/PDIF in, a pair, takes the first free pair");
  for (const line of lines) for (const text of [line.label, line.text, line.send?.label ?? "", line.send?.title ?? "", line.noSend ?? ""]) assert.doesNotMatch(text, DASHES, text);
});
