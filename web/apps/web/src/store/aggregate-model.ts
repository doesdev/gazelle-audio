// The part of the aggregate the store keeps from the start: the model that polls the server's answer
// while the Aggregate page is open, and the calls a button on that page can make. Everything the page
// shows of that answer, the naming of each interface's channels and the measurement's views, is in
// `aggregate.ts`, which re-exports this and travels with the pages that need it rather than with the
// app (`test/bundle-split.test.ts`).
//
// Two things shape the polling. The route answers only a caller on the same machine, so a phone is
// refused, and a server too old to know it answers 404: that is "this server does not offer it", not
// a failure, and there is then nothing to keep asking for. And the answer reads the audio drivers,
// which is not free, so it is asked for often only when the figures move: every second while a DAW
// is streaming, less often while it merely has the driver open, and slowly while nothing is going on
// at all, which is the ordinary case.

import { signal, type ReadonlySignal } from "../core/signal.ts";
import type {
  AggregateAnswer,
  AggregateCalibrateRequest,
  AggregateCalibration,
  AggregateFix,
  AggregateMatchBuffers,
  AggregateRegistrationRun,
  AggregateStatusReading,
  Timers,
} from "gazelle-audio-client";

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
  | { call: "setup-rate"; rate: number }
  | { call: "register" }
  | { call: "unregister" }
  | { call: "match-buffers"; bufferSize: number }
  | { call: "command"; deviceId: string; command: string; args: Record<string, unknown> };

const COMMAND_ROUTE = /^devices\/([^/]+)\/command\/([a-z0-9_]+)$/;

/**
 * `restore_phase_path` is not answered here: it is routing, which the Aggregate page writes itself
 * through the routing model (`phase-path.ts`, which comes with the page), so this says nothing of it.
 */
export function fixRequest(fix: AggregateFix): FixRequest | undefined {
  const body = typeof fix.body === "object" && fix.body !== null ? (fix.body as Record<string, unknown>) : {};
  // The one the page makes itself, in the setup it keeps in the workspace.
  if (fix.kind === "set_setup_rate") {
    const rate = body["rate"];
    return typeof rate === "number" && Number.isInteger(rate) && rate > 0 ? { call: "setup-rate", rate } : undefined;
  }
  if (fix.method !== "POST") return undefined;
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
  return fix.kind === "match_buffers" || fix.kind === "set_setup_rate" || fix.kind === "restore_phase_path";
}

/** A rate in words: "96 kHz", "44.1 kHz". */
export function khz(hz: number): string {
  return `${Number((hz / 1000).toFixed(3))} kHz`;
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

/** Whether a run is going, which is the only time the page asks the server about one again. */
export function calibrateRunning(state: AggregateCalibration | undefined): boolean {
  return state?.state === "running";
}

/** What the model needs of the client, so it can be decided without one. */
export interface AggregateContext {
  read(): Promise<AggregateAnswer>;
  matchBuffers(bufferSize: number, options?: { force?: boolean }): Promise<AggregateMatchBuffers>;
  register(): Promise<AggregateRegistrationRun>;
  unregister(): Promise<AggregateRegistrationRun>;
  /** One command to one device, as the Devices page sends one. True when it went. */
  command(deviceId: string, command: string, args: Record<string, unknown>): Promise<boolean>;
  /** Puts a rate into the aggregate's setup in the workspace. True when it was saved. */
  setupRate(rate: number): boolean;
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
   * false when asked from a phone, where the page says so rather than showing a failure.
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
    if (request.call === "setup-rate") {
      this.#outcome.value = this.#context.setupRate(request.rate)
        ? { text: `The aggregate's rate is ${khz(request.rate)} now, in the setup.`, problem: false }
        : { text: "The setup's rate was not changed: Gazelle is not connected.", problem: true };
      await this.refresh();
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

  /**
   * A fix the page makes itself (a phase path's routing): busy while it runs, what it came to said
   * under the reasons, and the answer read again afterwards, as for every other fix.
   */
  async perform(task: () => Promise<{ text: string; problem: boolean }>): Promise<void> {
    this.#busy.value = "fixing";
    try {
      this.#outcome.value = await task();
    } catch (error) {
      this.#outcome.value = { text: message(error), problem: true };
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
