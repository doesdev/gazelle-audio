// A device's hardware output levels, the discrete controls the vendor panels bind: volume, mute
// and, on the Quadro, dim. Ids and report fields come from the panels' bytecode (reference/devices.md,
// "Output ids" and "Studio+ output volumes"):
// - Quadro set_volume / set_mute / set_dim ids: MONITOR 0, HP1 1, HP2 2, LINE OUT 3, reported in the
//   cyclic `volumes[id]` {volume, mute, dim_on};
// - Studio+ set_volume / set_mute ids: MONITOR 0, HP1 1, HP2 2, LINE OUT 3, REAMP 4, reported in
//   `<output>_vol` and `<output>_mute`; it has no dim.
// Volume is dB of attenuation, 0..96 with 96 as -inf (hardware session 1 Q5 for the Quadro monitor;
// the Studio+ slider has the same range). A change outranks the device's reports for ECHO_HOLD_MS,
// as input changes do.

import { computed, signal, type ReadonlySignal, type Signal } from "../core/signal.ts";
import { ECHO_HOLD_MS } from "./inputs.ts";

export const VOLUME_MAX = 96;

export function formatVolume(volume: number): string {
  return volume >= VOLUME_MAX ? "-inf" : volume === 0 ? "0 dB" : `-${volume} dB`;
}

export interface OutputInfo {
  id: number;
  name: string;
  /** Whether the output has a dim control (the Quadro's do). */
  dim: boolean;
}

export interface OutputState {
  /** False until the device has reported this output; values are defaults until then. */
  known: boolean;
  volume: number;
  mute: boolean;
  dim: boolean;
}

const NAMES = ["Monitor", "HP1", "HP2", "Line out", "Reamp"] as const;
const STUDIO_FIELDS = ["monitor", "hp1", "hp2", "line_out", "reamp"] as const;

interface OutputsTimers {
  setTimeout(callback: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

export interface OutputsContext {
  deviceId: string;
  family: "quadro" | "studio";
  invoke(command: string, args: Record<string, number>, options: { coalesce?: string }): Promise<boolean>;
  field(name: string): ReadonlySignal<unknown>;
  watch(): () => void;
  timers: OutputsTimers;
}

export class OutputsModel {
  readonly deviceId: string;
  readonly family: "quadro" | "studio";
  readonly outputs: readonly OutputInfo[];
  readonly #context: OutputsContext;
  readonly #states: ReadonlySignal<OutputState>[];
  readonly #holds = new Map<string, { value: Signal<number | undefined>; timer: unknown }>();

  constructor(context: OutputsContext) {
    this.#context = context;
    this.deviceId = context.deviceId;
    this.family = context.family;
    const quadro = context.family === "quadro";
    this.outputs = NAMES.slice(0, quadro ? 4 : 5).map((name, id) => ({ id, name, dim: quadro }));
    const volumes = quadro ? context.field("volumes") : undefined;
    this.#states = this.outputs.map(({ id }) => {
      const studio = quadro ? undefined : { volume: context.field(this.#field(id, "volume")), mute: context.field(this.#field(id, "mute")) };
      return computed(() => {
        let reported: { volume: number; mute: number; dim: number } | undefined;
        if (volumes !== undefined) {
          const list = volumes.value;
          const entry = Array.isArray(list) ? (list[id] as Record<string, unknown> | undefined) : undefined;
          if (entry !== undefined) reported = { volume: Number(entry["volume"] ?? 0), mute: Number(entry["mute"] ?? 0), dim: Number(entry["dim_on"] ?? 0) };
        } else if (studio !== undefined && studio.volume.value !== undefined) {
          reported = { volume: Number(studio.volume.value), mute: Number(studio.mute.value ?? 0), dim: 0 };
        }
        return {
          known: reported !== undefined,
          volume: this.#hold(`${id}:volume`).value ?? reported?.volume ?? 0,
          mute: (this.#hold(`${id}:mute`).value ?? reported?.mute ?? 0) === 1,
          dim: (this.#hold(`${id}:dim`).value ?? reported?.dim ?? 0) === 1,
        };
      }, sameState);
    });
  }

  /** Follows the device's reports. Returns a disposer. */
  activate(): () => void {
    return this.#context.watch();
  }

  state(id: number): ReadonlySignal<OutputState> {
    this.#check(id);
    return this.#states[id] as ReadonlySignal<OutputState>;
  }

  setVolume(id: number, volume: number): void {
    this.#check(id);
    const value = Math.min(VOLUME_MAX, Math.max(0, Math.round(volume)));
    this.#change(`${id}:volume`, value, this.#field(id, "volume"));
    this.#send("set_volume", { id, volume: value }, `out_volume:${id}`);
  }

  setMute(id: number, on: boolean): void {
    this.#check(id);
    this.#change(`${id}:mute`, on ? 1 : 0, this.#field(id, "mute"));
    this.#send("set_mute", { id, mute: on ? 1 : 0 }, `out_mute:${id}`);
  }

  setDim(id: number, on: boolean): void {
    this.#check(id);
    if (!(this.outputs[id] as OutputInfo).dim) throw new Error(`the ${this.family === "studio" ? "Studio+" : this.family} has no dim for ${(this.outputs[id] as OutputInfo).name}`);
    this.#change(`${id}:dim`, on ? 1 : 0, "volumes");
    this.#send("set_dim", { periph_id: id, dim: on ? 1 : 0 }, `out_dim:${id}`);
  }

  /** The report field that carries an output's volume or mute. */
  #field(id: number, part: "volume" | "mute"): string {
    return this.family === "quadro" ? "volumes" : `${STUDIO_FIELDS[id]}_${part === "volume" ? "vol" : "mute"}`;
  }

  #check(id: number): void {
    if (!Number.isInteger(id) || id < 0 || id >= this.outputs.length) throw new RangeError(`output ${id} is outside 0..${this.outputs.length - 1}`);
  }

  #send(command: string, args: Record<string, number>, key: string): void {
    void this.#context.invoke(command, args, { coalesce: `${key}:${this.deviceId}` });
  }

  #hold(key: string): Signal<number | undefined> {
    let entry = this.#holds.get(key);
    if (entry === undefined) {
      entry = { value: signal<number | undefined>(undefined), timer: undefined };
      this.#holds.set(key, entry);
    }
    return entry.value;
  }

  /** Shows `value` now and keeps it over the device's reports for ECHO_HOLD_MS. */
  #change(key: string, value: number, reportedField: string): void {
    const hold = this.#hold(key);
    const entry = this.#holds.get(key) as { value: Signal<number | undefined>; timer: unknown };
    hold.value = value;
    this.#context.timers.clearTimeout(entry.timer);
    const reported = this.#context.field(reportedField);
    entry.timer = this.#context.timers.setTimeout(() => {
      entry.timer = undefined;
      // Without any report the local value is all there is, so it stays.
      if (reported.peek() !== undefined) hold.value = undefined;
    }, ECHO_HOLD_MS);
  }
}

function sameState(a: OutputState, b: OutputState): boolean {
  return a.known === b.known && a.volume === b.volume && a.mute === b.mute && a.dim === b.dim;
}
