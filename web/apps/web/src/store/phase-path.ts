// A cable dedicated to the aggregate's phase measurement and the clock.
//
// The driver measures each follower's phase at the start of every session: it writes a short burst
// (four samples, about -42 dBFS) into the callback master's USB playback channel named by the
// follower's `phase.master_output`, and listens for it on the follower's USB record channel named by
// `phase.input`. It routes nothing itself, so whether the burst reaches the cable, and only the
// cable, is the interfaces' own routing. On the owner's PC it did not: the playback channel had gone
// into two mixes for a headphone amp and the S/PDIF output was muted, so the burst went towards the
// monitors and the measurement heard nothing.
//
// Dedicating a digital cable from the callback master to a follower gives the measurement a path of
// its own, written once after a confirm that lists every change:
//   - on the master, a USB playback channel nothing uses (the highest free one) goes straight to the
//     cable's first channel, and on S/PDIF the pair's other side is muted, so nothing else crosses
//     the cable (S/PDIF carries its clock whatever it plays, so muting it costs the clock nothing);
//   - on the follower, a USB record channel records the cable's first channel: one that already
//     does, else the highest free one;
//   - the follower's phase setup names those two channels, and a reference measured over another
//     path is taken out, since it would line every session up to a state this path was never in.
// Every write goes through the routing model, which reads the group before it writes it, one
// `set_routing` per group changed, exactly as the Routing page writes. The clock is not touched: a
// clock change interrupts the audio and has a confirm of its own, which the page offers after.
//
// While a cable is dedicated, the Routing page marks its channels and asks before a change that
// would break the path (`breaks`), and the server's readiness answer says when the path is broken
// and offers to put it back. Turning the dedication off leaves the routing and the phase setup as
// they are, since both still work; the page offers to clear the phase setup as well, which gives the
// two channels back to a DAW.

import type { Aggregate, AggregateDevice, AggregateFix, AggregatePhaseSetting, AggregateReason, Cable, CableDedication, DeviceMixer, DigitalPort, Topology, Workspace } from "gazelle-audio-client";
import { masterIndex, phaseFromPicks, phaseSetting, usbGroups, withPhase } from "./aggregate.ts";
import { channelSpan, portName } from "./cables.ts";
import { sourceLabel } from "./channels.ts";
import type { RouteSlot } from "./routing.ts";
import { displayName, type Store } from "./store.ts";

/** Writes routing as the Routing page does: one group of one device, read first. True when it was sent. */
export type RouteWriter = (deviceId: string, destination: number, changes: { channel: number; source: RouteSlot | null }[]) => Promise<boolean>;

/** What the logic here reads, as the page has it now. */
export interface PhasePathContext {
  workspace: Workspace | undefined;
  /** An attached device's model, or undefined. */
  topology(deviceId: string): Topology | undefined;
  /** One routing group of one device as read, or undefined while it has not been. */
  routing(deviceId: string, destination: number): readonly RouteSlot[] | undefined;
  /** Gazelle's name for a device. */
  deviceName(deviceId: string): string;
  /** The clock source a device reports and its model's sources, or undefined before it reports. */
  clock?(deviceId: string): { source: number; sources: readonly string[] } | undefined;
  /** The server is in dry run, where a read answers nothing. */
  dryRun?: boolean;
}

/**
 * The context as the store has it, read reactively: a watch that works a plan or a mark out from it
 * runs again when the workspace, the routing or a clock report changes.
 */
export function phasePathContext(store: Store): PhasePathContext {
  return {
    workspace: store.workspace.value,
    dryRun: store.server.value.dry_run,
    topology: (deviceId) => store.topology(deviceId),
    routing: (deviceId, destination) => (store.topology(deviceId) === undefined ? undefined : store.routing(deviceId).destination(destination).value),
    deviceName: (deviceId) => {
      const device = store.devices.value.find((one) => one.id === deviceId);
      return device === undefined ? (store.workspace.value?.aliases?.[deviceId] ?? deviceId) : displayName(device, store.workspace.value);
    },
    clock: (deviceId) => {
      const card = store.deviceCard(deviceId);
      const sources = store.clock(deviceId)?.sources;
      return card?.reporting !== true || card.clock === undefined || sources === undefined ? undefined : { source: card.clock.source, sources };
    },
  };
}

/** Writes routing through the store's routing model, as the Routing page does. */
export function storeRouteWriter(store: Store): RouteWriter {
  return (deviceId, destination, changes) => store.routing(deviceId).routeMany(destination, changes);
}

/** The device an aggregate entry is: the one chosen, else the one last worked out. */
function entryDevice(device: AggregateDevice): string | undefined {
  return typeof device.device_id === "string" ? device.device_id : device.known?.device_id;
}

