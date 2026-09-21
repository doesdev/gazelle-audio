// The aggregate audio driver, as the Aggregate page needs it: one answer from the server, polled
// while the page is open, and the few calls a button on that page can make.
//
// The server works everything out (`GET /api/v1/aggregate`): which audio drivers this PC has,
// whether Gazelle's own is registered, each configured interface's live clock, rate and buffer, the
// driver's own record and event log, and a single ready or not ready with the reasons behind it.
// Nothing here decides any of that again. What it does is keep the answer current, turn a reason's
// `fix` into the exact call that request names, and say in one place what a gap or a stall reads as.
//
// Two things shape the polling. The route is served only on a loopback bind and only to a caller on
// the same machine, so a server reachable from the network answers 404 or refuses: that is "this
// server does not offer it", not a failure, and there is then nothing to keep asking for. And the
// answer reads the audio drivers, which is not free, so it is asked for often only when the figures
// move: every second while a DAW is streaming, less often while it merely has the driver open, and
// slowly while nothing is going on at all, which is the ordinary case.

import { signal, type ReadonlySignal } from "../core/signal.ts";
import type {
  Aggregate,
  AggregateAnswer,
  AggregateCalibrateDirection,
  AggregateCalibrateOutcome,
  AggregateCalibrateReading,
  AggregateCalibrateRequest,
  AggregateCalibrateTrim,
  AggregateCalibration,
  AggregateChannelNames,
  AggregateDeviceReport,
  AggregateDeviceStatus,
  AggregateEvent,
  AggregateFix,
  AggregateMatchBuffers,
  AggregateMatchedBy,
  AggregatePlan,
  AggregateReason,
  AggregateRegistrationRun,
  AggregateDevice,
  AggregateStatusReading,
  Timers,
} from "gazelle-audio-client";

// Elements do not import the client package, so what the page needs comes through here.
export type {
  Aggregate,
  AggregateAnswer,
  AggregateCalibrateDirection,
  AggregateCalibrateOutcome,
  AggregateCalibrateReading,
  AggregateCalibrateRequest,
  AggregateCalibrateTrim,
  AggregateCalibration,
  AggregateChannelNames,
  AggregateDevice,
  AggregateDeviceReport,
  AggregateDeviceStatus,
  AggregateEvent,
  AggregateFix,
  AggregateMatchBuffers,
  AggregateMatchedBy,
  AggregatePlan,
  AggregateReason,
  AggregateRegistrationRun,
  AggregateStatusReading,
};

/** How often the answer is asked for while nothing has the driver open, which is the usual case. */
export const AGGREGATE_POLL_MS = 5_000;
/** While a DAW holds the driver open but is not running audio. */
export const AGGREGATE_OPEN_POLL_MS = 2_000;
/** While audio is running: the gap and the counters move, and this is what makes them readable. */
export const AGGREGATE_LIVE_POLL_MS = 1_000;

/** How long to wait before asking again, from what the last answer said the driver was doing. */
export function pollDelayMs(status: AggregateStatusReading | undefined): number {
  if (status === undefined || status.state !== "read") return AGGREGATE_POLL_MS;
  if (status.streaming) return AGGREGATE_LIVE_POLL_MS;
  return status.open ? AGGREGATE_OPEN_POLL_MS : AGGREGATE_POLL_MS;
}

/**
 * The call a reason's `fix` names, taken apart so the model can make it.
 *
 * The server prepares the whole request (method, route and body) and this turns that into the one
 * call that sends it, unchanged. A route no version of this page knows is answered with
 * `undefined` rather than guessed at, so a newer server's fix simply offers no button.
 */
export type FixRequest =
  | { call: "register" }
  | { call: "unregister" }
  | { call: "match-buffers"; bufferSize: number }
  | { call: "command"; deviceId: string; command: string; args: Record<string, unknown> };

const COMMAND_ROUTE = /^devices\/([^/]+)\/command\/([a-z0-9_]+)$/;

export function fixRequest(fix: AggregateFix): FixRequest | undefined {
  if (fix.method !== "POST") return undefined;
  const body = typeof fix.body === "object" && fix.body !== null ? (fix.body as Record<string, unknown>) : {};
  if (fix.route === "aggregate/register") return { call: "register" };
  if (fix.route === "aggregate/unregister") return { call: "unregister" };
  if (fix.route === "aggregate/match-buffers") {
    const size = body["buffer_size"];
    return typeof size === "number" ? { call: "match-buffers", bufferSize: size } : undefined;
  }
  const command = COMMAND_ROUTE.exec(fix.route);
  if (command === null) return undefined;
  return { call: "command", deviceId: decodeURIComponent(command[1] as string), command: command[2] as string, args: body };
}

/**
 * Whether pressing this fix's button asks for a confirming second click, the way 48V does.
 *
 * Matching buffer sizes restarts the audio of every program using those drivers, so a DAW that is
 * recording drops out. Registering puts up Windows' own administrator prompt, which is a
 * confirmation already, and a clock or rate change carries its own Confirm on the Devices page but
 * is offered here as one press: it is the fix for a reason that already says what it will do.
 */
export function fixNeedsConfirming(fix: AggregateFix): boolean {
  return fix.kind === "match_buffers";
}

/** What a device's gap reads as. `tone` is what the page colours it by; zero is the good one. */
export interface GapView {
  text: string;
  tone: "good" | "off" | "stalled" | "idle";
}

/**
 * The gap between one device and the one driving the callback, in samples.
 *
 * It means something only while both are streaming: the driver reports this device's sample count
 * minus the master's, so zero is the two of them in step and anything that keeps growing in one
 * direction is two clocks that are not the same clock. A stalled device is said first, because its
 * inputs are reading as silence and its outputs are muted whatever its counter says.
 */
