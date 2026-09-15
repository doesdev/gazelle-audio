// A client double for store tests: devices, events, cyclic reports, workspace and themes, with
// every invoke recorded.

import { readFileSync } from "node:fs";

import { GazelleError, type Client, type ClientEvents, type DeviceDescriptor, type DeviceHandle, type ServerInfo, type Status, type UserTheme, type Workspace } from "gazelle-audio-client";

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
  server: ServerInfo = { version: "0.1.0", backend: "loopback", dry_run: true };
  readonly devices = new Map<string, DeviceDescriptor>();
  readonly listeners = new Map<string, Set<(value: unknown) => void>>();
  readonly cyclic = new Map<string, (fields: Record<string, unknown>) => void>();
  readonly invocations: Invocation[] = [];
  /** How an invoke settles; by default like a dry run that sent one byte. */
  respond: (call: Invocation) => Promise<unknown> = async (call) => ({ device_id: call.deviceId, command: call.command, sent_hex: "70", sent_len: 1, dry_run: true, response: null, response_error: null });
  stored: Workspace = { version: 1, groups: [], links: [], aliases: {} };
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

  async themes(): Promise<UserTheme[]> {
    return this.userThemes;
  }

  async close(): Promise<void> {
    this.closed = true;
  }
}

export class MemoryStorage implements KeyValueStorage {
  readonly items = new Map<string, string>();
  getItem(key: string): string | null {
    return this.items.get(key) ?? null;
  }
  setItem(key: string, value: string): void {
    this.items.set(key, value);
  }
}

const THEMES = new URL("../themes/", import.meta.url);

export const builtInThemes: ThemeSource[] = ["gazelle-dark", "gazelle-light"].map((id) => ({
  id,
  origin: "built-in" as const,
  data: JSON.parse(readFileSync(new URL(`${id}.json`, THEMES), "utf8")),
}));

export const flush = () => new Promise<void>((resolve) => setImmediate(resolve));
