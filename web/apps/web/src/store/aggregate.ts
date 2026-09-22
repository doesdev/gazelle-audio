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
  AggregateCalibrateChannel,
  AggregateCalibrateDirection,
  AggregateCalibrateOutcome,
  AggregateCalibratePhase,
  AggregateCalibrateReading,
  AggregateCalibrateRequest,
  AggregateCalibrateTrim,
  AggregateCalibrateWitness,
  AggregateCalibration,
  AggregateDeviceReport,
  AggregateDeviceStatus,
  AggregateEvent,
  AggregateFix,
  AggregateMatchBuffers,
  AggregateMatchedBy,
  AggregatePhaseSetting,
  AggregatePlan,
  AggregateReason,
  AggregateRegistrationRun,
  AggregateDevice,
  AggregateStatusReading,
  AggregateTrimReference,
  AggregateUsbChannels,
  Cable,
  DeviceMixer,
  Timers,
  Topology,
  TopologyGroup,
} from "gazelle-audio-client";
import { topologies as builtInTopologies } from "gazelle-audio-client";
import { sourceLabel } from "./channels.ts";
import type { RouteSlot } from "./routing.ts";

// Elements do not import the client package, so what the page needs comes through here.
export type {
  Aggregate,
  AggregateAnswer,
  AggregateCalibrateChannel,
  AggregateCalibrateDirection,
  AggregateCalibrateOutcome,
  AggregateCalibratePhase,
  AggregateCalibrateReading,
  AggregateCalibrateRequest,
  AggregateCalibrateTrim,
  AggregateCalibrateWitness,
  AggregateCalibration,
  AggregateDevice,
  AggregateDeviceReport,
  AggregateDeviceStatus,
  AggregateEvent,
  AggregateFix,
  AggregateMatchBuffers,
  AggregateMatchedBy,
  AggregatePhaseSetting,
  AggregatePlan,
  AggregateReason,
  AggregateRegistrationRun,
  AggregateStatusReading,
  AggregateTrimReference,
  AggregateUsbChannels,
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
 * The two halves come from different places and are joined by name, which is the only thing both
 * sides carry: the report is what Gazelle read about the interface, named by Gazelle's name for the
 * device, and the live part is what the driver published while a DAW had it open, named by the name
 * Gazelle gave it in its file, which is the same name. A device the driver names and the setup does
 * not is kept rather than dropped, because a driver running a plan the setup no longer describes
 * (one from before a rename, until its next reset) is exactly what somebody needs to see.
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
// What the aggregate calls things: Gazelle's names, and one naming for the whole page
// ---------------------------------------------------------------------------------------------
//
// An interface is called by Gazelle's name for its device: the person's own name for it where they
// gave one, else its model, exactly what the sidebar shows. There is no second name to keep in step:
// renaming the device in Gazelle renames it here and in what the DAW sees.
//
// An aggregate channel is one of the interface's USB audio channels, not a preamp or an output.
// Input k is the interface's USB record channel k, and output k its USB playback channel k, which
// was measured at the hardware channel by channel. So a channel is named first for what it carries
// and then by that USB channel: an input for what Gazelle's routing sends to its record channel
// ("Vocal mic, USB A REC 1"), an output for the Mixer channel that plays it where the person named
// one ("Click, USB 1 PLAY 1"). A name the person types for a channel wins over both.
//
// The server works the very same names out for the driver's file (its naming module), and keeps
// them in step with the routing; the rules here are those rules, for what this page shows.

/**
 * Which routing groups a model's aggregate channels are, by topology id: the group the inputs record
 * from and the group the outputs play into. The Studio+'s `TB_REC` and `TB_PLAY` are its
 * Thunderbolt audio, which Gazelle does not use.
 */
export const AGGREGATE_USB_GROUPS: Readonly<Record<string, { record: string; playback: string }>> = {
  quadro: { record: "COM_REC0", playback: "COM_PLAY0" },
  studio: { record: "USB_REC0", playback: "USB_PLAY0" },
};

/** An interface's USB channels, as its model's topology has them. */
export interface UsbGroups {
  /** The destination group its aggregate inputs record from. */
  record: TopologyGroup;
  /** Its place among the destinations, which `get_routing` reads it by. */
  recordPosition: number;
  /** The source group its aggregate outputs play into. */
  playback: TopologyGroup;
  /** Its place among the sources, which is how a routing slot names it. */
  playbackPosition: number;
}

export function usbGroups(topology: Topology | undefined): UsbGroups | undefined {
  const ids = topology === undefined ? undefined : AGGREGATE_USB_GROUPS[topology.family];
  if (topology === undefined || ids === undefined) return undefined;
  const recordPosition = topology.outputs.findIndex((group) => group.id === ids.record);
  const playbackPosition = topology.inputs.findIndex((group) => group.id === ids.playback);
  const record = topology.outputs[recordPosition];
  const playback = topology.inputs[playbackPosition];
  return record === undefined || playback === undefined ? undefined : { record, recordPosition, playback, playbackPosition };
}

/** What to call a model when nothing better is known, as the rest of the app names it. */
const FAMILY_WORDS: Readonly<Record<string, string>> = { quadro: "Zen Quadro Synergy Core", studio: "Zen Studio+" };

/** Everything one interface's names are worked out from, as the page has it now. */
export interface InterfaceNaming {
  /** Gazelle's name for the device, told apart from the others in the setup. */
  name: string;
  /** The Gazelle device it is, when that is known. */
  deviceId?: string;
  topology?: Topology;
  /** What its routing sends to each of its USB record channels, once that group has been read. */
  record?: readonly RouteSlot[];
  /** Its Mixer layout, where the person's own names for channels and mixes are. */
  layout?: DeviceMixer;
  /** How many channels it has each way: its USB groups' own counts, else what a running driver said. */
  inputs?: number;
  outputs?: number;
  /** What a running driver published, kept beside the topology's count as a check. */
  published?: { inputs?: number; outputs?: number };
}

/** The naming of every interface of the setup, in its order. */
export type AggregateNaming = readonly InterfaceNaming[];

/** Where the naming comes from: the store's devices, the workspace, and the routing it has read. */
export interface NamingSources {
  devices: readonly { id: string; model: string | null; family: string | null }[];
  aliases?: Readonly<Record<string, string>> | undefined;
  layouts?: Readonly<Record<string, DeviceMixer>> | undefined;
  /** Every model's topology; the client's own when not given. */
  topologies?: Readonly<Record<string, Topology>>;
  /** One routing group of one device as read, or nothing while it has not been. */
  record?: (deviceId: string, destination: number) => readonly RouteSlot[] | undefined;
}

/**
 * Names made distinct without case: the later of two alike takes its place in the setup from one
 * after it. Two interfaces of one model that nobody has named would otherwise come out the same,
 * and the server tells them apart in exactly this way.
 */
export function distinctNames(names: readonly string[]): string[] {
  return names.map((name, at) => (names.slice(0, at).some((earlier) => earlier.toLowerCase() === name.toLowerCase()) ? `${name} (${at + 1})` : name));
}

const positive = (count: number | undefined): number | undefined => (typeof count === "number" && count > 0 ? count : undefined);

/**
 * How every interface of the setup is named, and what its channels are named from.
 *
 * Which device an entry is comes from the answer, as everything else on the page does. Its name is
 * the person's own name for that device, else its model; with neither known, its model as last
 * known, then the server's own name for it, then the vendor driver's key.
 */
export function aggregateNaming(config: Aggregate | undefined, answer: AggregateAnswer | undefined, sources: NamingSources): AggregateNaming {
  const built = (config?.devices ?? []).map((device, index): InterfaceNaming => {
    const view = viewFor(answer, index);
    const known = device.known;
    // The device it is now, else the one it was last known to be, which is how the server names an
    // interface that is unplugged, so the page and the driver's file call it alike.
    const id = resolvedDeviceId(view, device) ?? known?.device_id;
    const attached = id === undefined ? undefined : sources.devices.find((one) => one.id === id);
    const family = attached?.family ?? view?.report?.family ?? known?.family ?? undefined;
    const topology = family === undefined || family === null ? undefined : (sources.topologies ?? (builtInTopologies as Readonly<Record<string, Topology>>))[family];
    const alias = id === undefined ? undefined : sources.aliases?.[id]?.trim();
    const name =
      (alias === undefined || alias === "" ? undefined : alias) ??
      attached?.model ??
      known?.model ??
      (family === undefined || family === null ? undefined : FAMILY_WORDS[family]) ??
      view?.report?.name ??
      device.key ??
      device.clsid ??
      `Interface ${index + 1}`;
    const groups = usbGroups(topology);
    const live = view?.live;
    const reported = view?.report?.channels;
    const record = groups !== undefined && id !== undefined ? sources.record?.(id, groups.recordPosition) : undefined;
    const layout = id === undefined ? undefined : sources.layouts?.[id];
    const inputs = groups?.record.channels ?? positive(live?.inputs) ?? positive(reported?.inputs);
    const outputs = groups?.playback.channels ?? positive(live?.outputs) ?? positive(reported?.outputs);
    return {
      name,
      ...(id === undefined ? {} : { deviceId: id }),
      ...(topology === undefined ? {} : { topology }),
      ...(record === undefined ? {} : { record }),
      ...(layout === undefined ? {} : { layout }),
      ...(inputs === undefined ? {} : { inputs }),
      ...(outputs === undefined ? {} : { outputs }),
      ...(live === undefined ? {} : { published: { ...(live.inputs === undefined ? {} : { inputs: live.inputs }), ...(live.outputs === undefined ? {} : { outputs: live.outputs }) } }),
    };
  });
  const names = distinctNames(built.map((one) => one.name));
  return built.map((one, at) => ({ ...one, name: names[at] as string }));
}

/** What one interface is called: Gazelle's name for it, else what little the setup says. */
export function interfaceName(naming: AggregateNaming | undefined, index: number, device?: AggregateDevice): string {
  return naming?.[index]?.name ?? device?.key ?? device?.clsid ?? `Interface ${index + 1}`;
}

/** Every interface's name, in the setup's order. */
export function interfaceNames(config: Aggregate | undefined, naming: AggregateNaming | undefined): string[] {
  return (config?.devices ?? []).map((device, index) => interfaceName(naming, index, device));
}

/** A person's name made fit for a channel's: one line, trimmed. */
const plain = (text: string): string => text.replace(/\p{Cc}/gu, " ").trim();

/**
 * What a routing source is called, in Gazelle's own words, with the person's own names first: the
 * Mixer channel they named that takes this source, else the mix they named when the source is a
 * mix's output, else the source as the Routing page and the Mixer show it ("PREAMP 1", "AFX OUT 3").
 * MUTE is nothing.
 */
export function sourceName(topology: Topology, source: RouteSlot, layout?: DeviceMixer): string | undefined {
  const group = topology.inputs[source.source];
  if (group === undefined || group.type === "MUTE") return undefined;
  const named = layout?.channels.find((one) => one.source?.group === source.source && one.source.channel === source.channel && one.name.trim() !== "");
  if (named !== undefined) return plain(named.name);
  const mix = topology.mixers.outputGroups.indexOf(group.id);
  const mixName = mix < 0 ? undefined : layout?.mixes[mix]?.name?.trim();
  if (mixName !== undefined && mixName !== "") return `${plain(mixName)} ${source.channel === 0 ? "L" : source.channel === 1 ? "R" : source.channel + 1}`;
  return sourceLabel(topology, { group: source.source, channel: source.channel });
}

/** The most a channel label may be, in characters, which is what the interface carries. */
export const CHANNEL_LABEL_MAX = 31;

/** As much of a label as the interface carries, in whole characters. */
export function fitLabel(text: string): string {
  return [...text].slice(0, CHANNEL_LABEL_MAX).join("").trimEnd();
}

/** One channel's name, in its parts. */
export interface ChannelName {
  /** The USB channel it is: "USB A REC 1", "USB 1 PLAY 5". */
  usb: string;
  /** What it carries: the name the person typed, else what routing sends it or the Mixer channel that plays it. */
  carries?: string;
  /** The whole name, as the page shows it everywhere: "Vocal mic, USB A REC 1". */
  text: string;
  /** The label the driver is given when nobody has typed one, or nothing while the model is not known. */
  automatic?: string;
  /** The name the person typed, when they did. */
  typed?: string;
}

/**
 * One channel of one interface, named: first for what it carries, then by its USB channel. An input
 * is named for what the routing sends to its USB record channel, once that routing has been read;
 * an output for the Mixer channel that plays its USB playback channel, when the person named one.
 */
export function channelName(device: AggregateDevice | undefined, naming: InterfaceNaming | undefined, input: boolean, channel: number): ChannelName {
  const topology = naming?.topology;
  const groups = usbGroups(topology);
  const group = input ? groups?.record : groups?.playback;
  const usb = group === undefined ? `${input ? "Input" : "Output"} ${channel + 1}` : group.channels > 1 ? `${group.name} ${channel + 1}` : group.name;
  let routed: string | undefined;
  if (topology !== undefined && groups !== undefined) {
    if (input) {
      const slot = naming?.record?.[channel];
      routed = slot === undefined ? undefined : sourceName(topology, slot, naming?.layout);
    } else {
      const named = naming?.layout?.channels.find((one) => one.source?.group === groups.playbackPosition && one.source.channel === channel && one.name.trim() !== "");
      routed = named === undefined ? undefined : plain(named.name);
    }
  }
  const typedRaw = channelLabel(device?.[input ? "input_names" : "output_names"], channel).trim();
  const typed = typedRaw === "" ? undefined : typedRaw;
  const carries = typed ?? routed;
  return {
    usb,
    ...(carries === undefined ? {} : { carries }),
    text: carries === undefined ? usb : `${carries}, ${usb}`,
    ...(groups === undefined ? {} : { automatic: fitLabel(routed ?? usb) }),
    ...(typed === undefined ? {} : { typed }),
  };
}

const bytes = (text: string): number => new TextEncoder().encode(text).length;

/** As much of a name as fits in `room`: whole characters, and never more bytes than that. */
function cut(text: string, room: number): string {
  const kept = [...text].slice(0, Math.max(0, room));
  while (kept.length > 0 && bytes(kept.join("")) > room) kept.pop();
  return kept.join("").trimEnd();
}

/**
 * The name a DAW shows for a channel, built as the driver builds it: the label, with the driver's
 * own reference "{interface name} {number}" in brackets after it, all within the interface's 31
 * characters. When both will not fit, the label alone; with no label, the reference alone.
 */
export function dawChannelName(interface_: string, channel: number, label: string | undefined): string {
  const tail = ` ${channel + 1}`;
  const reference = `${cut(interface_, CHANNEL_LABEL_MAX - tail.length)}${tail}`;
  const given = label?.trim() ?? "";
  if (given === "") return reference;
  const both = `${given} (${reference})`;
  return [...both].length <= CHANNEL_LABEL_MAX && bytes(both) <= CHANNEL_LABEL_MAX ? both : cut(given, CHANNEL_LABEL_MAX);
}

/** How many channels an interface has each way, or nothing where that is not known yet. */
export interface ChannelCounts {
  inputs: number | undefined;
  outputs: number | undefined;
}

/**
 * How many channels to list for one interface: its USB groups' own counts, which are exact and known
 * without a DAW, else what a running driver published. With neither, the number is not known, and a
 * page offers nothing to edit rather than guessing at one.
 */
export function channelCounts(naming: InterfaceNaming | undefined): ChannelCounts {
  return { inputs: naming?.inputs, outputs: naming?.outputs };
}

/**
 * What to say when a running driver published a count that is not the interface's own, which is the
 * one check the published count is still good for. Nothing when they agree or there is nothing to
 * compare.
 */
export function countCheck(naming: InterfaceNaming | undefined): string | undefined {
  const groups = usbGroups(naming?.topology);
  const published = naming?.published;
  if (groups === undefined || published === undefined) return undefined;
  const ins = positive(published.inputs);
  const outs = positive(published.outputs);
  if ((ins === undefined || ins === groups.record.channels) && (outs === undefined || outs === groups.playback.channels)) return undefined;
  return `The driver reports ${ins ?? "no"} inputs and ${outs ?? "no"} outputs for this interface, and it has ${groups.record.channels} USB record and ${groups.playback.channels} USB playback channels. Gazelle lists the USB channels.`;
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
 * The `input_names` or `output_names` map after naming, or un-naming, one channel. These are only
 * ever the names the person typed: the automatic ones are worked out, never written here.
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

/** How many of a device's channels carry a name the person typed, counting only ones it actually has. */
export function namedCount(names: Record<string, string> | undefined, count: number | undefined): number {
  return Object.entries(names ?? {}).filter(([key, value]) => {
    const channel = Number(key);
    if (!Number.isInteger(channel) || channel < 0 || value.trim() === "") return false;
    return count === undefined || channel < count;
  }).length;
}

/**
 * The one line that stands for a card's Channels part while it is closed: how many channels there
 * are and how many are exposed, and how many have been given a name of their own.
 */
export function channelSummary(device: AggregateDevice, counts: ChannelCounts): string {
  const exposed = (chosen: number[] | undefined, count: number | undefined, first: boolean) => {
    const all = first ? "All" : "all";
    if (chosen === undefined) return count === undefined ? all : `${all} ${count}`;
    return count === undefined ? String(chosen.length) : `${chosen.filter((one) => one < count).length} of ${count}`;
  };
  const named = namedCount(device.input_names, counts.inputs) + namedCount(device.output_names, counts.outputs);
  const shown = `${exposed(device.inputs, counts.inputs, true)} in, ${exposed(device.outputs, counts.outputs, false)} out`;
  return named === 0 ? shown : `${shown}, ${named} named`;
}

/** What Gazelle writes into `callback_master` for a device: its registry key, else its class id, neither of which a rename changes. */
export function masterReference(device: AggregateDevice): string | undefined {
  const reference = (device.key ?? device.clsid)?.trim();
  return reference === undefined || reference === "" ? undefined : reference;
}

/**
 * What the menu of drivers to add calls one: Gazelle's name for the device it would be, where
 * exactly one connected device is of the model the driver's words name, with the driver's own
 * name after it as a detail; else the driver's own name alone.
 */
export function driverOfferText(
  entry: { key: string; description?: string },
  devices: readonly { id: string; model: string | null; family: string | null; backend?: string }[],
  aliases: Readonly<Record<string, string>> | undefined,
): string {
  const own = entry.description ?? entry.key;
  const words = `${entry.key} ${entry.description ?? ""}`.toLowerCase();
  const family = words.includes("quadro") ? "quadro" : words.includes("studio") ? "studio" : undefined;
  const matching = family === undefined ? [] : devices.filter((device) => device.family === family && (device.backend === undefined || device.backend === "usb"));
  const only = matching.length === 1 ? matching[0] : undefined;
  if (only === undefined) return own;
  const name = aliases?.[only.id]?.trim() || only.model || own;
  return name === own ? own : `${name} (${own})`;
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

/** This configured interface's place in the answer: the report for its place in the setup. */
export function viewFor(answer: AggregateAnswer | undefined, index: number): AggregateDeviceView | undefined {
  const views = deviceViews(answer);
  return views.find((view) => view.report?.index === index) ?? views[index];
}

/**
 * One channel a run can be cabled to: the interface it is on and that interface's own number for
 * it, and what it is called.
 *
 * **Never a number in the aggregate's own list.** That numbering depends on how many channels each
 * interface's driver really has, and a count that is two short puts every cable on the next
 * interface along. So a run is asked for by interface and channel, and the run, which opens the
 * drivers, finds where each one really is.
 */
export interface InterfaceChannel {
  /** The interface it is on, by Gazelle's name for it. */
  device: string;
  /** That interface's place in the setup, from zero, which is how a request names it. */
  index: number;
  /** Its number on that interface, from zero. */
  channel: number;
  /** The USB channel it is, always: "USB A REC 1". */
  usb: string;
  /** Its whole name, as the rest of the page names it: "Vocal mic, USB A REC 1". */
  text: string;
}

/**
 * The channels the setup keeps for the phase measurement, by interface: each follower's own input
 * the measurement arrives on, and the callback master's outputs it leaves from. The driver keeps
 * them out of the aggregate, so a run cannot be cabled to them and the pickers do not offer them.
 */
function phaseKept(config: Aggregate | undefined, input: boolean): Set<string> {
  const kept = new Set<string>();
  const devices = config?.devices ?? [];
  const master = masterIndex(config);
  for (const [index, device] of devices.entries()) {
    const phase = phaseSetting(device);
    if (phase === undefined || index === master) continue;
    if (input) kept.add(`${index}:${phase.input}`);
    else if (master !== undefined) kept.add(`${master}:${phase.master_output}`);
  }
  return kept;
}

/**
 * Every input, or every output, a run could be cabled to, interface by interface in the setup's
 * order: what the setup exposes, less what it keeps for the phase measurement. How many channels an
 * interface has is its USB groups' own count, known without a DAW.
 */
export function interfaceChannels(config: Aggregate | undefined, naming: AggregateNaming | undefined, input: boolean): InterfaceChannel[] {
  const listed: InterfaceChannel[] = [];
  const devices = config?.devices ?? [];
  const kept = phaseKept(config, input);
  for (const [index, device] of devices.entries()) {
    const named = naming?.[index];
    const counts = channelCounts(named);
    const count = input ? counts.inputs : counts.outputs;
    if (count === undefined) continue;
    for (let channel = 0; channel < count; channel += 1) {
      if (!isExposed(device[input ? "inputs" : "outputs"], channel) || kept.has(`${index}:${channel}`)) continue;
      const name = channelName(device, named, input, channel);
      listed.push({ device: interfaceName(naming, index, device), index, channel, usb: name.usb, text: name.text });
    }
  }
  return listed;
}

/** The channels of that list that are on one interface. */
export function channelsOf(channels: InterfaceChannel[], device: string): InterfaceChannel[] {
  return channels.filter((one) => one.device === device);
}

/**
 * How one interface's channel reads in a sentence, including one that is no longer in the list: it
 * is still named, from the interface's own naming.
 */
export function channelText(channels: InterfaceChannel[], device: string, channel: number | undefined, config?: Aggregate, naming?: AggregateNaming, input = true): string {
  if (channel === undefined) return "not chosen";
  const listed = channels.find((one) => one.device === device && one.channel === channel);
  if (listed !== undefined) return listed.text;
  const index = interfaceNames(config, naming).indexOf(device);
  if (index < 0) return `${device} ${channel + 1}`;
  return channelName(config?.devices?.[index], naming?.[index], input, channel).text;
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
 * `outputs[n]` is the channel cabled into `inputs[n]`. Each is that interface's own channel number
 * from zero, on the interface its picker offers (`slotDevice`). A channel not chosen yet is
 * `undefined` rather than a guess, and a run with one of those in it is not offered.
 */
export interface CalibratePicks {
  direction: AggregateCalibrateDirection;
  /** The one interface every cable has an end on, by Gazelle's name for it. */
  reference: string;
  outputs: (number | undefined)[];
  inputs: (number | undefined)[];
  clicks: number;
  level_dbfs: number;
}

/** The interfaces a run is over, by Gazelle's names for them, in the setup's order. */
export function calibrateDevices(config: Aggregate | undefined, naming?: AggregateNaming): string[] {
  return interfaceNames(config, naming);
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
  naming: AggregateNaming | undefined,
  direction: AggregateCalibrateDirection = "inputs",
  reference?: string,
): CalibratePicks {
  const devices = calibrateDevices(config, naming);
  const against = reference !== undefined && devices.includes(reference) ? reference : (devices[0] ?? "");
  const lists = { outputs: interfaceChannels(config, naming, false), inputs: interfaceChannels(config, naming, true) };
  const pick = (side: "outputs" | "inputs", at: number) => {
    const offered = channelsOf(lists[side], slotDevice(direction, against, side, at, devices));
    return offered[side === sharedSide(direction) ? at : 0]?.channel;
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
 * exposed can leave a choice pointing at nothing. Every choice that is still a channel its picker
 * offers is kept exactly as it was; everything else falls back to where the defaults would have put
 * it, so a poll landing never moves a menu somebody has just set.
 */
export function reconcilePicks(stored: CalibratePicks | undefined, config: Aggregate | undefined, naming: AggregateNaming | undefined): CalibratePicks {
  const base = defaultPicks(config, naming, stored?.direction, stored?.reference);
  if (stored === undefined) return base;
  const devices = calibrateDevices(config, naming);
  const lists = { outputs: interfaceChannels(config, naming, false), inputs: interfaceChannels(config, naming, true) };
  const keep = (side: "outputs" | "inputs", at: number): number | undefined => {
    const chosen = stored[side][at];
    const wanted = slotDevice(base.direction, base.reference, side, at, devices);
    return channelsOf(lists[side], wanted).some((one) => one.channel === chosen) ? chosen : base[side][at];
  };
  return {
    ...base,
    outputs: base.outputs.map((_, at) => keep("outputs", at)),
    inputs: base.inputs.map((_, at) => keep("inputs", at)),
    clicks: CLICKS.includes(stored.clicks) ? stored.clicks : base.clicks,
    level_dbfs: LEVELS_DBFS.includes(stored.level_dbfs) ? stored.level_dbfs : base.level_dbfs,
  };
}

/**
 * The picks for another pass or another reference: the cabling starts again from the defaults,
 * because a channel number chosen on one interface means nothing on the next, and only how many
 * clicks and how loud are carried over.
 */
export function withPass(
  picks: CalibratePicks,
  config: Aggregate | undefined,
  naming: AggregateNaming | undefined,
  direction: AggregateCalibrateDirection,
  reference: string,
): CalibratePicks {
  return { ...defaultPicks(config, naming, direction, reference), clicks: picks.clicks, level_dbfs: picks.level_dbfs };
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
export function cablingSteps(picks: CalibratePicks, config: Aggregate | undefined, naming: AggregateNaming | undefined): CalibrateCable[] {
  const outputs = interfaceChannels(config, naming, false);
  const inputs = interfaceChannels(config, naming, true);
  const devices = calibrateDevices(config, naming);
  return devices.map((_, at) => {
    const leaves = slotDevice(picks.direction, picks.reference, "outputs", at, devices);
    const arrives = slotDevice(picks.direction, picks.reference, "inputs", at, devices);
    const from = `${channelText(outputs, leaves, picks.outputs[at], config, naming, false)} on ${leaves}`;
    const to = `${channelText(inputs, arrives, picks.inputs[at], config, naming, true)} on ${arrives}`;
    return { from, to, text: `${from} into ${to}` };
  });
}

/**
 * Why this run cannot be made, in words, or nothing when it can. It is the one place that decides
 * whether the button does anything, so the element asks and does not work it out again.
 *
 * Which interface each pick is on is never in question here, because each picker only offers the
 * channels of one interface and the request names that interface. What is checked is what the
 * cabling means: every pick made, a channel its interface is offering, and no one channel of the
 * shared interface carrying two cables.
 */
export function calibrateProblem(picks: CalibratePicks, config: Aggregate | undefined, naming: AggregateNaming | undefined): string | undefined {
  const devices = calibrateDevices(config, naming);
  if (devices.length < 2) return "The aggregate needs at least two interfaces before there is anything to line up.";
  if (picks.outputs.length !== devices.length || picks.inputs.length !== devices.length) return "Every interface needs one output and one input chosen.";
  if (picks.outputs.includes(undefined) || picks.inputs.includes(undefined)) return "Every interface needs one output and one input chosen.";
  if (!devices.includes(picks.reference)) return "Choose the interface every cable has an end on.";
  const lists = { outputs: interfaceChannels(config, naming, false), inputs: interfaceChannels(config, naming, true) };
  for (const side of ["outputs", "inputs"] as const) {
    const offered = (at: number) => channelsOf(lists[side], slotDevice(picks.direction, picks.reference, side, at, devices)).some((one) => one.channel === picks[side][at]);
    if (!devices.every((_, at) => offered(at))) return "Every interface needs one output and one input chosen.";
  }
  const shared = picks[sharedSide(picks.direction)];
  if (new Set(shared).size !== shared.length) {
    return picks.direction === "inputs"
      ? "Two interfaces cannot take their click from one output. Choose a different output for each."
      : "Two interfaces cannot record on one input. Choose a different input for each.";
  }
  if (!CLICKS.includes(picks.clicks)) return "Choose how many clicks to play.";
  return undefined;
}

/**
 * The request that starts this run, or nothing when the picks do not make one.
 *
 * Every channel goes as `{ device, channel }`: the interface by its place in the setup and that
 * interface's own channel number, both from zero. The run works out where that is in the
 * aggregate once it has the drivers open, which is the only place it can be known for sure.
 *
 * `check` asks for a check rather than a measurement: the same cabling and the same clicks, with
 * the session lined up as a DAW's would be, so what it hears is how far apart a recording would
 * land now. A measurement leaves the field out, which is what the server takes as a measurement.
 */
export function calibrateRequest(
  picks: CalibratePicks,
  config: Aggregate | undefined,
  naming: AggregateNaming | undefined,
  options: { check?: boolean } = {},
): AggregateCalibrateRequest | undefined {
  if (calibrateProblem(picks, config, naming) !== undefined) return undefined;
  const devices = calibrateDevices(config, naming);
  // The shared side is all on the reference, and the other side is each row's own interface.
  const reference = devices.indexOf(picks.reference);
  const ends = (side: "outputs" | "inputs"): AggregateCalibrateChannel[] =>
    devices.map((_, at) => ({ device: side === sharedSide(picks.direction) ? reference : at, channel: picks[side][at] as number }));
  return {
    direction: picks.direction,
    outputs: ends("outputs"),
    inputs: ends("inputs"),
    clicks: picks.clicks,
    level_dbfs: picks.level_dbfs,
    ...(options.check === true ? { check: true } : {}),
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

/**
 * A number of samples to two decimals, for a check's verdict: a check that lands 0.02 samples out
 * has to say 0.02 rather than round it to nothing, or a good result and a perfect one read alike.
 */
function fineSamples(value: number): string {
  const rounded = Number(value.toFixed(2));
  return `${rounded} sample${Math.abs(rounded) === 1 ? "" : "s"}`;
}

/** How many blocks, in words. */
function blocks(count: number): string {
  return `${count} block${count === 1 ? "" : "s"}`;
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
  /** What this interface's audio lost while the run was going, when it lost anything. */
  lost?: string;
  tone: "good" | "off" | "drift";
}

/** What an interface lost during a run, in words, or nothing when it lost nothing. */
function lostText(dropped: number | undefined, starved: number | undefined): string | undefined {
  const thrown = dropped ?? 0;
  const missed = starved ?? 0;
  if (thrown === 0 && missed === 0) return undefined;
  const parts = [thrown > 0 ? `dropped ${blocks(thrown)}` : "", missed > 0 ? `missed ${blocks(missed)}` : ""].filter((part) => part !== "");
  return `Its audio ${parts.join(" and ")} while this ran, so the clicks were measured across a fault.`;
}

/** How steady the clicks were, against the widest they could disagree and still be one measurement. */
function spreadText(reading: { spread_samples: number; spread_limit_samples?: number; clicks_found: number }): string {
  if (reading.clicks_found === 0) return "Nothing to measure";
  const limit = reading.spread_limit_samples;
  if (limit === undefined) return `Clicks agreed to ${samples(reading.spread_samples)}`;
  const allowed = Number(limit.toFixed(1));
  return reading.spread_samples > limit
    ? `Clicks disagreed by ${samples(reading.spread_samples)}, past the ${allowed} allowed`
    : `Clicks agreed to ${samples(reading.spread_samples)}, within the ${allowed} allowed`;
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
  const lost = lostText(reading.blocks_dropped, reading.blocks_starved);
  const spreadPast = reading.spread_limit_samples !== undefined && reading.clicks_found > 0 && reading.spread_samples > reading.spread_limit_samples;
  return {
    device: reading.device,
    lag,
    spread: spreadText(reading),
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
    ...(lost === undefined ? {} : { lost }),
    tone: real ? "drift" : (reading.is_reference || reading.lag_samples === 0) && lost === undefined && !spreadPast ? "good" : "off",
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
  /** Whether applying it would change anything at all, the phase reference beside it included. */
  changed: boolean;
  /** Why this one is not offered, when the server says it is not. */
  notApplied?: string;
  /** What happens to the phase reference written beside this trim, when it has one. Never a trim. */
  reference?: string;
}

export function trimRows(outcome: AggregateCalibrateOutcome | undefined): TrimRow[] {
  return (outcome?.trims ?? []).map((trim) => {
    const reference = referenceText(trim.phase_reference);
    return {
      device: trim.device,
      what: trimField(trim) === "input_trim" ? "Input trim" : "Output trim",
      was: `${trim.was}`,
      measured: `${trim.measured}`,
      now: `${trim.now}`,
      changed: trimWouldChange(trim),
      ...(trim.not_applied === undefined ? {} : { notApplied: trim.not_applied }),
      ...(reference === undefined ? {} : { reference }),
    };
  });
}

/** The setup field a trim writes: the one the server names, and failing that the pass's own. */
export function trimField(trim: AggregateCalibrateTrim): "input_trim" | "output_trim" {
  if (trim.field === "input_trim" || trim.field === "output_trim") return trim.field;
  return trim.direction === "inputs" ? "input_trim" : "output_trim";
}

/**
 * Whether writing this trim would change the phase reference beside it.
 *
 * A first run on an interface is the case this is for: it had no reference, it has one now, and its
 * trim may well have come out exactly what it was. The trim alone would say there is nothing to
 * write, and every session after it would go on unlined up.
 */
export function referenceChanges(trim: AggregateCalibrateTrim): boolean {
  const reference = trim.phase_reference;
  if (reference === undefined) return false;
  return (reference.was ?? null) !== (reference.now ?? null);
}

/** Whether writing this one would change anything: the trim, or the reference written with it. */
function trimWouldChange(trim: AggregateCalibrateTrim): boolean {
  return trim.not_applied === undefined && (trim.now !== trim.was || referenceChanges(trim));
}

/**
 * What happens to the phase reference beside a trim, in words, or nothing when it has none.
 *
 * `now` null is nothing heard on the cable in this run, and then the old reference is taken out
 * rather than left beside a trim it was never measured with.
 */
export function referenceText(reference: AggregateTrimReference | undefined): string | undefined {
  if (reference === undefined) return undefined;
  const was = reference.was ?? null;
  const now = reference.now ?? null;
  if (was === now) return now === null ? "Phase reference: none, and nothing was heard on the cable to give it one" : `Phase reference: stays at ${samples(now)}`;
  if (now === null) return `Phase reference: ${samples(was as number)} is taken out, because nothing was heard on the cable in this run`;
  if (was === null) return `Phase reference: none yet, becomes ${samples(now)}`;
  return `Phase reference: ${samples(was)} becomes ${samples(now)}`;
}

/** What a finished run came to, as one line above the readings. */
export function outcomeSummary(outcome: AggregateCalibrateOutcome): string {
  const rate = `${Number((outcome.rate / 1000).toFixed(3))} kHz`;
  const pass = outcome.direction === "inputs" ? "what the interfaces record" : "what the interfaces play";
  if (outcome.checking === true) {
    return `Checked ${pass} at ${rate}, ${outcome.buffer_size} samples, against ${outcome.reference}, lined up as a DAW's session would be. A check writes nothing: it says how far apart a recording would land now.`;
  }
  return `Measured ${pass} at ${rate}, ${outcome.buffer_size} samples, against ${outcome.reference}. A trim is only true for this rate and this buffer size.`;
}

/**
 * The trims there are to write, which is what the button is for: every one the server offers that
 * would change the trim or the phase reference beside it. A check offers none, whatever it carries.
 */
export function trimsToApply(outcome: AggregateCalibrateOutcome | undefined): AggregateCalibrateTrim[] {
  if (outcome?.checking === true) return [];
  return (outcome?.trims ?? []).filter(trimWouldChange);
}

/**
 * The setup with the measured trims written into it, each to the interface the run named. A run
 * names each interface as the driver's file does, which is Gazelle's name for it, so `names` is the
 * page's names for the setup's interfaces, in order.
 *
 * Zero is written as the field being absent, as everything else on this page writes a trim; an
 * interface the measurement names that the setup no longer has is passed over rather than added,
 * and so is a trim the server says it is not offering.
 *
 * **An input trim's phase reference is written with it**, into that interface's `phase.reference`,
 * and taken out when nothing was heard on the cable, so a new trim is never left beside a reference
 * from another session. An interface whose phase setting has gone since the run keeps no reference:
 * a reference with no path to measure on means nothing.
 */
export function withMeasuredTrims(config: Aggregate, outcome: AggregateCalibrateOutcome | undefined, names?: readonly string[]): Aggregate {
  const trims = trimsToApply(outcome);
  if (trims.length === 0) return config;
  const called = names ?? interfaceNames(config, undefined);
  const devices = (config.devices ?? []).map((device, index) => {
    const trim = trims.find((one) => one.device === called[index]);
    if (trim === undefined) return device;
    const field = trimField(trim);
    const next = { ...device };
    if (trim.now === 0) delete next[field];
    else next[field] = trim.now;
    const reference = trim.phase_reference;
    const setting = phaseSetting(device);
    if (reference !== undefined && field === "input_trim" && setting !== undefined) {
      const phase: AggregatePhaseSetting = { ...setting };
      if (reference.now === null || reference.now === undefined) delete phase.reference;
      else phase.reference = reference.now;
      next.phase = phase;
    }
    return next;
  });
  return { ...config, devices };
}

/** What applying the trims came to, as one line. */
export function appliedTrimsText(trims: AggregateCalibrateTrim[]): string {
  if (trims.length === 0) return "Nothing to change: every trim is already what was measured.";
  const named = trims
    .map((trim) => {
      const written = `${trim.device} ${trim.direction === "inputs" ? "in" : "out"} ${trim.now}`;
      if (!referenceChanges(trim)) return written;
      const now = trim.phase_reference?.now ?? null;
      return now === null ? `${written} (phase reference taken out)` : `${written} (phase reference ${now})`;
    })
    .join(", ");
  return `${trims.length} trim${trims.length === 1 ? "" : "s"} written: ${named}.`;
}

/** Whether a run lost any audio while it went, in words, or nothing from a server too old to say. */
export function runCleanText(outcome: AggregateCalibrateOutcome | undefined): { text: string; problem: boolean } | undefined {
  if (outcome === undefined || outcome.clean === undefined) return undefined;
  if (outcome.clean) return { text: "Clean: no interface lost a block while it ran.", problem: false };
  const lost = outcome.blocks_lost ?? 0;
  const what = lost > 0 ? `${blocks(lost)} lost while it ran` : "Blocks were lost while it ran";
  return { text: `Not clean: ${what}. A lost block moves the very thing being measured, so run it again rather than believing these figures.`, problem: true };
}

/** How close a check has to land for the interfaces to count as lined up, in samples. */
export const CHECK_TOLERANCE_SAMPLES = 1;

/** One interface's verdict from a check: how far apart a recording would land now. */
export interface VerdictView {
  device: string;
  text: string;
  tone: "good" | "off" | "drift";
  note?: string;
}

/**
 * What a check says about each interface but the reference: a verdict, not an offer. A check was
 * lined up exactly as a DAW's session is, so the lag it hears is what a recording would get.
 */
export function checkVerdicts(outcome: AggregateCalibrateOutcome | undefined): VerdictView[] {
  if (outcome?.checking !== true) return [];
  return outcome.readings
    .filter((reading) => !reading.is_reference)
    .map((reading) => {
      const note = reading.note === undefined || reading.note.trim() === "" ? {} : { note: reading.note };
      if (reading.clicks_found === 0) return { device: reading.device, text: "Nothing was heard, so there is no verdict. Check the cable and its routing.", tone: "off" as const, ...note };
      if (reading.drift?.real === true) return { device: reading.device, text: "Drifting: the interfaces are not sharing one clock, and nothing lines that up.", tone: "drift" as const, ...note };
      const off = Math.abs(reading.lag_samples);
      const where = reading.lag_samples === 0 ? `in step with ${outcome.reference}` : `${fineSamples(off)} ${reading.lag_samples > 0 ? "late" : "early"} against ${outcome.reference}`;
      return off < CHECK_TOLERANCE_SAMPLES
        ? { device: reading.device, text: `Lined up: a recording would land ${where}.`, tone: "good" as const, ...note }
        : { device: reading.device, text: `Out: a recording would land ${where}. Measure again, then check.`, tone: "off" as const, ...note };
    });
}

/** One extra channel a run listened in on, in the words a reading is written in, with the channel it was. */
export interface WitnessView extends ReadingView {
  channel: string;
}

/** The channels a run listened in on, each named as every other channel on the page is. */
export function witnessViews(outcome: AggregateCalibrateOutcome | undefined, inputs: InterfaceChannel[], config?: Aggregate, naming?: AggregateNaming): WitnessView[] {
  return (outcome?.witnesses ?? []).map((witness) => ({
    ...readingView({ ...witness, is_reference: false }),
    channel: channelText(inputs, witness.device, witness.channel, config, naming, true),
  }));
}

// ---------------------------------------------------------------------------------------------
// The phase: where each interface's capture started this session
// ---------------------------------------------------------------------------------------------
//
// Two interfaces record a fixed distance apart within a session, and a different distance each time
// the driver is opened, by whole steps of 32 samples. A trim is a constant and cannot follow that.
// So the driver measures each follower's phase down the digital cable at the start of every
// session, and lines the session up by the reference minus that phase before the trim applies.
//
// Three numbers, which this page keeps apart in every word it writes:
//
//   - the trim, a constant, measured once with a click;
//   - the reference, the phase measured in the session the trim was measured in, written with it;
//   - the phase, what each session measures, live and read only.

const isChannel = (value: unknown): value is number => typeof value === "number" && Number.isInteger(value) && value >= 0;

/** The interface's phase setting, when it has a whole one: both channels, and a reference if any. */
export function phaseSetting(device: AggregateDevice | undefined): AggregatePhaseSetting | undefined {
  const phase = device?.phase;
  if (typeof phase !== "object" || phase === null) return undefined;
  if (!isChannel(phase.master_output) || !isChannel(phase.input)) return undefined;
  return phase;
}

/** The phase reference, when the setting has one. */
export function phaseReference(device: AggregateDevice | undefined): number | undefined {
  const reference = phaseSetting(device)?.reference;
  return typeof reference === "number" && Number.isFinite(reference) ? reference : undefined;
}

/**
 * Which interface in the setup drives the callback, by its place in the list.
 *
 * The setup names it by registry key or class id, which a rename cannot change, or, in a setup an
 * older Gazelle wrote, by the name it gave the device; the first interface when it names none, which
 * is what the driver does. One that matches nothing falls back to whichever one the answer says is
 * the master, and then to the first.
 */
export function masterIndex(config: Aggregate | undefined, answer?: AggregateAnswer): number | undefined {
  const devices = config?.devices ?? [];
  if (devices.length === 0) return undefined;
  const named = config?.callback_master;
  if (typeof named === "string" && named.trim() !== "") {
    const wanted = named.trim().toLowerCase();
    const found = devices.findIndex((device) => [device.key, device.clsid, device.name].some((one) => typeof one === "string" && one.trim().toLowerCase() === wanted));
    if (found >= 0) return found;
    const reported = answer?.devices.findIndex((report) => report.is_master) ?? -1;
    const at = reported < 0 ? -1 : (answer?.devices[reported]?.index ?? reported);
    if (at >= 0 && at < devices.length) return at;
  }
  return 0;
}

/** One channel a phase picker offers: the device's own number from zero, and what to call it. */
export interface PhaseChoice {
  value: number;
  text: string;
}

/** What the two phase pickers on one follower's card offer, or nothing on the callback master's. */
export interface PhaseChoices {
  /** The callback master, by Gazelle's name for it. */
  master: string;
  /** This interface, by Gazelle's name for it. */
  own: string;
  /** The master's own outputs, or undefined while how many it has is not known. */
  outputs: PhaseChoice[] | undefined;
  /** This interface's own inputs, or undefined while how many it has is not known. */
  inputs: PhaseChoice[] | undefined;
}

/**
 * What the two phase pickers offer: the callback master's own outputs and this interface's own
 * inputs, each named as every channel on the page is. These are the devices' own channels, not the
 * aggregate's list, and every one is offered, exposed or not: the driver opens these two itself.
 */
export function phaseChoices(config: Aggregate | undefined, answer: AggregateAnswer | undefined, naming: AggregateNaming | undefined, index: number): PhaseChoices | undefined {
  const devices = config?.devices ?? [];
  const device = devices[index];
  const at = masterIndex(config, answer);
  if (device === undefined || at === undefined || at === index) return undefined;
  const master = devices[at] as AggregateDevice;
  const list = (of: AggregateDevice, place: number, input: boolean): PhaseChoice[] | undefined => {
    const counts = channelCounts(naming?.[place]);
    const count = input ? counts.inputs : counts.outputs;
    if (count === undefined) return undefined;
    return Array.from({ length: count }, (_, channel) => ({ value: channel, text: channelName(of, naming?.[place], input, channel).text }));
  };
  return { master: interfaceName(naming, at, master), own: interfaceName(naming, index, device), outputs: list(master, at, false), inputs: list(device, index, true) };
}

/**
 * The choices a picker shows, with the one chosen kept even when the list does not have it, so a
 * setting made while more channels were known is shown as it is rather than as nothing. `named`
 * says what that channel is called.
 */
export function choicesWith(choices: PhaseChoice[] | undefined, chosen: number | undefined, named: (channel: number) => string): PhaseChoice[] {
  const listed = choices ?? [];
  if (chosen === undefined || listed.some((choice) => choice.value === chosen)) return listed;
  return [...listed, { value: chosen, text: `${named(chosen)} (not listed now)` }];
}

/** What the two pickers hold: a channel each, or undefined for one not chosen yet. */
export interface PhasePicks {
  master_output?: number;
  input?: number;
}

/** What the pickers show: what this tab has chosen and not yet written, and otherwise the setting. */
export function phasePicks(setting: AggregatePhaseSetting | undefined, draft: PhasePicks | undefined): PhasePicks {
  const master_output = draft?.master_output ?? setting?.master_output;
  const input = draft?.input ?? setting?.input;
  return { ...(master_output === undefined ? {} : { master_output }), ...(input === undefined ? {} : { input }) };
}

/**
 * The setting two picks make, or nothing while one of them is not chosen: the driver refuses half
 * a path, so nothing is written until both are there.
 *
 * The reference is kept only while the path is the same one it was measured on. A different pair of
 * channels is a different path, and a reference from the old one would line every session up to a
 * state the new one was never in; one measurement gives the new path its own.
 */
export function phaseFromPicks(current: AggregatePhaseSetting | undefined, picks: PhasePicks): AggregatePhaseSetting | undefined {
  if (!isChannel(picks.master_output) || !isChannel(picks.input)) return undefined;
  if (current !== undefined && current.master_output === picks.master_output && current.input === picks.input) return current;
  const next: AggregatePhaseSetting = { ...(current ?? {}), master_output: picks.master_output, input: picks.input };
  delete next.reference;
  return next;
}

/** The interface with its phase setting written, or taken out altogether for undefined. */
export function withPhase(device: AggregateDevice, setting: AggregatePhaseSetting | undefined): AggregateDevice {
  const next = { ...device };
  if (setting === undefined) delete next.phase;
  else next.phase = setting;
  return next;
}

/** What a card's phase setup says about itself: one short line while closed, and a sentence open. */
export interface PhaseSetupView {
  summary: string;
  note: string;
  tone: "idle" | "warn" | "good";
}

export function phaseSetupView(device: AggregateDevice, isMaster: boolean, master: string): PhaseSetupView {
  const setting = phaseSetting(device);
  if (isMaster) {
    return setting === undefined
      ? { summary: "Not measured: the others are measured against it", note: "This interface drives the callback, so every other interface's phase is measured against it and it has none of its own.", tone: "idle" }
      : { summary: "Refused on the callback master", note: "This interface drives the callback, and the driver refuses a phase setup on it: the others are measured against it. Clear it.", tone: "warn" };
  }
  if (setting === undefined) {
    return {
      summary: "Not set up",
      note: `Not set up, so every session lines this interface up by the figures its driver reports, and it lands a different distance from ${master} each time. Choose the channel the cable leaves ${master} on and the one it arrives on here.`,
      tone: "warn",
    };
  }
  const reference = phaseReference(device);
  if (reference === undefined) {
    return {
      summary: "Set up, no reference yet",
      note: "Set up, with no reference yet, so each session is measured and left where it lands. One measurement under Line the interfaces up gives it one, written together with its input trim, and every session after that is lined up.",
      tone: "warn",
    };
  }
  return {
    summary: `Set up, reference ${samples(reference)}`,
    note: `Every session measures where this interface's capture started and lines it up to ${samples(reference)}, the phase measured in the session its input trim was measured in. The reference changes only when the interfaces are measured again.`,
    tone: "good",
  };
}

/** The kind of digital socket a declared cable runs between two devices, when one is declared. */
export function cablePort(cables: Cable[] | undefined, from: string | undefined, to: string | undefined): "S/PDIF" | "ADAT" | undefined {
  if (from === undefined || to === undefined) return undefined;
  const cable = (cables ?? []).find((one) => one.from.device_id === from && one.to.device_id === to);
  if (cable === undefined) return undefined;
  return cable.from.port.startsWith("ADAT") ? "ADAT" : "S/PDIF";
}

/**
 * What has to be routed for the phase measurement to be heard, naming the two ends. On a fresh
 * setup the path is not there, and that reads as nothing heard rather than as a missing route.
 */
export function phaseRoutingNote(master: string, own: string, port: "S/PDIF" | "ADAT" | undefined): string {
  const socket = port ?? "S/PDIF";
  return `This is only heard if the interfaces' own routing carries it: on ${master}, route the playback channel chosen under Leaves the callback master on to its ${socket} output, where the cable leaves; on ${own}, route its ${socket} input, where the cable arrives, to the record channel chosen under Arrives on. A fresh setup has neither, and that reads as nothing heard. Both are on the Routing page.`;
}

/** What one session's phase reads as, live. `tone` is what it is coloured by. */
export interface PhaseLiveView {
  text: string;
  /** What was measured and what was applied, in samples, where there is a measurement to say. */
  figures?: string;
  tone: "good" | "off" | "warn" | "idle";
  /** A measurement the driver would not use, so the session ran on the drivers' own figures. */
  refused: boolean;
}

const PHASE_WORDS: Record<string, { text: string; tone: PhaseLiveView["tone"]; refused?: true; figures?: true }> = {
  not_configured: { text: "Not measured: no phase setup", tone: "idle" },
  measuring: { text: "Measuring", tone: "idle" },
  applied: { text: "Lined up to its reference", tone: "good", figures: true },
  no_reference: { text: "Measured, not lined up: no reference yet", tone: "warn", figures: true },
  measured_only: { text: "Measured and not applied, as a measurement run does", tone: "idle", figures: true },
  not_heard: { text: "Refused: nothing heard on the cable", tone: "off", refused: true },
  off_the_grid: { text: "Refused: not a whole number of 32 sample steps from its reference", tone: "off", refused: true, figures: true },
  too_far: { text: "Refused: further than the driver can move it", tone: "off", refused: true, figures: true },
};

function phaseWords(state: string, measured: number | undefined, applied: number | undefined): PhaseLiveView {
  const known = PHASE_WORDS[state];
  if (known === undefined) return { text: state, tone: "idle", refused: false };
  const figures = known.figures === true && typeof measured === "number" ? `Measured ${samples(measured)}, applied ${samples(applied ?? 0)}` : undefined;
  return { text: known.text, tone: known.tone, refused: known.refused === true, ...(figures === undefined ? {} : { figures }) };
}

/**
 * The live phase beside a follower's gap, while a DAW has the driver open. Nothing for the callback
 * master, which the others are measured against, and nothing from a driver too old to say.
 */
export function livePhaseView(device: AggregateDeviceStatus | undefined): PhaseLiveView | undefined {
  if (device === undefined || device.is_master || typeof device.phase !== "string" || device.phase === "") return undefined;
  return phaseWords(device.phase, device.phase_measured, device.phase_applied);
}

/**
 * The Phase now line on a card: the live phase for a follower, what the master is for, and why
 * there is nothing to read while no DAW has the driver open.
 */
export function cardPhaseView(live: AggregateDeviceStatus | undefined, isMaster: boolean): { text: string; tone: PhaseLiveView["tone"] } {
  if (isMaster) return { text: "The others are measured against it", tone: "idle" };
  if (live === undefined) return { text: "No DAW has it open", tone: "idle" };
  const view = livePhaseView(live);
  if (view === undefined) return { text: "Not reported", tone: "idle" };
  return { text: view.figures === undefined ? view.text : `${view.text}. ${view.figures}`, tone: view.tone };
}

/** One interface's phase as a run measured it. */
export interface RunPhaseView extends PhaseLiveView {
  device: string;
  state: string;
  note?: string;
}

/**
 * The phases a run carries. In a measurement each is measured and nothing is applied, on purpose:
 * it is the reference written with the trim. In a check they are lined up as a DAW's session is.
 */
export function runPhaseViews(outcome: AggregateCalibrateOutcome | undefined): RunPhaseView[] {
  return (outcome?.phases ?? []).map((phase) => {
    const words = phaseWords(phase.state, phase.measured_samples, phase.applied_samples);
    const text = phase.state === "measured_only" && outcome?.checking !== true ? "Measured and not applied, on purpose: this is the reference written with the trim" : words.text;
    const note = phase.note === undefined || phase.note.trim() === "" ? {} : { note: phase.note };
    return { ...words, text, device: phase.device, state: phase.state, ...note };
  });
}

/** Every interface whose phase the run could not use, which is the line said before the trims. */
export function phaseRefusedText(outcome: AggregateCalibrateOutcome | undefined): string | undefined {
  const refused = runPhaseViews(outcome).filter((phase) => phase.refused);
  if (refused.length === 0) return undefined;
  const named = refused.map((phase) => phase.device).join(" and ");
  return outcome?.checking === true
    ? `The phase was not measured on ${named}, so this check ran on the drivers' own figures and says nothing about the trims. Check the cable and its routing, then check again.`
    : `The phase was not measured on ${named}, so writing the trims takes that interface's old reference out rather than leaving it beside a new trim. Check the cable and its routing, then measure again.`;
}

// ---------------------------------------------------------------------------------------------
// The log, and the reason that points at the phase setup
// ---------------------------------------------------------------------------------------------

/** How a line one of Gazelle's own measurements wrote begins. */
export const GAZELLE_MEASUREMENT = "Gazelle's own measurement:";

const EVENT_WORDS: Record<string, string> = {
  refused: "Refused",
  stalled: "Stalled",
  recovered: "Recovered",
  glitched: "Lost a block",
  phase: "Phase measured",
  "session-started": "Session started",
  "session-ended": "Session ended",
  adopted: "Setup taken up",
  "reset-asked": "Reset asked for",
};

/** One line of the driver's log, in words. */
export interface EventView {
  at: string;
  kind: string;
  message: string;
  /** Written by one of Gazelle's own measurements rather than by a DAW's session. */
  gazelle: boolean;
  /** Something that went wrong, which is coloured as such. */
  problem: boolean;
}

export function eventView(event: AggregateEvent): EventView {
  const gazelle = event.message.startsWith(GAZELLE_MEASUREMENT);
  const message = gazelle ? event.message.slice(GAZELLE_MEASUREMENT.length).trim() : event.message;
  const refusal = event.kind === "phase" && /\bnot lined up\b/.test(message);
  return {
    at: event.at,
    kind: EVENT_WORDS[event.kind] ?? event.kind,
    message,
    gazelle,
    problem: event.kind === "refused" || event.kind === "stalled" || event.kind === "glitched" || refusal,
  };
}

/** The sentence a reason carries beside the server's own message, when the page has one to add. */
export function reasonHint(reason: AggregateReason): string | undefined {
  if (reason.code !== "phase_not_measured") return undefined;
  const which = reason.device === undefined ? "its card" : `${reason.device}'s card`;
  return `The phase setup is under Phase on ${which}. A trim does not answer this: the trim is a constant, and this moves every session.`;
}

/**
 * The card a reason is about, by its place in the setup, for a button that goes to it: the place the
 * server gives, else the card of that name.
 */
export function reasonCard(reason: AggregateReason, config: Aggregate | undefined, naming?: AggregateNaming): number | undefined {
  if (reason.code !== "phase_not_measured") return undefined;
  const count = (config?.devices ?? []).length;
  if (typeof reason.device_index === "number") return reason.device_index >= 0 && reason.device_index < count ? reason.device_index : undefined;
  if (reason.device === undefined) return undefined;
  const found = interfaceNames(config, naming).indexOf(reason.device);
  return found >= 0 ? found : undefined;
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