/** An aggregate entry's place, by the device it is, or -1. */
function entryOf(config: Aggregate | undefined, deviceId: string): number {
  return (config?.devices ?? []).findIndex((device) => entryDevice(device) === deviceId);
}

/** "S/PDIF" or "ADAT", for a port. */
const portKind = (port: string): "S/PDIF" | "ADAT" => (port.startsWith("ADAT") ? "ADAT" : "S/PDIF");

/** A channel of a stereo pair as L or R, and of anything else as its number from one. */
const side = (channel: number, channels: number): string => (channels === 2 ? (channel === 0 ? "L" : "R") : String(channel + 1));

/** "Quadro S/PDIF out 1 and 2 → Studio+ S/PDIF in 1 and 2", as the Workspace page names a cable. */
export function cableLabel(cable: Cable, deviceName: (deviceId: string) => string): string {
  const end = (e: Cable["from"]) => `${deviceName(e.device_id)} ${portName(e.port)} ${channelSpan(e.first + 1, e.first + cable.channels)}`;
  return `${end(cable.from)} → ${end(cable.to)}`;
}

/** Where a destination channel is, as a person names it: "S/PDIF out L", "Monitor R", "USB REC 24", "Mix 2". */
export function destinationWords(topology: Topology, destination: number, channel: number, layout?: DeviceMixer): string {
  const group = topology.outputs[destination];
  if (group === undefined) return `Destination ${destination}:${channel + 1}`;
  const mix = topology.mixers.inputGroups.indexOf(group.id);
  if (mix >= 0) return mixName(layout, mix);
  const socket = SOCKETS[group.type];
  if (socket !== undefined) return `${socket} ${side(channel, group.channels)}`;
  if (group.type === "HEADPHONES") return `${group.name} ${side(channel, group.channels)}`;
  return group.channels > 1 ? `${group.name} ${channel + 1}` : group.name;
}

const SOCKETS: Readonly<Record<string, string>> = { MONITOR: "Monitor", LINE_OUT: "Line out", SPDIF_OUT: "S/PDIF out", ADAT_OUT: "ADAT out", REAMP: "Reamp" };

/** A mix as a person names it: their name for it, else "Mix 1". */
function mixName(layout: DeviceMixer | undefined, mix: number): string {
  const named = layout?.mixes[mix]?.name?.trim();
  return named === undefined || named === "" ? `Mix ${mix + 1}` : named;
}

/** What a routing slot plays, in the person's words ("Mix 1 L", "S/PDIF in L", "USB 1 PLAY 3"), or undefined for MUTE. */
export function slotWords(topology: Topology, slot: RouteSlot | undefined, layout?: DeviceMixer): string | undefined {
  if (slot === undefined) return undefined;
  const group = topology.inputs[slot.source];
  if (group === undefined || group.type === "MUTE") return undefined;
  const mix = topology.mixers.outputGroups.indexOf(group.id);
  if (mix >= 0) return `${mixName(layout, mix)} ${side(slot.channel, group.channels)}`;
  if (group.type === "SPDIF_IN" || group.type === "ADAT_IN") return `${portKind(group.type)} in ${side(slot.channel, group.channels)}`;
  return sourceLabel(topology, { group: slot.source, channel: slot.channel });
}

const sameSlot = (a: RouteSlot | undefined, b: RouteSlot | undefined): boolean => a !== undefined && b !== undefined && a.source === b.source && a.channel === b.channel;

// ---------------------------------------------------------------------------------------------
// Whether a cable can be dedicated
// ---------------------------------------------------------------------------------------------

/** The two ends of a cable as the aggregate has them: its callback master and a follower. */
export interface CableRoles {
  masterId: string;
  followerId: string;
  /** Their places in the aggregate's setup. */
  masterIndex: number;
  followerIndex: number;
}

export type Eligibility = { ok: true; roles: CableRoles } | { ok: false; why: string };

/**
 * Whether a cable can be dedicated to the phase measurement: a digital cable between two interfaces
 * of the aggregate, leaving the callback master for a follower, both connected, and no other cable
 * into that follower dedicated already. Otherwise why not, in a sentence the page shows beside the
 * setting it greys out.
 */
