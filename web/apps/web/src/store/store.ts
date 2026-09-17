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
// - Selection: the device last opened on a page and each device's last mix are remembered per
//   browser, so a page whose address names neither lands where the user last was. Other view
//   state (scroll, open sections, a selection in progress) lives in `view`, for the tab only:
//   pages are rebuilt on every change of address and read it back when they are.

import { connect, GazelleError, topologies, type Client, type DeviceDescriptor, type ChannelRef, type DeviceMixer, type Group, type Link, type LinkKind, type MixerChannel, type RouteSource, type ServerInfo, type Status, type Surface, type SurfaceStrip, type Topology, type Workspace, type Cable, type CableEnd, type DigitalPort } from "gazelle-audio-client";

import { ChannelsModel, emptyLayout, sourceLabel } from "./channels.ts";
import { EffectsModel } from "./effects.ts";
import { ECHO_HOLD_MS, InputsModel } from "./inputs.ts";
import { LinksModel } from "./links.ts";
import { CONTROL_ROOM_DEFAULT, OutputsModel } from "./outputs.ts";
import { LEVEL_MAX, MixerModel } from "./mixer.ts";
import { RoutingModel, type RoutingRead } from "./routing.ts";
import { SurfacesModel } from "./surfaces.ts";
import { CablesModel } from "./cables.ts";
import { clampStripWidth, migratePanels, parseMixerWidth, parseSelectedDevice, parseSelectedMixes, parseSidebar, persisted, SIDEBAR_DEFAULT, STRIP_WIDTH_DEFAULT, type MixerWidth, type SidebarSection, type SidebarState } from "./preferences.ts";

export type { MixerWidth, SidebarSection, SidebarState };
export { SIDEBAR_SECTIONS } from "./preferences.ts";
export const MIXER_WIDTH_STORAGE_KEY = "gazelle.mixer.width";
/** The two side panels' collapse, from before the single sidebar; carried over and dropped on load. */
export const PANELS_STORAGE_KEY = "gazelle.layout.panels";
export const SIDEBAR_STORAGE_KEY = "gazelle.layout.sidebar";
export const SELECTED_DEVICE_STORAGE_KEY = "gazelle.selection.device";
export const SELECTED_MIXES_STORAGE_KEY = "gazelle.selection.mixes";
export const CLIP_AUTO_CLEAR_STORAGE_KEY = "gazelle.meters.clipAutoClear";
export const MIXER_DOCK_STORAGE_KEY = "gazelle.layout.mixerDock";
export const MIXER_DOCK_SURFACE_STORAGE_KEY = "gazelle.layout.mixerDockSurface";

// Elements may not import the client (spec §6.1), so the store passes on the data types they show.
export type { Cable, CableEnd, ChannelRef, DeviceDescriptor, DeviceMixer, DigitalPort, Group, Link, LinkKind, MixerChannel, RouteSource, ServerInfo, Status, Surface, SurfaceStrip, Topology, Workspace };
export type { NewStrip } from "./surfaces.ts";

/** The most recent command the mixer sent, with the bytes the server reported. */
export interface SentCommand {
  deviceId: string;
  command: string;
  hex: string;
  dryRun: boolean;
}

import { frameWriter, type FrameWriter, type RequestFrame } from "../core/frame.ts";
import { batch, computed, effect, signal, untracked, type ReadonlySignal, type Signal } from "../core/signal.ts";
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

/**
 * How long a device card keeps showing a clip. A clip is a single report long, and reports arrive
 * many times a second, so without a hold it would flash too briefly to notice.
 */
export const CLIP_HOLD_MS = 1500;

/** The meter byte at or below which an input counts as carrying signal: -60 dBFS. */
const SIGNAL_BELOW_FS = 60;

/**
 * The hardware inputs a device card watches for signal and clips, per model. Both report input
 * peaks in fixed fields; the Studio+'s `peaks_meters` follow a selectable source, so they are not.
 */
const INPUT_PEAKS: Readonly<Record<"quadro" | "studio", readonly string[]>> = {
  quadro: ["peaks_preamp", "peaks_spdif", "peaks_adat"],
  studio: ["peaks_preamp", "peaks_line", "peaks_spdif", "peaks_adat"],
};

/**
 * The status report field that meters each kind of input, by the topology's input type. The Quadro's
 * mixer meters stay on Mix 1's inputs whatever `set_peak_source` asks (hardware, 2026-09-16), so a
 * channel is metered at its input: the signal arriving, before any fader. An input type missing
 * here reports no meter of its own (the Studio+'s USB and Thunderbolt playback and effect returns
 * only reach its selectable meter bank).
 */
const INPUT_METER_FIELDS: Readonly<Record<"quadro" | "studio", Readonly<Record<string, string>>>> = {
  quadro: { PREAMP: "peaks_preamp", COM_PLAY: "peaks_usb_play", USB_PLAY: "peaks_usb_play2", ADAT_IN: "peaks_adat", SPDIF_IN: "peaks_spdif" },
  studio: { PREAMP: "peaks_preamp", LINE_IN: "peaks_line", ADAT_IN: "peaks_adat", SPDIF_IN: "peaks_spdif" },
};

/**
 * How long a clip light stays lit after the signal stops clipping, in ms, or null to hold it until
 * it is cleared. The countdown starts when the clip ends, not when it starts: a device repeats the
 * same report for as long as it clips, and a light should not go out while it does.
 */
export const CLIP_AUTO_CLEAR_CHOICES: readonly (number | null)[] = [2000, 5000, 10000, 30000, null];
const CLIP_AUTO_CLEAR_DEFAULT = 5000;

function parseClipAutoClear(stored: unknown): number | null | undefined {
  return CLIP_AUTO_CLEAR_CHOICES.includes(stored as number | null) ? (stored as number | null) : undefined;
}

