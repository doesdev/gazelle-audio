import { test } from "node:test";
import assert from "node:assert/strict";

import { GazelleError, type DriverChange, type DriverReport, type DriverWriteReport } from "gazelle-audio-client";

import { asioInUseText, driverControls, driverView, latencyText, writeText } from "../src/store/driver.ts";
import { Store } from "../src/store/store.ts";
import { device, FakeClient, flush, MemoryStorage } from "./fake-client.ts";

const OFFERED = [8, 16, 32, 64, 128, 256, 512, 1024, 2048];

/** The Quadro as its driver answered on 2026-09-18. */
export const QUADRO_REPORT: DriverReport = {
  device_id: "serial:1000000000001",
  read_at_ms: 1_789_700_000_000,
  cached: false,
  state: "read",
  dll: "C:\\Program Files\\Antelope Audio\\Zen Quadro Synergy Core USB Audio Driver\\x64\\Zen_Quadro_Synergy_Coreapi_x64.dll",
  service: "Zen_Quadro_Synergy_Core",
  api_version: "5.12",
  api_known: true,
  driver_version: { state: "read", value: "5.68.0" },
  sample_rate: { state: "read", value: 44100 },
  asio_instances: 1,
  asio_instance: 0,
  asio: { state: "read", value: { sample_rate: 44100, reference_rate: 44100, buffer_size: 512, input_latency: 571, output_latency: 632, buffer_sizes: OFFERED, safe_mode: true, asio_clients: 0 } },
  safe_mode: { state: "read", value: true },
};

test("latency reads as the vendor panel shows it, in samples and milliseconds", () => {
  assert.equal(latencyText(571, 44100), "571 samples (12.95 ms)");
  // The vendor panel's own rounding, from its four values on the user's two devices (2026-09-18):
  // whole microseconds first, then hundredths of a millisecond with a half going to the even digit.
  // Plain rounding would give 13.27 for the Studio+'s 585 samples; the panel shows 13.26.
  assert.deepEqual(
    [[571, 44100], [632, 44100], [568, 44100], [585, 44100]].map(([samples, rate]) => latencyText(samples as number, rate as number)),
    ["571 samples (12.95 ms)", "632 samples (14.33 ms)", "568 samples (12.88 ms)", "585 samples (13.26 ms)"],
  );
  assert.equal(latencyText(587, 44100), "587 samples (13.31 ms)", "13310 us: no half, so the ordinary way");
  assert.equal(latencyText(632, 44100), "632 samples (14.33 ms)");
  assert.equal(latencyText(568, 44100), "568 samples (12.88 ms)");
  assert.equal(latencyText(512, 48000), "512 samples (10.67 ms)");
});

test("a reading becomes the rows the Driver section shows", () => {
  const view = driverView(QUADRO_REPORT);
  assert.equal(view.message, undefined);
  assert.deepEqual(
    view.rows.map(({ label, value }) => [label, value]),
    [
      ["Driver version", "5.68.0 (API 5.12)"],
      ["Sample rate", "44.1 kHz"],
      ["Buffer size", "512 samples"],
      ["Input latency", "571 samples (12.95 ms)"],
      ["Output latency", "632 samples (14.33 ms)"],
      ["Safe Mode", "On"],
    ],
  );
  assert.ok(view.rows.every((row) => !row.unread));
});

test("a value that could not be read says why, in its own row, and the others still show", () => {
  const report: DriverReport = { ...QUADRO_REPORT, asio: { state: "unread", message: "Could not be read: its buffer size (3) is not among the sizes it offers." }, safe_mode: { state: "read", value: false } } as DriverReport;
  const rows = driverView(report).rows;
  const buffer = rows.find((row) => row.label === "Buffer size");
  assert.deepEqual([buffer?.value, buffer?.unread], ["Could not be read: its buffer size (3) is not among the sizes it offers.", true]);
  assert.equal(rows.find((row) => row.label === "Input latency"), undefined, "one message for the three values it carries");
  assert.equal(rows.find((row) => row.label === "Safe Mode")?.value, "Off");
  assert.equal(rows.find((row) => row.label === "Sample rate")?.value, "44.1 kHz");
});

test("an API version the server has not seen is marked", () => {
  const rows = driverView({ ...QUADRO_REPORT, api_version: "5.30", api_known: false } as DriverReport).rows;
  assert.equal(rows[0]?.value, "5.68.0 (API 5.30, not one Gazelle was checked against)");
});

test("no reading is a message and no rows", () => {
  const view = driverView({ device_id: "loopback-0", read_at_ms: 1, cached: false, state: "no_driver", message: "The loopback backend has no audio driver on this PC." });
  assert.deepEqual(view, { rows: [], message: "The loopback backend has no audio driver on this PC." });
});

test("the store reads a device's driver on demand, and a failed request becomes a message", async () => {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"));
  const asked: { id: string; refresh: boolean }[] = [];
  client.driver = async (id, options) => {
    asked.push({ id, refresh: options?.refresh === true });
    return QUADRO_REPORT;
  };
  const store = new Store(client, { storage: new MemoryStorage() });
  await store.start();
  assert.equal(store.driver("loopback-0").value, undefined, "nothing until asked");
  await store.loadDriver("loopback-0");
  assert.equal(store.driver("loopback-0").value, QUADRO_REPORT);
  await store.loadDriver("loopback-0", true);
  assert.deepEqual(asked, [{ id: "loopback-0", refresh: false }, { id: "loopback-0", refresh: true }]);

  client.driver = async () => {
    throw new GazelleError("not_connected", "GET failed: ECONNREFUSED");
  };
  await store.loadDriver("loopback-0");
  const failed = store.driver("loopback-0").value;
  assert.deepEqual([failed?.state, failed?.state === "read" ? "" : failed?.message], ["failed", "The driver's settings could not be asked for: GET failed: ECONNREFUSED"]);
  await flush();
});

