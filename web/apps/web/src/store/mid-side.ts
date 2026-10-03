// Monitoring a mid and a side microphone as stereo in a hardware mix (the owner, 2026-10-02): the
// two are recorded raw, for decoding later, and heard decoded while tracking. Left is mid plus side
// and right is mid minus side, so the mid channel is centred, the side signal is panned hard left,
// and an inverted copy of it is panned hard right at the same level.
//
// A strip has level, pan, mute and solo but no polarity switch, on either model, and nothing in the
// mixer or routing commands inverts. So the inverted copy comes from an effect chain: the side input
// is routed into a free chain holding one effect that has a polarity switch (the BAE-1084, BAE-1023
// and BAE-1073 equaliser models have one, on both interfaces), switched on.
//
// An effect delays what passes through it. Measured on the owner's Quadro (2026-10-03, 96 kHz, see
// `MEASURED`): an empty chain adds no delay and no level change, and each of those three at its
// starting values delays by 4 samples; the BAE-1073 is also 0.2 dB lower. A dry side strip against a
// copy through the effect cancels in mono by only 3.4 dB. So the side signal goes through two chains,
// each holding the same effect at the same settings, one with its polarity switch off (the hard-left
// strip) and one with it on (the hard-right strip): the two carry the same delay and level and cancel
// in mono (by 90 dB or more). The side channel the person chose stays as it is, muted in the mix
// while the decode plays there. The mid stays dry, so it leads both sides alike by the effect's delay,
// which leaves the mono sum alone. The Gyratec IX also has a polarity switch, but colours, lifts the
// level 2.2 dB and adds 10 samples, so it is not used.
//
// Two earlier ways are gone: a second preamp fed by a split of the side microphone (the owner will
// not spend a preamp on M/S), and one chain for the inverted copy only, against the dry side. A
// decode saved either way still loads; it is said to be no longer supported, and only removing it is
// offered.
//
// Everything here follows the pattern of tidying a mix (`mix-tidy.ts`): a plan is worked out and
// shown line by line, and nothing is sent until it is confirmed. Each line carries the change it
// describes, so what is listed and what is done cannot drift apart. The decode is kept on its group
// in the workspace (`MidSide`), with what setting it up changed, so removing it puts that back.
// `problems` is the guard: what would quietly break the decode, each with the change that mends it.
// Only the Mixer page's lazy elements use this, so it travels in their chunk.

import type { Link, MidSide, MixerChannel, MixerGroup, RouteSource } from "gazelle-audio-client";

import { EFFECT_NAMES } from "./effect-catalogue.ts";
import { formatLevel, formatPan, LEVEL_MAX, PAN_CENTRE, PAN_MAX, PAN_MIN } from "./mixer.ts";
import type { Store } from "./store.ts";

/**
 * The effects with a polarity switch, on both models, in the order they are tried, the most
 * transparent first (the BAE-1084, BAE-1023 and BAE-1073): its parameter is `phase_inv`.
 */
export const INVERTING_EFFECTS: readonly number[] = [25, 24, 7];
const POLARITY = "phase_inv";

/**
 * What each was measured to do at its starting values (the owner's Quadro, 2026-10-03, 96 kHz, a
 * click at -18 dBFS, steady over 18 or more clicks): its delay in samples and microseconds, and its
 * level change in dB. Two of one, one inverting, cancel to -104 dB (BAE-1084), -90 dB (BAE-1023)
 * and -102 dB (BAE-1073). An empty chain adds no delay and no level change.
 */
const MEASURED: ReadonlyMap<number, { samples: number; micros: number; level: number }> = new Map([
  [25, { samples: 4, micros: 42, level: 0 }],
  [24, { samples: 4, micros: 42, level: 0 }],
  [7, { samples: 4, micros: 41, level: -0.2 }],
]);

/** Where the widest Width goes either way, in dB: the side strips this far above or below the mid. */
export const WIDTH_RANGE = 30;

/** A decode's channels: the mid; the side microphone's own channel, muted; and the two side strips through the chains. */
export type Role = "mid" | "dry" | "side" | "inverted";

/** One change of a plan: the line the confirm shows, and the change itself. */
interface Step {
  readonly line: string;
  run(): unknown;
}

export interface Plan {
  /** One line per change, in the order they are made. */
  readonly lines: readonly string[];
  /** What the person should know that is not a change: what is recorded, and what has been measured. */
  readonly notes: readonly string[];
  /** Makes every change, in order. Rejects with what stopped it; the changes before that stay made. */
  apply(): Promise<void>;
}

export type Planned = { ok: true; plan: Plan } | { ok: false; why: string };

/** A decode as the layout holds it: its group and its channels, each undefined once removed by hand. */
export interface Decode {
  readonly group: MixerGroup;
  readonly decode: MidSide;
  readonly mid: MixerChannel | undefined;
  /** The side microphone's own channel, muted where the decode plays. */
  readonly side: MixerChannel | undefined;
  /** The side signal through its chain, hard left. Undefined on a decode saved by an earlier way. */
  readonly left: MixerChannel | undefined;
  /** The inverted copy through the other chain, hard right. */
  readonly inverted: MixerChannel | undefined;
}

/** Something that breaks a decode, in plain words, with the change that mends it when there is one. */
export interface Problem {
  readonly text: string;
  readonly fix?: Step;
}

/** A plan that stopped and left nothing behind: what it had changed up to there is put back already. */
export class UndoneError extends Error {}

const refuse = (why: string): Planned => ({ ok: false, why });

function plan(steps: readonly (Step | undefined)[], notes: readonly string[]): Plan {
  const kept = steps.filter((step): step is Step => step !== undefined);
  return {
    lines: kept.map((step) => step.line),
    notes,
    apply: async () => {
      for (const step of kept) await step.run();
    },
  };
}

const same = (a: RouteSource | undefined, b: RouteSource | undefined) => a !== undefined && b !== undefined && a.group === b.group && a.channel === b.channel;

/** The topology positions this needs, and the device's models. Throws for a device of unknown model. */
function parts(store: Store, deviceId: string) {
  const topology = store.topology(deviceId);
  if (topology === undefined) throw new Error(`${deviceId} has no known model, so no mixer`);
  return {
    topology,
    channels: store.channels(deviceId),
    effects: store.effects(deviceId),
    routing: store.routing(deviceId),
    preampGroup: topology.inputs.findIndex((g) => g.type === "PREAMP"),
    afxOut: topology.inputs.findIndex((g) => g.type === "AFX_OUT"),
    afxIn: topology.outputs.findIndex((g) => g.type === "AFX_IN"),
    /** The Quadro keeps AFX Out k on slot k of every mix, outside the layout: its effect returns. */
    returns: store.channels(deviceId).firstSlot,
  };
}

