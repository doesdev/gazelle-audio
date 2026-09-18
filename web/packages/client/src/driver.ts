// The audio driver's own settings for one device, as `GET /devices/{id}/driver` answers them
// (crates/gazelle-audio-server/src/driver). They belong to the driver on the PC, not to the device,
// and are read only: nothing here changes them. Keys stay snake_case as on the wire.

/** A value that was read, or why it was not. */
export type DriverReading<T> = { state: "read"; value: T } | { state: "unread"; message: string };

/** The driver's ASIO instance: every count in samples except the rate. */
export interface AsioInstance {
  sample_rate: number;
  buffer_size: number;
  input_latency: number;
  output_latency: number;
  buffer_sizes: number[];
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
