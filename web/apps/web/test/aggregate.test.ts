// The aggregate store: how often it asks the server, what it does when the server does not offer
// the routes at all, how a reason's `fix` becomes exactly one request, and what a gap, a stall and
// a buffer match read as. No server is started here and nothing reaches a device.

import { test } from "node:test";
import assert from "node:assert/strict";

import { GazelleError, type AggregateAnswer, type AggregateFix, type AggregateMatchBuffers, type AggregateRegistrationRun, type AggregateStatusReading } from "gazelle-audio-client";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import {
  AGGREGATE_LIVE_POLL_MS,
  AGGREGATE_OPEN_POLL_MS,
  AGGREGATE_POLL_MS,
  AggregateModel,
  autoChannelName,
  buffersMatch,
  channelCounts,
  channelLabel,
  channelSummary,
  CHANNEL_LABEL_MAX,
  deviceViews,
  fixNeedsConfirming,
  fixRequest,
  gapView,
  isExposed,
  matchedBy,
  matchNote,
  matchBuffersText,
  matchTarget,
  namedCount,
  pollDelayMs,
  registrationText,
  resolvedDeviceId,
  statusLine,
  suggestedChannelName,
  withChannelExposed,
  withChannelName,
  type AggregateContext,
  type AggregateDevice,
} from "../src/store/aggregate.ts";
import { Store } from "../src/store/store.ts";
import { device, FakeClient, flush, MemoryStorage } from "./fake-client.ts";

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
}

function model(first: AggregateAnswer = answer()): Recorded {
  const timers = new ManualTimers();
  const recorded: Recorded = { calls: [], timers, model: undefined as unknown as AggregateModel, next: { answer: first } };
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
  assert.deepEqual(it.calls, ["read"]);
  assert.deepEqual(it.timers.pending(), [AGGREGATE_POLL_MS], "and asks again at the silent rate");

  it.timers.advance(AGGREGATE_POLL_MS);
  await flush();
  assert.equal(it.calls.length, 2);

  stop();
  assert.deepEqual(it.timers.pending(), [], "the page going takes the next read with it");
  it.timers.advance(AGGREGATE_POLL_MS * 4);
  await flush();
  assert.equal(it.calls.length, 2, "and no more are made");
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
  assert.equal(it.calls.length, 1);
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
    timers: it.timers,
  });
  await failing.matchBuffers(256);
  assert.equal(failing.outcome.value?.problem, true);
  assert.match(String(failing.outcome.value?.text), /were not changed: A program is using/);
  await failing.setRegistered(true);
  assert.match(String(failing.outcome.value?.text), /not registered: gazelle_aggregate\.dll was not found/);
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
  assert.deepEqual(it.client.aggregateCalls, ["read"]);
  assert.equal(it.store.aggregate.answer.value?.ready, true);
  stop();
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