/** A strip's pan as the page shows it: where it returns to, while its mix is mono. */
function panOf(store: Store, deviceId: string, mix: number, slot: number): number {
  const mixer = store.mixer(deviceId, mix);
  return mixer.monoPan(slot) ?? mixer.strip(slot).value.pan;
}

const panWords = (pan: number) => (pan === PAN_MIN ? "hard left" : pan === PAN_MAX ? "hard right" : pan === PAN_CENTRE ? "centre" : formatPan(pan));

/** Whether a decode was saved by an earlier way (a second preamp, or one chain against the dry side), which is no longer set up, checked or mended. */
export const retired = (decode: Pick<MidSide, "via" | "left" | "left_chain">): boolean => decode.via !== "effect" || decode.left === undefined || decode.left_chain === undefined;

function retiredText(decode: Pick<MidSide, "via">): string {
  return decode.via === "effect"
    ? "This decode sends only the inverted copy through an effect chain, against the dry side, so the effect's delay keeps the side from cancelling in mono. Gazelle no longer sets it up that way or checks it. Remove it to put the pans, links, chain and channels back, and set it up again."
    : "This decode makes its inverted copy with a second preamp, which Gazelle no longer supports, so it is not checked any more. Remove it to put the pans, links and channels back.";
}

/** One of a decode's two chains: the hard-left side strip's (polarity off) or the inverted copy's (polarity on). */
interface Leg {
  readonly chain: number;
  readonly inst: number;
  /** What fed the chain before, none when it was muted. */
  readonly input: RouteSource | undefined;
  /** Mixes whose effect return for the chain this decode muted. */
  readonly returns: readonly number[];
  readonly polarity: 0 | 1;
  /** What its output carries, for a line. */
  readonly carries: string;
}

/** A decode's chains, the hard-left one first; a decode saved with one chain has only the inverted copy's. */
function legs(decode: MidSide): Leg[] {
  if (decode.via !== "effect") return [];
  const out: Leg[] = [];
  if (decode.left_chain !== undefined) out.push({ chain: decode.left_chain, inst: decode.left_effect_inst ?? 0, input: decode.left_chain_input, returns: decode.left_returns_muted ?? [], polarity: 0, carries: "the side signal" });
  if (decode.chain !== undefined) out.push({ chain: decode.chain, inst: decode.effect_inst ?? 0, input: decode.chain_input, returns: decode.returns_muted ?? [], polarity: 1, carries: "the inverted copy" });
  return out;
}

/** Every decode in a device's layout. Reading it is reactive. */
export function decodes(store: Store, deviceId: string): Decode[] {
  const layout = store.channels(deviceId).layout.value;
  const channel = (id: string | undefined) => (id === undefined ? undefined : layout.channels.find((c) => c.id === id));
  return layout.groups.flatMap((group) => {
    const decode = group.mid_side;
    return decode === undefined ? [] : [{ group, decode, mid: channel(decode.mid), side: channel(decode.side), left: channel(decode.left), inverted: channel(decode.inverted) }];
  });
}

export function decodeOf(store: Store, deviceId: string, groupId: string): Decode | undefined {
  return decodes(store, deviceId).find((d) => d.group.id === groupId);
}

/** Whether a decode plays in a mix: one of its side strips is set up there. Reactive. */
export function playsIn(store: Store, deviceId: string, found: Decode, mix: number): boolean {
  const channels = store.channels(deviceId);
  const strips = retired(found.decode) ? [found.side, found.inverted] : [found.left, found.inverted];
  return strips.some((c) => c !== undefined && channels.strip(c, mix).inMix);
}

/** The two free chains and the effect the side strips would go through, or why there are none and what to free up. */
export type Chains = { chains: readonly [number, number]; type: number; label: string } | { why: string };

/** Where the two side strips of a side channel would be made. Reactive. */
export function freeChains(store: Store, deviceId: string, side: MixerChannel): Chains {
  const { topology, channels, effects, routing, afxOut, afxIn } = parts(store, deviceId);
  const layout = channels.layout.value;
  if (afxOut < 0 || afxIn < 0) return { why: "This model has no effect chains, and the side signal goes through two." };
  const chains = effects.chains.value;
  if (chains === undefined || !chains.some((chain) => chain.known)) return { why: "The effect chains have not been read from the device, so it is not known which are free." };
  const feeding = routing.destination(afxIn).value;
  if (feeding === undefined) return { why: "What feeds the effect chains has not been read from the device." };
  const outputs = topology.inputs[afxOut]?.channels ?? 0;
  const free = chains.filter((chain) => {
    const fed = feeding[chain.index];
    const unfed = fed !== undefined && (fed.source === routing.mute || same({ group: fed.source, channel: fed.channel }, side.source));
    return chain.known && chain.slots.length === 0 && !chain.linked && chain.index < outputs && unfed && !layout.channels.some((c) => c.source?.group === afxOut && c.source.channel === chain.index);
  });
  const [first, second] = free;
  if (first === undefined || second === undefined) {
    return {
      why:
        `The side signal goes through two effect chains, one for each side strip, and ${first === undefined ? "none is" : `only ${first.name} is`} free: the others hold an effect, are linked, are fed by another input or have a channel on their output. ` +
        `Free ${first === undefined ? "two" : "one more"} first: on the Effects page take every effect out of a chain and unlink it, route Mute into it, and take any mixer channel off its AFX Out.`,
    };
  }
  const names = INVERTING_EFFECTS.map((type) => EFFECT_NAMES[effects.family].get(type) ?? `Effect ${type}`);
  const type = INVERTING_EFFECTS.find((t) => effects.offers(first.index).some((o) => o.type === t && o.unavailable === undefined && o.free >= 2));
  if (type !== undefined) return { chains: [first.index, second.index], type, label: `${EFFECT_NAMES[effects.family].get(type) ?? `Effect ${type}`} in ${first.name} and ${second.name}` };
  return { why: `${first.name} and ${second.name} are free, but two instances of one of ${names.slice(0, -1).join(", ")} or ${names.at(-1)} are needed, one for each side strip, and no two of one are left. Take some out of other chains on the Effects page first.` };
}

/** What every plan says about the recording, so nobody wonders whether the tracks are decoded. */
function recordedNote(store: Store, deviceId: string, decode: Pick<MidSide, "mid_source" | "side_source">): string[] {
  const { channels } = parts(store, deviceId);
  return [`What is recorded does not change: ${channels.sourceLabel(decode.mid_source)} and ${channels.sourceLabel(decode.side_source)} reach your DAW raw, as they do now, for decoding later. Only the mix hears the decoded stereo.`];
}

