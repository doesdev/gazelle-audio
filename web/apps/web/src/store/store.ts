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

import { connect, GazelleError, topologies, type Client, type DeviceDescriptor, type Group, type ServerInfo, type Status, type Topology, type Workspace } from "gazelle-audio-client";

import { ChannelsModel, emptyLayout } from "./channels.ts";
import { InputsModel } from "./inputs.ts";
import { MixerModel } from "./mixer.ts";
import { RoutingModel, type RoutingRead } from "./routing.ts";
import { clampStripWidth, parseMixerWidth, parsePanels, persisted, STRIP_WIDTH_DEFAULT, type MixerWidth, type PanelState } from "./preferences.ts";

export type { MixerWidth, PanelState };
export const MIXER_WIDTH_STORAGE_KEY = "gazelle.mixer.width";
export const PANELS_STORAGE_KEY = "gazelle.layout.panels";

// Elements may not import the client (spec §6.1), so the store passes on the data types they show.
export type { DeviceDescriptor, Group, ServerInfo, Status, Topology, Workspace };

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
      field: (name) => this.field(deviceId, "0x73", name),
      watch: () => this.watchReport(deviceId, "0x73"),
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
    });
    this.#channels.set(deviceId, model);
    return model;
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
      field: (name) => this.field(deviceId, "0x73", name),
      watch: () => this.watchReport(deviceId, "0x73"),
      timers: this.#timers,
    });
    this.#inputs.set(deviceId, model);
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