export function gapView(device: AggregateDeviceStatus): GapView {
  if (device.stalled) return { text: "Stalled", tone: "stalled" };
  if (device.is_master) return { text: "Master", tone: "good" };
  if (!device.streaming) return { text: "Not streaming", tone: "idle" };
  if (device.sample_gap === 0) return { text: "In step", tone: "good" };
  const behind = device.sample_gap < 0;
  const samples = Math.abs(device.sample_gap);
  return { text: `${samples} sample${samples === 1 ? "" : "s"} ${behind ? "behind" : "ahead"}`, tone: "off" };
}

/** The one line at the top of the page: whether a DAW has the driver, and what it is doing. */
export function statusLine(status: AggregateStatusReading | undefined): string {
  if (status === undefined) return "The driver has not been asked about yet.";
  if (status.state !== "read") return status.message;
  if (status.streaming) return "A DAW has the aggregate open and audio is running.";
  if (status.open) return "A DAW has the aggregate open, and audio is not running.";
  return "The driver is loaded and nothing is using it.";
}

/** What matching buffer sizes came to, as one line. */
export function matchBuffersText(result: AggregateMatchBuffers): { text: string; problem: boolean } {
  const { buffer_size, changed, refused } = result;
  const devices = (count: number) => `${count} ${count === 1 ? "device" : "devices"}`;
  if (refused === 0) return { text: `${devices(changed)} put on ${buffer_size} samples.`, problem: false };
  const why = result.devices
    .filter((device) => device.error !== undefined)
    .map((device) => `${device.device}: ${device.error?.message ?? ""}`)
    .join(" ");
  return { text: `${devices(changed)} put on ${buffer_size} samples, ${refused} refused. ${why}`.trim(), problem: true };
}

/** What registering or unregistering came to, as one line. A declined prompt is not a failure. */
export function registrationText(run: AggregateRegistrationRun, undo: boolean): { text: string; problem: boolean } {
  const what = undo ? "Unregistering" : "Registering";
  if (!run.run.started) return { text: `${what} was not started: the administrator prompt was declined.`, problem: false };
  if (run.run.exit_code !== undefined && run.run.exit_code !== 0) return { text: `${what} failed (code ${run.run.exit_code}). ${run.run.message}`, problem: true };
  return { text: `${what} finished. ${run.run.message}`, problem: false };
}

/**
 * One configured interface, with whatever the driver is saying about it now beside it.
 *
 * The two halves come from different places and are joined by the name the setup gives a device,
 * which is the only thing both sides carry: the report is what Gazelle read about the interface,
 * the live part is what the driver published while a DAW had it open. A device the driver names and
 * the setup does not is kept rather than dropped, because a driver running a plan the setup no
 * longer describes is exactly what somebody needs to see.
 */
export interface AggregateDeviceView {
  name: string;
  report?: AggregateDeviceReport;
  live?: AggregateDeviceStatus;
}

export function deviceViews(answer: AggregateAnswer | undefined): AggregateDeviceView[] {
  if (answer === undefined) return [];
  const live = new Map((answer.status.state === "read" ? answer.status.devices : []).map((device) => [device.name, device]));
  const views: AggregateDeviceView[] = answer.devices.map((report) => {
    const found = live.get(report.name);
    live.delete(report.name);
    return found === undefined ? { name: report.name, report } : { name: report.name, report, live: found };
  });
  for (const [name, device] of live) views.push({ name, live: device });
  return views;
}

// ---------------------------------------------------------------------------------------------
// Which Gazelle device an entry is
// ---------------------------------------------------------------------------------------------

/**
 * The device this entry turned out to be: what the server resolved, and failing that whatever the
 * workspace pinned.
 *
 * The server settles this the same way for every route, so an entry nobody has pinned still has a
 * device behind it whenever the model and what is connected leave only one answer. The page reads
 * its clock, rate and buffer through this, which is why they are live without anybody choosing.
 */
export function resolvedDeviceId(view: AggregateDeviceView | undefined, device: AggregateDevice | undefined): string | undefined {
  const resolved = view?.report?.device_id;
  const configured = typeof device?.device_id === "string" ? device.device_id : undefined;
  return resolved ?? configured;
}

/**
 * How that device was arrived at. A server too old to say falls back to the plain reading: an id
 * is one somebody pinned, and no id is nothing to go on.
 */
export function matchedBy(view: AggregateDeviceView | undefined): AggregateMatchedBy {
  const report = view?.report;
  if (report?.matched_by !== undefined) return report.matched_by;
  return report?.device_id === undefined ? "none" : "chosen";
}

/** Why the device could not be told, when it could not. Nothing at all when it could. */
export function matchNote(view: AggregateDeviceView | undefined): string | undefined {
  return matchedBy(view) === "none" ? view?.report?.match_note : undefined;
}

// ---------------------------------------------------------------------------------------------
// The channels of one interface
// ---------------------------------------------------------------------------------------------

/** The most a channel label may be, in characters, which is what the driver's file takes. */
export const CHANNEL_LABEL_MAX = 31;

/** How many channels an interface has each way, or nothing where that is not known yet. */
export interface ChannelCounts {
  inputs: number | undefined;
  outputs: number | undefined;
}

/**
 * How many channels to list for one interface.
 *
 * The count the driver published while a DAW had it open is the true one, because it is what the
 * vendor driver itself said. Gazelle's own channel names are the fallback. With neither, the
 * number is not known, and a page offers nothing to edit rather than guessing at one.
 */