/** What has been measured of the effect, and how to check by ear. */
function measuredNote(name: string, type: number): string {
  const found = MEASURED.get(type);
  const measured =
    found === undefined
      ? `${name} has not been measured.`
      : `Measured on a Quadro at 96 kHz, at a modest level: ${name} at its starting values delays by ${found.samples} samples (${found.micros} microseconds) and ${found.level === 0 ? "leaves the level as it is" : `is ${Math.abs(found.level)} dB ${found.level < 0 ? "lower" : "higher"}`}; an empty chain adds nothing. Louder signals may add some saturation.`;
  return (
    `${measured} Both side strips go through the same effect at the same settings, so they match and cancel in mono. The mid does not go through one, so it reaches left and right alike that much sooner than the side, which leaves the mono sum untouched. ` +
    "To check by ear, mute the mid and switch the mix to mono: the side strips should cancel to near silence."
  );
}

/** The mixer link that ties the two side strips: one joining both, every member taking the same value. Reactive. */
function sideLink(store: Store, deviceId: string, side: number, inverted: number): Link | undefined {
  const link = store.links.linkOf("mixer", deviceId, side);
  return link !== undefined && link.mode === "absolute" && link.members.some((m) => m.device_id === deviceId && m.channel === inverted) ? link : undefined;
}

/** Makes a link again as it was, leaving out members that can no longer be linked. */
function relink(store: Store, link: Link): void {
  try {
    store.links.create(link.kind, link.members, link.mode);
  } catch {
    // A member's device is gone, or the channel no longer exists: there is nothing to put back.
  }
}

/** The names a list of link members go by, for a line. */
function memberNames(store: Store, deviceId: string, link: Link, except: readonly number[] = []): string {
  const channels = store.channels(deviceId);
  const names = link.members
    .filter((m) => m.device_id !== deviceId || !except.includes(m.channel))
    .map((m) => {
      if (link.kind !== "mixer") return channels.sourceLabel({ group: parts(store, deviceId).preampGroup, channel: m.channel });
      const own = m.device_id === deviceId ? channels.layout.peek().channels.find((c) => c.slot === m.channel) : undefined;
      return own === undefined ? `strip ${m.channel + 1}` : channels.displayName(own);
    });
  return names.join(", ");
}

/**
 * Sets an effect up in an empty chain: added, read, then every setting at its starting value (or
 * as `like` has it) with the polarity switch as asked. Throws when any of that could not be done,
 * having taken the effect out again, so a chain is never left holding an effect set some other way.
 */
async function insertWith(store: Store, deviceId: string, chain: number, type: number, polarity: 0 | 1, like?: Readonly<Record<string, number>>): Promise<number> {
  const { effects } = parts(store, deviceId);
  const name = EFFECT_NAMES[effects.family].get(type) ?? `Effect ${type}`;
  if (!effects.addEffect(chain, type)) throw new Error(`${name} could not be added to AFX In ${chain + 1}.`);
  const added = () => effects.chains.peek()?.[chain]?.slots.find((slot) => slot.type === type);
  const inst = added()?.inst;
  const undo = () => {
    const slot = added();
    if (slot !== undefined) effects.removeEffect(chain, slot.position);
  };
  try {
    await effects.loadCatalogue();
    if (inst !== undefined) await effects.readParameters(type, inst);
    const slot = added();
    const description = effects.description(type);
    if (slot === undefined || inst === undefined || description === undefined) throw new Error(`${name} could not be added to AFX In ${chain + 1}.`);
    let set = true;
    // Every change resends every setting, so the last one carries them all.
    for (const parameter of description.parameters) {
      if (parameter.control === undefined) continue;
      set = effects.setParameter(chain, slot.position, parameter.name, parameter.name === POLARITY ? polarity : (like?.[parameter.name] ?? parameter.default ?? 0)) && set;
    }
    if (!set || effects.parameters(type, inst).peek()?.values[POLARITY] !== polarity) throw new Error(`${name}'s settings in AFX In ${chain + 1} could not be read, so they could not be set.`);
    effects.setBypass(chain, slot.position, false);
    return inst;
  } catch (error) {
    undo();
    throw error;
  }
}

/** Whether a Quadro effect return for a chain is heard in a mix: routed on its slot, unmuted, above the floor. Reactive. */
function returnPlays(store: Store, deviceId: string, mix: number, chain: number): boolean {
  const { routing, afxOut, returns } = parts(store, deviceId);
  if (chain >= returns) return false;
  const fed = routing.destination(store.mixInput(deviceId, mix)).value?.[chain];
  const strip = store.mixer(deviceId, mix).strip(chain).value;
  return fed !== undefined && fed.source === afxOut && fed.channel === chain && !strip.mute && strip.level < LEVEL_MAX;
}

/** Whether the side microphone's own channel plays dry in a mix: there, and not muted. Reactive. */
function dryPlays(store: Store, deviceId: string, side: MixerChannel | undefined, mix: number): boolean {
  return side !== undefined && store.channels(deviceId).strip(side, mix).inMix && !store.mixer(deviceId, mix).strip(side.slot).value.mute;
}

/**
 * The plan that starts monitoring two channels of a mix as mid and side: every change, in order,
 * and the notes. Refused, with the reason, when the two cannot be a mid and a side, or there are not
 * two free effect chains and two free instances of one inverting effect.
 */
