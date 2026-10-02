// Monitoring a mid and a side microphone as stereo in a hardware mix (the owner, 2026-10-02): the
// two are recorded raw, for decoding later, and heard decoded while tracking. Left is mid plus side
// and right is mid minus side, so the mid channel is centred, the side channel is panned hard left,
// and an inverted copy of the side signal is panned hard right at the same level.
//
// A strip has level, pan, mute and solo but no polarity switch, on either model, and nothing in the
// mixer or routing commands inverts. So the inverted copy is a channel of its own on a source that
// does invert, and there are two:
//
// - **A second preamp** fed by a split (a Y cable) of the side microphone, with its polarity switch
//   (the Inputs page's phase button) the opposite of the side preamp's, and its gain linked to it.
//   It always works; it costs a preamp and a cable, and that preamp is one more recorded input.
// - **An effect chain** fed by the side input as well, holding one effect that has a polarity
//   switch (the BAE-1073, BAE-1023 and BAE-1084 equaliser models have one, on both interfaces) with
//   every other setting at its starting value; the chain's output is the copy. It costs nothing,
//   but whether an effect chain delays or colours what passes through it is not in anything the
//   vendor's software says, and has not been measured: a delay of even a sample or two against the
//   dry side signal combs the decode. So it is offered second and says so, and the second preamp is
//   the default.
//
// Everything here follows the pattern of tidying a mix (`mix-tidy.ts`): a plan is worked out and
// shown line by line, and nothing is sent until it is confirmed. Each line carries the change it
// describes, so what is listed and what is done cannot drift apart. The decode is kept on its group
// in the workspace (`MidSide`), with what setting it up changed, so removing it puts that back.
// `problems` is the guard: what would quietly break the decode, each with the change that mends it.
// Only the Mixer page's lazy elements use this, so it travels in their chunk.

import type { Link, MidSide, MixerChannel, MixerGroup, RouteSource } from "gazelle-audio-client";

import { EFFECT_NAMES } from "./effect-catalogue.ts";
import { PREAMP_TYPES } from "./inputs.ts";
import { formatLevel, formatPan, LEVEL_MAX, PAN_CENTRE, PAN_MAX, PAN_MIN } from "./mixer.ts";
import type { Store } from "./store.ts";

/** The effects with a polarity switch, on both models, in the order they are tried: its parameter is `phase_inv`. */
export const INVERTING_EFFECTS: readonly number[] = [7, 24, 25];
const POLARITY = "phase_inv";

/** Where the widest Width goes either way, in dB: the side strips this far above or below the mid. */
export const WIDTH_RANGE = 30;

export type Role = "mid" | "side" | "inverted";

/** How the inverted copy is made, as chosen in the confirm. */
export type Way = { via: "preamp"; preamp: number } | { via: "effect"; chain: number; type: number };

/** One change of a plan: the line the confirm shows, and the change itself. */
interface Step {
  readonly line: string;
  run(): unknown;
}

export interface Plan {
  /** One line per change, in the order they are made. */
  readonly lines: readonly string[];
  /** What the person should know that is not a change: what is recorded, and what has not been checked. */
  readonly notes: readonly string[];
  /** Makes every change, in order. Rejects with what stopped it; the changes before that stay made. */
  apply(): Promise<void>;
}

export type Planned = { ok: true; plan: Plan } | { ok: false; why: string };

/** A decode as the layout holds it: its group and the three channels, each undefined once removed by hand. */
export interface Decode {
  readonly group: MixerGroup;
  readonly decode: MidSide;
  readonly mid: MixerChannel | undefined;
  readonly side: MixerChannel | undefined;
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
    inputs: store.inputs(deviceId),
    effects: store.effects(deviceId),
    routing: store.routing(deviceId),
    preampGroup: topology.inputs.findIndex((g) => g.type === "PREAMP"),
    afxOut: topology.inputs.findIndex((g) => g.type === "AFX_OUT"),
    afxIn: topology.outputs.findIndex((g) => g.type === "AFX_IN"),
    /** The Quadro keeps AFX Out k on slot k of every mix, outside the layout: its effect returns. */
    returns: store.channels(deviceId).firstSlot,
  };
}

/** The preamp a source is, or undefined for any other input. */
function preampOf(store: Store, deviceId: string, source: RouteSource | undefined): number | undefined {
  const { preampGroup } = parts(store, deviceId);
  return source !== undefined && source.group === preampGroup ? source.channel : undefined;
}

/** A strip's pan as the page shows it: where it returns to, while its mix is mono. */
function panOf(store: Store, deviceId: string, mix: number, slot: number): number {
  const mixer = store.mixer(deviceId, mix);
  return mixer.monoPan(slot) ?? mixer.strip(slot).value.pan;
}

const panWords = (pan: number) => (pan === PAN_MIN ? "hard left" : pan === PAN_MAX ? "hard right" : pan === PAN_CENTRE ? "centre" : formatPan(pan));

/** The source the inverted copy plays from. */
export function invertedSource(store: Store, deviceId: string, decode: Pick<MidSide, "via" | "preamp" | "chain">): RouteSource {
  const { preampGroup, afxOut } = parts(store, deviceId);
  return decode.via === "preamp" ? { group: preampGroup, channel: decode.preamp ?? 0 } : { group: afxOut, channel: decode.chain ?? 0 };
}

