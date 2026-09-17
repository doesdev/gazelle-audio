// The devices panel on the left (the user's observations, 2026-09-16): a card per device with its
// state at a glance, and picking one switches the page you are on to that device rather than
// taking you to its Devices page. That makes each page's own device dropdown redundant, so they
// are gone.

import { expect, test, type Page } from "@playwright/test";

import { startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { resetWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  // Reports every 50 ms, so the cards have something to show.
  server = await startServer(["--dry-run", "--loopback-cyclic-ms", "50"], { webUi: true });
});

test.beforeEach(() => resetWorkspace(server));

test.afterAll(async () => {
  await server?.stop();
});

const card = (page: Page, id: string) => page.locator(`ga-device-list a[data-device-id="${id}"]`);

test("picking a device switches the page you are on to it, on every page with a device", async ({ page }) => {
  for (const [name, element] of [
    ["devices", "ga-device-status"],
    ["inputs", "ga-inputs"],
    ["outputs", "ga-outputs"],
    ["mixer", "ga-mixer"],
    ["routing", "ga-routing"],
  ] as const) {
    await page.goto(`${server.url}/#/${name}/loopback-0`);
    await expect(page.locator(element)).toHaveAttribute("device-id", "loopback-0");
    await card(page, "loopback-1").click();
    await expect(page).toHaveURL(new RegExp(`#/${name}/loopback-1$`));
    await expect(page.locator(element)).toHaveAttribute("device-id", "loopback-1");
    await expect(card(page, "loopback-1")).toHaveAttribute("aria-current", "page");
    await expect(card(page, "loopback-0")).not.toHaveAttribute("aria-current", "page");
  }
});

test("on the Workspace page, picking a device selects it without leaving, and the next page opens on it", async ({ page }) => {
  await page.goto(`${server.url}/#/inputs/loopback-0`);
  await page.locator('ga-header nav a[data-page="workspace"]').click();
  await card(page, "loopback-1").click();
  await expect(page).toHaveURL(/#\/workspace$/);
  await expect(page.locator("ga-workspace")).toBeVisible();
  await expect(card(page, "loopback-1")).toHaveAttribute("aria-current", "page");

  await page.locator('ga-header nav a[data-page="outputs"]').click();
  await expect(page.locator("ga-outputs")).toHaveAttribute("device-id", "loopback-1");
});

test("the pages have no device dropdown of their own", async ({ page }) => {
  for (const [name, element] of [
    ["inputs", "ga-inputs"],
    ["outputs", "ga-outputs"],
    ["mixer", "ga-mixer"],
    ["routing", "ga-routing"],
  ] as const) {
    await page.goto(`${server.url}/#/${name}/loopback-0`);
    await expect(page.locator(element)).toBeVisible();
    await expect(page.locator(element).locator('select[aria-label="Device"]')).toHaveCount(0);
  }
});

test("each card shows the device's clock, power, preset and input level from its status report", async ({ page }) => {
  await page.goto(`${server.url}/#/workspace`);
  for (const id of ["loopback-0", "loopback-1"]) {
    const it = card(page, id);
    // The loopback's report bytes cycle, so the values move: what is checked is that each part shows.
    await expect(it.locator(".rate")).toHaveText(/kHz|—/);
    await expect(it.locator(".lock")).toHaveText(/LOCKED|NO LOCK/);
    await expect(it.locator(".power")).toHaveText(/^(On|Standby)$/);
    await expect(it.locator(".preset")).toHaveText(/^Preset \d+$/);
    await expect(it.locator(".input")).toHaveAttribute("data-level", /^(quiet|signal|clip)$/);
  }
});

test("the right panel meters the device's outputs: Monitor, HP1, HP2 and Line out on the Quadro; the Studio+ reports none", async ({ page }) => {
  await page.goto(`${server.url}/#/inputs/loopback-0`);
  const meters = page.locator("ga-output-meters");
  await expect(meters.locator("[data-output]")).toHaveCount(4);
  await expect(meters.locator("[data-output]").locator(".output-name")).toHaveText(["Monitor", "HP1", "HP2", "Line out"]);
  // The loopback's report bytes cycle, so the meter moves.
  const bar = meters.locator('[data-output="Line out"] .mask').first();
  const first = await bar.evaluate((el) => (el as HTMLElement).style.width);
  await expect.poll(() => bar.evaluate((el) => (el as HTMLElement).style.width)).not.toBe(first);

  await page.locator('ga-device-list a[data-device-id="loopback-1"]').click();
  await expect(meters.locator("[data-output]")).toHaveCount(0);
  await expect(meters).toContainText("reports no output meters");
});