export function setupPlan(store: Store, deviceId: string, mix: number, midId: string, sideId: string): Planned {
  const { channels, effects, routing, afxIn, afxOut } = parts(store, deviceId);
  const mid = channels.channel(midId);
  const side = channels.channel(sideId);
  if (mid === undefined || side === undefined || mid.id === side.id) return refuse("Select two channels: the mid microphone's and the side microphone's.");
  const taken = decodes(store, deviceId).find((d) => [d.decode.mid, d.decode.side, d.decode.left, d.decode.inverted].some((id) => id === mid.id || id === side.id));
  if (taken !== undefined) return refuse(`${taken.group.name} already uses one of these channels. Remove it first.`);
  if (mid.source === undefined || side.source === undefined || !channels.strip(mid, mix).inMix || !channels.strip(side, mix).inMix) return refuse("Both channels need an input and a place in this mix.");
  if (same(mid.source, side.source)) return refuse("Both channels are on the same input, so there is no side signal to decode.");
  if (channels.layout.peek().channels.length + 2 > 32 - channels.firstSlot) return refuse("The two side strips need two more mixer channels, and there are not two free.");
  const offered = freeChains(store, deviceId, side);
  if ("why" in offered) return refuse(offered.why);
  const { chains: pair, type } = offered;
  const [leftChain, copyChain] = pair;
  const own = (chain: number) => effects.chains.value?.[chain];
  const fedOf = (chain: number) => routing.destination(afxIn).value?.[chain];
  const leftOwn = own(leftChain);
  const copyOwn = own(copyChain);
  const leftFed = fedOf(leftChain);
  const copyFed = fedOf(copyChain);
  if (leftOwn === undefined || copyOwn === undefined || leftFed === undefined || copyFed === undefined) return refuse("Those effect chains are not free any more.");

  const mixer = store.mixer(deviceId, mix);
  const mixName = channels.mixName(mix);
  const midName = channels.displayName(mid);
  const sideName = channels.displayName(side);
  const leftName = `${sideName} L`;
  const copyName = `${sideName} Ø`;
  const sideSource = side.source;
  const midSource = mid.source;
  const name = EFFECT_NAMES[effects.family].get(type) ?? `Effect ${type}`;
  const decode: MidSide = { mid: mid.id, side: side.id, left: "", inverted: "", via: "effect", chain: copyChain, effect_type: type, effect_inst: 0, left_chain: leftChain, left_effect_inst: 0, mid_source: midSource, side_source: sideSource, pans: {} };
  const steps: (Step | undefined)[] = [];
  const label = (source: RouteSource) => channels.sourceLabel(source);
  const fedLabel = (fed: { source: number; channel: number }) => (fed.source === routing.mute ? "Mute" : label({ group: fed.source, channel: fed.channel }));
  const back = (chain: number, fed: { source: number; channel: number }) => routing.route(afxIn, chain, fed.source === routing.mute ? null : fed);

  // The side input into both chains.
  for (const [chain, chainOwn, fed] of [[leftChain, leftOwn, leftFed], [copyChain, copyOwn, copyFed]] as const) {
    const before = { group: fed.source, channel: fed.channel };
    if (fed.source !== routing.mute && !same(before, sideSource)) {
      if (chain === leftChain) decode.left_chain_input = before;
      else decode.chain_input = before;
    }
    if (same(before, sideSource)) continue;
    steps.push({
      line: `Route ${label(sideSource)} into ${chainOwn.name} as well, in place of ${fedLabel(fed)}`,
      run: async () => {
        if (!(await routing.route(afxIn, chain, { source: sideSource.group, channel: sideSource.channel }))) throw new Error(`${label(sideSource)} could not be routed into ${chainOwn.name}.`);
      },
    });
  }
  // The same effect in both, the second inverting. Either failing puts both chains back as they were.
  const undoChains = async (removeLeft: boolean) => {
    if (removeLeft) {
      const slot = effects.chains.peek()?.[leftChain]?.slots.find((s) => s.type === type && s.inst === decode.left_effect_inst);
      if (slot !== undefined) effects.removeEffect(leftChain, slot.position);
    }
    await back(leftChain, leftFed);
    await back(copyChain, copyFed);
  };
  steps.push({
    line: `Add ${name} to ${leftOwn.name}, which is empty, with every setting at its starting value`,
    run: async () => {
      try {
        decode.left_effect_inst = await insertWith(store, deviceId, leftChain, type, 0);
      } catch (error) {
        await undoChains(false);
        throw new UndoneError(`${error instanceof Error ? error.message : String(error)} The chains are as they were, and nothing else was changed.`);
      }
    },
  });
  steps.push({
    line: `Add ${name} to ${copyOwn.name}, which is empty, with the same settings and its polarity switch on`,
    run: async () => {
      try {
        decode.effect_inst = await insertWith(store, deviceId, copyChain, type, 1);
      } catch (error) {
        await undoChains(true);
        throw new UndoneError(`${error instanceof Error ? error.message : String(error)} The chains are as they were, and nothing else was changed.`);
      }
    },
  });
  for (const [chain, carries] of [[leftChain, "the side signal"], [copyChain, "the inverted copy"]] as const) {
    if (!returnPlays(store, deviceId, mix, chain)) continue;
    if (chain === leftChain) decode.left_returns_muted = [mix];
    else decode.returns_muted = [mix];
    steps.push({ line: `Mute the effect return ${label({ group: afxOut, channel: chain })} on slot ${chain + 1} of ${mixName}, which would play ${carries} a second time`, run: () => mixer.setAlone(chain, { mute: true }) });
  }

  const midPan = panOf(store, deviceId, mix, mid.slot);
  decode.pans[String(mix)] = { mid: midPan, side: panOf(store, deviceId, mix, side.slot) };
  if (midPan !== PAN_CENTRE) steps.push({ line: `Pan ${midName} to the centre in ${mixName} (it is at ${formatPan(midPan)})`, run: () => mixer.setPan(mid.slot, PAN_CENTRE) });

  const sideStrip = mixer.strip(side.slot).value;
  const slots = { left: -1, copy: -1 };
  /** Adds a side strip: on its chain's output, at the side channel's level and mute. */
  const addStrip = (chain: number, title: string, key: "left" | "copy", pan: number) => async () => {
    const id = await channels.addFed({ group: afxOut, channel: chain }, mix);
    const added = id === undefined ? undefined : channels.channel(id);
    if (id === undefined || added === undefined) throw new Error(`The channel ${title} could not be added.`);
    channels.rename(id, title);
    if (key === "left") decode.left = id;
    else decode.inverted = id;
    slots[key] = added.slot;
    mixer.setPan(added.slot, pan);
    mixer.setLevel(added.slot, sideStrip.level);
    if (sideStrip.mute !== mixer.strip(added.slot).peek().mute) await mixer.setAlone(added.slot, { mute: sideStrip.mute });
  };
  const muted = sideStrip.mute ? `, and muted as ${sideName} is` : "";
  steps.push({ line: `Add a channel, ${leftName}, on ${label({ group: afxOut, channel: leftChain })} to ${mixName}: the side signal, panned hard left at ${sideName}'s level, ${formatLevel(sideStrip.level)}${muted}`, run: addStrip(leftChain, leftName, "left", PAN_MIN) });
  steps.push({ line: `Add a channel, ${copyName}, on ${label({ group: afxOut, channel: copyChain })} to ${mixName}: the inverted copy, panned hard right at the same level${muted}`, run: addStrip(copyChain, copyName, "copy", PAN_MAX) });
  if (!sideStrip.mute) {
    decode.side_muted = [mix];
    steps.push({ line: `Mute ${sideName} in ${mixName} while the decode plays there: ${leftName} carries the side signal instead, with the same delay as ${copyName}`, run: () => mixer.setAlone(side.slot, { mute: true }) });
  }
  steps.push({
    line: `Link ${leftName} and ${copyName}, so their levels, mutes and solos always match`,
    run: () => {
      // A free slot can still sit in a link an earlier channel left behind: it is put back with the rest.
      for (const slot of [slots.left, slots.copy]) {
        const stale = store.links.linkOf("mixer", deviceId, slot);
        if (stale !== undefined && !(decode.displaced_links ?? []).some((l) => l.id === stale.id)) decode.displaced_links = [...(decode.displaced_links ?? []), stale];
      }
      store.links.create("mixer", [{ device_id: deviceId, channel: slots.left }, { device_id: deviceId, channel: slots.copy }], "absolute");
    },
  });

  const groupName = `M/S: ${midName}`;
  const was = [channels.groupOf(mid.id), channels.groupOf(side.id)];
  if (was[0] !== undefined) decode.mid_group = was[0].id;
  if (was[1] !== undefined) decode.side_group = was[1].id;
  const leaving = [...new Set(was.flatMap((g) => (g === undefined ? [] : [g.name])))];
  steps.push({
    line: `Group the four as "${groupName}"` + (leaving.length === 0 ? "" : `, out of ${leaving.length === 1 ? "the group" : "the groups"} ${leaving.join(" and ")}`),
    run: () => {
      const id = channels.addGroup(groupName, [mid.id, side.id, decode.left ?? "", decode.inverted]);
      if (id === undefined || !channels.setMidSide(id, decode)) throw new Error("The group could not be saved: Gazelle is not connected.");
    },
  });
  return { ok: true, plan: plan(steps, [measuredNote(name, type), ...recordedNote(store, deviceId, decode)]) };
}

