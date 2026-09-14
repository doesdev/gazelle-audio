export { API_PATH, DEFAULT_TIMEOUT_MS, RECONNECT_INITIAL_MS, RECONNECT_MAX_MS, connect } from "./client.ts";
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
export { GazelleError } from "./errors.ts";
export type { ClientErrorCode, ErrorCode, ServerErrorCode } from "./errors.ts";
export { fromHex, toHex } from "./bytes.ts";
export type { Bytes, CommandDescriptor, FamilySchema, FieldDescriptor, Scalar } from "./schema.ts";
export { schemas } from "./generated/index.ts";
export type { Family, FamilyTypes } from "./generated/index.ts";
export type { ChannelRef, Group, Link, Workspace } from "./workspace.ts";