/** One input's meter: its latest peak byte (dB below full scale) and a clip light that latches. */
export interface InputMeter {
  level: ReadonlySignal<number | undefined>;
  clipped: ReadonlySignal<boolean>;
  clearClip(): void;
  /**
   * What the meter shows, for a strip's tooltip. A meter that reads nothing says why (P85): an
   * effect chain may be empty or simply not read yet, and either way the device reports nothing
   * for it.
   */
  note: ReadonlySignal<string>;
}

/** An effect's meter: an input meter's peak and clip, plus the gain reduction the device reports. */
export interface EffectMeter extends InputMeter {
  /** The gain reduction byte, `undefined` until a report arrives. Its unit is the effect's own. */
  reduction: ReadonlySignal<number | undefined>;
}

/** The report both models push their effect meters in, many times a second. */
const EFFECT_METER_REPORT = "0x83";

/**
 * The topology input type a mixer strip is fed from when it carries an effect chain's output, and
 * the destination type routing feeds that chain through. The channel is the chain index in both
 * models, so AFX OUT k is what chain k plays and AFX IN k is what it processes.
 */
const AFX_OUT = "AFX_OUT";
const AFX_IN = "AFX_IN";

/** What a strip's meter shows, by default: the input's level before the fader. */
const INPUT_METER_NOTE = "The input's level, before the fader";

/** One stereo output's meter, from the fixed output fields only the Quadro reports. */
export interface OutputMeter {
  name: string;
  left: ReadonlySignal<number | undefined>;
  right: ReadonlySignal<number | undefined>;
  /** Lit when either side clips; as an input's. */
  clipped: ReadonlySignal<boolean>;
  clearClip(): void;
}

interface ClipLight {
  lit: Signal<boolean>;
  clipping: ReadonlySignal<boolean>;
  timer: unknown;
}

const QUADRO_OUTPUT_METERS: readonly (readonly [name: string, field: string])[] = [
  ["Monitor", "peaks_monitor"],
  ["HP1", "peaks_hp1"],
  ["HP2", "peaks_hp2"],
  ["Line out", "line_out"],
];