/** The Quadro after a change to 256 samples with Safe Mode off, as its driver read back on 2026-09-18. */
const QUADRO_256_OFF: DriverReport = {
  ...QUADRO_REPORT,
  asio: { state: "read", value: { sample_rate: 44100, reference_rate: 44100, buffer_size: 256, input_latency: 315, output_latency: 191, buffer_sizes: OFFERED, safe_mode: false, asio_clients: 0 } },
  safe_mode: { state: "read", value: false },
} as DriverReport;

test("the controls come from a reading: the offered sizes, the buffer, Safe Mode and who is using ASIO", () => {
  assert.deepEqual(driverControls(QUADRO_REPORT), { sizes: OFFERED, buffer: 512, safeMode: true, asioClients: 0 });
  assert.equal(driverControls({ ...QUADRO_REPORT, asio: { state: "unread", message: "Could not be read." } } as DriverReport), undefined, "nothing to change from without a reading");
  assert.equal(driverControls({ device_id: "loopback-0", read_at_ms: 1, cached: false, state: "no_driver", message: "m" }), undefined);
});

test("ASIO in use is said without a count, since one DAW counts as several clients", () => {
  // Seen 2026-09-18: one DAW recording on the Quadro made the driver count 4.
  const text = "The driver's ASIO interface is in use now (by a DAW, most likely). A change is refused while it is, unless you choose Change anyway.";
  assert.equal(asioInUseText(0), undefined);
  assert.equal(asioInUseText(1), text);
  assert.equal(asioInUseText(4), text);
});

test("a write's result reads plainly, and a mismatch or failure is not dressed up as success", () => {
  const report = (outcome: DriverWriteReport["outcome"], message: string): DriverWriteReport => ({ device_id: "d", outcome, message, call: null, read_back: QUADRO_256_OFF });
  assert.deepEqual(writeText({ state: "done", report: report("applied", "The driver now reports a buffer of 256 samples with Safe Mode off.") }), {
    text: "Changed. The driver now reports a buffer of 256 samples with Safe Mode off. Input latency 315 samples (7.14 ms), output latency 191 samples (4.33 ms).",
    problem: false,
  });
  assert.equal(writeText({ state: "done", report: report("mismatch", "The driver did not take the change as sent: x.") }).problem, true);
  assert.match(writeText({ state: "done", report: report("mismatch", "The driver did not take the change as sent: x.") }).text, /^Not as sent\. /);
  assert.equal(writeText({ state: "done", report: report("failed", "The driver refused the change: y.") }).problem, true);
  assert.equal(writeText({ state: "done", report: report("unconfirmed", "z") }).problem, true);
  assert.equal(writeText({ state: "done", report: report("unchanged", "The driver already has these settings, so nothing was sent.") }).problem, false);
  assert.deepEqual(writeText({ state: "sending", change: { buffer_size: 256 } }), { text: "Sending to the driver...", problem: false });
  assert.deepEqual(writeText({ state: "refused", code: "not_offered", message: "No.", change: { buffer_size: 3 } }), { text: "Not changed. No.", problem: true });
  // In use: the line above already says why, and Change anyway is beside it; the server's API wording stays out.
  assert.deepEqual(writeText({ state: "refused", code: "asio_in_use", message: "ask again with force", change: { buffer_size: 256 } }), { text: "Not changed.", problem: true });
});

test("the store sends a change, shows the driver's read-back, and keeps a refusal with the change it refused", async () => {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"));
  const sent: DriverChange[] = [];
  let refuse: GazelleError | undefined;
  client.driver = async () => QUADRO_REPORT;
  client.setDriver = async (id, change) => {
    sent.push(change);
    if (refuse !== undefined) throw refuse;
    return { device_id: id, outcome: "applied", message: "The driver now reports a buffer of 256 samples with Safe Mode off.", call: null, read_back: QUADRO_256_OFF };
  };
  const store = new Store(client, { storage: new MemoryStorage() });
  await store.start();
  await store.loadDriver("loopback-0");

  const sending = store.setDriver("loopback-0", { buffer_size: 256 });
  assert.equal(store.driverWrite("loopback-0").value?.state, "sending");
  await sending;
  assert.equal(store.driver("loopback-0").value, QUADRO_256_OFF, "the read-back is what the section shows");
  assert.equal(store.driverWrite("loopback-0").value?.state, "done");

  refuse = new GazelleError("asio_in_use", "1 program is using the driver's ASIO interface.");
  await store.setDriver("loopback-0", { safe_mode: true });
  assert.deepEqual(store.driverWrite("loopback-0").value, { state: "refused", code: "asio_in_use", message: "1 program is using the driver's ASIO interface.", change: { safe_mode: true } });
  assert.equal(store.driver("loopback-0").value, QUADRO_256_OFF, "a refusal changes nothing shown");
  assert.deepEqual(sent, [{ buffer_size: 256 }, { safe_mode: true }]);

  // A new read clears the last result, so it never stands beside values it does not describe.
  await store.loadDriver("loopback-0", true);
  assert.equal(store.driverWrite("loopback-0").value, undefined);
  await flush();
});
