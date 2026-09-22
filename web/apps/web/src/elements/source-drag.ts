// Dragging sources from the Routing page onto the mixer dock, to build a mix from the routing (the
// owner's request, 2026-09-21). It is native drag and drop, so the Routing page's chips and the
// dock, separate elements with separate shadow roots, know nothing of each other beyond this module.
//
// The drag carries its sources under SOURCE_MIME as JSON: the device they belong to and the source
// channels, in order. A browser shows only the types of a drag's data while it is over a target, not
// the data itself, so the drag in flight is also kept here (`activeSourceDrag`) for the dock to say
// what a drop would do before it happens; the drop itself reads the data.
//
// What a drop does is what the Mixer page's own controls do, one channel at a time: "+" adds a
// channel, its Input menu chooses the source and its Main mix menu the mix (`ChannelsModel.addFed`).
// So Gazelle routes it exactly as it would have, and the same question is asked first when the mix
// already has that input (`doublingsOf`, the words the Input menu's Confirm uses).

import type { ChannelsModel } from "../store/channels.ts";
import type { Store } from "../store/store.ts";

/** A source channel: its group in the topology's inputs, and its channel in that group. */
type RouteSource = Parameters<ChannelsModel["addFed"]>[0];

/** The type a drag of routing sources carries its data under. */
export const SOURCE_MIME = "application/x-gazelle-sources";

/** What a drag of routing sources carries: whose sources, and which, in order. */
export interface SourceDrag {
  deviceId: string;
  sources: RouteSource[];
}

let active: SourceDrag | undefined;

/** The drag of sources in flight in this page, if any. */
export function activeSourceDrag(): SourceDrag | undefined {
  return active;
}

/** Records the drag in flight (on dragstart), or clears it (on dragend). */
export function setActiveSourceDrag(drag: SourceDrag | undefined): void {
  active = drag;
}

export function encodeSourceDrag(drag: SourceDrag): string {
  return JSON.stringify({ deviceId: drag.deviceId, sources: drag.sources.map(({ group, channel }) => ({ group, channel })) });
}

const isIndex = (value: unknown): value is number => typeof value === "number" && Number.isInteger(value) && value >= 0;

/** Reads a drag's data back, or undefined when it is not a drag of sources this app made. */
export function decodeSourceDrag(data: string): SourceDrag | undefined {
  let parsed: unknown;
  try {
    parsed = JSON.parse(data);
  } catch {
    return undefined;
  }
  if (typeof parsed !== "object" || parsed === null) return undefined;
  const { deviceId, sources } = parsed as { deviceId?: unknown; sources?: unknown };
  if (typeof deviceId !== "string" || deviceId === "" || !Array.isArray(sources) || sources.length === 0) return undefined;
  const read: RouteSource[] = [];
  for (const source of sources as unknown[]) {
    const { group, channel } = (source ?? {}) as { group?: unknown; channel?: unknown };
    if (!isIndex(group) || !isIndex(channel)) return undefined;
    read.push({ group, channel });
  }
  return { deviceId, sources: read };
}

/** Whether a drag's types include a drag of sources: all a target can see before the drop. */
export function carriesSources(types: readonly string[] | DOMStringList | undefined): boolean {
  if (types === undefined) return false;
  return Array.from(types as ArrayLike<string>).includes(SOURCE_MIME);
}

/** What the dock says a drop would do: "Drop to add a channel to Monitors". */
export function dropHint(count: number, mixName: string): string {
  return `Drop to add ${count === 1 ? "a channel" : `${count} channels`} to ${mixName}`;
}

/**
 * Why putting these sources into `mix` would sum an input twice, one line per source that would,
 * in the words the Input menu's Confirm uses; empty when none would. Reading it is reactive.
 */
export function doublingsOf(store: Store, deviceId: string, mix: number, sources: readonly RouteSource[]): string[] {
  return sources.flatMap((source) => {
    const warning = store.doublingIfAdded(deviceId, mix, "", source);
    return warning === undefined ? [] : [warning];
  });
}

/**
 * Gives a device with no layout yet the one it would take from its routing on opening the Mixer
 * page, so a drop adds to what the device already has rather than starting a mixer over it. Call it
 * before `doublingsOf`, which can only see channels the layout has.
 */
export async function layoutForDrop(store: Store, deviceId: string): Promise<void> {
  const channels = store.channels(deviceId);
  if (!channels.configured) await channels.importFromDevice();
}

/**
 * Adds one channel per source, each fed by it with `mix` as its main mix, in order. Resolves to the
 * new channels' ids; it stops at the first that cannot be added (the mixer is full, say, which
 * `add` has already said).
 */
export async function addDroppedSources(store: Store, deviceId: string, mix: number, sources: readonly RouteSource[]): Promise<string[]> {
  const channels = store.channels(deviceId);
  const added: string[] = [];
  for (const source of sources) {
    const id = await channels.addFed(source, mix);
    if (id === undefined) break;
    added.push(id);
  }
  return added;
}
