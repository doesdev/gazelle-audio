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

/** What the model needs of the client, so it can be decided without one. */
export interface AggregateContext {
  read(): Promise<AggregateAnswer>;
  matchBuffers(bufferSize: number, options?: { force?: boolean }): Promise<AggregateMatchBuffers>;
  register(): Promise<AggregateRegistrationRun>;
  unregister(): Promise<AggregateRegistrationRun>;
  /** One command to one device, as the Devices page sends one. True when it went. */
  command(deviceId: string, command: string, args: Record<string, unknown>): Promise<boolean>;
  timers: Timers;
}

/** What the page is doing, so a button can say so and not be pressed twice. */
export type AggregateBusy = "reading" | "fixing" | "matching" | "registering" | "unregistering" | undefined;

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
  #watchers = 0;
  #timer: unknown;

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

  /**
   * Keeps the answer current while the page is on screen. Returns the stop, which the page calls
   * when it goes: nothing is asked for while no page is watching, which is the whole of the rule
   * about not polling a page that is not shown.
   */
  activate(): () => void {
    this.#watchers += 1;
    if (this.#watchers === 1) this.#follow();
    let stopped = false;
    return () => {
      if (stopped) return;
      stopped = true;
      this.#watchers -= 1;
      if (this.#watchers === 0) {
        this.#context.timers.clearTimeout(this.#timer);
        this.#timer = undefined;
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
