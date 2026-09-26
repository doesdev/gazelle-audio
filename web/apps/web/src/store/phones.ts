// Phones on the network, as the Workspace page's Phones section shows them: whether phones are
// allowed, the addresses a phone would use, pairing with its code and QR code, and the paired
// phones with a way to revoke each. Everything here is answered only to the computer Gazelle runs
// on; on a phone the section says so instead.

import { GazelleError, type RemotePairing, type RemoteQr, type RemoteStatus } from "gazelle-audio-client";

import { signal, type ReadonlySignal } from "../core/signal.ts";

export type { RemotePairing, RemoteQr, RemoteStatus };

/** What the section can call. */
export interface PhonesApi {
  status(): Promise<RemoteStatus>;
  setAllowPhones(on: boolean): Promise<RemoteStatus>;
  startPairing(): Promise<RemotePairing>;
  cancelPairing(): Promise<{ cancelled: boolean }>;
  revoke(id: string): Promise<{ revoked: boolean }>;
}

export interface PhonesTimers {
  setTimeout(callback: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

/** How often the list is read while the section is open, and while a pairing waits for its phone. */
export const PHONES_POLL_MS = 10_000;
export const PAIRING_POLL_MS = 1_500;

/** The section's state: still reading, answered, or not for this device to see. */
export type PhonesState =
  | { state: "loading" }
  | { state: "ready"; status: RemoteStatus }
  /** Asked from a phone: the section says where phones are managed. */
  | { state: "elsewhere" }
  | { state: "failed"; message: string };

const message = (error: unknown): string => (error instanceof Error ? error.message : String(error));

export class PhonesModel {
  readonly #api: PhonesApi;
  readonly #timers: PhonesTimers;
  readonly #state = signal<PhonesState>({ state: "loading" });
  readonly #busy = signal(false);
  readonly #problem = signal<string | undefined>(undefined);
  /** A phone that has just paired, named for a moment so the person sees it land. */
  readonly #justPaired = signal<string | undefined>(undefined);
  #watchers = 0;
  #timer: unknown;

  constructor(api: PhonesApi, timers: PhonesTimers) {
    this.#api = api;
    this.#timers = timers;
  }

  get state(): ReadonlySignal<PhonesState> {
    return this.#state;
  }

  /** True while a change is on its way, so the buttons wait for it. */
  get busy(): ReadonlySignal<boolean> {
    return this.#busy;
  }

  /** Why the last thing pressed did not work, cleared by the next that does. */
  get problem(): ReadonlySignal<string | undefined> {
    return this.#problem;
  }

  get justPaired(): ReadonlySignal<string | undefined> {
    return this.#justPaired;
  }

  /** Keep the section current while something shows it. Returns the way to stop. */
  follow(): () => void {
    this.#watchers += 1;
    if (this.#watchers === 1) void this.refresh();
    let stopped = false;
    return () => {
      if (stopped) return;
      stopped = true;
      this.#watchers -= 1;
      if (this.#watchers === 0) this.#timers.clearTimeout(this.#timer);
    };
  }

  /** Read the status now, and again later while anything follows. */
  async refresh(): Promise<void> {
    this.#timers.clearTimeout(this.#timer);
    try {
      this.#land(await this.#api.status());
    } catch (error) {
      if (error instanceof GazelleError && error.code === "not_local") {
        this.#state.value = { state: "elsewhere" };
        return;
      }
      this.#state.value = { state: "failed", message: message(error) };
    }
    this.#schedule();
  }

  #schedule(): void {
    if (this.#watchers === 0) return;
    const current = this.#state.peek();
    const waiting = current.state === "ready" && current.status.pairing !== null;
    this.#timer = this.#timers.setTimeout(() => void this.refresh(), waiting ? PAIRING_POLL_MS : PHONES_POLL_MS);
  }

  /** A new answer. A phone that appeared while a pairing was running is the one that paired. */
  #land(status: RemoteStatus): void {
    const before = this.#state.peek();
    if (before.state === "ready" && before.status.pairing !== null && status.pairing === null) {
      const known = new Set(before.status.phones.map((p) => p.id));
      const added = status.phones.find((p) => !known.has(p.id));
      if (added !== undefined) this.#justPaired.value = added.name;
    }
    this.#state.value = { state: "ready", status };
  }

  async #act(work: () => Promise<void>): Promise<void> {
    if (this.#busy.peek()) return;
    this.#busy.value = true;
    try {
      await work();
      this.#problem.value = undefined;
    } catch (error) {
      this.#problem.value = message(error);
    } finally {
      this.#busy.value = false;
    }
  }

  setAllowPhones(on: boolean): Promise<void> {
    return this.#act(async () => {
      this.#land(await this.#api.setAllowPhones(on));
    });
  }

  startPairing(): Promise<void> {
    return this.#act(async () => {
      this.#justPaired.value = undefined;
      await this.#api.startPairing();
      await this.refresh();
    });
  }

  cancelPairing(): Promise<void> {
    return this.#act(async () => {
      await this.#api.cancelPairing();
      await this.refresh();
    });
  }

  revoke(id: string): Promise<void> {
    return this.#act(async () => {
      await this.#api.revoke(id);
      await this.refresh();
    });
  }
}

