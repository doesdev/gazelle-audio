// One device mixer (spec §10 row 4). Values and commands follow what the vendor panels do
// (reference/devices.md, "Mixer and meter value scales"; decisions P26–P30):
// - level is dB of attenuation, 0..90 on a linear fader, shown 0 dB … −90 dB;
// - pan is 2..62 with 32 as centre (27..38 snaps to it), shown −30…+30;
// - Studio+ `send` is a raw byte whose scale is not known;
// - a meter byte is dB below full scale on Antelope's piecewise scale, and 0 latches clip;
// - device channel 0 is the master and strip i is device channel i + 1 (assumed for Studio+).
// Every strip command carries the whole strip, so coalescing per strip never loses a field.
// `load()` reads the mixer's state: get_mixer with the mixer in ext3 (33 entries, master first)
// and its 16 pairs of get_mixer_links (entry k covers strips 2k and 2k+1). Until then, and in dry
// run, values are defaults.

import type { Topology } from "gazelle-audio-client";

import { batch, computed, effect, signal, type ReadonlySignal, type Signal } from "../core/signal.ts";

export const LEVEL_MAX = 90;
export const PAN_MIN = 2;
export const PAN_MAX = 62;
export const PAN_CENTRE = 32;
const PAN_SNAP = [27, 38] as const;
export const SEND_MAX = 255;
/** Scale marks in dB below full scale. */
export const METER_MARKS = [0, 5, 10, 15, 20, 30, 40, 60] as const;
/** Quadro meter sources for mixers 1–3: MONITOR, HP2, LINE_OUT (mixer 4's is not known). */
const QUADRO_METER_SOURCES = [15, 12, 19] as const;

export function levelDb(level: number): number {
  return level === 0 ? 0 : -level;
}

export function levelFromDb(db: number): number {
  return Math.min(LEVEL_MAX, Math.max(0, Math.round(-db)));
}

export function formatLevel(level: number): string {
  return `${levelDb(level)} dB`;
}

export function clampPan(value: number): number {
  const pan = Math.min(PAN_MAX, Math.max(PAN_MIN, Math.round(value)));
  return pan >= PAN_SNAP[0] && pan <= PAN_SNAP[1] ? PAN_CENTRE : pan;
}

export function formatPan(pan: number): string {
  const offset = pan - PAN_CENTRE;
  return offset > 0 ? `+${offset}` : String(offset);
}

/** Antelope's meter scale: how far (0..100) a bar reaches for a byte of dB below full scale. */
export function meterDeflection(byte: number): number {
  if (byte > 60) return 0;
  if (byte > 50) return (60 - byte) * 0.5;
  if (byte > 40) return 50 - byte + 5;
  if (byte > 30) return (40 - byte) * 1.5 + 15;
  if (byte > 20) return (30 - byte) * 2 + 30;
  if (byte > 0) return (20 - byte) * 2.5 + 50;
  return 100;
}

export interface StripState {
  readonly level: number;
  readonly pan: number;
  readonly mute: boolean;
  readonly solo: boolean;
  readonly send: number;
  readonly linked: boolean;
}

export type StripId = number | "master";

export type MixerInvoke = (command: string, args: Record<string, number>, options: { coalesce?: string }) => Promise<boolean>;

/** Reads a command's reply (`ext3` for selectors); `response` is null in dry run or when it failed (which the store reports). */
export type CommandRead = (command: string, ext3?: number) => Promise<{ response: Record<string, unknown> | null; dryRun: boolean }>;

export interface MixerContext {
  deviceId: string;
  family: "quadro" | "studio";
  index: number;
  topology: Topology;
  invoke: MixerInvoke;
  read: CommandRead;
  field(name: string): ReadonlySignal<unknown>;
  watch(): () => void;
}

const DEFAULT_STRIP: StripState = { level: 0, pan: PAN_CENTRE, mute: false, solo: false, send: 0, linked: false };

export class MixerModel {
  readonly deviceId: string;
  readonly index: number;
  readonly channels: number;
  /** Studio+ strips have a send; Quadro's do not. */
  readonly hasSend: boolean;
  /** Whether choosing this mixer can point the device's meters at it. */
  readonly meterSourceSelectable: boolean;
  readonly #known = signal(false);
  readonly #context: MixerContext;
  readonly #strips: Signal<StripState>[];
  readonly #master = signal<StripState>(DEFAULT_STRIP);
  readonly #clips: Signal<boolean>[];
  readonly #meters = new Map<number, ReadonlySignal<number | undefined>>();

  constructor(context: MixerContext) {
    this.#context = context;
    this.deviceId = context.deviceId;
    this.index = context.index;
    this.channels = context.topology.mixers.channels;
    this.hasSend = context.topology.mixers.command === "set_mixer_cfg";
    this.meterSourceSelectable = context.family === "studio" || context.index < QUADRO_METER_SOURCES.length;
    this.#strips = Array.from({ length: this.channels }, () => signal(DEFAULT_STRIP));
    this.#clips = Array.from({ length: this.channels }, () => signal(false));
  }

  /** True once the device's state has been read; until then values are defaults. */
  get stateKnown(): ReadonlySignal<boolean> {
    return this.#known;
  }