export function eligibility(cable: Cable, context: PhasePathContext): Eligibility {
  const config = context.workspace?.aggregate;
  const from = cable.from.device_id;
  const to = cable.to.device_id;
  const [sender, receiver] = [context.deviceName(from), context.deviceName(to)];
  if ((config?.devices ?? []).length < 2) return { ok: false, why: "Only a cable between two interfaces of the aggregate can carry its phase measurement: add both on the Aggregate page first." };
  const fromIndex = entryOf(config, from);
  const toIndex = entryOf(config, to);
  if (fromIndex < 0) return { ok: false, why: `${sender} is not in the aggregate, so this cable carries nothing the aggregate measures.` };
  if (toIndex < 0) return { ok: false, why: `${receiver} is not in the aggregate, so this cable carries nothing the aggregate measures.` };
  const master = masterIndex(config);
  if (toIndex === master) return { ok: false, why: `This cable runs into ${receiver}, which drives the callback. A phase is measured from the callback master to another interface, so only a cable leaving ${receiver} can carry it.` };
  if (fromIndex !== master) {
    const masterDevice = master === undefined ? undefined : config?.devices?.[master];
    const masterId = masterDevice === undefined ? undefined : entryDevice(masterDevice);
    return { ok: false, why: `${sender} does not drive the callback${masterId === undefined ? "" : `; ${context.deviceName(masterId)} does`}, and a phase is measured from the callback master.` };
  }
  for (const [id, name] of [[from, sender], [to, receiver]] as const) {
    if (context.topology(id) === undefined) return { ok: false, why: `${name} is not connected, so its routing cannot be read or changed.` };
  }
  const other = (context.workspace?.cables ?? []).find((one) => one.id !== cable.id && one.dedicated !== undefined && one.to.device_id === to);
  if (other !== undefined) return { ok: false, why: `Another cable into ${receiver} is dedicated already: ${cableLabel(other, context.deviceName)}. An interface has one phase measurement.` };
  return { ok: true, roles: { masterId: from, followerId: to, masterIndex: fromIndex, followerIndex: toIndex } };
}

// ---------------------------------------------------------------------------------------------
// The plan: what dedicating writes
// ---------------------------------------------------------------------------------------------

/** One `set_routing`: the channels of one destination group of one device that change. */
export interface GroupWrite {
  deviceId: string;
  destination: number;
  changes: { channel: number; source: RouteSlot | null }[];
}

export interface DedicationPlan {
  cable: Cable;
  roles: CableRoles;
  /** The master's USB playback channel the burst is played into, from zero. */
  output: number;
  /** The follower's USB record channel the burst is heard on, from zero. */
  input: number;
  /** The routing writes, one per group, in the order they are made. Empty when the path is already there. */
  writes: GroupWrite[];
  /** The follower's phase setup as it will be written. */
  phase: AggregatePhaseSetting;
  /** The reference taken out because the path changed, when there was one. */
  referenceCleared?: number;
  /** What the confirm lists, one change or fact to a line. */
  lines: string[];
}

export type PlanResult = { ok: true; plan: DedicationPlan } | { ok: false; why: string };

/** The groups a plan needs read first: every destination group of the master, and the follower's record group. */
export function groupsToRead(roles: CableRoles, context: PhasePathContext): { deviceId: string; destinations: number[] }[] {
  const master = context.topology(roles.masterId);
  const record = usbGroups(context.topology(roles.followerId))?.recordPosition;
  return [
    { deviceId: roles.masterId, destinations: master === undefined ? [] : master.outputs.map((_, at) => at) },
    { deviceId: roles.followerId, destinations: record === undefined ? [] : [record] },
  ];
}

/**
 * Works out what dedicating a cable writes, from the routing as read. Nothing is written here.
 *
 * The playback channel is the one already playing straight to the cable's first channel when it
 * goes nowhere else, so dedicating twice changes nothing; otherwise the highest-numbered one nothing
 * uses: no destination takes it, no Mixer channel is laid out on it, and no other interface's phase
 * is measured from it. The record channel is the follower's phase channel when it already records
 * the cable, else the highest that does, else the highest that records nothing (MUTE).
 */
