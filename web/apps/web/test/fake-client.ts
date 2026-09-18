// A client double for store tests: devices, events, cyclic reports, workspace and themes, with
// every invoke recorded.

import { readFileSync } from "node:fs";

import { GazelleError, type Client, type ClientEvents, type DeviceDescriptor, type DeviceHandle, type ServerInfo, type RecallAsk, type RecallPlan, type Snapshot, type SnapshotDiff, type SnapshotSummary, type Status, type UserTheme, type Workspace } from "gazelle-audio-client";

import type { KeyValueStorage } from "../src/store/store.ts";
import type { ThemeSource } from "../src/themes/theme.ts";

export const device = (id: string, family: DeviceDescriptor["family"], model: string | null): DeviceDescriptor => ({
  id,
  vid: 9189,
  pid: 1,
  slug: family,
  model,
  family,
  command_count: family === null ? null : 1,
  identity_stable: true,
  backend: "loopback",
  max_packet_size: 64,
});

export interface Invocation {
  deviceId: string;
  command: string;
  args: Record<string, unknown> | undefined;
  options: Record<string, unknown> | undefined;
}

export class FakeClient implements Client {
  status: Status = "open";
  server: ServerInfo = { version: "0.1.0", backend: "loopback", dry_run: true, notices: [] };
  readonly devices = new Map<string, DeviceDescriptor>();
  readonly listeners = new Map<string, Set<(value: unknown) => void>>();
  readonly cyclic = new Map<string, (fields: Record<string, unknown>) => void>();
  readonly invocations: Invocation[] = [];
  /** How an invoke settles; by default like a dry run that sent one byte. */
  respond: (call: Invocation) => Promise<unknown> = async (call) => ({ device_id: call.deviceId, command: call.command, sent_hex: "70", sent_len: 1, dry_run: true, response: null, response_error: null });
  stored: Workspace = { version: 1, groups: [], links: [], aliases: {}, mixers: {} };
  readonly puts: Workspace[] = [];
  failPuts: Error | undefined;
  userThemes: UserTheme[] = [];
  closed = false;

  constructor(...initial: DeviceDescriptor[]) {
    for (const d of initial) this.devices.set(d.id, d);
  }

  on<E extends keyof ClientEvents>(event: E, listener: (value: ClientEvents[E]) => void): () => void {
    const set = this.listeners.get(event) ?? new Set();
    this.listeners.set(event, set);
    const entry = listener as (value: unknown) => void;
    set.add(entry);
    return () => set.delete(entry);
  }

  emit<E extends keyof ClientEvents>(event: E, value: ClientEvents[E]): void {
    for (const listener of this.listeners.get(event) ?? []) listener(value);
  }

  device(id: string): DeviceHandle {
    const descriptor = this.devices.get(id);
    if (descriptor === undefined) throw new GazelleError("unknown_device", `no device ${id}`);
    if (descriptor.family === null) return { id, family: null, descriptor };
    const handle = {
      id,
      family: descriptor.family,
      descriptor,
      invoke: (command: string, args?: Record<string, unknown>, options?: Record<string, unknown>) => {
        const call = { deviceId: id, command, args, options };
        this.invocations.push(call);
        return this.respond(call);
      },
      onCyclic: (reportId: string, listener: (fields: Record<string, unknown>) => void) => {
        const key = `${id}|${reportId}`;
        this.cyclic.set(key, listener);
        return () => this.cyclic.delete(key);
      },
    };
    return handle as unknown as DeviceHandle;
  }

  readonly workspace = {
    get: async (): Promise<Workspace> => structuredClone(this.stored),
    put: async (workspace: Workspace): Promise<Workspace> => {
      if (this.failPuts !== undefined) throw this.failPuts;
      this.puts.push(structuredClone(workspace));
      this.stored = structuredClone(workspace);
      return structuredClone(workspace);
    },
  };

  /** Snapshots the fake server holds, newest first, with their whole documents. */
  storedSnapshots: Snapshot[] = [];
  /** What `compare` answers; a comparison with nothing differing by default. */
  diff: SnapshotDiff | undefined;
  /** Set to make the next snapshot call fail, as the server refusing one does. */
  failSnapshots: Error | undefined;

