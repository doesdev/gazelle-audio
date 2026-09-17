// A device's effect chains and reverb (specs/2026-09-17-effects-and-reverb.md; reference/devices.md,
// "Effects (AFX) and reverb"), as both vendor panels read and bind them:
// - Chains are fed by routing (AFX IN k), up to eight slots each, a slot being {type, inst}: `type` an
//   AfxType id (0 is empty), `inst` that type's instance. The Quadro has six user chains, read one at a
//   time with get_afx_strip_order and the chain in ext3; the Studio+ sixteen, read with get_afx_order.
//   Link byte k pairs chains 2k and 2k+1. Chains are shown, not changed: order writes wait (spec Q4).
// - Bypass is per instance: set_afx_bypass(instance, type, enabled) with enabled 1 = processing. Only an
//   effect's own parameter read returns it, so it is unknown until that is read or this app sets it. A
//   linked chain's partner follows, as the panels mirror a link's changes onto the partner's own instances.
// - One reverb per device: set_reverb_config carries every field, mixer 0, and density always 100 (the
//   panels never pass it). The Quadro adds returns into mixes 1-2 (0 full .. 90 lowest, no scale shown)
//   and sends from mix 1's channels 1-16 (dB of attenuation, 96 = -inf, with a pan).
// - Parameters are per effect type: its own get and set (store/effect-parameters.ts, generated from both
//   panels). The Quadro reads one instance, named in `id`; the Studio+ reads every instance of the type at
//   once. The reply's `enabled` is the instance's bypass. A change resends every parameter with the type and
//   instance, as the panels do, and a linked chain's partner gets the same settings on its own instance at
//   the same slot. The Studio+ Equalizer is the exception: it is read in two parts (ext3 0 and 1, eight
//   instances each), both before any write, and set one band per command, coalesced per band.
// Reads are the page's own, so quiet (P63), and kept until forgotten (P80). A dry run reads nothing and
// counts as read, with the panels' starting values, so the controls still show what they would send.
// Writes that carry more than the value changed wait for a read: a default must not overwrite the
// device's reverb or an effect's settings.

import type { Topology } from "gazelle-audio-client";

import { batch, signal, type ReadonlySignal, type Signal } from "../core/signal.ts";
import { EFFECT_NAMES } from "./effect-catalogue.ts";
import { EFFECT_PARAMETERS, UNSUPPORTED_EFFECTS, type EffectDescription, type EffectParameter } from "./effect-parameters.ts";
import { clampPan, PAN_CENTRE } from "./mixer.ts";

/** Effect chains the app shows: the Quadro's AFX IN 1-6 (its AFX2DAW chains belong to the plugin), the Studio+'s 16. */
const CHAINS = { quadro: 6, studio: 16 } as const;
const SLOTS = 8;
/** The quietest reverb return on the Quadro's slider. */
export const REVERB_RETURN_MAX = 90;
/** Reverb send attenuation at which the send is off (-inf). */
export const REVERB_SEND_MAX = 96;
export const REVERB_LEVEL_MIN = 1;
export const REVERB_LEVEL_MAX = 100;
/** The reverb level the panels show as 0 dB. */
export const REVERB_LEVEL_UNITY = 25;
/** Quadro returns the panel drives, into mixes 1 and 2 (`MONITOR/HP 1`, `HEADPHONES 2`). */
const RETURNS = 2;
/** Quadro reverb sends: mix 1's channels 1..16. */
const SENDS = 16;
/** `set_reverb_config`'s density: in neither panel's UI, and always sent as its schema default. */
const DENSITY = 100;

/** The reverb level as the panels show it: 20·log10(v / 25) dB, whole. */
export function formatReverbLevel(level: number): string {
  const db = Math.round(20 * Math.log10(level / REVERB_LEVEL_UNITY));
  return db === 0 ? "0 dB" : `${db > 0 ? "+" : ""}${db} dB`;
}

/** The room size as the panels show it: three lengths, 42, 23 and 16.3 times (v + 10) / 100. */
export function formatRoomSize(value: number): string {
  const scale = (value + 10) / 100;
  return [42, 23, 16.3].map((length) => Math.round(length * scale)).join(" / ");
}

export interface EffectSlot {
  /** Where it sits in the chain, 0 first. */
  position: number;
  type: number;
  inst: number;
  name: string;
}

