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

/**
 * How the device this entry names was settled on: the workspace pinned it, the server worked it
 * out from the model and what is connected, or it could not be told at all.
 */
export type AggregateMatchedBy = "chosen" | "worked_out" | "none";

/**
 * One interface's channels in the aggregate, which are its USB audio channels: aggregate input k is
 * its USB record channel k, and aggregate output k its USB playback channel k. The counts are those
 * groups' own, from the model's topology, so they are exact and known without a DAW.
 */
export interface AggregateUsbChannels {
  inputs: number;
  outputs: number;
  /** What Gazelle calls the group its inputs are: "USB A REC" on the Quadro, "USB REC" on the Studio+. */
  input_group: string;
  /** What Gazelle calls the group its outputs are: "USB 1 PLAY", "USB PLAY". */
  output_group: string;
}

/** One configured device, with everything known about it now. */
export interface AggregateDeviceReport {
  /** Its place in the setup, from zero. Older servers leave it out. */
  index?: number;
  /**
   * Gazelle's name for the device: the person's own name for it, else its model. It is the name the
   * driver is given, so the driver's own record and log call it the same.
   */
  name: string;
  /**
   * What the driver is given for it, and so what the driver's record, its log and a measurement
   * call it: the person's own name for the device, else its model's short form. Older servers
   * leave it out.
   */
  daw_name?: string;
  key?: string;
  clsid?: string;
  /** Whether a driver on this PC registers it. */
  registered: boolean;
  /** The registry key that matched. */
  entry_key?: string;
  /** The device this entry resolved to, pinned by the workspace or worked out by the server. */
  device_id?: string;
  /** How `device_id` was arrived at. Older servers leave it out. */
  matched_by?: AggregateMatchedBy;
  /** Why it could not be told which device this is, and what would settle it. Only with `none`. */
  match_note?: string;
  /** Its channels in the aggregate, when its model is known. */
  channels?: AggregateUsbChannels;
  /** Whether that device is connected to Gazelle now, which is what makes the readings possible. */
  attached: boolean;
  family?: "quadro" | "studio";
  clock?: AggregateClock;
  driver: AggregateDriverSummary;
  controller?: UsbController;
  controller_error?: string;
  /** Whether it is the device that drives the callback. */
  is_master: boolean;
  /** Whether the setup says where to measure this interface's capture phase. Older servers leave it out. */
  phase_configured?: boolean;
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
  | "device_not_matched"
  | "driver_unreadable"
  | "one_usb_controller"
  | "controller_unknown"
  | "rates_differ"
  | "buffers_differ"
  | "no_cable"
  | "clock_not_cabled"
  | "not_locked"
  | "phase_not_measured";

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
  /** The configured device this is about, by Gazelle's name for it. */
  device?: string;
  /** That device's place in the setup, from zero. Older servers leave it out. */
  device_index?: number;
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

/**
 * What a session made of where one interface's capture actually started.
 *
 * `not_configured`: nothing is set up to measure it. `measuring`: the measurement is in flight.
 * `applied`: lined up to the phase its trim was measured at. `no_reference`: measured, with no such
 * phase to line it up to yet. `measured_only`: a calibration run, which measures and moves nothing.
 * `not_heard`, `off_the_grid` and `too_far`: a measurement the driver would not use, so the session
 * ran on the figures the drivers reported.
 */
export type AggregatePhaseState =
  | "not_configured"
  | "measuring"
  | "applied"
  | "not_heard"
  | "off_the_grid"
  | "too_far"
  | "no_reference"
  | "measured_only";

/** One sub-device, as the driver reports it now. */
export interface AggregateDeviceStatus {
  name: string;
  driver_name?: string;
  streaming: boolean;
  /** Its inputs read as silence and its outputs are muted until it calls back again. */
  stalled: boolean;
  is_master: boolean;
  /** How many of this device's channels the driver published for the session it is running. */
  inputs?: number;
  outputs?: number;
  /** This device's sample count minus the master's. Growing in one direction is two clocks. */
  sample_gap: number;
  callbacks: number;
  /** Blocks thrown away because its ring was full. */
  dropped: number;
  /** Blocks that were not there when they were wanted, which a person hears as a click. */
  starved: number;
  /**
   * What this session made of the interface's capture phase. Not the trim: the trim is a constant,
   * and this is measured again at the start of every session. Older drivers leave it out, and a
   * word this page does not know is shown as it came.
   */
  phase?: AggregatePhaseState | (string & {});
  /** What the phase measurement came to, in samples. Only worth reading once `phase` says something was measured. */
  phase_measured?: number;
  /** What was added to its input path because of it, in samples. Zero unless `phase` is `applied`. */
  phase_applied?: number;
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
  /**
   * One word: `refused`, `stalled`, `recovered`, `glitched`, `phase`, `session-started`,
   * `session-ended`, `adopted` or `reset-asked`. A line written by one of Gazelle's own measurements
   * starts its message with `Gazelle's own measurement:`.
   */
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

// ---------------------------------------------------------------------------------------------
// Lining the interfaces up: the measurement, and what it measured
// ---------------------------------------------------------------------------------------------

/**
 * Which way round a run is measuring.
 *
 * `inputs` plays one click out of one interface, into an input of every interface at once, so what
 * it measures is how far apart the interfaces record. `outputs` is the same thing the other way
 * round: one output on each interface, all cabled into inputs of one interface.
 */
export type AggregateCalibrateDirection = "inputs" | "outputs";

/**
 * A clock that is not the same clock, found by the click positions moving across a run rather than
 * sitting still. `real` is the server's own judgement of whether it is more than the measurement's
 * own noise; no trim can fix a real one.
 */
export interface AggregateDrift {
  samples_per_second: number;
  ppm: number;
  real: boolean;
}

/** One interface's reading: how far out it is, how steady that was, and how much it had to go on. */
export interface AggregateCalibrateReading {
  /** The interface, by the name the setup gives it. */
  device: string;
  /** Whether this is the interface everything else was measured against, which is zero by definition. */
  is_reference: boolean;
  /** How far this interface is from the reference, in samples. Positive is late. */
  lag_samples: number;
  /** How far the clicks disagreed with each other, in samples. Small is a measurement to trust. */
  spread_samples: number;
  /** The widest they could have disagreed and still be one measurement. Past it, the reading was thrown out. */
  spread_limit_samples?: number;
  clicks_found: number;
  /** How many were played, when the server says. */
  clicks_expected?: number;
  /** Blocks this interface's audio lost while the run was going: thrown away, and not there in time. */
  blocks_dropped?: number;
  blocks_starved?: number;
  /** The server's own sentence for this interface, whether it went well or not. */
  note?: string;
  drift?: AggregateDrift;
}

/**
 * An extra input channel a run listened in on. It is an observation, never an interface's
 * measurement, and it changes no trim. `channel` is that interface's own input number from zero,
 * exactly as it was asked for.
 */
export interface AggregateCalibrateWitness {
  channel: number;
  /** The interface that channel belongs to, by the name the setup gives it. */
  device: string;
  lag_samples: number;
  spread_samples: number;
  spread_limit_samples?: number;
  clicks_found: number;
  clicks_expected?: number;
  blocks_dropped?: number;
  blocks_starved?: number;
  note?: string;
  drift?: AggregateDrift;
}

/**
 * What the driver made of one interface's capture phase at the start of a run. A measurement run
 * measures it and moves nothing (`measured_only`), on purpose: it is where the reference comes from.
 */
export interface AggregateCalibratePhase {
  device: string;
  state: AggregatePhaseState | (string & {});
  measured_samples: number;
  applied_samples: number;
  note?: string;
}

/**
 * The phase a trim was measured at, written with the trim into that interface's `phase.reference`.
 * `now` null means nothing was heard on the cable, so writing the trim takes the old reference out.
 */
export interface AggregateTrimReference {
  was: number | null;
  now: number | null;
}

/** One trim the measurement implies: what it is now, what was measured, and what it would become. */
export interface AggregateCalibrateTrim {
  device: string;
  direction: AggregateCalibrateDirection;
  /** The field in the setup this changes: `input_trim` or `output_trim`. */
  field?: string;
  was: number;
  measured: number;
  now: number;
  is_reference?: boolean;
  /** Why this one is not offered, when it is not. */
  not_applied?: string;
  /**
   * The phase reference written beside this trim. Present only on an input trim for an interface
   * whose phase the driver measures; never on the reference interface or on an output trim.
   */
  phase_reference?: AggregateTrimReference;
}

/** What a finished run came to. */
export interface AggregateCalibrateOutcome {
  direction: AggregateCalibrateDirection;
  /** The rate it ran at, in Hz, and the buffer size, in samples: a trim is only true for these. */
  rate: number;
  buffer_size: number;
  /** The interface everything was measured against, by the name the setup gives it. */
  reference: string;
  readings: AggregateCalibrateReading[];
  /** Extra channels the run listened in on. Older servers leave it out. */
  witnesses?: AggregateCalibrateWitness[];
  trims: AggregateCalibrateTrim[];
  /** One per interface: what the driver's phase measurement came to at the start of the run. */
  phases?: AggregateCalibratePhase[];
  /** True when no interface lost a block while the run was going. Older servers leave it out. */
  clean?: boolean;
  /** How many blocks the run lost altogether, across every interface. */
  blocks_lost?: number;
  warnings: string[];
  /** True for a check: lined up as a DAW's session is, so its lags are what a recording would get, and it offers no trims. */
  checking?: boolean;
}

/**
 * The one run at a time, as `GET /aggregate/calibrate` answers it. `step` and `progress` are there
 * only while it runs, `outcome` only when it is done, and `refusal` only when it failed.
 */
export interface AggregateCalibration {
  state: "idle" | "running" | "done" | "failed";
  started_at_ms?: number;
  step?: string;
  /** How far along, from 0 to 1. */
  progress?: number;
  refusal?: string;
  outcome?: AggregateCalibrateOutcome;
}

/**
 * One channel of one interface, as a run is asked for it: `device` is the interface's place in the
 * setup and `channel` is that interface's own channel number, both from zero. It is never a number
 * in the aggregate's own list, which depends on how many channels each driver really has; the run
 * opens the drivers and works out where each one is, and refuses one it cannot place.
 */
export interface AggregateCalibrateChannel {
  device: number;
  channel: number;
}

/**
 * What starting a run takes: one output and one input per interface, in the order the interfaces
 * are in, so `outputs[n]` is cabled into `inputs[n]`. Every entry names its own interface.
 */
export interface AggregateCalibrateRequest {
  direction: AggregateCalibrateDirection;
  outputs: AggregateCalibrateChannel[];
  inputs: AggregateCalibrateChannel[];
  /** Extra inputs to record and report, which take no part in any trim. Left out means none. */
  witnesses?: AggregateCalibrateChannel[];
  clicks: number;
  /** How loud the click is, in dBFS. Modest: it comes out of a real output. */
  level_dbfs: number;
  /**
   * A check rather than a measurement: the session is lined up as a DAW's would be, and what it
   * hears says whether the trims hold. Left out means a measurement, which is what offers trims.
   */
  check?: boolean;
}

/** What starting a run answers. A run it will not start is a refusal rather than this. */
export interface AggregateCalibrateStarted {
  started: boolean;
}

/** What stopping answers: whether anything was going. Stopping nothing is not an error. */
export interface AggregateCalibrateStopped {
  stopped: boolean;
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