export function channelCounts(view: AggregateDeviceView | undefined): ChannelCounts {
  const names = view?.report?.channels;
  const count = (published: number | undefined, listed: string[] | undefined): number | undefined => {
    if (typeof published === "number" && published > 0) return published;
    return listed !== undefined && listed.length > 0 ? listed.length : undefined;
  };
  return { inputs: count(view?.live?.inputs, names?.inputs), outputs: count(view?.live?.outputs, names?.outputs) };
}

/** What a channel is called when nobody has named it: the interface's name and its number from one. */
export function autoChannelName(interfaceName: string, channel: number): string {
  return `${interfaceName} ${channel + 1}`;
}

/** Gazelle's own name for a channel, when it has one. It is a suggestion, never written by itself. */
export function suggestedChannelName(names: AggregateChannelNames | undefined, input: boolean, channel: number): string | undefined {
  if (names === undefined || names.source !== "gazelle") return undefined;
  const found = (input ? names.inputs : names.outputs)[channel];
  return found === undefined || found.trim() === "" ? undefined : found;
}

/** The name the workspace gives a channel, by the device's own numbering from zero. */
export function channelLabel(names: Record<string, string> | undefined, channel: number): string {
  const found = names?.[String(channel)];
  return typeof found === "string" ? found : "";
}

/** Whether a channel is exposed. Nothing chosen means every channel is, which is what absent means. */
export function isExposed(chosen: number[] | undefined, channel: number): boolean {
  return chosen === undefined || chosen.includes(channel);
}

/**
 * The `inputs` or `outputs` field after exposing or not exposing one channel.
 *
 * Absent means all of them, both in the workspace and in the driver's file, so the field appears
 * only once something is not exposed and goes away again the moment everything is.
 */
export function withChannelExposed(chosen: number[] | undefined, count: number, channel: number, exposed: boolean): number[] | undefined {
  const all = Array.from({ length: count }, (_, at) => at);
  const kept = new Set(chosen === undefined ? all : chosen.filter((one) => Number.isInteger(one) && one >= 0 && one < count));
  if (exposed) kept.add(channel);
  else kept.delete(channel);
  if (kept.size === count) return undefined;
  return [...kept].sort((a, b) => a - b);
}

/**
 * The `input_names` or `output_names` map after naming, or un-naming, one channel.
 *
 * A label that is empty or only spaces means the channel is not named, so its entry comes out
 * rather than being written as an empty string, and a map with nothing left in it goes away too.
 */
export function withChannelName(names: Record<string, string> | undefined, channel: number, label: string): Record<string, string> | undefined {
  const next = { ...(names ?? {}) };
  const trimmed = label.trim().slice(0, CHANNEL_LABEL_MAX);
  if (trimmed === "") delete next[String(channel)];
  else next[String(channel)] = trimmed;
  return Object.keys(next).length === 0 ? undefined : next;
}

/** How many of a device's channels carry a name, counting only ones the interface actually has. */
export function namedCount(names: Record<string, string> | undefined, count: number | undefined): number {
  return Object.entries(names ?? {}).filter(([key, value]) => {
    const channel = Number(key);
    if (!Number.isInteger(channel) || channel < 0 || value.trim() === "") return false;
    return count === undefined || channel < count;
  }).length;
}

/**
 * The one line that stands for a card's Channels part while it is closed: what is exposed, and how
 * many channels have been given a name of their own.
 */
export function channelSummary(device: AggregateDevice, counts: ChannelCounts): string {
  const exposed = (chosen: number[] | undefined) => (chosen === undefined ? "All" : String(chosen.length));
  const named = namedCount(device.input_names, counts.inputs) + namedCount(device.output_names, counts.outputs);
  const shown = `${exposed(device.inputs)} in, ${exposed(device.outputs)} out`;
  return named === 0 ? shown : `${shown}, ${named} named`;
}

/**
 * The buffer size to put every interface on, when somebody presses Match.
 *
 * The one that drives the callback decides, because that is the clock everything else is padded
 * against; the setup's own choice comes next, and failing both, the first size that could be read.
 * `undefined` means nothing is known to match to, and the button has nothing to do.
 */
export function matchTarget(answer: AggregateAnswer | undefined): number | undefined {
  if (answer === undefined) return undefined;
  const master = answer.devices.find((device) => device.is_master)?.driver.buffer_size;
  if (master !== undefined) return master;
  if (typeof answer.config?.buffer_size === "number") return answer.config.buffer_size;
  return answer.devices.map((device) => device.driver.buffer_size).find((size) => size !== undefined);
}

/** Whether every interface whose buffer could be read is already on one size. */
export function buffersMatch(answer: AggregateAnswer | undefined): boolean {
  const sizes = (answer?.devices ?? []).map((device) => device.driver.buffer_size).filter((size): size is number => size !== undefined);
  return sizes.length > 0 && sizes.every((size) => size === sizes[0]);
}

// ---------------------------------------------------------------------------------------------
// Lining the interfaces up
// ---------------------------------------------------------------------------------------------
//
// The aggregate lines its interfaces up from the latency figures their own drivers report, and
// those figures are a little wrong, so two interfaces can still record a few tens of samples
// apart. The trims put that right, and this is how the numbers to put in them are arrived at
// rather than guessed: one click, played out of a real output, recorded by both interfaces at
// once, through the aggregate itself. Both copies left the same device on the same sample, so
// wherever they land differently in the recording, that difference is the error.

/** What the setup calls one interface, which is the name every answer joins it by. */
export function deviceName(device: AggregateDevice, index: number): string {
  return device.name ?? device.key ?? device.clsid ?? `Interface ${index + 1}`;
}

/** This configured interface's place in the answer, by name first and then by position. */
export function viewFor(answer: AggregateAnswer | undefined, device: AggregateDevice, index: number): AggregateDeviceView | undefined {
  const views = deviceViews(answer);
  const name = device.name ?? device.key;
  return views.find((view) => view.name === name) ?? views[index];
}

