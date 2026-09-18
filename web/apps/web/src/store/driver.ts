// The audio driver's settings as the Devices page shows them: buffer size, latency and Safe Mode,
// which belong to the driver on the PC, not to the device. The buffer size and Safe Mode can be
// changed from the page, through the driver's one setter; what the driver reports afterwards is
// what the page then shows.

import type { DriverChange, DriverReading, DriverReport, DriverWriteReport } from "gazelle-audio-client";

export type { DriverChange, DriverReport, DriverWriteReport };

/** The last change asked of a device's driver: on its way, answered, or refused with nothing sent. */
export type DriverWriteState =
  | { state: "sending"; change: DriverChange }
  | { state: "done"; report: DriverWriteReport }
  | { state: "refused"; code: string; message: string; change: DriverChange };

/** What the Driver section's controls need, or `undefined` when there is no reading to change from. */
export interface DriverControls {
  /** The sizes the driver offers, smallest first. */
  sizes: number[];
  buffer: number;
  safeMode: boolean;
  /** Programs using the driver's ASIO interface now. */
  asioClients: number;
}

export function driverControls(report: DriverReport | undefined): DriverControls | undefined {
  if (report === undefined || report.state !== "read" || report.asio.state !== "read") return undefined;
  const { buffer_sizes, buffer_size, safe_mode, asio_clients } = report.asio.value;
  return { sizes: buffer_sizes, buffer: buffer_size, safeMode: safe_mode, asioClients: asio_clients };
}

/** The line that names programs using ASIO, before anything is tried; none when there are none. */
export function asioInUseText(clients: number): string | undefined {
  if (clients <= 0) return undefined;
  const [who, they] = clients === 1 ? ["1 program is", "it is"] : [`${clients} programs are`, "they are"];
  return `${who} using the driver's ASIO interface now (a DAW, most likely). A change is refused while ${they}, unless you choose Change anyway.`;
}

const OUTCOME_LEAD: Record<DriverWriteReport["outcome"], [lead: string, problem: boolean]> = {
  applied: ["Changed.", false],
  unchanged: ["Unchanged.", false],
  dry_run: ["Not sent (dry run).", false],
  mismatch: ["Not as sent.", true],
  unconfirmed: ["Not confirmed.", true],
  failed: ["Not changed.", true],
};

/**
 * The result line under the controls. A change that went through carries the latencies the driver
 * reports now, which is what a buffer or Safe Mode change is for; one that did not says so first.
 */
export function writeText(write: DriverWriteState): { text: string; problem: boolean } {
  if (write.state === "sending") return { text: "Sending to the driver...", problem: false };
  if (write.state === "refused") return { text: `Not changed. ${write.message}`, problem: true };
  const [lead, problem] = OUTCOME_LEAD[write.report.outcome];
  const back = write.report.read_back;
  const latencies =
    write.report.outcome === "applied" && back.state === "read" && back.asio.state === "read"
      ? ` Input latency ${latencyText(back.asio.value.input_latency, back.asio.value.sample_rate)}, output latency ${latencyText(back.asio.value.output_latency, back.asio.value.sample_rate)}.`
      : "";
  return { text: `${lead} ${write.report.message}${latencies}`, problem };
}

/** One line of the Driver section. `unread` marks a value the driver would not give. */
export interface DriverRow {
  label: string;
  value: string;
  unread: boolean;
  /** For tests: which value this is. */
  field: string;
}

/**
 * A latency as the vendor's panel shows it: "571 samples (12.95 ms)". The panel counts whole
 * microseconds, then rounds to hundredths of a millisecond with a half going to the even digit:
 * that is the one rule its four values on the user's devices all fit (585 samples at 44.1 kHz is
 * 13265 us, which it shows as 13.26, where ordinary rounding gives 13.27). Whether it truncates or
 * rounds the microseconds first, those four values cannot tell; truncation is assumed.
 */
export const latencyText = (samples: number, rate: number): string => {
  const micros = Math.floor((samples * 1_000_000) / rate);
  const rest = micros % 10;
  let hundredths = (micros - rest) / 10;
  if (rest > 5 || (rest === 5 && hundredths % 2 === 1)) hundredths += 1;
  return `${samples} samples (${Math.floor(hundredths / 100)}.${String(hundredths % 100).padStart(2, "0")} ms)`;
};

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
