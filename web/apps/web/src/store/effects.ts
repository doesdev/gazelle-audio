// A device's effect chains and reverb (specs/2026-09-17-effects-and-reverb.md; reference/devices.md,
// "Effects (AFX) and reverb"), as both vendor panels read and bind them:
// - Chains are fed by routing (AFX IN k), up to eight slots each, a slot being {type, inst}: `type` an
//   AfxType id (0 is empty), `inst` that type's instance. The Quadro has six user chains, read one at a
//   time with get_afx_strip_order and the chain in ext3; the Studio+ sixteen, read with get_afx_order.
//   Link byte k pairs chains 2k and 2k+1.
// - A chain is changed by writing all eight slots with set_afx_order, the effects packed from the first
//   slot and the rest empty, which is the shape the hardware probe confirmed (P114): it inserts, removes
//   and, untried on a device, reorders. The device sets the instance's `enabled` to 1 on insert and 0 on
//   removal by itself, so no bypass goes with either. A linked chain's partner gets the same change on
//   its own instances. Which instances are free comes from get_afx_available_instances, on the Quadro
//   with get_afx_remaining_featured_instances for the licence; the Studio+'s reply has no length in the
//   schema, so its free instances are what its sixteen chains leave.
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
import { catalogue, loadCatalogue, type EffectDescription, type EffectParameter } from "./effect-parameters.ts";
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
/** Instances of an effect type the firmware has (`MAX_INSTANCE_COUNT`). */
const MAX_INSTANCES = 16;
/** Types with a smaller pool than the rest: the Studio+'s Guitar Amp and Guitar Cabinet have four. */
const FEWER_INSTANCES: Readonly<Record<"quadro" | "studio", ReadonlyMap<number, number>>> = {
  quadro: new Map<number, number>(),
  studio: new Map<number, number>([[3, 4], [4, 4]]),
};

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

/** An effect type as the "Add effect" control offers it for one chain. */
export interface EffectOffer {
  type: number;
  name: string;
  /** Instances of it free to use. */
  free: number;
  /** True when the device counted them; false when they are only what the chains this app has read leave. */
  counted: boolean;
  /** Why it cannot be added to this chain now, if it cannot. */
  unavailable?: string;
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
  /** Follows the device's effect-meter report until the returned function is called. */
  watchMeters(): () => void;
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

/** One chain's `slots` as it came back, with what became of the read. */
interface OrderRead {
  index: number;
  dryRun: boolean;
  failed: boolean;
  slots: unknown;
}

/** A chain as `set_afx_order` carries it: eight `{type, inst}` slots, the effects first and the rest empty. */
function chainBytes(slots: readonly EffectSlot[]): Uint8Array {
  const bytes = new Uint8Array(SLOTS * 2);
  slots.slice(0, SLOTS).forEach((slot, position) => {
    bytes[position * 2] = slot.type;
    bytes[position * 2 + 1] = slot.inst;
  });
  return bytes;
}

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
  readonly #instances = signal<{ counted: boolean; free: ReadonlyMap<number, number> } | undefined>(undefined);
  readonly #bypass = new Map<string, Signal<boolean | undefined>>();
  readonly #needsRead = signal(true);
  #generation = 0;
  #reading: number | undefined;
  /** The generation the chains were last read in, and the one a chains-only read is running for. */
  #chainsRead: number | undefined;
  #readingChains: number | undefined;
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

  /**
   * Reads the chains alone, unless they have been read since the last `forget()`. Resolves true when
   * this call read.
   *
   * The Mixer needs to know what each chain holds to meter a strip fed by AFX OUT, and its last
   * effect is as far as the device meters the chain. That is a fraction of what the Effects page
   * reads, so it asks for the orders and the links only, and leaves `needsRead` set for the page's
   * own read. Quiet, like every read a page makes of its own accord (P63).
   */
  /**
   * Follows the effect meters and makes sure the chains are read, for a page that shows them.
   * Returns a disposer.
   *
   * The report arrives about 125 times a second, so it is followed only while a page showing it is
   * open, and which effect each of its bytes belongs to is knowable only from the chains.
   */
  activate(): () => void {
    void this.readChainsOnce();
    return this.#context.watchMeters();
  }