  readonly snapshots = {
    list: async (): Promise<SnapshotSummary[]> => {
      if (this.failSnapshots !== undefined) throw this.failSnapshots;
      return this.storedSnapshots.map((snapshot) => summarise(snapshot));
    },
    get: async (id: string): Promise<Snapshot> => {
      const found = this.storedSnapshots.find((snapshot) => snapshot.id === id);
      if (found === undefined) throw new GazelleError("unknown_snapshot", `no such snapshot: ${id}`);
      return structuredClone(found);
    },
    create: async (name: string, note = ""): Promise<SnapshotSummary> => {
      if (this.failSnapshots !== undefined) throw this.failSnapshots;
      const snapshot: Snapshot = { version: 1, id: `snap-${this.storedSnapshots.length + 1}`, name, created: new Date(2026, 8, 17, 21, 15).toISOString(), note, workspace: structuredClone(this.stored), devices: {} };
      this.storedSnapshots = [snapshot, ...this.storedSnapshots];
      return summarise(snapshot);
    },
    rename: async (id: string, change: { name?: string; note?: string }): Promise<SnapshotSummary> => {
      if (this.failSnapshots !== undefined) throw this.failSnapshots;
      const found = this.storedSnapshots.find((snapshot) => snapshot.id === id);
      if (found === undefined) throw new GazelleError("unknown_snapshot", `no such snapshot: ${id}`);
      if (change.name !== undefined) found.name = change.name;
      if (change.note !== undefined) found.note = change.note;
      return summarise(found);
    },
    delete: async (id: string): Promise<void> => {
      if (this.failSnapshots !== undefined) throw this.failSnapshots;
      this.storedSnapshots = this.storedSnapshots.filter((snapshot) => snapshot.id !== id);
    },
    compare: async (id: string): Promise<SnapshotDiff> => {
      if (this.failSnapshots !== undefined) throw this.failSnapshots;
      const found = this.storedSnapshots.find((snapshot) => snapshot.id === id);
      if (found === undefined) throw new GazelleError("unknown_snapshot", `no such snapshot: ${id}`);
      return this.diff ?? { snapshot: summarise(found), compared_at: found.created, workspace: [], devices: [], changes: 0, same: true };
    },
    import: async (snapshots: Snapshot[]): Promise<{ added: string[]; skipped: string[] }> => {
      if (this.failSnapshots !== undefined) throw this.failSnapshots;
      const added: string[] = [];
      const skipped: string[] = [];
      for (const snapshot of snapshots) {
        if (this.storedSnapshots.some((stored) => stored.id === snapshot.id)) skipped.push(snapshot.id);
        else {
          this.storedSnapshots = [...this.storedSnapshots, structuredClone(snapshot)];
          added.push(snapshot.id);
        }
      }
      return { added, skipped };
    },
    recallPlan: async (id: string, ask: RecallAsk = {}): Promise<RecallPlan> => {
      if (this.failSnapshots !== undefined) throw this.failSnapshots;
      const found = this.storedSnapshots.find((snapshot) => snapshot.id === id);
      if (found === undefined) throw new GazelleError("unknown_snapshot", `no such snapshot: ${id}`);
      this.lastRecallAsk = ask;
      return this.plan ?? emptyPlan(summarise(found));
    },
  };

  /** What `recallPlan` answers; a plan with nothing to send by default. */
  plan: RecallPlan | undefined;
  /** What the last `recallPlan` was asked for, so a test can see the page's choices. */
  lastRecallAsk: RecallAsk | undefined;

  async themes(): Promise<UserTheme[]> {
    return this.userThemes;
  }

  async close(): Promise<void> {
    this.closed = true;
  }
}

/** A recall plan with nothing to send: the shape the page reads, with none of it to do. */
function emptyPlan(snapshot: SnapshotSummary): RecallPlan {
  const part = (name: string, title: string, on: boolean, confirm: boolean) => ({ name, title, default_on: on, chosen: on, needs_confirming: confirm, confirmed: !confirm, steps: 0 });
  return {
    snapshot,
    prepared_at: snapshot.created,
    current_state_read: true,
    raise_threshold_db: 6,
    parts: [
      part("silence", "Silence the outputs", true, false),
      part("clock", "Clock", false, true),
      part("settings", "Device settings", false, false),
      part("dc_coupling", "DC coupling", false, true),
      part("inputs", "Inputs", true, false),
      part("phantom", "48V", false, true),
      part("routing", "Routing", true, false),
      part("mixer", "Mixer", true, false),
      part("outputs", "Outputs", true, false),
      part("restore", "Restore the outputs", true, false),
    ],
    devices: [],
    steps: [],
    excluded: [],
    raised_outputs: [],
    phantom_on: [],
    ready: 0,
    workspace_changes: 0,
    sent: false,
    note: "Nothing has been sent to any device.",
  };
}

function summarise(snapshot: Snapshot): SnapshotSummary {
  return {
    version: snapshot.version,
    id: snapshot.id,
    name: snapshot.name,
    created: snapshot.created,
    note: snapshot.note,
    devices: Object.entries(snapshot.devices).map(([device_id, device]) => ({
      device_id,
      family: device.family,
      model: device.model,
      read_at: device.read_at,
      current_preset: device.current_preset ?? null,
      sections: Object.keys(device.sections),
      unreadable: device.unreadable.length,
    })),
  };
}

export class MemoryStorage implements KeyValueStorage {
  readonly items = new Map<string, string>();
  getItem(key: string): string | null {
    return this.items.get(key) ?? null;
  }
  setItem(key: string, value: string): void {
    this.items.set(key, value);
  }
  removeItem(key: string): void {
    this.items.delete(key);
  }
}

const THEMES = new URL("../themes/", import.meta.url);

export const builtInThemes: ThemeSource[] = ["gazelle-dark", "gazelle-light"].map((id) => ({
  id,
  origin: "built-in" as const,
  data: JSON.parse(readFileSync(new URL(`${id}.json`, THEMES), "utf8")),
}));

export const flush = () => new Promise<void>((resolve) => setImmediate(resolve));
