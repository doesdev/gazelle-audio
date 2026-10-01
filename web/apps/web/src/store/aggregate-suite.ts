// The alignment suite, as the Aggregate page shows it: every setup a person uses, measured in turn.
//
// A trim is only right at the rate and buffer size it was measured at, so the setup keeps one per
// setup (`trims` on each interface) and the driver uses the one for the session it is in. Lining up
// one setup at a time with Measure works; this does all of them. The person ticks the rates and
// buffer sizes they use in a grid and says how many runs each gets; the server then, for each in
// turn, puts the aggregate at that rate and every interface's driver on that buffer size, measures
// it that many times, keeps the trim when the runs agree and measures again when they do not, and
// at the end puts back the rate and buffer size that were in force. It runs on the server, so this
// page can close and come back to it.
//
// The changes it makes are listed and confirmed once, up front, for the whole run. Nothing here
// decides what a run means: the server reads the runs, compares them and keeps the trims. What is
// here is the grid, the request, the list to confirm, how each setup's progress reads, and a small
// model that follows the suite while the page is open. It comes with the page, so none of it is in
// what the app loads at startup.

import { signal, type ReadonlySignal } from "../core/signal.ts";
import { khz, type AggregateAnswer, type AggregateCalibrateRequest, type AggregateDevice, type AggregateSetup, type AggregateSetupTrim, type AggregateSuite, type AggregateSuiteRequest, type AggregateSuiteSetup, type SuiteCalls } from "./aggregate.ts";
import type { Timers } from "gazelle-audio-client";

/** The rates the grid offers: every one the interfaces take. */
export const SUITE_RATES = [32000, 44100, 48000, 88200, 96000, 176400, 192000];

/** The buffer sizes it offers when no driver has said which it takes. */
export const SUITE_BUFFERS = [16, 32, 64, 128, 256, 512, 1024, 2048];

/** How many runs each setup can be measured with, and what it starts at. */
export const SUITE_RUNS = [2, 3, 4, 5, 6, 8, 10];
export const SUITE_RUNS_DEFAULT = 3;

/** How often the suite is asked about while it runs. */
export const SUITE_POLL_MS = 500;

/** How many times over the server measures one setup before it gives up on it. */
export const SUITE_ROUNDS = 3;

/** What the person has ticked, kept in this tab's view state. */
export interface SuitePicks {
  /** Each ticked setup as `rate/buffer`. */
  setups: string[];
  runs: number;
}

export const NO_PICKS: SuitePicks = { setups: [], runs: SUITE_RUNS_DEFAULT };

/** A setup as the grid keys it. */
export function setupKey(setup: AggregateSetup): string {
  return `${setup.rate}/${setup.buffer_size}`;
}

/** "96 kHz at 512 samples", as every sentence about a setup says it. */
export function setupWords(setup: AggregateSetup): string {
  return `${khz(setup.rate)} at ${setup.buffer_size} samples`;
}

/** The ticked setups, in the order the grid lists them: by rate, then by buffer size. */
export function pickedSetups(picks: SuitePicks): AggregateSetup[] {
  return picks.setups
    .map((key) => key.split("/").map(Number))
    .filter((pair): pair is [number, number] => pair.length === 2 && pair.every((value) => Number.isInteger(value) && value > 0))
    .map(([rate, buffer_size]) => ({ rate, buffer_size }))
    .sort((a, b) => a.rate - b.rate || a.buffer_size - b.buffer_size);
}

/** Tick or untick one setup. */
export function toggled(picks: SuitePicks, setup: AggregateSetup, on: boolean): SuitePicks {
  const key = setupKey(setup);
  const rest = picks.setups.filter((one) => one !== key);
  return { ...picks, setups: on ? [...rest, key] : rest };
}

/**
 * The buffer sizes every interface's driver takes, from what each driver said, or the usual list
 * where none has said. A size one driver does not offer is not one the aggregate can run at.
 */
export function suiteBuffers(offered: (readonly number[] | undefined)[]): number[] {
  const known = offered.filter((sizes): sizes is readonly number[] => sizes !== undefined && sizes.length > 0);
  if (known.length === 0) return SUITE_BUFFERS;
  return [...(known[0] as readonly number[])].filter((size) => known.every((sizes) => sizes.includes(size))).sort((a, b) => a - b);
}

/**
 * Why the suite cannot be started as it stands, or nothing. The cabling is Line the interfaces up's,
 * on its Inputs pass, so whatever stops a measurement there stops this too.
 */
export function suiteProblem(picks: SuitePicks, single: AggregateCalibrateRequest | undefined, direction: string): string | undefined {
  if (direction !== "inputs") return "The suite measures what the interfaces record. Set Pass to Inputs under Line the interfaces up, and cable it as that says.";
  if (single === undefined) return "Set up the cabling under Line the interfaces up first: the suite plays its clicks through the same cables.";
  if (pickedSetups(picks).length === 0) return "Tick at least one rate and buffer size.";
  return undefined;
}

