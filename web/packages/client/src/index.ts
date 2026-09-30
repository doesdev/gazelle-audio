export { API_PATH, CLOSE_UNPAIRED, DEFAULT_TIMEOUT_MS, RECONNECT_INITIAL_MS, RECONNECT_MAX_MS, connect } from "./client.ts";
export type {
  Client,
  ClientEvents,
  ConnectOptions,
  DeviceDescriptor,
  DeviceHandle,
  FetchLike,
  InvokeArgs,
  InvokeOptions,
  InvokeResult,
  ServerInfo,
  SocketFactory,
  SocketLike,
  Status,
  Timers,
  TypedDevice,
  UntypedDevice,
  UserTheme,
} from "./client.ts";
export type { AsioInstance, DriverChange, DriverReading, DriverReport, DriverSettings, DriverSetterCall, DriverUnread, DriverWriteReport } from "./driver.ts";
export type { AggregateAnswer, AggregateCalibrateChannel, AggregateCalibrateDirection, AggregateCalibrateOutcome, AggregateCalibratePhase, AggregateCalibrateReading, AggregateCalibrateRequest, AggregateCalibrateStarted, AggregateCalibrateStopped, AggregateCalibrateTrim, AggregateCalibrateWitness, AggregateCalibration, AggregateClock, AggregateDeviceOutcome, AggregateDeviceReport, AggregateDeviceStatus, AggregateDriverSummary, AggregateDrift, AggregateEvent, AggregateFix, AggregateMatchBuffers, AggregateMatchedBy, AggregatePhaseState, AggregatePlan, AggregateRateInForce, AggregateReason, AggregateReasonCode, AggregateRegistration, AggregateRoutingWrite, AggregateRegistrationRun, AggregateStatus, AggregateStatusReading, AggregateTrimReference, AggregateUsbChannels, AsioEntry, DllSearch, ElevatedRun, UsbController } from "./aggregate.ts";
export type { MetronomeSettings, MetronomeSound, MetronomeStatus, MetronomeSubdivision, RecordingAutoArm, RecordingAutoArmPhase, RecordingChannelLevel, RecordingDropouts, RecordingPreroll, RecordingSeedCheck, RecordingSettings, RecordingState, RecordingStatus, RecordingTake, RecordingTakeLive, RecordingWindows } from "./recording.ts";
export type { UpdateRestart, UpdateState, UpdateStatus } from "./update.ts";
export { pairPhone } from "./remote.ts";
export type { PairFetch, RemoteAddress, RemotePaired, RemotePairing, RemotePhone, RemoteQr, RemoteStatus } from "./remote.ts";
export { GazelleError } from "./errors.ts";
export type { ClientErrorCode, ErrorCode, ServerErrorCode } from "./errors.ts";
export { fromHex, toHex } from "./bytes.ts";
export type { Bytes, CommandDescriptor, FamilySchema, FieldDescriptor, Scalar, Topology, TopologyGroup } from "./schema.ts";
export { schemas, topologies } from "./generated/index.ts";
export type { Family, FamilyTypes } from "./generated/index.ts";
export { SNAPSHOT_VERSION } from "./snapshots.ts";
export type { Change, ChangeKind, DeviceDiff, DeviceSnapshot, RecallAsk, RecallDevicePlan, RecallExcluded, RecallPart, RecallPlan, RecallRaisedOutput, RecallStep, SectionDiff, Snapshot, SnapshotDeviceSummary, SnapshotDiff, SnapshotImport, SnapshotSummary, Unreadable } from "./snapshots.ts";
export type { Recording, RecordingChannel, RecordingFormat, RecordingPreset } from "./workspace.ts";
export type { Aggregate, AggregateDevice, AggregateKnown, AggregatePhaseSetting, ChannelRef, DeviceMixer, Group, InputRef, Link, LinkKind, MixConfig, MixerChannel, MixerGroup, RouteSource, SavedLayout, Cable, CableDedication, CableEnd, ControlRoom, DigitalPort, Surface, SurfaceStrip, SurfaceStripKind, Workspace } from "./workspace.ts";
