// A device's mixer channels as the user builds them (plan 2026-09-16). A channel is nothing until
// it has an input and a main mix; then its input is routed to its mixer input slot in the main mix
// and every send mix, and muted in the others. The layout (which channels exist, their order,
// names, groups, and mix names) is kept in the workspace; routing and levels stay on the device.
//
// Every change works out which mixes the channel fed before and after, mutes the ones it left and
// routes the ones it joined (all of them when its input changed), so the device always matches
// the layout. On the Quadro, mixer inputs 1-6 carry the effect returns in the vendor's layout, so
// channels start at slot 7 (the user's choice). A device with no layout yet can import one from
// its current routing: one channel per slot that is routed (not MUTE) in any mix.

import { computed, signal, type ReadonlySignal, type Signal } from "../core/signal.ts";
import type { DeviceMixer, MixConfig, MixerChannel, MixerGroup, RouteSource, SavedLayout, Topology } from "gazelle-audio-client";
import { PAN_CENTRE } from "./mixer.ts";
import type { MixerModel } from "./mixer.ts";
import { PROFILES } from "./profiles.ts";
import type { RouteSlot, RoutingModel } from "./routing.ts";

/** Quadro mixer inputs 1-6 carry the effect returns (vendor layout), so channels start after them. */
export const QUADRO_EFFECT_SLOTS = 6;
const SLOTS = 32;

export interface ChannelsContext {
  deviceId: string;
  family: "quadro" | "studio";
  topology: Topology;
  /** The device's layout in the workspace, or undefined when it has none yet. */
  layout: ReadonlySignal<DeviceMixer | undefined>;
  /** Changes the layout (starting from an empty one); false when the workspace is not loaded. */
  edit(update: (layout: DeviceMixer) => DeviceMixer): boolean;
  routing: Pick<RoutingModel, "route" | "load" | "destination" | "mute">;
  notify(text: string): void;
  /** Every saved layout in the workspace, of any model. */
  saved: ReadonlySignal<readonly SavedLayout[]>;
  /** Changes the saved layouts; false when the workspace is not loaded. */
  editSaved(update: (layouts: SavedLayout[]) => SavedLayout[]): boolean;
  /** One of the device's mixes, for mono. */
  mixer(mix: number): Pick<MixerModel, "strip" | "sendPan">;
  /** Where the metered mix is kept: the store remembers it per device. A signal of its own without. */
  meteredMix?: { get(): ReadonlySignal<number>; set(mix: number): void };
}

export const emptyLayout = (): DeviceMixer => ({ mixes: [], groups: [], channels: [] });

/** A place a mix can play: a destination group's left/right pair (or a lone last channel). */
export interface OutputPair {
  /** Destination group position in the topology's `outputs`. */
  destination: number;
  /** The left channel. */
  channel: number;
  label: string;
}

let nextId = 0;

export class ChannelsModel {
  readonly deviceId: string;
  /** The first slot channels may use. */
  readonly firstSlot: number;
  readonly mixCount: number;
  readonly layout: ReadonlySignal<DeviceMixer>;
  /** The mix the device's meters show; it meters one mix at a time. It is the Mixer page's selected mix. */
  readonly meteredMix: Signal<number>;
  readonly #context: ChannelsContext;
  /** Each mix's MIX IN destination position in the topology. */
  readonly #mixInputs: readonly number[];
  /** Each mix's MIX OUT source position in the topology. */
  readonly #mixSources: readonly number[];
  readonly #pairs: readonly OutputPair[];
  readonly #outputs = new Map<number, ReadonlySignal<readonly OutputPair[]>>();