  async readChainsOnce(): Promise<boolean> {
    const generation = this.#generation;
    if (this.#chainsRead === generation || this.#readingChains === generation) return false;
    this.#readingChains = generation;
    try {
      const { family, read } = this.#context;
      const [chains, links] = await Promise.all([
        this.#readOrders(Array.from({ length: CHAINS[family] }, (_, chain) => chain)),
        read("get_afx_links", undefined, true),
      ]);
      if (generation !== this.#generation) return true;
      batch(() => this.#applyChains(chains, links));
      if (!chains.some((chain) => chain.failed)) this.#chainsRead = generation;
    } finally {
      if (this.#readingChains === generation) this.#readingChains = undefined;
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
    const [chains, links, reverb, returns, sends] = await Promise.all([
      this.#readOrders(Array.from({ length: count }, (_, chain) => chain)),
      read("get_afx_links", undefined, true),
      read("get_reverb_config", undefined, true),
      family === "quadro" ? read("get_reverb_returns", undefined, true) : undefined,
      family === "quadro" ? read("get_reverb_sends", undefined, true) : undefined,
      // The free instance counts are an extra: a refusal leaves them unknown rather than failing the page's read.
      this.#readInstances(),
    ]);
    const outcomes: Outcome[] = [
      ...chains.map((r) => (r.dryRun ? "dry" : r.failed ? "failed" : "read") as Outcome),
      ...[links, reverb, ...(returns === undefined ? [] : [returns]), ...(sends === undefined ? [] : [sends])].map((r): Outcome => (r.dryRun ? "dry" : r.response === null ? "failed" : "read")),
    ];

    // The page's read covers the chains too, so a later `readChainsOnce` has nothing to do.
    if (!chains.some((chain) => chain.failed)) this.#chainsRead = this.#generation;
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

  /** One chain each, as each model's panel reads them: the Quadro one at a time with the chain in `ext3`. */
  async #readOrders(indexes: readonly number[]): Promise<OrderRead[]> {
    const { family, read } = this.#context;
    if (family === "quadro") {
      return Promise.all(
        indexes.map(async (index) => {
          const r = await read("get_afx_strip_order", index, true);
          return { index, dryRun: r.dryRun, failed: !r.dryRun && r.response === null, slots: entries(r.response)?.[0]?.["slots"] };
        }),
      );
    }
    const r = await read("get_afx_order", undefined, true);
    return indexes.map((index) => ({ index, dryRun: r.dryRun, failed: !r.dryRun && r.response === null, slots: entries(r.response)?.[index]?.["slots"] }));
  }

  /**
   * How many instances of each type are free, for the "Add effect" control. Only the Quadro is asked: the
   * Studio+'s reply has no length in our schema (`unresolved_reply_counts`), so it would decode as one
   * record, and its free instances are worked out from its chains instead. The Quadro's count is the lower
   * of the free instances and the licence's remaining featured instances, as its panel does.
   */
  async #readInstances(): Promise<void> {
    if (this.family !== "quadro") return;
    const { read } = this.#context;
    const [available, featured] = await Promise.all([read("get_afx_available_instances", undefined, true), read("get_afx_remaining_featured_instances", undefined, true)]);
    const counts = (response: Record<string, unknown> | null): Map<number, number> | undefined => {
      const list = entries(response);
      return list === undefined ? undefined : new Map(list.map((entry) => [Number(entry["type_id"] ?? 0), Number(entry["inst_count"] ?? 0)]));
    };
    const free = available.dryRun ? undefined : counts(available.response);
    if (free === undefined) return;
    // What a negative featured count means is unknown, so it is taken as none left rather than guessed at.
    const licence = featured.dryRun ? undefined : counts(featured.response);
    this.#instances.value = { counted: true, free: new Map([...free].map(([type, count]) => [type, licence === undefined ? count : Math.min(count, Math.max(0, licence.get(type) ?? count))])) };
  }

  /** Takes what was read into the chains: a chain not read this time keeps what was known, links and all. */
  #applyChains(chains: readonly OrderRead[], links?: { response: Record<string, unknown> | null; dryRun: boolean }): void {
    const linkBytes = links === undefined ? undefined : entries(links.response);
    const previous = this.#chains.peek();
    const destination = this.#context.topology.outputs.findIndex((output) => output.type === "AFX_IN");
    const names = EFFECT_NAMES[this.family];
    const read = new Map(chains.map((chain) => [chain.index, chain]));
    const base = (index: number) => ({ index, name: `AFX IN ${index + 1}`, destination: destination < 0 ? undefined : destination, partner: index % 2 === 0 ? index + 1 : index - 1 });
    const next = Array.from({ length: CHAINS[this.family] }, (_, index): EffectChain | undefined => {
      const before = previous?.[index];
      const linked = linkBytes === undefined ? (before?.linked ?? false) : Number(linkBytes[Math.floor(index / 2)]?.["linked"] ?? 0) === 1;
      const chain = read.get(index);
      if (chain === undefined) return before === undefined ? undefined : { ...before, linked };
      if (chain.dryRun) return { ...base(index), known: false, slots: [], linked };
      if (!Array.isArray(chain.slots)) return before === undefined ? undefined : { ...before, linked };
      const slots = (chain.slots as Record<string, unknown>[]).slice(0, SLOTS).flatMap((slot, position) => {
        const type = Number(slot["type"] ?? 0);
        const inst = Number(slot["inst"] ?? 0);
        return type === 0 ? [] : [{ position, type, inst, name: names.get(type) ?? `Effect ${type}` }];
      });
      return { ...base(index), known: true, slots, linked };
    });
    if (next.every((chain) => chain === undefined)) return;
    this.#chains.value = next.map((chain, index) => chain ?? { ...base(index), known: false, slots: [], linked: false });
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

  /**
   * The effect types this model has, for one chain's "Add effect" control: how many instances of each are
   * free, whether the device counted them, and why one cannot be added to this chain now. In name order.
   */
  offers(index: number): EffectOffer[] {
    const chains = this.#chains.value;
    const chain = chains?.[index];
    const partner = chain?.linked === true ? chains?.[chain.partner] : undefined;
    const needed = partner === undefined ? 1 : 2;
    const counts = this.#instances.value;
    const used = this.#used();
    return [...EFFECT_NAMES[this.family]]
      .map(([type, name]): EffectOffer => {
        const counted = counts?.free.get(type);
        const free = counted ?? Math.max(0, this.#maxInstances(type) - (used.get(type)?.size ?? 0));
        const unavailable =
          chain === undefined || !chain.known
            ? "The chains have not been read from the device."
            : chain.slots.length >= SLOTS
              ? "This chain has all eight slots filled."
              : partner !== undefined && (!partner.known || partner.slots.length >= SLOTS)
                ? `AFX IN ${chain.partner + 1}, which this chain is linked with, has no free slot.`
                : free < needed || this.#freeInstances(type, needed).length < needed
                  ? needed === 2
                    ? "A linked pair needs two free instances of this effect."
                    : "No free instance of this effect is left."
                  : undefined;
        return { type, name, free, counted: counted !== undefined, ...(unavailable === undefined ? {} : { unavailable }) };
      })
      .sort((a, b) => a.name.localeCompare(b.name));
  }

  /**
   * Adds an effect of `type` to a chain: the lowest free instance, after what is already there, written as
   * the whole chain. A linked chain's partner gets one of its own at the same place. Refused (false) when
   * the chain is unread or full or there is no free instance, as `offers` says.
   */
  addEffect(index: number, type: number): boolean {
    const offer = this.offers(index).find((o) => o.type === type);
    if (offer === undefined) throw new RangeError(`this model has no effect of type ${type}`);
    if (offer.unavailable !== undefined) return false;
    const chain = this.#chain(index);
    const partner = chain.linked ? this.#chains.peek()?.[chain.partner] : undefined;
    const needed = partner === undefined ? 1 : 2;
    const free = this.#freeInstances(type, needed);
    if (free.length < needed) return false;
    const name = EFFECT_NAMES[this.family].get(type) ?? `Effect ${type}`;
    const slot = (position: number, inst: number): EffectSlot => ({ position, type, inst, name });
    const writes: [number, EffectSlot[]][] = [[index, [...chain.slots, slot(chain.slots.length, free[0] as number)]]];
    if (partner !== undefined) writes.push([partner.index, [...partner.slots, slot(partner.slots.length, free[1] as number)]]);
    for (const inst of free) {
      // The device sets an inserted instance's `enabled` to 1 itself (P114), so no bypass goes with it.
      this.#bypassOf(type, inst).value = false;
      // Its settings survive, but the device may have changed them: read them again when it is opened.
      this.#forgetParameters(type, inst);
    }
    this.#writeChains(writes);
    return true;
  }

  /**
   * Removes the effect in one slot: the chain goes out without it, what followed moved up and the trailing
   * slots empty. A linked partner's same effect in the same slot goes too.
   */
  removeEffect(index: number, position: number): boolean {
    const chain = this.#chain(index);
    const slot = chain.slots.find((s) => s.position === position);
    if (slot === undefined) throw new RangeError(`AFX IN ${index + 1} has no effect in slot ${position + 1}`);
    if (!chain.known) return false;
    const writes: [number, EffectSlot[]][] = [[index, chain.slots.filter((s) => s !== slot)]];
    const mirror = this.#mirror(chain, position, slot.type);
    if (mirror !== undefined) writes.push([mirror.chain.index, mirror.chain.slots.filter((s) => s !== mirror.slot)]);
    // Removal clears the instance's `enabled` on the device (P114), which is bypassed as this app shows it.
    this.#bypassOf(slot.type, slot.inst).value = true;
    if (mirror !== undefined) this.#bypassOf(mirror.slot.type, mirror.slot.inst).value = true;
    this.#writeChains(writes);
    return true;
  }

  /**
   * Moves the effect in a slot `by` places earlier (negative) or later in its chain, and the same effect in
   * a linked partner's slot with it. Refused (false) when it would leave the chain.
   * **Reordering has never been tried on a device** (P114 covered only inserting and removing).
   */
  moveEffect(index: number, position: number, by: number): boolean {
    const chain = this.#chain(index);
    const from = chain.slots.findIndex((s) => s.position === position);
    if (from < 0) throw new RangeError(`AFX IN ${index + 1} has no effect in slot ${position + 1}`);
    const to = from + by;
    if (!chain.known || !Number.isInteger(by) || by === 0 || to < 0 || to >= chain.slots.length) return false;
    const moved = (slots: readonly EffectSlot[]): EffectSlot[] => {
      const next = [...slots];
      const [slot] = next.splice(from, 1);
      if (slot !== undefined) next.splice(to, 0, slot);
      return next;
    };
    const writes: [number, EffectSlot[]][] = [[index, moved(chain.slots)]];
    const type = chain.slots[from]?.type;
    const mirror = type === undefined ? undefined : this.#mirror(chain, position, type);
    if (mirror !== undefined && mirror.chain.slots.length > to) writes.push([mirror.chain.index, moved(mirror.chain.slots)]);
    this.#writeChains(writes);
    return true;
  }

  /** A linked partner's slot at the same position holding the same effect, which a change is mirrored onto. */
  #mirror(chain: EffectChain, position: number, type: number): { chain: EffectChain; slot: EffectSlot } | undefined {
    const partner = chain.linked ? this.#chains.peek()?.[chain.partner] : undefined;
    const slot = partner?.slots.find((s) => s.position === position && s.type === type);
    return partner === undefined || slot === undefined ? undefined : { chain: partner, slot };
  }

  /** Every instance of each type the chains this app has read are using. */
  #used(): Map<number, Set<number>> {
    const used = new Map<number, Set<number>>();
    for (const chain of this.#chains.value ?? []) {
      if (!chain.known) continue;
      for (const slot of chain.slots) {
        const instances = used.get(slot.type) ?? new Set<number>();
        instances.add(slot.inst);
        used.set(slot.type, instances);
      }
    }
    return used;
  }

  #maxInstances(type: number): number {
    return FEWER_INSTANCES[this.family].get(type) ?? MAX_INSTANCES;
  }

  /** The `needed` lowest instances of a type no chain this app has read is using. */
  #freeInstances(type: number, needed: number): number[] {
    const used = this.#used().get(type) ?? new Set<number>();
    const free: number[] = [];
    for (let inst = 0; inst < this.#maxInstances(type) && free.length < needed; inst++) {
      if (!used.has(inst)) free.push(inst);
    }
    return free;
  }

  /**
   * An instance whose chain changed: read its settings again when its editor is next opened. Both
   * keys go, the type's (the Studio+ reads every instance at once) and this instance's, so that a
   * chain changed before the catalogue arrived is forgotten too.
   */
  #forgetParameters(type: number, inst: number): void {
    this.#parametersRead.delete(`${type}`);
    this.#parametersRead.delete(`${type}:${inst}`);
  }

