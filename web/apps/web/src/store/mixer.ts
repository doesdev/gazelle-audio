// One device mixer (spec §10 row 4). Values and commands follow what the vendor panels do
// (reference/devices.md, "Mixer and meter value scales"; decisions P26–P30):
// - level is dB of attenuation, 0..90 on a linear fader, shown 0 dB … −90 dB;
// - pan is 2..62 with 32 as centre (27..38 snaps to it), shown −30…+30;
// - Studio+ `send` is dB of attenuation, 0 (loudest) to 95, where 95 reads -inf on the panel: one
//   step below set_volume's 96 (hardware sessions 1 and 2);
// - a meter byte is dB below full scale on Antelope's piecewise scale, and 0 latches clip;
// - device channel 0 is the master and strip i is device channel i + 1 (assumed for Studio+).
// Every strip command carries the whole strip, so coalescing per strip never loses a field.
// `load()` reads the mixer's state: get_mixer with the mixer in ext3 (33 entries, master first)
// and its 16 pairs of get_mixer_links (entry k covers strips 2k and 2k+1). Until then, and in dry
// run, values are defaults. `readOnce()` is what a page calls: the state read is kept and reused
// (decision P80), since this app's own commands keep it current, until `forget()` says the device
// may have changed without it (the connection dropped, or the device went away).

import type { Topology } from "gazelle-audio-client";

import { batch, computed, effect, signal, type ReadonlySignal, type Signal } from "../core/signal.ts";

export const LEVEL_MAX = 90;
export const PAN_MIN = 2;
export const PAN_MAX = 62;
export const PAN_CENTRE = 32;
const PAN_SNAP = [27, 38] as const;
/** Send attenuation at which the send is off (−inf). */
export const SEND_MAX = 95;
/** Scale marks in dB below full scale. */
export const METER_MARKS = [0, 5, 10, 15, 20, 30, 40, 60] as const;

export function levelDb(level: number): number {
  return level === 0 ? 0 : -level;
}

/**
 * The fader's taper (the user, 2026-09-16): an audio taper, so most of the travel sits near 0 dB
 * where mixing happens. A power curve spaced the scale's 10 dB marks almost evenly, which read as
 * linear; the fader now follows the meters' own scale (`meterDeflection`, Antelope's) down to -60 dB
 * in the top nine tenths of its travel, so each 10 dB down takes less room than the one above
 * (22, 22, 18, 13, 9 and 4 per cent), and -60 to -90 dB shares the last tenth.
 */
const FADER_TAIL = 0.1;
const METER_RANGE = 60;

/** Where a level sits on the fader, as a fraction of its travel from the top (0 dB) to the bottom. */
export function faderPosition(level: number): number {
  const clamped = Math.min(LEVEL_MAX, Math.max(0, level));
  const height = clamped <= METER_RANGE ? FADER_TAIL + ((1 - FADER_TAIL) * meterDeflection(clamped)) / 100 : (FADER_TAIL * (LEVEL_MAX - clamped)) / (LEVEL_MAX - METER_RANGE);
  return 1 - height;
}

/** The level at a fraction of the fader's travel from the top: `faderPosition`'s inverse. */
export function levelAtFaderPosition(position: number): number {
  const height = 1 - Math.min(1, Math.max(0, position));
  if (height < FADER_TAIL) return LEVEL_MAX - (height / FADER_TAIL) * (LEVEL_MAX - METER_RANGE);
  return meterLevelAt(((height - FADER_TAIL) / (1 - FADER_TAIL)) * 100);
}

/** `meterDeflection`'s inverse: the level (dB below full scale) a bar reaching `deflection` (0..100) shows. */
function meterLevelAt(deflection: number): number {
  if (deflection >= 50) return 20 - (deflection - 50) / 2.5;
  if (deflection >= 30) return 30 - (deflection - 30) / 2;
  if (deflection >= 15) return 40 - (deflection - 15) / 1.5;
  if (deflection >= 5) return 50 - (deflection - 5);
  return 60 - deflection * 2;
}