export interface EffectChain {
  index: number;
  name: string;
  /** False when nothing was read (a dry run): the chain shows empty but is not known to be. */
  known: boolean;
  /** The routing destination group that feeds it (AFX IN), if the topology has one. */
  destination: number | undefined;
  slots: EffectSlot[];
  linked: boolean;
  partner: number;
}

export interface ReverbConfig {
  known: boolean;
  roomSize: number;
  color: number;
  predelay: number;
  density: number;
  earlyRefGain: number;
  lateRefDelay: number;
  richness: number;
  reverbTime: number;
  level: number;
  on: boolean;
}

export interface ReverbReturn {
  level: number;
  mute: boolean;
}

export interface ReverbSend {
  level: number;
  pan: number;
}

/** The panels' starting values (`ReverbData.VALUES`), for a dry run. */
const REVERB_DEFAULT: ReverbConfig = { known: false, roomSize: 0, color: 0, predelay: 0, density: DENSITY, earlyRefGain: 0, lateRefDelay: 0, richness: 0, reverbTime: 0, level: REVERB_LEVEL_UNITY, on: true };

export interface EffectsContext {
  deviceId: string;
  family: "quadro" | "studio";
  topology: Topology;
  invoke(command: string, args: Record<string, unknown>, options: { coalesce?: string }): Promise<boolean>;
  read(command: string, ext3: number | undefined, quiet: boolean, args?: Record<string, unknown>): Promise<{ response: Record<string, unknown> | null; dryRun: boolean }>;
}

/** An effect instance's parameters by field name, in the values the editor shows (signed where the range is). */
export interface EffectParameterValues {
  /** False when nothing was read (a dry run): the values are the panel's starting values. */
  known: boolean;
  values: Readonly<Record<string, number>>;
}

const WIRE_BITS = { u8: 8, i8: 8, u16: 16, i16: 16, u32: 32, i32: 32 } as const;

/** The key of a band's parameter in an effect's values (the Studio+ Equalizer's `gain` of band 2 is `gain.2`). */
export function bandKey(name: string, band: number): string {
  return `${name}.${band}`;
}

/** A value as the device holds it: a negative value in an unsigned field is its two's complement, as the panel's ctypes field makes it. */
function toWire(parameter: EffectParameter, value: number): number {
  return value < 0 && parameter.wire.startsWith("u") ? value + 2 ** WIRE_BITS[parameter.wire] : value;
}

/** A reply's value as the editor shows it: an unsigned field whose range goes negative reads its byte back as negative. */
function fromWire(parameter: EffectParameter, raw: number): number {
  const bits = WIRE_BITS[parameter.wire];
  return parameter.min !== null && parameter.min < 0 && parameter.wire.startsWith("u") && raw >= 2 ** (bits - 1) ? raw - 2 ** bits : raw;
}

/** A parameter's value as the panel's code shows it: a menu's or bit's labels, On/Off, or the value (scaled, with its unit) where the code gives one. */
export function formatParameter(parameter: EffectParameter, value: number): string {
  switch (parameter.control) {
    case "menu":
      return parameter.options?.find(([v]) => v === value)?.[1] ?? String(value);
    case "bits": {
      const on = (parameter.options ?? []).filter(([bit]) => (value & bit) !== 0).map(([, text]) => text);
      return on.length === 0 ? "None" : on.join(" + ");
    }
    case "switch":
      return value === 0 ? "Off" : "On";
    default: {
      // The panel's frequency display: value / 1000 to one decimal above 1000, dropping ".0" (5k). Python's
      // format and toFixed agree on the float's exact value except at an exact half (11250 / 1000 is 11.25
      // exactly), which Python rounds to even (11.2k) and toFixed up.
      if (parameter.kilo === true && value > 1000) {
        const down = Math.floor(value / 100);
        const text = value % 500 === 250 ? ((down + (down % 2)) / 10).toFixed(1) : (value / 1000).toFixed(1);
        return `${text.replace(/[.]0$/, "")}k`;
      }
      const shown = parameter.scale === undefined ? value : value / parameter.scale;
      const text = parameter.decimals === undefined ? String(Math.round(shown * 1000) / 1000) : shown.toFixed(parameter.decimals);
      return parameter.unit === undefined ? text : `${text} ${parameter.unit}`;
    }
  }
}

