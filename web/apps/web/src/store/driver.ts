// The audio driver's settings as the Devices page shows them: buffer size, latency and Safe Mode,
// which belong to the driver on the PC, not to the device. Read only; changing them is a later step.

import type { DriverReading, DriverReport } from "gazelle-audio-client";

export type { DriverReport };

/** One line of the Driver section. `unread` marks a value the driver would not give. */
export interface DriverRow {
  label: string;
  value: string;
  unread: boolean;
  /** For tests: which value this is. */
  field: string;
}

/** A latency as the vendor's panel shows it: "571 samples (12.95 ms)". */
export const latencyText = (samples: number, rate: number): string => `${samples} samples (${((samples / rate) * 1000).toFixed(2)} ms)`;

const kHz = (rate: number) => `${Number((rate / 1000).toFixed(3))} kHz`;

/** The rows to show, or, when there is no reading at all, why. */
export function driverView(report: DriverReport): { rows: DriverRow[]; message?: string } {
  if (report.state !== "read") return { rows: [], message: report.message };
  const row = <T>(field: string, label: string, reading: DriverReading<T>, show: (value: T) => string): DriverRow =>
    reading.state === "read" ? { field, label, value: show(reading.value), unread: false } : { field, label, value: reading.message, unread: true };
  const api = report.api_known ? `API ${report.api_version}` : `API ${report.api_version}, not one Gazelle was checked against`;
  const rows = [row("driver_version", "Driver version", report.driver_version, (version) => `${version} (${api})`), row("sample_rate", "Sample rate", report.sample_rate, kHz)];
  const asio = report.asio;
  if (asio.state === "read") {
    const { buffer_size, input_latency, output_latency, sample_rate } = asio.value;
    rows.push(
      { field: "buffer_size", label: "Buffer size", value: `${buffer_size} samples`, unread: false },
      { field: "input_latency", label: "Input latency", value: latencyText(input_latency, sample_rate), unread: false },
      { field: "output_latency", label: "Output latency", value: latencyText(output_latency, sample_rate), unread: false },
    );
  } else {
    rows.push({ field: "buffer_size", label: "Buffer size", value: asio.message, unread: true });
  }
  rows.push(row("safe_mode", "Safe Mode", report.safe_mode, (on) => (on ? "On" : "Off")));
  return { rows };
}