/** How a channel joins a mix it is not in: as a send, or as its main mix when it has none. */
function joinMix(store: Store, deviceId: string, channel: MixerChannel, mix: number): Promise<boolean> {
  const channels = store.channels(deviceId);
  return channel.main_mix === undefined ? channels.setMainMix(channel.id, mix) : channels.setSend(channel.id, mix, true);
}

/** The plan that plays a decode in another mix as well: the same pans, matched side strips, the dry side and the returns muted there. */
export function addToMixPlan(store: Store, deviceId: string, groupId: string, mix: number): Planned {
  const { channels, afxOut } = parts(store, deviceId);
  const found = decodeOf(store, deviceId, groupId);
  if (found === undefined) return refuse("This decode is gone.");
  const { decode, mid, side, left, inverted } = found;
  if (retired(decode)) return refuse(retiredText(decode));
  if (mid === undefined || left === undefined || inverted === undefined) return refuse("One of its channels has been removed. Remove the decode and set it up again.");
  if (playsIn(store, deviceId, found, mix) && decode.pans[String(mix)] !== undefined) return refuse("It already plays in this mix.");
  const mixer = store.mixer(deviceId, mix);
  const mixName = channels.mixName(mix);
  const name = (c: MixerChannel) => channels.displayName(c);
  const steps: (Step | undefined)[] = [];
  for (const c of [mid, left, inverted]) if (!channels.strip(c, mix).inMix) steps.push({ line: `Send ${name(c)} to ${mixName} too`, run: () => joinMix(store, deviceId, c, mix) });
  const before = { mid: panOf(store, deviceId, mix, mid.slot), side: side === undefined ? PAN_CENTRE : panOf(store, deviceId, mix, side.slot) };
  const wanted: [MixerChannel, number][] = [[mid, PAN_CENTRE], [left, PAN_MIN], [inverted, PAN_MAX]];
  for (const [c, pan] of wanted) {
    const now = panOf(store, deviceId, mix, c.slot);
    if (now !== pan) steps.push({ line: `Pan ${name(c)} ${pan === PAN_CENTRE ? "to the centre" : panWords(pan)} in ${mixName} (it is at ${formatPan(now)})`, run: () => mixer.setPan(c.slot, pan) });
  }
  const level = mixer.strip(left.slot).value;
  const copy = mixer.strip(inverted.slot).value;
  if (copy.level !== level.level) steps.push({ line: `Set ${name(inverted)} to ${name(left)}'s level in ${mixName}, ${formatLevel(level.level)}`, run: () => mixer.setLevel(inverted.slot, level.level) });
  if (copy.mute !== level.mute) steps.push({ line: `${level.mute ? "Mute" : "Unmute"} ${name(inverted)} in ${mixName}, as ${name(left)} is`, run: () => mixer.setAlone(inverted.slot, { mute: level.mute }) });
  const muting = legs(decode).filter((leg) => returnPlays(store, deviceId, mix, leg.chain));
  for (const leg of muting) steps.push({ line: `Mute the effect return ${channels.sourceLabel({ group: afxOut, channel: leg.chain })} on slot ${leg.chain + 1} of ${mixName}, which would play ${leg.carries} a second time`, run: () => mixer.setAlone(leg.chain, { mute: true }) });
  const dry = dryPlays(store, deviceId, side, mix);
  if (dry && side !== undefined) steps.push({ line: `Mute ${name(side)} in ${mixName} while the decode plays there: ${name(left)} carries the side signal instead`, run: () => mixer.setAlone(side.slot, { mute: true }) });
  const add = (list: number[] | undefined, yes: boolean) => (yes ? [...new Set([...(list ?? []), mix])] : list);
  steps.push({
    line: `Remember ${mixName}'s pans and mutes, so removing the decode puts them back`,
    run: () => {
      const current = decodeOf(store, deviceId, groupId)?.decode ?? decode;
      const leftMuted = add(current.left_returns_muted, muting.some((leg) => leg.polarity === 0));
      const copyMuted = add(current.returns_muted, muting.some((leg) => leg.polarity === 1));
      const sideMuted = add(current.side_muted, dry);
      channels.setMidSide(groupId, {
        ...current,
        pans: { ...current.pans, [String(mix)]: current.pans[String(mix)] ?? before },
        ...(leftMuted === undefined ? {} : { left_returns_muted: leftMuted }),
        ...(copyMuted === undefined ? {} : { returns_muted: copyMuted }),
        ...(sideMuted === undefined ? {} : { side_muted: sideMuted }),
      });
    },
  });
  return { ok: true, plan: plan(steps, recordedNote(store, deviceId, decode)) };
}

/**
 * The plan that takes a decode down in every mix it plays in: pans back where they were, the side
 * strips' channels removed, the side channel unmuted, the links and the effect chains put back, and
 * the group removed. A channel already removed by hand is simply passed over. A decode saved by an
 * earlier way is taken down the same way; a second preamp is left as it is, which the notes say.
 */