export function planDedication(cable: Cable, context: PhasePathContext): PlanResult {
  const eligible = eligibility(cable, context);
  if (!eligible.ok) return eligible;
  const { roles } = eligible;
  const workspace = context.workspace;
  const config = workspace?.aggregate;
  const masterTopology = context.topology(roles.masterId) as Topology;
  const followerTopology = context.topology(roles.followerId) as Topology;
  const master = usbGroups(masterTopology);
  const follower = usbGroups(followerTopology);
  const [masterName, followerName] = [context.deviceName(roles.masterId), context.deviceName(roles.followerId)];
  if (master === undefined || follower === undefined) return { ok: false, why: "Gazelle does not know these interfaces' USB channels." };
  const outDestination = masterTopology.outputs.findIndex((group) => group.type === cable.from.port);
  const inSource = followerTopology.inputs.findIndex((group) => group.type === cable.to.port);
  if (outDestination < 0 || inSource < 0) return { ok: false, why: "One of the interfaces does not have the port this cable names." };
  const masterLayout = workspace?.mixers?.[roles.masterId];
  const followerLayout = workspace?.mixers?.[roles.followerId];

  // Everything on the master has to have been read to say what is free.
  const masterRouting = masterTopology.outputs.map((_, at) => context.routing(roles.masterId, at));
  const record = context.routing(roles.followerId, follower.recordPosition);
  if (masterRouting.some((slots) => slots === undefined) || record === undefined) {
    if (context.dryRun === true) return { ok: false, why: "The server is in dry run, where the interfaces' routing cannot be read, so which channels are free is not known. A cable is dedicated only with the routing read." };
    return { ok: false, why: `The routing of ${masterRouting.some((slots) => slots === undefined) ? masterName : followerName} has not been read, so which channels are free is not known. Read it on the Routing page, and try again.` };
  }
  const muteMaster = masterTopology.inputs.findIndex((group) => group.type === "MUTE");
  const muteFollower = followerTopology.inputs.findIndex((group) => group.type === "MUTE");
  const outFirst = cable.from.first;
  const playing = (channel: number): RouteSlot => ({ source: master.playbackPosition, channel });

  // Where each playback channel goes now, every destination slot that takes it.
  const uses = (channel: number) =>
    masterRouting.flatMap((slots, destination) => (slots ?? []).flatMap((slot, at) => (at < (masterTopology.outputs[destination]?.channels ?? 0) && sameSlot(slot, playing(channel)) ? [{ destination, channel: at }] : [])));
  const laidOut = new Set((masterLayout?.channels ?? []).flatMap((one) => (one.source?.group === master.playbackPosition ? [one.source.channel] : [])));
  const followerIndex = roles.followerIndex;
  const otherPhases = new Set(
    (config?.devices ?? []).flatMap((device, at) => {
      const setting = phaseSetting(device);
      return at === followerIndex || setting === undefined ? [] : [setting.master_output];
    }),
  );
  const nowOnCable = masterRouting[outDestination]?.[outFirst];
  const onlyOnCable = (channel: number) => {
    const used = uses(channel);
    return used.length === 1 && used[0]?.destination === outDestination && used[0].channel === outFirst && !laidOut.has(channel) && !otherPhases.has(channel);
  };
  let output: number | undefined;
  if (nowOnCable !== undefined && nowOnCable.source === master.playbackPosition && onlyOnCable(nowOnCable.channel)) output = nowOnCable.channel;
  for (let channel = master.playback.channels - 1; channel >= 0 && output === undefined; channel -= 1) {
    if (uses(channel).length === 0 && !laidOut.has(channel) && !otherPhases.has(channel)) output = channel;
  }
  if (output === undefined) return { ok: false, why: `Every USB playback channel of ${masterName} goes somewhere already, so none is free to keep for the phase measurement.` };

  const wanted: RouteSlot = { source: inSource, channel: cable.to.first };
  const current = phaseSetting(config?.devices?.[followerIndex]);
  const recording = (channel: number) => sameSlot(record[channel], wanted);
  let input: number | undefined;
  if (current !== undefined && current.input < follower.record.channels && recording(current.input)) input = current.input;
  for (let channel = follower.record.channels - 1; channel >= 0 && input === undefined; channel -= 1) if (recording(channel)) input = channel;
  for (let channel = follower.record.channels - 1; channel >= 0 && input === undefined; channel -= 1) if (record[channel]?.source === muteFollower) input = channel;
  if (input === undefined) return { ok: false, why: `Every USB record channel of ${followerName} is in use and none records ${slotWords(followerTopology, wanted)}, so none is free to keep for the phase measurement.` };

  // The writes, and a line for each.
  const writes: GroupWrite[] = [];
  const lines: string[] = [];
  const add = (deviceId: string, destination: number, channel: number, source: RouteSlot | null) => {
    let group = writes.find((one) => one.deviceId === deviceId && one.destination === destination);
    if (group === undefined) {
      group = { deviceId, destination, changes: [] };
      writes.push(group);
    }
    group.changes.push({ channel, source });
  };
  const usbOut = sourceLabel(masterTopology, { group: master.playbackPosition, channel: output });
  const usbIn = follower.record.channels > 1 ? `${follower.record.name} ${input + 1}` : follower.record.name;
  const outWords = (channel: number) => destinationWords(masterTopology, outDestination, channel, masterLayout);
  const was = (words: string | undefined, verb: "plays" | "records") => (words === undefined ? (verb === "plays" ? "it is muted now" : "it records nothing now") : `it ${verb} ${words} now`);

  if (!sameSlot(nowOnCable, playing(output))) {
    add(roles.masterId, outDestination, outFirst, playing(output));
    lines.push(`On ${masterName}, ${outWords(outFirst)} plays ${usbOut} directly, with no mix between (${was(slotWords(masterTopology, nowOnCable, masterLayout), "plays")}).`);
  } else {
    lines.push(`On ${masterName}, ${outWords(outFirst)} already plays ${usbOut} and nothing else does, so it stays as it is.`);
  }
  // On S/PDIF the pair's other side is muted, so the cable carries the measurement and the clock and
  // nothing else. An ADAT cable's other channels are channels of their own, and are left alone.
  if (cable.from.port === "SPDIF_OUT") {
    for (let lane = 1; lane < cable.channels; lane += 1) {
      const other = masterRouting[outDestination]?.[outFirst + lane];
      if (other === undefined || other.source === muteMaster) continue;
      add(roles.masterId, outDestination, outFirst + lane, null);
      lines.push(`On ${masterName}, ${outWords(outFirst + lane)} is muted (${was(slotWords(masterTopology, other, masterLayout), "plays")}), so the cable carries the measurement and its clock and nothing else. S/PDIF carries its clock whatever it plays.`);
    }
  }
  // Anywhere else the playback channel goes, which there is not when it was free.
  for (const use of uses(output)) {
    if (use.destination === outDestination && use.channel === outFirst) continue;
    add(roles.masterId, use.destination, use.channel, null);
    lines.push(`On ${masterName}, ${destinationWords(masterTopology, use.destination, use.channel, masterLayout)} stops playing ${usbOut}, so the burst plays nowhere but the cable.`);
  }
  const arriving = slotWords(followerTopology, wanted) as string;
  if (!recording(input)) {
    add(roles.followerId, follower.recordPosition, input, wanted);
    lines.push(`On ${followerName}, ${usbIn} records ${arriving} (${was(slotWords(followerTopology, record[input], followerLayout), "records")}).`);
  } else {
    lines.push(`On ${followerName}, ${usbIn} already records ${arriving}, so it stays as it is.`);
  }

  const phase = phaseFromPicks(current, { master_output: output, input }) as AggregatePhaseSetting;
  const changed = current === undefined || current.master_output !== output || current.input !== input;
  const before = current === undefined ? "" : changed ? ` (it was ${sourceLabel(masterTopology, { group: master.playbackPosition, channel: current.master_output })} to ${follower.record.name} ${current.input + 1})` : " (as it is already)";
  lines.push(`In the aggregate's setup, ${followerName}'s phase is measured from ${masterName}'s ${usbOut} to its ${usbIn}${before}.`);
  const reference = typeof current?.reference === "number" ? current.reference : undefined;
  const referenceCleared = changed && reference !== undefined ? reference : undefined;
  if (referenceCleared !== undefined) {
    lines.push(`Its phase reference, ${referenceCleared} samples, is taken out, because it was measured over the old path: measure the interfaces again under Line the interfaces up on the Aggregate page to give it one.`);
  }
  const given = current === undefined ? [] : [...(current.master_output === output ? [] : [sourceLabel(masterTopology, { group: master.playbackPosition, channel: current.master_output })]), ...(current.input === input ? [] : [`${follower.record.name} ${current.input + 1}`])];
  const released = given.length === 0 ? "" : ` ${given.join(" and ")} ${given.length === 1 ? "is" : "are"} given back to it.`;
  lines.push(`${usbOut} and ${usbIn} are kept for the phase measurement and hidden from your DAW.${released}`);
  return { ok: true, plan: { cable, roles, output, input, writes, phase, ...(referenceCleared === undefined ? {} : { referenceCleared }), lines } };
}