export function levelFromDb(db: number): number {
  return Math.min(LEVEL_MAX, Math.max(0, Math.round(-db)));
}

export function formatLevel(level: number): string {
  return `${levelDb(level)} dB`;
}

export function formatSend(send: number): string {
  return send >= SEND_MAX ? "-inf" : send === 0 ? "0 dB" : `-${send} dB`;
}

/** A pan held to the byte's range, every step of it: the wheel and keys move one step (3%) at a time. */
export function clampPan(value: number): number {
  return Math.min(PAN_MAX, Math.max(PAN_MIN, Math.round(value)));
}

/**
 * The pan at a fraction of a pan bar's width, for a pointer. Dragging keeps the panel knob's centre
 * detent, 27..38 landing on centre, so centre is easy to find by hand; the wheel and keys skip it
 * and step through every value (the user, 2026-09-17).
 */
export function panAtPosition(fraction: number): number {
  const pan = clampPan(PAN_MIN + fraction * (PAN_MAX - PAN_MIN));
  return pan >= PAN_SNAP[0] && pan <= PAN_SNAP[1] ? PAN_CENTRE : pan;
}

/**
 * A pan as a side and how far towards it (the user, 2026-09-17): "L 100%", "C", "R 27%". The byte's
 * steps either side of centre are a share of full left or right, so signed numbers read as dB.
 */
export function formatPan(pan: number): string {
  const offset = pan - PAN_CENTRE;
  if (offset === 0) return "C";
  const share = Math.round((Math.abs(offset) / (PAN_MAX - PAN_CENTRE)) * 100);
  return `${offset < 0 ? "L" : "R"} ${share}%`;
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
  /** The other members of a strip's workspace link, as their strips in this mix on their devices (LinksModel, P51). */
  peers(strip: number): readonly { model: MixerModel; strip: number; mode: "absolute" | "relative" }[];
  /** While this mix is mono (decision P57): the pans to restore, by strip. Reading it is reactive. */
  monoPans(): Readonly<Record<string, number>> | undefined;
  /** Saves a pan to restore when mono ends, instead of sending it. */
  rememberPan(strip: number, pan: number): void;
}

const DEFAULT_STRIP: StripState = { level: 0, pan: PAN_CENTRE, mute: false, solo: false, send: 0, linked: false };

export class MixerModel {
  readonly deviceId: string;
  readonly index: number;
  readonly channels: number;
  /** Studio+ strips carry a send byte in every mix; Quadro's do not. */
  readonly hasSend: boolean;
  /**
   * Whether strips show the send: the Studio+'s send byte is its reverb send, which the vendor panel
   * shows on Mix 1 only (effects spec Q8, the user's answer). Every mix still sends the byte it holds.
   */
  readonly hasReverbSend: boolean;
  readonly #known = signal(false);
  readonly #needsRead = signal(true);
  /** Bumped by `forget()`, so a read begun before it neither counts nor holds back the next. */
  #generation = 0;
  #reading: number | undefined;
  readonly #context: MixerContext;
  readonly #strips: Signal<StripState>[];
  readonly #master = signal<StripState>(DEFAULT_STRIP);

  constructor(context: MixerContext) {
    this.#context = context;
    this.deviceId = context.deviceId;
    this.index = context.index;
    this.channels = context.topology.mixers.channels;
    this.hasSend = context.topology.mixers.command === "set_mixer_cfg";
    this.hasReverbSend = this.hasSend && this.index === 0;
    this.#strips = Array.from({ length: this.channels }, () => signal(DEFAULT_STRIP));
  }

  /** True once the device's state has been read; until then values are defaults. */
  get stateKnown(): ReadonlySignal<boolean> {
    return this.#known;
  }

  /** True until the state has been read (or found to have nothing to read, in dry run), and again after `forget()`. */
  get needsRead(): ReadonlySignal<boolean> {
    return this.#needsRead;
  }

