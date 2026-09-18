// The Devices page's Driver section: the audio driver's buffer size, latency and Safe Mode, and the
// menu and switch that change the buffer size and Safe Mode. A loopback server has no driver, so the
// section says so; a device read through a real driver is shown by answering the page's requests
// with what the Quadro's driver answered on 2026-09-18, and a change by answering its PUT as the
// server would. No test here reaches a driver.

import { expect, test, type Page, type Route } from "@playwright/test";

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

/** The Quadro's reading; the ASIO values can be changed per test. */
const quadro = (id: string, asio: Partial<{ buffer_size: number; input_latency: number; output_latency: number; safe_mode: boolean; asio_clients: number }> = {}) => {
  const value = { sample_rate: 44100, reference_rate: 44100, buffer_size: 512, input_latency: 571, output_latency: 632, buffer_sizes: OFFERED, safe_mode: true, asio_clients: 0, ...asio };
  return {
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
    asio: { state: "read", value },
    safe_mode: { state: "read", value: value.safe_mode },
  };
};

const section = (page: Page) => page.locator('ga-device-status ga-section[heading="Driver"]');
const value = (page: Page, field: string) => page.locator(`ga-device-status [data-testid="driver-${field}"]`);

interface Driver {
  /** Every PUT body the page sent. */
  puts: unknown[];
  /** Every GET path the page asked. */
  gets: string[];
}

/**
 * Answers the page's driver requests: a GET with `reading`, a PUT with `answer(body)` (a write
 * report, or an error with its status).
 */
async function fakeDriver(page: Page, reading: ReturnType<typeof quadro>, answer: (body: Record<string, unknown>) => { status: number; json: unknown } = () => ({ status: 200, json: {} })): Promise<Driver> {
  const driver: Driver = { puts: [], gets: [] };
  await page.route("**/api/v1/devices/*/driver*", async (route: Route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (request.method() === "PUT") {
      const body = request.postDataJSON() as Record<string, unknown>;
      driver.puts.push(body);
      const { status, json } = answer(body);
      await route.fulfill({ status, json });
    } else {
      driver.gets.push(url.pathname + url.search);
      await route.fulfill({ json: reading });
    }
  });
  return driver;
}

/** The write report the server gives when the driver took the change, with its read-back. */
const applied = (readBack: ReturnType<typeof quadro>, message: string, outcome = "applied") => ({ status: 200, json: { device_id: "loopback-0", outcome, message, call: null, read_back: readBack } });

test("a loopback device's Driver section says there is no driver, and offers nothing to change", async ({ page }) => {
  await page.goto(`${server.url}/#/devices/loopback-0`);
  await expect(value(page, "message")).toHaveText("The loopback backend has no audio driver on this PC.");
  await expect(value(page, "note")).toContainText("restarts its audio");
  await expect(value(page, "fields")).toBeHidden();
  await expect(section(page).locator("input, select, [role=slider], [data-testid=driver-safe-mode-switch]")).toHaveCount(0);
  await expect(value(page, "refresh")).toBeHidden();
});

test("a driver's reading shows the version, rate, a buffer menu, both latencies and a Safe Mode switch", async ({ page }) => {
  const driver = await fakeDriver(page, quadro("loopback-0"));
  await page.goto(`${server.url}/#/devices/loopback-0`);
  await expect(value(page, "driver_version")).toHaveText("5.68.0 (API 5.12)");
  await expect(value(page, "sample_rate")).toHaveText("44.1 kHz");
  await expect(value(page, "buffer-menu")).toHaveValue("512");
  await expect(value(page, "buffer-menu").locator("option")).toHaveText(OFFERED.map((size) => `${size} samples`));
  await expect(value(page, "input_latency")).toHaveText("571 samples (12.95 ms)");
  await expect(value(page, "output_latency")).toHaveText("632 samples (14.33 ms)");
  await expect(value(page, "safe-mode-switch")).toHaveText("On");
  await expect(value(page, "safe-mode-switch")).toHaveAttribute("aria-pressed", "true");
  await expect(value(page, "message")).toBeHidden();
  await expect(value(page, "in-use")).toBeHidden();
  await expect(value(page, "result")).toBeHidden();
  await expect(value(page, "force")).toBeHidden();

  await value(page, "refresh").click();
  await expect.poll(() => driver.gets).toEqual(["/api/v1/devices/loopback-0/driver", "/api/v1/devices/loopback-0/driver?refresh=true"]);
  expect(driver.puts).toEqual([]);
});