/** The workspace with a plan's dedication and phase setup written, for one `editWorkspace`. */
export function withDedication(workspace: Workspace, plan: DedicationPlan): Workspace {
  const dedicated: CableDedication = { phase_output: plan.output, phase_input: plan.input };
  const devices = [...(workspace.aggregate?.devices ?? [])];
  const at = entryOf(workspace.aggregate, plan.roles.followerId);
  const device = devices[at];
  if (device !== undefined) devices[at] = withPhase(device, phaseFromPicks(phaseSetting(device), { master_output: plan.output, input: plan.input }));
  return {
    ...workspace,
    cables: (workspace.cables ?? []).map((cable) => (cable.id === plan.cable.id ? { ...cable, dedicated } : cable)),
    aggregate: { ...(workspace.aggregate ?? {}), devices },
  };
}

/**
 * Makes a plan: its routing writes first, one `set_routing` per group, each read first, then the
 * dedication and the phase setup, together, in the workspace. A write that is not sent stops it
 * there, before the workspace says anything it did not do.
 */
export async function applyDedication(plan: DedicationPlan, deps: { route: RouteWriter; editWorkspace(update: (workspace: Workspace) => Workspace): boolean; deviceName(deviceId: string): string }): Promise<{ ok: boolean; text: string }> {
  let written = 0;
  for (const write of plan.writes) {
    if (!(await deps.route(write.deviceId, write.destination, write.changes))) {
      const done = written === 0 ? "Nothing was changed." : `${written === 1 ? "One routing group was" : `${written} routing groups were`} written before it; the cable is not marked dedicated.`;
      return { ok: false, text: `The cable was not dedicated: the routing on ${deps.deviceName(write.deviceId)} could not be written. ${done}` };
    }
    written += 1;
  }
  if (!deps.editWorkspace((workspace) => withDedication(workspace, plan))) {
    return { ok: false, text: "The routing was written, and the dedication was not saved: Gazelle is not connected. Dedicate the cable again once it is." };
  }
  const reference = plan.referenceCleared === undefined ? "" : " Measure the interfaces again on the Aggregate page to give the phase a reference.";
  return { ok: true, text: `Dedicated to the phase measurement and the clock.${reference}` };
}

