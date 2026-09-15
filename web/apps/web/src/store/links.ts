// Channel links (decision P51). A link joins two or more channels of one kind, on any devices, and
// lives in the workspace: a change made to one member through this app goes to every member, the
// same value (absolute) or the same step keeping offsets (relative). Changes the device reports are
// not repeated, so the app and the vendor panel never echo each other.
//
// Devices only know pairs (channels 2k and 2k+1) and keep a link flag for them. The flag is kept
// in step: on exactly when a link joins exactly that pair, so the vendor panel and device presets
// agree with the workspace where they can. Pairs a device reports linked are imported as absolute
// links, once.

import type { ChannelRef, Link, LinkKind } from "gazelle-audio-client";

import type { ReadonlySignal } from "../core/signal.ts";
import type { InputsModel } from "./inputs.ts";
import type { MixerModel } from "./mixer.ts";

export type LinkMode = Link["mode"];

export interface LinkPeer {
  deviceId: string;
  channel: number;
  mode: LinkMode;
}

export interface LinksContext {
  links: ReadonlySignal<readonly Link[]>;
  /** Replaces the workspace's links; false when the workspace is not loaded. */
  edit(update: (links: Link[]) => Link[]): boolean;
  /** A device's inputs, or undefined for a device that is absent or of unknown model. */
  inputs(deviceId: string): InputsModel | undefined;
  /** A device's mixers, likewise. */
  mixers(deviceId: string): readonly MixerModel[] | undefined;
}

let nextId = 0;
const key = (ref: ChannelRef) => `${ref.device_id}:${ref.channel}`;

export class LinksModel {
  readonly links: ReadonlySignal<readonly Link[]>;
  readonly #context: LinksContext;

  constructor(context: LinksContext) {
    this.#context = context;
    this.links = context.links;
  }

  /** The link a channel is in, for this kind. */
  linkOf(kind: LinkKind, deviceId: string, channel: number): Link | undefined {
    return this.links.value.find((l) => l.kind === kind && l.members.some((m) => m.device_id === deviceId && m.channel === channel));
  }

  /** The other members of a channel's link, with the link's mode; empty when it is not linked. */
  peers(kind: LinkKind, deviceId: string, channel: number): LinkPeer[] {
    const link = this.links.peek().find((l) => l.kind === kind && l.members.some((m) => m.device_id === deviceId && m.channel === channel));
    if (link === undefined) return [];
    return link.members.filter((m) => m.device_id !== deviceId || m.channel !== channel).map((m) => ({ deviceId: m.device_id, channel: m.channel, mode: link.mode }));
  }

