// The audio driver's own settings for one device, as `GET /devices/{id}/driver` answers them
// (crates/gazelle-audio-server/src/driver). They belong to the driver on the PC, not to the device.
// `PUT /devices/{id}/driver` changes the buffer size and Safe Mode through the driver's one setter
// and answers what the driver reports afterwards. Keys stay snake_case as on the wire.

/** A value that was read, or why it was not. */
export type DriverReading<T> = { state: "read"; value: T } | { state: "unread"; message: string };

/** The driver's ASIO instance: every count in samples except the rates. */
export interface AsioInstance {
  sample_rate: number;
  /** The rate the preferred buffer applies at: what a change is sent with. */
  reference_rate: number;
  buffer_size: number;
  input_latency: number;
  output_latency: number;
  buffer_sizes: number[];
  safe_mode: boolean;
  /** How many programs (a DAW) are using the driver's ASIO interface; a change is refused while any are, unless forced. */
  asio_clients: number;
}

export interface DriverSettings {
  state: "read";
  dll: string;
  service: string;
  api_version: string;
  /** Whether that API version is one the server was checked against. */
  api_known: boolean;
  driver_version: DriverReading<string>;
  sample_rate: DriverReading<number>;
  asio_instances: number | null;
  asio_instance: number;
  asio: DriverReading<AsioInstance>;
  safe_mode: DriverReading<boolean>;
}

/** No reading: the backend has no driver, none lists the device, or one could not be read. */
export interface DriverUnread {
  state: "no_driver" | "not_found" | "failed";
  message: string;
}

export type DriverReport = { device_id: string; read_at_ms: number; cached: boolean } & (DriverSettings | DriverUnread);

/** What to change; anything left out is sent as the driver has it now. */
export interface DriverChange {
  buffer_size?: number;
  safe_mode?: boolean;
  /** Change it even while a program is using the driver's ASIO interface; its audio restarts. */
  force?: boolean;
}

/** The driver's one setter as it was called, with the vendor's names for its arguments. */
export interface DriverSetterCall {
  asio_instance: number;
  reference_sample_rate: number;
  preferred_size: number;
  /** 0x10000 is Safe Mode. */
  options: number;
}

/**
 * A change's result. `applied`: the driver reports what was sent. `mismatch`: it reports something
 * else. `unconfirmed`: it could not be read back. `failed`: it answered an error status. `unchanged`:
 * it already had it, so nothing was sent. `dry_run`: built and not sent. A refusal (nothing sent) is
 * a `GazelleError` instead, with codes `not_offered`, `nothing_to_change`, `asio_in_use`,
 * `unavailable` or `unreadable`.
 */
export interface DriverWriteReport {
  device_id: string;
  outcome: "applied" | "mismatch" | "unconfirmed" | "failed" | "unchanged" | "dry_run";
  message: string;
  call: DriverSetterCall | null;
  read_back: DriverReport;
}