/** What starting the suite sends, from the ticks and the cabling a single run would take. */
export function suiteRequest(picks: SuitePicks, single: AggregateCalibrateRequest): AggregateSuiteRequest {
  return {
    setups: pickedSetups(picks),
    runs: picks.runs,
    outputs: single.outputs,
    inputs: single.inputs,
    clicks: single.clicks,
    level_dbfs: single.level_dbfs,
    confirmed: true,
  };
}

/** The buffer size the interfaces' drivers are on now, the callback master's first. */
function driverBuffer(answer: AggregateAnswer | undefined): number | undefined {
  const devices = answer?.devices ?? [];
  return devices.find((device) => device.is_master)?.driver.buffer_size ?? devices.find((device) => device.driver.buffer_size !== undefined)?.driver.buffer_size;
}

/**
 * **Every change the suite makes, one line each**, which is what the person confirms once for the
 * whole run: each setup it puts the aggregate and the drivers at, and what it puts back at the end.
 */
export function suiteChanges(setups: AggregateSetup[], runs: number, answer: AggregateAnswer | undefined): string[] {
  const lines = setups.map(
    (setup) =>
      `Put the aggregate at ${khz(setup.rate)}, and every interface's driver on ${setup.buffer_size} samples, which restarts the audio of every program using those drivers; then play ${runs} runs of clicks.`,
  );
  const rate = answer?.config?.rate;
  const buffer = answer?.config?.buffer_size;
  const drivers = driverBuffer(answer);
  const running = answer?.rate_in_force?.hz;
  const backRate = rate === undefined ? `whatever the interfaces are on${running === undefined ? "" : `, with the interfaces back at ${khz(running)}`}` : khz(rate);
  const backBuffer = buffer === undefined ? "whatever the drivers are on" : `${buffer} samples`;
  const backDrivers = drivers === undefined ? "" : `, and every interface's driver back on ${drivers} samples`;
  lines.push(`At the end, or when stopped, put the aggregate's rate back to ${backRate} and its buffer size to ${backBuffer}${backDrivers}.`);
  return lines;
}

/** How one setup reads in the progress list. `tone` is what the page colours it by. */
export interface SuiteSetupView {
  setup: string;
  text: string;
  tone: "waiting" | "busy" | "good" | "bad";
}

export function suiteSetupView(progress: AggregateSuiteSetup, runs: number | undefined): SuiteSetupView {
  const setup = setupWords(progress);
  switch (progress.state) {
    case "waiting":
      return { setup, text: "Waiting", tone: "waiting" };
    case "switching":
      return { setup, text: "Switching to this setup", tone: "busy" };
    case "measuring": {
      const of = runs === undefined ? `run ${progress.run ?? 1}` : `${progress.run ?? 1} of ${runs}`;
      const again = (progress.round ?? 1) > 1 ? `, again (${progress.round} of ${SUITE_ROUNDS})` : "";
      return { setup, text: `Measuring ${of}${again}`, tone: "busy" };
    }
    case "saved": {
      const kept = (progress.trims ?? []).map((trim) => `${trim.device} ${trim.input_trim}${trim.reference === undefined ? "" : ` (phase reference ${trim.reference})`}, runs ${trim.spread_samples} apart`);
      return { setup, text: `Saved: ${kept.join("; ")}`, tone: "good" };
    }
    case "failed":
      return { setup, text: `Failed: ${progress.why ?? "no reason was given."}`, tone: "bad" };
    case "stopped":
      return { setup, text: "Stopped: nothing kept for this setup", tone: "waiting" };
  }
}

/** Whether the suite is going, stopping included. */
export function suiteRunning(state: AggregateSuite | undefined): boolean {
  return state?.state === "running" || state?.state === "stopping";
}

/** The line under the list once the suite is over: what it came to, and what was put back. */
export function suiteSummary(state: AggregateSuite | undefined): { text: string; problem: boolean } | undefined {
  if (state === undefined || state.state === "idle" || suiteRunning(state)) return undefined;
  if (state.state === "failed") return { text: state.refusal ?? "The suite did not run.", problem: true };
  const failed = state.setups.filter((setup) => setup.state === "failed").length;
  const total = state.setups.length;
  const what = state.state === "stopped" ? `Stopped. ${state.saved} of ${total} setups saved before it was.` : `Finished: ${state.saved} of ${total} setups saved${failed === 0 ? "" : `, ${failed} failed`}.`;
  return { text: `${what} ${state.restored ?? ""}`.trim(), problem: failed > 0 || state.restore_failed === true };
}