/**
 * The parameters an effect shows controls for, given its values, in the set command's order. Where the
 * controls depend on a value (the Guitar Amp's model, whose panel has one view per model), only the chosen
 * layout's are shown, with what that layout makes of them (a switch's name and positions); a field no
 * layout of this value lists has no control and goes back as read. Hidden fields are never shown.
 */
export function shownParameters(description: EffectDescription, values: Readonly<Record<string, number>>): EffectParameter[] {
  const layouts = description.layouts;
  const chosen = layouts === undefined ? undefined : layouts.models.get(values[layouts.by] ?? description.parameters.find((p) => p.name === layouts.by)?.default ?? -1);
  return description.parameters.flatMap((parameter): EffectParameter[] => {
    if (layouts === undefined || !layouts.fields.includes(parameter.name)) return parameter.control === undefined ? [] : [parameter];
    const control = chosen?.find((c) => c.name === parameter.name);
    if (control === undefined) return [];
    const { hidden: _hidden, ...shown } = parameter;
    const merged: EffectParameter = { ...shown, ...control };
    return merged.control === undefined ? [] : [merged];
  });
}

/** An effect's starting values as the panel's code gives them, band by band where it has bands. */
function startingValues(description: EffectDescription): Record<string, number> {
  const values: Record<string, number> = Object.fromEntries(description.parameters.map((p) => [p.name, p.default ?? 0]));
  description.bands?.bands.forEach((band, b) => band.forEach((p) => (values[bandKey(p.name, b)] = p.default ?? 0)));
  return values;
}

/** One reply entry's values: each parameter, and each band's from the entry's list of bands. */
function readValues(description: EffectDescription, entry: Record<string, unknown>): Record<string, number> {
  const values: Record<string, number> = Object.fromEntries(description.parameters.map((p) => [p.name, fromWire(p, Number(entry[p.name] ?? p.default ?? 0))]));
  const bands = description.bands;
  if (bands !== undefined) {
    const list = entry[bands.list];
    bands.bands.forEach((band, b) => {
      const read = Array.isArray(list) ? (list[b] as Record<string, unknown> | undefined) : undefined;
      for (const p of band) values[bandKey(p.name, b)] = fromWire(p, Number(read?.[p.name] ?? p.default ?? 0));
    });
  }
  return values;
}

type Outcome = "read" | "dry" | "failed";

function entries(response: Record<string, unknown> | null): Record<string, unknown>[] | undefined {
  const list = response?.["entries"];
  return Array.isArray(list) ? (list as Record<string, unknown>[]) : undefined;
}

export class EffectsModel {
  readonly deviceId: string;
  readonly family: "quadro" | "studio";
  readonly #context: EffectsContext;
  readonly #chains = signal<readonly EffectChain[] | undefined>(undefined);
  readonly #reverb = signal<ReverbConfig | undefined>(undefined);
  readonly #returns = signal<{ known: boolean; entries: ReverbReturn[] } | undefined>(undefined);
  readonly #sends = signal<{ known: boolean; entries: ReverbSend[] } | undefined>(undefined);
  readonly #bypass = new Map<string, Signal<boolean | undefined>>();
  readonly #needsRead = signal(true);
  #generation = 0;
  #reading: number | undefined;
  readonly #parameters = new Map<string, Signal<EffectParameterValues | undefined>>();
  /** The generation each parameter read was made in, by read key (`type:inst` on the Quadro, `type` on the Studio+). */
  readonly #parametersRead = new Map<string, number>();
  readonly #parametersReading = new Map<string, Promise<boolean>>();

  constructor(context: EffectsContext) {
    this.#context = context;
    this.deviceId = context.deviceId;
    this.family = context.family;
  }

  /** Every chain the app shows, in index order; undefined until read. */
  get chains(): ReadonlySignal<readonly EffectChain[] | undefined> {
    return this.#chains;
  }

  get reverb(): ReadonlySignal<ReverbConfig | undefined> {
    return this.#reverb;
  }

  /** The Quadro's reverb returns into mixes 1 and 2; always undefined on the Studio+, whose return is the reverb level. */
  get returns(): ReadonlySignal<{ known: boolean; entries: ReverbReturn[] } | undefined> {
    return this.#returns;
  }

  /** The Quadro's reverb sends from mix 1's channels 1..16 (entry i is channel i + 1); the Studio+'s are the Mixer's sends. */
  get sends(): ReadonlySignal<{ known: boolean; entries: ReverbSend[] } | undefined> {
    return this.#sends;
  }