test("a buffer size is sent only from its Confirm, and the driver's read-back is shown with its latencies", async ({ page }) => {
  // The Quadro's driver after 256 samples with Safe Mode kept on (reference/driver-api.md).
  const after = quadro("loopback-0", { buffer_size: 256, input_latency: 315, output_latency: 367 });
  const driver = await fakeDriver(page, quadro("loopback-0"), () => applied(after, "The driver now reports a buffer of 256 samples with Safe Mode on."));
  await page.goto(`${server.url}/#/devices/loopback-0`);
  await expect(value(page, "buffer-menu")).toHaveValue("512");

  // Choosing is the first click: nothing is sent, and a Confirm appears naming the restart.
  await value(page, "buffer-menu").selectOption("256");
  await expect(value(page, "buffer-confirm")).toBeVisible();
  await expect(value(page, "buffer-confirm")).toHaveText("Confirm");
  await expect(value(page, "buffer-confirm")).toHaveAttribute("title", "Change the driver's buffer to 256 samples: a program using the driver (a DAW) restarts its audio");
  await page.waitForTimeout(300);
  expect(driver.puts).toEqual([]);

  // Confirm is the second: one PUT of only the buffer, so the server keeps Safe Mode as the driver has it.
  await value(page, "buffer-confirm").click();
  await expect.poll(() => driver.puts).toEqual([{ buffer_size: 256 }]);
  await expect(value(page, "buffer-confirm")).toBeHidden();
  await expect(value(page, "buffer-menu")).toHaveValue("256");
  await expect(value(page, "input_latency")).toHaveText("315 samples (7.14 ms)");
  await expect(value(page, "output_latency")).toHaveText("367 samples (8.32 ms)");
  await expect(value(page, "result")).toHaveText(
    "Changed. The driver now reports a buffer of 256 samples with Safe Mode on. Input latency 315 samples (7.14 ms), output latency 367 samples (8.32 ms).",
  );
  await expect(value(page, "result")).not.toHaveClass(/warning/);

  if (process.env["GAZELLE_DRIVER_SCREENSHOT"] !== undefined) {
    await section(page).scrollIntoViewIfNeeded();
    await section(page).screenshot({ path: process.env["GAZELLE_DRIVER_SCREENSHOT"] });
  }
});

test("a buffer choice left unconfirmed goes back and sends nothing", async ({ page }) => {
  const driver = await fakeDriver(page, quadro("loopback-0"));
  await page.goto(`${server.url}/#/devices/loopback-0`);
  await value(page, "buffer-menu").selectOption("128");
  await expect(value(page, "buffer-confirm")).toBeVisible();
  await expect(value(page, "buffer-confirm")).toBeHidden({ timeout: 5000 });
  await expect(value(page, "buffer-menu")).toHaveValue("512");
  expect(driver.puts).toEqual([]);
});

test("Safe Mode takes a confirming second click, and only Safe Mode is sent", async ({ page }) => {
  const after = quadro("loopback-0", { output_latency: 367, safe_mode: false });
  const driver = await fakeDriver(page, quadro("loopback-0"), () => applied(after, "The driver now reports a buffer of 512 samples with Safe Mode off."));
  await page.goto(`${server.url}/#/devices/loopback-0`);
  const safe = value(page, "safe-mode-switch");
  await expect(safe).toHaveText("On");

  await safe.click();
  await expect(safe).toHaveText("Confirm");
  await expect(safe).toHaveAttribute("data-armed", "");
  await page.waitForTimeout(300);
  expect(driver.puts, "one click sends nothing").toEqual([]);

  await safe.click();
  await expect.poll(() => driver.puts).toEqual([{ safe_mode: false }]);
  await expect(safe).toHaveText("Off");
  await expect(safe).toHaveAttribute("aria-pressed", "false");
  await expect(value(page, "output_latency")).toHaveText("367 samples (8.32 ms)");
  await expect(value(page, "buffer-menu")).toHaveValue("512");
});

