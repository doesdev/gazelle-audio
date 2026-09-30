// What a device mix plays that the Mixer page does not show (found on the owner's Quadro,
// 2026-09-30). The page shows the layout's channels; the device keeps routing and strip state
// outside the layout that still plays: a leftover route on a slot no channel of the mix uses, a
// channel whose input on the device is not the one the layout gives it, a solo on a strip the page
// never shows (which silences everything else in the mix), and on the Quadro the effect returns on
// slots 1 to 6, which the vendor panel keeps in every mix.
//
// Worked out from what the app has already read: the mix's routing group and its strips. Nothing is
// asked of the device here, and nothing is known until both have been read (never in dry run).

import type { MixerChannel, Topology } from "gazelle-audio-client";

import { QUADRO_EFFECT_SLOTS } from "./channels.ts";
import { LEVEL_MAX, type StripState } from "./mixer.ts";
import type { RouteSlot } from "./routing.ts";

/** One strip of a mix as the device holds it: what routing feeds its slot, and its level, mute and solo. */
export interface SlotState {
  readonly slot: number;
  readonly source: RouteSlot;
  readonly level: number;
  readonly mute: boolean;
  readonly solo: boolean;
}

/**
 * A strip playing into a mix that the layout does not account for: its slot is no channel of the
 * mix, or (with `channel`) it is that channel's slot, but the device feeds it another input.
 */
export interface Stray extends SlotState {
  readonly channel?: MixerChannel;
}

export interface MixHealth {
  readonly strays: readonly Stray[];
  /** Every soloed strip of the mix, `shown` when it is one of the mix's channels. */
  readonly solos: readonly (SlotState & { readonly shown: boolean })[];
  /** The Quadro's effect returns (slots 1 to 6 on AFX OUT) that are audible: routed, unmuted, above the floor. */
  readonly returns: readonly SlotState[];
}

/**
 * Sorts a mix's strips into strays, solos and effect returns. `channels` are the layout's channels
 * in this mix; `routing` is the mix's input group as read; `strip` a strip's state as read.
 *
 * A strip counts as playing when its slot is routed (not MUTE), it is not muted, and its fader is
 * above the floor. A solo counts whatever else is true of its strip: a soloed strip that is muted
 * still silences the rest of the mix.
 */
export function mixHealth(topology: Topology, routing: readonly RouteSlot[], strip: (slot: number) => StripState, channels: readonly MixerChannel[]): MixHealth {
  const mute = topology.inputs.findIndex((g) => g.type === "MUTE");
  const afx = topology.inputs.findIndex((g) => g.type === "AFX_OUT");
  const fixed = topology.family === "quadro" ? QUADRO_EFFECT_SLOTS : 0;
  const strays: Stray[] = [];
  const solos: (SlotState & { shown: boolean })[] = [];
  const returns: SlotState[] = [];
  for (let slot = 0; slot < topology.mixers.channels; slot++) {
    const source = routing[slot];
    if (source === undefined) continue;
    const { level, mute: muted, solo } = strip(slot);
    const state = { slot, source, level, mute: muted, solo };
    const channel = channels.find((c) => c.slot === slot);
    if (solo) solos.push({ ...state, shown: channel !== undefined });
    if (source.source === mute || muted || level >= LEVEL_MAX) continue;
    if (channel !== undefined) {
      if (channel.source?.group !== source.source || channel.source.channel !== source.channel) strays.push({ ...state, channel });
    } else if (slot < fixed && source.source === afx) returns.push(state);
    else strays.push(state);
  }
  return { strays, solos, returns };
}

/** The mix buttons' tooltip: how many solos and strays a mix has, or undefined when it has none. Effect returns are not warned about. */
export function mixWarning(health: MixHealth | undefined): string | undefined {
  if (health === undefined) return undefined;
  const parts: string[] = [];
  const solos = health.solos.length;
  const strays = health.strays.length;
  if (solos > 0) parts.push(`${solos} soloed ${solos === 1 ? "strip" : "strips"}`);
  if (strays > 0) parts.push(`${strays} ${strays === 1 ? "strip plays" : "strips play"} outside this mix's channels`);
  return parts.length === 0 ? undefined : parts.join(" and ");
}
