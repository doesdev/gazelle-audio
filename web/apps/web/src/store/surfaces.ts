// Cross-device mix surfaces (workspace spec §4): named rows of strips drawn from any device, kept in
// the workspace. A surface holds only what to show. Its strips are the devices' own controls (a
// mixer channel in a mix, a mix master, an input, an output), and every change they make goes
// through the same models the device's pages use, so nothing here sends anything to a device.
//
// Mixes are per device: a surface keeps one selected mix for each device (the user's answer to Q8),
// separate from the Mixer page's selection, and a channel or master strip may pin its own. Each
// device has a badge colour, chosen in the workspace or taken from the theme palette, so that two
// devices' strips side by side are never read as one mixer.
//
// Strips are checked here as the server checks them, so an edit the server would refuse is refused
// at once with the same reason rather than undone after its save fails.

import { computed, type ReadonlySignal } from "../core/signal.ts";
import type { Surface, SurfaceStrip, Topology } from "gazelle-audio-client";

/** What a device model has, for checking a strip; undefined for a device of unknown model or not attached. */
export interface SurfaceDeviceModel {
  family: "quadro" | "studio";
  topology: Topology;
  /** Outputs `set_volume` addresses (OutputsModel). */
  outputs: number;
}

export interface SurfacesContext {
  surfaces: ReadonlySignal<readonly Surface[]>;
  /** Changes the surfaces; false when the workspace is not loaded or not connected. */
  edit(update: (surfaces: Surface[]) => Surface[]): boolean;
  colors: ReadonlySignal<Readonly<Record<string, string>>>;
  editColors(update: (colors: Record<string, string>) => Record<string, string>): boolean;
  /** Attached devices' ids, sorted: a device's place among them picks its default colour. */
  deviceIds: ReadonlySignal<readonly string[]>;
  palette: ReadonlySignal<readonly string[]>;
  model(deviceId: string): SurfaceDeviceModel | undefined;
}

/** A strip before it has an id. */
export type NewStrip = Omit<SurfaceStrip, "id">;

const KINDS: readonly SurfaceStrip["kind"][] = ["channel", "master", "input", "output", "port", "label"];
/** Each input kind's routing source type in the topology. */
export const INPUT_TYPES = { preamp: "PREAMP", line: "LINE_IN", adat: "ADAT_IN", spdif: "SPDIF_IN" } as const;
/** Mixes per device, in both families; the server's MIXER_COUNT. */
const MIXES = 4;

let nextId = 0;

export class SurfacesModel {
  readonly list: ReadonlySignal<readonly Surface[]>;
  readonly #context: SurfacesContext;

  constructor(context: SurfacesContext) {
    this.#context = context;
    this.list = computed(() => context.surfaces.value);
  }

  /** A surface by id; reading it is reactive. */
  surface(id: string): Surface | undefined {
    return this.list.value.find((s) => s.id === id);
  }

  /** Makes an empty surface; returns its id, or undefined when the workspace cannot be edited. */
  create(name: string): string | undefined {
    const trimmed = name.trim();
    if (trimmed === "") throw new RangeError("a surface needs a name");
    const id = this.#newId("s", this.list.peek().map((s) => s.id));
    return this.#context.edit((surfaces) => [...surfaces, { id, name: trimmed, mixes: {}, strips: [] }]) ? id : undefined;
  }

  rename(id: string, name: string): boolean {
    const trimmed = name.trim();
    if (trimmed === "") throw new RangeError("a surface needs a name");
    return this.#editSurface(id, (s) => ({ ...s, name: trimmed }));
  }

