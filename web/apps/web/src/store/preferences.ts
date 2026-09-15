// Per-browser preferences (like the theme pick): each is a signal whose writes are saved to
// storage and whose stored value is validated on load, falling back to its default. Storage can
// be unavailable (private windows, blocked site data); preferences then last for the page only.

import { signal, type Signal } from "../core/signal.ts";

export interface KeyValueStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/** A signal that saves each new value under `key` and starts from the stored one when `parse` accepts it. */
export function persisted<T>(storage: KeyValueStorage | undefined, key: string, fallback: T, parse: (stored: unknown) => T | undefined): Signal<T> {
  let initial = fallback;
  try {
    const raw = storage?.getItem(key);
    if (raw !== null && raw !== undefined) initial = parse(JSON.parse(raw)) ?? fallback;
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

export interface PanelState {
  leftCollapsed: boolean;
  rightCollapsed: boolean;
}

export function parsePanels(stored: unknown): PanelState | undefined {
  if (!isRecord(stored) || typeof stored["leftCollapsed"] !== "boolean" || typeof stored["rightCollapsed"] !== "boolean") return undefined;
  return { leftCollapsed: stored["leftCollapsed"], rightCollapsed: stored["rightCollapsed"] };
}