/** Minutes and seconds left, `4:07`, never below `0:00`. */
export function countdown(expiresMs: number, nowMs: number): string {
  const left = Math.max(0, Math.ceil((expiresMs - nowMs) / 1000));
  return `${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}`;
}

/** When something happened, as a person would say it. */
export function ago(thenMs: number | null, nowMs: number): string {
  if (thenMs === null) return "never";
  const seconds = Math.max(0, Math.round((nowMs - thenMs) / 1000));
  if (seconds < 60) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return minutes === 1 ? "a minute ago" : `${minutes} minutes ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return hours === 1 ? "an hour ago" : `${hours} hours ago`;
  const days = Math.round(hours / 24);
  return days === 1 ? "yesterday" : `${days} days ago`;
}

/** The line under the switch: whether and where phones can reach Gazelle, or why not. */
export function reachText(status: RemoteStatus): { text: string; problem: boolean } {
  if (status.fixed_by_bind !== null) {
    return { text: `Gazelle was started with --bind ${status.fixed_by_bind}, so other devices on the network can reach it and this switch has no say. Every phone still has to be paired.`, problem: false };
  }
  if (!status.allow_phones) return { text: "Off: only this computer can reach Gazelle.", problem: false };
  if (status.error !== null) return { text: status.error, problem: true };
  if (!status.listening) return { text: "Starting to listen on the network...", problem: false };
  if (status.urls.length === 0) return { text: `Listening on port ${status.port}, but this computer's network address could not be found. Use the address your network settings show, with :${status.port}.`, problem: true };
  return { text: `Phones on this network can reach Gazelle at ${status.urls.join(" or ")}, once paired.`, problem: false };
}

/** What Windows may ask, said before it asks. */
export const FIREWALL_NOTE =
  "The first time Gazelle listens on the network, Windows may ask whether to allow it. Allow it on private networks only. Gazelle never changes the firewall itself.";

/** What a phone on the network can and cannot see, said plainly. */
export const PLAIN_HTTP_NOTE =
  "The connection is plain HTTP: anyone on the same network who can watch its traffic could see what the phone and Gazelle send, pairing included. Pairing decides who can control Gazelle, not who can watch. Use a network you trust.";

/**
 * A QR code as one SVG path, a unit square per dark module, drawn inside a quiet zone of four
 * modules as the standard asks. Returns the path and the full size in modules.
 */
export function qrPath(qr: RemoteQr, quiet = 4): { d: string; size: number } {
  const parts: string[] = [];
  qr.rows.forEach((row, y) => {
    let x = 0;
    while (x < row.length) {
      if (row[x] !== "1") {
        x += 1;
        continue;
      }
      // One rectangle for a run of dark modules along the row.
      let end = x;
      while (end < row.length && row[end] === "1") end += 1;
      parts.push(`M${x + quiet} ${y + quiet}h${end - x}v1h${x - end}z`);
      x = end;
    }
  });
  return { d: parts.join(""), size: qr.size + quiet * 2 };
}
