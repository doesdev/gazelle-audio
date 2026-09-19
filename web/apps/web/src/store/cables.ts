// Digital cables between devices: the user says that one device's S/PDIF or
// ADAT output is plugged into another's input. A cable is a fact about the room, kept in the
// workspace. It never routes, clocks or links anything; it lets the app say
//   - what feeds a digital output, pair by pair (a mix, or a source played bit for bit, since
//     neither model has a level of its own for its digital outputs), and route a different
//     mix or source there: a real routing change on the device that owns the port, through its
//     RoutingModel, which reads the group before it writes (the user's choice);
//   - where a digital input's signal comes from ("from Drum rack ADAT out 3 ← PREAMP 3 (Snare)");
//   - when the two ends disagree: sample rates, a receiver that is not locked, or signal leaving the
//     sender with none arriving.
// Declared cables are checked here as the server checks them, with its wording.

import { computed, type ReadonlySignal } from "../core/signal.ts";
import type { Cable, CableEnd, DigitalPort, MixerChannel, RouteSource, Topology } from "gazelle-audio-client";
import type { RouteSlot, RoutingModel } from "./routing.ts";

/** A meter byte at or below this (dB below full scale) is signal: -60 dBFS, as the device cards count it. */
const SIGNAL_BELOW_FS = 60;

export type OutputPort = "SPDIF_OUT" | "ADAT_OUT";
export type InputPort = "SPDIF_IN" | "ADAT_IN";

const SENDS: readonly OutputPort[] = ["SPDIF_OUT", "ADAT_OUT"];
const RECEIVES: readonly InputPort[] = ["SPDIF_IN", "ADAT_IN"];
const PORT_NAMES: Readonly<Record<DigitalPort, string>> = { SPDIF_OUT: "S/PDIF out", ADAT_OUT: "ADAT out", SPDIF_IN: "S/PDIF in", ADAT_IN: "ADAT in" };

/** A port's name as people say it: "ADAT out". */
export const portName = (port: DigitalPort): string => PORT_NAMES[port];

/** Channels `first` to `last`, counted from 1, as a label says them: "3", "1 and 2", "9 to 16". */
export const channelSpan = (first: number, last: number): string =>
  first === last ? `${first}` : last === first + 1 ? `${first} and ${last}` : `${first} to ${last}`;

/** Channels one cable or port strip carries: a stereo pair for S/PDIF, eight for an ADAT port. */
export const portWidth = (port: DigitalPort): number => (port.startsWith("ADAT") ? 8 : 2);

/** One stereo pair of an output port and what feeds it. */
export interface PortPair {
  /** Its left channel, in the whole port. */
  channel: number;
  /** "1/2". */
  label: string;
  /** "Mix 4", "USB 1 PLAY 1/2", "PREAMP 3 + MUTE", "Muted", or "Not read". */
  text: string;
  /** The mix whose left and right feed it, in order. */
  mix: number | undefined;
  /** Fed by something other than a mix or silence: bit for bit, with no level on this device. */
  direct: boolean;
  /** Whether the device's routing to the port has been read (or written by this app). */
  known: boolean;
}

/** What `routePair` can send to a pair. */
export type RouteChoice = { mix: number } | { group: number; channel: number } | { mute: true };

export interface Provenance {
  cable: Cable;
  /** The sender's channel, in its whole port. */
  channel: number;
  text: string;
}

export interface CablesContext {
  cables: ReadonlySignal<readonly Cable[]>;
  edit(update: (cables: Cable[]) => Cable[]): boolean;
  /** An attached device's model, or undefined. */
  model(deviceId: string): { family: "quadro" | "studio"; topology: Topology } | undefined;
  routing(deviceId: string): Pick<RoutingModel, "destination" | "routeMany" | "mute">;
  /** A device's mix name and its mixer channels, for labels. */
  mixName(deviceId: string, mix: number): string;
  mixerChannels(deviceId: string): readonly MixerChannel[];
  deviceName(deviceId: string): string;
  /** What the device reports about its clock, or undefined before it reports. Reactive. */
  clock(deviceId: string): { rate: number; locked: boolean } | undefined;
  /** Whether the receiving device's S/PDIF converter is on, or undefined for a model without one. Reactive. */
  spdifSrc(deviceId: string): boolean | undefined;
  /** An input's meter byte, or undefined without one. Reactive. */
  inputLevel(deviceId: string, source: RouteSource): number | undefined;
  rateNames: readonly string[];
}

let nextId = 0;

export class CablesModel {
  readonly list: ReadonlySignal<readonly Cable[]>;
  readonly #context: CablesContext;

