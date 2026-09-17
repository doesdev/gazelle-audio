// One sidebar (the user, 2026-09-16): the device cards, the meter and the Control Room in one
// column that docks to either side, on the right unless moved, and collapses to its rail; the
// side, the collapse and each section's collapse are remembered per browser. At phone width it is
// a drawer opened from the header, and no page scrolls sideways: wide content scrolls in its own box.

import { expect, test, type Page } from "@playwright/test";

import { startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { putWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--dry-run"], { webUi: true });
  // Enough channels on the Quadro that the Mixer's strips cannot fit a phone.
  const channels = Array.from({ length: 12 }, (_, i) => ({ id: `c${i}`, name: `Ch ${i + 1}`, slot: 6 + i, sends: [], ...(i < 4 ? { source: { group: 0, channel: i }, main_mix: 0 } : {}) }));
  await putWorkspace(server, { mixers: { "loopback-0": { channels } } });
});

test.afterAll(async () => {
  await server?.stop();
});

const ROUTES = ["devices/loopback-0", "workspace", "inputs/loopback-0", "inputs/loopback-1", "outputs/loopback-0", "mixer/loopback-0", "routing/loopback-0", "routing/loopback-1"];

const box = async (page: Page, selector: string) => (await page.locator(selector).boundingBox()) ?? { x: 0, y: 0, width: 0, height: 0 };

/** How far the page itself, and the app's main area, scroll sideways. */
const sideways = (page: Page) =>
  page.evaluate(() => {
    const main = document.querySelector("ga-app")?.shadowRoot?.querySelector("main");
    return { page: document.documentElement.scrollWidth - window.innerWidth, main: main === null || main === undefined ? -1 : main.scrollWidth - main.clientWidth };
  });

