// Moving between pages keeps your place (hands-on feedback items 2-4, decision P71): the device last
// opened follows you to pages whose address names none, each device keeps its selected mix, and a
// page left and come back to finds its scroll, open sections and selections as they were. The
// device and mix are remembered per browser; the rest for the tab.

import { expect, test, type Page } from "@playwright/test";

import { startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { putWorkspace, resetWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--dry-run"], { webUi: true });
});

test.beforeEach(() => resetWorkspace(server));

test.afterAll(async () => {
  await server?.stop();
});

/** Follows a header link, as a person moving between pages does. */
const open = (page: Page, name: "devices" | "workspace" | "inputs" | "outputs" | "mixer" | "routing") => page.locator(`ga-header nav a[data-page="${name}"]`).click();

test("the device last opened follows you to every page, and across a reload", async ({ page }) => {
  await page.goto(`${server.url}/#/inputs/loopback-1`);
  await expect(page.locator("ga-inputs").getByLabel("Device")).toHaveValue("loopback-1");

  await open(page, "outputs");
  await expect(page.locator("ga-outputs").getByLabel("Device")).toHaveValue("loopback-1");
  await open(page, "mixer");
  await expect(page.locator("ga-mixer").getByLabel("Device", { exact: true })).toHaveValue("loopback-1");
  await open(page, "routing");
  await expect(page.locator("ga-routing").getByLabel("Device")).toHaveValue("loopback-1");
  await open(page, "devices");
  await expect(page.locator("ga-device-status")).toHaveAttribute("device-id", "loopback-1");
  await expect(page.locator('ga-device-list a[data-device-id="loopback-1"]')).toHaveAttribute("aria-current", "page");
  // The Control Room panel shows it too, on a page without a device.
  await open(page, "workspace");
  await expect(page.locator("ga-monitor")).toHaveAttribute("device-id", "loopback-1");

  // A device picked on a page's own picker is the new one.
  await open(page, "outputs");
  await page.locator("ga-outputs").getByLabel("Device").selectOption("loopback-0");
  await open(page, "inputs");
  await expect(page.locator("ga-inputs").getByLabel("Device")).toHaveValue("loopback-0");

  // An old link to a device that is not connected does not replace it.
  await page.goto(`${server.url}/#/inputs/usb-gone`);
  await expect(page.locator("ga-inputs")).toContainText("not connected");
  await page.goto(`${server.url}/#/routing`);
  await expect(page.locator("ga-routing").getByLabel("Device")).toHaveValue("loopback-0");

  await page.getByLabel("Pages").locator('a[data-page="devices"]').click();
  await page.locator('ga-device-list a[data-device-id="loopback-1"]').click();
  await page.reload();
  await open(page, "outputs");
  await expect(page.locator("ga-outputs").getByLabel("Device")).toHaveValue("loopback-1");
});

test("each device keeps its selected mix: the address names it, and a page without one uses the last", async ({ page }) => {
  // A deep link still opens its mix.
  await page.goto(`${server.url}/#/mixer/loopback-0/2`);
  const metered = page.getByTestId("metered-mix");
  await expect(metered).toHaveValue("2");

  // Choosing a mix puts it in the address without rebuilding the page.
  await page.locator("ga-mixer").evaluate((el) => ((el as unknown as { __kept: boolean }).__kept = true));
  await metered.selectOption("1");
  await expect(page).toHaveURL(/#\/mixer\/loopback-0\/1$/);
  expect(await page.locator("ga-mixer").evaluate((el) => (el as unknown as { __kept?: boolean }).__kept)).toBe(true);

  await page.locator("ga-mixer").getByLabel("Device", { exact: true }).selectOption("loopback-1");
  await expect(metered).toHaveValue("0");
  await metered.selectOption("3");

  await open(page, "inputs");
  await open(page, "mixer");
  await expect(page.locator("ga-mixer").getByLabel("Device", { exact: true })).toHaveValue("loopback-1");
  await expect(metered).toHaveValue("3");
  await page.locator("ga-mixer").getByLabel("Device", { exact: true }).selectOption("loopback-0");
  await expect(metered).toHaveValue("1");

  await page.goto(`${server.url}/#/mixer`);
  await page.reload();
  await expect(page.locator("ga-mixer").getByLabel("Device", { exact: true })).toHaveValue("loopback-0");
  await expect(metered).toHaveValue("1");
});

test("a page left and come back to finds its scroll, sections and selections as they were", async ({ page }) => {
  await page.setViewportSize({ width: 1400, height: 600 });
  const main = page.locator("ga-app main");

  // Devices: a collapsed section and the scroll position.
  await page.goto(`${server.url}/#/devices/loopback-0`);
  const clock = page.locator('ga-device-status ga-section[heading="Clock"]');
  await clock.getByRole("button", { name: "Clock" }).click();
  await expect(clock).toHaveJSProperty("collapsed", true);
  await main.evaluate((el) => (el.scrollTop = 200));
  await expect.poll(() => main.evaluate((el) => el.scrollTop)).toBe(200);
  await page.waitForTimeout(100);
  await open(page, "workspace");
  await expect.poll(() => main.evaluate((el) => el.scrollTop)).toBe(0);
  await open(page, "devices");
  await expect(clock).toHaveJSProperty("collapsed", true);
  await expect.poll(() => main.evaluate((el) => el.scrollTop)).toBe(200);

  // Routing: the sources picked for a fill.
  await open(page, "routing");
  await page.getByTestId("source-0-0").click();
  await page.getByTestId("source-0-2").click({ modifiers: ["Shift"] });
  await open(page, "inputs");
  await open(page, "routing");
  await expect(page.getByTestId("source-0-1")).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("source-0-3")).toHaveAttribute("aria-pressed", "false");

  // Mixer, before any channel is set up: the starting layout picked but not yet applied.
  await open(page, "mixer");
  await page.getByTestId("profile-select").selectOption("podcast");
  await open(page, "routing");
  await open(page, "mixer");
  await expect(page.getByTestId("profile-select")).toHaveValue("podcast");

  // Mixer: the notes, the channels' horizontal scroll, and a half-typed layout name.
  const channels = Array.from({ length: 26 }, (_, i) => (i === 0 ? { id: "c0", name: "Vox", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] } : { id: `c${i}`, name: "", slot: 6 + i, sends: [] }));
  await putWorkspace(server, { mixers: { "loopback-0": { channels } } });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  await page.reload();
  const strips = page.locator("ga-mixer .strips");
  await expect(page.getByTestId("fader-6")).toBeVisible();
  await page.locator("ga-mixer details.notes summary").click();
  await strips.evaluate((el) => (el.scrollLeft = 150));
  await expect.poll(() => strips.evaluate((el) => el.scrollLeft)).toBe(150);
  await page.getByTestId("layout-save-name").fill("Half a na");
  await page.waitForTimeout(100);
  await open(page, "outputs");
  await open(page, "mixer");
  await expect(page.locator("ga-mixer details.notes")).toHaveJSProperty("open", true);
  await expect.poll(() => strips.evaluate((el) => el.scrollLeft)).toBe(150);
  await expect(page.getByTestId("layout-save-name")).toHaveValue("Half a na");
});
