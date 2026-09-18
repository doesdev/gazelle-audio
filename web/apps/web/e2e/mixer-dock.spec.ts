// The compact mixer dock under every page (the user's choice, 2026-09-16): the device in view's
// selected mix as slim strips and that mix's master, so levels can be ridden from Inputs or
// Routing. It follows the device cards and the Mix menu, sends the same commands as the Mixer
// page's strips, and is hidden on the Mixer page, where it would repeat what is on screen.

import { expect, test, type Page } from "@playwright/test";

import { startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { putWorkspace, resetWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--backend", "loopback", "--dry-run", "--loopback-cyclic-ms", "50"], { webUi: true });
});

test.beforeEach(() => resetWorkspace(server));

test.afterAll(async () => {
  await server?.stop();
});

interface Frame {
  command?: string;
  args?: Record<string, number>;
}

function recordFrames(page: Page): Frame[] {
  const frames: Frame[] = [];
  page.on("websocket", (socket) => {
    socket.on("framesent", (event) => {
      if (typeof event.payload === "string") frames.push(JSON.parse(event.payload) as Frame);
    });
  });
  return frames;
}

const channel = (id: string, name: string, slot: number, main: number | undefined, sends: number[] = [], source: number | undefined = 0) => ({
  id,
  name,
  slot,
  sends,
  ...(source === undefined ? {} : { source: { group: 0, channel: source } }),
  ...(main === undefined ? {} : { main_mix: main }),
});

/** Two devices with channels in different mixes: the Quadro's order differs from its slots. */
const MIXERS = {
  "loopback-0": {
    mixes: [{ name: "Monitors" }, { name: "Cue" }],
    groups: [{ id: "g", name: "Drums", collapsed: false, color: "#b5473a" }],
    channels: [channel("b", "Guitar", 8, 0, [], 1), { ...channel("a", "Vox", 6, 0, [1], 0), group: "g" }, channel("c", "Click", 7, 1, [], 2), channel("d", "", 9, undefined, [], undefined)],
  },
  "loopback-1": { mixes: [{ name: "Main" }], channels: [channel("k", "Kick", 0, 0), channel("s", "Snare", 1, 0, [], 1)] },
};

const dock = (page: Page) => page.locator("ga-mixer-dock");
/** The dock's channel strips' mixer inputs, in the order shown. */
const slots = (page: Page) => dock(page).locator('ga-strip:not([strip="master"])').evaluateAll((strips) => strips.map((s) => s.getAttribute("strip")));

test("on the Inputs page the dock shows the selected mix's channels for the device in view, in the Mixer page's order, and its master", async ({ page }) => {
  await putWorkspace(server, { mixers: MIXERS });
  await page.goto(`${server.url}/#/inputs/loopback-0`);
  await expect(page.locator("ga-inputs")).toBeVisible();
  await expect(dock(page)).toBeVisible();
  // Mix 1 (Monitors): Guitar and Vox, as laid out; Click is in Cue only and d is not set up.
  await expect.poll(() => slots(page)).toEqual(["8", "6"]);
  const strips = dock(page).locator("ga-strip");
  for (const strip of await strips.all()) {
    await expect(strip).toHaveAttribute("compact", "");
    await expect(strip).toHaveAttribute("device-id", "loopback-0");
    await expect(strip).toHaveAttribute("mixer", "0");
  }
  await expect(dock(page).locator('ga-strip[strip="6"]')).toHaveAttribute("label", "Vox");
  await expect(dock(page).locator('ga-strip[strip="6"]')).toHaveAttribute("color", "#b5473a");
  await expect(dock(page).locator('ga-strip[strip="6"]')).toHaveAttribute("input-group", "0");
  await expect(dock(page).locator('ga-strip[strip="master"]')).toHaveAttribute("label", "Monitors");
  // Compact: fader, meter, mute and solo, but no pan, send or link.
  await expect(dock(page).getByTestId("fader-6")).toBeVisible();
  await expect(dock(page).getByTestId("meter-6")).toBeVisible();
  await expect(dock(page).getByRole("button", { name: "Vox mute" })).toBeVisible();
  await expect(dock(page).getByRole("button", { name: "Vox solo" })).toBeVisible();
  await expect(dock(page).getByTestId("pan-6")).toHaveCount(0);
  await expect(dock(page).getByTestId("mixer-link-6")).toHaveCount(0);
  const box = await dock(page).locator('ga-strip[strip="6"]').boundingBox();
  expect(box?.width).toBeGreaterThanOrEqual(40);
  expect(box?.width).toBeLessThanOrEqual(48);
  expect(box?.height, "tall enough for a usable fader").toBeGreaterThanOrEqual(150);

  const shot = process.env["GAZELLE_DOCK_SCREENSHOT"];
  if (shot !== undefined && shot !== "") {
    await page.evaluate(() => document.fonts.ready);
    await page.screenshot({ path: shot });
  }
});

