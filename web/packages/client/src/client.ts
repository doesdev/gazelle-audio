// The connection. Invocation goes over the WebSocket, the workspace over HTTP. Choices
// agreed with the user: a 5 s default timeout; reconnect from 250 ms
// doubling to 5 s with jitter until close(), replaying nothing; coalescing keeps at most one call
// per key waiting on the server and rejects the one it replaces with `superseded`; server error
// codes and `detail` pass through unchanged. Data from the server keeps its snake_case keys.

import { decodeFields, encodeArgs, isObject } from "./bytes.ts";
import type { AggregateAnswer, AggregateCalibrateRequest, AggregateCalibrateStarted, AggregateCalibrateStopped, AggregateCalibration, AggregateMatchBuffers, AggregateRegistrationRun } from "./aggregate.ts";
import type { DriverChange, DriverReport, DriverWriteReport } from "./driver.ts";
import type { UpdateRestart, UpdateStatus } from "./update.ts";
import { GazelleError } from "./errors.ts";
import { schemas, type Family, type FamilyTypes } from "./generated/index.ts";
import type { FamilySchema, FieldDescriptor } from "./schema.ts";
import type { RecallAsk, RecallPlan, Snapshot, SnapshotDiff, SnapshotImport, SnapshotSummary } from "./snapshots.ts";
import type { Workspace } from "./workspace.ts";

export const API_PATH = "/api/v1";
export const DEFAULT_TIMEOUT_MS = 5000;
export const RECONNECT_INITIAL_MS = 250;
export const RECONNECT_MAX_MS = 5000;

export type Status = "open" | "reconnecting" | "closed";

export interface DeviceDescriptor {
  id: string;
  vid: number;
  pid: number;
  slug: string | null;
  model: string | null;
  family: Family | null;
  command_count: number | null;
  identity_stable: boolean;
  backend: string;
  max_packet_size: number;
}

/** From the server's `hello`. */
export interface ServerInfo {
  version: string;
  backend: string;
  dry_run: boolean;
  /** What the server wants said beyond the device list, in the order it sent them; see its
   * `notice.rs`. Empty from a server that sends none. */
  notices: readonly string[];
}

export interface ClientEvents {
  status: Status;
  device_added: DeviceDescriptor;
  device_removed: string;
  /** This connection fell behind; that many events are gone. */
  lagged: number;
}

export interface InvokeOptions {
  /** Replace any call with this key that is still waiting to be sent. */
  coalesce?: string;
  dryRun?: boolean;
  timeoutMs?: number;
  /** The header `ext3` selector, for commands that take one (`get_routing`'s destination group, `get_mixer`'s mixer). */
  ext3?: number;
}

export interface InvokeResult<R> {
  device_id: string;
  command: string;
  sent_hex: string;
  sent_len: number;
  dry_run: boolean;
  response: R | null;
  response_error: string | null;
}

type Commands<F extends Family> = FamilyTypes[F]["commands"];
type Cyclic<F extends Family> = FamilyTypes[F]["cyclic"];
type ParamsOf<F extends Family, K extends keyof Commands<F>> = Commands<F>[K] extends { params: infer P } ? P : never;
type ReturnsOf<F extends Family, K extends keyof Commands<F>> = Commands<F>[K] extends { returns: infer R } ? R : never;
/** Arguments may be left out when every parameter is optional. */
export type InvokeArgs<P> = {} extends P ? [args?: P, options?: InvokeOptions] : [args: P, options?: InvokeOptions];

export interface TypedDevice<F extends Family> {
  readonly id: string;
  readonly family: F;
  readonly descriptor: DeviceDescriptor;
  invoke<K extends keyof Commands<F> & string>(name: K, ...rest: InvokeArgs<ParamsOf<F, K>>): Promise<InvokeResult<ReturnsOf<F, K>>>;
  onCyclic<K extends keyof Cyclic<F> & string>(reportId: K, listener: (fields: Cyclic<F>[K]) => void): () => void;
}