/**
 * One channel of the aggregate's own list: the number a DAW sees it at, and what it is called.
 *
 * The number counts from zero over the channels the aggregate actually exposes, in the order the
 * interfaces are in, inputs and outputs counted separately. It is what the measurement is asked
 * for, because the run happens through the aggregate and not through one interface's own driver.
 */
export interface AggregateChannel {
  number: number;
  /** The interface it is on, by the name the setup gives it. */
  device: string;
  /** Its number on that interface, from zero. */
  channel: number;
  /** The name it has of its own, which is the automatic one where nobody has named it. */
  label: string;
  /** The interface's name and the channel's number from one, always. */
  auto: string;
  /** What to call it in a sentence: the name, with the automatic one after it when they differ. */
  text: string;
}

/** Every input, or every output, the aggregate exposes, in the order a DAW sees them. */
export function aggregateChannels(config: Aggregate | undefined, answer: AggregateAnswer | undefined, input: boolean): AggregateChannel[] {
  const listed: AggregateChannel[] = [];
  const devices = config?.devices ?? [];
  for (const [index, device] of devices.entries()) {
    const named = deviceName(device, index);
    const counts = channelCounts(viewFor(answer, device, index));
    const count = input ? counts.inputs : counts.outputs;
    if (count === undefined) continue;
    for (let channel = 0; channel < count; channel += 1) {
      if (!isExposed(device[input ? "inputs" : "outputs"], channel)) continue;
      const auto = autoChannelName(named, channel);
      const label = channelLabel(device[input ? "input_names" : "output_names"], channel) || auto;
      listed.push({ number: listed.length, device: named, channel, label, auto, text: label === auto ? auto : `${label} (${auto})` });
    }
  }
  return listed;
}

/** The channels of the aggregate's list that are on one interface. */
export function channelsOf(channels: AggregateChannel[], device: string): AggregateChannel[] {
  return channels.filter((one) => one.device === device);
}

/** How a channel number reads in a sentence, for a channel that is no longer in the list. */
export function channelText(channels: AggregateChannel[], number: number | undefined): string {
  if (number === undefined) return "not chosen";
  return channels.find((one) => one.number === number)?.text ?? `channel ${number + 1}`;
}

/** The clicks and the level the measurement is offered with, and what it starts at. */
export const CLICKS = [4, 8, 16, 32];
export const DEFAULT_CLICKS = 8;
export const LEVELS_DBFS = [-30, -20, -12, -6];
export const DEFAULT_LEVEL_DBFS = -20;

/**
 * What the person has chosen for a run.
 *
 * `outputs` and `inputs` are one per interface, in the order the interfaces are in, so
 * `outputs[n]` is the channel cabled into `inputs[n]`. A channel not chosen yet is `undefined`
 * rather than a guess, and a run with one of those in it is not offered.
 */
export interface CalibratePicks {
  direction: AggregateCalibrateDirection;
  /** The one interface every cable has an end on, by the name the setup gives it. */
  reference: string;
  outputs: (number | undefined)[];
  inputs: (number | undefined)[];
  clicks: number;
  level_dbfs: number;
}

/** The interfaces a run is over, by the names the setup gives them, in their own order. */
export function calibrateDevices(config: Aggregate | undefined): string[] {
  return (config?.devices ?? []).map((device, index) => deviceName(device, index));
}

/** Which side of the cabling is all on one interface: the outputs play them all, or one records them all. */
export function sharedSide(direction: AggregateCalibrateDirection): "outputs" | "inputs" {
  return direction === "inputs" ? "outputs" : "inputs";
}

/**
 * The interface one picker may choose from: the reference on the side that is all on one
 * interface, and the interface the row is about on the other.
 */
export function slotDevice(direction: AggregateCalibrateDirection, reference: string, side: "outputs" | "inputs", at: number, devices: string[]): string {
  return side === sharedSide(direction) ? reference : (devices[at] ?? "");
}

/**
 * Where to start: the clicks leave the reference for the input pass and arrive at it for the
 * output pass, so one side takes a channel of the reference per interface, in turn, and the other
 * takes the first channel of each interface. The reference's own pair is the loop back into
 * itself, which is what makes its reading the zero everything else is measured against.
 */
export function defaultPicks(
  config: Aggregate | undefined,
  answer: AggregateAnswer | undefined,
  direction: AggregateCalibrateDirection = "inputs",
  reference?: string,
): CalibratePicks {
  const devices = calibrateDevices(config);
  const against = reference !== undefined && devices.includes(reference) ? reference : (devices[0] ?? "");
  const lists = { outputs: aggregateChannels(config, answer, false), inputs: aggregateChannels(config, answer, true) };
  const pick = (side: "outputs" | "inputs", at: number) => {
    const offered = channelsOf(lists[side], slotDevice(direction, against, side, at, devices));
    return offered[side === sharedSide(direction) ? at : 0]?.number;
  };
  return {
    direction,
    reference: against,
    outputs: devices.map((_, at) => pick("outputs", at)),
    inputs: devices.map((_, at) => pick("inputs", at)),
    clicks: DEFAULT_CLICKS,
    level_dbfs: DEFAULT_LEVEL_DBFS,
  };
}

/**
 * What was chosen, made to fit what the aggregate is now.
 *
 * The picks are kept for the tab rather than saved, so an interface added or a channel no longer
 * exposed can leave a choice pointing at nothing. Every choice that still names a channel on the
 * interface its picker offers is kept exactly as it was; everything else falls back to where the
 * defaults would have put it, so a poll landing never moves a menu somebody has just set.
 */
