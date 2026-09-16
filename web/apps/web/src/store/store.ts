// The app's state (spec §6): the only layer that imports gazelle-audio-client. Elements read
// signals and call methods here; they never talk to the client or the server.
//
// - Connection: status, server info and devices follow the client's events. Controls are only
//   enabled while `connected` (spec §6.3).
// - Workspace: edits apply at once and are saved by a debounced PUT. A failed save restores the
//   last workspace the server accepted and posts an error notice; unsaved edits made meanwhile
//   are discarded with it.
// - Cyclic reports: each field is its own signal, written through the frame writer, so a control
//   re-renders only when its own value changes, at most once per frame.
// - Themes: built-in and community sources from the app, user themes from the server; the pick
//   is remembered per browser.

import { connect, GazelleError, topologies, type Client, type DeviceDescriptor, type ChannelRef, type DeviceMixer, type Group, type Link, type LinkKind, type MixerChannel, type RouteSource, type ServerInfo, type Status, type Topology, type Workspace } from "gazelle-audio-client";

import { ChannelsModel, emptyLayout } from "./channels.ts";
import { ECHO_HOLD_MS, InputsModel } from "./inputs.ts";
import { LinksModel } from "./links.ts";
import { OutputsModel } from "./outputs.ts";
import { MixerModel } from "./mixer.ts";
import { RoutingModel, type RoutingRead } from "./routing.ts";
import { clampStripWidth, parseMixerWidth, parsePanels, persisted, STRIP_WIDTH_DEFAULT, type MixerWidth, type PanelState } from "./preferences.ts";

export type { MixerWidth, PanelState };
export const MIXER_WIDTH_STORAGE_KEY = "gazelle.mixer.width";
export const PANELS_STORAGE_KEY = "gazelle.layout.panels";

// Elements may not import the client (spec §6.1), so the store passes on the data types they show.
export type { ChannelRef, DeviceDescriptor, DeviceMixer, Group, Link, LinkKind, MixerChannel, RouteSource, ServerInfo, Status, Topology, Workspace };

/** The most recent command the mixer sent, with the bytes the server reported. */
export interface SentCommand {
  deviceId: string;
  command: string;
  hex: string;
  dryRun: boolean;
}

import { frameWriter, type FrameWriter, type RequestFrame } from "../core/frame.ts";
import { batch, computed, signal, type ReadonlySignal, type Signal } from "../core/signal.ts";
import { BASE_THEME, resolveThemes, type ResolvedTheme, type ThemeProblem, type ThemeSource } from "../themes/theme.ts";

export const SAVE_DEBOUNCE_MS = 300;
/**
 * The device's own preset slots, numbered 1..5 as both panels number them (`bind_presets`
 * enumerates from 1). These are the device's memory, not the workspace layouts (decision 0011).
 */
export const PRESET_SLOTS = 5;

/** The panels' brightness range, 0..100 (their sliders' max_value). */
export const BRIGHTNESS_MAX = 100;

/**
 * How much a centre-panned signal is attenuated, in `set_panning_law`'s index order — which is the
 * Quadro settings dialog's own order, not ascending. The Studio+ has no such command. Every pan in
 * the mixer, and so the mono downmix built on them, is heard through whichever of these is set.
 */
export const PANNING_LAWS = ["0 dB", "-6 dB", "-3 dB", "-4.5 dB"] as const;

/**
 * The test oscillator's frequencies, in `set_sine_gen`'s index order. Both panels offer these two,
 * though the field is two bits wide; the other two codes are not used and are not offered here.
 */
export const OSCILLATOR_FREQUENCIES = ["1 kHz", "440 Hz"] as const;

/** The test oscillator's level, shared by both tones, in `set_sine_gen`'s index order. */
export const OSCILLATOR_LEVELS = ["0 dBFS", "-6 dBFS", "-12 dBFS", "-18 dBFS"] as const;

/**
 * The test oscillator: one tone per side, each with its own frequency, over a shared level. The
 * device carries the two as mute bits; here they are tones that are on, which is how they are used.
 */
export interface OscillatorState {
  /** Frequencies as indexes into [`OSCILLATOR_FREQUENCIES`]. */
  left: number;
  right: number;
  /** An index into [`OSCILLATOR_LEVELS`]. */
  level: number;
  onLeft: boolean;
  onRight: boolean;
}

/** The sample rates both panels offer, in `set_samp_rate`'s index order. */
export const SAMPLE_RATES = ["32 kHz", "44.1 kHz", "48 kHz", "88.2 kHz", "96 kHz", "176.4 kHz", "192 kHz"] as const;

/**
 * Clock sources in `set_sync_source`'s index order, per model, as each panel lists them
 * (`app/ui/cpanel.py` and `zenstudiotb/ui/widgets/comboboxes.py`). The Studio+'s internal clock is
 * its oven-controlled oscillator, and it alone has a word clock input.
 */
export const CLOCK_SOURCES: Readonly<Record<"quadro" | "studio", readonly string[]>> = {
  quadro: ["Internal", "ADAT x1", "ADAT x2", "ADAT x4", "S/PDIF", "USB"],
  studio: ["Oven", "Word clock", "ADAT", "ADAT x2", "ADAT x4", "S/PDIF", "USB"],
};

