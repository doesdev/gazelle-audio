// Snapshots (workspace spec §2): the list the Workspace page shows, taking one, renaming, deleting,
// and comparing one with the devices as they are now.
//
// Every read and every write here is to the server's snapshot store or its capture, which only asks
// the devices for their state. **Nothing in this file sends a setting to a device**, and taking or
// comparing a snapshot changes nothing on one. Recall — putting a snapshot back — is phase 6 and
// waits for a hardware session (spec §2.3, decision 0012).

import { signal, type ReadonlySignal } from "../core/signal.ts";
import { SNAPSHOT_VERSION } from "gazelle-audio-client";
import type { Change, DeviceDiff, SectionDiff, Snapshot, SnapshotDiff, SnapshotSummary } from "gazelle-audio-client";

// Elements do not import the client package, so what the Workspace page needs of a snapshot comes
// through here with the rest of the model.
export { SNAPSHOT_VERSION };
export type { Change, DeviceDiff, SectionDiff, Snapshot, SnapshotDiff, SnapshotSummary };

/** What the model needs from the client, so it can be tested without one. */
export interface SnapshotsContext {
  list(): Promise<SnapshotSummary[]>;
  get(id: string): Promise<Snapshot>;
  create(name: string, note?: string): Promise<SnapshotSummary>;
  rename(id: string, change: { name?: string; note?: string }): Promise<SnapshotSummary>;
  delete(id: string): Promise<void>;
  compare(id: string): Promise<SnapshotDiff>;
  import(snapshots: Snapshot[]): Promise<{ added: string[]; skipped: string[] }>;
}

/** What the section is doing, so its controls can say so and not be pressed twice. */
export type Busy = "loading" | "taking" | "comparing" | "renaming" | "deleting" | undefined;

const message = (error: unknown): string => (error instanceof Error ? error.message : String(error));

export class SnapshotsModel {
  readonly #context: SnapshotsContext;
  readonly #list = signal<readonly SnapshotSummary[]>([]);
  readonly #busy = signal<Busy>(undefined);
  readonly #problem = signal<string | undefined>(undefined);
  readonly #diff = signal<SnapshotDiff | undefined>(undefined);
  readonly #known = signal(false);
  #loaded = false;

  constructor(context: SnapshotsContext) {
    this.#context = context;
  }

  /** Every snapshot the server holds, newest first. */
  get list(): ReadonlySignal<readonly SnapshotSummary[]> {
    return this.#list;
  }

  get busy(): ReadonlySignal<Busy> {
    return this.#busy;
  }

  /** Why the last thing asked for did not happen; cleared when the next one is asked. */
  get problem(): ReadonlySignal<string | undefined> {
    return this.#problem;
  }

  /** The comparison being shown, if any. */
  get diff(): ReadonlySignal<SnapshotDiff | undefined> {
    return this.#diff;
  }

  /** True once the list has been fetched, so an empty list is not shown before it is known. */
  get known(): ReadonlySignal<boolean> {
    return this.#known;
  }

  async #run<T>(what: Exclude<Busy, undefined>, work: () => Promise<T>): Promise<T | undefined> {
    if (this.#busy.peek() !== undefined) return undefined;
    this.#busy.value = what;
    this.#problem.value = undefined;
    try {
      return await work();
    } catch (error) {
      this.#problem.value = message(error);
      return undefined;
    } finally {
      this.#busy.value = undefined;
    }
  }