export function reconcilePicks(stored: CalibratePicks | undefined, config: Aggregate | undefined, answer: AggregateAnswer | undefined): CalibratePicks {
  const base = defaultPicks(config, answer, stored?.direction, stored?.reference);
  if (stored === undefined) return base;
  const devices = calibrateDevices(config);
  const lists = { outputs: aggregateChannels(config, answer, false), inputs: aggregateChannels(config, answer, true) };
  const keep = (side: "outputs" | "inputs", at: number): number | undefined => {
    const chosen = stored[side][at];
    const found = lists[side].find((one) => one.number === chosen);
    const wanted = slotDevice(base.direction, base.reference, side, at, devices);
    return found !== undefined && found.device === wanted ? found.number : base[side][at];
  };
  return {
    ...base,
    outputs: base.outputs.map((_, at) => keep("outputs", at)),
    inputs: base.inputs.map((_, at) => keep("inputs", at)),
    clicks: CLICKS.includes(stored.clicks) ? stored.clicks : base.clicks,
    level_dbfs: LEVELS_DBFS.includes(stored.level_dbfs) ? stored.level_dbfs : base.level_dbfs,
  };
}

/** One cable to patch, in the words of the channels the pickers name. */
export interface CalibrateCable {
  from: string;
  to: string;
  text: string;
}

/**
 * What to plug in, named channel by channel, so a person with cables in hand can just patch it.
 * One line per interface: the output that carries the click, and the input that records it.
 */
export function cablingSteps(picks: CalibratePicks, config: Aggregate | undefined, answer: AggregateAnswer | undefined): CalibrateCable[] {
  const outputs = aggregateChannels(config, answer, false);
  const inputs = aggregateChannels(config, answer, true);
  return calibrateDevices(config).map((_, at) => {
    const from = channelText(outputs, picks.outputs[at]);
    const to = channelText(inputs, picks.inputs[at]);
    return { from, to, text: `${from} into ${to}` };
  });
}

/**
 * Why this run cannot be made, in words, or nothing when it can. It is the one place that decides
 * whether the button does anything, so the element asks and does not work it out again.
 */
export function calibrateProblem(picks: CalibratePicks, config: Aggregate | undefined, answer: AggregateAnswer | undefined): string | undefined {
  const devices = calibrateDevices(config);
  if (devices.length < 2) return "The aggregate needs at least two interfaces before there is anything to line up.";
  if (picks.outputs.length !== devices.length || picks.inputs.length !== devices.length) return "Every interface needs one output and one input chosen.";
  if (picks.outputs.includes(undefined) || picks.inputs.includes(undefined)) return "Every interface needs one output and one input chosen.";
  if (new Set(picks.outputs).size !== picks.outputs.length) return "Two interfaces cannot take their click from one output. Choose a different output for each.";
  if (new Set(picks.inputs).size !== picks.inputs.length) return "Two interfaces cannot record on one input. Choose a different input for each.";
  const outputs = aggregateChannels(config, answer, false);
  const inputs = aggregateChannels(config, answer, true);
  const on = (list: AggregateChannel[], number: number | undefined) => list.find((one) => one.number === number)?.device;
  const shared = picks.direction === "inputs" ? picks.outputs.map((number) => on(outputs, number)) : picks.inputs.map((number) => on(inputs, number));
  const spread = picks.direction === "inputs" ? picks.inputs.map((number) => on(inputs, number)) : picks.outputs.map((number) => on(outputs, number));
  if (shared.some((device) => device !== picks.reference)) {
    return picks.direction === "inputs"
      ? `Every click has to be played by one interface. Choose outputs on ${picks.reference}.`
      : `Every click has to be recorded by one interface. Choose inputs on ${picks.reference}.`;
  }
  if (spread.some((device, at) => device !== devices[at])) {
    return picks.direction === "inputs" ? "Each interface records on its own input." : "Each interface plays from its own output.";
  }
  if (!CLICKS.includes(picks.clicks)) return "Choose how many clicks to play.";
  return undefined;
}

/** The request that starts this run, or nothing when the picks do not make one. */
export function calibrateRequest(picks: CalibratePicks, config: Aggregate | undefined, answer: AggregateAnswer | undefined): AggregateCalibrateRequest | undefined {
  if (calibrateProblem(picks, config, answer) !== undefined) return undefined;
  return {
    direction: picks.direction,
    outputs: picks.outputs.filter((number): number is number => number !== undefined),
    inputs: picks.inputs.filter((number): number is number => number !== undefined),
    clicks: picks.clicks,
    level_dbfs: picks.level_dbfs,
  };
}

/** Whether a run is going, which is the only time the page asks the server about one again. */
export function calibrateRunning(state: AggregateCalibration | undefined): boolean {
  return state?.state === "running";
}

/** The line while it runs: the step the server names, and how far along it says it is. */
export function calibrateStepText(state: AggregateCalibration | undefined): string {
  if (!calibrateRunning(state)) return "";
  const step = state?.step ?? "Measuring";
  const progress = state?.progress;
  return typeof progress === "number" && Number.isFinite(progress) ? `${step} (${Math.round(Math.min(1, Math.max(0, progress)) * 100)}%)` : step;
}

/** How far along, from 0 to 1, for the bar. Nothing known reads as nothing done. */
export function calibrateProgress(state: AggregateCalibration | undefined): number {
  const progress = state?.progress;
  if (!calibrateRunning(state) || typeof progress !== "number" || !Number.isFinite(progress)) return 0;
  return Math.min(1, Math.max(0, progress));
}

/** A number of samples as this page writes one: at most one decimal, and no trailing zero. */
function samples(value: number): string {
  const rounded = Number(value.toFixed(1));
  return `${rounded} sample${Math.abs(rounded) === 1 ? "" : "s"}`;
}

