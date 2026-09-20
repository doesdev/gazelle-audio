// The aggregate audio driver: one driver a DAW opens, with several interfaces underneath it
// (crates/gazelle-audio-server/src/aggregate). Its setup lives in the workspace, as `Aggregate`
// in workspace.ts, and Gazelle exports it to the one file the driver reads whenever it changes.
//
// `GET /aggregate` is everything a page needs in one answer: the drivers on this PC, whether ours
// is registered, each configured device live, the driver's own record and log, and a single ready
// or not ready with the reasons. A reason carries a code to switch on and, where Gazelle can put
// it right, a `fix` naming the route and the body to send. Keys stay snake_case as on the wire.
//
// These routes are served only on a loopback bind and only to a caller on the same machine, like
// the update routes: a page that shows them treats a rejection as "this server does not offer
// them" rather than as a failure.

import type { DriverWriteReport } from "./driver.ts";
import type { Aggregate } from "./workspace.ts";

/** One entry under the PC's audio driver registry. */
export interface AsioEntry {
  /** The key's own name, which is what a DAW shows in its driver list. */
  key: string;
  description?: string;
  clsid: string;
  /** The DLL the class id names. */
  dll?: string;
  /** Why it could not be read, when it could not. */
  dll_error?: string;
  /** Whether that DLL is a file now. */
  dll_present: boolean;
  /** Whether the aggregate's setup names this entry. */
  configured: boolean;
  /** Whether this is Gazelle's own aggregate driver. */
  is_aggregate: boolean;
}

/** The USB host controller an interface is on. Two interfaces on one cannot both stream. */
export interface UsbController {
  instance_id: string;
  description: string;
}

/** What a device says about its clock now, which is the only reading that means anything. */
export interface AggregateClock {
  /** `set_sync_source`'s index. */
  source_index: number;
  /** Its name on this model, when the model is known. */
  source?: string;
  locked: boolean;
  /** The rate the device reports, in Hz. */
  hz: number;
  /** `set_samp_rate`'s index, as the device reports it. */
  rate_index: number;
}

/** What the audio driver says about one device, as much of it as the aggregate cares about. */
export interface AggregateDriverSummary {
  sample_rate?: number;
  buffer_size?: number;
  safe_mode?: boolean;
  /** Nonzero while something holds the driver's interface; one DAW can count as several. */
  asio_clients?: number;
  /** Why there is less here than there could be. */
  message?: string;
}

/** One configured device, with everything known about it now. */
export interface AggregateDeviceReport {
  /** What the setup calls it, which is what its channels are named after. */
  name: string;
  key?: string;
  clsid?: string;
  /** Whether a driver on this PC registers it. */
  registered: boolean;
  /** The registry key that matched. */
  entry_key?: string;
  device_id?: string;
  /** Whether that device is connected to Gazelle now, which is what makes the readings possible. */
  attached: boolean;
  family?: "quadro" | "studio";
  clock?: AggregateClock;
  driver: AggregateDriverSummary;
  controller?: UsbController;
  controller_error?: string;
  /** Whether it is the device that drives the callback. */
  is_master: boolean;
}

/** Where the driver's DLL is, or every place that was looked in. */
export type DllSearch = { state: "found"; dll: string } | { state: "missing"; message: string; looked_in: string[] };

/** Whether Gazelle's own driver is registered, and what its class id points at. */
export interface AggregateRegistration {
  registered: boolean;
  clsid: string;
  name: string;
  dll?: string;
  /** Registered with this false is "registered, pointing at a copy that is not there any more". */
  dll_present: boolean;
  message: string;
  /** The command a person could run themselves, when a DLL to register was found. */
  register_command?: string;
  unregister_command?: string;
  dll_search: DllSearch;
}

/** Why the aggregate is not ready. Switch on this rather than reading the message. */
export type AggregateReasonCode =
  | "not_configured"
  | "device_missing"
  | "not_registered"
  | "dll_missing"
  | "device_not_attached"
  | "driver_unreadable"
  | "one_usb_controller"
  | "controller_unknown"
  | "rates_differ"
  | "buffers_differ"
  | "no_cable"
  | "clock_not_cabled"
  | "not_locked";