  /** Fetches the list. Safe to call whenever the section is shown; it only fetches once by itself. */
  async load(): Promise<void> {
    await this.#run("loading", async () => {
      this.#list.value = await this.#context.list();
      this.#known.value = true;
    });
  }

  /** Fetches the list the first time the section is shown. */
  async loadOnce(): Promise<void> {
    if (this.#loaded) return;
    this.#loaded = true;
    await this.load();
  }

  /** Takes a snapshot of the workspace and every attached device. Reads only. */
  async take(name: string, note = ""): Promise<SnapshotSummary | undefined> {
    const taken = await this.#run("taking", async () => {
      const summary = await this.#context.create(name.trim(), note);
      this.#list.value = [summary, ...this.#list.peek()];
      return summary;
    });
    return taken;
  }

  async rename(id: string, name: string): Promise<boolean> {
    const renamed = await this.#run("renaming", async () => {
      const summary = await this.#context.rename(id, { name: name.trim() });
      this.#list.value = this.#list.peek().map((s) => (s.id === id ? summary : s));
      return true;
    });
    return renamed === true;
  }

  async remove(id: string): Promise<boolean> {
    const removed = await this.#run("deleting", async () => {
      await this.#context.delete(id);
      this.#list.value = this.#list.peek().filter((s) => s.id !== id);
      if (this.#diff.peek()?.snapshot.id === id) this.#diff.value = undefined;
      return true;
    });
    return removed === true;
  }

  /** Reads every device again and shows what differs from this snapshot. Sends nothing. */
  async compare(id: string): Promise<SnapshotDiff | undefined> {
    return this.#run("comparing", async () => {
      const diff = await this.#context.compare(id);
      this.#diff.value = diff;
      return diff;
    });
  }

  closeDiff(): void {
    this.#diff.value = undefined;
  }

  /** Every snapshot whole, for a backup file (spec §3.3). */
  async all(): Promise<Snapshot[] | undefined> {
    return this.#run("loading", async () => {
      const listed = await this.#context.list();
      this.#list.value = listed;
      this.#known.value = true;
      const snapshots: Snapshot[] = [];
      for (const summary of listed) snapshots.push(await this.#context.get(summary.id));
      return snapshots;
    });
  }

  /** Adds snapshots from a backup, keeping any already here. */
  async importAll(snapshots: Snapshot[]): Promise<{ added: string[]; skipped: string[] } | undefined> {
    return this.#run("loading", async () => {
      const result = await this.#context.import(snapshots);
      this.#list.value = await this.#context.list();
      this.#known.value = true;
      return result;
    });
  }
}

/** "17 Sep 2026, 21:15" from an RFC 3339 instant, in the reader's own time zone. */
export function formatWhen(created: string, now = new Date()): string {
  const when = new Date(created);
  if (Number.isNaN(when.getTime())) return created;
  const months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
  const two = (n: number) => String(n).padStart(2, "0");
  const year = when.getFullYear() === now.getFullYear() ? "" : ` ${when.getFullYear()}`;
  return `${when.getDate()} ${months[when.getMonth()]}${year}, ${two(when.getHours())}:${two(when.getMinutes())}`;
}

/** "Zen Quadro and Zen Studio+ · 12 values unread", or what a snapshot of nothing holds. */
export function describeSnapshot(summary: SnapshotSummary): string {
  const models = summary.devices.map((device) => device.model || device.device_id);
  const unread = summary.devices.reduce((total, device) => total + device.unreadable, 0);
  const devices = models.length === 0 ? "no devices were attached" : models.length === 1 ? models[0]! : `${models.slice(0, -1).join(", ")} and ${models.at(-1)}`;
  return unread === 0 ? devices : `${devices} · ${unread} ${unread === 1 ? "value" : "values"} could not be read`;
}

/** A captured value as one line: a number or string plainly, anything else as JSON. */
export function formatValue(value: unknown): string {
  if (value === undefined) return "—";
  if (value === null) return "none";
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  if (typeof value === "string") return value;
  return JSON.stringify(value);
}

/** How a change reads on its own line: "64 → 50", "was 2", "now 5", or why it is unknown. */
export function describeChange(change: Change): string {
  switch (change.kind) {
    case "changed":
      return `${formatValue(change.from)} → ${formatValue(change.to)}`;
    case "only_in_snapshot":
      return `was ${formatValue(change.from)}, not set now`;
    case "only_now":
      return `not in the snapshot, now ${formatValue(change.to)}`;
    case "unknown":
      return change.reason === undefined ? "could not be read" : `could not be read: ${change.reason}`;
  }
}

/** "3 changes, 1 value unread" for one section, so a collapsed section still says what is in it. */
export function describeSection(section: SectionDiff): string {
  const unknown = section.changes.filter((change) => change.kind === "unknown").length;
  const changed = section.changes.length - unknown;
  const parts: string[] = [];
  if (changed > 0) parts.push(`${changed} ${changed === 1 ? "change" : "changes"}`);
  if (unknown > 0) parts.push(`${unknown} ${unknown === 1 ? "value" : "values"} unread`);
  return parts.join(", ");
}

/** What a comparison amounts to in one sentence. */
export function describeDiff(diff: SnapshotDiff): string {
  const changed = diff.devices.flatMap((device) => device.sections).flatMap((section) => section.changes).concat(diff.workspace).filter((change) => change.kind !== "unknown").length;
  const unknown = diff.changes - changed;
  const missing = diff.devices.filter((device) => device.missing).length;
  const parts: string[] = [];
  parts.push(changed === 0 ? "Nothing that could be read differs" : `${changed} ${changed === 1 ? "difference" : "differences"}`);
  if (unknown > 0) parts.push(`${unknown} ${unknown === 1 ? "value" : "values"} could not be read on one side or the other`);
  if (missing > 0) parts.push(`${missing} ${missing === 1 ? "device is" : "devices are"} not attached`);
  return `${parts.join(" · ")}.`;
}