/** One interface's reading, in the words the page shows. `tone` is what it is coloured by. */
export interface ReadingView {
  device: string;
  /** How far out it is: the reference is the zero, and everything else is early or late by so much. */
  lag: string;
  /** How much the clicks disagreed with each other, which is how much the reading is worth. */
  spread: string;
  clicks: string;
  /** The server's own sentence for this interface, when it sends one. */
  note?: string;
  /** The serious finding, when there is one: two clocks, which no trim can put right. */
  drift?: string;
  tone: "good" | "off" | "drift";
}

/**
 * What one interface's reading says.
 *
 * A drift finding is said before anything else about it, because it is the one finding a trim
 * cannot answer: the interfaces are not sharing a clock, and a number that is right now will be
 * wrong in a minute.
 */
export function readingView(reading: AggregateCalibrateReading): ReadingView {
  const drift = reading.drift;
  const real = drift !== undefined && drift.real;
  const lag = reading.is_reference
    ? "The one everything else is measured against"
    : reading.lag_samples === 0
      ? "In step"
      : `${samples(Math.abs(reading.lag_samples))} ${reading.lag_samples > 0 ? "late" : "early"}`;
  return {
    device: reading.device,
    lag,
    spread: reading.clicks_found === 0 ? "Nothing to measure" : `Clicks agreed to ${samples(reading.spread_samples)}`,
    clicks:
      reading.clicks_expected === undefined
        ? `${reading.clicks_found} click${reading.clicks_found === 1 ? "" : "s"} found`
        : `${reading.clicks_found} of ${reading.clicks_expected} clicks found`,
    ...(reading.note === undefined || reading.note.trim() === "" ? {} : { note: reading.note }),
    ...(drift === undefined
      ? {}
      : {
          drift: real
            ? `Its clock is drifting: ${samples(drift.samples_per_second)} a second, ${Number(drift.ppm.toFixed(2))} ppm. The interfaces are not sharing one clock, and no trim can put that right. Check the digital cable, and that this interface is clocked from it.`
            : `No drift worth the name: ${samples(drift.samples_per_second)} a second. The clocks are holding together.`,
        }),
    tone: real ? "drift" : reading.is_reference || reading.lag_samples === 0 ? "good" : "off",
  };
}

/** Whether anything measured found a real drift, which is what the section says before the trims. */
export function driftFound(outcome: AggregateCalibrateOutcome | undefined): boolean {
  return (outcome?.readings ?? []).some((reading) => reading.drift?.real === true);
}

/** One trim the measurement implies: what it is now, what was measured, and what it would become. */
export interface TrimRow {
  device: string;
  /** "Input trim" or "Output trim", which is the field on the card it would be written into. */
  what: string;
  was: string;
  measured: string;
  now: string;
  /** Whether applying it would change anything at all. */
  changed: boolean;
  /** Why this one is not offered, when the server says it is not. */
  notApplied?: string;
}

export function trimRows(outcome: AggregateCalibrateOutcome | undefined): TrimRow[] {
  return (outcome?.trims ?? []).map((trim) => ({
    device: trim.device,
    what: trimField(trim) === "input_trim" ? "Input trim" : "Output trim",
    was: `${trim.was}`,
    measured: `${trim.measured}`,
    now: `${trim.now}`,
    changed: trim.now !== trim.was && trim.not_applied === undefined,
    ...(trim.not_applied === undefined ? {} : { notApplied: trim.not_applied }),
  }));
}

/** The setup field a trim writes: the one the server names, and failing that the pass's own. */
export function trimField(trim: AggregateCalibrateTrim): "input_trim" | "output_trim" {
  if (trim.field === "input_trim" || trim.field === "output_trim") return trim.field;
  return trim.direction === "inputs" ? "input_trim" : "output_trim";
}

/** What a finished run came to, as one line above the readings. */
export function outcomeSummary(outcome: AggregateCalibrateOutcome): string {
  const rate = `${Number((outcome.rate / 1000).toFixed(3))} kHz`;
  const pass = outcome.direction === "inputs" ? "what the interfaces record" : "what the interfaces play";
  return `Measured ${pass} at ${rate}, ${outcome.buffer_size} samples, against ${outcome.reference}. A trim is only true for this rate and this buffer size.`;
}

/** Whether there is a trim here that would change something, which is what the button is for. */
export function trimsToApply(outcome: AggregateCalibrateOutcome | undefined): AggregateCalibrateTrim[] {
  return (outcome?.trims ?? []).filter((trim) => trim.now !== trim.was && trim.not_applied === undefined);
}

/**
 * The setup with the measured trims written into it, by the name the setup gives each interface.
 *
 * Zero is written as the field being absent, as everything else on this page writes a trim; an
 * interface the measurement names that the setup no longer has is passed over rather than added,
 * and so is a trim the server says it is not offering.
 */
export function withMeasuredTrims(config: Aggregate, outcome: AggregateCalibrateOutcome | undefined): Aggregate {
  const trims = trimsToApply(outcome);
  if (trims.length === 0) return config;
  const devices = (config.devices ?? []).map((device, index) => {
    const trim = trims.find((one) => one.device === deviceName(device, index));
    if (trim === undefined) return device;
    const field = trimField(trim);
    const next = { ...device };
    if (trim.now === 0) delete next[field];
    else next[field] = trim.now;
    return next;
  });
  return { ...config, devices };
}