export function removePlan(store: Store, deviceId: string, groupId: string): Planned {
  const { channels, effects, routing, preampGroup, afxIn, afxOut } = parts(store, deviceId);
  const found = decodeOf(store, deviceId, groupId);
  if (found === undefined) return refuse("This decode is gone.");
  const { decode, group, mid, side, left, inverted } = found;
  const name = (c: MixerChannel) => channels.displayName(c);
  const steps: (Step | undefined)[] = [];
  const notes = [`Nothing recorded changes: ${channels.sourceLabel(decode.mid_source)} and ${channels.sourceLabel(decode.side_source)} reach your DAW as they did all along.`];
  for (const [key, pans] of Object.entries(decode.pans).sort(([a], [b]) => Number(a) - Number(b))) {
    const mix = Number(key);
    if (!Number.isInteger(mix) || mix < 0 || mix >= channels.mixCount) continue;
    const mixer = store.mixer(deviceId, mix);
    const back: [MixerChannel | undefined, number][] = [[mid, pans.mid], [side, pans.side]];
    for (const [c, pan] of back) {
      if (c === undefined || panOf(store, deviceId, mix, c.slot) === pan) continue;
      steps.push({ line: `Pan ${name(c)} back to ${panWords(pan)} in ${channels.mixName(mix)}`, run: () => mixer.setPan(c.slot, pan) });
    }
  }
  const strips = [left, inverted].filter((c): c is MixerChannel => c !== undefined);
  const linked = strips.filter((c) => store.links.linkOf("mixer", deviceId, c.slot) !== undefined);
  if (linked.length > 0) {
    const pair = left !== undefined && inverted !== undefined ? `${name(left)} and ${name(inverted)}` : `${side === undefined ? "the side channel" : name(side)} and ${name(linked[0] as MixerChannel)}`;
    steps.push({ line: `Unlink ${pair}`, run: () => linked.forEach((c) => store.links.removeMember("mixer", deviceId, c.slot)) });
  }
  if (left !== undefined) steps.push({ line: `Remove the channel ${name(left)}, the side signal through its effect chain`, run: () => channels.remove(left.id) });
  if (inverted !== undefined) steps.push({ line: `Remove the channel ${name(inverted)}, the inverted copy`, run: () => channels.remove(inverted.id) });
  if (side !== undefined) {
    for (const mix of decode.side_muted ?? []) {
      if (mix < 0 || mix >= channels.mixCount) continue;
      steps.push({ line: `Unmute ${name(side)} in ${channels.mixName(mix)}`, run: () => store.mixer(deviceId, mix).setAlone(side.slot, { mute: false }) });
    }
  }
  if (decode.via === "preamp") {
    const second = decode.preamp === undefined ? "The second preamp" : channels.sourceLabel({ group: preampGroup, channel: decode.preamp });
    notes.push(`${second} is left as it is, with its polarity (Ø) and any link to the side preamp: check it on the Inputs page before you use it for something else.`);
  }
  for (const leg of legs(decode)) {
    const own = effects.chains.value?.[leg.chain];
    const slot = own?.slots.find((s) => s.type === decode.effect_type && s.inst === leg.inst);
    if (own !== undefined && slot !== undefined) steps.push({ line: `Remove ${slot.name} from ${own.name}`, run: () => effects.removeEffect(leg.chain, slot.position) });
    const fed = routing.destination(afxIn).value?.[leg.chain];
    const wanted = leg.input === undefined ? { source: routing.mute, channel: 0 } : { source: leg.input.group, channel: leg.input.channel };
    if (fed === undefined || fed.source !== wanted.source || (leg.input !== undefined && fed.channel !== wanted.channel)) {
      steps.push({ line: `Route ${leg.input === undefined ? "Mute" : channels.sourceLabel(leg.input)} into AFX In ${leg.chain + 1} again`, run: () => routing.route(afxIn, leg.chain, leg.input === undefined ? null : wanted) });
    }
    for (const mix of leg.returns) {
      if (mix < 0 || mix >= channels.mixCount) continue;
      steps.push({ line: `Unmute the effect return ${channels.sourceLabel({ group: afxOut, channel: leg.chain })} in ${channels.mixName(mix)}`, run: () => store.mixer(deviceId, mix).setAlone(leg.chain, { mute: false }) });
    }
  }
  for (const link of decode.displaced_links ?? []) {
    steps.push({ line: `Link ${memberNames(store, deviceId, link).replace(/, ([^,]*)$/, " and $1")} again`, run: () => relink(store, link) });
  }
  const groups = channels.layout.value.groups;
  const home = (id: string | undefined) => groups.find((g) => g.id === id && g.id !== group.id);
  const midHome = home(decode.mid_group);
  const sideHome = home(decode.side_group);
  const returning = [mid !== undefined && midHome !== undefined ? `${name(mid)} goes back to the group ${midHome.name}` : "", side !== undefined && sideHome !== undefined ? `${name(side)} goes back to the group ${sideHome.name}` : ""].filter((text) => text !== "");
  steps.push({
    line: `Remove the group "${group.name}"; its channels stay${returning.length === 0 ? "" : `, and ${returning.join(", and ")}`}`,
    run: () => {
      channels.removeGroup(group.id);
      if (mid !== undefined && midHome !== undefined) channels.setGroup(mid.id, midHome.id);
      if (side !== undefined && sideHome !== undefined) channels.setGroup(side.id, sideHome.id);
    },
  });
  return { ok: true, plan: plan(steps, notes) };
}

/**
 * What would quietly break a decode, as it stands in one mix: the side strips' levels or mutes
 * apart, an effect bypassed or set differently from the other, the copy no longer inverted, the
 * dry side playing as well, a pan off its side. Each comes with the change that mends it where there
 * is one. Empty for a sound decode, and for anything not yet known (an effect's settings before they
 * are read): the guard never warns on a guess. A decode saved by an earlier way is not checked: it
 * has one problem, that it is no longer supported, with no mend but removing it. Reading it is reactive.
 */