/** A request the page can make to put one reason right, already addressed and filled in. */
export interface AggregateFix {
  kind: "match_buffers" | "set_clock_source" | "set_sample_rate" | "register";
  method: "POST";
  /** Relative to the API root, as `Client` takes it. */
  route: string;
  body: unknown;
  /** What a button would say. */
  label: string;
}

export interface AggregateReason {
  code: AggregateReasonCode;
  /** `ready` is false only when there is at least one blocking reason. */
  severity: "blocking" | "warning";
  message: string;
  /** The configured device this is about, by the name the setup gives it. */
  device?: string;
  device_id?: string;
  fix?: AggregateFix;
}

/** The plan the driver is running, as it reports it. Counts are in samples. */
export interface AggregatePlan {
  master: string;
  rate: number;
  buffer_size: number;
  inputs: number;
  outputs: number;
  alignment: "aligned" | "lowest_latency";
  input_latency: number;
  output_latency: number;
}

/** One sub-device, as the driver reports it now. */
export interface AggregateDeviceStatus {
  name: string;
  driver_name?: string;
  streaming: boolean;
  /** Its inputs read as silence and its outputs are muted until it calls back again. */
  stalled: boolean;
  is_master: boolean;
  /** This device's sample count minus the master's. Growing in one direction is two clocks. */
  sample_gap: number;
  callbacks: number;
  /** Blocks thrown away because its ring was full. */
  dropped: number;
  /** Blocks that were not there when they were wanted, which a person hears as a click. */
  starved: number;
}

/** The driver's live record. */
export interface AggregateStatus {
  open: boolean;
  streaming: boolean;
  /** The generation Gazelle last asked for. */
  generation: number;
  /** The generation the driver is actually running. */
  generation_in_force: number;
  /** False while the driver has not taken up what Gazelle last asked for. */
  up_to_date: boolean;
  plan?: AggregatePlan;
  devices: AggregateDeviceStatus[];
  last_refusal?: string;
  config_source?: string;
}

/**
 * The status, or why there is none. `silent` is the ordinary case: the driver publishes only
 * while a DAW has it open, so nothing published is not a fault.
 */
export type AggregateStatusReading =
  | { state: "silent"; message: string }
  | { state: "unread"; message: string }
  | ({ state: "read" } & AggregateStatus);

/** One line of the driver's durable event log. */
export interface AggregateEvent {
  /** `YYYY-MM-DD HH:MM:SS`, local time, as the driver wrote it. */
  at: string;
  /** One word: `refused`, `stalled`, `recovered`, `session-started`, `session-ended`, and so on. */
  kind: string;
  message: string;
}

/** Everything `GET /aggregate` answers. */
export interface AggregateAnswer {
  read_at_ms: number;
  /** Whether the workspace names at least one interface for the aggregate. */
  configured: boolean;
  config?: Aggregate;
  /** Where the driver's own file is written. */
  export_path: string;
  drivers: AsioEntry[];
  drivers_error?: string;
  registration: AggregateRegistration;
  devices: AggregateDeviceReport[];
  ready: boolean;
  reasons: AggregateReason[];
  status: AggregateStatusReading;
  events: AggregateEvent[];
  events_error?: string;
}

/** What one device's buffer change came to. A refusal keeps the driver route's own codes. */
export interface AggregateDeviceOutcome {
  device: string;
  device_id?: string;
  result?: DriverWriteReport;
  error?: { code: string; message: string };
}

/** What matching buffers came to: each device on its own, changed or refused. */
export interface AggregateMatchBuffers {
  buffer_size: number;
  changed: number;
  refused: number;
  devices: AggregateDeviceOutcome[];
}

/** What running the elevated registrar came to. `started` false is a declined prompt. */
export interface ElevatedRun {
  started: boolean;
  exit_code?: number;
  message: string;
}

/** Registering or unregistering the driver. The command is given so a person can run it instead. */
export interface AggregateRegistrationRun {
  dll: string;
  command: string;
  run: ElevatedRun;
}