test("the wheel over the buffer menu changes nothing and sends nothing", async ({ page }) => {
  const driver = await fakeDriver(page, quadro("loopback-0"));
  await page.goto(`${server.url}/#/devices/loopback-0`);
  const menu = value(page, "buffer-menu");
  await expect(menu).toHaveValue("512");
  await menu.scrollIntoViewIfNeeded();
  const box = await menu.boundingBox();
  if (box === null) throw new Error("the menu has no box");
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  // One notch down: a select that took the wheel would step to 1024 and show its Confirm.
  await page.mouse.wheel(0, 100);
  await page.waitForTimeout(400);
  // Read once, not retried: a Confirm the wheel raised would put the menu back by itself after three seconds.
  expect(await menu.inputValue()).toBe("512");
  expect(await value(page, "buffer-confirm").isHidden()).toBe(true);
  expect(driver.puts).toEqual([]);
});

test("a program using ASIO is named, a change is refused, and Change anyway sends it forced after a confirm", async ({ page }) => {
  const after = quadro("loopback-0", { buffer_size: 256, input_latency: 315, output_latency: 367, asio_clients: 1 });
  const driver = await fakeDriver(page, quadro("loopback-0", { asio_clients: 1 }), (body) =>
    body["force"] === true
      ? applied(after, "The driver now reports a buffer of 256 samples with Safe Mode on.")
      : { status: 409, json: { error: { code: "asio_in_use", message: "The driver's ASIO interface is in use (by a DAW, most likely), and changing the buffer or Safe Mode restarts its audio. Nothing was sent; ask again with force to change it anyway." } } },
  );
  await page.goto(`${server.url}/#/devices/loopback-0`);
  await expect(value(page, "in-use")).toBeVisible();
  await expect(value(page, "in-use")).toHaveText("The driver's ASIO interface is in use now (by a DAW, most likely). A change is refused while it is, unless you choose Change anyway.");

  await value(page, "buffer-menu").selectOption("256");
  await value(page, "buffer-confirm").click();
  await expect.poll(() => driver.puts).toEqual([{ buffer_size: 256 }]);
  await expect(value(page, "result")).toContainText("Not changed. The driver's ASIO interface is in use");
  await expect(value(page, "result")).toHaveClass(/warning/);
  await expect(value(page, "buffer-menu"), "the menu shows what the driver still has").toHaveValue("512");
  await expect(value(page, "input_latency")).toHaveText("571 samples (12.95 ms)");

  const force = value(page, "force");
  await expect(force).toBeVisible();
  await force.click();
  await expect(force).toHaveText("Confirm");
  await page.waitForTimeout(300);
  expect(driver.puts).toHaveLength(1);
  await force.click();
  await expect.poll(() => driver.puts).toEqual([{ buffer_size: 256 }, { buffer_size: 256, force: true }]);
  await expect(value(page, "input_latency")).toHaveText("315 samples (7.14 ms)");
  await expect(force).toBeHidden();
});

test("a read-back that differs from what was sent is shown as it is, and says so", async ({ page }) => {
  const driver = await fakeDriver(page, quadro("loopback-0"), () =>
    applied(quadro("loopback-0"), "The driver did not take the change as sent: a buffer of 256 samples was sent and the driver reports 512.", "mismatch"),
  );
  await page.goto(`${server.url}/#/devices/loopback-0`);
  await value(page, "buffer-menu").selectOption("256");
  await value(page, "buffer-confirm").click();
  await expect.poll(() => driver.puts).toHaveLength(1);
  await expect(value(page, "result")).toHaveText("Not as sent. The driver did not take the change as sent: a buffer of 256 samples was sent and the driver reports 512.");
  await expect(value(page, "result")).toHaveClass(/warning/);
  await expect(value(page, "buffer-menu")).toHaveValue("512");
});

test("a value the driver would not give says why in its own row, and there is nothing to change", async ({ page }) => {
  await page.route("**/api/v1/devices/*/driver*", (route) =>
    route.fulfill({ json: { ...quadro("loopback-0"), asio: { state: "unread", message: "Could not be read: its buffer size (3) is not among the sizes it offers." } } }),
  );
  await page.goto(`${server.url}/#/devices/loopback-0`);
  await expect(value(page, "buffer_size")).toHaveText("Could not be read: its buffer size (3) is not among the sizes it offers.");
  await expect(value(page, "input_latency")).toHaveCount(0);
  await expect(value(page, "safe_mode")).toHaveText("On");
  await expect(section(page).locator("select")).toHaveCount(0);
});