  get hasReturnsAndSends(): boolean {
    return this.family === "quadro";
  }

  /** True until read (or found to have nothing to read, in dry run), and again after `forget()`. */
  get needsRead(): ReadonlySignal<boolean> {
    return this.#needsRead;
  }

  /** Reads unless read since the last `forget()` or a read is under way. Resolves true when this call read. */
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

  /** Reads everything now. Resolves true when every read was answered. */
  async load(): Promise<boolean> {
    const generation = this.#generation;
    const outcome = await this.#load();
    if (outcome !== "failed" && generation === this.#generation) this.#needsRead.value = false;
    return outcome === "read";
  }

  /** The device may have changed without this app: read again next time. */
  forget(): void {
    this.#generation += 1;
    this.#needsRead.value = true;
  }

  async #load(): Promise<Outcome> {
    const { family, read } = this.#context;
    const count = CHAINS[family];
    const orders =
      family === "quadro"
        ? Promise.all(Array.from({ length: count }, (_, chain) => read("get_afx_strip_order", chain, true).then((r) => ({ ...r, slots: entries(r.response)?.[0]?.["slots"] }))))
        : read("get_afx_order", undefined, true).then((r) => Array.from({ length: count }, (_, chain) => ({ ...r, slots: entries(r.response)?.[chain]?.["slots"] })));
    const [chains, links, reverb, returns, sends] = await Promise.all([
      orders,
      read("get_afx_links", undefined, true),
      read("get_reverb_config", undefined, true),
      family === "quadro" ? read("get_reverb_returns", undefined, true) : undefined,
      family === "quadro" ? read("get_reverb_sends", undefined, true) : undefined,
    ]);
    const outcomes: Outcome[] = [...chains, links, reverb, ...(returns === undefined ? [] : [returns]), ...(sends === undefined ? [] : [sends])].map((r) => (r.dryRun ? "dry" : r.response === null ? "failed" : "read"));

    batch(() => {
      this.#applyChains(chains, links);
      if (reverb.dryRun) this.#reverb.value = { ...REVERB_DEFAULT };
      else if (reverb.response !== null) this.#reverb.value = this.#parseReverb(reverb.response);
      if (returns !== undefined) {
        const read = entries(returns.response);
        if (returns.dryRun) this.#returns.value = { known: false, entries: Array.from({ length: RETURNS }, () => ({ level: 0, mute: false })) };
        else if (read !== undefined) this.#returns.value = { known: true, entries: read.slice(0, RETURNS).map((e) => ({ level: Number(e["level"] ?? 0), mute: Number(e["mute"] ?? 0) === 1 })) };
      }
      if (sends !== undefined) {
        const read = entries(sends.response);
        if (sends.dryRun) this.#sends.value = { known: false, entries: Array.from({ length: SENDS }, () => ({ level: REVERB_SEND_MAX, pan: PAN_CENTRE })) };
        // Entry 0 is the master, which the panel skips: channel k is entry k.
        else if (read !== undefined) this.#sends.value = { known: true, entries: read.slice(1, SENDS + 1).map((e) => ({ level: Number(e["level"] ?? 0), pan: Number(e["pan"] ?? PAN_CENTRE) })) };
      }
    });
    return outcomes.includes("failed") ? "failed" : outcomes.every((o) => o === "dry") ? "dry" : "read";
  }