/** Every decode in a device's layout. Reading it is reactive. */
export function decodes(store: Store, deviceId: string): Decode[] {
  const layout = store.channels(deviceId).layout.value;
  const channel = (id: string) => layout.channels.find((c) => c.id === id);
  return layout.groups.flatMap((group) => (group.mid_side === undefined ? [] : [{ group, decode: group.mid_side, mid: channel(group.mid_side.mid), side: channel(group.mid_side.side), inverted: channel(group.mid_side.inverted) }]));
}

export function decodeOf(store: Store, deviceId: string, groupId: string): Decode | undefined {
  return decodes(store, deviceId).find((d) => d.group.id === groupId);
}

/** Whether a decode plays in a mix: one of its side strips is set up there. Reactive. */
export function playsIn(store: Store, deviceId: string, found: Decode, mix: number): boolean {
  const channels = store.channels(deviceId);
  return [found.side, found.inverted].some((c) => c !== undefined && channels.strip(c, mix).inMix);
}

/** The ways the inverted copy can be made for a side channel, and why a way cannot be offered. Reactive. */
export interface Ways {
  /** The side channel's own preamp, when its input is one: the second preamp follows its gain. */
  readonly sidePreamp: number | undefined;
  /** Every preamp but the mid's and the side's own; `used` names a channel already on it. */
  readonly preamps: readonly { preamp: number; label: string; used?: string }[];
  readonly effect: { chain: number; type: number; label: string } | { why: string };
}

export function ways(store: Store, deviceId: string, mid: MixerChannel, side: MixerChannel): Ways {
  const { topology, channels, inputs, effects, routing, preampGroup, afxOut, afxIn } = parts(store, deviceId);
  const layout = channels.layout.value;
  const sidePreamp = preampOf(store, deviceId, side.source);
  const midPreamp = preampOf(store, deviceId, mid.source);
  const preamps = Array.from({ length: inputs.preampCount }, (_, preamp) => preamp)
    .filter((preamp) => preamp !== sidePreamp && preamp !== midPreamp)
    .map((preamp) => {
      const on = layout.channels.find((c) => c.source?.group === preampGroup && c.source.channel === preamp);
      return { preamp, label: channels.sourceLabel({ group: preampGroup, channel: preamp }), ...(on === undefined ? {} : { used: channels.displayName(on) }) };
    });

  const effect = ((): Ways["effect"] => {
    const names = INVERTING_EFFECTS.map((type) => EFFECT_NAMES[effects.family].get(type) ?? `Effect ${type}`).join(", ");
    if (afxOut < 0 || afxIn < 0) return { why: "This model has no effect chains." };
    const chains = effects.chains.value;
    if (chains === undefined || !chains.some((chain) => chain.known)) return { why: "The effect chains have not been read from the device, so it is not known which is free." };
    const feeding = routing.destination(afxIn).value;
    if (feeding === undefined) return { why: "What feeds the effect chains has not been read from the device." };
    const outputs = topology.inputs[afxOut]?.channels ?? 0;
    const free = chains.filter((chain) => {
      const fed = feeding[chain.index];
      const unfed = fed !== undefined && (fed.source === routing.mute || same({ group: fed.source, channel: fed.channel }, side.source));
      return chain.known && chain.slots.length === 0 && !chain.linked && chain.index < outputs && unfed && !layout.channels.some((c) => c.source?.group === afxOut && c.source.channel === chain.index);
    });
    if (free.length === 0) return { why: "Every effect chain holds an effect, is linked, is fed by another input or has a channel on its output. This way needs an empty, unused chain." };
    for (const chain of free) {
      const offers = effects.offers(chain.index);
      const type = INVERTING_EFFECTS.find((t) => offers.some((o) => o.type === t && o.unavailable === undefined));
      if (type !== undefined) return { chain: chain.index, type, label: `${EFFECT_NAMES[effects.family].get(type) ?? `Effect ${type}`} in ${chain.name}` };
    }
    return { why: `No free instance of ${names} is left, and those are the effects with a polarity switch.` };
  })();
  return { sidePreamp, preamps, effect };
}

/** What every plan says about the recording, so nobody wonders whether the tracks are decoded. */
function recordedNote(store: Store, deviceId: string, decode: Pick<MidSide, "mid_source" | "side_source" | "via" | "preamp">): string[] {
  const { channels, preampGroup } = parts(store, deviceId);
  const notes = [`What is recorded does not change: ${channels.sourceLabel(decode.mid_source)} and ${channels.sourceLabel(decode.side_source)} reach your DAW raw, as they do now, for decoding later. Only the mix hears the decoded stereo.`];
  if (decode.via === "preamp") notes.push(`${channels.sourceLabel({ group: preampGroup, channel: decode.preamp ?? 0 })} is one more input your DAW can record: the side microphone again, inverted. It can be ignored.`);
  return notes;
}

const EFFECT_UNCHECKED =
  "Not checked on a device: whether an effect chain delays or colours what passes through it. If it does, the decode combs: the sound turns thin or hollow, and a voice dead ahead of the mid microphone does not sit in the centre. Listen for that, and use a second preamp if you hear it.";

/** The mixer link that ties the two side strips: one joining both, every member taking the same value. Reactive. */
function sideLink(store: Store, deviceId: string, side: number, inverted: number): Link | undefined {
  const link = store.links.linkOf("mixer", deviceId, side);
  return link !== undefined && link.mode === "absolute" && link.members.some((m) => m.device_id === deviceId && m.channel === inverted) ? link : undefined;
}