  /**
   * Links channels. A channel already in a link of this kind moves to the new one, and a link left
   * with one channel is removed. Throws RangeError for channels that cannot be linked.
   */
  create(kind: LinkKind, members: readonly ChannelRef[], mode: LinkMode = "absolute"): string | undefined {
    if (members.length < 2) throw new RangeError("a link needs at least two channels");
    const keys = new Set(members.map(key));
    if (keys.size !== members.length) throw new RangeError("a channel is listed twice");
    for (const member of members) this.#check(kind, member);
    const id = `l${(nextId++).toString(36)}${Math.random().toString(36).slice(2, 6)}`;
    const touched: ChannelRef[] = [...members];
    const saved = this.#context.edit((links) => {
      const next: Link[] = [];
      for (const link of links) {
        const kept = link.kind === kind ? link.members.filter((m) => !keys.has(key(m))) : link.members;
        if (kept.length === link.members.length) {
          next.push(link);
          continue;
        }
        touched.push(...link.members);
        if (kept.length >= 2) next.push({ ...link, members: kept });
      }
      next.push({ id, kind, mode, members: members.map((m) => ({ device_id: m.device_id, channel: m.channel })) });
      return next;
    });
    if (!saved) return undefined;
    this.#syncPairs(kind, touched);
    return id;
  }

  remove(id: string): boolean {
    const link = this.links.peek().find((l) => l.id === id);
    if (link === undefined) return false;
    const saved = this.#context.edit((links) => links.filter((l) => l.id !== id));
    if (saved) this.#syncPairs(link.kind, link.members);
    return saved;
  }

  setMode(id: string, mode: LinkMode): boolean {
    return this.#context.edit((links) => links.map((l) => (l.id === id ? { ...l, mode } : l)));
  }

  /** Takes one channel out of its link; a link left with one channel is removed. */
  removeMember(kind: LinkKind, deviceId: string, channel: number): boolean {
    const link = this.links.peek().find((l) => l.kind === kind && l.members.some((m) => m.device_id === deviceId && m.channel === channel));
    if (link === undefined) return false;
    const saved = this.#context.edit((links) =>
      links.flatMap((l) => {
        if (l.id !== link.id) return [l];
        const kept = l.members.filter((m) => m.device_id !== deviceId || m.channel !== channel);
        return kept.length >= 2 ? [{ ...l, members: kept }] : [];
      }),
    );
    if (saved) this.#syncPairs(kind, link.members);
    return saved;
  }

  /** Reads a device's link flags and makes an absolute link of each linked pair not already in a link. */
  async importDevicePairs(deviceId: string): Promise<void> {
    const inputs = this.#context.inputs(deviceId);
    if (inputs === undefined) return;
    const kinds: [LinkKind, number, (pair: number) => boolean][] = [];
    if (await inputs.loadLinks()) {
      kinds.push(["preamp", inputs.pairCount, (pair) => inputs.pairLinked(pair).peek()]);
      for (const d of inputs.digital) if (d.linkPairs > 0) kinds.push([d.kind, d.linkPairs, (pair) => inputs.digitalPairLinked(d.kind, pair).peek()]);
    }
    // Mixer pairs: those linked in any mixer whose state has been read (a channel is in every mix).
    const mixers = (this.#context.mixers(deviceId) ?? []).filter((m) => m.stateKnown.peek());
    if (mixers.length > 0) kinds.push(["mixer", (mixers[0] as MixerModel).channels / 2, (pair) => mixers.some((m) => m.strip(2 * pair).peek().linked)]);
    for (const [kind, pairs, linked] of kinds) {
      for (let pair = 0; pair < pairs; pair++) {
        if (!linked(pair) || this.linkOf(kind, deviceId, 2 * pair) !== undefined || this.linkOf(kind, deviceId, 2 * pair + 1) !== undefined) continue;
        this.create(kind, [{ device_id: deviceId, channel: 2 * pair }, { device_id: deviceId, channel: 2 * pair + 1 }]);
      }
    }
  }

  #check(kind: LinkKind, member: ChannelRef): void {
    const count = kind === "mixer" ? this.#context.mixers(member.device_id)?.[0]?.channels : this.#inputCount(kind, member.device_id);
    if (count === undefined) throw new RangeError(`${member.device_id} is not a connected device of known model`);
    if (!Number.isInteger(member.channel) || member.channel < 0 || member.channel >= count) throw new RangeError(`${member.device_id} has no ${kind} channel ${member.channel + 1} that this app can set`);
  }

  #inputCount(kind: Exclude<LinkKind, "mixer">, deviceId: string): number | undefined {
    const inputs = this.#context.inputs(deviceId);
    if (inputs === undefined) return undefined;
    return kind === "preamp" ? inputs.preampCount : (inputs.digital.find((d) => d.kind === kind && d.editable)?.count ?? 0);
  }

  /** Sets each touched device pair's flag: on exactly when a link joins exactly that pair. */
  #syncPairs(kind: LinkKind, refs: readonly ChannelRef[]): void {
    const links = this.links.peek().filter((l) => l.kind === kind);
    const seen = new Set<string>();
    for (const ref of refs) {
      const pair = Math.floor(ref.channel / 2);
      const pairKey = `${ref.device_id}:${pair}`;
      if (seen.has(pairKey)) continue;
      seen.add(pairKey);
      // Members are distinct, so a link whose members are all in one pair is exactly that pair.
      const exact = links.some((l) => l.members.every((m) => m.device_id === ref.device_id && Math.floor(m.channel / 2) === pair));
      if (kind === "mixer") {
        // A channel is in every mix, so its pair's flag is kept on every mixer.
        for (const mixer of this.#context.mixers(ref.device_id) ?? []) if (mixer.strip(2 * pair).peek().linked !== exact) mixer.setPairLinked(2 * pair, exact);
        continue;
      }
      const inputs = this.#context.inputs(ref.device_id);
      if (inputs === undefined) continue;
      if (kind === "preamp") {
        if (pair < inputs.pairCount && inputs.pairLinked(pair).peek() !== exact) inputs.setPairLinked(pair, exact);
      } else {
        const group = inputs.digital.find((d) => d.kind === kind);
        if (group !== undefined && pair < group.linkPairs && inputs.digitalPairLinked(kind, pair).peek() !== exact) inputs.setDigitalPairLinked(kind, pair, exact);
      }
    }
  }
}
