// Layout feedback on the phase 4 mixer: the mixer fills its window's height, strips either fit the
// width (between limits) or take a set width and scroll, and each side panel collapses; the
// choices are remembered per browser.

import { expect, test, type Locator, type Page } from "@playwright/test";

import { startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--dry-run"], { webUi: true });
});

test.afterAll(async () => {
  await server?.stop();
});

const width = async (locator: Locator) => (await locator.boundingBox())?.width ?? 0;
const strips = (page: Page) => page.locator("ga-mixer .strips");

test("the mixer fills the window's height and stretches with it", async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 800 });
  await page.goto(`${server.url}/#/mixer/loopback-0/0`);
  const main = page.locator("ga-app main");
  const bottomGap = async () => {
    const [m, s] = await Promise.all([main.boundingBox(), strips(page).boundingBox()]);
    return (m?.y ?? 0) + (m?.height ?? 0) - ((s?.y ?? 0) + (s?.height ?? 0));
  };
  await expect(page.getByTestId("fader-0")).toBeVisible();
  expect(await bottomGap()).toBeLessThanOrEqual(16);
  const before = (await strips(page).boundingBox())?.height ?? 0;

  await page.setViewportSize({ width: 1600, height: 1100 });
  await expect.poll(async () => (await strips(page).boundingBox())?.height ?? 0).toBeGreaterThan(before + 250);
  expect(await bottomGap()).toBeLessThanOrEqual(16);
  expect(await main.evaluate((el) => el.scrollHeight - el.clientHeight)).toBeLessThanOrEqual(1);
});

test("auto width fits strips to the row within limits; a set width scrolls; both are remembered", async ({ page }) => {
  // With both side panels open, 32 strips reach about 90 px at 3600 px wide and about 60 px at 2600.
  await page.setViewportSize({ width: 3600, height: 900 });
  await page.goto(`${server.url}/#/mixer/loopback-0/0`);
  const strip = page.locator('ga-strip[strip="4"]');
  const row = strips(page);
  await expect(page.getByTestId("strip-width-auto")).toBeChecked();
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

  await page.getByTestId("strip-width-auto").uncheck();
  const input = page.getByTestId("strip-width");
  await input.fill("100");
  await input.press("Enter");
  await expect.poll(() => width(strip)).toBe(100);
  expect(await row.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeGreaterThan(1000);

  await page.setViewportSize({ width: 3600, height: 900 });
  await expect.poll(() => width(strip)).toBe(100);

  await page.reload();
  await expect(page.getByTestId("strip-width-auto")).not.toBeChecked();
  await expect(page.getByTestId("strip-width")).toHaveValue("100");
  await expect.poll(() => width(strip)).toBe(100);
});

test("each side panel collapses to a rail and stays collapsed after a reload", async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await page.goto(`${server.url}/#/mixer/loopback-0/0`);
  const left = page.locator("ga-app aside.left");
  const right = page.locator("ga-app aside.right");
  const main = page.locator("ga-app main");
  await expect(page.locator("ga-device-list a[data-device-id]").first()).toBeVisible();
  const mainBefore = await width(main);

  await page.getByRole("button", { name: "Collapse the devices panel" }).click();
  await expect.poll(() => width(left)).toBeLessThanOrEqual(30);
  await expect(page.locator("ga-device-list")).toBeHidden();
  await expect(right.getByText("Control Room")).toBeVisible();

  await page.getByRole("button", { name: "Collapse the meters panel" }).click();
  await expect.poll(() => width(right)).toBeLessThanOrEqual(30);
  const edge = await page.locator("ga-mixer .strips").evaluate((row) => {
    row.scrollLeft = 200;
    return row.getBoundingClientRect().right - (row.querySelector(".master") as HTMLElement).getBoundingClientRect().right;
  });
  expect(edge, "the master stays flush with the row's right edge while strips scroll").toBeLessThanOrEqual(0.5);
  expect(await width(main)).toBeGreaterThan(mainBefore + 350);

  await page.reload();
  await expect.poll(() => width(left)).toBeLessThanOrEqual(30);
  await expect.poll(() => width(right)).toBeLessThanOrEqual(30);

  await page.getByRole("button", { name: "Expand the devices panel" }).click();
  await expect(page.locator("ga-device-list a[data-device-id]").first()).toBeVisible();
  await expect(right.getByText("Control Room")).toBeHidden();
});
