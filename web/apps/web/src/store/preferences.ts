// Per-browser preferences (like the theme pick): each is a signal whose writes are saved to
// storage and whose stored value is validated on load, falling back to its default. Storage can
// be unavailable (private windows, blocked site data); preferences then last for the page only.

import { signal, type Signal } from "../core/signal.ts";

export interface KeyValueStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  /** Optional, so that a store without it can still keep preferences; a dropped key then stays. */
  removeItem?(key: string): void;
}

/** A signal that saves each new value under `key` and starts from the stored one when `parse` accepts it. */
export function persisted<T>(storage: KeyValueStorage | undefined, key: string, fallback: T, parse: (stored: unknown) => T | undefined): Signal<T> {
  let initial = fallback;
  try {
    const raw = storage?.getItem(key);
    if (raw !== null && raw !== undefined) {
      // Undefined is "not a valid value"; null can be a preference of its own.
      const parsed = parse(JSON.parse(raw));
      if (parsed !== undefined) initial = parsed;
    }
  } catch {
    // Unreadable or corrupt: use the default.
  }
  const inner = signal(initial);
  return {
    get value() {
      return inner.value;
    },
    set value(next: T) {
      inner.value = next;
      try {
        storage?.setItem(key, JSON.stringify(next));
      } catch {
        // Without storage the preference lasts for this page only.
      }
    },
    peek: () => inner.peek(),
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Channel strip width limits: narrower strips cannot fit their controls, wider ones waste the screen. */
export const STRIP_WIDTH_MIN = 48;
export const STRIP_WIDTH_MAX = 120;
export const STRIP_WIDTH_DEFAULT = 64;

export interface MixerWidth {
  /** Fit strips to the available width, between the limits. */
  auto: boolean;
  /** Strip width when not automatic; the strips scroll when they do not fit. */
  px: number;
}

export function clampStripWidth(px: number): number {
  return Math.min(STRIP_WIDTH_MAX, Math.max(STRIP_WIDTH_MIN, Math.round(px)));
}

export function parseMixerWidth(stored: unknown): MixerWidth | undefined {
  if (!isRecord(stored) || typeof stored["auto"] !== "boolean" || typeof stored["px"] !== "number" || !Number.isFinite(stored["px"])) return undefined;
  return { auto: stored["auto"], px: clampStripWidth(stored["px"]) };
}

/** The sidebar's sections, top to bottom. */
export const SIDEBAR_SECTIONS = ["devices", "meter", "controlRoom"] as const;
export type SidebarSection = (typeof SIDEBAR_SECTIONS)[number];

export interface SidebarState {
  /** The side of the page it docks to. */
  side: "left" | "right";
  /** Folded to its rail. */
  collapsed: boolean;
  /** Each section's collapse, once the person has toggled it; a section not here is open. */
  sections: Partial<Record<SidebarSection, boolean>>;
}

export const SIDEBAR_DEFAULT: SidebarState = { side: "right", collapsed: false, sections: {} };

/** Each part of a stored sidebar is checked on its own, so one bad part does not lose the rest. */
export function parseSidebar(stored: unknown): SidebarState | undefined {
  if (!isRecord(stored)) return undefined;
  const side = stored["side"] === "left" || stored["side"] === "right" ? stored["side"] : SIDEBAR_DEFAULT.side;
  const collapsed = typeof stored["collapsed"] === "boolean" ? stored["collapsed"] : SIDEBAR_DEFAULT.collapsed;
  const given = isRecord(stored["sections"]) ? stored["sections"] : {};
  const sections: Partial<Record<SidebarSection, boolean>> = {};
  for (const id of SIDEBAR_SECTIONS) {
    const value = given[id];
    if (typeof value === "boolean") sections[id] = value;
  }
  return { side, collapsed, sections };
}

/**
 * Before the single sidebar there were two side panels, the devices on the left and the meter and
 * Control Room on the right, each collapsed on its own (`{ leftCollapsed, rightCollapsed }`). Their
 * collapse becomes the sections' collapse, and the sidebar's when both were. Returns undefined when
 * there is nothing valid to carry over.
 */
export function sidebarFromPanels(stored: unknown): SidebarState | undefined {
  if (!isRecord(stored) || typeof stored["leftCollapsed"] !== "boolean" || typeof stored["rightCollapsed"] !== "boolean") return undefined;
  const left = stored["leftCollapsed"];
  const right = stored["rightCollapsed"];
  return { side: "right", collapsed: left && right, sections: { devices: left, meter: right, controlRoom: right } };
}

/**
 * Carries the two panels' preference under `fromKey` over to the sidebar's under `toKey`, unless a
 * sidebar is already stored, then drops the old one. Returns the carried-over state, which holds
 * for this page even where storage cannot be written.
 */
export function migratePanels(storage: KeyValueStorage | undefined, fromKey: string, toKey: string): SidebarState | undefined {
  let carried: SidebarState | undefined;
  try {
    const old = storage?.getItem(fromKey);
    if (old === null || old === undefined) return undefined;
    if (storage?.getItem(toKey) === null) {
      try {
        carried = sidebarFromPanels(JSON.parse(old));
      } catch {
        // Corrupt: nothing to carry over.
      }
      if (carried !== undefined) storage?.setItem(toKey, JSON.stringify(carried));
    }
    storage?.removeItem?.(fromKey);
  } catch {
    // Storage unavailable: whatever was carried over lasts for this page.
  }
  return carried;
}

/** A remembered device id, from a page that named it. */
export function parseSelectedDevice(stored: unknown): string | undefined {
  return typeof stored === "string" && stored !== "" ? stored : undefined;
}

/** The mix last chosen per device id; entries that are not a mix index are dropped. */
export function parseSelectedMixes(stored: unknown): Record<string, number> | undefined {
  if (!isRecord(stored)) return undefined;
  return Object.fromEntries(Object.entries(stored).filter((entry): entry is [string, number] => Number.isInteger(entry[1]) && (entry[1] as number) >= 0));
}

/**
 * Whether the Mixer page shows every configured channel, per device id, rather than only the ones
 * in the selected mix. Missing means the default, which is to show only the selected mix's.
 */
export function parseShowAllChannels(stored: unknown): Record<string, boolean> | undefined {
  if (!isRecord(stored)) return undefined;
  return Object.fromEntries(Object.entries(stored).filter((entry): entry is [string, boolean] => typeof entry[1] === "boolean"));
}
