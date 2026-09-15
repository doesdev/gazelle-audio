// A device's hardware inputs: preamps (type, gain, 48V, phase invert, and the HPF the device
// reports) and digital input gains. State comes from the cyclic 0x73 report; changes send the
// vendor panels' commands. Scales and rules are the panels' (bytecode research, 2026-09):
// - type 0 Mic, 1 Line, 2 Hi-Z; Hi-Z exists on Quadro preamps 1-2 and Studio+ preamps 1-4;
// - gain is whole dB, its range set by the type: Mic 0..65, Line -6..+20, Hi-Z 0..40;
// - 48V applies to Mic only: the Quadro panel reads a reported 48V as off on any other type;
// - HPF has no command in either panel, so it is shown, not set;
// - digital gains are -6..+12 dB; only the Studio+ panel sets them (line, ADAT, S/PDIF).
// A local change outranks the device's reports for half a second, as the Quadro panel does, so a
// report still carrying the old value does not undo a change that is on its way.

import { batch, computed, signal, type ReadonlySignal, type Signal } from "../core/signal.ts";
import type { Topology } from "gazelle-audio-client";

export type PreampType = 0 | 1 | 2;

export const PREAMP_TYPES: readonly { value: PreampType; label: string }[] = [
  { value: 0, label: "Mic" },
  { value: 1, label: "Line" },
  { value: 2, label: "Hi-Z" },
];

export const GAIN_RANGE: Readonly<Record<PreampType, { min: number; max: number }>> = {
  0: { min: 0, max: 65 },
  1: { min: -6, max: 20 },
  2: { min: 0, max: 40 },
};

export const DIGITAL_GAIN = { min: -6, max: 12 } as const;

/** How long a local change outranks the device's reports (the Quadro panel's 0.5 s). */
export const ECHO_HOLD_MS = 500;

const HIZ_PREAMPS = { quadro: 2, studio: 4 } as const;

export interface PreampState {
  /** False until the device has reported its preamps; values are defaults until then. */
  known: boolean;
  type: number;
  gain: number;
  phantom: boolean;
  phaseInvert: boolean;
  hpf: boolean;
}

export type DigitalKind = "line" | "adat" | "spdif";

/** The input kinds a workspace link can join. */
export type InputLinkKind = "preamp" | DigitalKind;

export interface DigitalGroup {
  kind: DigitalKind;
  label: string;
  count: number;
  /** Whether this device's panel sets these gains. */
  editable: boolean;
  /** Stereo pairs this device's panel links (pair k is inputs 2k and 2k+1); 0 when it links none. */
  linkPairs: number;
}

/** Studio+ `set_stereo_link` peripheral ids, which are also its links reads' ext3 (research 2026-09). */
const DIGITAL_LINK_PERIPH: Readonly<Record<DigitalKind, number>> = { line: 1, adat: 2, spdif: 3 };
const DIGITAL_LINKS_READ: Readonly<Record<DigitalKind, string>> = { line: "get_lines_links", adat: "get_adats_links", spdif: "get_spdifs_links" };