test("moving a dock fader sends set_mixer for the selected mix, and the Mixer page shows the new level", async ({ page }) => {
  const frames = recordFrames(page);
  await putWorkspace(server, { mixers: MIXERS });
  await page.goto(`${server.url}/#/routing/loopback-0`);
  // The dock's own Mix menu picks Cue, where Vox is a send and Click the main mix.
  await dock(page).getByTestId("dock-mix-select").selectOption("1");
  await expect.poll(() => slots(page)).toEqual(["6", "7"]);
  const fader = dock(page).getByTestId("fader-7");
  await expect(fader).toHaveAttribute("aria-disabled", "false");
  await fader.focus();
  for (let i = 0; i < 3; i++) await fader.press("PageDown");
  await expect(dock(page).getByTestId("level-7")).toHaveText("-18 dB");
  // Quadro mixer input 8 is channel 8 of mix 2.
  await expect.poll(() => frames.filter((f) => f.command === "set_mixer").at(-1)?.args).toMatchObject({ mixer_id: 1, channel: 8, level: 18 });

  await page.locator('ga-header nav a[data-page="mixer"]').click();
  await expect(page.getByTestId("mix-select")).toHaveValue("1");
  await expect(page.locator("ga-mixer").getByTestId("level-7")).toHaveText("-18 dB");
});

