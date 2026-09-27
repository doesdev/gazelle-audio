// The recorder, as the Recording page and the Remote page's transport both show it: one model per
// store, made the first time either page asks, so it travels with those pages and not with the app.
//
// The state comes from the server: once when a page opens, then as `recording` frames on the socket,
// five times a second while armed. Record and Stop are one press each, and are sent as soon as they
// are pressed: missing the start of a take is the worse failure, so nothing here asks first. Arm is
// one press too, and the page explains it the first time (`ARM_EXPLAINED_KEY`). Disarming while a
// take is running asks first, on the page and at the server (`confirm_disarm`).

import { GazelleError, type RecordingPreset, type RecordingStatus, type RecordingTake } from "gazelle-audio-client";

import { signal, type ReadonlySignal } from "../core/signal.ts";

export type { RecordingPreset, RecordingStatus, RecordingTake };

/** What the model calls; the store's `recorder`. */
export interface RecorderApi {
  status(): Promise<RecordingStatus>;
  arm(preset: string): Promise<RecordingStatus>;
  record(): Promise<RecordingStatus>;
  stop(): Promise<RecordingStatus>;
  disarm(confirm: boolean): Promise<RecordingStatus>;
  takes(): Promise<{ takes: RecordingTake[] }>;
  /** Follows the live state; returns how to stop. */
  follow(listener: (status: RecordingStatus) => void): () => void;
}

/** Remembered in this browser once Arm has been explained, so it is explained once. */
export const ARM_EXPLAINED_KEY = "gazelle.recording.armExplained";
/** The preset last chosen in this browser, when the server has none to offer. */
export const PRESET_KEY = "gazelle.recording.preset";
/** How often the list of takes is read while armed, to catch a take that has just been written. */
export const TAKES_POLL_MS = 1_500;

/** The press being sent, so its button waits for the answer. */
export type RecordingAction = "arm" | "record" | "stop" | "disarm";

const message = (error: unknown): string => (error instanceof Error ? error.message : String(error));

export class RecordingModel {
  readonly #api: RecorderApi;
  readonly #status = signal<RecordingStatus | undefined>(undefined);
  readonly #takes = signal<RecordingTake[]>([]);
  readonly #busy = signal<RecordingAction | undefined>(undefined);
  readonly #problem = signal<string | undefined>(undefined);
  #watchers = 0;
  #unfollow: (() => void) | undefined;
  #timer: ReturnType<typeof setInterval> | undefined;

  constructor(api: RecorderApi) {
    this.#api = api;
  }

  get status(): ReadonlySignal<RecordingStatus | undefined> {
    return this.#status;
  }

  get takes(): ReadonlySignal<RecordingTake[]> {
    return this.#takes;
  }

  get busy(): ReadonlySignal<RecordingAction | undefined> {
    return this.#busy;
  }

  /** What the last press came to when it was refused, in the server's words. */
  get problem(): ReadonlySignal<string | undefined> {
    return this.#problem;
  }

  /** Follow the recorder while a page shows it. Returns how to stop. */
  activate(): () => void {
    this.#watchers += 1;
    if (this.#watchers === 1) {
      this.#unfollow = this.#api.follow((status) => this.#took(status));
      void this.refresh();
      this.#timer = setInterval(() => {
        if (this.#status.peek()?.state !== "off") void this.#readTakes();
      }, TAKES_POLL_MS);
    }
    let released = false;
    return () => {
      if (released) return;
      released = true;
      this.#watchers -= 1;
      if (this.#watchers === 0) {
        this.#unfollow?.();
        this.#unfollow = undefined;
        clearInterval(this.#timer);
        this.#timer = undefined;
      }
    };
  }

  /** Ask for the whole state now. */
  async refresh(): Promise<void> {
    try {
      this.#took(await this.#api.status());
    } catch (error) {
      this.#problem.value = message(error);
    }
    await this.#readTakes();
  }

  async #readTakes(): Promise<void> {
    try {
      const { takes } = await this.#api.takes();
      if (JSON.stringify(takes) !== JSON.stringify(this.#takes.peek())) this.#takes.value = takes;
    } catch {
      // The list is read again in a moment; the transport says what is wrong if anything is.
    }
  }

  #took(status: RecordingStatus): void {
    const was = this.#status.peek();
    this.#status.value = status;
    // A take that has just ended, or a disarm, is a new file in the list.
    if (was?.take !== undefined && status.take === undefined) void this.#readTakes();
  }

