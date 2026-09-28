// The soft link, after Cubase's Q-Link: channels selected on the Mixer page or in the mixer dock
// move together while two or more are selected. A fader or pan moved on one moves the others by the
// same step, each held inside its own range; mute and solo set the others to the clicked channel's
// new state. It is this tab's alone: never kept in the workspace, never sent to a device as a stereo
// link, and gone once cleared or the page is reloaded. One device's channels at a time, since its
// strips are one device's mix; selecting on another device starts over there.
//
// The changes themselves go through the mixer (`MixerModel`), which sends each channel the command
// its own control would and makes sure no strip is changed twice when a permanent link joins some
// of the same channels.

import { computed, signal, type ReadonlySignal } from "../core/signal.ts";

export interface SoftSelection {
  deviceId: string;
  /** Mixer strips (channel slots), in the order they were selected. */
  slots: readonly number[];
}

/** How a click selects: only this channel, add or remove it (Ctrl or Cmd, or a tap), or a range from the last one (Shift). */
export type SelectHow = "only" | "toggle" | "range";

export class SoftLinkModel {
  readonly #selection = signal<SoftSelection | undefined>(undefined);
  #anchor: number | undefined;
  /** The selection, undefined when nothing is selected. Reading it is reactive. */
  readonly selection: ReadonlySignal<SoftSelection | undefined> = this.#selection;
  /** How many channels move together: 0 unless two or more are selected. Reading it is reactive. */
  readonly linked = computed(() => ((this.#selection.value?.slots.length ?? 0) >= 2 ? (this.#selection.value?.slots.length ?? 0) : 0));

  /** Whether a device's strip is selected. Reading it is reactive. */
  selected(deviceId: string, slot: number): boolean {
    const s = this.#selection.value;
    return s?.deviceId === deviceId && s.slots.includes(slot);
  }

  /**
   * Selects as a click on a channel's name does. `order` is the channels in the order they are
   * shown, which a range follows. A plain click on the only selected channel clears it.
   */
  select(deviceId: string, slot: number, how: SelectHow, order: readonly number[] = []): void {
    const current = this.#selection.peek();
    const mine = current?.deviceId === deviceId ? current.slots : [];
    let slots: readonly number[];
    if (how === "range" && this.#anchor !== undefined && mine.length > 0 && order.includes(this.#anchor) && order.includes(slot)) {
      const [from, to] = [order.indexOf(this.#anchor), order.indexOf(slot)].sort((a, b) => a - b) as [number, number];
      slots = order.slice(from, to + 1);
    } else {
      this.#anchor = slot;
      if (how === "only") slots = mine.length === 1 && mine[0] === slot ? [] : [slot];
      else slots = mine.includes(slot) ? mine.filter((s) => s !== slot) : [...mine, slot];
    }
    this.#selection.value = slots.length === 0 ? undefined : { deviceId, slots };
  }

  clear(): void {
    this.#anchor = undefined;
    if (this.#selection.peek() !== undefined) this.#selection.value = undefined;
  }

  /** The other strips a change to this one goes to: none unless it is one of two or more selected. */
  peers(deviceId: string, slot: number): readonly number[] {
    const s = this.#selection.peek();
    if (s === undefined || s.deviceId !== deviceId || s.slots.length < 2 || !s.slots.includes(slot)) return [];
    return s.slots.filter((other) => other !== slot);
  }
}