/** A device of unknown model: the server has no registry for it (its `501`), so no commands. */
export interface UntypedDevice {
  readonly id: string;
  readonly family: null;
  readonly descriptor: DeviceDescriptor;
}

export type DeviceHandle = { [F in Family]: TypedDevice<F> }[Family] | UntypedDevice;

export interface Client {
  readonly status: Status;
  readonly server: ServerInfo;
  readonly devices: ReadonlyMap<string, DeviceDescriptor>;
  /** Returns an unsubscribe function. */
  on<E extends keyof ClientEvents>(event: E, listener: (value: ClientEvents[E]) => void): () => void;
  /** Throws `unknown_device` when the id is not currently known. */
  device(id: string): DeviceHandle;
  readonly workspace: { get(): Promise<Workspace>; put(workspace: Workspace): Promise<Workspace> };
  /**
   * Snapshots the server keeps. `create` and `compare` read every attached device and send
   * nothing to any of them; `rename` changes a snapshot's name and note and nothing it recorded.
   */
  readonly snapshots: {
    list(): Promise<SnapshotSummary[]>;
    get(id: string): Promise<Snapshot>;
    create(name: string, note?: string): Promise<SnapshotSummary>;
    rename(id: string, change: { name?: string; note?: string }): Promise<SnapshotSummary>;
    delete(id: string): Promise<void>;
    compare(id: string): Promise<SnapshotDiff>;
    /** Add snapshots from a backup file, keeping any already stored. */
    import(snapshots: Snapshot[]): Promise<SnapshotImport>;
    /**
     * What recall would send to put this snapshot back: an ordered list of commands with their
     * bytes and their guards. Reads every device and **sends nothing**; there is deliberately no
     * way here to apply one.
     */
    recallPlan(id: string, ask?: RecallAsk): Promise<RecallPlan>;
  };
  /** User theme files from the server's themes directory; the UI validates each theme. */
  themes(): Promise<UserTheme[]>;
  /**
   * The audio driver's settings for a device (buffer size, latency, Safe Mode), read from the
   * driver on the server's PC. Read only. The server keeps an answer a few seconds; `refresh`
   * asks the driver again.
   */
  driver(id: string, options?: { refresh?: boolean }): Promise<DriverReport>;
  /**
   * Changes the driver's buffer size and/or Safe Mode for a device, through the driver's one setter,
   * and answers what the driver reports afterwards. Rejects, having sent nothing, when the change
   * cannot be made: a size the driver does not offer, no driver, or a program using its ASIO
   * interface without `force` (code `asio_in_use`).
   */
  setDriver(id: string, change: DriverChange): Promise<DriverWriteReport>;
  /**
   * The aggregate audio driver: one driver a DAW opens with several interfaces underneath it.
   * Served only on a loopback bind and only to a caller on the same machine, like `update`, so a
   * page treats a rejection as "this server does not offer it" rather than as a failure. The
   * setup itself is the workspace's `aggregate` section, saved with the rest of it.
   */
  readonly aggregate: {
    /**
     * Everything at once: the drivers on this PC, whether ours is registered, each configured
     * device's live clock, rate, buffer and USB controller, the driver's own record and log, and
     * a single ready or not ready with reasons. Reads the audio drivers, so it is not free;
     * a second or two apart is often enough.
     */
    read(): Promise<AggregateAnswer>;
    /**
     * Put every configured interface on one buffer size, through the same write path and the
     * same refusals as `setDriver`. Each device answers for itself: one refusing (`asio_in_use`
     * without `force`) does not stop the others.
     */
    matchBuffers(buffer_size: number, options?: { force?: boolean }): Promise<AggregateMatchBuffers>;
    /**
     * Register the driver's DLL, which needs administrator rights: Windows puts up its own
     * prompt, and a declined prompt comes back as `run.started` false rather than as a failure.
     * The answer always carries the command a person could run instead. Rejects with
     * `dll_not_found`, having run nothing, when there is no DLL to register; the error names
     * every place that was looked in.
     */
    register(): Promise<AggregateRegistrationRun>;
    unregister(): Promise<AggregateRegistrationRun>;
    /**
     * The one measurement at a time that lines the interfaces up: idle, running with a step and how
     * far along it is, done with what it measured, or failed with the refusal that says why.
     */
    calibration(): Promise<AggregateCalibration>;
    /**
     * Starts a measurement. It plays a click out of a real output and takes the audio drivers for
     * itself, so it rejects, having played nothing, while a DAW has them open, and when the
     * channels named do not make sense for the pass being run.
     */
    calibrate(request: AggregateCalibrateRequest): Promise<AggregateCalibrateStarted>;
    /** Stops the run that is going. Stopping one that is not is answered, not refused. */
    stopCalibrate(): Promise<AggregateCalibrateStopped>;
  };
  /**
   * The in-app updater. Served only on a loopback bind, so every call rejects with `http_404` on
   * a server reachable from the network; a caller that shows update state treats that as "this
   * server does not do updates" rather than as a failure.
   */
  readonly update: {
    /** What is known now. Asks the release source nothing, so it may be polled. */
    status(): Promise<UpdateStatus>;
    /** Ask the release source. What it finds is fetched too unless the settings say not to. */
    check(): Promise<UpdateStatus>;
    /** Fetch, verify and stage what the last check found. */
    download(): Promise<UpdateStatus>;
    /**
     * Stop the server and start the staged version. Answers first and stops a moment later, so
     * the connection drops straight after; rejects with `nothing_staged`, having stopped nothing,
     * when there is nothing to restart into.
     */
    restart(): Promise<UpdateRestart>;
  };
  close(): Promise<void>;
}