/**
 * The workspace with a cable's dedication taken off. The routing is left as it is, and so is the
 * follower's phase setup unless `clearPhase` says otherwise: the path still works, and the two
 * channels stay hidden from a DAW while the phase setup names them.
 */
export function withoutDedication(workspace: Workspace, cableId: string, clearPhase: boolean): Workspace {
  const cable = (workspace.cables ?? []).find((one) => one.id === cableId);
  if (cable === undefined) return workspace;
  const cables = (workspace.cables ?? []).map((one) => {
    if (one.id !== cableId) return one;
    const { dedicated: _gone, ...rest } = one;
    return rest;
  });
  if (!clearPhase || workspace.aggregate === undefined) return { ...workspace, cables };
  const at = entryOf(workspace.aggregate, cable.to.device_id);
  const devices = [...(workspace.aggregate.devices ?? [])];
  const device = devices[at];
  if (device !== undefined) devices[at] = withPhase(device, undefined);
  return { ...workspace, cables, aggregate: { ...workspace.aggregate, devices } };
}

// ---------------------------------------------------------------------------------------------
// While a cable is dedicated
// ---------------------------------------------------------------------------------------------

/**
 * Why a dedicated cable's dedication means nothing now, or undefined while it does: one of its
 * devices left the aggregate, its sending device does not drive the callback, or the follower's
 * phase setup names other channels. The routing is left as it was; nothing guards it any more.
 */
export function staleness(cable: Cable, workspace: Workspace | undefined, deviceName: (deviceId: string) => string): string | undefined {
  const dedication = cable.dedicated;
  if (dedication === undefined) return undefined;
  const config = workspace?.aggregate;
  const [sender, receiver] = [deviceName(cable.from.device_id), deviceName(cable.to.device_id)];
  const fromIndex = entryOf(config, cable.from.device_id);
  const toIndex = entryOf(config, cable.to.device_id);
  const tail = "so the dedication means nothing now: its routing stays as it is and nothing guards it. Turn it off to release its channels, or dedicate it again.";
  if (fromIndex < 0) return `${sender} is not in the aggregate now, ${tail}`;
  if (toIndex < 0) return `${receiver} is not in the aggregate now, ${tail}`;
  if (masterIndex(config) !== fromIndex) return `${sender} does not drive the callback now, and a phase is measured from the interface that does, ${tail}`;
  const phase = phaseSetting(config?.devices?.[toIndex]);
  if (phase === undefined) return `${receiver} has no phase setup now, ${tail}`;
  if (phase.master_output !== dedication.phase_output || phase.input !== dedication.phase_input) return `${receiver}'s phase setup names other channels now, ${tail}`;
  return undefined;
}

/** One dedicated cable's path, in routing terms, for the Routing page's marks and guard. */
export interface DedicatedPath {
  cable: Cable;
  /** "Quadro S/PDIF out 1 and 2 → Studio+ S/PDIF in 1 and 2". */
  label: string;
  masterId: string;
  followerId: string;
  /** The master's cable output channel, and what it has to play. */
  out: { destination: number; channel: number; source: RouteSlot; words: string; sourceWords: string };
  /** The follower's record channel, and what it has to record. */
  record: { destination: number; channel: number; source: RouteSlot; words: string; sourceWords: string };
}

