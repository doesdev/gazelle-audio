// Pages that only one route shows are not loaded with the app: the route fetches the page's chunk
// when it opens (elements/lazy.ts). Holding that request back, or refusing it, shows what the shell
// does meanwhile, which no other test can see — on this machine the chunk is there long before
// anything is looked at.

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

/** Holds every request for `chunk`'s file back until the returned function is called. */
async function hold(page: Page, chunk: string): Promise<{ arrive: () => void; requests: () => number }> {
  let arrive = () => {};
  const held = new Promise<void>((done) => (arrive = done));
  let requests = 0;
  await page.route(`**/assets/${chunk}-*.js`, async (route) => {
    requests++;
    await held;
    await route.continue();
  });
  return { arrive, requests: () => requests };
}

test("a page waits for its chunk behind a line saying so, and builds as soon as it is here", async ({ page }) => {
  const effects = await hold(page, "effects-page");

  await page.goto(`${server.url}/#/effects/loopback-0`);
  await expect(page.getByTestId("page-loading")).toHaveText("Loading the Effects page…");
  await expect(page.locator("ga-effects")).toHaveCount(0);
  expect(effects.requests(), "and it is fetched by the route, not with the app").toBe(1);
  // The shell is not waiting with it: the header, the sidebar and the dock are all there.
  await expect(page.locator('ga-header nav a[data-page="mixer"]')).toBeVisible();
  await expect(page.locator("ga-device-list")).toBeVisible();
  await expect(page.locator("ga-mixer-dock")).toBeVisible();

  effects.arrive();
  await expect(page.locator("ga-effects")).toBeVisible();
  await expect(page.getByTestId("page-loading")).toHaveCount(0);
});

test("a route left before its chunk arrives is not pushed aside by it", async ({ page }) => {
  const effects = await hold(page, "effects-page");

  await page.goto(`${server.url}/#/effects/loopback-0`);
  await expect(page.getByTestId("page-loading")).toBeVisible();
  await page.locator('ga-header nav a[data-page="routing"]').click();
  await expect(page.locator("ga-routing")).toBeVisible();

  effects.arrive();
  // Give the late chunk every chance to land on top of the page that is shown.
  await page.waitForTimeout(500);
  await expect(page.locator("ga-routing")).toBeVisible();
  await expect(page.locator("ga-effects")).toHaveCount(0);
});

test("a chunk that never arrives leaves a message and a way to try again, not an empty page", async ({ page }) => {
  let refuse = true;
  await page.route("**/assets/effects-page-*.js", async (route) => {
    if (refuse) await route.abort("failed");
    else await route.continue();
  });

  await page.goto(`${server.url}/#/effects/loopback-0`);
  const failed = page.getByTestId("page-failed");
  await expect(failed).toBeVisible();
  await expect(failed).toContainText("The Effects page could not be loaded");
  await expect(page.locator("ga-effects")).toHaveCount(0);
  // The page can still be left, so a failed chunk is not a dead end.
  await expect(page.locator('ga-header nav a[data-page="routing"]')).toBeVisible();

  // Try again reloads: a browser will not fetch a module file it has already failed on, so asking
  // for it again in this document would fail at once however well the network is by then.
  refuse = false;
  await page.getByTestId("page-retry").click();
  await expect(page.locator("ga-effects")).toBeVisible();
  await expect(page.getByTestId("page-failed")).toHaveCount(0);
});

test("the dock waits for the surface strip the same way, and the rest of the dock is unaffected", async ({ page }) => {
  await putWorkspace(server, {
    mixers: { "loopback-0": { mixes: [{ name: "Monitors" }], groups: [], channels: [{ id: "vox", name: "Vox", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }] } },
    surfaces: [{ id: "s", name: "Cue rig", mixes: {}, strips: [{ id: "a", kind: "channel", device_id: "loopback-0", channel: "vox" }] }],
  });
  const strip = await hold(page, "surface-strip");

  await page.goto(`${server.url}/#/routing/loopback-0`);
  await page.getByTestId("dock-source-select").selectOption("s");
  await expect(page.getByTestId("dock-loading")).toHaveText("Loading Cue rig…");
  await expect(page.locator("ga-surface-strip")).toHaveCount(0);
  expect(strip.requests()).toBe(1);

  strip.arrive();
  await expect(page.locator("ga-surface-strip")).toHaveCount(1);
  await expect(page.getByTestId("dock-loading")).toHaveCount(0);
});

// The surface page and the dock both build surface strips, and the page's chunk defines the strip
// as well as the page: with both waiting on the same file, whichever is served first defines it and
// the other must find it already there rather than defining it twice, which throws.
test("the dock and the surface page waiting on the same chunk at once both get it, once", async ({ page }) => {
  await putWorkspace(server, {
    mixers: { "loopback-0": { mixes: [{ name: "Monitors" }], groups: [], channels: [{ id: "vox", name: "Vox", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }] } },
    surfaces: [
      { id: "s", name: "Cue rig", mixes: {}, strips: [{ id: "a", kind: "channel", device_id: "loopback-0", channel: "vox" }] },
      { id: "t", name: "Monitors rig", mixes: {}, strips: [{ id: "b", kind: "channel", device_id: "loopback-0", channel: "vox" }] },
    ],
  });
  const strip = await hold(page, "surface-strip");
  const failures: string[] = [];
  page.on("pageerror", (error) => failures.push(error.message));

  await page.goto(`${server.url}/#/routing/loopback-0`);
  await page.getByTestId("dock-source-select").selectOption("s");
  await expect(page.getByTestId("dock-loading")).toBeVisible();
  await page.locator('ga-header nav a[data-page="workspace"]').click();
  await page.goto(`${server.url}/#/surface/t`);
  await expect(page.getByTestId("page-loading")).toBeVisible();

  strip.arrive();
  // The surface page's strip and the dock's, both built from the one chunk.
  await expect(page.locator("ga-surface ga-surface-strip")).toHaveCount(1);
  await expect(page.locator("ga-mixer-dock ga-surface-strip")).toHaveCount(1);
  expect(strip.requests(), "and the file is asked for once, however many are waiting").toBe(1);
  expect(failures).toEqual([]);
});