/** One interface's trims per setup, as its card lists them, the one in force marked. */
export interface SetupTrimView {
  text: string;
  inForce: boolean;
}

export function setupTrimViews(device: AggregateDevice | undefined, inForce: AggregateSetup | undefined): SetupTrimView[] {
  const trims: AggregateSetupTrim[] = Array.isArray(device?.trims) ? (device.trims as AggregateSetupTrim[]) : [];
  return [...trims]
    .sort((a, b) => a.rate - b.rate || a.buffer_size - b.buffer_size)
    .map((trim) => {
      const now = inForce !== undefined && inForce.rate === trim.rate && inForce.buffer_size === trim.buffer_size;
      const reference = trim.reference === undefined ? "" : `, phase reference ${trim.reference}`;
      return { text: `${setupWords(trim)}: ${trim.input_trim}${reference}${now ? " (in force)" : ""}`, inForce: now };
    });
}

/**
 * The suite as the server has it, kept current while the page is open and while it runs. When it
 * saves a setup's trims the workspace is read again (`onSaved`), so the cards show them and a later
 * save from this page never writes the old setup back over them.
 */
export class SuiteModel {
  readonly #calls: SuiteCalls | undefined;
  readonly #timers: Timers;
  readonly #onSaved: () => void;
  readonly #state = signal<AggregateSuite | undefined>(undefined);
  readonly #offered = signal<boolean | undefined>(undefined);
  readonly #problem = signal<string | undefined>(undefined);
  readonly #busy = signal(false);
  #timer: unknown;
  #watching = false;
  /** What the server said last, which is what a change is a change from. */
  #last: AggregateSuite | undefined;

  constructor(calls: SuiteCalls | undefined, timers: Timers, onSaved: () => void) {
    this.#calls = calls;
    this.#timers = timers;
    this.#onSaved = onSaved;
  }

  get state(): ReadonlySignal<AggregateSuite | undefined> {
    return this.#state;
  }

  /** False on a server too old to run the suite. */
  get offered(): ReadonlySignal<boolean | undefined> {
    return this.#offered;
  }

  /** Why the last call did not work. */
  get problem(): ReadonlySignal<string | undefined> {
    return this.#problem;
  }

  get busy(): ReadonlySignal<boolean> {
    return this.#busy;
  }

  /** Follows the suite while the page is on screen; returns the stop. */
  activate(): () => void {
    this.#watching = true;
    this.#follow();
    return () => {
      this.#watching = false;
      this.#timers.clearTimeout(this.#timer);
      this.#timer = undefined;
    };
  }

  #follow(): void {
    void (async () => {
      const state = await this.#read();
      if (this.#watching && suiteRunning(state)) this.#timer = this.#timers.setTimeout(() => this.#follow(), SUITE_POLL_MS);
    })();
  }

  async #read(): Promise<AggregateSuite | undefined> {
    if (this.#calls === undefined) {
      this.#offered.value = false;
      return undefined;
    }
    try {
      const state = await this.#calls.state();
      this.#state.value = state;
      this.#offered.value = true;
      // The setup changed under this page: a setup's trims were saved, or the suite finished and
      // put the rate and buffer size back. A page that comes back to trims saved while it was away
      // reads it once as well.
      const was = this.#last;
      this.#last = state;
      const moved = was !== undefined && (was.saved !== state.saved || (suiteRunning(was) && !suiteRunning(state)));
      if (moved || (was === undefined && state.saved > 0)) this.#onSaved();
      return state;
    } catch (error) {
      const code = (error as { code?: unknown } | null)?.code;
      if (code === "http_404" || code === "http_403" || code === "not_local") this.#offered.value = false;
      else this.#problem.value = `The suite could not be read: ${error instanceof Error ? error.message : String(error)}`;
      return undefined;
    }
  }

  /** Starts it, with every change already confirmed by the person. */
  async start(request: AggregateSuiteRequest): Promise<void> {
    if (this.#calls === undefined) return;
    this.#busy.value = true;
    this.#problem.value = undefined;
    try {
      const started = await this.#calls.start(request);
      if (!started.started) this.#problem.value = "The suite did not start.";
    } catch (error) {
      this.#problem.value = `The suite did not start: ${error instanceof Error ? error.message : String(error)}`;
    } finally {
      this.#busy.value = false;
    }
    this.#timers.clearTimeout(this.#timer);
    this.#follow();
  }

  /** Stops it after the run that is going; what was saved stays saved. */
  async stop(): Promise<void> {
    if (this.#calls === undefined) return;
    this.#busy.value = true;
    try {
      await this.#calls.stop();
    } catch (error) {
      this.#problem.value = `The suite was not stopped: ${error instanceof Error ? error.message : String(error)}`;
    } finally {
      this.#busy.value = false;
    }
    this.#timers.clearTimeout(this.#timer);
    this.#follow();
  }
}