  async #press(action: RecordingAction, call: () => Promise<RecordingStatus>): Promise<boolean> {
    this.#busy.value = action;
    this.#problem.value = undefined;
    try {
      this.#took(await call());
      return true;
    } catch (error) {
      this.#problem.value = message(error);
      return false;
    } finally {
      this.#busy.value = undefined;
    }
  }

  arm(preset: string): Promise<boolean> {
    return this.#press("arm", () => this.#api.arm(preset));
  }

  record(): Promise<boolean> {
    return this.#press("record", () => this.#api.record());
  }

  stop(): Promise<boolean> {
    return this.#press("stop", () => this.#api.stop());
  }

  /** Disarm; `confirm` when a take is running, which it stops. */
  async disarm(confirm: boolean): Promise<boolean> {
    const done = await this.#press("disarm", () => this.#api.disarm(confirm));
    await this.#readTakes();
    return done;
  }

  /** Record when armed, Stop when recording: what Space does on the Recording page. */
  toggle(): Promise<boolean> | undefined {
    const state = this.#status.peek()?.state;
    if (state === "armed") return this.record();
    if (state === "recording") return this.stop();
    return undefined;
  }
}

const models = new WeakMap<object, RecordingModel>();

/** The one model for this store. */
export function recordingModel(store: { readonly recorder: RecorderApi }): RecordingModel {
  let model = models.get(store);
  if (model === undefined) {
    model = new RecordingModel(store.recorder);
    models.set(store, model);
  }
  return model;
}

/** Whether an error is the server asking for Disarm to be confirmed. */
export function needsConfirming(error: unknown): boolean {
  return error instanceof GazelleError && error.code === "confirm_disarm";
}

// ---------------------------------------------------------------------------------------------
// What the pages show.
// ---------------------------------------------------------------------------------------------

/** A clock: "0:07.4", "12:03.0", "1:02:03". */
export function clockText(seconds: number | undefined): string {
  const total = Math.max(0, seconds ?? 0);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const rest = total - hours * 3600 - minutes * 60;
  const secs = rest.toFixed(1).padStart(4, "0");
  return hours > 0 ? `${hours}:${String(minutes).padStart(2, "0")}:${secs}` : `${minutes}:${secs}`;
}

/** Seconds in words for a note: "4.8 s", "2 min 15 s", "1 h 3 min". */
export function secondsText(seconds: number): string {
  if (seconds < 60) return `${seconds < 10 ? seconds.toFixed(1) : Math.round(seconds)} s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)} min ${Math.round(seconds % 60)} s`;
  return `${Math.floor(seconds / 3600)} h ${Math.round((seconds % 3600) / 60)} min`;
}

/** Bytes in words: "812 MB", "1.6 GB". */
export function bytesText(bytes: number): string {
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
  return `${Math.round(bytes / 1024 ** 2)} MB`;
}

/** The state as the big display says it. */
export function stateLabel(status: RecordingStatus | undefined): string {
  switch (status?.state) {
    case "recording":
      return "Recording";
    case "armed":
      return "Armed";
    case "arming":
      return "Arming…";
    case "disarming":
      return "Disarming…";
    default:
      return "Off";
  }
}

/** The pre-roll held and what it can hold, as the transport says it: "4.8 of 60.0 s held (10 % of free memory)". */
export function prerollText(status: RecordingStatus | undefined): string {
  const preroll = status?.preroll;
  if (preroll === undefined) return "No pre-roll held";
  if (status?.state === "recording") return `Started with ${secondsText(preroll.held_seconds)} of pre-roll`;
  return `${preroll.held_seconds.toFixed(1)} of ${preroll.preroll_seconds.toFixed(1)} s of pre-roll held (${preroll.percent.toFixed(1)} % of free memory)`;
}

/** How far the pre-roll has filled, 0 to 1. */
export function prerollFill(status: RecordingStatus | undefined): number {
  const preroll = status?.preroll;
  if (preroll === undefined || preroll.preroll_seconds <= 0) return 0;
  return Math.min(1, Math.max(0, preroll.held_seconds / preroll.preroll_seconds));
}