/** What the device reports about its clock. */
export interface ClockState {
  /** Index into the model's `CLOCK_SOURCES`. */
  source: number;
  /** The measured rate in Hz, from the status report's three frequency bytes. */
  hz: number;
  locked: boolean;
  /** The rate the device is on, as an index into [`SAMPLE_RATES`] (the report's `base_index`). */
  rate: number;
}
export const THEME_STORAGE_KEY = "gazelle.theme";

export interface Timers {
  setTimeout(callback: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

export type { KeyValueStorage } from "./preferences.ts";
import type { KeyValueStorage } from "./preferences.ts";

export interface StoreDependencies {
  timers?: Timers;
  /** Where the theme pick is remembered; defaults to localStorage when there is one. */
  storage?: KeyValueStorage;
  requestFrame?: RequestFrame;
  /** Built-in and community themes; gazelle-dark must be among them. */
  themeSources?: readonly ThemeSource[];
}

export interface Notice {
  id: number;
  level: "error" | "warning" | "info";
  message: string;
}

export interface ThemeCatalog {
  themes: ResolvedTheme[];
  problems: ThemeProblem[];
}

const defaultTimers: Timers = {
  setTimeout: (callback, ms) => globalThis.setTimeout(callback, ms),
  clearTimeout: (handle) => globalThis.clearTimeout(handle as Parameters<typeof globalThis.clearTimeout>[0]),
};

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value) && !ArrayBuffer.isView(value);
}

/** Structural equality for decoded report values, so an unchanged byte array is not a change. */
export function sameValue(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (a instanceof Uint8Array && b instanceof Uint8Array) return a.length === b.length && a.every((byte, i) => byte === b[i]);
  if (Array.isArray(a) && Array.isArray(b)) return a.length === b.length && a.every((item, i) => sameValue(item, b[i]));
  if (isRecord(a) && isRecord(b)) {
    const keys = Object.keys(a);
    return keys.length === Object.keys(b).length && keys.every((key) => sameValue(a[key], b[key]));
  }
  return false;
}

/** The server's spelling of a report id: `0x` and uppercase hex digits. */
function reportKey(id: string): string {
  const n = Number.parseInt(id, 16);
  return Number.isNaN(n) ? id : `0x${n.toString(16).toUpperCase()}`;
}

/** The name a person sees for a device: its alias, else its model, else its id. */
export function displayName(device: DeviceDescriptor, workspace: Workspace | undefined): string {
  return workspace?.aliases[device.id] ?? device.model ?? device.id;
}

function mapGroups(groups: readonly Group[], id: string, change: (group: Group) => Group): Group[] {
  return groups.map((group) => (group.id === id ? change(group) : { ...group, children: mapGroups(group.children, id, change) }));
}

function sortDevices(devices: ReadonlyMap<string, DeviceDescriptor>): DeviceDescriptor[] {
  return [...devices.values()].sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
}

function once(fn: () => void): () => void {
  let done = false;
  return () => {
    if (!done) {
      done = true;
      fn();
    }
  };
}

/** Connects to the server at `baseUrl` and loads the workspace and user themes. */
export async function openStore(baseUrl: string, dependencies: StoreDependencies = {}): Promise<Store> {
  const store = new Store(await connect(baseUrl), dependencies);
  await store.start();
  return store;
}

export class Store {
  readonly #client: Client;
  readonly #timers: Timers;
  readonly #storage: KeyValueStorage | undefined;
  readonly #frames: FrameWriter;
  readonly #themeSources: readonly ThemeSource[];
  readonly #unsubscribe: (() => void)[] = [];

  readonly #status: Signal<Status>;
  readonly #server: Signal<ServerInfo>;
  readonly #devices: Signal<readonly DeviceDescriptor[]>;
  readonly #workspace = signal<Workspace | undefined>(undefined);
  readonly #saving = signal(false);
  readonly #notices = signal<readonly Notice[]>([]);
  readonly #userThemes = signal<readonly ThemeSource[]>([]);
  readonly #userThemeProblems = signal<readonly ThemeProblem[]>([]);
  readonly #themeId: Signal<string>;
  readonly #fields = new Map<string, Signal<unknown>>();
  readonly #reports = new Map<string, { watchers: number; off: () => void }>();
  readonly #mixers = new Map<string, MixerModel>();
  readonly #lastSent = signal<SentCommand | undefined>(undefined);
  #confirmed: Workspace | undefined;
  #saveTimer: unknown;
  #nextNotice = 1;

  /** True only while the connection is open; every control is disabled otherwise. */
  readonly connected: ReadonlySignal<boolean>;
  readonly themeCatalog: ReadonlySignal<ThemeCatalog>;
  readonly theme: ReadonlySignal<ResolvedTheme>;
  readonly #mixerWidth: Signal<MixerWidth>;
  readonly #panels: Signal<PanelState>;

