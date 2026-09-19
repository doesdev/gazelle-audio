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
import { MIC_EMULATIONS, MIC_LICENCE_BITS, MIC_PATTERNS, MIC_TARGETS, type PatternRange } from "./mic-emulations.ts";
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

/**
 * A preamp's mic emulation: which Antelope microphone is on it and which microphone it is made to
 * sound like. `pattern` is the stereo pattern preset, which this does not set (a pair-wide feature
 * of the dual-capsule microphones) but which every change must carry back unchanged.
 */
export interface MicEmulationState {
  /** A value of `MIC_TARGETS`; 0 is no Antelope microphone. */
  target: number;
  /** An index into that target's `emulationModels`. */
  model: number;
  swap: boolean;
  pattern: number;
}

/** The microphones a preamp can have on it, in `MicTarget` order. */
const MIC_TARGET_NAMES: readonly string[] = ["None", "Edge Duo", "Verge", "Edge Solo", "Edge Quadro", "Accord", "Edge Note"];

/**
 * `get_feature_mask`'s licence bits start at its reply's second byte: the panel's
 * `parse_feature_mask` drops the first before reading `_feature_mask_format` from bit 0.
 */
const LICENCE_MASK_OFFSET = 1;

/** The last licence bit any microphone has, so a reply too short to carry it is no licence at all. */
const LICENCE_LAST_BIT = Math.max(...Object.values(MIC_LICENCE_BITS).flat());

/** The stereo techniques two heads can be set to, in `MicPatternPreset` order. */
const PATTERN_PRESET_NAMES: readonly string[] = ["None", "XY", "M/S", "Blumlein"];

/** A polar angle of +1 is omni, 0 cardioid and -1 figure-8; the positions between have no name. */
const PATTERN_LANDMARKS: readonly (readonly [angle: number, name: string])[] = [
  [1, "Omni"],
  [0, "Cardioid"],
  [-1, "Figure-8"],
];

/** Rounds away the error of stepping through a range, so a landmark is recognised as one. */
const near = (a: number, b: number) => Math.abs(a - b) < 1e-6;

/** A head's polar pattern: where it is, what it can be, and what that means. */
export interface PatternState {
  /** The `pattern` byte itself. */
  value: number;
  min: number;
  max: number;
  /** +1 omni, 0 cardioid, -1 figure-8. */
  angle: number;
  label: string;
  /** Each position this model offers, or undefined when the range is too long to list. */
  steps: readonly { value: number; label: string }[] | undefined;
}

/** The longest range still worth showing as a list of positions rather than a sweep. */
const PATTERN_STEP_LIMIT = 8;

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
  /**
   * Reads a command's reply; `response` is null in dry run or when it failed. The store reports a
   * failure, unless `quiet`: a read the page makes on its own leaves the value unknown instead.
   */
  read(command: string, ext3?: number, quiet?: boolean): Promise<{ response: Record<string, unknown> | null; dryRun: boolean }>;
  /** The other members of this input's workspace link, if it is in one (LinksModel). */
  peers(kind: InputLinkKind, index: number): readonly { model: InputsModel; index: number; mode: "absolute" | "relative" }[];
  /**
   * Links or unlinks these preamps in the workspace, absolute. A microphone that covers more than
   * one preamp is still one microphone, so its channels belong together (the user, 2026-09-16).
   */
  linkPreamps(channels: readonly number[], on: boolean): void;
  field(name: string): ReadonlySignal<unknown>;
  watch(): () => void;
  timers: InputsTimers;
}

const signed = (byte: number) => (byte > 127 ? byte - 256 : byte);

/** The panel's own conversion (`MicModelBaseWithPAngle.pattern_to_pangle`). */
function angleOf(range: PatternRange, pattern: number): number {
  const span = range.max - range.min;
  return span === 0 ? range.minAngle : range.minAngle + (pattern / span) * (range.maxAngle - range.minAngle);
}

/** Its inverse (`pangle_to_pattern`), truncated as the panel truncates it. */
function patternForAngle(range: PatternRange, angle: number): number {
  const span = range.maxAngle - range.minAngle;
  const relative = span === 0 ? 0 : ((angle - range.minAngle) / span) * (range.max - range.min);
  return Math.min(range.max, Math.max(range.min, Math.trunc(range.min + relative)));
}