/** Where a meter's bar reaches for a peak in dBFS: 0 at -60 dB and below, 1 at 0 dB. */
export function meterFill(dbfs: number | undefined): number {
  if (dbfs === undefined) return 0;
  return Math.min(1, Math.max(0, (dbfs + 60) / 60));
}

/** What was lost, when anything was: for the transport's warning line. */
export function lossText(status: RecordingStatus | undefined): string | undefined {
  if (status === undefined) return undefined;
  const parts: string[] = [];
  if (status.overruns > 0) parts.push(`${status.overruns} block${status.overruns === 1 ? "" : "s"} lost because the disk fell behind (each is silence in its take, and its log says where)`);
  if (status.dropouts > 0) {
    const who = status.dropouts_by_device.filter((d) => d.dropped + d.starved > 0).map((d) => `${d.device} ${d.dropped + d.starved}`);
    parts.push(`${status.dropouts} block${status.dropouts === 1 ? "" : "s"} lost by the aggregate (${who.join(", ")})`);
  }
  return parts.length === 0 ? undefined : `Since Arm: ${parts.join("; ")}.`;
}

/** The disk, in words, while armed. */
export function diskText(status: RecordingStatus | undefined): string | undefined {
  if (status?.disk_free_bytes === undefined) return undefined;
  const left = status.disk_seconds_left === undefined ? "" : `, about ${secondsText(status.disk_seconds_left)} of recording`;
  return `${bytesText(status.disk_free_bytes)} free${left}`;
}

/** The folder a file is in, from its whole path. */
export function folderOf(path: string): string {
  const at = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
  return at < 0 ? "" : path.slice(0, at);
}

/** The file's own name, from its whole path. */
export function fileOf(path: string): string {
  return path.slice(Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/")) + 1);
}

/** A take in a line: "T003, 2026-09-27 14:03:20, 1 min 12 s (8.0 s pre-roll), 4 files". */
export function takeLine(take: RecordingTake): string {
  return `T${String(take.number).padStart(3, "0")}, ${take.date} ${take.time}, ${secondsText(take.seconds)} (${secondsText(take.preroll_seconds)} pre-roll), ${take.files.length} file${take.files.length === 1 ? "" : "s"}`;
}

/** A new preset's id, not one the list has. */
export function newPresetId(presets: readonly RecordingPreset[]): string {
  for (let n = presets.length + 1; ; n += 1) {
    const id = `preset-${n}`;
    if (!presets.some((p) => p.id === id)) return id;
  }
}

/** A new preset: named, recording nothing yet, with every other choice at its default. */
export function newPreset(presets: readonly RecordingPreset[]): RecordingPreset {
  let n = presets.length + 1;
  while (presets.some((p) => p.name === `Preset ${n}`)) n += 1;
  return { id: newPresetId(presets), name: `Preset ${n}`, channels: [] };
}

/** The preset to offer first: the one armed, else the server's last, else this browser's, else the first. */
export function presetToOffer(presets: readonly RecordingPreset[], status: RecordingStatus | undefined, remembered: string | undefined): string | undefined {
  const known = (id: string | null | undefined) => (id !== undefined && id !== null && presets.some((p) => p.id === id) ? id : undefined);
  return known(status?.preset?.id) ?? known(status?.last_preset) ?? known(remembered) ?? presets[0]?.id;
}

/** Whether a preset could be armed with, as far as the page can tell, or why not. */
export function presetProblem(preset: RecordingPreset | undefined): string | undefined {
  if (preset === undefined) return "Make a preset first.";
  if ((preset.channels ?? []).length === 0) return `${preset.name} records no channels yet. Tick at least one.`;
  const pattern = preset.pattern;
  if (pattern !== undefined && pattern.trim() !== "") {
    if (!pattern.includes("{channel}")) return "The file name pattern needs {channel}.";
    if (!pattern.includes("{take}")) return "The file name pattern needs {take}.";
  }
  return undefined;
}

/** The server's default file name pattern. */
export const PATTERN_DEFAULT = "{date} T{take} {channel}";
/** What a preset's pre-roll takes when it does not say, in percent of the free memory. */
export const PERCENT_DEFAULT = 10;
/** The percentages offered. */
export const PERCENTS = [2, 5, 10, 15, 20, 30, 50];
