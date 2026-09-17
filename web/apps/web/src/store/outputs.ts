// A device's hardware output controls, as its vendor panel binds them (reference/devices.md,
// "Output ids", "Studio+ output volumes" and "Control Room controls"):
// - volume, mute and, on the Quadro, dim per output. Quadro set_volume / set_mute / set_dim ids:
//   MONITOR 0, HP1 1, HP2 2, LINE OUT 3, reported in the cyclic `volumes[id]` {volume, mute, dim_on,
//   mono}; Studio+ set_volume / set_mute ids: those four plus REAMP 4, reported in `<output>_vol`
//   and `<output>_mute`, with no dim;
// - mono, which only the Quadro reports and neither model can set, so it is shown, not sent;
// - trims in seven steps from 20 to 14 dBu: Quadro set_trim_config (MONITOR 0, LINE OUT 1),
//   Studio+ set_trim (MONITOR 0, LINE OUT 1, ADC 2), reported in `monitor_trim`, `line_out_trim`,
//   `adc_trim`;
// - talkback, Studio+ only: set_talk (momentary in the UI), set_tbk_vol for the talkback level
//   (`tb_mic_volume`) and set_tbk_enable to HP1 0, HP2 1, MONITOR 2. The panel's talkback control is
//   a level fader, so it takes the same scale as the outputs' volume: dB of attenuation, 96 = -inf
//   (the user and hardware session 2, 2026-09-15).
// Volume is dB of attenuation, 0..96 with 96 as -inf (hardware session 1 Q5 for the Quadro monitor;
// assumed for the Studio+, Q10). A change outranks the device's reports for ECHO_HOLD_MS, as input
// changes do.

import { computed, signal, type ReadonlySignal, type Signal } from "../core/signal.ts";
import { ECHO_HOLD_MS } from "./inputs.ts";

export const VOLUME_MAX = 96;

/** The outputs a Control Room panel shows until the user chooses (P92): Monitor, HP1 and HP2, the same ids on both models. */
export const CONTROL_ROOM_DEFAULT: readonly number[] = [0, 1, 2];

/** The panels' trim steps, by index. */
export const TRIM_LABELS = ["20 dBu", "19 dBu", "18 dBu", "17 dBu", "16 dBu", "15 dBu", "14 dBu"] as const;

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
  /** Reported by the Quadro only; no command sets it. */
  mono: boolean;
}

export interface TrimInfo {
  id: number;
  name: string;
}

export interface TrimState {
  known: boolean;
  /** Into TRIM_LABELS. */
  index: number;
}

export interface TalkState {
  known: boolean;
  on: boolean;
  volume: number;
  /** Whether talkback reaches each destination, in `talkback.destinations` order. */
  to: boolean[];
}

const NAMES = ["Monitor", "HP1", "HP2", "Line out", "Reamp"] as const;
const STUDIO_FIELDS = ["monitor", "hp1", "hp2", "line_out", "reamp"] as const;
const TRIMS: readonly TrimInfo[] = [
  { id: 0, name: "Monitor" },
  { id: 1, name: "Line out" },
  { id: 2, name: "ADC" },
];
const TRIM_FIELDS = ["monitor_trim", "line_out_trim", "adc_trim"] as const;
const TALKBACK_DESTINATIONS: readonly TrimInfo[] = [
  { id: 0, name: "HP1" },
  { id: 1, name: "HP2" },
  { id: 2, name: "Monitor" },
];
const TALKBACK_FIELDS = ["hp1_enabled", "hp2_enabled", "mon_enabled"] as const;
/** set_trim_config carries 32 two-byte level entries; the Quadro panel fills the first. */
const TRIM_LEVEL_ENTRIES = 32;