export function problems(store: Store, deviceId: string, groupId: string, mix: number): Problem[] {
  const { channels, effects, routing, afxIn, afxOut } = parts(store, deviceId);
  const found = decodeOf(store, deviceId, groupId);
  if (found === undefined) return [];
  const { decode, mid, side, left, inverted } = found;
  if (retired(decode)) return [{ text: retiredText(decode) }];
  const gone = [mid === undefined ? "mid" : "", left === undefined ? "hard-left side" : "", inverted === undefined ? "inverted copy's" : ""].filter((role) => role !== "");
  if (mid === undefined || left === undefined || inverted === undefined) return [{ text: `The ${gone.join(" and the ")} channel has been removed, so nothing is decoded any more. Remove this decode to put the rest back.` }];
  const out: Problem[] = [];
  const name = (c: MixerChannel) => channels.displayName(c);
  const label = (source: RouteSource | undefined) => (source === undefined ? "no input" : channels.sourceLabel(source));
  const mixName = channels.mixName(mix);
  const [leftLeg, copyLeg] = legs(decode) as [Leg, Leg];
  const type = decode.effect_type ?? 0;
  const effect = EFFECT_NAMES[effects.family].get(type) ?? `Effect ${type}`;

  // The inputs: the mid on the mid microphone, and each side strip on its chain's output.
  if (!same(mid.source, decode.mid_source)) out.push({ text: `${name(mid)} is on ${label(mid.source)}, not ${label(decode.mid_source)}, the mid microphone.`, fix: { line: `Put ${name(mid)} back on ${label(decode.mid_source)}`, run: () => channels.setSource(mid.id, decode.mid_source) } });
  for (const [c, leg] of [[left, leftLeg], [inverted, copyLeg]] as const) {
    const wanted = { group: afxOut, channel: leg.chain };
    if (!same(c.source, wanted)) out.push({ text: `${name(c)} is on ${label(c.source)}, not ${label(wanted)}, which carries ${leg.carries}.`, fix: { line: `Put ${name(c)} back on ${label(wanted)}`, run: () => channels.setSource(c.id, wanted) } });
  }

  if (sideLink(store, deviceId, left.slot, inverted.slot) === undefined) {
    out.push({
      text: `${name(left)} and ${name(inverted)} are no longer linked, so their levels and mutes can drift apart.`,
      fix: { line: `Link ${name(left)} and ${name(inverted)} again, each taking the same value`, run: () => store.links.create("mixer", [{ device_id: deviceId, channel: left.slot }, { device_id: deviceId, channel: inverted.slot }], "absolute") },
    });
  }

  // Each chain: the effect alone in it, processing, its polarity switch as it should be, fed by the side input.
  const settled: { chain: number; position: number; values: Readonly<Record<string, number>> }[] = [];
  for (const leg of [leftLeg, copyLeg]) {
    const other = leg === leftLeg ? copyLeg : leftLeg;
    const own = effects.chains.value?.[leg.chain];
    if (own !== undefined && own.known) {
      const ours = own.slots.find((s) => s.type === type && s.inst === leg.inst);
      const others = own.slots.filter((s) => s !== ours);
      if (ours === undefined || others.length > 0) {
        out.push({
          text:
            ours === undefined
              ? `${own.name} ${own.slots.length === 0 ? "is empty" : `holds ${others.map((s) => s.name).join(", ")}`}, not the ${effect} that ${leg.polarity === 1 ? "inverts the copy" : "matches the inverted copy's delay"}, so the side does not cancel in mono.`
              : `${own.name} also holds ${others.map((s) => s.name).join(", ")}, so the two side strips no longer match.`,
          fix: {
            line: [
              others.length === 0 ? "" : `Remove ${others.map((s) => s.name).join(", ")} from ${own.name}`,
              ours !== undefined ? "" : `${others.length === 0 ? "Add" : "add"} ${effect} to ${own.name} again, set like the one in AFX In ${other.chain + 1} but with its polarity switch ${leg.polarity === 1 ? "on" : "off"}`,
            ]
              .filter((part) => part !== "")
              .join(", and "),
            run: async () => {
              // Last first, so the positions of the ones still to go do not move.
              for (const slot of [...others].sort((a, b) => b.position - a.position)) effects.removeEffect(leg.chain, effects.chains.peek()?.[leg.chain]?.slots.find((s) => s.type === slot.type && s.inst === slot.inst)?.position ?? slot.position);
              if (ours !== undefined) return;
              // Set like the other chain's, as far as that is known, so the two side strips match again.
              const like = effects.parameters(type, other.inst).peek();
              const made = await insertWith(store, deviceId, leg.chain, type, leg.polarity, like?.known === true ? like.values : undefined);
              const current = decodeOf(store, deviceId, groupId)?.decode;
              if (current === undefined) return;
              if (leg.polarity === 1 && made !== current.effect_inst) channels.setMidSide(groupId, { ...current, effect_inst: made });
              if (leg.polarity === 0 && made !== current.left_effect_inst) channels.setMidSide(groupId, { ...current, left_effect_inst: made });
            },
          },
        });
      } else {
        if (effects.bypass(type, leg.inst).value === true) out.push({ text: `${effect} in ${own.name} is bypassed, so the side does not cancel in mono.`, fix: { line: `Let ${effect} in ${own.name} process again`, run: () => effects.setBypass(leg.chain, ours.position, false) } });
        const settings = effects.parameters(type, leg.inst).value;
        if (settings !== undefined && settings.known) {
          if (settings.values[POLARITY] !== leg.polarity) {
            out.push({
              text: `${effect}'s polarity switch in ${own.name} is ${leg.polarity === 1 ? "off, so the copy is not inverted" : "on, so both side strips are inverted"}.`,
              fix: { line: `Switch ${effect}'s polarity in ${own.name} ${leg.polarity === 1 ? "on" : "off"} again`, run: () => effects.setParameter(leg.chain, ours.position, POLARITY, leg.polarity) },
            });
          }
          settled.push({ chain: leg.chain, position: ours.position, values: settings.values });
        }
      }
    }
    const fed = routing.destination(afxIn).value?.[leg.chain];
    if (fed !== undefined && !same({ group: fed.source, channel: fed.channel }, decode.side_source)) {
      out.push({
        text: `AFX In ${leg.chain + 1} takes ${fed.source === routing.mute ? "nothing" : label({ group: fed.source, channel: fed.channel })}, not ${label(decode.side_source)}, the side microphone.`,
        fix: { line: `Route ${label(decode.side_source)} into AFX In ${leg.chain + 1} again`, run: () => routing.route(afxIn, leg.chain, { source: decode.side_source.group, channel: decode.side_source.channel }) },
      });
    }
  }
  // The two effects set alike, polarity apart: otherwise the two side strips differ and do not cancel.
  const [lead, follow] = settled;
  const description = effects.description(type);
  if (lead !== undefined && follow !== undefined && description !== undefined) {
    const apart = description.parameters.filter((p) => p.control !== undefined && p.name !== POLARITY && lead.values[p.name] !== follow.values[p.name]);
    if (apart.length > 0) {
      out.push({
        text: `${effect} in AFX In ${follow.chain + 1} is not set like the one in AFX In ${lead.chain + 1} (${apart.map((p) => p.label).join(", ")}), so the two side strips do not match.`,
        fix: { line: `Set ${effect} in AFX In ${follow.chain + 1} like the one in AFX In ${lead.chain + 1}, polarity apart`, run: () => apart.forEach((p) => effects.setParameter(follow.chain, follow.position, p.name, lead.values[p.name] ?? p.default ?? 0)) },
      });
    }
  }

  // The rest is this mix's own: nothing to say where the decode does not play.
  if (!playsIn(store, deviceId, found, mix)) return out;
  const mixer = store.mixer(deviceId, mix);
  for (const leg of [leftLeg, copyLeg]) {
    if (returnPlays(store, deviceId, mix, leg.chain)) out.push({ text: `The effect return ${label({ group: afxOut, channel: leg.chain })} also plays in ${mixName}, so ${leg.carries} is heard twice.`, fix: { line: `Mute the effect return ${label({ group: afxOut, channel: leg.chain })} in ${mixName}`, run: () => mixer.setAlone(leg.chain, { mute: true }) } });
  }
  if (dryPlays(store, deviceId, side, mix) && side !== undefined) {
    out.push({
      text: `${name(side)} plays dry in ${mixName} as well, ahead of the side strips, so the side does not cancel in mono.`,
      fix: {
        line: `Mute ${name(side)} in ${mixName}`,
        run: () => {
          mixer.setAlone(side.slot, { mute: true });
          const current = decodeOf(store, deviceId, groupId)?.decode;
          if (current !== undefined && !(current.side_muted ?? []).includes(mix)) channels.setMidSide(groupId, { ...current, side_muted: [...(current.side_muted ?? []), mix] });
        },
      },
    });
  }
  for (const c of [mid, left, inverted]) {
    if (!channels.strip(c, mix).inMix) out.push({ text: `${name(c)} is not in ${mixName}, so the decode is incomplete here.`, fix: { line: `Send ${name(c)} to ${mixName} again`, run: () => joinMix(store, deviceId, c, mix) } });
  }
  const pans: [MixerChannel, number, string][] = [[left, PAN_MIN, "hard left"], [inverted, PAN_MAX, "hard right"], [mid, PAN_CENTRE, "centre"]];
  for (const [c, pan, where] of pans) {
    const now = panOf(store, deviceId, mix, c.slot);
    if (now === pan) continue;
    out.push({ text: `${name(c)} is panned ${panWords(now)} in ${mixName}, not ${where}${pan === PAN_CENTRE ? ", so the image leans to one side" : ", so left and right no longer decode"}.`, fix: { line: `Pan ${name(c)} ${pan === PAN_CENTRE ? "to the centre" : where} in ${mixName}`, run: () => mixer.setPan(c.slot, pan) } });
  }
  const first = mixer.strip(left.slot).value;
  const copy = mixer.strip(inverted.slot).value;
  if (first.level !== copy.level) out.push({ text: `${name(left)} is at ${formatLevel(first.level)} and ${name(inverted)} at ${formatLevel(copy.level)} in ${mixName}: the two must match.`, fix: { line: `Set ${name(inverted)} to ${formatLevel(first.level)} in ${mixName}`, run: () => mixer.setLevel(inverted.slot, first.level) } });
  if (first.mute !== copy.mute) out.push({ text: `${name(first.mute ? left : inverted)} is muted in ${mixName} and ${name(first.mute ? inverted : left)} is not, so only one side plays.`, fix: { line: `${first.mute ? "Mute" : "Unmute"} ${name(inverted)} in ${mixName}`, run: () => mixer.setAlone(inverted.slot, { mute: first.mute }) } });
  return out;
}