/** Every dedicated cable whose two devices' models are known, as routing. */
export function dedicatedPaths(cables: readonly Cable[], context: Pick<PhasePathContext, "topology" | "deviceName">): DedicatedPath[] {
  return cables.flatMap((cable) => {
    const dedication = cable.dedicated;
    if (dedication === undefined) return [];
    const masterTopology = context.topology(cable.from.device_id);
    const followerTopology = context.topology(cable.to.device_id);
    const master = usbGroups(masterTopology);
    const follower = usbGroups(followerTopology);
    if (masterTopology === undefined || followerTopology === undefined || master === undefined || follower === undefined) return [];
    const outDestination = masterTopology.outputs.findIndex((group) => group.type === cable.from.port);
    const inSource = followerTopology.inputs.findIndex((group) => group.type === cable.to.port);
    if (outDestination < 0 || inSource < 0) return [];
    const play: RouteSlot = { source: master.playbackPosition, channel: dedication.phase_output };
    const arriving: RouteSlot = { source: inSource, channel: cable.to.first };
    return [
      {
        cable,
        label: cableLabel(cable, context.deviceName),
        masterId: cable.from.device_id,
        followerId: cable.to.device_id,
        out: { destination: outDestination, channel: cable.from.first, source: play, words: destinationWords(masterTopology, outDestination, cable.from.first), sourceWords: sourceLabel(masterTopology, { group: play.source, channel: play.channel }) },
        record: {
          destination: follower.recordPosition,
          channel: dedication.phase_input,
          source: arriving,
          words: destinationWords(followerTopology, follower.recordPosition, dedication.phase_input),
          sourceWords: slotWords(followerTopology, arriving) as string,
        },
      },
    ];
  });
}

/** What the Routing page marks on one device: destination cells and source chips, by "group:channel", with what to say about each. */
export function pathMarks(paths: readonly DedicatedPath[], deviceId: string): { destinations: Map<string, string>; sources: Map<string, string> } {
  const destinations = new Map<string, string>();
  const sources = new Map<string, string>();
  for (const path of paths) {
    const kept = `Kept for the phase measurement over the dedicated ${portKind(path.cable.from.port)} cable, ${path.label}`;
    if (path.masterId === deviceId) {
      destinations.set(`${path.out.destination}:${path.out.channel}`, `${kept}: it plays ${path.out.sourceWords} and nothing else.`);
      sources.set(`${path.out.source.source}:${path.out.source.channel}`, `${kept}: it plays to ${path.out.words} and nowhere else, and is hidden from your DAW.`);
    }
    if (path.followerId === deviceId) destinations.set(`${path.record.destination}:${path.record.channel}`, `${kept}: it records ${path.record.sourceWords}, and is hidden from your DAW.`);
  }
  return { destinations, sources };
}

/**
 * What a routing change on one device would break of the dedicated paths, a sentence each naming the
 * cable, or nothing. It breaks one when it takes the cable's output away from its playback channel
 * (routing another source there, or muting it), sends that playback channel anywhere else (where the
 * burst would then play too), or takes the follower's record channel away from the cable. The change
 * is still allowed; the page asks first.
 */
export function breaks(paths: readonly DedicatedPath[], deviceId: string, destination: number, changes: readonly { channel: number; source: RouteSlot | null }[], mute: number, topology: Topology): string[] {
  const said: string[] = [];
  for (const path of paths) {
    const cable = `the dedicated ${portKind(path.cable.from.port)} cable, ${path.label}`;
    for (const { channel, source } of changes) {
      const slot = source ?? { source: mute, channel: 0 };
      if (path.masterId === deviceId && destination === path.out.destination && channel === path.out.channel && !sameSlot(slot, path.out.source)) {
        said.push(`${path.out.words} would stop playing ${path.out.sourceWords}, so the phase measurement over ${cable} would hear nothing.`);
      }
      if (path.masterId === deviceId && sameSlot(slot, path.out.source) && !(destination === path.out.destination && channel === path.out.channel)) {
        said.push(`${path.out.sourceWords} would play to ${destinationWords(topology, destination, channel)} as well, and the burst the driver plays into it at the start of every session with it. It is kept for the phase measurement over ${cable}.`);
      }
      if (path.followerId === deviceId && destination === path.record.destination && channel === path.record.channel && !sameSlot(slot, path.record.source)) {
        said.push(`${path.record.words} would stop recording ${path.record.sourceWords}, so the phase measurement over ${cable} would hear nothing.`);
      }
    }
  }
  return [...new Set(said)];
}

// ---------------------------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------------------------

/** The clock source a model should be on to follow a cable arriving at `port`, as its index and name. */
export function clockSourceFor(sources: readonly string[], port: DigitalPort | string): { index: number; name: string } | undefined {
  const wanted = portKind(port);
  const index = sources.findIndex((name) => name === wanted || (wanted === "ADAT" && name === "ADAT x1"));
  return index < 0 ? undefined : { index, name: sources[index] as string };
}