  remove(id: string): boolean {
    if (!this.#has(id)) return false;
    return this.#context.edit((surfaces) => surfaces.filter((s) => s.id !== id));
  }

  /** Adds a strip at the end; returns its id. Throws with the reason for a strip its device cannot have. */
  addStrip(surfaceId: string, strip: NewStrip): string | undefined {
    this.#check(strip);
    const surface = this.list.peek().find((s) => s.id === surfaceId);
    if (surface === undefined) return undefined;
    const id = this.#newId("st", surface.strips.map((s) => s.id));
    return this.#editSurface(surfaceId, (s) => ({ ...s, strips: [...s.strips, { id, ...strip }] })) ? id : undefined;
  }

  removeStrip(surfaceId: string, stripId: string): boolean {
    if (!this.#hasStrip(surfaceId, stripId)) return false;
    return this.#editSurface(surfaceId, (s) => ({ ...s, strips: s.strips.filter((strip) => strip.id !== stripId) }));
  }

  /** Moves a strip to `index` among the other strips, as a drop does. */
  moveStrip(surfaceId: string, stripId: string, index: number): boolean {
    if (!this.#hasStrip(surfaceId, stripId)) return false;
    return this.#editSurface(surfaceId, (s) => {
      const moving = s.strips.find((strip) => strip.id === stripId) as SurfaceStrip;
      const rest = s.strips.filter((strip) => strip.id !== stripId);
      rest.splice(Math.max(0, Math.min(rest.length, Math.round(index))), 0, moving);
      return { ...s, strips: rest };
    });
  }

  /** The mix a device's strips on a surface follow; reading it is reactive. */
  mixOf(surfaceId: string, deviceId: string): number {
    return this.surface(surfaceId)?.mixes[deviceId] ?? 0;
  }

  setMix(surfaceId: string, deviceId: string, mix: number): boolean {
    this.#checkMix(deviceId, mix);
    return this.#editSurface(surfaceId, (s) => ({ ...s, mixes: { ...s.mixes, [deviceId]: mix } }));
  }

  /** The mix a strip shows: its own pin, else its device's on the surface. Reactive. */
  stripMix(surfaceId: string, strip: SurfaceStrip): number {
    return strip.mix ?? (strip.device_id === undefined ? 0 : this.mixOf(surfaceId, strip.device_id));
  }

  /** Pins a channel or master strip to a mix, or (undefined) lets it follow its device's. */
  pin(surfaceId: string, stripId: string, mix: number | undefined): boolean {
    const strip = this.list.peek().find((s) => s.id === surfaceId)?.strips.find((s) => s.id === stripId);
    if (strip === undefined) return false;
    if (mix !== undefined) this.#checkMix(strip.device_id ?? "", mix);
    return this.#editSurface(surfaceId, (s) => ({
      ...s,
      strips: s.strips.map((one) => {
        if (one.id !== stripId) return one;
        const { mix: _previous, ...rest } = one;
        return mix === undefined ? rest : { ...rest, mix };
      }),
    }));
  }

  /** The devices a surface's strips name, in the order they first appear. Reactive. */
  devicesOf(surfaceId: string): string[] {
    return [...new Set((this.surface(surfaceId)?.strips ?? []).flatMap((s) => (s.device_id === undefined ? [] : [s.device_id])))];
  }

  /**
   * A device's badge colour: the one chosen for it, else the theme palette's by its place among
   * the attached devices, half the palette apart (neighbouring palette colours are alike: red and
   * crimson), so two devices differ until someone chooses. Reactive.
   */
  deviceColor(deviceId: string): string | undefined {
    const chosen = this.#context.colors.value[deviceId];
    if (chosen !== undefined) return chosen;
    const palette = this.#context.palette.value;
    const at = Math.max(0, this.#context.deviceIds.value.indexOf(deviceId));
    const step = Math.max(1, Math.floor(palette.length / 2));
    return palette.length === 0 ? undefined : palette[(at * step) % palette.length];
  }

  /** Chooses a device's badge colour (`#rrggbb`), or clears it back to the palette's. */
  setDeviceColor(deviceId: string, color: string | undefined): boolean {
    if (color !== undefined && !/^#[0-9a-fA-F]{6}$/.test(color)) throw new RangeError(`a device colour is #rrggbb, not ${color}`);
    return this.#context.editColors((colors) => {
      const next = { ...colors };
      if (color === undefined) delete next[deviceId];
      else next[deviceId] = color;
      return next;
    });
  }

  /** The server's checks for a strip, with its wording. */
  #check(strip: NewStrip): void {
    if (!KINDS.includes(strip.kind)) throw new RangeError(`kind must be one of ${KINDS.join(", ")}, not ${JSON.stringify(strip.kind)}`);
    if (strip.kind === "label") {
      if (strip.text === undefined) throw new RangeError("a label strip needs text");
      return;
    }
    if (strip.device_id === undefined) throw new RangeError(`a ${strip.kind} strip needs a device_id`);
    if (strip.mix !== undefined && (!Number.isInteger(strip.mix) || strip.mix < 0 || strip.mix >= MIXES)) throw new RangeError(`mix ${strip.mix} is outside 0..${MIXES - 1}`);
    const model = this.#context.model(strip.device_id);
    if (strip.kind === "channel" && (strip.channel === undefined || strip.channel === "")) throw new RangeError("a channel strip needs a channel id");
    if (strip.kind === "input") {
      if (strip.input === undefined) throw new RangeError("an input strip needs an input");
      const type = INPUT_TYPES[strip.input.kind];
      if (type === undefined) throw new RangeError(`input kind must be one of ${Object.keys(INPUT_TYPES).join(", ")}, not ${JSON.stringify(strip.input.kind)}`);
      if (model !== undefined) {
        const count = model.topology.inputs.filter((g) => g.type === type).reduce((sum, g) => sum + g.channels, 0);
        if (count === 0) throw new RangeError(`the ${model.family} has no ${strip.input.kind} inputs`);
        if (!Number.isInteger(strip.input.channel) || strip.input.channel < 0 || strip.input.channel >= count) throw new RangeError(`the ${model.family} has ${strip.input.kind} inputs 0..${count - 1}, not ${strip.input.channel}`);
      }
    }
    if (strip.kind === "output") {
      if (strip.output === undefined) throw new RangeError("an output strip needs an output");
      if (model !== undefined && (!Number.isInteger(strip.output) || strip.output < 0 || strip.output >= model.outputs)) throw new RangeError(`the ${model.family} has outputs 0..${model.outputs - 1}, not ${strip.output}`);
    }
    if (strip.kind === "port") {
      if (strip.port === undefined) throw new RangeError("a port strip needs a port");
      if (strip.port !== "SPDIF_OUT" && strip.port !== "ADAT_OUT") throw new RangeError(`port must be SPDIF_OUT or ADAT_OUT, not ${JSON.stringify(strip.port)}`);
      const width = strip.port === "ADAT_OUT" ? 8 : 2;
      const first = strip.first ?? 0;
      if (!Number.isInteger(first) || first < 0 || first % width !== 0) throw new RangeError(`an ${strip.port === "ADAT_OUT" ? "ADAT" : "S/PDIF"} port starts at a multiple of ${width}, not ${first}`);
      if (model !== undefined) {
        const channels = model.topology.outputs.filter((g) => g.type === strip.port).reduce((sum, g) => sum + g.channels, 0);
        if (channels === 0) throw new RangeError(`the ${model.family} has no ${strip.port}`);
        if (first + width > channels) throw new RangeError(`the ${model.family}'s ${strip.port} has channels 0..${channels - 1}, not ${first}..${first + width - 1}`);
      }
    }
  }

  #checkMix(deviceId: string, mix: number): void {
    const count = this.#context.model(deviceId)?.topology.mixers.count ?? 0;
    if (!Number.isInteger(mix) || mix < 0 || mix >= count) throw new RangeError(count === 0 ? `${deviceId} has no known mixes` : `${deviceId} has mixes 0..${count - 1}, not ${mix}`);
  }

  #has(id: string): boolean {
    return this.list.peek().some((s) => s.id === id);
  }

  #hasStrip(surfaceId: string, stripId: string): boolean {
    return this.list.peek().find((s) => s.id === surfaceId)?.strips.some((s) => s.id === stripId) ?? false;
  }

  #editSurface(id: string, change: (surface: Surface) => Surface): boolean {
    if (!this.#has(id)) return false;
    return this.#context.edit((surfaces) => surfaces.map((s) => (s.id === id ? change(s) : s)));
  }

  #newId(prefix: string, taken: readonly string[]): string {
    let id: string;
    do id = `${prefix}${(nextId++).toString(36)}${Math.random().toString(36).slice(2, 6)}`;
    while (taken.includes(id));
    return id;
  }
}
