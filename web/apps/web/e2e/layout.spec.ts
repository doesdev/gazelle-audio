// Layout feedback on the mixer: it fills its window's height, channels either fit the width
// (between limits) or take a set width and scroll, and the sidebar collapses; the choices are
// remembered per browser.

import { expect, test, type Locator, type Page } from "@playwright/test";

import { startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { putWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--dry-run"], { webUi: true });
  // 26 channels (every free Quadro input): one active on Mix 1, so its master shows, and 25 inactive.
  const channels = Array.from({ length: 26 }, (_, i) => (i === 0 ? { id: "c0", name: "Vox", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] } : { id: `c${i}`, name: "", slot: 6 + i, sends: [] }));
  // The Studio+ has the tallest channel: a grouped preamp channel over a strip with its own send section.
  const studio = { groups: [{ id: "g", name: "Drums", collapsed: false }], channels: [{ id: "s0", name: "Kick", slot: 0, group: "g", source: { group: 0, channel: 0 }, main_mix: 0, sends: [1] }] };
  await putWorkspace(server, { mixers: { "loopback-0": { channels }, "loopback-1": studio } });
});

test.afterAll(async () => {
  await server?.stop();
});

const width = async (locator: Locator) => (await locator.boundingBox())?.width ?? 0;
const strips = (page: Page) => page.locator("ga-mixer .strips");

test("the mixer fills the window's height and stretches with it", async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 800 });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  const main = page.locator("ga-app main");
  const bottomGap = async () => {
    const [m, s] = await Promise.all([main.boundingBox(), strips(page).boundingBox()]);
    return (m?.y ?? 0) + (m?.height ?? 0) - ((s?.y ?? 0) + (s?.height ?? 0));
  };
  await expect(page.getByTestId("fader-6")).toBeVisible();
  expect(await bottomGap()).toBeLessThanOrEqual(16);
  const before = (await strips(page).boundingBox())?.height ?? 0;

  await page.setViewportSize({ width: 1600, height: 1100 });
  await expect.poll(async () => (await strips(page).boundingBox())?.height ?? 0).toBeGreaterThan(before + 250);
  expect(await bottomGap()).toBeLessThanOrEqual(16);
  expect(await main.evaluate((el) => el.scrollHeight - el.clientHeight)).toBeLessThanOrEqual(1);
});

test("the tallest channel still fits a short window: the page does not scroll and the name bar shows", async ({ page }) => {
  await page.setViewportSize({ width: 1400, height: 860 });
  await page.goto(`${server.url}/#/mixer/loopback-1`);
  await expect(page.getByTestId("pre-ch-0")).not.toHaveAttribute("data-empty", "");
  const main = page.locator("ga-app main");
  expect(await main.evaluate((el) => el.scrollHeight - el.clientHeight)).toBeLessThanOrEqual(1);
  const [nameBar, row] = await Promise.all([page.locator('ga-channel[data-channel-slot="0"] ga-strip .name').boundingBox(), page.locator("ga-mixer .strips").boundingBox()]);
  expect((nameBar?.y ?? 0) + (nameBar?.height ?? 0)).toBeLessThanOrEqual((row?.y ?? 0) + (row?.height ?? 0) + 1);
});

test("auto width fits strips to the row within limits; a set width scrolls; both are remembered", async ({ page }) => {
  // With the sidebar open, 26 channels reach the 120 px ceiling at 3600 px wide and about 85 px at 2600.
  await page.setViewportSize({ width: 3600, height: 900 });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  const strip = page.locator('ga-channel[data-channel-slot="10"]');
  const row = strips(page);
  await expect(page.getByTestId("strip-width-auto")).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("strip-width-fixed")).toHaveAttribute("aria-pressed", "false");
  await expect(page.getByTestId("strip-width")).toBeDisabled();

  const wide = await width(strip);
  expect(wide).toBeGreaterThan(80);
  expect(wide).toBeLessThanOrEqual(120);
  expect(await row.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(1);

  await page.setViewportSize({ width: 2600, height: 900 });
  await expect.poll(() => width(strip)).toBeLessThan(wide - 20);
  expect(await row.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(1);

  await page.setViewportSize({ width: 1000, height: 900 });
  await expect.poll(() => width(strip)).toBe(48);
  expect(await row.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeGreaterThan(0);

  await page.getByTestId("strip-width-fixed").click();
  const input = page.getByTestId("strip-width");
  await expect(input).toBeFocused();
  await input.fill("100");
  await input.press("Enter");
  await expect.poll(() => width(strip)).toBe(100);
  expect(await row.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeGreaterThan(1000);

  await page.setViewportSize({ width: 3600, height: 900 });
  await expect.poll(() => width(strip)).toBe(100);

  await page.reload();
  await expect(page.getByTestId("strip-width-fixed")).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("strip-width")).toHaveValue("100");
  await expect.poll(() => width(strip)).toBe(100);
});

test("the sidebar collapses to a rail and stays collapsed after a reload", async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  const sidebar = page.locator("ga-app aside.sidebar");
  const main = page.locator("ga-app main");
  await expect(page.locator("ga-device-list a[data-device-id]").first()).toBeVisible();
  const mainBefore = await width(main);

  await page.getByRole("button", { name: "Collapse the sidebar" }).click();
  await expect.poll(() => width(sidebar)).toBeLessThanOrEqual(30);
  await expect(page.locator("ga-device-list")).toBeHidden();
  await expect(sidebar.getByText("Control Room")).toBeHidden();
  const edge = await page.locator("ga-mixer .strips").evaluate((row) => {
    row.scrollLeft = 200;
    return row.getBoundingClientRect().right - (row.querySelector(".masters") as HTMLElement).getBoundingClientRect().right;
  });
  expect(edge, "the masters stay flush with the row's right edge while channels scroll").toBeLessThanOrEqual(0.5);
  expect(await width(main)).toBeGreaterThan(mainBefore + 200);

  await page.reload();
  await expect.poll(() => width(sidebar)).toBeLessThanOrEqual(30);

  await page.getByRole("button", { name: "Expand the sidebar" }).click();
  await expect(page.locator("ga-device-list a[data-device-id]").first()).toBeVisible();
  await expect(sidebar.getByText("Control Room")).toBeVisible();
});