/**
 * The clock fix a dedicated cable's receiver needs, when it reports a clock that does not follow the
 * cable: the same change the Aggregate page's readiness offers, which the page sends only after its
 * own confirm, because a clock change interrupts the audio.
 */
export function clockAdvice(cable: Cable, context: PhasePathContext): { deviceId: string; index: number; name: string; now: string; label: string } | undefined {
  const clock = context.clock?.(cable.to.device_id);
  if (clock === undefined) return undefined;
  const wanted = portKind(cable.to.port);
  const now = clock.sources[clock.source];
  // A source the model does not have is a report not understood, and says nothing either way.
  if (now === undefined || now.startsWith(wanted)) return undefined;
  const target = clockSourceFor(clock.sources, cable.to.port);
  if (target === undefined) return undefined;
  const name = context.deviceName(cable.to.device_id);
  return { deviceId: cable.to.device_id, index: target.index, name: target.name, now, label: `Put ${name} on ${target.name}` };
}

// ---------------------------------------------------------------------------------------------
// The Aggregate page's readiness: putting a dedicated path back
// ---------------------------------------------------------------------------------------------

/** One routing change a fix names: one channel of one destination group, to a source or to MUTE (null). */
export interface RoutingWrite {
  deviceId: string;
  destination: number;
  channel: number;
  source: RouteSlot | null;
}

const whole = (value: unknown): value is number => typeof value === "number" && Number.isInteger(value) && value >= 0;

/**
 * The routing writes of a `restore_phase_path` fix, which the server works out from the routing it
 * last knew and the page makes itself, or undefined for any other fix or one whose writes are not.
 */
export function restoreWrites(fix: AggregateFix): RoutingWrite[] | undefined {
  if (fix.kind !== "restore_phase_path") return undefined;
  const writes = typeof fix.body === "object" && fix.body !== null ? (fix.body as Record<string, unknown>)["writes"] : undefined;
  if (!Array.isArray(writes) || writes.length === 0) return undefined;
  const parsed: RoutingWrite[] = [];
  for (const write of writes as unknown[]) {
    if (typeof write !== "object" || write === null) return undefined;
    const { device_id: deviceId, destination, channel, source } = write as Record<string, unknown>;
    if (typeof deviceId !== "string" || !whole(destination) || !whole(channel)) return undefined;
    if (source !== null && !(Array.isArray(source) && source.length === 2 && whole(source[0]) && whole(source[1]))) return undefined;
    parsed.push({ deviceId, destination, channel, source: source === null ? null : { source: source[0] as number, channel: source[1] as number } });
  }
  return parsed;
}

/**
 * Routing writes grouped as `set_routing` takes them: one change list per destination group of one
 * device, in the order the writes first name each group, so each group is read once and written once.
 */
export function routingGroups(writes: readonly RoutingWrite[]): GroupWrite[] {
  const groups: GroupWrite[] = [];
  for (const write of writes) {
    let group = groups.find((one) => one.deviceId === write.deviceId && one.destination === write.destination);
    if (group === undefined) {
      group = { deviceId: write.deviceId, destination: write.destination, changes: [] };
      groups.push(group);
    }
    group.changes = [...group.changes.filter((change) => change.channel !== write.channel), { channel: write.channel, source: write.source }];
  }
  return groups;
}

/**
 * Puts a dedicated path back: one `set_routing` per group, each through the routing model, which
 * reads the group first and changes only the channels named, exactly as the Routing page does. A
 * group not sent stops the rest, and what it came to says so.
 */
export async function restorePath(writes: readonly RoutingWrite[], route: RouteWriter): Promise<{ text: string; problem: boolean }> {
  const groups = routingGroups(writes);
  let sent = 0;
  for (const group of groups) {
    if (!(await route(group.deviceId, group.destination, group.changes))) break;
    sent += 1;
  }
  return sent === groups.length
    ? { text: `The phase path is back: ${sent === 1 ? "one routing group was" : `${sent} routing groups were`} sent.`, problem: false }
    : { text: `The phase path is not back: ${groups.length - sent} of ${groups.length} routing groups were not sent.`, problem: true };
}

/**
 * The page a reason is put right on, when it is not the Aggregate page and no fix does it there: a
 * dedication that no longer means anything is turned off, and a broken path that is not dedicated is
 * given one, on the Workspace page, where the cables are.
 */
export function reasonPage(reason: AggregateReason): "workspace" | undefined {
  if (reason.code === "phase_dedication_stale") return "workspace";
  if (reason.code === "phase_path_broken" && reason.fix === undefined) return "workspace";
  return undefined;
}