/** What applying the trims came to, as one line. */
export function appliedTrimsText(trims: AggregateCalibrateTrim[]): string {
  if (trims.length === 0) return "Nothing to change: every trim is already what was measured.";
  const named = trims.map((trim) => `${trim.device} ${trim.direction === "inputs" ? "in" : "out"} ${trim.now}`).join(", ");
  return `${trims.length} trim${trims.length === 1 ? "" : "s"} written: ${named}.`;
}

/** What the model needs of the client, so it can be decided without one. */
export interface AggregateContext {
  read(): Promise<AggregateAnswer>;
  matchBuffers(bufferSize: number, options?: { force?: boolean }): Promise<AggregateMatchBuffers>;
  register(): Promise<AggregateRegistrationRun>;
  unregister(): Promise<AggregateRegistrationRun>;
  /** One command to one device, as the Devices page sends one. True when it went. */
  command(deviceId: string, command: string, args: Record<string, unknown>): Promise<boolean>;
  /** The one measurement at a time that lines the interfaces up. */
  calibration(): Promise<AggregateCalibration>;
  calibrate(request: AggregateCalibrateRequest): Promise<{ started: boolean }>;
  stopCalibrate(): Promise<{ stopped: boolean }>;
  timers: Timers;
}

/** What the page is doing, so a button can say so and not be pressed twice. */
export type AggregateBusy = "reading" | "fixing" | "matching" | "registering" | "unregistering" | "calibrating" | undefined;

/**
 * How often the run being measured is asked about while it goes. It is a short run with a step and
 * a progress bar to move, and it is asked about only while it is running: an idle, done or failed
 * answer is the end of the asking until somebody presses something.
 */
export const CALIBRATE_POLL_MS = 500;

const message = (error: unknown): string => (error instanceof Error ? error.message : String(error));

/** A server that does not serve these routes at all answers one of these. */
const NOT_OFFERED = new Set(["http_404", "http_403", "not_local"]);

const notOffered = (error: unknown): boolean => {
  const code = (error as { code?: unknown } | null)?.code;
  return typeof code === "string" && NOT_OFFERED.has(code);
};

export class AggregateModel {
  readonly #context: AggregateContext;
  readonly #answer = signal<AggregateAnswer | undefined>(undefined);
  readonly #offered = signal<boolean | undefined>(undefined);
  readonly #problem = signal<string | undefined>(undefined);
  readonly #busy = signal<AggregateBusy>(undefined);
  readonly #outcome = signal<{ text: string; problem: boolean } | undefined>(undefined);
  readonly #calibration = signal<AggregateCalibration | undefined>(undefined);
  readonly #calibrationOffered = signal<boolean | undefined>(undefined);
  readonly #calibrationProblem = signal<string | undefined>(undefined);
  #watchers = 0;
  #timer: unknown;
  #calibrateTimer: unknown;
  /** Whether the measurement problem showing is one a read raised, which is the only one a read clears. */
  #calibrationProblemWasRead = false;

  constructor(context: AggregateContext) {
    this.#context = context;
  }

  /** The whole answer, or undefined until one has landed. */
  get answer(): ReadonlySignal<AggregateAnswer | undefined> {
    return this.#answer;
  }

  /**
   * Whether this server offers the aggregate routes at all: undefined until the first read settles,
   * false on a server bound off loopback, where the page says so rather than showing a failure.
   */
  get offered(): ReadonlySignal<boolean | undefined> {
    return this.#offered;
  }

  /** Why the last read did not answer, cleared by one that does. */
  get problem(): ReadonlySignal<string | undefined> {
    return this.#problem;
  }

  get busy(): ReadonlySignal<AggregateBusy> {
    return this.#busy;
  }

  /** The line under the buttons: what the last thing pressed came to. */
  get outcome(): ReadonlySignal<{ text: string; problem: boolean } | undefined> {
    return this.#outcome;
  }

  /** The one measurement at a time, as the server has it now, or undefined until one has landed. */
  get calibration(): ReadonlySignal<AggregateCalibration | undefined> {
    return this.#calibration;
  }

  /** Whether this server measures at all: false on one too old to know the route. */
  get calibrationOffered(): ReadonlySignal<boolean | undefined> {
    return this.#calibrationOffered;
  }

  /** Why the last measurement call did not work, cleared by one that does. */
  get calibrationProblem(): ReadonlySignal<string | undefined> {
    return this.#calibrationProblem;
  }

  /**
   * Keeps the answer current while the page is on screen. Returns the stop, which the page calls
   * when it goes: nothing is asked for while no page is watching, which is the whole of the rule
   * about not polling a page that is not shown.
   */
  activate(): () => void {
    this.#watchers += 1;
    if (this.#watchers === 1) {
      this.#follow();
      // Once, to find a run that was already going or one whose outcome is still there to show.
      this.#followCalibration();
    }
    let stopped = false;
    return () => {
      if (stopped) return;
      stopped = true;
      this.#watchers -= 1;
      if (this.#watchers === 0) {
        this.#context.timers.clearTimeout(this.#timer);
        this.#timer = undefined;
        this.#context.timers.clearTimeout(this.#calibrateTimer);
        this.#calibrateTimer = undefined;
      }
    };
  }

  /** Reads once, now. What a button presses after it has changed something. */
  async refresh(): Promise<void> {
    await this.#read();
  }

  #follow(): void {
    void (async () => {
      const answer = await this.#read();
      if (this.#offered.peek() === false) return; // nothing to keep asking for
      if (this.#watchers > 0) this.#timer = this.#context.timers.setTimeout(() => this.#follow(), pollDelayMs(answer?.status));
    })();
  }

  async #read(): Promise<AggregateAnswer | undefined> {
    try {
      const answer = await this.#context.read();
      this.#answer.value = answer;
      this.#offered.value = true;
      this.#problem.value = undefined;
      return answer;
    } catch (error) {
      if (notOffered(error)) {
        this.#offered.value = false;
        this.#problem.value = undefined;
      } else {
        this.#offered.value = this.#offered.peek() ?? true;
        this.#problem.value = `The aggregate could not be read: ${message(error)}`;
      }
      return undefined;
    }
  }