  /** Reads the mixer's strips and links from the device. Resolves false when nothing was read (dry run, or a failure the store reports). */
  async load(): Promise<boolean> {
    const strips = (await this.#context.read("get_mixer", this.index)).response?.["entries"];
    if (!Array.isArray(strips)) return false;
    const links = (await this.#context.read("get_mixer_links")).response?.["entries"];
    const pairsPerMixer = this.channels / 2;
    batch(() => {
      strips.forEach((raw, entry) => {
        const id: StripId = entry === 0 ? "master" : entry - 1;
        if (id !== "master" && id >= this.channels) return;
        const values = raw as Record<string, unknown>;
        const pair = id === "master" || !Array.isArray(links) ? undefined : (links[this.index * pairsPerMixer + Math.floor(id / 2)] as Record<string, unknown> | undefined);
        const strip = this.#signal(id);
        strip.value = {
          level: Number(values["level"] ?? 0),
          pan: Number(values["pan"] ?? PAN_CENTRE),
          mute: Number(values["mute"] ?? 0) === 1,
          solo: Number(values["solo"] ?? 0) === 1,
          send: Number(values["send"] ?? 0),
          linked: pair === undefined ? strip.peek().linked : Number(pair["linked"] ?? 0) === 1,
        };
      });
      this.#known.value = true;
    });
    return true;
  }

  strip(id: StripId): ReadonlySignal<StripState> {
    return this.#signal(id);
  }

  /** The latest peak byte for a strip (dB below full scale), or undefined before a report. */
  meter(strip: number): ReadonlySignal<number | undefined> {
    this.#check(strip);
    let meter = this.#meters.get(strip);
    if (meter === undefined) {
      const peaks = this.#context.field("peaks_mixer");
      meter = computed(() => {
        const bytes = peaks.value;
        return bytes instanceof Uint8Array && strip < bytes.length ? bytes[strip] : undefined;
      });
      this.#meters.set(strip, meter);
    }
    return meter;
  }

  clipped(strip: number): ReadonlySignal<boolean> {
    this.#check(strip);
    return this.#clips[strip] as Signal<boolean>;
  }

  clearClip(strip: number): void {
    this.#check(strip);
    (this.#clips[strip] as Signal<boolean>).value = false;
  }

  /** Follows the meters (latching clips) and points the device's meters at this mixer. Returns a disposer. */
  activate(): () => void {
    const stopWatching = this.#context.watch();
    const peaks = this.#context.field("peaks_mixer");
    const stopClips = effect(() => {
      const bytes = peaks.value;
      if (!(bytes instanceof Uint8Array)) return;
      batch(() => {
        for (let i = 0; i < Math.min(bytes.length, this.channels); i++) if (bytes[i] === 0) (this.#clips[i] as Signal<boolean>).value = true;
      });
    });
    if (this.#context.family === "studio") {
      void this.#context.invoke("set_peak_source", { bank_id: 1, source_id: this.index }, {});
    } else if (this.meterSourceSelectable) {
      void this.#context.invoke("set_peak_source", { bank_id: 0, source_id: QUADRO_METER_SOURCES[this.index] as number }, {});
    }
    return () => {
      stopClips();
      stopWatching();
    };
  }

  setLevel(id: StripId, level: number): void {
    this.#update(id, { level: Math.min(LEVEL_MAX, Math.max(0, Math.round(level))) });
  }

  setPan(id: StripId, pan: number): void {
    this.#update(id, { pan: clampPan(pan) });
  }

  setSend(id: StripId, send: number): void {
    this.#update(id, { send: Math.min(SEND_MAX, Math.max(0, Math.round(send))) });
  }

  toggleMute(id: StripId): void {
    this.#update(id, { mute: !this.#signal(id).peek().mute });
  }

  toggleSolo(id: StripId): void {
    this.#update(id, { solo: !this.#signal(id).peek().solo });
  }

  /** Links or unlinks a strip with its stereo partner (strips 2k and 2k+1). */
  toggleLink(strip: number): void {
    this.#check(strip);
    const first = strip - (strip % 2);
    const linked = !(this.#strips[first] as Signal<StripState>).peek().linked;
    batch(() => {
      for (const s of [first, first + 1]) {
        const target = this.#strips[s];
        if (target !== undefined) target.value = { ...target.peek(), linked };
      }
    });
    void this.#context.invoke(
      "set_stereo_link",
      { periph_id: this.#context.topology.mixers.stereoLinkId, channel_id: Math.floor((first + this.index * this.channels) / 2), linked: linked ? 1 : 0 },
      { coalesce: `link:${this.deviceId}:${this.index}:${first}` },
    );
  }

  #check(strip: number): void {
    if (!Number.isInteger(strip) || strip < 0 || strip >= this.channels) throw new RangeError(`strip ${strip} is outside 0..${this.channels - 1}`);
  }

  #signal(id: StripId): Signal<StripState> {
    if (id === "master") return this.#master;
    this.#check(id);
    return this.#strips[id] as Signal<StripState>;
  }

  #update(id: StripId, change: Partial<StripState>): void {
    const targets: [StripId, Partial<StripState>][] = [[id, change]];
    if (id !== "master" && this.#signal(id).peek().linked) {
      const partner = id % 2 === 0 ? id + 1 : id - 1;
      // A linked partner follows level, mute and solo, but keeps its own pan (as the panel does).
      const { pan: _ownPan, ...mirrored } = change;
      if (partner < this.channels && Object.keys(mirrored).length > 0) targets.push([partner, mirrored]);
    }
    batch(() => {
      for (const [target, values] of targets) {
        const strip = this.#signal(target);
        strip.value = { ...strip.peek(), ...values };
      }
    });
    for (const [target] of targets) void this.#send(target);
  }

  #send(id: StripId): Promise<boolean> {
    const state = this.#signal(id).peek();
    const { mixers } = this.#context.topology;
    const channel = id === "master" ? mixers.masterChannel : id + 1;
    const args: Record<string, number> = { mixer_id: this.index, channel, level: state.level, pan: state.pan, mute: state.mute ? 1 : 0, solo: state.solo ? 1 : 0 };
    if (this.hasSend) args["send"] = state.send;
    return this.#context.invoke(mixers.command, args, { coalesce: `mixer:${this.deviceId}:${this.index}:${channel}` });
  }
}
