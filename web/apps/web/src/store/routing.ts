// A device's routing, as the device holds it: every destination group (topology `outputs`) has
// one source slot per channel, each naming a source group (topology `inputs`) and a channel in it.
// Group positions in the topology are the wire ids (P38).
//
// Writes follow the device's shape: `set_routing` replaces all 32 slots of one destination group,
// so each change reads that group fresh first and alters only its own slot, which keeps routes
// made elsewhere (the vendor panel, another browser) intact. Changes to one group run one at a
// time for the same reason. Unused slots, and slots past a group's channel count, are MUTE (the
// user's choice, plan 2026-09-16). A failed write restores what the device last reported.
//
// In dry run nothing is sent, so a read has no reply: the write then builds on what is already
// known (or MUTE), which shows the bytes without risking anything on a device.

import { signal, type ReadonlySignal, type Signal } from "../core/signal.ts";
import type { Topology } from "gazelle-audio-client";

/** Slots in one `set_routing` group, in both families. */
export const ROUTING_SLOTS = 32;

export interface RouteSlot {
  /** Source group position in the topology's `inputs`. */
  readonly source: number;
  /** Channel within that source group, from 0. */
  readonly channel: number;
}

export interface RoutingRead {
  /** The group's slots as the device reported them, or undefined when there was no reply. */
  readonly slots: readonly RouteSlot[] | undefined;
  /** The server is in dry run, so no reply was expected. */
  readonly dryRun: boolean;
}

export interface RoutingContext {
  deviceId: string;
  topology: Topology;
  /** `get_routing` for one destination group; rejects when the read failed. */
  read(destination: number): Promise<RoutingRead>;
  /** `set_routing` with all 32 slots; resolves false when it was not sent. */
  write(destination: number, slots: readonly RouteSlot[]): Promise<boolean>;
  /** Tells the user why a change did not happen. */
  notify(message: string): void;
}

export class RoutingModel {
  readonly deviceId: string;
  /** The MUTE source's position. */
  readonly mute: number;
  readonly #context: RoutingContext;
  readonly #groups: Signal<readonly RouteSlot[] | undefined>[];
  readonly #queues = new Map<number, Promise<unknown>>();

  constructor(context: RoutingContext) {
    this.#context = context;
    this.deviceId = context.deviceId;
    const mute = context.topology.inputs.findIndex((g) => g.type === "MUTE");
    if (mute < 0) throw new Error(`${context.topology.family} topology has no MUTE source`);
    this.mute = mute;
    this.#groups = context.topology.outputs.map(() => signal<readonly RouteSlot[] | undefined>(undefined));
  }

  /** A destination group's slots (one per channel), or undefined until read. */
  destination(index: number): ReadonlySignal<readonly RouteSlot[] | undefined> {
    return this.#group(index);
  }

  /** Reads one destination group from the device. Resolves false when it could not be read. */
  load(index: number): Promise<boolean> {
    return this.#serially(index, async () => {
      const read = await this.#read(index);
      if (read?.slots !== undefined) this.#group(index).value = read.slots;
      return read?.slots !== undefined;
    });
  }

  /** Reads every destination group, one after another as the panel does. */
  async loadAll(): Promise<void> {
    for (let i = 0; i < this.#groups.length; i++) await this.load(i);
  }

  /**
   * Routes `source` (or MUTE, for null) to one destination channel. Resolves true when the
   * change was sent; the group then holds it.
   */
  route(destination: number, channel: number, source: RouteSlot | null): Promise<boolean> {
    return this.routeMany(destination, [{ channel, source }]);
  }

  /**
   * Changes several channels of one destination group at once (null routes MUTE): one fresh read
   * and one write, whatever the count. Resolves true when sent; an empty change sends nothing.
   */
  routeMany(destination: number, changes: readonly { channel: number; source: RouteSlot | null }[]): Promise<boolean> {
    const group = this.#context.topology.outputs[destination];
    if (group === undefined) throw new RangeError(`destination ${destination} is outside 0..${this.#groups.length - 1}`);
    const slots = changes.map(({ channel, source }) => {
      if (!Number.isInteger(channel) || channel < 0 || channel >= group.channels) throw new RangeError(`${group.name} has channels 0..${group.channels - 1}, not ${channel}`);
      const slot = source ?? this.#muted();
      const from = this.#context.topology.inputs[slot.source];
      if (from === undefined || !Number.isInteger(slot.channel) || slot.channel < 0 || slot.channel >= from.channels) throw new RangeError(`no source channel ${slot.source}:${slot.channel}`);
      return { channel, slot };
    });
    if (slots.length === 0) return Promise.resolve(true);

    return this.#serially(destination, async () => {
      const signal = this.#group(destination);
      const read = await this.#read(destination);
      if (read === undefined) return false;
      if (read.slots === undefined && !read.dryRun) {
        this.#context.notify(`Routing to ${group.name} was not changed: the device did not report its current routing.`);
        return false;
      }
      const current = read.slots ?? signal.peek();
      if (read.slots !== undefined) signal.value = read.slots;
      const known = signal.peek();

      // `current` holds one slot per channel, so everything past the group's channels is MUTE.
      const next = Array.from({ length: ROUTING_SLOTS }, (_, i) => current?.[i] ?? this.#muted());
      for (const { channel, slot } of slots) next[channel] = slot;
      signal.value = next.slice(0, group.channels);
      const sent = await this.#context.write(destination, next);
      if (!sent) signal.value = known;
      return sent;
    });
  }

  #muted(): RouteSlot {
    return { source: this.mute, channel: 0 };
  }

  async #read(destination: number): Promise<RoutingRead | undefined> {
    try {
      const read = await this.#context.read(destination);
      const channels = this.#context.topology.outputs[destination]?.channels ?? 0;
      return read.slots === undefined ? read : { ...read, slots: read.slots.slice(0, channels) };
    } catch (error) {
      const name = this.#context.topology.outputs[destination]?.name ?? String(destination);
      this.#context.notify(`Could not read the routing to ${name}: ${error instanceof Error ? error.message : String(error)}`);
      return undefined;
    }
  }

  #group(index: number): Signal<readonly RouteSlot[] | undefined> {
    const group = this.#groups[index];
    if (group === undefined) throw new RangeError(`destination ${index} is outside 0..${this.#groups.length - 1}`);
    return group;
  }

  /** Runs `task` after earlier work on the same destination group has settled. */
  #serially<T>(destination: number, task: () => Promise<T>): Promise<T> {
    const previous = this.#queues.get(destination) ?? Promise.resolve();
    const next = previous.then(task, task);
    this.#queues.set(destination, next.catch(() => undefined));
    return next;
  }
}