function patternLabel(angle: number): string {
  const landmark = PATTERN_LANDMARKS.find(([at]) => near(at, angle));
  // The positions between the three named patterns have no name, so they show as the angle itself.
  return landmark === undefined ? angle.toFixed(2) : landmark[1];
}
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
  /** Quadro only: its panel is the one with the mic emulation feature. */
  readonly hasMicEmulation: boolean;
  readonly #emulations: Signal<MicEmulationState>[];
  /** The licence bits of `get_feature_mask`, or null while they are unknown. */
  readonly #licence = signal<Uint8Array | null>(null);
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
    this.hasMicEmulation = context.family === "quadro";
    this.#emulations = this.hasMicEmulation ? Array.from({ length: this.preampCount }, () => signal<MicEmulationState>({ target: 0, model: 0, swap: false, pattern: 0 })) : [];
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

  /**
   * The microphones a preamp can have on it. Quadro only: the Studio+ panel has no mic emulation.
   */
  get micTargets(): readonly { value: number; name: string; licensed: boolean }[] {
    return MIC_TARGET_NAMES.map((name, value) => ({ value, name, licensed: this.emulationLicensed(value, 0) }));
  }

  /**
   * What a target can be made to sound like; empty when no microphone is on the preamp. Every
   * emulation is listed, licensed or not, since the index is the device's (`emulationLicensed`).
   */
  emulationModels(target: number): readonly string[] {
    return MIC_EMULATIONS[target] ?? [];
  }

  /**
   * Whether the device's licence covers an emulation; model 0, the microphone itself, stands for
   * the microphone. True while the licence is unknown, since a read that failed must not hide what the
   * device may well have, and for "None", which needs nothing. Follows `loadLicence`.
   */
  emulationLicensed(target: number, model: number): boolean {
    const mask = this.#licence.value;
    if (mask === null) return true;
    // "None" has no licence bit, and needs none.
    const bit = MIC_LICENCE_BITS[target]?.[model];
    if (bit === undefined) return true;
    return (((mask[bit >> 3] ?? 0) >> (bit & 7)) & 1) === 1;
  }

  /**
   * Reads which microphones and emulations the device is licensed for (`get_feature_mask`, the
   * bitmap the device reports). Resolves false when nothing usable
   * was read, and then everything stays offered. The Inputs page reads it on its own, so quietly.
   */
  async loadLicence(): Promise<boolean> {
    if (!this.hasMicEmulation) return false;
    const payload = (await this.#context.read("get_feature_mask", undefined, true)).response?.["payload"];
    const usable = payload instanceof Uint8Array && payload.length > LICENCE_MASK_OFFSET + (LICENCE_LAST_BIT >> 3);
    this.#licence.value = usable ? payload.slice(LICENCE_MASK_OFFSET) : null;
    return usable;
  }

  /** A preamp's emulation, as last read or set. Read once with `loadEmulations`. */
  emulation(index: number): ReadonlySignal<MicEmulationState> {
    return this.#emulation(index);
  }

  /**
   * Reads every preamp's emulation (`get_mic_emulations`). Resolves false when nothing was read,
   * which includes a model without the feature. The schema declares the reply as two entries; the
   * panel reads one per preamp, so this takes as many as it is given, up to the preamp count.
   */
  async loadEmulations(): Promise<boolean> {
    if (!this.hasMicEmulation) return false;
    // The Inputs page reads this every time it opens, so a refusal is not the user's to hear about.
    const entries = (await this.#context.read("get_mic_emulations", undefined, true)).response?.["entries"];
    if (!Array.isArray(entries)) return false;
    batch(() => {
      entries.slice(0, this.preampCount).forEach((entry, index) => {
        const fields = entry as Record<string, unknown>;
        this.#emulation(index).value = {
          target: Number(fields["target"] ?? 0),
          model: Number(fields["emu_model"] ?? 0),
          swap: Number(fields["ch_swap"] ?? 0) === 1,
          pattern: Number(fields["pattern"] ?? 0),
        };
      });
    });
    return true;
  }

  /**
   * How many preamps a microphone occupies. The Edge Duo is one capsule with two membranes on two
   * XLRs, and the Edge Quadro is two such heads on four, which is the only way its emulations work
   * (the Edge manual, and the panel's own `_regroup_link`: a link group of 4, 2, or 1).
   */
  emulationSpan(target: number): number {
    if (target === MIC_TARGETS.EDGE_QUADRO) return 4;
    if (target === MIC_TARGETS.EDGE_DUO) return 2;
    return 1;
  }

  /**
   * The preamps a microphone picked on `index` covers. A microphone sits in a block aligned to its
   * own size, as the panel's `get_quad_buddy_pre` does; one that would not fit is refused.
   */
  emulationChannels(index: number, target: number): readonly number[] {
    this.#emulation(index);
    const span = this.emulationSpan(target);
    const first = Math.floor(index / span) * span;
    if (first + span > this.preampCount) throw new RangeError(`a ${MIC_TARGET_NAMES[target] ?? target} needs ${span} preamps, and this device has ${this.preampCount}`);
    return Array.from({ length: span }, (_, k) => first + k);
  }

  /**
   * The heads of a microphone at `index`, each with the first of its two membranes. The Edge Quadro
   * has two, and each takes its own emulation (`get_quad_models` returns the pair); everything else
   * has one. Channels 3 and 4 are the Top head (`is_quad_top`: `ch % 4 >= 2`).
   */
  emulationHeads(index: number, target: number): readonly { name: string; channel: number }[] {
    const channels = this.emulationChannels(index, target);
    if (channels.length < 4) return [{ name: "", channel: channels[0] as number }];
    return [
      { name: "Bottom", channel: channels[0] as number },
      { name: "Top", channel: channels[2] as number },
    ];
  }

  /**
   * Says which Antelope microphone is on a preamp, over all the preamps it covers. Its catalogue is
   * its own, so the model resets; a microphone on more than one preamp is linked, and one that is
   * replaced by a single-membrane microphone takes its link away with it.
   */
  setEmulationTarget(index: number, target: number): void {
    if (!Number.isInteger(target) || target < 0 || target >= MIC_TARGET_NAMES.length) throw new RangeError(`no microphone ${target}: 0..${MIC_TARGET_NAMES.length - 1}`);
    // One the device already reports stays as it is; the licence only stops picking it.
    if (!this.emulationLicensed(target, 0)) throw new Error(`the ${MIC_TARGET_NAMES[target]} is not licensed on this device`);
    const previous = this.#group(index);
    const channels = this.emulationChannels(index, target);
    // The microphone that was there is gone from every preamp it was on, so the new one replaces it
    // across all of them: swapping an Edge Duo for an Edge Solo leaves no half of a Duo behind.
    const replaced = [...new Set([...previous, ...channels])].sort((a, b) => a - b);
    this.#apply(replaced, { target, model: 0, swap: false });
    if (channels.length > 1) this.#context.linkPreamps(channels, true);
    else this.#context.linkPreamps(previous.length > 1 ? previous : channels, false);
  }

  /**
   * Picks what a head is made to sound like, by index into its target's catalogue. It reaches that
   * head's membranes and no further: the Edge Quadro's two heads take an emulation each.
   */
  setEmulationModel(index: number, model: number): void {
    const current = this.#emulation(index).peek();
    const models = this.emulationModels(current.target);
    if (!Number.isInteger(model) || model < 0 || model >= models.length) throw new RangeError(`no emulation ${model} for this microphone: 0..${models.length - 1}`);
    if (!this.emulationLicensed(current.target, model)) throw new Error(`${models[model]} is not licensed on this device`);
    this.#apply(this.#head(index), { model });
  }

  /** Swaps a microphone's front and rear membranes. It is one microphone, so it covers all of it. */
  setEmulationSwap(index: number, on: boolean): void {
    this.#apply(this.#group(index), { swap: on });
  }

  /**
   * A head's polar pattern, or undefined for a microphone whose pattern means nothing. Only the
   * Edge Duo, Edge Quadro and Accord models carry one (`MicModelBaseWithPAngle`).
   */
  emulationPattern(index: number): PatternState | undefined {
    const range = this.#patternRange(index);
    if (range === undefined) return undefined;
    const value = Math.min(range.max, Math.max(range.min, this.#emulation(index).peek().pattern));
    const steps =
      range.max - range.min > PATTERN_STEP_LIMIT
        ? undefined
        : Array.from({ length: range.max - range.min + 1 }, (_, k) => ({ value: range.min + k, label: patternLabel(angleOf(range, range.min + k)) }));
    return { value, min: range.min, max: range.max, angle: angleOf(range, value), label: patternLabel(angleOf(range, value)), steps };
  }

  /** Points a head's capsule, from omni through cardioid to figure-8 as far as its model allows. */
  setEmulationPattern(index: number, pattern: number): void {
    const range = this.#patternRange(index);
    if (range === undefined) throw new Error("this microphone has no polar pattern");
    if (!Number.isInteger(pattern) || pattern < range.min || pattern > range.max) throw new RangeError(`no polar pattern ${pattern}: ${range.min}..${range.max}`);
    this.#apply(this.#head(index), { pattern });
  }

  /**
   * The stereo techniques a microphone's two heads can be set to. Empty for a microphone with one
   * head, and a technique is offered only where both heads' models reach the patterns it needs
   * (the panel's `xy_supported`, `ms_supported` and `bl_supported`).
   */
  emulationPresets(index: number): readonly { value: number; name: string; available: boolean }[] {
    const heads = this.#heads(index);
    if (heads.length < 2) return [];
    const [a, b] = heads.map((head) => this.#patternRange(head));
    const cardioid = (range: PatternRange | undefined) => range !== undefined && range.maxAngle <= 0 && 0 <= range.minAngle;
    const figure8 = (range: PatternRange | undefined) => range !== undefined && near(range.maxAngle, -1);
    const available = [true, cardioid(a) && cardioid(b), (cardioid(a) && figure8(b)) || (figure8(a) && cardioid(b)), figure8(a) && figure8(b)];
    return PATTERN_PRESET_NAMES.map((name, value) => ({ value, name, available: available[value] === true }));
  }

  /** Which technique the heads are at now, or 0 when they are at none of them. */
  emulationPreset(index: number): number {
    const heads = this.#heads(index);
    if (heads.length < 2) return 0;
    const angles = heads.map((head) => this.emulationPattern(head)?.angle);
    const at = (angle: number | undefined, wanted: number) => angle !== undefined && near(angle, wanted);
    if (angles.every((angle) => at(angle, 0))) return 1;
    if (angles.every((angle) => at(angle, -1))) return 3;
    if (angles.some((angle) => at(angle, 0)) && angles.some((angle) => at(angle, -1))) return 2;
    return 0;
  }

  /**
   * Sets both heads for a stereo technique. XY is both capsules at cardioid and Blumlein both at
   * figure-8; M/S is the top head at cardioid over the bottom at figure-8 (the Edge manual). The
   * 90-degree offset each technique also wants is the user turning the head, not a command.
   */
  setEmulationPreset(index: number, preset: number): void {
    const chosen = this.emulationPresets(index)[preset];
    if (chosen === undefined) throw new RangeError(`no stereo preset ${preset} for this microphone`);
    if (!chosen.available) throw new Error(`these microphones cannot be set to ${chosen.name}`);
    if (preset === 0) return;
    const wanted: Readonly<Record<number, readonly [bottom: number, top: number]>> = { 1: [0, 0], 2: [-1, 0], 3: [-1, -1] };
    const angles = wanted[preset] as readonly [number, number];
    // One batch for the whole microphone: both heads move together, so what is watching it sees
    // one change rather than a half-applied technique.
    batch(() => {
      this.#heads(index).forEach((head, which) => {
        const range = this.#patternRange(head);
        if (range === undefined) return;
        this.#apply(this.#head(head), { pattern: patternForAngle(range, angles[which] as number) });
      });
    });
  }

  /** The first membrane of each of a microphone's heads, as `emulationHeads` names them. */
  #heads(index: number): readonly number[] {
    return this.emulationHeads(index, this.#emulation(index).peek().target).map((head) => head.channel);
  }

  #patternRange(index: number): PatternRange | undefined {
    const current = this.#emulation(index).peek();
    return MIC_PATTERNS[current.target]?.[current.model];
  }

  /**
   * The preamps the microphone at `index` is actually on: its block, less any channel carrying a
   * different microphone. A Duo with one XLR unplugged is a single-membrane microphone (the Edge
   * manual), so its neighbour is free for something else and must not be written over.
   */
  #group(index: number): readonly number[] {
    const target = this.#emulation(index).peek().target;
    return this.emulationChannels(index, target).filter((channel) => channel === index || this.#emulation(channel).peek().target === target);
  }

  /** The membranes of one head: a capsule pair of the same microphone, else the preamp alone. */
  #head(index: number): readonly number[] {
    const group = this.#group(index);
    if (group.length < 2) return [index];
    const first = Math.floor(index / 2) * 2;
    return group.filter((channel) => channel === first || channel === first + 1);
  }

  #apply(channels: readonly number[], change: Partial<MicEmulationState>): void {
    const next = channels.map((channel) => ({ channel, state: { ...this.#emulation(channel).peek(), ...change } }));
    batch(() => {
      for (const { channel, state } of next) this.#emulation(channel).value = state;
    });
    for (const { channel, state } of next) {
      // All five travel together, and `pattern` goes back as it was read: this does not set it.
      this.#send("set_mic_emulation", { preamp_ch: channel, target: state.target, emu_model: state.model, ch_swap: state.swap ? 1 : 0, pattern: state.pattern }, `mic_emu:${channel}`);
    }
  }

  #emulation(index: number): Signal<MicEmulationState> {
    if (!this.hasMicEmulation) throw new Error(`the ${this.family === "quadro" ? "Quadro" : "Studio+"} has no mic emulation`);
    const held = this.#emulations[index];
    if (held === undefined) throw new RangeError(`preamp ${index} is outside 0..${this.preampCount - 1}`);
    return held;
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