interface OutputsTimers {
  setTimeout(callback: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

export interface OutputsContext {
  deviceId: string;
  family: "quadro" | "studio";
  invoke(command: string, args: Record<string, unknown>, options: { coalesce?: string }): Promise<boolean>;
  field(name: string): ReadonlySignal<unknown>;
  watch(): () => void;
  timers: OutputsTimers;
}

export class OutputsModel {
  readonly deviceId: string;
  readonly family: "quadro" | "studio";
  readonly outputs: readonly OutputInfo[];
  readonly trims: readonly TrimInfo[];
  /** Studio+ only; its level uses the same scale as the outputs' volume. */
  readonly talkback: { destinations: readonly TrimInfo[] } | undefined;
  readonly talk: ReadonlySignal<TalkState>;
  /** Quadro only: one switch that mutes every output, for changing monitors safely. */
  readonly hasHardMute: boolean;
  readonly hardMute: ReadonlySignal<boolean>;
  readonly #context: OutputsContext;
  readonly #states: ReadonlySignal<OutputState>[];
  readonly #trims: ReadonlySignal<TrimState>[];
  readonly #holds = new Map<string, { value: Signal<number | undefined>; timer: unknown }>();

  constructor(context: OutputsContext) {
    this.#context = context;
    this.deviceId = context.deviceId;
    this.family = context.family;
    const quadro = context.family === "quadro";
    this.outputs = NAMES.slice(0, quadro ? 4 : 5).map((name, id) => ({ id, name, dim: quadro }));
    this.trims = TRIMS.slice(0, quadro ? 2 : 3);
    this.talkback = quadro ? undefined : { destinations: TALKBACK_DESTINATIONS };
    this.hasHardMute = quadro;
    const hardMute = context.field("hard_mute");
    this.hardMute = computed(() => (this.#hold("hard_mute").value ?? Number(hardMute.value ?? 0)) === 1);

    const volumes = quadro ? context.field("volumes") : undefined;
    this.#states = this.outputs.map(({ id }) => {
      const studio = quadro ? undefined : { volume: context.field(this.#field(id, "volume")), mute: context.field(this.#field(id, "mute")) };
      return computed(() => {
        let reported: { volume: number; mute: number; dim: number; mono: number } | undefined;
        if (volumes !== undefined) {
          const list = volumes.value;
          const entry = Array.isArray(list) ? (list[id] as Record<string, unknown> | undefined) : undefined;
          if (entry !== undefined) reported = { volume: Number(entry["volume"] ?? 0), mute: Number(entry["mute"] ?? 0), dim: Number(entry["dim_on"] ?? 0), mono: Number(entry["mono"] ?? 0) };
        } else if (studio !== undefined && studio.volume.value !== undefined) {
          reported = { volume: Number(studio.volume.value), mute: Number(studio.mute.value ?? 0), dim: 0, mono: 0 };
        }
        return {
          known: reported !== undefined,
          volume: this.#hold(`${id}:volume`).value ?? reported?.volume ?? 0,
          mute: (this.#hold(`${id}:mute`).value ?? reported?.mute ?? 0) === 1,
          dim: (this.#hold(`${id}:dim`).value ?? reported?.dim ?? 0) === 1,
          mono: reported?.mono === 1,
        };
      }, sameState);
    });

    this.#trims = this.trims.map(({ id }) => {
      const reported = context.field(TRIM_FIELDS[id] as string);
      return computed(() => {
        const value = reported.value;
        return { known: value !== undefined, index: this.#hold(`trim:${id}`).value ?? Number(value ?? 0) };
      }, (a, b) => a.known === b.known && a.index === b.index);
    });

    const talkOn = context.field("talkback_on");
    const micVolume = context.field("tb_mic_volume");
    const destinations = TALKBACK_FIELDS.map((name) => context.field(name));
    this.talk = computed(() => {
      if (quadro) return { known: false, on: false, volume: 0, to: [false, false, false] };
      return {
        known: talkOn.value !== undefined,
        on: (this.#hold("talk:on").value ?? Number(talkOn.value ?? 0)) === 1,
        volume: this.#hold("talk:volume").value ?? Number(micVolume.value ?? 0),
        to: destinations.map((field, id) => (this.#hold(`talk:to:${id}`).value ?? Number(field.value ?? 0)) === 1),
      };
    }, (a, b) => a.known === b.known && a.on === b.on && a.volume === b.volume && a.to.every((t, i) => t === b.to[i]));
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
    if (!(this.outputs[id] as OutputInfo).dim) throw new Error(`the ${this.#modelName()} has no dim for ${(this.outputs[id] as OutputInfo).name}`);
    this.#change(`${id}:dim`, on ? 1 : 0, "volumes");
    this.#send("set_dim", { periph_id: id, dim: on ? 1 : 0 }, `out_dim:${id}`);
  }

  trim(id: number): ReadonlySignal<TrimState> {
    this.#checkTrim(id);
    return this.#trims[id] as ReadonlySignal<TrimState>;
  }

  setTrim(id: number, index: number): void {
    this.#checkTrim(id);
    const value = Math.min(TRIM_LABELS.length - 1, Math.max(0, Math.round(index)));
    this.#change(`trim:${id}`, value, TRIM_FIELDS[id] as string);
    if (this.family === "quadro") {
      const level = Array.from({ length: TRIM_LEVEL_ENTRIES }, (_, i) => Uint8Array.of(i === 0 ? value : 0, 0));
      this.#send("set_trim_config", { trim_id: id, control: 1, level }, `out_trim:${id}`);
    } else {
      this.#send("set_trim", { id, trim_idx: value }, `out_trim:${id}`);
    }
  }

  /**
   * Mutes or unmutes every output at once. The vendor panel uses this while it restores a session,
   * so nothing plays through half-applied routing; here it is the same idea under the user's hand.
   */
  setHardMute(on: boolean): void {
    if (!this.hasHardMute) throw new Error(`the ${this.#modelName()} has no hard mute`);
    this.#change("hard_mute", on ? 1 : 0, "hard_mute");
    this.#send("set_hard_mute", { value: on ? 1 : 0 }, "hard_mute");
  }

  setTalk(on: boolean): void {
    this.#checkTalkback();
    this.#change("talk:on", on ? 1 : 0, "talkback_on");
    this.#send("set_talk", { on: on ? 1 : 0 }, "talk");
  }

  /** Sets the talkback level: dB of attenuation on the outputs' scale, 0..VOLUME_MAX. */
  setTalkbackVolume(volume: number): void {
    this.#checkTalkback();
    const value = Math.min(VOLUME_MAX, Math.max(0, Math.round(volume)));
    this.#change("talk:volume", value, "tb_mic_volume");
    this.#send("set_tbk_vol", { volume: value }, "talk_volume");
  }

  /** Sends talkback to a destination (`talkback.destinations`) or stops it. */
  setTalkbackTo(id: number, on: boolean): void {
    this.#checkTalkback();
    if (!Number.isInteger(id) || id < 0 || id >= TALKBACK_DESTINATIONS.length) throw new RangeError(`talkback destination ${id} is outside 0..${TALKBACK_DESTINATIONS.length - 1}`);
    this.#change(`talk:to:${id}`, on ? 1 : 0, TALKBACK_FIELDS[id] as string);
    this.#send("set_tbk_enable", { id, enabled: on ? 1 : 0 }, `talk_to:${id}`);
  }

  /** The report field that carries an output's volume or mute. */
  #field(id: number, part: "volume" | "mute"): string {
    return this.family === "quadro" ? "volumes" : `${STUDIO_FIELDS[id]}_${part === "volume" ? "vol" : "mute"}`;
  }

  #modelName(): string {
    return this.family === "studio" ? "Studio+" : "Quadro";
  }

  #check(id: number): void {
    if (!Number.isInteger(id) || id < 0 || id >= this.outputs.length) throw new RangeError(`output ${id} is outside 0..${this.outputs.length - 1}`);
  }

  #checkTrim(id: number): void {
    if (!Number.isInteger(id) || id < 0 || id >= this.trims.length) throw new RangeError(`the ${this.#modelName()} has no trim ${id} (trims 0..${this.trims.length - 1})`);
  }

  #checkTalkback(): void {
    if (this.talkback === undefined) throw new Error(`the ${this.#modelName()} has no talkback`);
  }

  #send(command: string, args: Record<string, unknown>, key: string): void {
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
  return a.known === b.known && a.volume === b.volume && a.mute === b.mute && a.dim === b.dim && a.mono === b.mono;
}
