// Tidying a mix: undoing what it plays outside its channels (`mix-health.ts`), after a confirm that
// lists every change. Only the Mixer page's lazy notice uses it, so it travels in that chunk.
//
// - A stray on a slot none of the mix's channels uses is routed to MUTE, Gazelle's way of leaving a
//   slot unused; a channel whose device input differs from its layout input is routed back to its
//   layout input, as applying the layout would. Both go in one `set_routing` for the mix's group,
//   through the routing model, which reads the group fresh first and changes only these slots.
// - Every solo in the mix is cleared: a solo anywhere silences the rest of the mix, and the simple
//   honest rule is to offer all of them, listed.
// - Effect returns are left alone unless the person ticks "also mute the effect returns".
//
// Afterwards the mix's routing and strips are read again, so what the notice says next is what the
// device holds.

import type { RouteSource } from "gazelle-audio-client";

import type { MixHealth, SlotState } from "./mix-health.ts";
import type { RouteSlot } from "./routing.ts";
import type { Store } from "./store.ts";

export interface TidyRoute {
  readonly slot: number;
  /** What the device routes there now. */
  readonly from: RouteSlot;
  /** The channel's layout input, or null for MUTE. */
  readonly to: RouteSource | null;
  /** The layout channel whose input is put back, when `to` is one. */
  readonly channel?: string;
}

export interface TidyPlan {
  readonly routes: readonly TidyRoute[];
  /** Strips whose solo is cleared. */
  readonly solos: readonly SlotState[];
  /** Effect returns muted, only when asked for. */
  readonly returns: readonly SlotState[];
}

export function tidyPlan(health: MixHealth, muteReturns: boolean): TidyPlan {
  return {
    routes: health.strays.map((s) => (s.channel === undefined ? { slot: s.slot, from: s.source, to: null } : { slot: s.slot, from: s.source, to: s.channel.source ?? null, channel: s.channel.id })),
    solos: health.solos,
    returns: muteReturns ? health.returns : [],
  };
}

/** How many changes a plan makes. */
export function planSize(plan: TidyPlan): number {
  return plan.routes.length + plan.solos.length + plan.returns.length;
}

/**
 * How the notice names things: a source (the channels model's `sourceLabel`) and the MUTE source's
 * position. Passed in rather than imported, so this chunk takes nothing of the app's store modules
 * (importing one from a lazy chunk made the bundler pull other shared chunks into the entry).
 */
export interface Naming {
  source(source: RouteSource): string;
  mute: number;
}

/** What a strip is called in the notice: its input on the device, or its slot when nothing is routed there. */
export function slotLabel(naming: Naming, state: { slot: number; source: RouteSlot }): string {
  return state.source.source === naming.mute ? `Slot ${state.slot + 1}` : naming.source({ group: state.source.source, channel: state.source.channel });
}

/** The confirm's list: one line per change, saying which routes are muted and which are put back. `name` names a layout channel by id. */
export function planLines(plan: TidyPlan, naming: Naming, name: (channel: string) => string): string[] {
  return [
    ...plan.routes.map((r) =>
      r.to === null || r.channel === undefined
        ? `Slot ${r.slot + 1}: route MUTE in place of ${slotLabel(naming, { slot: r.slot, source: r.from })}, which is not one of this mix's channels`
        : `Slot ${r.slot + 1}: route ${name(r.channel)}'s input ${naming.source(r.to)} again, in place of ${slotLabel(naming, { slot: r.slot, source: r.from })}, as applying the layout would`,
    ),
    ...plan.solos.map((s) => `${slotLabel(naming, s)} (slot ${s.slot + 1}): clear its solo`),
    ...plan.returns.map((s) => `${slotLabel(naming, s)} (slot ${s.slot + 1}): mute this effect return`),
  ];
}

/**
 * Carries out a plan on one mix, then reads the mix's routing and strips again. Resolves to how
 * many changes were sent. A strip changed twice (a solo cleared and a return muted) is one command.
 */
export async function applyTidy(store: Store, deviceId: string, mix: number, plan: TidyPlan): Promise<number> {
  const routing = store.routing(deviceId);
  const mixer = store.mixer(deviceId, mix);
  const destination = store.mixInput(deviceId, mix);
  let changes = 0;
  if (plan.routes.length > 0) {
    const sent = await routing.routeMany(
      destination,
      plan.routes.map((r) => ({ channel: r.slot, source: r.to === null ? null : { source: r.to.group, channel: r.to.channel } })),
    );
    if (sent) changes += plan.routes.length;
  }
  const strips = new Map<number, { solo?: boolean; mute?: boolean; count: number }>();
  for (const s of plan.solos) strips.set(s.slot, { ...strips.get(s.slot), solo: false, count: (strips.get(s.slot)?.count ?? 0) + 1 });
  for (const s of plan.returns) strips.set(s.slot, { ...strips.get(s.slot), mute: true, count: (strips.get(s.slot)?.count ?? 0) + 1 });
  const sent = await Promise.all([...strips].map(async ([slot, { count, ...change }]) => ((await mixer.setAlone(slot, change)) ? count : 0)));
  changes += sent.reduce((a, b) => a + b, 0);
  await routing.load(destination);
  await mixer.load();
  return changes;
}