interface InputsTimers {
  setTimeout(callback: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

/** `set_stereo_link` peripheral id for preamps, and `get_preamps_links`' ext3, in both families. */
const PREAMP_LINK_PERIPH = 0;

export interface InputsContext {
  deviceId: string;
  family: "quadro" | "studio";
  topology: Topology;
  invoke(command: string, args: Record<string, number>, options: { coalesce?: string }): Promise<boolean>;
  /** Reads a command's reply; `response` is null in dry run or when it failed (which the store reports). */
  read(command: string, ext3?: number): Promise<{ response: Record<string, unknown> | null; dryRun: boolean }>;
  /** The other members of this input's workspace link, if it is in one (LinksModel, decision P51). */
  peers(kind: InputLinkKind, index: number): readonly { model: InputsModel; index: number; mode: "absolute" | "relative" }[];
  field(name: string): ReadonlySignal<unknown>;
  watch(): () => void;
  timers: InputsTimers;
}

const signed = (byte: number) => (byte > 127 ? byte - 256 : byte);
const clamp = (value: number, range: { min: number; max: number }) => Math.min(range.max, Math.max(range.min, Math.round(value)));

export class InputsModel {
  readonly deviceId: string;
  readonly family: "quadro" | "studio";
  readonly preampCount: number;
  /** Preamps 0..hizCount-1 have a Hi-Z input. */
  readonly hizCount: number;
  readonly digital: readonly DigitalGroup[];
  /** Preamp stereo pairs: pair k is preamps 2k and 2k+1. */
  readonly pairCount: number;
  readonly #links: Signal<boolean>[];
  readonly #digitalLinks = new Map<DigitalKind, Signal<boolean>[]>();
  readonly #context: InputsContext;
  readonly #preamps: ReadonlySignal<PreampState>[];
  readonly #digital = new Map<string, ReadonlySignal<number | undefined>>();
  readonly #holds = new Map<string, { value: Signal<number | undefined>; timer: unknown }>();

  constructor(context: InputsContext) {
    this.#context = context;
    this.deviceId = context.deviceId;
    this.family = context.family;
    const channels = (type: string) => context.topology.inputs.find((g) => g.type === type)?.channels ?? 0;
    this.preampCount = channels("PREAMP");
    this.hizCount = Math.min(HIZ_PREAMPS[context.family], this.preampCount);
    this.pairCount = Math.floor(this.preampCount / 2);
    this.#links = Array.from({ length: this.pairCount }, () => signal(false));
    // Only the Studio+ panel sets digital gains and links digital pairs; the Quadro's only shows its gains.
    const editable = context.family === "studio";
    const group = (kind: DigitalKind, label: string, type: string): DigitalGroup => {
      const count = channels(type);
      return { kind, label, count, editable, linkPairs: editable ? Math.floor(count / 2) : 0 };
    };
    this.digital = [...(context.family === "studio" ? [group("line", "Line in", "LINE_IN")] : []), group("adat", "ADAT in", "ADAT_IN"), group("spdif", "S/PDIF in", "SPDIF_IN")];
    for (const d of this.digital) this.#digitalLinks.set(d.kind, Array.from({ length: d.linkPairs }, () => signal(false)));

    const preamps = context.field("preamps");
    const gains = context.field("preamp_gains");
    const typeField = context.family === "quadro" ? "type" : "pretype";
    this.#preamps = Array.from({ length: this.preampCount }, (_, i) =>
      computed(() => {
        const list = preamps.value;
        const bytes = gains.value;
        const entry = Array.isArray(list) ? (list[i] as Record<string, unknown> | undefined) : undefined;
        const reportedGain = bytes instanceof Uint8Array && i < bytes.length ? signed(bytes[i] as number) : undefined;
        const type = this.#hold(`pre:${i}:type`).value ?? Number(entry?.[typeField] ?? 0);
        const phantom = this.#hold(`pre:${i}:phantom`).value ?? Number(entry?.["phantom"] ?? 0);
        return {
          known: entry !== undefined,
          type,
          gain: this.#hold(`pre:${i}:gain`).value ?? reportedGain ?? 0,
          phantom: type === 0 && phantom === 1,
          phaseInvert: (this.#hold(`pre:${i}:phase`).value ?? Number(entry?.["phase_inv"] ?? 0)) === 1,
          hpf: Number(entry?.["hpf"] ?? 0) === 1,
        };
      }, samePreamp),
    );
  }

  /** Follows the device's reports. Returns a disposer. */
  activate(): () => void {
    return this.#context.watch();
  }

  preamp(index: number): ReadonlySignal<PreampState> {
    this.#checkPreamp(index);
    return this.#preamps[index] as ReadonlySignal<PreampState>;
  }

  /** Whether a preamp pair is stereo-linked (as last read or set). */
  pairLinked(pair: number): ReadonlySignal<boolean> {
    return this.#pair(pair);
  }

  /**
   * Reads which preamp pairs are linked. The Quadro's bundled format reads back only its first
   * pair; later pairs keep what was last set. Resolves false when nothing was read.
   */
  async loadLinks(): Promise<boolean> {
    const reads: [string, Signal<boolean>[]][] = [["get_preamps_links", this.#links], ...this.digital.filter((d) => d.linkPairs > 0).map((d): [string, Signal<boolean>[]] => [DIGITAL_LINKS_READ[d.kind], this.#digitalLinks.get(d.kind) ?? []])];
    let read = false;
    for (const [command, pairs] of reads) {
      const entries = (await this.#context.read(command, undefined)).response?.["entries"];
      if (!Array.isArray(entries)) continue;
      read = true;
      batch(() => {
        entries.slice(0, pairs.length).forEach((entry, pair) => {
          (pairs[pair] as Signal<boolean>).value = Number((entry as Record<string, unknown>)["linked"] ?? 0) === 1;
        });
      });
    }
    return read;
  }

  /** Whether a digital input pair is stereo-linked (as last read or set). */
  digitalPairLinked(kind: DigitalKind, pair: number): ReadonlySignal<boolean> {
    return this.#digitalPair(kind, pair);
  }

  /** Sets a digital pair's device link flag. Which inputs change together is the workspace's (LinksModel). */
  setDigitalPairLinked(kind: DigitalKind, pair: number, on: boolean): void {
    this.#digitalPair(kind, pair).value = on;
    this.#send("set_stereo_link", { periph_id: DIGITAL_LINK_PERIPH[kind], channel_id: pair, linked: on ? 1 : 0 }, `${kind}_link:${pair}`);
  }

  #digitalPair(kind: DigitalKind, pair: number): Signal<boolean> {
    const group = this.digital.find((d) => d.kind === kind);
    if (group === undefined || group.linkPairs === 0) throw new Error(`the ${this.family} panel does not link ${group?.label ?? kind} pairs, so neither does this page`);
    const link = this.#digitalLinks.get(kind)?.[pair];
    if (link === undefined) throw new RangeError(`${group.label} pair ${pair} is outside 0..${group.linkPairs - 1}`);
    return link;
  }

  /** Sets a preamp pair's device link flag. Which inputs change together is the workspace's (LinksModel). */
  setPairLinked(pair: number, on: boolean): void {
    this.#pair(pair).value = on;
    this.#send("set_stereo_link", { periph_id: PREAMP_LINK_PERIPH, channel_id: pair, linked: on ? 1 : 0 }, `pre_link:${pair}`);
  }

  // Each setter changes this input and then, unless `follow` is false, the other members of its
  // workspace link (on any device): the same value, or for relative links the same step from each
  // member's own value, within that member's range. Both vendor panels send a linked input's changes
  // to its partner the same way (Quadro bytecode; Studio+ in hardware session 1).

  /** Sets a preamp's type, and its link's members'; when any of them has no Hi-Z, none changes. */
  setType(index: number, type: PreampType, follow = true): void {
    this.#checkPreamp(index);
    if (!(type in GAIN_RANGE)) throw new RangeError(`no preamp type ${type}`);
    const peers = follow ? this.#context.peers("preamp", index) : [];
    for (const target of [{ model: this as InputsModel, index }, ...peers]) {
      if (type === 2 && target.index >= target.model.hizCount) {
        throw new RangeError(`preamp ${target.index + 1}${target.model === this ? "" : ` on ${target.model.deviceId}`} has no Hi-Z input (only 1..${target.model.hizCount})`);
      }
    }
    this.#change(`pre:${index}:type`, type, "preamps");
    this.#send("set_pre_type", { id: index, pretype: type }, `pre_type:${index}`);
    for (const peer of peers) peer.model.setType(peer.index, type, false);
  }

  setGain(index: number, gain: number, follow = true): void {
    const state = this.preamp(index).peek();
    const value = clamp(gain, GAIN_RANGE[state.type as PreampType] ?? GAIN_RANGE[0]);
    this.#change(`pre:${index}:gain`, value, "preamp_gains");
    this.#send("set_pre_gain", { id: index, gain: value }, `pre_gain:${index}`);
    if (!follow) return;
    for (const peer of this.#context.peers("preamp", index)) {
      peer.model.setGain(peer.index, peer.mode === "relative" ? peer.model.preamp(peer.index).peek().gain + (value - state.gain) : value, false);
    }
  }

  /** Turns 48V on or off; returns false, sending nothing, when turning it on outside Mic. Members not on Mic are left off. */
  setPhantom(index: number, on: boolean, follow = true): boolean {
    if (on && this.preamp(index).peek().type !== 0) return false;
    this.#change(`pre:${index}:phantom`, on ? 1 : 0, "preamps");
    this.#send("set_pre_phantom", { id: index, phantom: on ? 1 : 0 }, `pre_phantom:${index}`);
    if (follow) for (const peer of this.#context.peers("preamp", index)) peer.model.setPhantom(peer.index, on, false);
    return true;
  }

  setPhaseInvert(index: number, on: boolean, follow = true): void {
    this.#checkPreamp(index);
    this.#change(`pre:${index}:phase`, on ? 1 : 0, "preamps");
    this.#send(this.family === "quadro" ? "set_pre_phase_inv" : "set_pre_phaseinv", { id: index, phase_inv: on ? 1 : 0 }, `pre_phase:${index}`);
    if (follow) for (const peer of this.#context.peers("preamp", index)) peer.model.setPhaseInvert(peer.index, on, false);
  }

  #pair(pair: number): Signal<boolean> {
    const link = this.#links[pair];
    if (link === undefined) throw new RangeError(`preamp pair ${pair} is outside 0..${this.pairCount - 1}`);
    return link;
  }

  /** A digital input's gain in dB, or undefined until reported. */
  digitalGain(kind: DigitalKind, index: number): ReadonlySignal<number | undefined> {
    this.#checkDigital(kind, index);
    const key = `dig:${kind}:${index}`;
    let gain = this.#digital.get(key);
    if (gain === undefined) {
      const bytes = this.#context.field(`${kind}_gains`);
      gain = computed(() => {
        const held = this.#hold(key).value;
        if (held !== undefined) return held;
        const value = bytes.value;
        return value instanceof Uint8Array && index < value.length ? signed(value[index] as number) : undefined;
      });
      this.#digital.set(key, gain);
    }
    return gain;
  }

  setDigitalGain(kind: DigitalKind, index: number, gain: number, follow = true): void {
    const group = this.#checkDigital(kind, index);
    if (!group.editable) throw new Error(`the ${this.family} panel does not set ${group.label} gain, so neither does this page`);
    const before = this.digitalGain(kind, index).peek() ?? 0;
    const value = clamp(gain, DIGITAL_GAIN);
    this.#change(`dig:${kind}:${index}`, value, `${kind}_gains`);
    this.#send(`set_${kind}_gain`, { id: index, gain: value }, `${kind}_gain:${index}`);
    if (!follow) return;
    for (const peer of this.#context.peers(kind, index)) {
      peer.model.setDigitalGain(kind, peer.index, peer.mode === "relative" ? (peer.model.digitalGain(kind, peer.index).peek() ?? 0) + (value - before) : value, false);
    }
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

  #checkPreamp(index: number): void {
    if (!Number.isInteger(index) || index < 0 || index >= this.preampCount) throw new RangeError(`preamp ${index} is outside 0..${this.preampCount - 1}`);
  }

  #checkDigital(kind: DigitalKind, index: number): DigitalGroup {
    const group = this.digital.find((d) => d.kind === kind);
    if (group === undefined) throw new RangeError(`the ${this.family} has no ${kind} inputs`);
    if (!Number.isInteger(index) || index < 0 || index >= group.count) throw new RangeError(`${group.label} ${index} is outside 0..${group.count - 1}`);
    return group;
  }
}

function samePreamp(a: PreampState, b: PreampState): boolean {
  return a.known === b.known && a.type === b.type && a.gain === b.gain && a.phantom === b.phantom && a.phaseInvert === b.phaseInvert && a.hpf === b.hpf;
}