  /**
   * Sends exactly the request a reason's `fix` names, then reads again, so what the page shows
   * afterwards is what the server makes of it rather than what this hoped for. `force` is carried
   * to a buffer match that a program using ASIO refused.
   */
  async applyFix(fix: AggregateFix, options: { force?: boolean } = {}): Promise<void> {
    const request = fixRequest(fix);
    if (request === undefined) {
      this.#outcome.value = { text: `Gazelle does not know how to send ${fix.route}. This server is newer than this page.`, problem: true };
      return;
    }
    if (request.call === "register") return this.setRegistered(true);
    if (request.call === "unregister") return this.setRegistered(false);
    if (request.call === "match-buffers") return this.matchBuffers(request.bufferSize, options);
    this.#busy.value = "fixing";
    try {
      const sent = await this.#context.command(request.deviceId, request.command, request.args);
      this.#outcome.value = sent ? { text: `Sent ${request.command}.`, problem: false } : { text: `${request.command} was not sent.`, problem: true };
    } catch (error) {
      this.#outcome.value = { text: `${request.command} was not sent: ${message(error)}`, problem: true };
    } finally {
      this.#busy.value = undefined;
    }
    await this.refresh();
  }

  /** Puts every configured interface on one buffer size. Every program using those drivers restarts its audio. */
  async matchBuffers(bufferSize: number, options: { force?: boolean } = {}): Promise<void> {
    this.#busy.value = "matching";
    try {
      this.#outcome.value = matchBuffersText(await this.#context.matchBuffers(bufferSize, options));
    } catch (error) {
      this.#outcome.value = { text: `The buffer sizes were not changed: ${message(error)}`, problem: true };
    } finally {
      this.#busy.value = undefined;
    }
    await this.refresh();
  }

  // -------------------------------------------------------------------------------------------
  // Lining the interfaces up
  // -------------------------------------------------------------------------------------------

  /**
   * Keeps the run current while it runs, and stops the moment it is not running any more. Nothing
   * is asked for while the page is away, and nothing is asked for at all while no run is going.
   */
  #followCalibration(): void {
    void (async () => {
      const state = await this.#readCalibration();
      if (this.#watchers > 0 && calibrateRunning(state)) {
        this.#calibrateTimer = this.#context.timers.setTimeout(() => this.#followCalibration(), CALIBRATE_POLL_MS);
      }
    })();
  }

  #setCalibrationProblem(text: string | undefined, fromRead: boolean): void {
    this.#calibrationProblem.value = text;
    this.#calibrationProblemWasRead = text === undefined ? false : fromRead;
  }

  async #readCalibration(): Promise<AggregateCalibration | undefined> {
    try {
      const state = await this.#context.calibration();
      this.#calibration.value = state;
      this.#calibrationOffered.value = true;
      // A read answering clears a read that did not, and leaves a refused start where it is: what
      // the server says about the run is not an answer to why the run was never made.
      if (this.#calibrationProblemWasRead) this.#setCalibrationProblem(undefined, true);
      return state;
    } catch (error) {
      if (notOffered(error)) {
        this.#calibrationOffered.value = false;
        this.#setCalibrationProblem(undefined, true);
      } else {
        this.#calibrationOffered.value = this.#calibrationOffered.peek() ?? true;
        this.#setCalibrationProblem(`The measurement could not be read: ${message(error)}`, true);
      }
      return undefined;
    }
  }

  /**
   * Starts one measurement. It plays a click out of a real output and takes both audio drivers for
   * itself, so a server that will not start it says why, and that refusal is what the page shows
   * rather than a run that never happened.
   */
  async startCalibration(request: AggregateCalibrateRequest): Promise<void> {
    this.#busy.value = "calibrating";
    this.#setCalibrationProblem(undefined, false);
    try {
      const started = await this.#context.calibrate(request);
      if (!started.started) this.#setCalibrationProblem("The measurement did not start.", false);
    } catch (error) {
      this.#setCalibrationProblem(`The measurement did not start: ${message(error)}`, false);
    } finally {
      this.#busy.value = undefined;
    }
    // Whatever it came to, what the server says now is what the page shows, and the polling
    // picks itself up again from that.
    this.#context.timers.clearTimeout(this.#calibrateTimer);
    this.#followCalibration();
  }

  /** Stops the run that is going. */
  async stopCalibration(): Promise<void> {
    this.#busy.value = "calibrating";
    try {
      await this.#context.stopCalibrate();
      this.#setCalibrationProblem(undefined, false);
    } catch (error) {
      this.#setCalibrationProblem(`The measurement was not stopped: ${message(error)}`, false);
    } finally {
      this.#busy.value = undefined;
    }
    this.#context.timers.clearTimeout(this.#calibrateTimer);
    this.#followCalibration();
  }

  /**
   * Registers or unregisters the driver, which asks Windows for administrator rights: the prompt is
   * Windows' own, and declining it comes back as nothing started rather than as a failure.
   */
  async setRegistered(on: boolean): Promise<void> {
    this.#busy.value = on ? "registering" : "unregistering";
    try {
      const run = on ? await this.#context.register() : await this.#context.unregister();
      this.#outcome.value = registrationText(run, !on);
    } catch (error) {
      this.#outcome.value = { text: `The driver was not ${on ? "registered" : "unregistered"}: ${message(error)}`, problem: true };
    } finally {
      this.#busy.value = undefined;
    }
    await this.refresh();
  }
}
