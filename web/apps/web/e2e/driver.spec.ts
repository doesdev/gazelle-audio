// The Devices page's Driver section: the audio driver's buffer size, latency and Safe Mode, read
// only. A loopback server has no driver, so the section says so; a device read through a real driver
// is shown by answering the page's request with what the Quadro's driver answered on 2026-09-18.

import { expect, test, type Page } from "@playwright/test";

import { startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { resetWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--backend", "loopback", "--dry-run"], { webUi: true });
});

test.beforeEach(() => resetWorkspace(server));

test.afterAll(async () => {
  await server?.stop();
});

const OFFERED = [8, 16, 32, 64, 128, 256, 512, 1024, 2048];
const quadro = (id: string) => ({
  device_id: id,
  read_at_ms: Date.UTC(2026, 8, 18, 12, 0, 0),
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
});

const section = (page: Page) => page.locator('ga-device-status ga-section[heading="Driver"]');
const value = (page: Page, field: string) => page.locator(`ga-device-status [data-testid="driver-${field}"]`);

test("a loopback device's Driver section says there is no driver, and offers nothing to change", async ({ page }) => {
  await page.goto(`${server.url}/#/devices/loopback-0`);
  await expect(value(page, "message")).toHaveText("The loopback backend has no audio driver on this PC.");
  await expect(value(page, "note")).toContainText("changing them from Gazelle is a later step");
  await expect(value(page, "fields")).toBeHidden();
  await expect(section(page).locator("input, select, [role=slider]")).toHaveCount(0);
  await expect(value(page, "refresh")).toBeHidden();
});

test("a driver's reading shows the version, rate, buffer, both latencies and Safe Mode, as the vendor panel does", async ({ page }) => {
  const asked: string[] = [];
  await page.route("**/api/v1/devices/*/driver*", async (route) => {
    const url = new URL(route.request().url());
    asked.push(url.pathname + url.search);
    await route.fulfill({ json: quadro(decodeURIComponent(url.pathname.split("/")[4] ?? "")) });
  });
  await page.goto(`${server.url}/#/devices/loopback-0`);
  await expect(value(page, "driver_version")).toHaveText("5.68.0 (API 5.12)");
  await expect(value(page, "sample_rate")).toHaveText("44.1 kHz");
  await expect(value(page, "buffer_size")).toHaveText("512 samples");
  await expect(value(page, "input_latency")).toHaveText("571 samples (12.95 ms)");
  await expect(value(page, "output_latency")).toHaveText("632 samples (14.33 ms)");
  await expect(value(page, "safe_mode")).toHaveText("On");
  await expect(value(page, "message")).toBeHidden();
  await expect(section(page).locator("input, select, [role=slider]")).toHaveCount(0);

  await value(page, "refresh").click();
  await expect.poll(() => asked).toEqual(["/api/v1/devices/loopback-0/driver", "/api/v1/devices/loopback-0/driver?refresh=true"]);

  if (process.env["GAZELLE_DRIVER_SCREENSHOT"] !== undefined) {
    await section(page).scrollIntoViewIfNeeded();
    await section(page).screenshot({ path: process.env["GAZELLE_DRIVER_SCREENSHOT"] });
  }
});

test("a value the driver would not give says why in its own row", async ({ page }) => {
  await page.route("**/api/v1/devices/*/driver*", (route) =>
    route.fulfill({ json: { ...quadro("loopback-0"), asio: { state: "unread", message: "Could not be read: its buffer size (3) is not among the sizes it offers." } } }),
  );
  await page.goto(`${server.url}/#/devices/loopback-0`);
  await expect(value(page, "buffer_size")).toHaveText("Could not be read: its buffer size (3) is not among the sizes it offers.");
  await expect(value(page, "input_latency")).toHaveCount(0);
  await expect(value(page, "safe_mode")).toHaveText("On");
});
