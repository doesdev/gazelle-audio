import { test } from "node:test";
import assert from "node:assert/strict";

import { GazelleError, type DriverReport } from "gazelle-audio-client";

import { driverView, latencyText } from "../src/store/driver.ts";
import { Store } from "../src/store/store.ts";
import { device, FakeClient, flush, MemoryStorage } from "./fake-client.ts";

const OFFERED = [8, 16, 32, 64, 128, 256, 512, 1024, 2048];

/** The Quadro as its driver answered on 2026-09-18 (`.agent/reference/driver-api.md`). */
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
  asio: { state: "read", value: { sample_rate: 44100, buffer_size: 512, input_latency: 571, output_latency: 632, buffer_sizes: OFFERED } },
  safe_mode: { state: "read", value: true },
};

test("latency reads as the vendor panel shows it, in samples and milliseconds", () => {
  assert.equal(latencyText(571, 44100), "571 samples (12.95 ms)");
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