test.describe("on a desktop", () => {
  test("the sidebar holds devices, meter and Control Room, on the right; it moves, collapses and remembers both", async ({ page }) => {
    await page.setViewportSize({ width: 1400, height: 860 });
    await page.goto(`${server.url}/#/mixer/loopback-0`);
    const sidebar = page.locator("ga-app aside.sidebar");
    await expect(page.locator("ga-app aside")).toHaveCount(1);
    expect(await sidebar.locator(":scope > .content > ga-section").evaluateAll((sections) => sections.map((s) => s.getAttribute("heading")))).toEqual(["Devices", "Meter", "Control Room"]);
    await expect(sidebar.locator("ga-device-list a[data-device-id]").first()).toBeVisible();
    await expect(sidebar.locator("ga-output-meters")).toBeVisible();
    await expect(sidebar.locator("ga-control-room")).toBeVisible();
    await expect(page.getByRole("button", { name: /open the sidebar/i })).toBeHidden();

    const main = await box(page, "ga-app main");
    let side = await box(page, "ga-app aside.sidebar");
    expect(side.x, "on the right by default").toBeGreaterThan(main.x + main.width - 2);

    await page.getByRole("button", { name: "Move the sidebar to the left" }).click();
    side = await box(page, "ga-app aside.sidebar");
    expect(side.x + side.width).toBeLessThanOrEqual((await box(page, "ga-app main")).x + 2);

    const mainOpen = (await box(page, "ga-app main")).width;
    await page.getByRole("button", { name: "Collapse the sidebar" }).click();
    await expect.poll(async () => (await box(page, "ga-app aside.sidebar")).width).toBeLessThanOrEqual(30);
    await expect(sidebar.locator("ga-device-list")).toBeHidden();
    expect((await box(page, "ga-app main")).width).toBeGreaterThan(mainOpen + 150);
    // Collapsed, it can still be moved back.
    await expect(page.getByRole("button", { name: "Move the sidebar to the right" })).toBeVisible();

    await page.reload();
    await expect.poll(async () => (await box(page, "ga-app aside.sidebar")).width).toBeLessThanOrEqual(30);
    expect((await box(page, "ga-app aside.sidebar")).x).toBeLessThan(10);

    await page.getByRole("button", { name: "Expand the sidebar" }).click();
    await page.getByRole("button", { name: "Move the sidebar to the right" }).click();
    await expect(sidebar.locator("ga-device-list a[data-device-id]").first()).toBeVisible();
    side = await box(page, "ga-app aside.sidebar");
    expect(side.x).toBeGreaterThan(1000);
  });

  test("each section's collapse is remembered per browser", async ({ page }) => {
    await page.setViewportSize({ width: 1400, height: 860 });
    await page.goto(`${server.url}/#/devices/loopback-0`);
    const meter = page.locator('ga-app aside.sidebar ga-section[heading="Meter"]');
    await meter.getByRole("button", { name: "Meter" }).click();
    await expect(meter).toHaveJSProperty("collapsed", true);
    await expect(page.locator("ga-app aside.sidebar ga-output-meters")).toBeHidden();

    await page.reload();
    await expect(meter).toHaveJSProperty("collapsed", true);
    await expect(page.locator('ga-app aside.sidebar ga-section[heading="Devices"]')).toHaveJSProperty("collapsed", false);
    await meter.getByRole("button", { name: "Meter" }).click();
    await page.reload();
    await expect(meter).toHaveJSProperty("collapsed", false);
  });

  test("the two side panels' stored collapse carries over to the sidebar's sections", async ({ page }) => {
    await page.setViewportSize({ width: 1400, height: 860 });
    await page.goto(`${server.url}/#/devices/loopback-0`);
    await page.evaluate(() => {
      localStorage.removeItem("gazelle.layout.sidebar");
      localStorage.setItem("gazelle.layout.panels", JSON.stringify({ leftCollapsed: true, rightCollapsed: false }));
    });
    await page.reload();
    await expect(page.locator('ga-app aside.sidebar ga-section[heading="Devices"]')).toHaveJSProperty("collapsed", true);
    await expect(page.locator('ga-app aside.sidebar ga-section[heading="Control Room"]')).toHaveJSProperty("collapsed", false);
    expect(await page.evaluate(() => localStorage.getItem("gazelle.layout.panels"))).toBeNull();
    await page.evaluate(() => localStorage.removeItem("gazelle.layout.sidebar"));
  });

  test("the lower zone is left for the mixer dock", async ({ page }) => {
    await page.setViewportSize({ width: 1400, height: 860 });
    await page.goto(`${server.url}/#/devices/loopback-0`);
    const lower = page.locator("ga-app footer ga-mixer-dock");
    await expect(lower).toBeVisible();
    const [footer, viewport] = [await box(page, "ga-app footer"), page.viewportSize()];
    expect(footer.y + footer.height).toBeLessThanOrEqual((viewport?.height ?? 0) + 1);
  });
});

test.describe("on a tablet", () => {
  test.use({ viewport: { width: 768, height: 1024 }, hasTouch: true });

  test("no page scrolls sideways, and the sidebar stays docked", async ({ page }) => {
    for (const route of ROUTES) {
      await page.goto(`${server.url}/#/${route}`);
      await expect(page.locator("ga-app main .page > *")).toBeVisible();
      await page.waitForTimeout(300);
      expect(await sideways(page), route).toEqual({ page: 0, main: 0 });
    }
    await expect(page.locator("ga-app aside.sidebar ga-device-list a[data-device-id]").first()).toBeVisible();
    await expect(page.getByRole("button", { name: /open the sidebar/i })).toBeHidden();

    // The header takes two lines: the brand with the safety badges, then every page tab.
    const brand = await page.locator("ga-header .brand").boundingBox();
    const middle = (brand?.y ?? 0) + (brand?.height ?? 0) / 2;
    for (const id of ["backend", "dry-run", "connection"]) {
      const badge = await page.getByTestId(id).boundingBox();
      expect(Math.abs((badge?.y ?? 0) + (badge?.height ?? 0) / 2 - middle), `${id} is on the brand's line`).toBeLessThan(8);
    }
    const nav = page.getByRole("navigation", { name: "Pages" });
    expect((await nav.boundingBox())?.y ?? 0).toBeGreaterThan((brand?.y ?? 0) + (brand?.height ?? 0) - 1);
    for (const link of await nav.getByRole("link").all()) await expect(link).toBeInViewport({ ratio: 1 });
  });
});