/** The preamp link that keeps the second preamp's gain with the side preamp's. Reactive. */
function preampLink(store: Store, deviceId: string, first: number, second: number): Link | undefined {
  const link = store.links.linkOf("preamp", deviceId, first);
  return link !== undefined && link.members.some((m) => m.device_id === deviceId && m.channel === second) ? link : undefined;
}

/** Joins the second preamp to the side preamp's link, or makes one of the two: the side preamp keeps the partners it had. */
function joinPreamps(store: Store, deviceId: string, first: number, second: number): void {
  const existing = store.links.linkOf("preamp", deviceId, first);
  const members = [...(existing?.members ?? [{ device_id: deviceId, channel: first }]).filter((m) => m.device_id !== deviceId || m.channel !== second), { device_id: deviceId, channel: second }];
  store.links.create("preamp", members, existing?.mode ?? "absolute");
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
 * Sets an effect up as the inverter in an empty chain: added, read, then every setting at its
 * starting value with the polarity switch on. Throws when any of that could not be done, having
 * taken the effect out again, so a chain is never left holding an effect that does not invert.
 */
async function invertWith(store: Store, deviceId: string, chain: number, type: number): Promise<number> {
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
      set = effects.setParameter(chain, slot.position, parameter.name, parameter.name === POLARITY ? 1 : (parameter.default ?? 0)) && set;
    }
    if (!set || effects.parameters(type, inst).peek()?.values[POLARITY] !== 1) throw new Error(`${name}'s settings could not be read, so its polarity switch could not be set.`);
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

/**
 * The plan that starts monitoring two channels of a mix as mid and side: every change, in order,
 * and the notes. Refused, with the reason, when the two cannot be a mid and a side, or the way
 * chosen is not there.
 */
export function setupPlan(store: Store, deviceId: string, mix: number, midId: string, sideId: string, way: Way): Planned {
  const { channels, inputs, effects, routing, preampGroup, afxIn, afxOut } = parts(store, deviceId);
  const mid = channels.channel(midId);
  const side = channels.channel(sideId);
  if (mid === undefined || side === undefined || mid.id === side.id) return refuse("Select two channels: the mid microphone's and the side microphone's.");
  const taken = decodes(store, deviceId).find((d) => [d.decode.mid, d.decode.side, d.decode.inverted].some((id) => id === mid.id || id === side.id));
  if (taken !== undefined) return refuse(`${taken.group.name} already uses one of these channels. Remove it first.`);
  if (mid.source === undefined || side.source === undefined || !channels.strip(mid, mix).inMix || !channels.strip(side, mix).inMix) return refuse("Both channels need an input and a place in this mix.");
  if (same(mid.source, side.source)) return refuse("Both channels are on the same input, so there is no side signal to decode.");
  if (channels.layout.peek().channels.length >= 32 - channels.firstSlot) return refuse("Every mixer channel is in use, and the inverted copy needs one.");

  const mixer = store.mixer(deviceId, mix);
  const mixName = channels.mixName(mix);
  const midName = channels.displayName(mid);
  const sideName = channels.displayName(side);
  const copyName = `${sideName} Ø`;
  const sideSource = side.source;
  const midSource = mid.source;
  const sidePreamp = preampOf(store, deviceId, sideSource);
  const decode: MidSide = { mid: mid.id, side: side.id, inverted: "", via: way.via, mid_source: midSource, side_source: sideSource, pans: {} };
  const steps: (Step | undefined)[] = [];
  const notes: string[] = [];
  const label = (source: RouteSource) => channels.sourceLabel(source);

  if (way.via === "preamp") {
    const second = way.preamp;
    if (!Number.isInteger(second) || second < 0 || second >= inputs.preampCount || second === sidePreamp || second === preampOf(store, deviceId, midSource)) return refuse("Choose a preamp other than the mid's and the side's own for the inverted copy.");
    decode.preamp = second;
    const secondName = label({ group: preampGroup, channel: second });
    const now = inputs.preamp(second).value;
    const first = sidePreamp === undefined ? undefined : inputs.preamp(sidePreamp).value;
    const firstName = sidePreamp === undefined ? label(sideSource) : label({ group: preampGroup, channel: sidePreamp });
    if (now.known) decode.phase_invert = now.phaseInvert;
    if (first !== undefined && sidePreamp !== undefined) {
      if (!now.known || !first.known) {
        notes.push(`The preamps' settings have not been reported yet, so ${secondName}'s gain is not matched here: check that it equals ${firstName}'s, or the decode will be lopsided.`);
      } else {
        if (now.type !== first.type) {
          const type = first.type;
          steps.push({ line: `Set ${secondName} to ${PREAMP_TYPES.find((t) => t.value === type)?.label ?? "the same type"}, as ${firstName} is`, run: () => inputs.setType(second, type as 0 | 1 | 2, false) });
        }
        if (now.gain !== first.gain || now.type !== first.type) steps.push({ line: `Set ${secondName}'s gain to ${first.gain} dB, as ${firstName}'s is`, run: () => inputs.setGain(second, first.gain, false) });
      }
      if (preampLink(store, deviceId, sidePreamp, second) === undefined) {
        const left = store.links.linkOf("preamp", deviceId, second);
        const others = store.links.linkOf("preamp", deviceId, sidePreamp);
        if (left !== undefined) decode.displaced_links = [...(decode.displaced_links ?? []), left];
        steps.push({
          line:
            `Link ${secondName} to ${firstName}${others === undefined ? "" : ` and what it is linked with (${memberNames(store, deviceId, others, [sidePreamp])})`}, so its gain, type and 48V follow` +
            (left === undefined ? "" : `; ${secondName} leaves its link with ${memberNames(store, deviceId, left, [second])}`),
          run: () => joinPreamps(store, deviceId, sidePreamp, second),
        });
      }
    } else {
      notes.push(`${firstName} is not one of this interface's preamps, so ${secondName}'s gain cannot follow it: set it by ear until the side signal vanishes from a source dead ahead.`);
    }
    // The copy is inverted when the two polarities differ, whichever way the side preamp's is set.
    const wanted = !(first?.phaseInvert ?? false);
    if (!now.known || now.phaseInvert !== wanted) {
      steps.push({ line: `Switch ${secondName}'s polarity (Ø) ${wanted ? "on" : `off, the opposite of ${firstName}'s`}`, run: () => inputs.setPhaseInvert(second, wanted, false) });
    }
    notes.push(`Feed ${secondName} from a split (a Y cable) of the side microphone.`);
  } else {
    const offered = ways(store, deviceId, mid, side).effect;
    if ("why" in offered) return refuse(offered.why);
    const { chain, type } = way;
    const own = effects.chains.value?.[chain];
    const fed = routing.destination(afxIn).value?.[chain];
    if (own === undefined || !own.known || own.slots.length > 0 || fed === undefined || !INVERTING_EFFECTS.includes(type)) return refuse("That effect chain is not free any more.");
    const name = EFFECT_NAMES[effects.family].get(type) ?? `Effect ${type}`;
    decode.chain = chain;
    decode.effect_type = type;
    decode.effect_inst = 0;
    if (fed.source !== routing.mute && !same({ group: fed.source, channel: fed.channel }, sideSource)) decode.chain_input = { group: fed.source, channel: fed.channel };
    if (!same({ group: fed.source, channel: fed.channel }, sideSource)) {
      steps.push({
        line: `Route ${label(sideSource)} into ${own.name} as well, in place of ${fed.source === routing.mute ? "Mute" : label({ group: fed.source, channel: fed.channel })}`,
        run: async () => {
          if (!(await routing.route(afxIn, chain, { source: sideSource.group, channel: sideSource.channel }))) throw new Error(`${label(sideSource)} could not be routed into ${own.name}.`);
        },
      });
    }
    steps.push({
      line: `Add ${name} to ${own.name}, which is empty, with its polarity switch on and every other setting at its starting value`,
      run: async () => {
        try {
          decode.effect_inst = await invertWith(store, deviceId, chain, type);
        } catch (error) {
          // The effect is out again; so is the route into its chain, the one change made before it.
          await routing.route(afxIn, chain, fed.source === routing.mute ? null : fed);
          throw new UndoneError(`${error instanceof Error ? error.message : String(error)} The chain is as it was, and nothing else was changed.`);
        }
      },
    });
    if (returnPlays(store, deviceId, mix, chain)) {
      decode.returns_muted = [mix];
      steps.push({ line: `Mute the effect return ${label({ group: afxOut, channel: chain })} on slot ${chain + 1} of ${mixName}, which would play the inverted copy a second time`, run: () => mixer.setAlone(chain, { mute: true }) });
    }
    notes.push(EFFECT_UNCHECKED);
  }

  const midPan = panOf(store, deviceId, mix, mid.slot);
  const sidePan = panOf(store, deviceId, mix, side.slot);
  decode.pans[String(mix)] = { mid: midPan, side: sidePan };
  if (midPan !== PAN_CENTRE) steps.push({ line: `Pan ${midName} to the centre in ${mixName} (it is at ${formatPan(midPan)})`, run: () => mixer.setPan(mid.slot, PAN_CENTRE) });
  if (sidePan !== PAN_MIN) steps.push({ line: `Pan ${sideName} hard left in ${mixName} (it is at ${formatPan(sidePan)})`, run: () => mixer.setPan(side.slot, PAN_MIN) });

  const sideStrip = mixer.strip(side.slot).value;
  const copySource = invertedSource(store, deviceId, decode);
  let copySlot = -1;
  steps.push({
    line: `Add a channel, ${copyName}, on ${label(copySource)} to ${mixName}: the inverted copy, panned hard right at ${sideName}'s level, ${formatLevel(sideStrip.level)}${sideStrip.mute ? ", and muted as it is" : ""}`,
    run: async () => {
      const id = await channels.addFed(copySource, mix);
      const added = id === undefined ? undefined : channels.channel(id);
      if (id === undefined || added === undefined) throw new Error("The channel for the inverted copy could not be added.");
      channels.rename(id, copyName);
      decode.inverted = id;
      copySlot = added.slot;
      mixer.setPan(copySlot, PAN_MAX);
      mixer.setLevel(copySlot, mixer.strip(side.slot).peek().level);
      if (mixer.strip(side.slot).peek().mute !== mixer.strip(copySlot).peek().mute) await mixer.setAlone(copySlot, { mute: mixer.strip(side.slot).peek().mute });
    },
  });

  const linked = store.links.linkOf("mixer", deviceId, side.slot);
  if (linked !== undefined) decode.displaced_links = [...(decode.displaced_links ?? []), linked];
  steps.push({
    line: `Link ${sideName} and ${copyName}, so their levels, mutes and solos always match` + (linked === undefined ? "" : `; ${sideName} leaves its link with ${memberNames(store, deviceId, linked, [side.slot])}`),
    run: () => {
      // A free slot can still sit in a link an earlier channel left behind: it is put back with the rest.
      const stale = store.links.linkOf("mixer", deviceId, copySlot);
      if (stale !== undefined && stale.id !== linked?.id) decode.displaced_links = [...(decode.displaced_links ?? []), stale];
      store.links.create("mixer", [{ device_id: deviceId, channel: side.slot }, { device_id: deviceId, channel: copySlot }], "absolute");
    },
  });

  const groupName = `M/S: ${midName}`;
  const was = [channels.groupOf(mid.id), channels.groupOf(side.id)];
  if (was[0] !== undefined) decode.mid_group = was[0].id;
  if (was[1] !== undefined) decode.side_group = was[1].id;
  const leaving = [...new Set(was.flatMap((g) => (g === undefined ? [] : [g.name])))];
  steps.push({
    line: `Group the three as "${groupName}"` + (leaving.length === 0 ? "" : `, out of ${leaving.length === 1 ? "the group" : "the groups"} ${leaving.join(" and ")}`),
    run: () => {
      const id = channels.addGroup(groupName, [mid.id, side.id, decode.inverted]);
      if (id === undefined || !channels.setMidSide(id, decode)) throw new Error("The group could not be saved: Gazelle is not connected.");
    },
  });
  return { ok: true, plan: plan(steps, [...notes, ...recordedNote(store, deviceId, decode)]) };
}

/** How a channel joins a mix it is not in: as a send, or as its main mix when it has none. */
function joinMix(store: Store, deviceId: string, channel: MixerChannel, mix: number): Promise<boolean> {
  const channels = store.channels(deviceId);
  return channel.main_mix === undefined ? channels.setMainMix(channel.id, mix) : channels.setSend(channel.id, mix, true);
}

/** The plan that plays a decode in another mix as well: the same pans and matched side strips there. */
export function addToMixPlan(store: Store, deviceId: string, groupId: string, mix: number): Planned {
  const { channels, afxOut } = parts(store, deviceId);
  const found = decodeOf(store, deviceId, groupId);
  if (found === undefined) return refuse("This decode is gone.");
  const { decode, mid, side, inverted } = found;
  if (mid === undefined || side === undefined || inverted === undefined) return refuse("One of its channels has been removed. Remove the decode and set it up again.");
  if (playsIn(store, deviceId, found, mix) && decode.pans[String(mix)] !== undefined) return refuse("It already plays in this mix.");
  const mixer = store.mixer(deviceId, mix);
  const mixName = channels.mixName(mix);
  const name = (c: MixerChannel) => channels.displayName(c);
  const steps: (Step | undefined)[] = [];
  for (const c of [mid, side, inverted]) if (!channels.strip(c, mix).inMix) steps.push({ line: `Send ${name(c)} to ${mixName} too`, run: () => joinMix(store, deviceId, c, mix) });
  const before = { mid: panOf(store, deviceId, mix, mid.slot), side: panOf(store, deviceId, mix, side.slot) };
  const wanted: [MixerChannel, number][] = [[mid, PAN_CENTRE], [side, PAN_MIN], [inverted, PAN_MAX]];
  for (const [c, pan] of wanted) {
    const now = panOf(store, deviceId, mix, c.slot);
    if (now !== pan) steps.push({ line: `Pan ${name(c)} ${pan === PAN_CENTRE ? "to the centre" : panWords(pan)} in ${mixName} (it is at ${formatPan(now)})`, run: () => mixer.setPan(c.slot, pan) });
  }
  const level = mixer.strip(side.slot).value;
  const copy = mixer.strip(inverted.slot).value;
  if (copy.level !== level.level) steps.push({ line: `Set ${name(inverted)} to ${name(side)}'s level in ${mixName}, ${formatLevel(level.level)}`, run: () => mixer.setLevel(inverted.slot, level.level) });
  if (copy.mute !== level.mute) steps.push({ line: `${level.mute ? "Mute" : "Unmute"} ${name(inverted)} in ${mixName}, as ${name(side)} is`, run: () => mixer.setAlone(inverted.slot, { mute: level.mute }) });
  const muteReturn = decode.via === "effect" && decode.chain !== undefined && returnPlays(store, deviceId, mix, decode.chain);
  const chain = decode.chain ?? 0;
  if (muteReturn) steps.push({ line: `Mute the effect return ${channels.sourceLabel({ group: afxOut, channel: chain })} on slot ${chain + 1} of ${mixName}, which would play the inverted copy a second time`, run: () => mixer.setAlone(chain, { mute: true }) });
  steps.push({
    line: `Remember ${mixName}'s pans, so removing the decode puts them back`,
    run: () => {
      const current = decodeOf(store, deviceId, groupId)?.decode ?? decode;
      channels.setMidSide(groupId, { ...current, pans: { ...current.pans, [String(mix)]: current.pans[String(mix)] ?? before }, ...(muteReturn ? { returns_muted: [...new Set([...(current.returns_muted ?? []), mix])] } : {}) });
    },
  });
  return { ok: true, plan: plan(steps, recordedNote(store, deviceId, decode)) };
}

/**
 * The plan that takes a decode down in every mix it plays in: pans back where they were, the
 * inverted copy's channel removed, the links, the second preamp's polarity or the effect chain put
 * back, and the group removed. A channel already removed by hand is simply passed over.
 */
export function removePlan(store: Store, deviceId: string, groupId: string): Planned {
  const { channels, inputs, effects, routing, preampGroup, afxIn, afxOut } = parts(store, deviceId);
  const found = decodeOf(store, deviceId, groupId);
  if (found === undefined) return refuse("This decode is gone.");
  const { decode, group, mid, side, inverted } = found;
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
  if (inverted !== undefined) {
    if (store.links.linkOf("mixer", deviceId, inverted.slot) !== undefined) steps.push({ line: `Unlink ${side === undefined ? "the side channel" : name(side)} and ${name(inverted)}`, run: () => store.links.removeMember("mixer", deviceId, inverted.slot) });
    steps.push({ line: `Remove the channel ${name(inverted)}, the inverted copy`, run: () => channels.remove(inverted.id) });
  }
  const second = decode.preamp;
  if (decode.via === "preamp" && second !== undefined) {
    const secondName = channels.sourceLabel({ group: preampGroup, channel: second });
    if (store.links.linkOf("preamp", deviceId, second) !== undefined && preampOf(store, deviceId, decode.side_source) !== undefined) steps.push({ line: `Unlink ${secondName} from ${channels.sourceLabel(decode.side_source)}`, run: () => store.links.removeMember("preamp", deviceId, second) });
    const before = decode.phase_invert;
    const now = inputs.preamp(second).value;
    if (before !== undefined && (!now.known || now.phaseInvert !== before)) steps.push({ line: `Switch ${secondName}'s polarity (Ø) back ${before ? "on" : "off"}`, run: () => inputs.setPhaseInvert(second, before, false) });
    // Set up before the preamp had reported anything: where its switch stood is not known, so it is not guessed at.
    if (before === undefined) notes.push(`${secondName}'s polarity (Ø) was not known when this was set up, so it is left as it is: check it on the Inputs page before you use that preamp for something else.`);
  }
  const chain = decode.chain;
  if (decode.via === "effect" && chain !== undefined) {
    const own = effects.chains.value?.[chain];
    const slot = own?.slots.find((s) => s.type === decode.effect_type && s.inst === decode.effect_inst);
    if (own !== undefined && slot !== undefined) steps.push({ line: `Remove ${slot.name} from ${own.name}`, run: () => effects.removeEffect(chain, slot.position) });
    const prior = decode.chain_input;
    const fed = routing.destination(afxIn).value?.[chain];
    const wanted = prior === undefined ? { source: routing.mute, channel: 0 } : { source: prior.group, channel: prior.channel };
    if (fed === undefined || fed.source !== wanted.source || (prior !== undefined && fed.channel !== wanted.channel)) {
      steps.push({ line: `Route ${prior === undefined ? "Mute" : channels.sourceLabel(prior)} into AFX In ${chain + 1} again`, run: () => routing.route(afxIn, chain, prior === undefined ? null : wanted) });
    }
    for (const mix of decode.returns_muted ?? []) {
      if (mix < 0 || mix >= channels.mixCount) continue;
      steps.push({ line: `Unmute the effect return ${channels.sourceLabel({ group: afxOut, channel: chain })} in ${channels.mixName(mix)}`, run: () => store.mixer(deviceId, mix).setAlone(chain, { mute: false }) });
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
 * apart, the copy no longer inverted, a pan off its side, the inputs swapped. Each comes with the
 * change that mends it where there is one. Empty for a sound decode, and for anything not yet known
 * (a preamp's polarity before the first report, an effect's settings before they are read): the
 * guard never warns on a guess. Reading it is reactive.
 */
export function problems(store: Store, deviceId: string, groupId: string, mix: number): Problem[] {
  const { channels, inputs, effects, routing, preampGroup, afxIn, afxOut } = parts(store, deviceId);
  const found = decodeOf(store, deviceId, groupId);
  if (found === undefined) return [];
  const { decode, mid, side, inverted } = found;
  const gone = [mid === undefined ? "mid" : "", side === undefined ? "side" : "", inverted === undefined ? "inverted copy's" : ""].filter((role) => role !== "");
  if (mid === undefined || side === undefined || inverted === undefined) return [{ text: `The ${gone.join(" and the ")} channel has been removed, so nothing is decoded any more. Remove this decode to put the rest back.` }];
  const out: Problem[] = [];
  const name = (c: MixerChannel) => channels.displayName(c);
  const label = (source: RouteSource | undefined) => (source === undefined ? "no input" : channels.sourceLabel(source));
  const mixName = channels.mixName(mix);
  const copySource = invertedSource(store, deviceId, decode);

  // The inputs: both side strips must carry the side microphone, and the mid must not.
  if (same(mid.source, decode.side_source) && same(side.source, decode.mid_source)) {
    out.push({
      text: `${name(mid)} and ${name(side)} have swapped inputs, so the side strips carry the mid microphone.`,
      fix: { line: `Put ${name(mid)} back on ${label(decode.mid_source)} and ${name(side)} back on ${label(decode.side_source)}`, run: async () => void (await Promise.all([channels.setSource(mid.id, decode.mid_source), channels.setSource(side.id, decode.side_source)])) },
    });
  } else {
    if (!same(side.source, decode.side_source)) out.push({ text: `${name(side)} is on ${label(side.source)}, not ${label(decode.side_source)}, which its inverted copy is made from.`, fix: { line: `Put ${name(side)} back on ${label(decode.side_source)}`, run: () => channels.setSource(side.id, decode.side_source) } });
    if (same(mid.source, decode.side_source) || same(mid.source, copySource)) out.push({ text: `${name(mid)} is on ${label(mid.source)}, the side microphone's input.`, fix: { line: `Put ${name(mid)} back on ${label(decode.mid_source)}`, run: () => channels.setSource(mid.id, decode.mid_source) } });
  }
  if (!same(inverted.source, copySource)) out.push({ text: `${name(inverted)} is on ${label(inverted.source)}, not ${label(copySource)}, which carries the inverted copy.`, fix: { line: `Put ${name(inverted)} back on ${label(copySource)}`, run: () => channels.setSource(inverted.id, copySource) } });

  if (sideLink(store, deviceId, side.slot, inverted.slot) === undefined) {
    out.push({
      text: `${name(side)} and ${name(inverted)} are no longer linked, so their levels and mutes can drift apart.`,
      fix: { line: `Link ${name(side)} and ${name(inverted)} again, each taking the same value`, run: () => store.links.create("mixer", [{ device_id: deviceId, channel: side.slot }, { device_id: deviceId, channel: inverted.slot }], "absolute") },
    });
  }

  if (decode.via === "preamp" && decode.preamp !== undefined) {
    const second = decode.preamp;
    const first = preampOf(store, deviceId, decode.side_source);
    const secondName = label({ group: preampGroup, channel: second });
    const now = inputs.preamp(second).value;
    const lead = first === undefined ? undefined : inputs.preamp(first).value;
    const sideInverted = lead?.phaseInvert ?? false;
    if (now.known && (lead === undefined || lead.known) && now.phaseInvert === sideInverted) {
      out.push({
        text: `${secondName}'s polarity (Ø) is ${now.phaseInvert ? "on" : "off"}, the same as ${label(decode.side_source)}'s, so the copy is not inverted: left and right play the same thing.`,
        fix: { line: `Switch ${secondName}'s polarity (Ø) ${sideInverted ? "off" : "on"}`, run: () => inputs.setPhaseInvert(second, !sideInverted, false) },
      });
    }
    if (first !== undefined && lead !== undefined) {
      if (now.known && lead.known && (now.gain !== lead.gain || now.type !== lead.type)) {
        out.push({
          text: `${secondName} is at ${now.gain} dB and ${label(decode.side_source)} at ${lead.gain} dB${now.type === lead.type ? "" : ", on different input types"}, so the two side copies do not match.`,
          fix: {
            line: `Set ${secondName} to ${lead.gain} dB, as ${label(decode.side_source)} is`,
            run: () => {
              if (now.type !== lead.type) inputs.setType(second, lead.type as 0 | 1 | 2, false);
              inputs.setGain(second, lead.gain, false);
            },
          },
        });
      }
      if (preampLink(store, deviceId, first, second) === undefined) out.push({ text: `${secondName}'s gain no longer follows ${label(decode.side_source)}'s.`, fix: { line: `Link ${secondName} to ${label(decode.side_source)} again`, run: () => joinPreamps(store, deviceId, first, second) } });
    }
  }

  if (decode.via === "effect" && decode.chain !== undefined && decode.effect_type !== undefined) {
    const chain = decode.chain;
    const type = decode.effect_type;
    const inst = decode.effect_inst ?? 0;
    const effect = EFFECT_NAMES[effects.family].get(type) ?? `Effect ${type}`;
    const own = effects.chains.value?.[chain];
    if (own !== undefined && own.known) {
      const ours = own.slots.find((s) => s.type === type && s.inst === inst);
      const others = own.slots.filter((s) => s !== ours);
      if (ours === undefined || others.length > 0) {
        out.push({
          text:
            ours === undefined
              ? `${own.name} ${own.slots.length === 0 ? "is empty" : `holds ${others.map((s) => s.name).join(", ")}`}, not the ${effect} that inverts the copy, so the copy is not inverted.`
              : `${own.name} also holds ${others.map((s) => s.name).join(", ")}, so the copy no longer matches the side signal.`,
          fix: {
            line: [others.length === 0 ? "" : `Remove ${others.map((s) => s.name).join(", ")} from ${own.name}`, ours !== undefined ? "" : `${others.length === 0 ? "Add" : "add"} ${effect} to ${own.name} again with its polarity switch on`].filter((part) => part !== "").join(", and "),
            run: async () => {
              // Last first, so the positions of the ones still to go do not move.
              for (const slot of [...others].sort((a, b) => b.position - a.position)) effects.removeEffect(chain, effects.chains.peek()?.[chain]?.slots.find((s) => s.type === slot.type && s.inst === slot.inst)?.position ?? slot.position);
              if (ours !== undefined) return;
              const made = await invertWith(store, deviceId, chain, type);
              const current = decodeOf(store, deviceId, groupId)?.decode;
              if (current !== undefined && made !== current.effect_inst) channels.setMidSide(groupId, { ...current, effect_inst: made });
            },
          },
        });
      } else {
        if (effects.bypass(type, inst).value === true) out.push({ text: `${effect} in ${own.name} is bypassed, so the copy is not inverted.`, fix: { line: `Let ${effect} in ${own.name} process again`, run: () => effects.setBypass(chain, ours.position, false) } });
        const settings = effects.parameters(type, inst).value;
        if (settings !== undefined && settings.known && settings.values[POLARITY] !== 1) {
          out.push({ text: `${effect}'s polarity switch in ${own.name} is off, so the copy is not inverted.`, fix: { line: `Switch ${effect}'s polarity on again`, run: () => effects.setParameter(chain, ours.position, POLARITY, 1) } });
        }
      }
    }
    const fed = routing.destination(afxIn).value?.[chain];
    if (fed !== undefined && !same({ group: fed.source, channel: fed.channel }, decode.side_source)) {
      out.push({
        text: `AFX In ${chain + 1} takes ${fed.source === routing.mute ? "nothing" : label({ group: fed.source, channel: fed.channel })}, not ${label(decode.side_source)}, so the copy is not the side signal.`,
        fix: { line: `Route ${label(decode.side_source)} into AFX In ${chain + 1} again`, run: () => routing.route(afxIn, chain, { source: decode.side_source.group, channel: decode.side_source.channel }) },
      });
    }
    if (playsIn(store, deviceId, found, mix) && returnPlays(store, deviceId, mix, chain)) {
      out.push({ text: `The effect return ${label({ group: afxOut, channel: chain })} also plays in ${mixName}, so the inverted copy is heard twice.`, fix: { line: `Mute the effect return ${label({ group: afxOut, channel: chain })} in ${mixName}`, run: () => store.mixer(deviceId, mix).setAlone(chain, { mute: true }) } });
    }
  }

  // The rest is this mix's own: nothing to say where the decode does not play.
  if (!playsIn(store, deviceId, found, mix)) return out;
  const mixer = store.mixer(deviceId, mix);
  for (const c of [mid, side, inverted]) {
    if (!channels.strip(c, mix).inMix) out.push({ text: `${name(c)} is not in ${mixName}, so the decode is incomplete here.`, fix: { line: `Send ${name(c)} to ${mixName} again`, run: () => joinMix(store, deviceId, c, mix) } });
  }
  const pans: [MixerChannel, number, string][] = [[side, PAN_MIN, "hard left"], [inverted, PAN_MAX, "hard right"], [mid, PAN_CENTRE, "centre"]];
  for (const [c, pan, where] of pans) {
    const now = panOf(store, deviceId, mix, c.slot);
    if (now === pan) continue;
    out.push({ text: `${name(c)} is panned ${panWords(now)} in ${mixName}, not ${where}${pan === PAN_CENTRE ? ", so the image leans to one side" : ", so left and right no longer decode"}.`, fix: { line: `Pan ${name(c)} ${pan === PAN_CENTRE ? "to the centre" : where} in ${mixName}`, run: () => mixer.setPan(c.slot, pan) } });
  }
  const lead = mixer.strip(side.slot).value;
  const copy = mixer.strip(inverted.slot).value;
  if (lead.level !== copy.level) out.push({ text: `${name(side)} is at ${formatLevel(lead.level)} and ${name(inverted)} at ${formatLevel(copy.level)} in ${mixName}: the two must match.`, fix: { line: `Set ${name(inverted)} to ${formatLevel(lead.level)} in ${mixName}`, run: () => mixer.setLevel(inverted.slot, lead.level) } });
  if (lead.mute !== copy.mute) out.push({ text: `${name(lead.mute ? side : inverted)} is muted in ${mixName} and ${name(lead.mute ? inverted : side)} is not, so only one side plays.`, fix: { line: `${lead.mute ? "Mute" : "Unmute"} ${name(inverted)} in ${mixName}`, run: () => mixer.setAlone(inverted.slot, { mute: lead.mute }) } });
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
  if (found === undefined || found.mid === undefined || found.side === undefined || !playsIn(store, deviceId, found, mix)) return undefined;
  const mixer = store.mixer(deviceId, mix);
  return mixer.strip(found.mid.slot).value.level - mixer.strip(found.side.slot).value.level;
}

/**
 * Sets the Width: the side strip moves to the mid's level plus `value` dB, held inside the fader's
 * range, and its link moves the inverted copy with it. The mid is never moved.
 */
export function setWidth(store: Store, deviceId: string, groupId: string, mix: number, value: number): void {
  const found = decodeOf(store, deviceId, groupId);
  if (found === undefined || found.mid === undefined || found.side === undefined) return;
  const mixer = store.mixer(deviceId, mix);
  mixer.setLevel(found.side.slot, mixer.strip(found.mid.slot).peek().level - Math.round(value));
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
        const role: Role | undefined = found.mid?.slot === slot ? "mid" : found.side?.slot === slot ? "side" : found.inverted?.slot === slot ? "inverted" : undefined;
        if (role !== undefined) return { role, group: found.group.id, name: found.group.name, warning: warning(deviceId, mix, found.group.id) };
      }
      return undefined;
    },
    group: (deviceId, mix, groupId) => (store.topology(deviceId) === undefined ? undefined : warning(deviceId, mix, groupId)),
  };
}