  #applyChains(chains: { response: Record<string, unknown> | null; dryRun: boolean; slots: unknown }[], links: { response: Record<string, unknown> | null; dryRun: boolean }): void {
    const linkBytes = entries(links.response);
    const previous = this.#chains.peek();
    const destination = this.#context.topology.outputs.findIndex((output) => output.type === "AFX_IN");
    const names = EFFECT_NAMES[this.family];
    const next = chains.map((read, index): EffectChain | undefined => {
      const partner = index % 2 === 0 ? index + 1 : index - 1;
      const linked = linkBytes === undefined ? (previous?.[index]?.linked ?? false) : Number(linkBytes[Math.floor(index / 2)]?.["linked"] ?? 0) === 1;
      const base = { index, name: `AFX IN ${index + 1}`, destination: destination < 0 ? undefined : destination, linked, partner };
      if (read.dryRun) return { ...base, known: false, slots: [] };
      if (!Array.isArray(read.slots)) return previous?.[index] === undefined ? undefined : { ...previous[index], linked };
      const slots = (read.slots as Record<string, unknown>[]).slice(0, SLOTS).flatMap((slot, position) => {
        const type = Number(slot["type"] ?? 0);
        const inst = Number(slot["inst"] ?? 0);
        return type === 0 ? [] : [{ position, type, inst, name: names.get(type) ?? `Effect ${type}` }];
      });
      return { ...base, known: true, slots };
    });
    if (next.every((chain) => chain === undefined)) return;
    this.#chains.value = next.map((chain, index) => chain ?? { index, name: `AFX IN ${index + 1}`, known: false, destination: destination < 0 ? undefined : destination, slots: [], linked: false, partner: index % 2 === 0 ? index + 1 : index - 1 });
  }

  #parseReverb(r: Record<string, unknown>): ReverbConfig {
    const n = (name: string, fallback = 0) => Number(r[name] ?? fallback);
    return {
      known: true,
      roomSize: n("room_size"),
      color: n("color"),
      predelay: n("predelay"),
      density: n("density", DENSITY),
      earlyRefGain: n("early_ref_gain"),
      lateRefDelay: n("late_ref_delay"),
      richness: n("richness"),
      reverbTime: n("reverb_time"),
      level: n("reverb_level", REVERB_LEVEL_UNITY),
      on: n("on") === 1,
    };
  }

  /** Whether an instance is bypassed, as far as this app knows: undefined until its parameters are read or this app sets it. */
  bypass(type: number, inst: number): ReadonlySignal<boolean | undefined> {
    return this.#bypassOf(type, inst);
  }

  #bypassOf(type: number, inst: number): Signal<boolean | undefined> {
    const key = `${type}:${inst}`;
    let state = this.#bypass.get(key);
    if (state === undefined) {
      state = signal<boolean | undefined>(undefined);
      this.#bypass.set(key, state);
    }
    return state;
  }

  /** Bypasses the effect in one slot of a chain, or lets it process; a linked partner's slot follows. */
  setBypass(chain: number, position: number, bypassed: boolean): void {
    const chains = this.#chains.peek();
    const own = this.#chain(chain);
    const slot = own.slots.find((s) => s.position === position);
    if (slot === undefined) throw new RangeError(`AFX IN ${chain + 1} has no effect in slot ${position + 1}`);
    this.#sendBypass(slot, bypassed);
    const partner = own.linked ? chains?.[own.partner] : undefined;
    const mirror = partner?.slots.find((s) => s.position === position && s.type === slot.type);
    if (mirror !== undefined) this.#sendBypass(mirror, bypassed);
  }

  /** Bypasses every effect in a chain, or lets them all process: one command each, as the panels' "BP ALL" does. */
  setChainBypass(chain: number, bypassed: boolean): void {
    for (const slot of this.#chain(chain).slots) this.setBypass(chain, slot.position, bypassed);
  }

  #chain(index: number): EffectChain {
    const chain = this.#chains.peek()?.[index];
    if (chain === undefined) throw new RangeError(`there is no read chain ${index} (chains 0..${CHAINS[this.family] - 1})`);
    return chain;
  }

  #sendBypass(slot: EffectSlot, bypassed: boolean): void {
    const state = this.#bypassOf(slot.type, slot.inst);
    const before = state.peek();
    state.value = bypassed;
    const enabled = bypassed ? 0 : 1;
    const args = this.family === "quadro" ? { periph_id: slot.inst, periph_type: slot.type, enabled } : { inst_id: slot.inst, type_id: slot.type, enabled };
    void this.#context.invoke("set_afx_bypass", args, { coalesce: `afx_bypass:${slot.type}:${slot.inst}:${this.deviceId}` }).then((sent) => {
      // Not sent: what was known before is all that is known.
      if (!sent && state.peek() === bypassed) state.value = before;
    });
  }

  /** Switches the reverb on or off. Refused (false) until the reverb has been read. */
  setReverbOn(on: boolean): boolean {
    return this.#changeReverb({ on });
  }

  /** Sets the reverb level, 1..100 (25 is 0 dB). Refused (false) until the reverb has been read. */
  setReverbLevel(level: number): boolean {
    return this.#changeReverb({ level: Math.min(REVERB_LEVEL_MAX, Math.max(REVERB_LEVEL_MIN, Math.round(level))) });
  }

  #changeReverb(change: Partial<ReverbConfig>): boolean {
    const current = this.#reverb.peek();
    if (current === undefined) return false;
    const next = { ...current, ...change };
    this.#reverb.value = next;
    const args = {
      mixer_id: 0,
      room_size: next.roomSize,
      color: next.color,
      predelay: next.predelay,
      density: DENSITY,
      early_ref_gain: next.earlyRefGain,
      late_ref_delay: next.lateRefDelay,
      richness: next.richness,
      reverb_time: next.reverbTime,
      reverb_level: next.level,
      on: next.on ? 1 : 0,
    };
    void this.#context.invoke("set_reverb_config", args, { coalesce: `reverb_config:${this.deviceId}` });
    return true;
  }

  /** Changes a Quadro reverb return (0 into mix 1, 1 into mix 2): level 0 (full) .. 90 (lowest), and mute. Refused until read. */
  setReturn(mix: number, change: Partial<ReverbReturn>): boolean {
    this.#checkQuadro("returns");
    if (!Number.isInteger(mix) || mix < 0 || mix >= RETURNS) throw new RangeError(`reverb return ${mix} is outside 0..${RETURNS - 1}`);
    const current = this.#returns.peek();
    if (current === undefined) return false;
    const entry = { ...(current.entries[mix] as ReverbReturn), ...change };
    entry.level = Math.min(REVERB_RETURN_MAX, Math.max(0, Math.round(entry.level)));
    this.#returns.value = { ...current, entries: current.entries.map((e, i) => (i === mix ? entry : e)) };
    void this.#context.invoke("set_reverb_return", { mixer_id: mix, level: entry.level, mute: entry.mute ? 1 : 0 }, { coalesce: `reverb_return:${mix}:${this.deviceId}` });
    return true;
  }

  /** Changes a Quadro reverb send from mix 1's channel 1..16: level (dB of attenuation, 96 = -inf) and pan travel together. Refused until read. */
  setSend(channel: number, change: Partial<ReverbSend>): boolean {
    this.#checkQuadro("sends");
    if (!Number.isInteger(channel) || channel < 1 || channel > SENDS) throw new RangeError(`reverb send channel ${channel} is outside 1..${SENDS}`);
    const current = this.#sends.peek();
    if (current === undefined) return false;
    const entry = { ...(current.entries[channel - 1] as ReverbSend), ...change };
    entry.level = Math.min(REVERB_SEND_MAX, Math.max(0, Math.round(entry.level)));
    entry.pan = clampPan(entry.pan);
    this.#sends.value = { ...current, entries: current.entries.map((e, i) => (i === channel - 1 ? entry : e)) };
    void this.#context.invoke("set_reverb_send", { mixer_id: 0, channel, level: entry.level, pan: entry.pan, mute: 0, solo: 0 }, { coalesce: `reverb_send:${channel}:${this.deviceId}` });
    return true;
  }

  /** What the editor knows of an effect type on this model; undefined when it is left out (see `unsupportedReason`). */
  description(type: number): EffectDescription | undefined {
    return EFFECT_PARAMETERS[this.family].get(type);
  }

  /** Why an effect type's parameters are not editable, when they are not. */
  unsupportedReason(type: number): string | undefined {
    if (this.description(type) !== undefined) return undefined;
    return UNSUPPORTED_EFFECTS[this.family].get(type) ?? "Its parameters were not found in the vendor panel's code.";
  }

  /** An instance's parameters; undefined until read. */
  parameters(type: number, inst: number): ReadonlySignal<EffectParameterValues | undefined> {
    return this.#parametersOf(type, inst);
  }

  #parametersOf(type: number, inst: number): Signal<EffectParameterValues | undefined> {
    const key = `${type}:${inst}`;
    let state = this.#parameters.get(key);
    if (state === undefined) {
      state = signal<EffectParameterValues | undefined>(undefined);
      this.#parameters.set(key, state);
    }
    return state;
  }

  /**
   * Reads an instance's parameters unless read since the last `forget()` (the Studio+'s read covers
   * every instance of the type). Resolves true when this call read. An effect left out is never read.
   */
  readParameters(type: number, inst: number): Promise<boolean> {
    const description = this.description(type);
    if (description === undefined) return Promise.resolve(false);
    const key = description.instanceParam === undefined ? `${type}` : `${type}:${inst}`;
    const generation = this.#generation;
    if (this.#parametersRead.get(key) === generation) return Promise.resolve(false);
    const reading = this.#parametersReading.get(key);
    if (reading !== undefined) return reading.then(() => false);
    const read = this.#readParameters(description, inst, key, generation).finally(() => this.#parametersReading.delete(key));
    this.#parametersReading.set(key, read);
    return read;
  }

  async #readParameters(description: EffectDescription, inst: number, key: string, generation: number): Promise<boolean> {
    const args = description.instanceParam === undefined ? undefined : { [description.instanceParam]: inst };
    // A read in parts names the part in ext3 (the Studio+ Equalizer); every part is read before any is taken.
    const parts = description.readParts;
    const reads = await Promise.all(Array.from({ length: parts ?? 1 }, (_, part) => this.#context.read(description.get, parts === undefined ? undefined : part, true, args)));
    const lists = reads.map((read) => entries(read.response));
    if (reads.some((read) => read.dryRun)) {
      const values = startingValues(description);
      batch(() => {
        const instances = description.instanceParam === undefined ? Array.from({ length: description.replyCount * (parts ?? 1) }, (_, i) => i) : [inst];
        for (const i of instances) this.#parametersOf(description.type, i).value = { known: false, values };
      });
    } else if (lists.every((list) => list !== undefined)) {
      batch(() => {
        lists.forEach((list, part) =>
          list?.forEach((entry, index) => {
            const instance = description.instanceParam === undefined ? part * description.replyCount + index : inst;
            this.#parametersOf(description.type, instance).value = { known: true, values: readValues(description, entry) };
            if (entry["enabled"] !== undefined) this.#bypassOf(description.type, instance).value = Number(entry["enabled"]) === 0;
          }),
        );
      });
    } else {
      return true;
    }
    if (generation === this.#generation) this.#parametersRead.set(key, generation);
    return true;
  }

  /**
   * Changes one parameter of the effect in a chain's slot, and resends all of them, as the panels do; a
   * linked chain's partner with the same effect at that slot gets the same settings on its own instance.
   * Refused (false) until the instance is read, or when the value is not one the control offers.
   */
  setParameter(chain: number, position: number, name: string, value: number, band?: number): boolean {
    const own = this.#chain(chain);
    const slot = own.slots.find((s) => s.position === position);
    if (slot === undefined) throw new RangeError(`AFX IN ${chain + 1} has no effect in slot ${position + 1}`);
    const description = this.description(slot.type);
    if (description === undefined) throw new RangeError(`${slot.name}'s parameters are not supported: ${this.unsupportedReason(slot.type)}`);
    if (description.bands !== undefined) return this.#setBandParameter(own, slot, description, name, value, band);
    const declared = description.parameters.find((p) => p.name === name);
    if (declared === undefined) throw new RangeError(`${slot.name} has no parameter ${name}`);
    const current = this.#parametersOf(slot.type, slot.inst).peek();
    const layouts = description.layouts;
    if (layouts?.fields.includes(name) !== true && declared.control === undefined) throw new RangeError(`${slot.name}'s ${name} is not a control (${declared.hidden ?? "hidden"})`);
    if (current === undefined) return false;
    const parameter = shownParameters(description, current.values).find((p) => p.name === name);
    if (parameter === undefined) {
      const by = description.parameters.find((p) => p.name === layouts?.by);
      const value = current.values[layouts?.by ?? ""];
      const chosen = by?.options?.find(([v]) => v === value)?.[1] ?? `${by?.label ?? "This setting"} ${value}`;
      throw new RangeError(`${chosen} does not use ${name}: it goes back as the device reported it`);
    }
    const next = this.#accept(parameter, value);
    if (next === undefined) return false;
    const values = { ...current.values, [name]: next };
    this.#sendParameters(description, slot.inst, { known: current.known, values });
    const partner = own.linked ? this.#chains.peek()?.[own.partner] : undefined;
    const mirror = partner?.slots.find((s) => s.position === position && s.type === slot.type);
    if (mirror !== undefined) this.#sendParameters(description, mirror.inst, { known: current.known, values });
    return true;
  }

  /** Sets a parameter (of one band, for an effect set band by band) back to the panel's starting value. */
  resetParameter(chain: number, position: number, name: string, band?: number): boolean {
    const slot = this.#chain(chain).slots.find((s) => s.position === position);
    const description = slot === undefined ? undefined : this.description(slot.type);
    const parameters = description?.bands === undefined ? description?.parameters : band === undefined ? undefined : description.bands.bands[band];
    const parameter = parameters?.find((p) => p.name === name);
    return this.setParameter(chain, position, name, parameter?.default ?? 0, band);
  }

  /**
   * One band's parameter of an effect set band by band (the Studio+ Equalizer): only that band is sent,
   * coalesced per band, and a linked partner's same effect gets it too. While a pass filter is chosen the
   * band's gain is 0 and refused, as the panel disables it; choosing one sends the gain as 0.
   */
  #setBandParameter(own: EffectChain, slot: EffectSlot, description: EffectDescription, name: string, value: number, band: number | undefined): boolean {
    const bands = description.bands?.bands ?? [];
    if (band === undefined || !Number.isInteger(band) || band < 0 || band >= bands.length) throw new RangeError(`${slot.name} is set one band at a time: name a band 0..${bands.length - 1}`);
    const parameters = bands[band] ?? [];
    const parameter = parameters.find((p) => p.name === name);
    if (parameter === undefined) throw new RangeError(`${slot.name} has no parameter ${name}`);
    if (parameter.control === undefined) throw new RangeError(`${slot.name}'s band ${band + 1} ${name} is not a control (${parameter.hidden ?? "hidden"})`);
    const current = this.#parametersOf(slot.type, slot.inst).peek();
    if (current === undefined) return false;
    const off = parameter.offWhen;
    if (off !== undefined && off.values.includes(current.values[bandKey(off.name, band)] ?? Number.NaN)) return false;
    const next = this.#accept(parameter, value);
    if (next === undefined) return false;
    const values: Record<string, number> = { ...current.values, [bandKey(name, band)]: next };
    for (const other of parameters) {
      if (other.offWhen?.name === name && other.offWhen.values.includes(next)) values[bandKey(other.name, band)] = 0;
    }
    this.#sendBand(description, slot.inst, band, { known: current.known, values });
    const partner = own.linked ? this.#chains.peek()?.[own.partner] : undefined;
    const mirror = partner?.slots.find((s) => s.position === slot.position && s.type === slot.type);
    if (mirror !== undefined) this.#sendBand(description, mirror.inst, band, { known: current.known, values });
    return true;
  }

  #sendBand(description: EffectDescription, inst: number, band: number, next: EffectParameterValues): void {
    const bands = description.bands;
    if (bands === undefined) return;
    const state = this.#parametersOf(description.type, inst);
    const before = state.peek();
    state.value = next;
    const args: Record<string, number> = { type_id: description.type, inst_id: inst, [bands.param]: band };
    for (const parameter of bands.bands[band] ?? []) args[parameter.name] = toWire(parameter, next.values[bandKey(parameter.name, band)] ?? parameter.default ?? 0);
    void this.#context.invoke(description.set, args, { coalesce: `afx_params:${description.type}:${inst}:${band}:${this.deviceId}` }).then((sent) => {
      // Not sent: what was known before is all that is known.
      if (!sent && state.peek() === next) state.value = before;
    });
  }

  /** The value a control takes for `value`: clamped and whole for a range, one of a menu's values, only a mask's bits. */
  #accept(parameter: EffectParameter, value: number): number | undefined {
    if (!Number.isFinite(value)) return undefined;
    switch (parameter.control) {
      case "menu":
        return parameter.options?.some(([v]) => v === value) ? value : undefined;
      case "bits":
        return Math.round(value) & (parameter.options ?? []).reduce((mask, [bit]) => mask | bit, 0);
      default:
        return Math.min(parameter.max ?? value, Math.max(parameter.min ?? value, Math.round(value)));
    }
  }

  #sendParameters(description: EffectDescription, inst: number, next: EffectParameterValues): void {
    const state = this.#parametersOf(description.type, inst);
    const before = state.peek();
    state.value = next;
    const args: Record<string, number> = { type_id: description.type, inst_id: inst };
    for (const parameter of description.parameters) args[parameter.name] = toWire(parameter, next.values[parameter.name] ?? parameter.default ?? 0);
    void this.#context.invoke(description.set, args, { coalesce: `afx_params:${description.type}:${inst}:${this.deviceId}` }).then((sent) => {
      // Not sent: what was known before is all that is known.
      if (!sent && state.peek() === next) state.value = before;
    });
  }

  #checkQuadro(what: string): void {
    if (this.family !== "quadro") throw new Error(`the Studio+ has no reverb ${what}: its sends are the Mixer's, its return the reverb level`);
  }
}