test.describe("on a phone", () => {
  test.use({ viewport: { width: 375, height: 812 }, isMobile: true, hasTouch: true, deviceScaleFactor: 2 });

  test("no page scrolls sideways; wide content scrolls in its own box", async ({ page }) => {
    for (const route of ROUTES) {
      await page.goto(`${server.url}/#/${route}`);
      await expect(page.locator("ga-app main .page > *")).toBeVisible();
      await page.waitForTimeout(300);
      expect(await sideways(page), route).toEqual({ page: 0, main: 0 });
      expect(await page.evaluate(() => window.innerWidth), `${route}: the layout viewport is not widened`).toBe(375);
    }
    await page.goto(`${server.url}/#/mixer/loopback-0`);
    await expect(page.getByTestId("fader-17")).toBeAttached();
    expect(await page.locator("ga-mixer .strips").evaluate((el) => el.scrollWidth - el.clientWidth)).toBeGreaterThan(100);
    await page.goto(`${server.url}/#/routing/loopback-1`);
    await expect(page.getByTestId("source-0-0")).toBeVisible();
    expect(await page.locator("ga-routing .table").first().evaluate((el) => el.scrollWidth - el.clientWidth)).toBeGreaterThan(100);
  });

  test("every page link is in the header and reachable", async ({ page }) => {
    await page.goto(`${server.url}/#/devices/loopback-0`);
    const nav = page.getByRole("navigation", { name: "Pages" });
    const header = await box(page, "ga-header");
    expect(header.width).toBeLessThanOrEqual(375);
    for (const label of ["Devices", "Workspace", "Inputs", "Outputs", "Mixer", "Routing"]) {
      const link = nav.getByRole("link", { name: label });
      await link.scrollIntoViewIfNeeded();
      await expect(link).toBeInViewport();
    }
    await nav.getByRole("link", { name: "Routing" }).tap();
    await expect(page).toHaveURL(/#\/routing/);
    await expect(page.getByTestId("backend")).toBeInViewport();
    await expect(page.getByTestId("connection")).toBeInViewport();
  });

  test("the sidebar is a drawer: closed at first, opened from the header, closed by Escape, a tap outside or a device picked", async ({ page }) => {
    await page.goto(`${server.url}/#/devices/loopback-0`);
    const sidebar = page.locator("ga-app aside.sidebar");
    const menu = page.getByRole("button", { name: "Open the sidebar" });
    await expect(sidebar).not.toBeInViewport();
    await expect(sidebar.locator("ga-device-list a[data-device-id]").first()).toBeHidden();
    await expect(menu).toHaveAttribute("aria-expanded", "false");

    await menu.tap();
    await expect(menu).toHaveAttribute("aria-expanded", "true");
    await expect(sidebar.locator("ga-device-list a[data-device-id]").first()).toBeInViewport();
    await expect(sidebar.locator("ga-control-room")).toBeAttached();
    const drawer = await box(page, "ga-app aside.sidebar");
    expect(drawer.x + drawer.width, "it comes in from the right").toBeGreaterThan(370);
    expect(drawer.width).toBeLessThan(375);
    await expect(page.getByRole("button", { name: "Close the sidebar" })).toBeFocused();

    await page.keyboard.press("Escape");
    await expect(sidebar).not.toBeInViewport();
    await expect(menu).toBeFocused();
    await expect(menu).toHaveAttribute("aria-expanded", "false");

    await menu.tap();
    await expect(sidebar.locator("ga-device-list a[data-device-id]").first()).toBeInViewport();
    await page.touchscreen.tap(20, 400);
    await expect(sidebar).not.toBeInViewport();

    await menu.tap();
    await sidebar.locator('ga-device-list a[data-device-id="loopback-1"]').tap();
    await expect(page).toHaveURL(/#\/devices\/loopback-1/);
    await expect(sidebar).not.toBeInViewport();

    // Moved to the left, it comes in from the left.
    await menu.tap();
    await page.getByRole("button", { name: "Move the sidebar to the left" }).tap();
    await expect.poll(async () => (await box(page, "ga-app aside.sidebar")).x).toBeLessThanOrEqual(1);
    await page.keyboard.press("Escape");
    await expect(sidebar).not.toBeInViewport();
    await menu.tap();
    await page.getByRole("button", { name: "Move the sidebar to the right" }).tap();
    await page.keyboard.press("Escape");
  });
});