  /**
   * Writes whole chains, each `set_afx_order` carrying eight slots packed from the first with the rest
   * empty, and reads back what the device made of it. Shown at once and put back when nothing was sent.
   * Not coalesced: a superseded write would read as a failure and put back a chain that did change.
   */
  #writeChains(writes: readonly (readonly [number, readonly EffectSlot[]])[]): void {
    const before = this.#chains.peek();
    const packed = writes.map(([index, slots]) => [index, slots.map((slot, position) => ({ ...slot, position }))] as const);
    if (before !== undefined) {
      this.#chains.value = before.map((chain) => {
        const write = packed.find(([index]) => index === chain.index);
        return write === undefined ? chain : { ...chain, slots: [...write[1]] };
      });
    }
    void Promise.all(packed.map(([index, slots]) => this.#context.invoke("set_afx_order", { ch_id: index, slots: chainBytes(slots) }, {}))).then((sent) => {
      if (sent.includes(false)) {
        if (before !== undefined) this.#chains.value = before;
        return;
      }
      void this.#refresh(packed.map(([index]) => index));
    });
  }

  /** After a change: the chains written and the free instance counts, as the panel reads them again. */
  async #refresh(indexes: readonly number[]): Promise<void> {
    const generation = this.#generation;
    const [orders] = await Promise.all([this.#readOrders(indexes), this.#readInstances()]);
    if (generation === this.#generation) this.#applyChains(orders);
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

  /**
   * Fetches the parameter catalogue, which travels in a chunk of its own: nothing below knows an
   * effect's parameters until it is here. The Effects page calls this as it opens.
   */
  loadCatalogue(): Promise<unknown> {
    return loadCatalogue();
  }

  /** True once the catalogue is here; reactive, so a page reading it is built again when it arrives. */
  get catalogueReady(): boolean {
    return catalogue.value !== undefined;
  }

  /**
   * What the editor knows of an effect type on this model; undefined when it is left out (see
   * `unsupportedReason`) and until the catalogue is here.
   */
  description(type: number): EffectDescription | undefined {
    return catalogue.peek()?.parameters[this.family].get(type);
  }

  /** Why an effect type's parameters are not editable, when they are not. */
  unsupportedReason(type: number): string | undefined {
    if (this.description(type) !== undefined) return undefined;
    return catalogue.peek()?.unsupported[this.family].get(type) ?? "Its parameters were not found in the vendor panel's code.";
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