/** One file from the server's themes directory: its parsed JSON, or why it cannot be used. */
export interface UserTheme {
  file: string;
  theme?: Record<string, unknown>;
  error?: string;
}

/** The part of the standard WebSocket the client uses; tests inject a fake. */
export interface SocketLike {
  send(data: string): void;
  close(code?: number, reason?: string): void;
  onopen: ((event: unknown) => void) | null;
  onmessage: ((event: { data: unknown }) => void) | null;
  onclose: ((event: unknown) => void) | null;
  onerror: ((event: unknown) => void) | null;
}

export type SocketFactory = new (url: string) => SocketLike;

export interface Timers {
  setTimeout(callback: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

export type FetchLike = (
  url: string,
  init?: { method?: string; headers?: Record<string, string>; body?: string },
) => Promise<{ ok: boolean; status: number; json(): Promise<unknown> }>;

export interface ConnectOptions {
  /** Default per-call timeout, and how long to wait for `hello`. */
  timeoutMs?: number;
  WebSocket?: SocketFactory;
  fetch?: FetchLike;
  timers?: Timers;
  /** In [0, 1); scales each reconnect delay between half and all of its ceiling. */
  random?: () => number;
  reconnect?: { initialMs?: number; maxMs?: number };
}

/** Connects and resolves once the server's `hello` arrives. */
export async function connect(baseUrl: string, options: ConnectOptions = {}): Promise<Client> {
  const connection = new Connection(baseUrl, options);
  await connection.start();
  return connection;
}

interface Call {
  deviceId: string;
  command: string;
  args: Record<string, unknown> | undefined;
  dryRun: boolean | undefined;
  ext3: number | undefined;
  timeoutMs: number;
  returns: readonly FieldDescriptor[] | null;
  resolve(result: InvokeResult<unknown>): void;
  reject(error: GazelleError): void;
}

interface Pending {
  call: Call;
  timer: unknown;
  coalesce: string | undefined;
}

type Frame = Record<string, unknown>;

const defaultTimers: Timers = {
  setTimeout: (callback, ms) => globalThis.setTimeout(callback, ms),
  clearTimeout: (handle) => globalThis.clearTimeout(handle as Parameters<typeof globalThis.clearTimeout>[0]),
};

/** The server's spelling of a report id: `0x` and uppercase hex digits. */
function reportKey(id: string): string {
  const n = Number.parseInt(id, 16);
  return Number.isNaN(n) ? id : `0x${n.toString(16).toUpperCase()}`;
}

function parseFrame(data: unknown): Frame | undefined {
  if (typeof data !== "string") return undefined;
  try {
    const frame: unknown = JSON.parse(data);
    return isObject(frame) ? frame : undefined;
  } catch {
    return undefined;
  }
}

function familySchema(family: Family): FamilySchema {
  return schemas[family];
}

class Connection implements Client {
  #status: Status = "closed";
  #server: ServerInfo = { version: "", backend: "", dry_run: false, notices: [] };
  readonly #devices = new Map<string, DeviceDescriptor>();
  readonly #listeners = new Map<string, Set<(value: unknown) => void>>();
  readonly #cyclic = new Map<string, Set<{ reportId: string; listener: (fields: unknown) => void }>>();
  readonly #pending = new Map<number, Pending>();
  /** Coalesce key → the call waiting behind the one in flight (the key's presence means one is). */
  readonly #slots = new Map<string, { queued: Call | undefined }>();
  #socket: SocketLike | undefined;
  #nextId = 0;
  #attempt = 0;
  #reconnectTimer: unknown;
  #helloTimer: unknown;

  readonly #base: string;
  readonly #timeoutMs: number;
  readonly #Socket: SocketFactory;
  readonly #fetch: FetchLike;
  readonly #timers: Timers;
  readonly #random: () => number;
  readonly #initialMs: number;
  readonly #maxMs: number;

  readonly workspace = {
    get: async (): Promise<Workspace> => (await this.#http("GET", "workspace")) as Workspace,
    put: async (workspace: Workspace): Promise<Workspace> => (await this.#http("PUT", "workspace", workspace)) as Workspace,
  };

  readonly snapshots = {
    list: async (): Promise<SnapshotSummary[]> => {
      const listed = await this.#http("GET", "snapshots");
      const snapshots = isObject(listed) ? listed["snapshots"] : undefined;
      return Array.isArray(snapshots) ? (snapshots as SnapshotSummary[]) : [];
    },
    get: async (id: string): Promise<Snapshot> => (await this.#http("GET", `snapshots/${encodeURIComponent(id)}`)) as Snapshot,
    create: async (name: string, note = ""): Promise<SnapshotSummary> => (await this.#http("POST", "snapshots", { name, note })) as SnapshotSummary,
    rename: async (id: string, change: { name?: string; note?: string }): Promise<SnapshotSummary> =>
      (await this.#http("PATCH", `snapshots/${encodeURIComponent(id)}`, change)) as SnapshotSummary,
    delete: async (id: string): Promise<void> => {
      await this.#http("DELETE", `snapshots/${encodeURIComponent(id)}`);
    },
    compare: async (id: string): Promise<SnapshotDiff> => (await this.#http("GET", `snapshots/${encodeURIComponent(id)}/compare`)) as SnapshotDiff,
    import: async (snapshots: Snapshot[]): Promise<SnapshotImport> => (await this.#http("POST", "snapshots/import", snapshots)) as SnapshotImport,
    recallPlan: async (id: string, ask: RecallAsk = {}): Promise<RecallPlan> =>
      (await this.#http("POST", `snapshots/${encodeURIComponent(id)}/recall/plan`, ask)) as RecallPlan,
  };

  async themes(): Promise<UserTheme[]> {
    const listed = await this.#http("GET", "themes");
    return Array.isArray(listed) ? (listed as UserTheme[]) : [];
  }

  async driver(id: string, options: { refresh?: boolean } = {}): Promise<DriverReport> {
    return (await this.#http("GET", `devices/${encodeURIComponent(id)}/driver${options.refresh === true ? "?refresh=true" : ""}`)) as DriverReport;
  }

  async setDriver(id: string, change: DriverChange): Promise<DriverWriteReport> {
    return (await this.#http("PUT", `devices/${encodeURIComponent(id)}/driver`, change)) as DriverWriteReport;
  }

  readonly aggregate = {
    read: async (): Promise<AggregateAnswer> => (await this.#http("GET", "aggregate")) as AggregateAnswer,
    matchBuffers: async (buffer_size: number, options: { force?: boolean } = {}): Promise<AggregateMatchBuffers> =>
      (await this.#http("POST", "aggregate/match-buffers", { buffer_size, ...(options.force === true ? { force: true } : {}) })) as AggregateMatchBuffers,
    register: async (): Promise<AggregateRegistrationRun> => (await this.#http("POST", "aggregate/register", {})) as AggregateRegistrationRun,
    unregister: async (): Promise<AggregateRegistrationRun> => (await this.#http("POST", "aggregate/unregister", {})) as AggregateRegistrationRun,
    calibration: async (): Promise<AggregateCalibration> => (await this.#http("GET", "aggregate/calibrate")) as AggregateCalibration,
    calibrate: async (request: AggregateCalibrateRequest): Promise<AggregateCalibrateStarted> =>
      (await this.#http("POST", "aggregate/calibrate", request)) as AggregateCalibrateStarted,
    stopCalibrate: async (): Promise<AggregateCalibrateStopped> => (await this.#http("POST", "aggregate/calibrate/stop", {})) as AggregateCalibrateStopped,
  };

  readonly update = {
    status: async (): Promise<UpdateStatus> => (await this.#http("GET", "update")) as UpdateStatus,
    check: async (): Promise<UpdateStatus> => (await this.#http("POST", "update/check")) as UpdateStatus,
    download: async (): Promise<UpdateStatus> => (await this.#http("POST", "update/download")) as UpdateStatus,
    restart: async (): Promise<UpdateRestart> => (await this.#http("POST", "update/restart")) as UpdateRestart,
  };

  constructor(baseUrl: string, options: ConnectOptions) {
    const Socket = options.WebSocket ?? ((globalThis as { WebSocket?: unknown }).WebSocket as SocketFactory | undefined);
    if (Socket === undefined) throw new GazelleError("not_connected", "no WebSocket implementation; use Node 22+ or pass options.WebSocket");
    this.#base = baseUrl;
    this.#timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
    this.#Socket = Socket;
    this.#fetch = options.fetch ?? ((url, init) => globalThis.fetch(url, init));
    this.#timers = options.timers ?? defaultTimers;
    this.#random = options.random ?? Math.random;
    this.#initialMs = options.reconnect?.initialMs ?? RECONNECT_INITIAL_MS;
    this.#maxMs = options.reconnect?.maxMs ?? RECONNECT_MAX_MS;
  }

  get status(): Status {
    return this.#status;
  }

  get server(): ServerInfo {
    return this.#server;
  }

  get devices(): ReadonlyMap<string, DeviceDescriptor> {
    return this.#devices;
  }

  start(): Promise<void> {
    return new Promise((resolve, reject) => this.#dial({ resolve, reject }));
  }

  on<E extends keyof ClientEvents>(event: E, listener: (value: ClientEvents[E]) => void): () => void {
    const set = this.#listeners.get(event) ?? new Set();
    this.#listeners.set(event, set);
    const entry = listener as (value: unknown) => void;
    set.add(entry);
    return () => set.delete(entry);
  }

  device(id: string): DeviceHandle {
    const descriptor = this.#devices.get(id);
    if (descriptor === undefined) throw new GazelleError("unknown_device", `no device ${id}`);
    if (descriptor.family === null) return { id, family: null, descriptor };
    const handle = {
      id,
      family: descriptor.family,
      descriptor,
      invoke: (name: string, args?: Record<string, unknown>, options?: InvokeOptions) => this.#invoke(this.#devices.get(id) ?? descriptor, name, args, options),
      onCyclic: (reportId: string, listener: (fields: unknown) => void) => this.#onCyclic(id, reportId, listener),
    };
    return handle as unknown as DeviceHandle;
  }

  async close(): Promise<void> {
    this.#setStatus("closed");
    this.#timers.clearTimeout(this.#reconnectTimer);
    this.#timers.clearTimeout(this.#helloTimer);
    this.#failAll(new GazelleError("closed", "the client was closed"));
    const socket = this.#socket;
    this.#socket = undefined;
    if (socket !== undefined) {
      socket.onclose = null;
      socket.onmessage = null;
      socket.close(1000);
    }
  }

  #emit<E extends keyof ClientEvents>(event: E, value: ClientEvents[E]): void {
    for (const listener of [...(this.#listeners.get(event) ?? [])]) listener(value);
  }

  #setStatus(status: Status): void {
    if (this.#status === status) return;
    this.#status = status;
    this.#emit("status", status);
  }

  /** Opens a socket. `first` is the initial connect, which fails instead of retrying. */
  #dial(first?: { resolve: () => void; reject: (error: GazelleError) => void }): void {
    const url = new URL(`${API_PATH}/ws`, this.#base);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    const failed = (error: GazelleError) => {
      if (first !== undefined) first.reject(error);
      else this.#scheduleReconnect();
    };
    let socket: SocketLike;
    try {
      socket = new this.#Socket(url.toString());
    } catch (e) {
      failed(new GazelleError("closed", `could not open ${url}: ${String(e)}`));
      return;
    }
    this.#socket = socket;
    let greeted = false;
    this.#helloTimer = this.#timers.setTimeout(() => {
      if (greeted || this.#socket !== socket) return;
      this.#socket = undefined;
      socket.onclose = null;
      socket.close();
      failed(new GazelleError("timeout", `no hello from ${url} within ${this.#timeoutMs} ms`));
    }, this.#timeoutMs);
    socket.onmessage = (event) => {
      if (this.#socket !== socket) return;
      const frame = parseFrame(event.data);
      if (frame === undefined) return;
      if (frame["type"] === "hello" && !greeted) {
        greeted = true;
        this.#timers.clearTimeout(this.#helloTimer);
        this.#hello(frame, first === undefined);
        this.#attempt = 0;
        this.#setStatus("open");
        first?.resolve();
      } else if (greeted) {
        this.#frame(frame);
      }
    };
    socket.onclose = () => {
      if (this.#socket !== socket) return;
      this.#socket = undefined;
      this.#timers.clearTimeout(this.#helloTimer);
      if (greeted) this.#lost();
      else failed(new GazelleError("closed", `the connection to ${url} closed before hello`));
    };
    socket.onerror = () => {
      // A close event follows; that is where failure is handled.
    };
  }

  #lost(): void {
    this.#failAll(new GazelleError("closed", "the connection closed before a reply"));
    if (this.#status === "closed") return;
    this.#setStatus("reconnecting");
    this.#scheduleReconnect();
  }

  #scheduleReconnect(): void {
    if (this.#status === "closed") return;
    const ceiling = Math.min(this.#maxMs, this.#initialMs * 2 ** Math.min(this.#attempt, 30));
    const delay = ceiling / 2 + this.#random() * (ceiling / 2);
    this.#attempt += 1;
    this.#reconnectTimer = this.#timers.setTimeout(() => {
      this.#reconnectTimer = undefined;
      this.#dial();
    }, delay);
  }

  /** Nothing is replayed: every waiting call fails, and the caller decides what to resend. */
  #failAll(error: GazelleError): void {
    for (const pending of this.#pending.values()) {
      this.#timers.clearTimeout(pending.timer);
      pending.call.reject(error);
    }
    this.#pending.clear();
    for (const slot of this.#slots.values()) slot.queued?.reject(error);
    this.#slots.clear();
  }

  #hello(frame: Frame, reconnect: boolean): void {
    this.#server = {
      version: String(frame["version"] ?? ""),
      backend: String(frame["backend"] ?? ""),
      dry_run: frame["dry_run"] === true,
      // Each notice is `{code, message}`; only the message is shown, and anything else is skipped
      // rather than rendered as `[object Object]`.
      notices: (Array.isArray(frame["notices"]) ? frame["notices"] : [])
        .filter((notice): notice is { message: string } => isObject(notice) && typeof notice["message"] === "string")
        .map((notice) => notice.message),
    };
    const next = new Map<string, DeviceDescriptor>();
    for (const device of Array.isArray(frame["devices"]) ? frame["devices"] : []) {
      if (isObject(device) && typeof device["id"] === "string") next.set(device["id"], device as unknown as DeviceDescriptor);
    }
    const removed = [...this.#devices.keys()].filter((id) => !next.has(id));
    const added = [...next.values()].filter((d) => !this.#devices.has(d.id));
    this.#devices.clear();
    for (const [id, device] of next) this.#devices.set(id, device);
    if (reconnect) {
      for (const id of removed) this.#emit("device_removed", id);
      for (const device of added) this.#emit("device_added", device);
    }
  }

  #frame(frame: Frame): void {
    switch (frame["type"]) {
      case "rpc_response":
        this.#settle(frame["id"], undefined, frame["result"]);
        return;
      case "rpc_error": {
        const error = isObject(frame["error"]) ? frame["error"] : {};
        const code = typeof error["code"] === "string" ? error["code"] : "protocol_error";
        const message = typeof error["message"] === "string" ? error["message"] : "the server reported an error";
        this.#settle(frame["id"], new GazelleError(code, message, error["detail"]));
        return;
      }
      case "device_added":
        if (isObject(frame["device"]) && typeof frame["device"]["id"] === "string") {
          const device = frame["device"] as unknown as DeviceDescriptor;
          this.#devices.set(device.id, device);
          this.#emit("device_added", device);
        }
        return;
      case "device_removed":
        if (typeof frame["device_id"] === "string") {
          this.#devices.delete(frame["device_id"]);
          this.#emit("device_removed", frame["device_id"]);
        }
        return;
      case "lagged":
        this.#emit("lagged", typeof frame["missed"] === "number" ? frame["missed"] : 0);
        return;
      case "cyclic":
        this.#cyclicFrame(frame);
        return;
    }
  }

  #invoke(device: DeviceDescriptor, name: string, args: Record<string, unknown> | undefined, options: InvokeOptions = {}): Promise<InvokeResult<unknown>> {
    if (this.#status !== "open") return Promise.reject(new GazelleError("not_connected", `cannot invoke ${name}: the client is ${this.#status}`));
    const descriptor = device.family === null ? undefined : familySchema(device.family).commands[name];
    return new Promise((resolve, reject) => {
      const call: Call = {
        deviceId: device.id,
        command: name,
        args: encodeArgs(descriptor?.params ?? [], args),
        dryRun: options.dryRun,
        ext3: options.ext3,
        timeoutMs: options.timeoutMs ?? this.#timeoutMs,
        returns: descriptor?.returns ?? null,
        resolve,
        reject,
      };
      const key = options.coalesce;
      if (key === undefined) return this.#send(call, undefined);
      const slot = this.#slots.get(key);
      if (slot === undefined) {
        this.#slots.set(key, { queued: undefined });
        return this.#send(call, key);
      }
      slot.queued?.reject(new GazelleError("superseded", `${slot.queued.command} was superseded by a newer call with coalesce key "${key}"`));
      slot.queued = call;
    });
  }

  #send(call: Call, coalesce: string | undefined): void {
    const id = ++this.#nextId;
    const frame: Frame = { id, device_id: call.deviceId, command: call.command };
    if (call.args !== undefined) frame["args"] = call.args;
    if (call.dryRun !== undefined) frame["dry_run"] = call.dryRun;
    if (call.ext3 !== undefined) frame["ext3"] = call.ext3;
    const timer = this.#timers.setTimeout(() => this.#settle(id, new GazelleError("timeout", `${call.command} got no reply within ${call.timeoutMs} ms`)), call.timeoutMs);
    this.#pending.set(id, { call, timer, coalesce });
    try {
      if (this.#socket === undefined) throw new Error("no open socket");
      this.#socket.send(JSON.stringify(frame));
    } catch (e) {
      this.#settle(id, new GazelleError("closed", `could not send ${call.command}: ${String(e)}`));
    }
  }

  #settle(id: unknown, error: GazelleError | undefined, result?: unknown): void {
    if (typeof id !== "number") return;
    const pending = this.#pending.get(id);
    if (pending === undefined) return;
    this.#pending.delete(id);
    this.#timers.clearTimeout(pending.timer);
    if (error !== undefined) {
      pending.call.reject(error);
    } else {
      const body = isObject(result) ? result : {};
      const raw = body["response"];
      const response = pending.call.returns !== null && isObject(raw) ? decodeFields(pending.call.returns, raw) : (raw ?? null);
      pending.call.resolve({ ...body, response } as InvokeResult<unknown>);
    }
    if (pending.coalesce !== undefined) this.#release(pending.coalesce);
  }

  /** The call in flight for `key` has settled: send the one waiting behind it, if any. */
  #release(key: string): void {
    const slot = this.#slots.get(key);
    if (slot === undefined) return;
    const next = slot.queued;
    if (next === undefined) {
      this.#slots.delete(key);
      return;
    }
    slot.queued = undefined;
    if (this.#status !== "open") {
      this.#slots.delete(key);
      next.reject(new GazelleError("not_connected", `cannot invoke ${next.command}: the client is ${this.#status}`));
      return;
    }
    this.#send(next, key);
  }

  #onCyclic(deviceId: string, reportId: string, listener: (fields: unknown) => void): () => void {
    const set = this.#cyclic.get(deviceId) ?? new Set();
    this.#cyclic.set(deviceId, set);
    const entry = { reportId: reportKey(reportId), listener };
    set.add(entry);
    return () => set.delete(entry);
  }

  #cyclicFrame(frame: Frame): void {
    const deviceId = frame["device_id"];
    const reportId = frame["report_id"];
    const values = frame["fields"];
    if (typeof deviceId !== "string" || typeof reportId !== "string" || !isObject(values)) return;
    const listeners = this.#cyclic.get(deviceId);
    if (listeners === undefined || listeners.size === 0) return;
    const key = reportKey(reportId);
    const family = this.#devices.get(deviceId)?.family;
    const layout = family === undefined || family === null ? undefined : familySchema(family).cyclic[key];
    const fields = layout === undefined ? values : decodeFields(layout, values);
    for (const entry of [...listeners]) if (entry.reportId === key) entry.listener(fields);
  }

  /** A JSON request to `API_PATH/<path>`; failures become GazelleError. */
  async #http(method: "GET" | "PUT" | "POST" | "PATCH" | "DELETE", path: string, body?: unknown): Promise<unknown> {
    const url = new URL(`${API_PATH}/${path}`, this.#base).toString();
    let response: Awaited<ReturnType<FetchLike>>;
    try {
      response = await this.#fetch(url, body === undefined ? { method } : { method, headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
    } catch (e) {
      throw new GazelleError("not_connected", `${method} ${url} failed: ${String(e)}`);
    }
    let payload: unknown;
    try {
      payload = await response.json();
    } catch {
      payload = undefined;
    }
    if (!response.ok) {
      const error = isObject(payload) && isObject(payload["error"]) ? payload["error"] : {};
      const code = typeof error["code"] === "string" ? error["code"] : `http_${response.status}`;
      const message = typeof error["message"] === "string" ? error["message"] : `${method} ${url} returned HTTP ${response.status}`;
      throw new GazelleError(code, message, error["detail"]);
    }
    return payload;
  }
}