  constructor(context: CablesContext) {
    this.#context = context;
    this.list = computed(() => context.cables.value);
  }

  /** Declares a cable; returns its id. Throws with the server's reason for one it would refuse. */
  declare(from: CableEnd, to: CableEnd, channels: number): string | undefined {
    const kind = SENDS.indexOf(from.port as OutputPort);
    if (kind < 0) throw new RangeError(`from must be ${SENDS.join(" or ")}, not ${JSON.stringify(from.port)}`);
    if (!RECEIVES.includes(to.port as InputPort)) throw new RangeError(`to must be ${RECEIVES.join(" or ")}, not ${JSON.stringify(to.port)}`);
    if (RECEIVES[kind] !== to.port) throw new RangeError(`${from.port} cannot feed ${to.port}`);
    if (from.device_id === to.device_id) throw new RangeError("a cable joins two devices");
    const most = portWidth(from.port);
    if (!Number.isInteger(channels) || channels < 1 || channels > most) throw new RangeError(`it needs 1..${most} channels, not ${channels}`);
    for (const end of [from, to]) this.#checkRange(end.device_id, end.port, end.first, channels);
    const taken = this.list.peek().map((c) => c.id);
    let id: string;
    do id = `c${(nextId++).toString(36)}${Math.random().toString(36).slice(2, 6)}`;
    while (taken.includes(id));
    const cable: Cable = { id, from: { device_id: from.device_id, port: from.port, first: from.first }, to: { device_id: to.device_id, port: to.port, first: to.first }, channels };
    return this.#context.edit((cables) => [...cables, cable]) ? id : undefined;
  }

  remove(id: string): boolean {
    if (!this.list.peek().some((c) => c.id === id)) return false;
    return this.#context.edit((cables) => cables.filter((c) => c.id !== id));
  }

  /** "Drum rack ADAT out 1 to 8 → Zen Quadro ADAT in 1 to 8". Reactive. */
  label(cable: Cable): string {
    const end = (e: CableEnd) => `${this.#context.deviceName(e.device_id)} ${portName(e.port)} ${channelSpan(e.first + 1, e.first + cable.channels)}`;
    return `${end(cable.from)} → ${end(cable.to)}`;
  }

  /** The digital ports a device has on one side, S/PDIF first. */
  portsOf(deviceId: string, side: "out" | "in"): DigitalPort[] {
    return (side === "out" ? SENDS : RECEIVES).filter((port) => this.portChannels(deviceId, port) > 0);
  }

  /** How many channels a device's port has: 0 when it has none or its model is unknown. */
  portChannels(deviceId: string, port: DigitalPort): number {
    const topology = this.#context.model(deviceId)?.topology;
    if (topology === undefined) return 0;
    return (port.endsWith("_OUT") ? topology.outputs : topology.inputs).filter((g) => g.type === port).reduce((sum, g) => sum + g.channels, 0);
  }

  /** The port's position among the topology's destinations (outputs) or sources (inputs). */
  position(deviceId: string, port: DigitalPort): number {
    const topology = this.#context.model(deviceId)?.topology;
    return topology === undefined ? -1 : (port.endsWith("_OUT") ? topology.outputs : topology.inputs).findIndex((g) => g.type === port);
  }