  /**
   * Reads the mixer's state unless it has been read since the last `forget()`, or a read is under
   * way. Resolves true when this call read. A read that fails leaves it to be tried next time.
   */
  async readOnce(): Promise<boolean> {
    const generation = this.#generation;
    if (!this.#needsRead.peek() || this.#reading === generation) return false;
    this.#reading = generation;
    try {
      const outcome = await this.#load();
      if (outcome !== "failed" && generation === this.#generation) this.#needsRead.value = false;
    } finally {
      if (this.#reading === generation) this.#reading = undefined;
    }
    return true;
  }

  /** The device may have changed without this app (it reconnected or was re-attached): read again next time. */
  forget(): void {
    this.#generation += 1;
    this.#needsRead.value = true;
  }

  /** Reads the mixer's strips and links from the device. Resolves false when nothing was read (dry run, or a failure the store reports). */
  async load(): Promise<boolean> {
    return (await this.#load()) === "read";
  }

  async #load(): Promise<"read" | "dry run" | "failed"> {
    const reply = await this.#context.read("get_mixer", this.index);
    const strips = reply.response?.["entries"];
    if (!Array.isArray(strips)) return reply.dryRun ? "dry run" : "failed";
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
    return "read";
  }

  strip(id: StripId): ReadonlySignal<StripState> {
    return this.#signal(id);
  }

  /**
   * Follows the device's report while a page shows this mixer. Returns a disposer. It points no
   * meter bank: strips meter their inputs (the store's `inputMeter`), because the Quadro keeps its
   * mixer meters on Mix 1's inputs whatever `set_peak_source` asks (hardware, 2026-09-16).
   */
  activate(): () => void {
    return this.#context.watch();
  }

  setLevel(id: StripId, level: number): void {
    this.#update(id, { level: Math.min(LEVEL_MAX, Math.max(0, Math.round(level))) });
  }

  /** Sets a strip's pan; while the mix is mono it is saved for when mono ends, not sent. */
  setPan(id: StripId, pan: number): void {
    if (id !== "master" && this.#context.monoPans() !== undefined) {
      this.#check(id);
      this.#context.rememberPan(id, clampPan(pan));
      return;
    }
    this.#update(id, { pan: clampPan(pan) });
  }

  /** Sends a pan whether or not the mix is mono: how mono centres and restores its channels. */
  sendPan(strip: number, pan: number): void {
    this.#update(strip, { pan: clampPan(pan) });
  }

  /** The pan a strip returns to when its mono mix ends; undefined when the mix is not mono. */
  monoPan(strip: number): number | undefined {
    this.#check(strip);
    return this.#context.monoPans()?.[String(strip)];
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

  /** Sets the device link flag of strips `first` and `first + 1`. Which strips change together is the workspace's (LinksModel). */
  setPairLinked(first: number, linked: boolean): void {
    this.#check(first);
    if (first % 2 !== 0) throw new RangeError(`strip pairs start at even strips, not ${first}`);
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

  #update(id: StripId, change: Partial<StripState>, follow = true): void {
    const strip = this.#signal(id);
    const before = strip.peek();
    strip.value = { ...before, ...change };
    void this.#send(id);
    if (!follow || id === "master") return;
    // Other members of the strip's link follow level, mute and solo in this mix, but keep their own
    // pan and send (as the panels treat a linked pair); relative links move levels by the same step.
    const followed: Partial<StripState> = {};
    for (const key of ["level", "mute", "solo"] as const) if (key in change) Object.assign(followed, { [key]: change[key] });
    if (Object.keys(followed).length === 0) return;
    const after = strip.peek();
    for (const peer of this.#context.peers(id)) {
      const values = { ...followed };
      if (values.level !== undefined && peer.mode === "relative") values.level = Math.min(LEVEL_MAX, Math.max(0, peer.model.strip(peer.strip).peek().level + (after.level - before.level)));
      peer.model.#update(peer.strip, values, false);
    }
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