test("the dock follows a device picked from the cards and a mix picked on the Mixer page", async ({ page }) => {
  await putWorkspace(server, { mixers: MIXERS });
  await page.goto(`${server.url}/#/inputs/loopback-0`);
  await expect.poll(() => slots(page)).toEqual(["8", "6"]);

  await page.locator('ga-device-list a[data-device-id="loopback-1"]').click();
  await expect(page).toHaveURL(/#\/inputs\/loopback-1$/);
  await expect.poll(() => slots(page)).toEqual(["0", "1"]);
  await expect(dock(page).locator("ga-strip").first()).toHaveAttribute("device-id", "loopback-1");
  await expect(dock(page).locator('ga-strip[strip="master"]')).toHaveAttribute("label", "Main");

  await page.locator('ga-device-list a[data-device-id="loopback-0"]').click();
  await expect.poll(() => slots(page)).toEqual(["8", "6"]);
  await page.locator('ga-header nav a[data-page="mixer"]').click();
  await page.getByTestId("mix-select").selectOption("1");
  await page.locator('ga-header nav a[data-page="outputs"]').click();
  await expect(page.locator("ga-outputs")).toBeVisible();
  await expect.poll(() => slots(page)).toEqual(["6", "7"]);
  await expect(dock(page).getByTestId("dock-mix-select")).toHaveValue("1");
  await expect(dock(page).locator('ga-strip[strip="master"]')).toHaveAttribute("label", "Cue");
});

test("the dock is hidden on the Mixer page, and stays collapsed across a reload once collapsed", async ({ page }) => {
  await putWorkspace(server, { mixers: MIXERS });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  await expect(page.locator("ga-mixer ga-channel").first()).toBeVisible();
  await expect(dock(page)).toBeHidden();
  await expect(dock(page).locator("ga-strip"), "nothing is built for it there").toHaveCount(0);

  await page.locator('ga-header nav a[data-page="inputs"]').click();
  await expect(dock(page)).toBeVisible();
  await expect.poll(() => slots(page)).toEqual(["8", "6"]);
  await dock(page).getByRole("button", { name: "Mixer", exact: true }).click();
  await expect(dock(page).locator("ga-strip")).toHaveCount(0);
  await page.reload();
  await expect(page.locator("ga-inputs")).toBeVisible();
  await expect(dock(page).getByRole("button", { name: "Mixer", exact: true })).toHaveAttribute("aria-expanded", "false");
  await expect(dock(page).locator("ga-strip")).toHaveCount(0);
  await dock(page).getByRole("button", { name: "Mixer", exact: true }).click();
  await expect.poll(() => slots(page)).toEqual(["8", "6"]);
});

test("a mix with no channel set up is one short line, without the master, and the strips come back with a mix that has some", async ({ page }) => {
  await putWorkspace(server, { mixers: { "loopback-0": { mixes: [{ name: "Monitors" }, { name: "Cue" }], channels: [channel("c", "Click", 7, 1, [], 2)] } } });
  await page.goto(`${server.url}/#/inputs/loopback-0`);
  const empty = dock(page).getByText("No channels in Monitors.");
  await expect(empty).toBeVisible();
  await expect(dock(page).getByRole("link", { name: "Open the Mixer page" })).toHaveAttribute("href", "#/mixer/loopback-0");
  await expect(dock(page).locator("ga-strip"), "no master for a mix with nothing in it").toHaveCount(0);
  const row = await dock(page).getByTestId("dock-strips").boundingBox();
  expect(row?.height, "the empty row is one line").toBeLessThanOrEqual(32);
  expect((await dock(page).boundingBox())?.height, "the whole dock stays short").toBeLessThanOrEqual(80);

  await dock(page).getByTestId("dock-mix-select").selectOption("1");
  await expect.poll(() => slots(page)).toEqual(["7"]);
  await expect(dock(page).locator('ga-strip[strip="master"]')).toHaveAttribute("label", "Cue");
  await expect(empty).toHaveCount(0);
  expect((await dock(page).getByTestId("dock-strips").boundingBox())?.height).toBeGreaterThanOrEqual(150);
});

test("a mix wider than the window scrolls inside the dock, not the page (the user, 2026-09-16)", async ({ page }) => {
  // 26 channels at 46 px a strip: wider than a 900 px window.
  const many = Array.from({ length: 26 }, (_, i) => channel(`n${i}`, `Ch ${i}`, 6 + i, 0, [], i % 4));
  await putWorkspace(server, { mixers: { "loopback-0": { channels: many } } });
  await page.setViewportSize({ width: 900, height: 800 });
  await page.goto(`${server.url}/#/inputs/loopback-0`);
  await expect(dock(page).locator('ga-strip:not([strip="master"])')).toHaveCount(26);
  const page_ = await page.evaluate(() => ({ scroll: document.documentElement.scrollWidth, client: document.documentElement.clientWidth }));
  expect(page_.scroll, "the page does not grow sideways").toBeLessThanOrEqual(page_.client);
  const app = await page.locator("ga-app").evaluate((el) => el.getBoundingClientRect().width);
  expect(app, "nor does the shell").toBeLessThanOrEqual(900);
  const strips = await dock(page).locator(".strips").evaluate((el) => ({ scroll: el.scrollWidth, client: el.clientWidth }));
  expect(strips.scroll, "the dock's strips overflow their own row").toBeGreaterThan(strips.client);
  await expect(dock(page).locator('ga-strip[strip="master"]')).toBeInViewport();
});

test.describe("on a phone", () => {
  test.use({ viewport: { width: 375, height: 812 }, isMobile: true, hasTouch: true, deviceScaleFactor: 2 });

  test("the dock starts collapsed, since open it takes a quarter of the screen, and stays open once opened", async ({ page }) => {
    await putWorkspace(server, { mixers: MIXERS });
    await page.goto(`${server.url}/#/inputs/loopback-0`);
    await expect(page.locator("ga-inputs")).toBeVisible();
    const toggle = dock(page).getByRole("button", { name: "Mixer", exact: true });
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
    await expect(dock(page).locator("ga-strip"), "nothing is built for it collapsed").toHaveCount(0);
    await expect(dock(page).getByTestId("dock-mix-select"), "nor an empty Mix menu in its bar").toBeHidden();
    expect((await dock(page).boundingBox())?.height).toBeLessThanOrEqual(48);

    await toggle.tap();
    await expect.poll(() => slots(page)).toEqual(["8", "6"]);
    await expect(dock(page).getByTestId("dock-mix-select")).toHaveValue("0");
    await page.reload();
    await expect(page.locator("ga-inputs")).toBeVisible();
    await expect(toggle, "a choice kept wins over the phone's default").toHaveAttribute("aria-expanded", "true");
    await expect.poll(() => slots(page)).toEqual(["8", "6"]);
  });

  test("a mix with no channel set up still fits one line", async ({ page }) => {
    await putWorkspace(server, { mixers: { "loopback-0": { mixes: [{ name: "Monitors" }], channels: [] } } });
    await page.goto(`${server.url}/#/inputs/loopback-0`);
    await dock(page).getByRole("button", { name: "Mixer", exact: true }).tap();
    await expect(dock(page).getByRole("link", { name: "Open the Mixer page" })).toBeVisible();
    expect((await dock(page).getByTestId("dock-strips").boundingBox())?.height, "one line at 375 px").toBeLessThanOrEqual(32);
  });
});