  /** What feeds each pair of an output port's strip, from `first` (a port strip is 2 or 8 channels). Reactive. */
  feed(deviceId: string, port: OutputPort, first: number): PortPair[] {
    const model = this.#context.model(deviceId);
    const destination = this.position(deviceId, port);
    if (model === undefined || destination < 0) return [];
    const count = Math.min(portWidth(port), this.portChannels(deviceId, port) - first);
    const slots = this.#context.routing(deviceId).destination(destination).value;
    const { topology } = model;
    const mixSources = topology.mixers.outputGroups.map((id) => topology.inputs.findIndex((g) => g.id === id));
    const mute = this.#context.routing(deviceId).mute;
    const pairs: PortPair[] = [];
    for (let channel = first; channel < first + count; channel += 2) {
      const stereo = channel + 1 < first + count;
      const label = stereo ? `${channel + 1}/${channel + 2}` : String(channel + 1);
      const left = slots?.[channel];
      const right = stereo ? slots?.[channel + 1] : left;
      if (left === undefined || right === undefined) {
        pairs.push({ channel, label, text: "Not read", mix: undefined, direct: false, known: false });
        continue;
      }
      const mix = mixSources.indexOf(left.source);
      if (mix >= 0 && left.channel === 0 && (!stereo || (right.source === left.source && right.channel === 1))) {
        pairs.push({ channel, label, text: this.#context.mixName(deviceId, mix), mix, direct: false, known: true });
      } else if (left.source === mute && right.source === mute) {
        pairs.push({ channel, label, text: "Muted", mix: undefined, direct: false, known: true });
      } else {
        const group = topology.inputs[left.source];
        const text =
          stereo && right.source === left.source && right.channel === left.channel + 1 && group !== undefined
            ? `${group.name} ${left.channel + 1}/${left.channel + 2}`
            : stereo
              ? `${this.#slotLabel(deviceId, topology, left)} + ${this.#slotLabel(deviceId, topology, right)}`
              : this.#slotLabel(deviceId, topology, left);
        pairs.push({ channel, label, text, mix: undefined, direct: true, known: true });
      }
    }
    return pairs;
  }

  /** What can be routed to a port's pair on a device: its mixes by name, and its sources in pairs. Mixes' own outputs and MUTE are left out of the sources. */
  routeChoices(deviceId: string): { mixes: { mix: number; label: string }[]; sources: { group: number; channel: number; label: string }[] } {
    const topology = this.#context.model(deviceId)?.topology;
    if (topology === undefined) return { mixes: [], sources: [] };
    const mixes = Array.from({ length: topology.mixers.count }, (_, mix) => ({ mix, label: this.#context.mixName(deviceId, mix) }));
    const sources: { group: number; channel: number; label: string }[] = [];
    topology.inputs.forEach((group, position) => {
      if (group.type === "MIXER_OUT" || group.type === "MUTE") return;
      for (let channel = 0; channel < group.channels; channel += 2) {
        const label = channel + 1 < group.channels ? `${group.name} ${channel + 1}/${channel + 2}` : group.channels > 1 ? `${group.name} ${channel + 1}` : group.name;
        sources.push({ group: position, channel, label });
      }
    });
    return { mixes, sources };
  }

  /**
   * Routes a mix's left and right, a source pair, or MUTE to the pair of an output port starting at
   * `channel`: one `set_routing` of the port's group on the device that owns it, read fresh first.
   * Throws for a pair or choice the device does not have; resolves true when it was sent.
   */
  routePair(deviceId: string, port: OutputPort, channel: number, choice: RouteChoice): Promise<boolean> {
    const model = this.#context.model(deviceId);
    const destination = SENDS.includes(port) ? this.position(deviceId, port) : -1;
    const channels = this.portChannels(deviceId, port);
    if (model === undefined || destination < 0) throw new RangeError(`${deviceId} has no ${port}`);
    // Past the port, RoutingModel refuses the channel itself.
    if (!Number.isInteger(channel) || channel % 2 !== 0) throw new RangeError(`${port} pairs start on even channels, not ${channel}`);
    const { topology } = model;
    let left: RouteSlot | null;
    let right: RouteSlot | null;
    if ("mute" in choice) {
      left = right = null;
    } else if ("mix" in choice) {
      const id = topology.mixers.outputGroups[choice.mix];
      const source = id === undefined ? -1 : topology.inputs.findIndex((g) => g.id === id);
      if (source < 0) throw new RangeError(`${deviceId} has no mix ${choice.mix}`);
      left = { source, channel: 0 };
      right = { source, channel: 1 };
    } else {
      const group = topology.inputs[choice.group];
      if (group === undefined || !Number.isInteger(choice.channel) || choice.channel < 0 || choice.channel >= group.channels) throw new RangeError(`${deviceId} has no source ${choice.group}:${choice.channel}`);
      left = { source: choice.group, channel: choice.channel };
      right = { source: choice.group, channel: Math.min(group.channels - 1, choice.channel + 1) };
    }
    const changes = [{ channel, source: left }];
    if (channel + 1 < channels) changes.push({ channel: channel + 1, source: right });
    return this.#context.routing(deviceId).routeMany(destination, changes);
  }

  /** The cable that brings a device's digital input channel in, and where from. Reactive. */
  provenance(deviceId: string, port: InputPort, channel: number): Provenance | undefined {
    const cable = this.#cableInto(deviceId, port, channel);
    if (cable === undefined) return undefined;
    const sent = cable.from.first + (channel - cable.to.first);
    let text = `from ${this.#context.deviceName(cable.from.device_id)} ${portName(cable.from.port)} ${sent + 1}`;
    const source = this.#sentFrom(cable, sent);
    const topology = this.#context.model(cable.from.device_id)?.topology;
    if (source !== undefined && topology !== undefined) {
      text += ` ← ${this.#slotLabel(cable.from.device_id, topology, source)}`;
      const named = this.#context.mixerChannels(cable.from.device_id).find((c) => c.name !== "" && c.source?.group === source.source && c.source.channel === source.channel);
      if (named !== undefined) text += ` (${named.name})`;
    }
    return { cable, channel: sent, text };
  }

  /** The sender's routing group a provenance needs, so a page can read it once. */
  sendersToRead(deviceId: string, port: InputPort, channel: number): { deviceId: string; destination: number } | undefined {
    const cable = this.#cableInto(deviceId, port, channel);
    if (cable === undefined) return undefined;
    const destination = this.position(cable.from.device_id, cable.from.port);
    return destination < 0 ? undefined : { deviceId: cable.from.device_id, destination };
  }

  /**
   * What is wrong along a cable, in sentences, from the two devices' reports: the sample rates
   * differ, the receiver is not locked, or signal leaves the sender with none arriving. Empty
   * when nothing is known to be wrong. Display only. Reactive.
   */
  health(cable: Cable): string[] {
    const sender = cable.from.device_id;
    const receiver = cable.to.device_id;
    const problems: string[] = [];
    const sent = this.#context.clock(sender);
    const received = this.#context.clock(receiver);
    // The receiver's sample-rate converter, on the S/PDIF input it converts: with it on, this cable
    // may carry another rate and the receiver need not follow the sender's clock, so neither is
    // worth saying (the user, 2026-09-19). A real failure still shows as signal that never arrives.
    const converted = cable.to.port === "SPDIF_IN" && this.#context.spdifSrc(receiver) === true;
    if (!converted && sent !== undefined && received !== undefined && sent.rate !== received.rate) {
      const rate = (index: number) => this.#context.rateNames[index] ?? `rate ${index}`;
      problems.push(`The sample rates differ: ${rate(sent.rate)} on ${this.#context.deviceName(sender)}, ${rate(received.rate)} on ${this.#context.deviceName(receiver)}.`);
    }
    if (!converted && received !== undefined && !received.locked) problems.push(`${this.#context.deviceName(receiver)} is not locked to its clock.`);
    const input = this.position(receiver, cable.to.port);
    for (let i = 0; i < cable.channels && input >= 0; i++) {
      const source = this.#sentFrom(cable, cable.from.first + i);
      if (source === undefined) continue;
      const leaving = this.#context.inputLevel(sender, { group: source.source, channel: source.channel });
      const arriving = this.#context.inputLevel(receiver, { group: input, channel: cable.to.first + i });
      if (leaving !== undefined && arriving !== undefined && leaving <= SIGNAL_BELOW_FS && arriving > SIGNAL_BELOW_FS) {
        problems.push(`Signal leaves ${this.#context.deviceName(sender)} ${portName(cable.from.port)} ${cable.from.first + i + 1} but none arrives at ${this.#context.deviceName(receiver)} ${portName(cable.to.port)} ${cable.to.first + i + 1}: check the cable, the routing and the clock.`);
        break;
      }
    }
    return problems;
  }

  #cableInto(deviceId: string, port: InputPort, channel: number): Cable | undefined {
    return this.list.value.find((c) => c.to.device_id === deviceId && c.to.port === port && channel >= c.to.first && channel < c.to.first + c.channels);
  }

  /** The source the sender routes to one channel of the cable's output port, once its routing is known. */
  #sentFrom(cable: Cable, channel: number): RouteSlot | undefined {
    const destination = this.position(cable.from.device_id, cable.from.port);
    return destination < 0 ? undefined : this.#context.routing(cable.from.device_id).destination(destination).value?.[channel];
  }

  /** A routed source as people know it: a mix's side by the mix's name ("Music L"), else "PREAMP 3". */
  #slotLabel(deviceId: string, topology: Topology, slot: RouteSlot): string {
    const group = topology.inputs[slot.source];
    if (group === undefined) return `Source ${slot.source}:${slot.channel + 1}`;
    const mix = topology.mixers.outputGroups.indexOf(group.id);
    if (mix >= 0) return `${this.#context.mixName(deviceId, mix)} ${slot.channel === 0 ? "L" : "R"}`;
    return group.channels > 1 ? `${group.name} ${slot.channel + 1}` : group.name;
  }

  #checkRange(deviceId: string, port: DigitalPort, first: number, count: number): void {
    const model = this.#context.model(deviceId);
    if (model === undefined) return;
    const channels = this.portChannels(deviceId, port);
    if (channels === 0) throw new RangeError(`the ${model.family} has no ${port}`);
    if (!Number.isInteger(first) || first < 0 || first + count > channels) throw new RangeError(`the ${model.family}'s ${port} has channels 0..${channels - 1}, not ${first}..${first + count - 1}`);
  }
}