  constructor(client: Client, dependencies: StoreDependencies = {}) {
    this.#client = client;
    this.#timers = dependencies.timers ?? defaultTimers;
    this.#storage = dependencies.storage ?? (typeof localStorage === "undefined" ? undefined : localStorage);
    this.#frames = frameWriter(dependencies.requestFrame);
    this.#themeSources = dependencies.themeSources ?? [];
    this.#status = signal(client.status);
    this.#server = signal(client.server);
    this.#devices = signal(sortDevices(client.devices));
    this.connected = computed(() => this.#status.value === "open");

    let stored: string | null = null;
    try {
      stored = this.#storage?.getItem(THEME_STORAGE_KEY) ?? null;
    } catch {
      // Storage can be unavailable (private windows, blocked site data); use the default.
    }
    this.#themeId = signal(stored ?? BASE_THEME);
    this.#mixerWidth = persisted(this.#storage, MIXER_WIDTH_STORAGE_KEY, { auto: true, px: STRIP_WIDTH_DEFAULT }, parseMixerWidth);
    this.#panels = persisted(this.#storage, PANELS_STORAGE_KEY, { leftCollapsed: false, rightCollapsed: false }, parsePanels);
    this.themeCatalog = computed(() => {
      const { themes, problems } = resolveThemes([...this.#themeSources, ...this.#userThemes.value]);
      return { themes: [...themes.values()], problems: [...problems, ...this.#userThemeProblems.value] };
    });
    this.theme = computed(() => {
      const { themes } = this.themeCatalog.value;
      return themes.find((t) => t.id === this.#themeId.value) ?? (themes.find((t) => t.id === BASE_THEME) as ResolvedTheme);
    });

    const refreshDevices = () => {
      this.#devices.value = sortDevices(client.devices);
    };
    this.#unsubscribe.push(
      client.on("status", (status) =>
        batch(() => {
          this.#status.value = status;
          this.#server.value = client.server;
          refreshDevices();
        }),
      ),
      client.on("device_added", refreshDevices),
      client.on("device_removed", refreshDevices),
      client.on("lagged", (missed) => this.#notify("warning", `This connection fell behind the server; ${missed} updates were skipped.`)),
    );
  }

  get status(): ReadonlySignal<Status> {
    return this.#status;
  }

  get server(): ReadonlySignal<ServerInfo> {
    return this.#server;
  }

  get devices(): ReadonlySignal<readonly DeviceDescriptor[]> {
    return this.#devices;
  }

  get workspace(): ReadonlySignal<Workspace | undefined> {
    return this.#workspace;
  }

  get saving(): ReadonlySignal<boolean> {
    return this.#saving;
  }

  get notices(): ReadonlySignal<readonly Notice[]> {
    return this.#notices;
  }

  get themeId(): ReadonlySignal<string> {
    return this.#themeId;
  }

  /** How mixer strips are sized; remembered per browser. */
  get mixerWidth(): ReadonlySignal<MixerWidth> {
    return this.#mixerWidth;
  }

  setMixerWidth(change: Partial<MixerWidth>): void {
    const current = this.#mixerWidth.peek();
    const next = { auto: change.auto ?? current.auto, px: clampStripWidth(change.px ?? current.px) };
    if (next.auto !== current.auto || next.px !== current.px) this.#mixerWidth.value = next;
  }

  /** Which side panels are collapsed; remembered per browser. */
  get panels(): ReadonlySignal<PanelState> {
    return this.#panels;
  }

  togglePanel(side: "left" | "right"): void {
    const current = this.#panels.peek();
    this.#panels.value = side === "left" ? { ...current, leftCollapsed: !current.leftCollapsed } : { ...current, rightCollapsed: !current.rightCollapsed };
  }

  /** The last mixer command sent and the bytes the server reported (shown in dry run). */
  get lastSent(): ReadonlySignal<SentCommand | undefined> {
    return this.#lastSent;
  }

  /** A device's topology, or undefined for a device of unknown model. */
  topology(deviceId: string): Topology | undefined {
    const family = this.#devices.peek().find((d) => d.id === deviceId)?.family;
    return family === undefined || family === null ? undefined : topologies[family];
  }

  /** One of a device's mixers. Throws for a device of unknown model or a mixer it does not have. */
  mixer(deviceId: string, index: number): MixerModel {
    const key = `${deviceId}|${index}`;
    const existing = this.#mixers.get(key);
    if (existing !== undefined) return existing;
    const family = this.#devices.peek().find((d) => d.id === deviceId)?.family;
    if (family === undefined || family === null) throw new Error(`${deviceId} has no known model, so no mixers`);
    const topology: Topology = topologies[family];
    if (!Number.isInteger(index) || index < 0 || index >= topology.mixers.count) throw new RangeError(`${deviceId} has mixers 0..${topology.mixers.count - 1}, not ${index}`);
    const model = new MixerModel({
      deviceId,
      family,
      index,
      topology,
      invoke: (command, args, options) => this.#invokeCommand(deviceId, command, args, options),
      read: (command, ext3) => this.#readCommand(deviceId, command, ext3),
      field: (name) => this.field(deviceId, "0x73", name),
      watch: () => this.watchReport(deviceId, "0x73"),
      // A linked channel's strip in this mix, on each member's device that has this mix.
      peers: (strip) =>
        this.links.peers("mixer", deviceId, strip).flatMap((peer) => {
          const peerFamily = this.#devices.peek().find((d) => d.id === peer.deviceId)?.family;
          if (peerFamily === undefined || peerFamily === null || index >= topologies[peerFamily].mixers.count || peer.channel >= topologies[peerFamily].mixers.channels) return [];
          return [{ model: this.mixer(peer.deviceId, index), strip: peer.channel, mode: peer.mode }];
        }),
      monoPans: () => this.#workspace.value?.mixers?.[deviceId]?.mixes?.[index]?.mono?.pans,
      rememberPan: (strip, pan) => {
        this.editWorkspace((workspace) => {
          const mono = workspace.mixers?.[deviceId]?.mixes?.[index]?.mono;
          if (mono !== undefined) mono.pans[String(strip)] = pan;
          return workspace;
        });
      },
    });
    this.#mixers.set(key, model);
    return model;
  }

  readonly #channels = new Map<string, ChannelsModel>();

  /** A device's user-built mixer channels, created on first use; throws for a device of unknown model. */
  channels(deviceId: string): ChannelsModel {
    const existing = this.#channels.get(deviceId);
    if (existing !== undefined) return existing;
    const family = this.#devices.peek().find((d) => d.id === deviceId)?.family;
    if (family === undefined || family === null) throw new Error(`${deviceId} has no known model, so no mixer channels`);
    const model = new ChannelsModel({
      deviceId,
      family,
      topology: topologies[family],
      layout: computed(() => this.#workspace.value?.mixers?.[deviceId]),
      edit: (update) => this.editWorkspace((workspace) => ({ ...workspace, mixers: { ...workspace.mixers, [deviceId]: update(workspace.mixers?.[deviceId] ?? emptyLayout()) } })),
      routing: this.routing(deviceId),
      notify: (text) => this.#notify("error", text),
      saved: computed(() => this.#workspace.value?.layouts ?? []),
      editSaved: (update) => this.editWorkspace((workspace) => ({ ...workspace, layouts: update([...(workspace.layouts ?? [])]) })),
      mixer: (mix) => this.mixer(deviceId, mix),
    });
    this.#channels.set(deviceId, model);
    return model;
  }

  /** Workspace channel links (decision P51). */
  readonly links: LinksModel = new LinksModel({
    links: computed(() => this.#workspace.value?.links ?? []),
    edit: (update) => this.editWorkspace((workspace) => ({ ...workspace, links: update([...workspace.links]) })),
    inputs: (deviceId) => this.#knownInputs(deviceId),
    mixers: (deviceId) => {
      const family = this.#devices.peek().find((d) => d.id === deviceId)?.family;
      return family === undefined || family === null ? undefined : Array.from({ length: topologies[family].mixers.count }, (_, m) => this.mixer(deviceId, m));
    },
  });

  #knownInputs(deviceId: string): InputsModel | undefined {
    const family = this.#devices.peek().find((d) => d.id === deviceId)?.family;
    return family === undefined || family === null ? undefined : this.inputs(deviceId);
  }

  /** Shows an error notice, for a page that refused a change and must say why. */
  reportError(text: string): void {
    this.#notify("error", text);
  }

  readonly #inputs = new Map<string, InputsModel>();

  /** A device's hardware inputs, created on first use; throws for a device of unknown model. */
  inputs(deviceId: string): InputsModel {
    const existing = this.#inputs.get(deviceId);
    if (existing !== undefined) return existing;
    const family = this.#devices.peek().find((d) => d.id === deviceId)?.family;
    if (family === undefined || family === null) throw new Error(`${deviceId} has no known model, so no known inputs`);
    const model = new InputsModel({
      deviceId,
      family,
      topology: topologies[family],
      invoke: (command, args, options) => this.#invokeCommand(deviceId, command, args, options),
      read: (command, ext3) => this.#readCommand(deviceId, command, ext3),
      peers: (kind, index) =>
        this.links.peers(kind, deviceId, index).flatMap((peer) => {
          const model = this.#knownInputs(peer.deviceId);
          return model === undefined ? [] : [{ model, index: peer.channel, mode: peer.mode }];
        }),
      field: (name) => this.field(deviceId, "0x73", name),
      watch: () => this.watchReport(deviceId, "0x73"),
      timers: this.#timers,
    });
    this.#inputs.set(deviceId, model);
    return model;
  }

  readonly #outputs = new Map<string, OutputsModel>();

  /** A device's hardware output levels, created on first use; throws for a device of unknown model. */
  outputs(deviceId: string): OutputsModel {
    const existing = this.#outputs.get(deviceId);
    if (existing !== undefined) return existing;
    const family = this.#devices.peek().find((d) => d.id === deviceId)?.family;
    if (family === undefined || family === null) throw new Error(`${deviceId} has no known model, so no known outputs`);
    const model = new OutputsModel({
      deviceId,
      family,
      invoke: (command, args, options) => this.#invokeCommand(deviceId, command, args, options),
      field: (name) => this.field(deviceId, "0x73", name),
      watch: () => this.watchReport(deviceId, "0x73"),
      timers: this.#timers,
    });
    this.#outputs.set(deviceId, model);
    return model;
  }

  readonly #routings = new Map<string, RoutingModel>();

  /** A device's routing model, created on first use; throws for a device of unknown model. */
  routing(deviceId: string): RoutingModel {
    const existing = this.#routings.get(deviceId);
    if (existing !== undefined) return existing;
    const topology = this.topology(deviceId);
    if (topology === undefined) throw new Error(`${deviceId} has no known model, so no routing`);
    const model = new RoutingModel({
      deviceId,
      topology,
      read: (destination) => this.#readRouting(deviceId, destination),
      // Not coalesced: the model already writes one group at a time, and a superseded write would read as a failure and roll back.
      write: (destination, slots) => this.#invokeCommand(deviceId, "set_routing", { bank_idx: destination, bank_configs: slots.map((s) => new Uint8Array([s.source, s.channel])) }, {}),
      notify: (text) => this.#notify("error", text),
    });
    this.#routings.set(deviceId, model);
    return model;
  }

  /** `get_routing` for one destination group, named in the header's ext3. */
  async #readRouting(deviceId: string, destination: number): Promise<RoutingRead> {
    const device = this.#client.device(deviceId);
    if (device.family === null) throw new Error(`${deviceId} has no known model`);
    const invoke = device.invoke as unknown as (name: string, args: undefined, options: { ext3: number }) => Promise<{ dry_run: boolean; response: { bank_configs?: { in_periph_id: number; in_chann: number }[] } | null; response_error: string | null }>;
    const result = await invoke("get_routing", undefined, { ext3: destination });
    if (result.response_error !== null) throw new Error(result.response_error);
    const slots = result.response?.bank_configs?.map((s) => ({ source: s.in_periph_id, channel: s.in_chann }));
    return { slots, dryRun: result.dry_run };
  }

  /**
   * Reads a command's reply (`ext3` for selectors). A failure becomes a notice and reads as no
   * response, except under `quiet`, which is for reads a page makes on its own: there a refusal
   * leaves the value unknown and is not the user's problem.
   */
  async #readCommand(deviceId: string, command: string, ext3: number | undefined, quiet = false): Promise<{ response: Record<string, unknown> | null; dryRun: boolean }> {
    try {
      const device = this.#client.device(deviceId);
      if (device.family === null) throw new Error(`${deviceId} has no known model`);
      const invoke = device.invoke as unknown as (name: string, args: undefined, options: { ext3?: number }) => Promise<{ dry_run: boolean; response: Record<string, unknown> | null; response_error: string | null }>;
      const result = await invoke(command, undefined, ext3 === undefined ? {} : { ext3 });
      if (result.response_error !== null) throw new Error(result.response_error);
      return { response: result.response, dryRun: result.dry_run };
    } catch (error) {
      if (!quiet) this.#notify("error", `${command} could not be read: ${message(error)}`);
      return { response: null, dryRun: false };
    }
  }

  /** Sends a command; failures other than being superseded become notices. Resolves true when it was sent. */
  async #invokeCommand(deviceId: string, command: string, args: Record<string, unknown>, options: { coalesce?: string }): Promise<boolean> {
    try {
      const device = this.#client.device(deviceId);
      if (device.family === null) throw new Error(`${deviceId} has no known model`);
      const invoke = device.invoke as unknown as (name: string, args: Record<string, unknown>, options: { coalesce?: string }) => Promise<{ sent_hex: string; dry_run: boolean }>;
      const result = await invoke(command, args, options);
      this.#lastSent.value = { deviceId, command, hex: result.sent_hex, dryRun: result.dry_run };
      return true;
    } catch (error) {
      if (error instanceof GazelleError && error.code === "superseded") return false;
      this.#notify("error", `${command} failed: ${message(error)}`);
      return false;
    }
  }

  async start(): Promise<void> {
    await Promise.all([this.loadWorkspace(), this.loadUserThemes()]);
  }

  async loadWorkspace(): Promise<void> {
    try {
      const workspace = await this.#client.workspace.get();
      this.#confirmed = workspace;
      this.#workspace.value = workspace;
    } catch (error) {
      this.#notify("error", `Could not load the workspace: ${message(error)}`);
    }
  }

  async loadUserThemes(): Promise<void> {
    try {
      const listed = await this.#client.themes();
      batch(() => {
        this.#userThemes.value = listed.filter((entry) => entry.theme !== undefined).map((entry) => ({ id: `user:${entry.file.replace(/\.json$/, "")}`, origin: "user" as const, data: entry.theme }));
        this.#userThemeProblems.value = listed
          .filter((entry) => entry.theme === undefined)
          .map((entry) => ({ id: `user:${entry.file.replace(/\.json$/, "")}`, origin: "user" as const, errors: [`could not be read: ${entry.error ?? "unknown error"}`] }));
      });
    } catch (error) {
      this.#notify("warning", `Could not load user themes: ${message(error)}`);
    }
  }

  selectTheme(id: string): void {
    this.#themeId.value = id;
    try {
      this.#storage?.setItem(THEME_STORAGE_KEY, id);
    } catch {
      // Without storage the pick lasts for this page only.
    }
  }

  /** Applies an edit at once and schedules a save. Returns false when there is nothing to edit or no connection. */
  editWorkspace(update: (workspace: Workspace) => Workspace): boolean {
    const current = this.#workspace.peek();
    if (current === undefined || !this.connected.peek()) return false;
    this.#workspace.value = update(structuredClone(current));
    this.#timers.clearTimeout(this.#saveTimer);
    this.#saveTimer = this.#timers.setTimeout(() => void this.#save(), SAVE_DEBOUNCE_MS);
    return true;
  }

  /**
   * Powers a device on or puts it in standby (`set_power`, which both models have and report back
   * as `power_on`). Returns false for a device whose model is unknown, so nothing is sent blind.
   * Each call is its own command: powering off is not something to coalesce with an earlier change.
   */
  setPower(deviceId: string, on: boolean): boolean {
    const family = this.#devices.peek().find((d) => d.id === deviceId)?.family;
    if (family === undefined || family === null) return false;
    void this.#invokeCommand(deviceId, "set_power", { power: on ? 1 : 0 }, {});
    return true;
  }

  /**
   * Sets the device's front-panel brightness, 0..100 as both panels' sliders use (`set_brightness`,
   * reported back as `brightness`). Returns false for a device whose model is unknown.
   */
  setBrightness(deviceId: string, value: number): boolean {
    const family = this.#devices.peek().find((d) => d.id === deviceId)?.family;
    if (family === undefined || family === null) return false;
    const brightness = Math.min(BRIGHTNESS_MAX, Math.max(0, Math.round(value)));
    void this.#invokeCommand(deviceId, "set_brightness", { brightness }, { coalesce: `brightness:${deviceId}` });
    return true;
  }

  /** The clock choices a device's model offers, or undefined when the model is unknown. */
  clock(deviceId: string): { rates: readonly string[]; sources: readonly string[] } | undefined {
    const family = this.#devices.peek().find((d) => d.id === deviceId)?.family;
    if (family === undefined || family === null) return undefined;
    return { rates: SAMPLE_RATES, sources: CLOCK_SOURCES[family] };
  }

  /**
   * What the device reports about its clock. Reading it is reactive, so a watch that calls it
   * re-runs as the report changes. The frequency is three bytes, high first; the lock bit is
   * `locked` on the Quadro and `locked_wc` on the Studio+.
   */
  clockState(deviceId: string): ClockState | undefined {
    const family = this.#devices.peek().find((d) => d.id === deviceId)?.family;
    if (family === undefined || family === null) return undefined;
    const byte = (name: string) => Number(this.field(deviceId, "0x73", name).value ?? 0);
    return {
      source: byte("sync_source"),
      hz: (byte("sync_freq_hi") << 16) | (byte("sync_freq_mid") << 8) | byte("sync_freq_low"),
      locked: byte(family === "quadro" ? "locked" : "locked_wc") === 1,
      rate: Math.min(SAMPLE_RATES.length - 1, Math.max(0, byte("base_index"))),
    };
  }

  /** Sets the sample rate by index into [`SAMPLE_RATES`]. */
  setSampleRate(deviceId: string, index: number): boolean {
    const clock = this.clock(deviceId);
    if (clock === undefined) return false;
    if (!Number.isInteger(index) || index < 0 || index >= clock.rates.length) throw new RangeError(`no sample rate ${index}: this model has 0..${clock.rates.length - 1}`);
    void this.#invokeCommand(deviceId, "set_samp_rate", { srate_idx: index }, { coalesce: `samp_rate:${deviceId}` });
    return true;
  }

  /** Sets the clock source by index into the model's [`CLOCK_SOURCES`]. */
  setClockSource(deviceId: string, index: number): boolean {
    const clock = this.clock(deviceId);
    if (clock === undefined) return false;
    if (!Number.isInteger(index) || index < 0 || index >= clock.sources.length) throw new RangeError(`no clock source ${index}: this model has 0..${clock.sources.length - 1}`);
    void this.#invokeCommand(deviceId, "set_sync_source", { src_index: index }, { coalesce: `sync_source:${deviceId}` });
    return true;
  }

  /**
   * The device's test oscillator, from the status report. Undefined for a model that is unknown;
   * both families have one. A change of its own is held over the device's reports for
   * `ECHO_HOLD_MS`, because all five fields share one byte: a stale report read back between two
   * quick changes would undo the first.
   */
  oscillator(deviceId: string): OscillatorState | undefined {
    const family = this.#devices.peek().find((d) => d.id === deviceId)?.family;
    if (family === undefined || family === null) return undefined;
    const held = this.#osc(deviceId).value.value;
    if (held !== undefined) return held;
    // A tone the device has not reported shows as off. Nothing is known either way until a report
    // arrives, and showing a tone as running is the misleading half of that.
    const byte = (name: string, unknown: number) => {
      const value = this.field(deviceId, "0x73", name).value;
      return value === undefined ? unknown : Number(value);
    };
    const choice = (name: string, choices: readonly string[]) => Math.min(choices.length - 1, Math.max(0, byte(name, 0)));
    return {
      left: choice("freq_left", OSCILLATOR_FREQUENCIES),
      right: choice("freq_right", OSCILLATOR_FREQUENCIES),
      level: choice("level", OSCILLATOR_LEVELS),
      onLeft: byte("mute_left", 1) === 0,
      onRight: byte("mute_right", 1) === 0,
    };
  }

  /** Changes part of the oscillator. All five fields go together, since they share one byte. */
  setOscillator(deviceId: string, change: Partial<OscillatorState>): boolean {
    const current = this.oscillator(deviceId);
    if (current === undefined) return false;
    const next = { ...current, ...change };
    const check = (value: number, choices: readonly string[], what: string) => {
      if (!Number.isInteger(value) || value < 0 || value >= choices.length) throw new RangeError(`no oscillator ${what} ${value}: 0..${choices.length - 1}`);
    };
    check(next.left, OSCILLATOR_FREQUENCIES, "frequency");
    check(next.right, OSCILLATOR_FREQUENCIES, "frequency");
    check(next.level, OSCILLATOR_LEVELS, "level");

    const held = this.#osc(deviceId);
    held.value.value = next;
    this.#timers.clearTimeout(held.timer);
    held.timer = this.#timers.setTimeout(() => {
      // Without any report the local state is all there is, so it stays, as an output's does.
      if (this.field(deviceId, "0x73", "mute_left").peek() !== undefined) held.value.value = undefined;
    }, ECHO_HOLD_MS);

    const args = { freq_left: next.left, freq_right: next.right, level: next.level, mute_left: next.onLeft ? 0 : 1, mute_right: next.onRight ? 0 : 1 };
    void this.#invokeCommand(deviceId, "set_sine_gen", args, { coalesce: `sine_gen:${deviceId}` });
    return true;
  }

  /**
   * One entry per device, made on first use and kept: a change must reach whatever is already
   * watching the oscillator, which a signal created only on the first change never would.
   */
  #osc(deviceId: string): { value: Signal<OscillatorState | undefined>; timer: unknown } {
    let entry = this.#oscillators.get(deviceId);
    if (entry === undefined) {
      entry = { value: signal<OscillatorState | undefined>(undefined), timer: undefined };
      this.#oscillators.set(deviceId, entry);
    }
    return entry;
  }

  readonly #oscillators = new Map<string, { value: Signal<OscillatorState | undefined>; timer: unknown }>();

  /** Whether the device can pass DC: the Quadro can, and the Studio+ has no such command. */
  hasDcCoupling(deviceId: string): boolean {
    return this.#devices.peek().find((d) => d.id === deviceId)?.family === "quadro";
  }

  /**
   * Whether each side passes DC, from the status report (`dc_coupled_in`, `dc_coupled_out`).
   * Reading it is reactive. Undefined for a model without it.
   */
  dcCoupling(deviceId: string): { inputs: boolean; outputs: boolean } | undefined {
    if (!this.hasDcCoupling(deviceId)) return undefined;
    const bit = (name: string) => Number(this.field(deviceId, "0x73", name).value ?? 0) === 1;
    return { inputs: bit("dc_coupled_in"), outputs: bit("dc_coupled_out") };
  }

  /** Lets one side pass DC, or blocks it. The side is `dc_coupled_io`: inputs 0, outputs 1. */
  setDcCoupled(deviceId: string, side: "inputs" | "outputs", on: boolean): boolean {
    if (!this.hasDcCoupling(deviceId)) return false;
    const io = side === "inputs" ? 0 : 1;
    void this.#invokeCommand(deviceId, "set_dc_coupled", { dc_coupled: on ? 1 : 0, dc_coupled_io: io }, { coalesce: `dc_coupled:${side}:${deviceId}` });
    return true;
  }

  /** The panning laws a device offers, or undefined for a model that has none (the Studio+). */
  panningLaws(deviceId: string): readonly string[] | undefined {
    const family = this.#devices.peek().find((d) => d.id === deviceId)?.family;
    return family === "quadro" ? PANNING_LAWS : undefined;
  }

  /**
   * The device's panning law as an index into [`PANNING_LAWS`], as last read or set. Unlike the
   * clock and brightness it is not in the status report, so it is only known after `loadPanningLaw`.
   */
  panningLaw(deviceId: string): ReadonlySignal<number> {
    let value = this.#panningLaws.get(deviceId);
    if (value === undefined) {
      value = signal(0);
      this.#panningLaws.set(deviceId, value);
    }
    return value;
  }

  /** Reads the device's panning law (`get_panning_law`). Resolves false when nothing was read. */
  async loadPanningLaw(deviceId: string): Promise<boolean> {
    if (this.panningLaws(deviceId) === undefined) return false;
    const panning = (await this.#readCommand(deviceId, "get_panning_law", undefined, true)).response?.["panning"];
    if (panning === undefined || panning === null) return false;
    (this.panningLaw(deviceId) as Signal<number>).value = Math.min(PANNING_LAWS.length - 1, Math.max(0, Number(panning)));
    return true;
  }

  /** Sets the panning law by index into [`PANNING_LAWS`]. */
  setPanningLaw(deviceId: string, index: number): boolean {
    const laws = this.panningLaws(deviceId);
    if (laws === undefined) return false;
    if (!Number.isInteger(index) || index < 0 || index >= laws.length) throw new RangeError(`no panning law ${index}: this model has 0..${laws.length - 1}`);
    (this.panningLaw(deviceId) as Signal<number>).value = index;
    void this.#invokeCommand(deviceId, "set_panning_law", { panning: index }, { coalesce: `panning_law:${deviceId}` });
    return true;
  }

  readonly #panningLaws = new Map<string, Signal<number>>();

  /** Recalls one of the device's own presets, 1..[`PRESET_SLOTS`]. */
  recallPreset(deviceId: string, slot: number): boolean {
    return this.#preset(deviceId, slot, "preset_recall");
  }

  /** Saves the device's current state into one of its presets, overwriting what is there. */
  savePreset(deviceId: string, slot: number): boolean {
    return this.#preset(deviceId, slot, "preset_save");
  }

  #preset(deviceId: string, slot: number, command: "preset_recall" | "preset_save"): boolean {
    const family = this.#devices.peek().find((d) => d.id === deviceId)?.family;
    if (family === undefined || family === null) return false;
    if (!Number.isInteger(slot) || slot < 1 || slot > PRESET_SLOTS) throw new RangeError(`no preset ${slot}: the device has 1..${PRESET_SLOTS}`);
    void this.#invokeCommand(deviceId, command, { preset_idx: slot }, {});
    return true;
  }

  /** Sets a device's alias; a blank name removes it. */
  renameDevice(deviceId: string, name: string): boolean {
    const alias = name.trim();
    return this.editWorkspace((workspace) => {
      const aliases = { ...workspace.aliases };
      if (alias === "") delete aliases[deviceId];
      else aliases[deviceId] = alias;
      return { ...workspace, aliases };
    });
  }

  setGroupColor(groupId: string, color: string | undefined): boolean {
    return this.editWorkspace((workspace) => ({
      ...workspace,
      groups: mapGroups(workspace.groups, groupId, (group) => {
        const { color: _previous, ...rest } = group;
        return color === undefined ? rest : { ...rest, color };
      }),
    }));
  }

  toggleGroup(groupId: string): boolean {
    return this.editWorkspace((workspace) => ({ ...workspace, groups: mapGroups(workspace.groups, groupId, (group) => ({ ...group, collapsed: !group.collapsed })) }));
  }

  async #save(): Promise<void> {
    this.#saveTimer = undefined;
    const sending = this.#workspace.peek();
    if (sending === undefined) return;
    this.#saving.value = true;
    try {
      const saved = await this.#client.workspace.put(sending);
      this.#confirmed = saved;
      if (this.#workspace.peek() === sending) this.#workspace.value = saved;
    } catch (error) {
      this.#timers.clearTimeout(this.#saveTimer);
      this.#saveTimer = undefined;
      this.#workspace.value = this.#confirmed;
      this.#notify("error", `The workspace change was not saved and has been undone: ${message(error)}`);
    } finally {
      this.#saving.value = false;
    }
  }

  /** The signal for one field of a device's cyclic report; `undefined` until a report arrives. */
  field(deviceId: string, reportId: string, name: string): ReadonlySignal<unknown> {
    const key = `${deviceId}|${reportKey(reportId)}|${name}`;
    let field = this.#fields.get(key);
    if (field === undefined) {
      field = signal<unknown>(undefined, sameValue);
      this.#fields.set(key, field);
    }
    return field;
  }

  /** Follows a device's cyclic report until the returned function is called. Watchers of one report share a subscription. */
  watchReport(deviceId: string, reportId: string): () => void {
    const key = `${deviceId}|${reportKey(reportId)}`;
    const existing = this.#reports.get(key);
    if (existing !== undefined) {
      existing.watchers++;
    } else {
      const device = this.#client.device(deviceId);
      if (device.family === null) throw new Error(`${deviceId} is of an unknown model, so it has no cyclic reports`);
      const onCyclic = device.onCyclic as unknown as (id: string, listener: (fields: Record<string, unknown>) => void) => () => void;
      const off = onCyclic(reportId, (fields) => {
        for (const [name, value] of Object.entries(fields)) this.#frames.write(this.field(deviceId, reportId, name) as Signal<unknown>, value);
      });
      this.#reports.set(key, { watchers: 1, off });
    }
    return once(() => {
      const entry = this.#reports.get(key);
      if (entry === undefined) return;
      entry.watchers--;
      if (entry.watchers === 0) {
        entry.off();
        this.#reports.delete(key);
      }
    });
  }

  dismiss(noticeId: number): void {
    this.#notices.value = this.#notices.peek().filter((notice) => notice.id !== noticeId);
  }

  #notify(level: Notice["level"], text: string): void {
    this.#notices.value = [...this.#notices.peek(), { id: this.#nextNotice++, level, message: text }];
  }

  /** Stops following the client, drops pending saves, and closes the connection. */
  async close(): Promise<void> {
    for (const off of this.#unsubscribe.splice(0)) off();
    for (const report of this.#reports.values()) report.off();
    this.#reports.clear();
    this.#timers.clearTimeout(this.#saveTimer);
    await this.#client.close();
  }
}