  constructor(context: ChannelsContext) {
    this.#context = context;
    const kept = context.meteredMix;
    this.meteredMix =
      kept === undefined
        ? signal(0)
        : {
            get value() {
              return kept.get().value;
            },
            set value(mix: number) {
              kept.set(mix);
            },
            peek: () => kept.get().peek(),
          };
    this.deviceId = context.deviceId;
    this.firstSlot = context.family === "quadro" ? QUADRO_EFFECT_SLOTS : 0;
    this.mixCount = context.topology.mixers.count;
    this.#mixInputs = context.topology.mixers.inputGroups.map((id) => {
      const position = context.topology.outputs.findIndex((g) => g.id === id);
      if (position < 0) throw new Error(`${context.topology.family} topology has no destination ${id}`);
      return position;
    });
    this.#mixSources = context.topology.mixers.outputGroups.map((id) => {
      const position = context.topology.inputs.findIndex((g) => g.id === id);
      if (position < 0) throw new Error(`${context.topology.family} topology has no source ${id}`);
      return position;
    });
    const pairs: OutputPair[] = [];
    context.topology.outputs.forEach((group, destination) => {
      if (group.type === "MIXER_IN") return;
      for (let channel = 0; channel < group.channels; channel += 2) {
        const label = group.channels <= 2 ? group.name : channel + 1 < group.channels ? `${group.name} ${channel + 1}/${channel + 2}` : `${group.name} ${channel + 1}`;
        pairs.push({ destination, channel, label });
      }
    });
    this.#pairs = pairs;
    const empty = emptyLayout();
    this.layout = computed(() => context.layout.value ?? empty);
  }

  /** Every place a mix can play, in topology order; mixer inputs are channels' business and are left out. */
  outputPairs(): readonly OutputPair[] {
    return this.#pairs;
  }

  /** Reads the routing of every output destination, so mixes show where the device sends them. */
  async loadOutputs(): Promise<void> {
    for (const destination of new Set(this.#pairs.map((p) => p.destination))) await this.#context.routing.load(destination);
  }

  /** Where a mix plays: the pairs whose left and right take the mix's left and right. */
  mixOutputs(mix: number): ReadonlySignal<readonly OutputPair[]> {
    this.#checkMix(mix);
    let outputs = this.#outputs.get(mix);
    if (outputs === undefined) {
      const source = this.#mixSources[mix] as number;
      outputs = computed(() =>
        this.#pairs.filter((pair) => {
          const slots = this.#context.routing.destination(pair.destination).value;
          if (slots === undefined) return false;
          const left = slots[pair.channel];
          const right = slots[pair.channel + 1];
          const stereo = pair.channel + 1 < (this.#context.topology.outputs[pair.destination]?.channels ?? 0);
          return left?.source === source && left.channel === 0 && (!stereo || (right?.source === source && right.channel === 1));
        }),
      );
      this.#outputs.set(mix, outputs);
    }
    return outputs;
  }

  /** Sends a mix to an output pair (its left and right), or mutes that pair. */
  setMixOutput(mix: number, pair: { destination: number; channel: number }, on: boolean): Promise<boolean> {
    this.#checkMix(mix);
    const group = this.#context.topology.outputs[pair.destination];
    if (group === undefined || group.type === "MIXER_IN" || !Number.isInteger(pair.channel) || pair.channel % 2 !== 0 || pair.channel < 0 || pair.channel >= group.channels) {
      throw new RangeError(`no output pair ${pair.destination}:${pair.channel}`);
    }
    const source = this.#mixSources[mix] as number;
    const writes = [this.#context.routing.route(pair.destination, pair.channel, on ? { source, channel: 0 } : null)];
    if (pair.channel + 1 < group.channels) writes.push(this.#context.routing.route(pair.destination, pair.channel + 1, on ? { source, channel: 1 } : null));
    return Promise.all(writes).then((sent) => sent.every(Boolean));
  }

  /** Whether this device has a layout yet; without one it can be imported from the device. */
  get configured(): boolean {
    return this.#context.layout.peek() !== undefined;
  }

  channel(id: string): MixerChannel | undefined {
    return this.layout.peek().channels.find((c) => c.id === id);
  }

  /** A channel routes, and its mixer controls work, once it has an input and a main mix. */
  isActive(channel: MixerChannel): boolean {
    return channel.source !== undefined && channel.main_mix !== undefined;
  }

  mixName(mix: number): string {
    return this.layout.value.mixes[mix]?.name || `Mix ${mix + 1}`;
  }

  /**
   * What a channel is called: the name typed for it, else its input's name, else its mixer input
   * (the user, 2026-09-16). An unnamed channel follows its input as the input changes.
   */
  displayName(channel: MixerChannel): string {
    if (channel.name !== "") return channel.name;
    return channel.source === undefined ? `Ch ${channel.slot + 1}` : this.sourceLabel(channel.source);
  }

  sourceLabel(source: RouteSource): string {
    const group = this.#context.topology.inputs[source.group];
    if (group === undefined) return `Source ${source.group}:${source.channel + 1}`;
    return group.channels > 1 ? `${group.name} ${source.channel + 1}` : group.name;
  }

  /** Adds an inactive channel on the lowest free slot; undefined (with a notice) when none is free. */
  add(): string | undefined {
    const used = new Set(this.layout.peek().channels.map((c) => c.slot));
    let slot = this.firstSlot;
    while (slot < SLOTS && used.has(slot)) slot++;
    if (slot >= SLOTS) {
      const reserved = this.firstSlot > 0 ? ` (inputs 1–${this.firstSlot} carry the effect returns)` : "";
      this.#context.notify(`All ${SLOTS - this.firstSlot} mixer channels are in use${reserved}.`);
      return undefined;
    }
    const id = this.#newId();
    const added = this.#context.edit((layout) => ({ ...layout, channels: [...layout.channels, { id, name: "", slot, sends: [] }] }));
    return added ? id : undefined;
  }

  /** Removes a channel, muting its slot in every mix it fed. */
  remove(id: string): Promise<boolean> {
    const before = this.#require(id);
    if (!this.#context.edit((layout) => ({ ...layout, channels: layout.channels.filter((c) => c.id !== id) }))) return Promise.resolve(false);
    return this.#apply(before, withOptional(before, "source", undefined));
  }

  move(id: string, index: number): boolean {
    this.#require(id);
    return this.#context.edit((layout) => {
      const channels = layout.channels.filter((c) => c.id !== id);
      channels.splice(Math.max(0, Math.min(channels.length, index)), 0, layout.channels.find((c) => c.id === id) as MixerChannel);
      return { ...layout, channels };
    });
  }

  /**
   * Moves a channel to `index` among the other channels, as a drop does, and sets its group from
   * where it lands: between two members of one group it joins that group, anywhere else it has
   * none. So a drag never splits a group.
   */
  place(id: string, index: number): boolean {
    const channel = this.#require(id);
    return this.#context.edit((layout) => {
      const rest = layout.channels.filter((c) => c.id !== id);
      const at = Math.max(0, Math.min(rest.length, Math.round(index)));
      const before = rest[at - 1]?.group;
      const group = before !== undefined && before === rest[at]?.group ? before : undefined;
      rest.splice(at, 0, withOptional(channel, "group", group));
      return { ...layout, channels: rest };
    });
  }

  rename(id: string, name: string): boolean {
    return this.#edit(id, (c) => ({ ...c, name }));
  }

  /**
   * Puts a channel in a group, or takes it out, keeping each group's channels together: joining
   * places it after the group's last member, leaving places it after the group it left.
   */
  setGroup(id: string, group: string | undefined): boolean {
    if (group !== undefined && !this.layout.peek().groups.some((g) => g.id === group)) throw new RangeError(`no group ${group}`);
    const channel = this.#require(id);
    return this.#context.edit((layout) => {
      const at = layout.channels.findIndex((c) => c.id === id);
      const rest = layout.channels.filter((c) => c.id !== id);
      const anchor = group ?? channel.group;
      let last = -1;
      if (anchor !== undefined) for (let i = rest.length - 1; i >= 0 && last < 0; i--) if (rest[i]?.group === anchor) last = i;
      rest.splice(last >= 0 ? last + 1 : at, 0, withOptional(channel, "group", group));
      return { ...layout, channels: rest };
    });
  }

  groupOf(id: string): MixerGroup | undefined {
    const group = this.channel(id)?.group;
    return group === undefined ? undefined : this.layout.peek().groups.find((g) => g.id === group);
  }

  /** Adds a group, optionally starting with channels (which move together). */
  addGroup(name: string, members: readonly string[] = []): string | undefined {
    const id = this.#newId("g");
    if (!this.#context.edit((layout) => ({ ...layout, groups: [...layout.groups, { id, name, collapsed: false }] }))) return undefined;
    for (const member of members) this.setGroup(member, id);
    return id;
  }

  /** Colours a group (`#rrggbb`), or clears its colour. */
  setGroupColor(id: string, color: string | undefined): boolean {
    if (color !== undefined && !/^#[0-9a-fA-F]{6}$/.test(color)) throw new RangeError(`a group colour is #rrggbb, not ${color}`);
    return this.#context.edit((layout) => ({ ...layout, groups: layout.groups.map((g) => (g.id === id ? withOptional(g, "color", color) : g)) }));
  }

  renameGroup(id: string, name: string): boolean {
    return this.#context.edit((layout) => ({ ...layout, groups: layout.groups.map((g) => (g.id === id ? { ...g, name } : g)) }));
  }

  toggleGroup(id: string): boolean {
    return this.#context.edit((layout) => ({ ...layout, groups: layout.groups.map((g) => (g.id === id ? { ...g, collapsed: !g.collapsed } : g)) }));
  }

  /** Removes a group; its channels stay, ungrouped. */
  removeGroup(id: string): boolean {
    return this.#context.edit((layout) => ({
      ...layout,
      groups: layout.groups.filter((g) => g.id !== id),
      channels: layout.channels.map((c) => (c.group === id ? withOptional(c, "group", undefined) : c)),
    }));
  }

  renameMix(mix: number, name: string): boolean {
    this.#checkMix(mix);
    return this.#context.edit((layout) => {
      const mixes = Array.from({ length: Math.max(layout.mixes.length, mix + 1) }, (_, i) => layout.mixes[i] ?? {});
      mixes[mix] = name.trim() === "" ? {} : { name };
      return { ...layout, mixes };
    });
  }

  /** Chooses the channel's input (undefined for none, which mutes what it fed). */
  setSource(id: string, source: RouteSource | undefined): Promise<boolean> {
    return this.#update(id, (c) => withOptional(c, "source", source));
  }

  /** Chooses the mix the channel's fader controls; it stops being a send to that mix. */
  setMainMix(id: string, mix: number | undefined): Promise<boolean> {
    if (mix !== undefined) this.#checkMix(mix);
    return this.#update(id, (c) => ({ ...withOptional(c, "main_mix", mix), sends: c.sends.filter((s) => s !== mix) }));
  }

  /** Sends the channel to another mix too, or stops; false for its own main mix. */
  setSend(id: string, mix: number, on: boolean): Promise<boolean> {
    this.#checkMix(mix);
    const channel = this.#require(id);
    if (mix === channel.main_mix) return Promise.resolve(false);
    return this.#update(id, (c) => ({ ...c, sends: on ? [...new Set([...c.sends, mix])].sort((a, b) => a - b) : c.sends.filter((s) => s !== mix) }));
  }

  /**
   * Builds a layout from the device's mixer routing when there is none: one channel per slot
   * routed in any mix, main mix the first such mix, sends the others with the same source. A slot
   * routed to different sources in different mixes keeps only the first; the rest stay on the
   * device untouched. With nothing routed (or no reports, as in dry run) the layout is one
   * inactive channel. Resolves false when a layout already exists.
   */
  async importFromDevice(): Promise<boolean> {
    if (this.configured) return false;
    const found = new Map<number, { source: RouteSlot; mixes: number[] }>();
    for (let mix = 0; mix < this.mixCount; mix++) {
      const destination = this.#mixInputs[mix] as number;
      if (!(await this.#context.routing.load(destination))) continue;
      const slots = this.#context.routing.destination(destination).peek() ?? [];
      for (let slot = this.firstSlot; slot < slots.length; slot++) {
        const routed = slots[slot];
        if (routed === undefined || routed.source === this.#context.routing.mute) continue;
        const entry = found.get(slot);
        if (entry === undefined) found.set(slot, { source: routed, mixes: [mix] });
        else if (entry.source.source === routed.source && entry.source.channel === routed.channel) entry.mixes.push(mix);
      }
    }
    if (this.configured) return false;
    const channels: MixerChannel[] = [...found.entries()]
      .sort(([a], [b]) => a - b)
      .map(([slot, { source, mixes }]) => {
        const routeSource = { group: source.source, channel: source.channel };
        return { id: this.#newId(), name: this.sourceLabel(routeSource), slot, source: routeSource, main_mix: mixes[0] as number, sends: mixes.slice(1) };
      });
    if (channels.length === 0) channels.push({ id: this.#newId(), name: "", slot: this.firstSlot, sends: [] });
    return this.#context.edit(() => ({ ...emptyLayout(), channels }));
  }

  /**
   * Replaces a layout with no channel set up by one of the device's starting layouts (profiles.ts):
   * its channels on the first free slots, its mix names, and each channel routed into its mixes.
   * Rejects when a channel is already set up, so a working mixer is never replaced.
   */
  async applyProfile(id: string): Promise<boolean> {
    const profile = PROFILES[this.#context.family].find((p) => p.id === id);
    if (profile === undefined) throw new RangeError(`the ${this.#context.family} has no starting layout ${id}`);
    const channels: MixerChannel[] = profile.channels.map((c, i) => {
      const group = this.#context.topology.inputs.findIndex((g) => g.type === c.input);
      if (group < 0) throw new RangeError(`the ${this.#context.family} has no ${c.input} input`);
      return { id: this.#newId(), name: c.name, slot: this.firstSlot + i, source: { group, channel: c.channel }, main_mix: c.main_mix, sends: [...c.sends] };
    });
    return this.#startFrom({ mixes: profile.mixes.map((name) => ({ name })), groups: [], channels });
  }

  /** The layouts saved for this device's model, in the order they were saved. */
  savedLayouts(): readonly SavedLayout[] {
    return this.#context.saved.value.filter((l) => l.family === this.#context.family);
  }

  /** Saves the current mixer layout by name for this model; returns the new layout's id. */
  saveLayout(name: string): string | undefined {
    const trimmed = name.trim();
    if (trimmed === "") throw new RangeError("a saved layout needs a name");
    const id = this.#newId("layout");
    const mixer = structuredClone(this.layout.peek());
    return this.#context.editSaved((layouts) => [...layouts, { id, name: trimmed, family: this.#context.family, mixer }]) ? id : undefined;
  }

  /** Starts from a saved layout, as `applyProfile` starts from a built-in one: fresh ids, routed channels. */
  async applySavedLayout(id: string): Promise<boolean> {
    const saved = this.savedLayouts().find((l) => l.id === id);
    if (saved === undefined) throw new RangeError(`no saved ${this.#context.family} layout ${id}`);
    const groupIds = new Map(saved.mixer.groups.map((g) => [g.id, this.#newId("grp")]));
    const channels = saved.mixer.channels.map((c): MixerChannel => {
      const { group, ...rest } = structuredClone(c);
      const renamed = group === undefined ? undefined : groupIds.get(group);
      return { ...rest, id: this.#newId(), ...(renamed === undefined ? {} : { group: renamed }) };
    });
    return this.#startFrom({ mixes: structuredClone(saved.mixer.mixes), groups: saved.mixer.groups.map((g) => ({ ...g, id: groupIds.get(g.id) as string })), channels });
  }

  /** Whether a mix is summed to mono (decision P57). Reading it is reactive. */
  isMono(mix: number): boolean {
    this.#checkMix(mix);
    return this.layout.value.mixes[mix]?.mono !== undefined;
  }

  /**
   * Sums a mix to mono, or ends it. Neither model has a mono switch, so the app pans every channel
   * in the mix to centre, keeping the pans in the workspace, and pans them back when mono ends. The
   * device's panning law sets the level of what is summed, so no level is changed (the user's call).
   */
  setMono(mix: number, on: boolean): boolean {
    this.#checkMix(mix);
    const mixer = this.#context.mixer(mix);
    const saved = this.layout.peek().mixes[mix]?.mono;
    if (on === (saved !== undefined)) return true;
    const slots = this.layout.peek().channels.map((c) => c.slot).sort((a, b) => a - b);
    const editMix = (change: (config: MixConfig) => MixConfig) =>
      this.#context.edit((layout) => {
        const mixes = Array.from({ length: Math.max(layout.mixes.length, mix + 1) }, (_, i) => layout.mixes[i] ?? {});
        mixes[mix] = change(mixes[mix] as MixConfig);
        return { ...layout, mixes };
      });
    if (on) {
      const pans = Object.fromEntries(slots.map((slot) => [String(slot), mixer.strip(slot).peek().pan]));
      if (!editMix((config) => ({ ...config, mono: { pans } }))) return false;
      for (const slot of slots) mixer.sendPan(slot, PAN_CENTRE);
      return true;
    }
    if (!editMix(({ mono: _mono, ...config }) => config)) return false;
    for (const [slot, pan] of Object.entries(saved?.pans ?? {}).sort(([a], [b]) => Number(a) - Number(b))) mixer.sendPan(Number(slot), pan);
    return true;
  }

  removeSavedLayout(id: string): boolean {
    if (!this.savedLayouts().some((l) => l.id === id)) return false;
    return this.#context.editSaved((layouts) => layouts.filter((l) => l.id !== id));
  }

  /** Replaces a mixer with no channel set up by `mixer` and routes its channels. */
  async #startFrom(mixer: DeviceMixer): Promise<boolean> {
    if (this.layout.peek().channels.some((c) => this.isActive(c))) throw new Error("a starting layout only replaces a mixer with no channel set up");
    if (!this.#context.edit(() => ({ ...emptyLayout(), ...mixer }))) return false;
    let routed = true;
    for (const c of mixer.channels) routed = (await this.#apply({ id: c.id, name: c.name, slot: c.slot, sends: [] }, c)) && routed;
    return routed;
  }

  #require(id: string): MixerChannel {
    const channel = this.channel(id);
    if (channel === undefined) throw new RangeError(`no channel ${id}`);
    return channel;
  }

  #checkMix(mix: number): void {
    if (!Number.isInteger(mix) || mix < 0 || mix >= this.mixCount) throw new RangeError(`mix ${mix} is outside 0..${this.mixCount - 1}`);
  }

  #edit(id: string, change: (channel: MixerChannel) => MixerChannel): boolean {
    this.#require(id);
    return this.#context.edit((layout) => ({ ...layout, channels: layout.channels.map((c) => (c.id === id ? change(c) : c)) }));
  }

  #update(id: string, change: (channel: MixerChannel) => MixerChannel): Promise<boolean> {
    const before = this.#require(id);
    const after = change(before);
    if (!this.#edit(id, () => after)) return Promise.resolve(false);
    return this.#apply(before, after);
  }

  /** The mixes a channel is routed into: its main mix and sends, once it is active. */
  #fed(channel: MixerChannel): number[] {
    return this.isActive(channel) ? [channel.main_mix as number, ...channel.sends] : [];
  }

  /** Routes the device from `before` to `after`: mutes mixes left, routes mixes joined or changed. */
  async #apply(before: MixerChannel, after: MixerChannel): Promise<boolean> {
    const was = this.#fed(before);
    const now = this.#fed(after);
    const sourceChanged = before.source?.group !== after.source?.group || before.source?.channel !== after.source?.channel || before.slot !== after.slot;
    const writes: Promise<boolean>[] = [];
    for (const mix of was) if (!now.includes(mix) || before.slot !== after.slot) writes.push(this.#route(mix, before.slot, undefined));
    for (const mix of now) if (!was.includes(mix) || sourceChanged) writes.push(this.#route(mix, after.slot, after.source));
    return (await Promise.all(writes)).every(Boolean);
  }

  #route(mix: number, slot: number, source: RouteSource | undefined): Promise<boolean> {
    const destination = this.#mixInputs[mix] as number;
    try {
      return this.#context.routing.route(destination, slot, source === undefined ? null : { source: source.group, channel: source.channel });
    } catch (error) {
      this.#context.notify(`Mix ${mix + 1} was not routed: ${error instanceof Error ? error.message : String(error)}`);
      return Promise.resolve(false);
    }
  }

  #newId(prefix = "ch"): string {
    const taken = new Set([...this.layout.peek().channels.map((c) => c.id), ...this.layout.peek().groups.map((g) => g.id)]);
    let id: string;
    do id = `${prefix}${(nextId++).toString(36)}${Math.random().toString(36).slice(2, 6)}`;
    while (taken.has(id));
    return id;
  }
}

/** The object with `key` set, or without it when `value` is undefined (the server omits unset fields). */
function withOptional<T extends object, K extends keyof T>(object: T, key: K, value: T[K] | undefined): T {
  const copy = { ...object };
  if (value === undefined) delete copy[key];
  else copy[key] = value;
  return copy;
}