/** The plan that mends everything `problems` finds in a mix, or a refusal when nothing it found can be mended here. */
export function repairPlan(store: Store, deviceId: string, groupId: string, mix: number): Planned {
  const found = decodeOf(store, deviceId, groupId);
  if (found === undefined) return refuse("This decode is gone.");
  const fixes = problems(store, deviceId, groupId, mix).flatMap((problem) => (problem.fix === undefined ? [] : [problem.fix]));
  if (fixes.length === 0) return refuse("There is nothing here that can be put back.");
  return { ok: true, plan: plan(fixes, recordedNote(store, deviceId, found.decode)) };
}

/**
 * A decode's Width in one mix: how far the side strips sit above the mid, in dB (0 when they are
 * level, negative for narrower). Undefined where it does not play. Reading it is reactive.
 */
export function width(store: Store, deviceId: string, groupId: string, mix: number): number | undefined {
  const found = decodeOf(store, deviceId, groupId);
  if (found === undefined || found.mid === undefined || found.left === undefined || !playsIn(store, deviceId, found, mix)) return undefined;
  const mixer = store.mixer(deviceId, mix);
  return mixer.strip(found.mid.slot).value.level - mixer.strip(found.left.slot).value.level;
}

/**
 * Sets the Width: the hard-left side strip moves to the mid's level plus `value` dB, held inside
 * the fader's range, and its link moves the inverted copy with it. The mid is never moved.
 */
export function setWidth(store: Store, deviceId: string, groupId: string, mix: number, value: number): void {
  const found = decodeOf(store, deviceId, groupId);
  if (found === undefined || found.mid === undefined || found.left === undefined) return;
  const mixer = store.mixer(deviceId, mix);
  mixer.setLevel(found.left.slot, mixer.strip(found.mid.slot).peek().level - Math.round(value));
}

/** What a strip is in a decode, and what is wrong with that decode in the strip's mix. */
export interface StripRole {
  readonly role: Role;
  /** The decode's group. */
  readonly group: string;
  readonly name: string;
  /** Every problem's text, or undefined for a sound decode. */
  readonly warning: string | undefined;
}

/**
 * What the Mixer's strips and group bands ask about decodes (`midSideGuard` in element.ts), on the
 * page and in the dock: reactive, and fetched only once the workspace holds a decode.
 */
export interface MidSideGuard {
  strip(deviceId: string, mix: number, slot: number): StripRole | undefined;
  /** A decode's problems in a mix as one line, or undefined. */
  group(deviceId: string, mix: number, groupId: string): string | undefined;
}

export function midSideGuard(store: Store): MidSideGuard {
  const warning = (deviceId: string, mix: number, groupId: string) => {
    const found = problems(store, deviceId, groupId, mix).map((problem) => problem.text);
    return found.length === 0 ? undefined : found.join(" ");
  };
  return {
    strip: (deviceId, mix, slot) => {
      if (store.topology(deviceId) === undefined) return undefined;
      for (const found of decodes(store, deviceId)) {
        // A decode saved by an earlier way played the side channel itself hard left.
        const side: Role = found.left === undefined ? "side" : "dry";
        const role: Role | undefined = found.mid?.slot === slot ? "mid" : found.left?.slot === slot ? "side" : found.side?.slot === slot ? side : found.inverted?.slot === slot ? "inverted" : undefined;
        if (role !== undefined) return { role, group: found.group.id, name: found.group.name, warning: warning(deviceId, mix, found.group.id) };
      }
      return undefined;
    },
    group: (deviceId, mix, groupId) => (store.topology(deviceId) === undefined ? undefined : warning(deviceId, mix, groupId)),
  };
}
