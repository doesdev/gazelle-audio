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

import { connect, type Client, type DeviceDescriptor, type Group, type ServerInfo, type Status, type Workspace } from "gazelle-audio-client";

// Elements may not import the client (spec §6.1), so the store passes on the data types they show.
export type { DeviceDescriptor, Group, ServerInfo, Status, Workspace };

import { frameWriter, type FrameWriter, type RequestFrame } from "../core/frame.ts";
import { batch, computed, signal, type ReadonlySignal, type Signal } from "../core/signal.ts";
import { BASE_THEME, resolveThemes, type ResolvedTheme, type ThemeProblem, type ThemeSource } from "../themes/theme.ts";

export const SAVE_DEBOUNCE_MS = 300;
export const THEME_STORAGE_KEY = "gazelle.theme";

export interface Timers {
  setTimeout(callback: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

export interface KeyValueStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

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
  #confirmed: Workspace | undefined;
  #saveTimer: unknown;
  #nextNotice = 1;

  /** True only while the connection is open; every control is disabled otherwise. */
  readonly connected: ReadonlySignal<boolean>;
  readonly themeCatalog: ReadonlySignal<ThemeCatalog>;
  readonly theme: ReadonlySignal<ResolvedTheme>;

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