/** A device at a glance, for its card in the devices panel. */
export interface DeviceCard {
  /** Whether the device's status report has arrived yet. */
  reporting: boolean;
  power: boolean | undefined;
  preset: number | undefined;
  clock: ClockState | undefined;
  /** The loudest of its hardware inputs: silent, carrying signal, or clipped (held a moment). */
  input: "quiet" | "signal" | "clip";
}

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
  /** Whether the window was phone width as the app opened: the mixer dock then starts collapsed unless a choice is kept. */
  narrow?: boolean;
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
  readonly #sidebar: Signal<SidebarState>;
  readonly #selectedDevice: Signal<string | undefined>;
  readonly #selectedMixes: Signal<Readonly<Record<string, number>>>;
  readonly #clipAutoClear: Signal<number | null>;
  readonly #mixerDockCollapsed: Signal<boolean>;
  readonly #mixerDockSurface: Signal<string | null>;
  /** The surface the mixer dock shows, while it exists; undefined for the device in view. */
  readonly mixerDockSurface: ReadonlySignal<string | undefined>;
  readonly #clipLights = new Set<ClipLight>();
  readonly #selectedMix = new Map<string, ReadonlySignal<number>>();
  readonly #views = new Map<string, Signal<unknown>>();

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
    const carried = migratePanels(this.#storage, PANELS_STORAGE_KEY, SIDEBAR_STORAGE_KEY);
    this.#sidebar = persisted(this.#storage, SIDEBAR_STORAGE_KEY, carried ?? SIDEBAR_DEFAULT, parseSidebar);
    this.#selectedDevice = persisted<string | undefined>(this.#storage, SELECTED_DEVICE_STORAGE_KEY, undefined, parseSelectedDevice);
    this.#selectedMixes = persisted<Readonly<Record<string, number>>>(this.#storage, SELECTED_MIXES_STORAGE_KEY, {}, parseSelectedMixes);
    this.#clipAutoClear = persisted<number | null>(this.#storage, CLIP_AUTO_CLEAR_STORAGE_KEY, CLIP_AUTO_CLEAR_DEFAULT, parseClipAutoClear);
    this.#mixerDockCollapsed = persisted(this.#storage, MIXER_DOCK_STORAGE_KEY, dependencies.narrow ?? false, (stored) => (typeof stored === "boolean" ? stored : undefined));
    this.#mixerDockSurface = persisted<string | null>(this.#storage, MIXER_DOCK_SURFACE_STORAGE_KEY, null, (stored) => (typeof stored === "string" || stored === null ? stored : undefined));
    // A surface deleted here or elsewhere hands the dock back to the device in view.
    this.mixerDockSurface = computed(() => {
      const id = this.#mixerDockSurface.value;
      return id !== null && (this.#workspace.value?.surfaces ?? []).some((s) => s.id === id) ? id : undefined;
    });
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
          // Mixes and routing are read once and kept (P80). While the connection is down the server
          // may restart or the device change, so they are read again once it is back.
          if (status !== "open") {
            for (const mixer of this.#mixers.values()) mixer.forget();
            for (const routing of this.#routings.values()) routing.forget();
            for (const effects of this.#effects.values()) effects.forget();
          }
        }),
      ),
      client.on("device_added", refreshDevices),
      client.on("device_removed", (deviceId) => {
        refreshDevices();
        // Unplugged, or about to be re-attached: what comes back may not be as it was.
        for (const mixer of this.#mixers.values()) if (mixer.deviceId === deviceId) mixer.forget();
        this.#routings.get(deviceId)?.forget();
        this.#effects.get(deviceId)?.forget();
      }),
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

  /** Which side the sidebar docks to, whether it is collapsed, and its sections' collapse; remembered per browser. */
  get sidebar(): ReadonlySignal<SidebarState> {
    return this.#sidebar;
  }

  toggleSidebar(): void {
    const current = this.#sidebar.peek();
    this.#sidebar.value = { ...current, collapsed: !current.collapsed };
  }

  /** Moves the sidebar to the other side of the page. */
  moveSidebar(): void {
    const current = this.#sidebar.peek();
    this.#sidebar.value = { ...current, side: current.side === "right" ? "left" : "right" };
  }

  setSidebarSection(section: SidebarSection, collapsed: boolean): void {
    const current = this.#sidebar.peek();
    if (current.sections[section] !== collapsed) this.#sidebar.value = { ...current, sections: { ...current.sections, [section]: collapsed } };
  }

  /** The device last opened on a page that names one; remembered per browser. */
  get selectedDevice(): ReadonlySignal<string | undefined> {
    return this.#selectedDevice;
  }

  /**
   * Remembers the device a page is open on. Only a connected device is remembered: an address
   * naming one that is gone (an old bookmark) shows a placeholder, and should not replace the
   * device the user was last working with. Returns whether it was remembered.
   */
  selectDevice(deviceId: string): boolean {
    if (!this.#devices.peek().some((d) => d.id === deviceId)) return false;
    this.#selectedDevice.value = deviceId;
    return true;
  }

  /**
   * The device a page whose address names none shows: the one last selected while it is connected
   * (and of known model, for `known`), else the first such device. Reading it is reactive.
   */
  deviceInView(known: boolean): string | undefined {
    const candidates = this.#devices.value.filter((d) => !known || d.family !== null);
    const selected = this.#selectedDevice.value;
    return (candidates.find((d) => d.id === selected) ?? candidates[0])?.id;
  }

  /** The mix last chosen on a device, within its mixes: 0 until one is, or for an unknown model. */
  selectedMix(deviceId: string): ReadonlySignal<number> {
    let mix = this.#selectedMix.get(deviceId);
    if (mix === undefined) {
      // One computed per device, so choosing a mix on one device does not wake watchers of the
      // other's (the Mixer page re-points the device's meters whenever its mix changes).
      mix = computed(() => {
        const family = this.#devices.value.find((d) => d.id === deviceId)?.family;
        const stored = this.#selectedMixes.value[deviceId] ?? 0;
        return family === undefined || family === null ? 0 : Math.min(topologies[family].mixers.count - 1, stored);
      });
      this.#selectedMix.set(deviceId, mix);
    }
    return mix;
  }

  /** Remembers a device's mix. Returns false for a mix the device does not have. */
  selectMix(deviceId: string, mix: number): boolean {
    const count = this.topology(deviceId)?.mixers.count ?? 0;
    if (!Number.isInteger(mix) || mix < 0 || mix >= count) return false;
    const mixes = this.#selectedMixes.peek();
    if (mixes[deviceId] !== mix) this.#selectedMixes.value = { ...mixes, [deviceId]: mix };
    return true;
  }

  /**
   * View state for the tab, by key: a page's scroll position, an open section, a selection in
   * progress. Pages are rebuilt whenever the address changes and read theirs back from here, so
   * leaving a page and coming back finds it as it was. Not saved: a reload starts afresh.
   */
  view<T>(key: string, initial: T): Signal<T> {
    let state = this.#views.get(key);
    if (state === undefined) {
      state = signal<unknown>(initial);
      this.#views.set(key, state);
    }
    return state as Signal<T>;
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

  /**
   * Whether a device's mixes want reading now (P80): the connection is open, the device is attached,
   * and some mix has not been read since it was last forgotten. Reading it is reactive.
   */
  mixesToRead(deviceId: string): boolean {
    if (!this.connected.value) return false;
    // Read reactively, so a device coming back is noticed.
    const family = this.#devices.value.find((d) => d.id === deviceId)?.family;
    const count = family === undefined || family === null ? 0 : topologies[family].mixers.count;
    return Array.from({ length: count }, (_, mix) => this.mixer(deviceId, mix).needsRead.value).some(Boolean);
  }

  /** Reads each of a device's mixes that wants it; after a read, pairs linked on the device (by its own panel) become links. */
  async readMixes(deviceId: string): Promise<void> {
    const count = this.topology(deviceId)?.mixers.count ?? 0;
    const read = await Promise.all(Array.from({ length: count }, (_, mix) => this.mixer(deviceId, mix).readOnce()));
    if (read.some(Boolean)) await this.links.importDevicePairs(deviceId);
  }

  /**
   * Whether some of a device's destination groups (all of them unless named) want reading now, as
   * `mixesToRead` does for mixes: the connection is open, the device is attached, and a group has
   * not been read since it was last forgotten. Reading it is reactive.
   */
  routesToRead(deviceId: string, destinations?: readonly number[]): boolean {
    if (!this.connected.value) return false;
    // Read reactively, so a device coming back is noticed.
    const family = this.#devices.value.find((d) => d.id === deviceId)?.family;
    if (family === undefined || family === null) return false;
    const routing = this.routing(deviceId);
    return (destinations ?? topologies[family].outputs.map((_, i) => i)).some((destination) => routing.needsRead(destination).value);
  }

  /** Reads each of a device's destination groups (all of them unless named) that wants it. */
  async readRoutes(deviceId: string, destinations?: readonly number[]): Promise<void> {
    const topology = this.topology(deviceId);
    if (topology === undefined) return;
    const routing = this.routing(deviceId);
    await Promise.all((destinations ?? topology.outputs.map((_, i) => i)).map((destination) => routing.readOnce(destination)));
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
      meteredMix: {
        get: () => this.selectedMix(deviceId),
        set: (mix) => this.selectMix(deviceId, mix),
      },
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

  /** Cross-device mix surfaces and each device's badge colour (workspace spec §4). */
  readonly surfaces: SurfacesModel = new SurfacesModel({
    surfaces: computed(() => this.#workspace.value?.surfaces ?? []),
    edit: (update) => this.editWorkspace((workspace) => ({ ...workspace, surfaces: update([...(workspace.surfaces ?? [])]) })),
    colors: computed(() => this.#workspace.value?.device_colors ?? {}),
    editColors: (update) => this.editWorkspace((workspace) => ({ ...workspace, device_colors: update({ ...(workspace.device_colors ?? {}) }) })),
    deviceIds: computed(() => this.#devices.value.map((d) => d.id)),
    palette: computed(() => this.theme.value.palette),
    model: (deviceId) => {
      const family = this.#devices.peek().find((d) => d.id === deviceId)?.family;
      return family === undefined || family === null ? undefined : { family, topology: topologies[family], outputs: family === "quadro" ? 4 : 5 };
    },
  });

  /** Digital cables between devices: where a digital input's signal comes from, and what is wrong along one (workspace spec §4.5). */
  readonly cables: CablesModel = new CablesModel({
    cables: computed(() => this.#workspace.value?.cables ?? []),
    edit: (update) => this.editWorkspace((workspace) => ({ ...workspace, cables: update([...(workspace.cables ?? [])]) })),
    model: (deviceId) => {
      const family = this.#devices.value.find((d) => d.id === deviceId)?.family;
      return family === undefined || family === null ? undefined : { family, topology: topologies[family] };
    },
    routing: (deviceId) => this.routing(deviceId),
    mixName: (deviceId, mix) => this.channels(deviceId).mixName(mix),
    mixerChannels: (deviceId) => (this.topology(deviceId) === undefined ? [] : this.channels(deviceId).layout.value.channels),
    deviceName: (deviceId) => {
      const device = this.#devices.value.find((d) => d.id === deviceId);
      return device === undefined ? deviceId : displayName(device, this.#workspace.value);
    },
    clock: (deviceId) => {
      const card = this.deviceCard(deviceId);
      return card?.clock === undefined ? undefined : { rate: card.clock.rate, locked: card.clock.locked };
    },
    inputLevel: (deviceId, source) => this.inputMeter(deviceId, source)?.level.value,
    rateNames: SAMPLE_RATES,
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
      read: (command, ext3, quiet) => this.#readCommand(deviceId, command, ext3, quiet),
      peers: (kind, index) =>
        this.links.peers(kind, deviceId, index).flatMap((peer) => {
          const model = this.#knownInputs(peer.deviceId);
          return model === undefined ? [] : [{ model, index: peer.channel, mode: peer.mode }];
        }),
      linkPreamps: (channels, on) => {
        if (on) this.links.create("preamp", channels.map((channel) => ({ device_id: deviceId, channel })), "absolute");
        else for (const channel of channels) this.links.removeMember("preamp", deviceId, channel);
      },
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

  readonly #controlRooms = new Map<string, ReadonlySignal<readonly number[]>>();

  /**
   * The outputs a device's Control Room panel shows, by id, in the device's order: the workspace's
   * choice for the device, else Monitor, HP1 and HP2. Reactive.
   */
  controlRoomOutputs(deviceId: string): ReadonlySignal<readonly number[]> {
    let outputs = this.#controlRooms.get(deviceId);
    if (outputs === undefined) {
      outputs = computed(
        () => [...(this.#workspace.value?.control_room?.[deviceId]?.outputs ?? CONTROL_ROOM_DEFAULT)].sort((a, b) => a - b),
        (a, b) => a.length === b.length && a.every((id, i) => id === b[i]),
      );
      this.#controlRooms.set(deviceId, outputs);
    }
    return outputs;
  }

  /**
   * Shows or hides an output in a device's Control Room panel, saved in the workspace. Throws for an
   * output the model lacks or a device of unknown model; false when there is no connection.
   */
  setInControlRoom(deviceId: string, output: number, on: boolean): boolean {
    const { outputs, family } = this.outputs(deviceId);
    if (!Number.isInteger(output) || output < 0 || output >= outputs.length) throw new RangeError(`the ${family === "studio" ? "Studio+" : "Quadro"} has outputs 0..${outputs.length - 1}, not ${output}`);
    const current = this.controlRoomOutputs(deviceId).peek();
    if (current.includes(output) === on) return this.connected.peek() && this.#workspace.peek() !== undefined;
    const next = on ? [...current, output].sort((a, b) => a - b) : current.filter((id) => id !== output);
    return this.editWorkspace((workspace) => ({ ...workspace, control_room: { ...workspace.control_room, [deviceId]: { outputs: next } } }));
  }

  readonly #effects = new Map<string, EffectsModel>();

  /** A device's effect chains and reverb, created on first use; throws for a device of unknown model. */
  effects(deviceId: string): EffectsModel {
    const existing = this.#effects.get(deviceId);
    if (existing !== undefined) return existing;
    const family = this.#devices.peek().find((d) => d.id === deviceId)?.family;
    if (family === undefined || family === null) throw new Error(`${deviceId} has no known model, so no known effects`);
    const model = new EffectsModel({
      deviceId,
      family,
      topology: topologies[family],
      invoke: (command, args, options) => this.#invokeCommand(deviceId, command, args, options),
      read: (command, ext3, quiet, args) => this.#readCommand(deviceId, command, ext3, quiet, args),
      watchMeters: () => this.watchReport(deviceId, EFFECT_METER_REPORT),
    });
    this.#effects.set(deviceId, model);
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
  async #readCommand(deviceId: string, command: string, ext3: number | undefined, quiet = false, args?: Record<string, unknown>): Promise<{ response: Record<string, unknown> | null; dryRun: boolean }> {
    try {
      const device = this.#client.device(deviceId);
      if (device.family === null) throw new Error(`${deviceId} has no known model`);
      const invoke = device.invoke as unknown as (name: string, args: Record<string, unknown> | undefined, options: { ext3?: number }) => Promise<{ dry_run: boolean; response: Record<string, unknown> | null; response_error: string | null }>;
      const result = await invoke(command, args, ext3 === undefined ? {} : { ext3 });
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

  /**
   * A device at a glance: power, preset, clock and input level from its status report, or undefined
   * for a model whose report cannot be read. Reading it is reactive. The report is only followed
   * while something watches it, as the devices panel does for every device it lists.
   */
  deviceCard(deviceId: string): DeviceCard | undefined {
    const family = this.#devices.peek().find((d) => d.id === deviceId)?.family;
    if (family === undefined || family === null) return undefined;
    const raw = (name: string) => this.field(deviceId, "0x73", name).value;
    const reporting = raw("power_on") !== undefined;
    const clock = reporting ? this.clockState(deviceId) : undefined;
    return {
      reporting,
      power: reporting ? Number(raw("power_on")) === 1 : undefined,
      preset: reporting && raw("current_preset") !== undefined ? Number(raw("current_preset")) : undefined,
      clock,
      input: this.#inputLevel(deviceId, family).value,
    };
  }

  /**
   * Whether any hardware input carries signal or has clipped. A clip latches for CLIP_HOLD_MS after
   * the last report that carried it. Made on first use per device and kept, like the other
   * per-device state here; it reads report fields only, so it costs nothing while nothing reports.
   */
  #inputLevel(deviceId: string, family: "quadro" | "studio"): ReadonlySignal<"quiet" | "signal" | "clip"> {
    const existing = this.#inputLevels.get(deviceId);
    if (existing !== undefined) return existing;
    const level = signal<"quiet" | "signal" | "clip">("quiet");
    const fields = INPUT_PEAKS[family].map((name) => this.field(deviceId, "0x73", name));
    let hold: unknown;
    let held = false;
    // The loudest input in the latest report, so a hold that ends shows what is there now.
    let latest = Number.POSITIVE_INFINITY;
    const shown = () => (held ? "clip" : latest <= SIGNAL_BELOW_FS ? "signal" : "quiet");
    // Made on first read, which is often inside a watch: untracked, so it stands on its own rather
    // than being taken for part of whatever effect happened to be running.
    untracked(() => effect(() => {
      latest = Number.POSITIVE_INFINITY;
      for (const field of fields) {
        const bytes = field.value;
        if (!(bytes instanceof Uint8Array || Array.isArray(bytes))) continue;
        const list = bytes as ArrayLike<number>;
        for (let i = 0; i < list.length; i++) latest = Math.min(latest, Number(list[i]));
      }
      if (latest === 0) {
        held = true;
        this.#timers.clearTimeout(hold);
        hold = this.#timers.setTimeout(() => {
          held = false;
          level.value = shown();
        }, CLIP_HOLD_MS);
      }
      level.value = shown();
    }));
    this.#inputLevels.set(deviceId, level);
    return level;
  }

  readonly #inputLevels = new Map<string, ReadonlySignal<"quiet" | "signal" | "clip">>();

  /**
   * The meter of one input, by its routing source, or undefined when that input reports none. One
   * per input, shared by every strip fed from it. Its report is only followed while something
   * watches it, as the Mixer page does.
   */
  inputMeter(deviceId: string, source: RouteSource): InputMeter | undefined {
    const family = this.#devices.peek().find((d) => d.id === deviceId)?.family;
    if (family === undefined || family === null) return undefined;
    const type = topologies[family].inputs[source.group]?.type;
    // An effect chain's output is metered by the chain, not by a field of the status report: the
    // device meters each effect, and the last one in the chain is as far as it goes.
    if (type === AFX_OUT) return this.#chainMeter(deviceId, family, source.channel);
    const fieldName = type === undefined ? undefined : INPUT_METER_FIELDS[family][type];
    if (fieldName === undefined) return undefined;
    const key = `${deviceId}|${fieldName}|${source.channel}`;
    const existing = this.#inputMeters.get(key);
    if (existing !== undefined) return existing;
    const field = this.field(deviceId, "0x73", fieldName);
    const level = computed(() => {
      const bytes = field.value;
      return (bytes instanceof Uint8Array || Array.isArray(bytes)) && source.channel < bytes.length ? Number((bytes as ArrayLike<number>)[source.channel]) : undefined;
    });
    const light = this.#clipLight(computed(() => level.value === 0));
    const meter: InputMeter = { level, clipped: light.lit, clearClip: () => this.#clearClip(light), note: computed(() => INPUT_METER_NOTE) };
    this.#inputMeters.set(key, meter);
    return meter;
  }

  readonly #inputMeters = new Map<string, InputMeter>();

  /**
   * The meter of one effect in a chain, from the effect-meter report (0x83), or undefined when that
   * position holds no effect.
   *
   * The models report the same two numbers differently (`reference/devices.md`, "Effects (AFX) and
   * reverb"). The Quadro sends two bytes -- peak, then gain reduction -- per **loaded** effect,
   * chain by chain in chain order, so an effect's place in the report depends on how many effects
   * the chains before it hold; that is the one place this mapping lives. The Studio+ sends fixed
   * fields indexed by chain and by the effect's position within it.
   *
   * The peak is dB below full scale, as every other meter is. The gain reduction's unit is the
   * effect's own: the vendor panel's meter widget reads some effects' bytes as dB, others as
   * quarter-dB, so it is shown as the device's own steps rather than a figure this app invents.
   */
  effectMeter(deviceId: string, chain: number, position: number): EffectMeter | undefined {
    const family = this.#devices.peek().find((d) => d.id === deviceId)?.family;
    if (family === undefined || family === null) return undefined;
    const slots = this.effects(deviceId).chains.peek()?.[chain]?.slots;
    if (slots !== undefined && position >= slots.length) return undefined;
    const at = computed(() => (position < (this.effects(deviceId).chains.value?.[chain]?.slots.length ?? 0) ? { chain, position } : undefined));
    const note = computed(() => (at.value === undefined ? "This slot holds no effect" : "The effect's output level"));
    return this.#effectMeterAt(deviceId, family, at, `${deviceId}|effect|${chain}|${position}`, note);
  }

  /**
   * The meter of a mixer strip fed by AFX OUT k: what that strip actually carries.
   *
   * A chain with effects is metered by its **last effect**, the chain's output as far as the device
   * reports it. An **empty chain passes its input straight through** (the user, at the hardware,
   * 2026-09-18: a mix carrying a preamp and an AFX OUT fed from that preamp is about 6 dB louder),
   * so the strip is carrying audio and is metered by whatever routing feeds **AFX IN k**, on the
   * ordinary input path (`INPUT_METER_FIELDS`), as if the strip sat on that input. A source with no
   * meter of its own -- MUTE, another chain's output, a mixer output, or an input the status report
   * does not meter -- shows none, and the title says which it is (P85). Routing is what was read
   * once (P97); nothing is sent to make any of this work.
   */
  #chainMeter(deviceId: string, family: "quadro" | "studio", chain: number): EffectMeter {
    const key = `${deviceId}|chain|${chain}`;
    const existing = this.#chainMeters.get(key);
    if (existing !== undefined) return existing;
    const at = computed(() => {
      const slots = this.effects(deviceId).chains.value?.[chain]?.slots;
      return slots === undefined || slots.length === 0 ? undefined : { chain, position: slots.length - 1 };
    });
    const effect = this.#effectMeterAt(deviceId, family, at, key, computed(() => "The chain's last effect, which is as far as the device meters it"));
    // Only while the chain is empty: with an effect loaded the chain's own meter is the answer.
    const through = computed(() => (at.value === undefined ? this.#chainInput(deviceId, chain) : undefined));
    const source = computed(() => through.value?.meter);
    const meter: EffectMeter = {
      level: computed(() => (at.value !== undefined ? effect.level.value : source.value?.level.value)),
      reduction: computed(() => (at.value !== undefined ? effect.reduction.value : undefined)),
      clipped: computed(() => (at.value !== undefined ? effect.clipped.value : source.value?.clipped.value === true)),
      clearClip: () => (at.peek() !== undefined ? effect.clearClip() : through.peek()?.meter?.clearClip()),
      note: computed(() => (at.value !== undefined ? effect.note.value : (through.value?.note ?? ""))),
    };
    this.#chainMeters.set(key, meter);
    return meter;
  }

  readonly #chainMeters = new Map<string, EffectMeter>();

  /**
   * Why a strip's audio reaches its mix twice, or undefined when it does not (the user, at the
   * hardware, 2026-09-18: "if I add a preamp channel to a mix and an AFX Out channel that is routed
   * to, my signal will be 6 dB higher"). Two channels of one mix carry the same audio when they
   * share a source, or when one is an **empty** AFX OUT chain fed by the other's source -- an empty
   * chain passes its input straight through, so the mix sums the same signal twice, about 6 dB
   * louder and comb-filtered by whatever delay the chain adds.
   *
   * A chain **with** an effect is not counted: dry and wet in one mix is a parallel setup someone
   * may well want. Neither is a channel that adds nothing to the mix, muted or with its fader at
   * the floor. This is only what the app can see from the layout, routing and the chains; nothing
   * is sent to a device, and the device is never asked.
   */
  doubledFeed(deviceId: string, mix: number, slot: number): ReadonlySignal<string | undefined> {
    const key = `${deviceId}|${mix}|${slot}`;
    const existing = this.#doubled.get(key);
    if (existing !== undefined) return existing;
    const warning = computed(() => this.#doubledFeeds(deviceId, mix).get(slot));
    this.#doubled.set(key, warning);
    return warning;
  }

  readonly #doubled = new Map<string, ReadonlySignal<string | undefined>>();

  /** Every doubled strip of one mix, by slot, with what to say about it. Reading it is reactive. */
  #doubledFeeds(deviceId: string, mix: number): ReadonlyMap<number, string> {
    const warnings = new Map<number, string>();
    const topology = this.topology(deviceId);
    if (topology === undefined || mix >= topology.mixers.count) return warnings;
    const channels = this.channels(deviceId);
    const mixer = this.mixer(deviceId, mix);
    // What each strip of the mix actually carries: its own source, or, through an empty chain, the
    // source routed into that chain. A strip that adds nothing to the mix is left out.
    const carried = channels.layout.value.channels.flatMap((channel) => {
      if (!channels.strip(channel, mix).inMix || channel.source === undefined) return [];
      const state = mixer.strip(channel.slot).value;
      if (state.mute || state.level >= LEVEL_MAX) return [];
      // Only an empty chain passes its input on: one with an effect makes a different signal, and
      // dry plus wet in one mix is a parallel setup rather than a doubling.
      const chain = topology.inputs[channel.source.group]?.type === AFX_OUT ? channel.source.channel : undefined;
      const empty = chain !== undefined && this.effects(deviceId).chains.value?.[chain]?.slots.length === 0;
      const through = empty ? this.#chainInput(deviceId, chain).source : undefined;
      const source = through ?? channel.source;
      return [{ slot: channel.slot, source, ...(through === undefined ? {} : { chain }) }];
    });
    const shared = new Map<string, typeof carried>();
    for (const entry of carried) shared.set(`${entry.source.group}:${entry.source.channel}`, [...(shared.get(`${entry.source.group}:${entry.source.channel}`) ?? []), entry]);
    for (const sharing of shared.values()) {
      if (sharing.length < 2) continue;
      const label = sourceLabel(topology, sharing[0]?.source as RouteSource);
      const times = sharing.length === 2 ? "twice (about +6 dB)" : `${sharing.length} times`;
      const chain = sharing.find((entry) => entry.chain !== undefined)?.chain;
      const how = chain === undefined ? `${label} is in this mix twice` : `${label} also reaches this mix through AFX OUT ${chain + 1}`;
      for (const entry of sharing) warnings.set(entry.slot, `${how}, so it is summed ${times}`);
    }
    return warnings;
  }

  /**
   * What an empty chain passes through: the meter of the input routing feeds AFX IN k, with what to
   * say about it in the strip's title. Reading it is reactive, so the strip follows a re-route.
   */
  #chainInput(deviceId: string, chain: number): { note: string; meter?: InputMeter; source?: RouteSource } {
    const topology = this.topology(deviceId);
    if (topology === undefined) return { note: "This device's model is not known, so nothing is metered" };
    if (this.effects(deviceId).chains.value?.[chain]?.known !== true) return { note: "This chain has not been read, so nothing is known to meter" };
    const destination = topology.outputs.findIndex((group) => group.type === AFX_IN);
    if (destination < 0) return { note: "This model has no effect inputs to follow" };
    const routing = this.routing(deviceId);
    const slot = routing.destination(destination).value?.[chain];
    if (slot === undefined) return { note: "This chain is empty, and what feeds it has not been read" };
    if (slot.source === routing.mute) return { note: "This chain is empty and nothing is routed into it, so there is nothing to meter" };
    const source = { group: slot.source, channel: slot.channel };
    const label = sourceLabel(topology, source);
    // An effect chain fed by another chain's output would recur, and neither is in the status
    // report anyway: both, and every other unmetered input, say so rather than showing a bar.
    const meter = topology.inputs[slot.source]?.type === AFX_OUT ? undefined : this.inputMeter(deviceId, source);
    return meter === undefined ? { note: `Through an empty chain from ${label}, which the device reports no meter for`, source } : { note: `Through an empty chain from ${label}`, meter, source };
  }

  /**
   * One effect meter, following whichever effect `at` names. `at` is a signal so a chain's meter
   * can follow its last effect as the chain is edited.
   */
  #effectMeterAt(
    deviceId: string,
    family: "quadro" | "studio",
    at: ReadonlySignal<{ chain: number; position: number } | undefined>,
    key: string,
    note: ReadonlySignal<string>,
  ): EffectMeter {
    const existing = this.#effectMeters.get(key);
    if (existing !== undefined) return existing;
    const byte = (which: "peak" | "reduction") =>
      computed(() => {
        const where = at.value;
        if (where === undefined) return undefined;
        if (family === "studio") {
          const field = this.field(deviceId, EFFECT_METER_REPORT, which === "peak" ? "channel_peaks" : "channel_gain_reductions").value;
          const entry = Array.isArray(field) ? (field[where.chain] as Record<string, unknown> | undefined) : undefined;
          const slots = entry?.[which === "peak" ? "effect_peaks" : "values"];
          return slots instanceof Uint8Array && where.position < slots.length ? Number(slots[where.position]) : undefined;
        }
        // The Quadro's effect meters run from the first byte, chain by chain, two per loaded effect,
        // so the offset is how many effects the chains before this one hold. Everything after them
        // is the mic emulation meters, which are not an effect's.
        const chains = this.effects(deviceId).chains.value;
        if (chains === undefined) return undefined;
        let before = 0;
        for (let i = 0; i < where.chain; i++) before += chains[i]?.slots.length ?? 0;
        const field = this.field(deviceId, EFFECT_METER_REPORT, "afx_meters").value;
        const data = Array.isArray(field) ? (field[0] as Record<string, unknown> | undefined)?.["data"] : undefined;
        const index = (before + where.position) * 2 + (which === "peak" ? 0 : 1);
        return data instanceof Uint8Array && index < data.length ? Number(data[index]) : undefined;
      });
    const level = byte("peak");
    const light = this.#clipLight(computed(() => level.value === 0));
    const meter: EffectMeter = {
      level,
      reduction: byte("reduction"),
      clipped: light.lit,
      clearClip: () => this.#clearClip(light),
      note,
    };
    this.#effectMeters.set(key, meter);
    return meter;
  }

  readonly #effectMeters = new Map<string, EffectMeter>();

  /** The output meters a device reports, or undefined for a model that reports none of its own. */
  outputMeters(deviceId: string): readonly OutputMeter[] | undefined {
    const family = this.#devices.peek().find((d) => d.id === deviceId)?.family;
    if (family !== "quadro") return undefined;
    const existing = this.#outputMeters.get(deviceId);
    if (existing !== undefined) return existing;
    const meters = QUADRO_OUTPUT_METERS.map(([name, fieldName]): OutputMeter => {
      const field = this.field(deviceId, "0x73", fieldName);
      const side = (i: number) =>
        computed(() => {
          const bytes = field.value;
          return (bytes instanceof Uint8Array || Array.isArray(bytes)) && i < bytes.length ? Number((bytes as ArrayLike<number>)[i]) : undefined;
        });
      const left = side(0);
      const right = side(1);
      const light = this.#clipLight(computed(() => left.value === 0 || right.value === 0));
      return { name, left, right, clipped: light.lit, clearClip: () => this.#clearClip(light) };
    });
    this.#outputMeters.set(deviceId, meters);
    return meters;
  }

  readonly #outputMeters = new Map<string, readonly OutputMeter[]>();

  /** Whether the compact mixer dock under the pages is collapsed; remembered per browser. Until it is, it starts collapsed at phone width. */
  get mixerDockCollapsed(): ReadonlySignal<boolean> {
    return this.#mixerDockCollapsed;
  }

  setMixerDockCollapsed(collapsed: boolean): void {
    if (this.#mixerDockCollapsed.peek() !== collapsed) this.#mixerDockCollapsed.value = collapsed;
  }

  /** Shows a surface in the mixer dock (remembered per browser), or the device in view again (undefined). False for a surface that does not exist. */
  setMixerDockSurface(surfaceId: string | undefined): boolean {
    if (surfaceId !== undefined && !(this.#workspace.peek()?.surfaces ?? []).some((s) => s.id === surfaceId)) return false;
    this.#mixerDockSurface.value = surfaceId ?? null;
    return true;
  }

  /** How long clip lights stay lit once a clip ends, in ms, or null to hold them until cleared. Remembered. */
  get clipAutoClear(): ReadonlySignal<number | null> {
    return this.#clipAutoClear;
  }

  /** Sets the clip lights' auto-clear to one of `CLIP_AUTO_CLEAR_CHOICES`; a light already lit counts down afresh. */
  setClipAutoClear(ms: number | null): void {
    if (!CLIP_AUTO_CLEAR_CHOICES.includes(ms)) throw new RangeError(`clip auto-clear must be one of ${CLIP_AUTO_CLEAR_CHOICES.join(", ")}`);
    this.#clipAutoClear.value = ms;
    for (const light of this.#clipLights) if (light.lit.peek() && !light.clipping.peek()) this.#countDown(light);
  }

  /** Clears every clip light, input and output, on every device. */
  clearAllClips(): void {
    batch(() => {
      for (const light of this.#clipLights) this.#clearClip(light);
    });
  }

  /** A clip light that lights while `clipping` and counts down to clearing once it stops. */
  #clipLight(clipping: ReadonlySignal<boolean>): ClipLight {
    const light: ClipLight = { lit: signal(false), clipping, timer: undefined };
    this.#clipLights.add(light);
    // Made on first use, often inside a watch: untracked, so it stands on its own.
    untracked(() =>
      effect(() => {
        if (clipping.value) {
          this.#timers.clearTimeout(light.timer);
          light.timer = undefined;
          light.lit.value = true;
        } else if (untracked(() => light.lit.value)) {
          this.#countDown(light);
        }
      }),
    );
    return light;
  }

  #countDown(light: ClipLight): void {
    this.#timers.clearTimeout(light.timer);
    light.timer = undefined;
    const ms = this.#clipAutoClear.peek();
    if (ms === null) return;
    light.timer = this.#timers.setTimeout(() => {
      light.timer = undefined;
      light.lit.value = false;
    }, ms);
  }

  #clearClip(light: ClipLight): void {
    this.#timers.clearTimeout(light.timer);
    light.timer = undefined;
    light.lit.value = false;
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
   * Whether the device has a switchable sample-rate converter on its S/PDIF input (`set_spdif_src`;
   * SRC is the converter, not a source). The Studio+ has it; the Quadro's command table lists it,
   * but nothing in its panel sends it and its report has no field for it, so it is left alone there.
   */
  hasSpdifSrc(deviceId: string): boolean {
    return this.#devices.peek().find((d) => d.id === deviceId)?.family === "studio";
  }

  /** Whether the S/PDIF converter is on, from the status report's `spdif_src`. Reactive; undefined for a model without it. */
  spdifSrc(deviceId: string): boolean | undefined {
    if (!this.hasSpdifSrc(deviceId)) return undefined;
    return Number(this.field(deviceId, "0x73", "spdif_src").value ?? 0) === 1;
  }

  /** Switches the S/PDIF converter on or off. */
  setSpdifSrc(deviceId: string, on: boolean): boolean {
    if (!this.hasSpdifSrc(deviceId)) return false;
    void this.#invokeCommand(deviceId, "set_spdif_src", { spdif_src: on ? 1 : 0 }, { coalesce: `spdif_src:${deviceId}` });
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

  /**
   * Replaces the whole workspace with an imported document, sent exactly as given, and resolves
   * with why when the server refuses it (undefined when it is saved). Unlike an edit it is not
   * applied before the server accepts it, so a refused file never shows. An edit still waiting for
   * its save is superseded, or saved as usual if the import is refused. Nothing goes to a device.
   */
  async replaceWorkspace(workspace: Workspace): Promise<string | undefined> {
    if (!this.connected.peek()) return "Not connected to the server.";
    if (this.#saving.peek()) return "A change is still being saved; try again in a moment.";
    const pending = this.#saveTimer !== undefined;
    this.#timers.clearTimeout(this.#saveTimer);
    this.#saveTimer = undefined;
    this.#saving.value = true;
    try {
      const saved = await this.#client.workspace.put(workspace);
      this.#confirmed = saved;
      this.#workspace.value = saved;
      return undefined;
    } catch (error) {
      if (pending) this.#saveTimer = this.#timers.setTimeout(() => void this.#save(), SAVE_DEBOUNCE_MS);
      // A server from before workspace spec phase 1 answers a document it cannot deserialise in plain
      // text, so the client knows only the status; newer ones say which part in a `bad_value`.
      if (error instanceof GazelleError && /^http_4\d\d$/.test(error.code)) return "The server could not read it as a workspace: a part of it is missing or has the wrong type.";
      return `The server refused it: ${message(error)}`;
    } finally {
      this.#saving.value = false;
    }
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
