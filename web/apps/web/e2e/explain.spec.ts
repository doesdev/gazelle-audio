// The explain mode (the user, 2026-09-18): off by default, turned on from the header and remembered
// per browser. While it is on, hovering or focusing a control, readout, badge or section heading shows
// a small panel saying what it is, what changing it does and what to watch for; on a touch screen a tap
// on the info button makes the next taps explain instead of act. The panel never covers what it
// explains, never takes a click, drag or wheel from a control, and hides while one is dragged. Its text
// is a chunk of its own, fetched the first time the mode is turned on.

import { expect, test, type Locator, type Page } from "@playwright/test";

import { startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { putWorkspace, resetWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--backend", "loopback", "--dry-run"], { webUi: true });
});

test.beforeEach(() => resetWorkspace(server));

test.afterAll(async () => {
  await server?.stop();
});

const MIXERS = {
  "loopback-0": { mixes: [{ name: "Monitors" }], channels: [{ id: "a", name: "Vox", slot: 6, sends: [], source: { group: 0, channel: 0 }, main_mix: 0 }] },
};

interface Frame {
  command?: string;
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

/** Requests for the explanations' chunk, by its module name in the built file's name. */
function catalogueRequests(page: Page): string[] {
  const seen: string[] = [];
  page.on("request", (request) => {
    if (/explain-catalogue/.test(request.url())) seen.push(request.url());
  });
  return seen;
}

const toggle = (page: Page) => page.getByRole("button", { name: "Explain mode" });
const panel = (page: Page) => page.getByTestId("explain-panel");

type Box = { x: number; y: number; width: number; height: number };
const overlaps = (a: Box, b: Box) => a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;

async function box(locator: Locator): Promise<Box> {
  const found = await locator.boundingBox();
  if (found === null) throw new Error("no box");
  return found;
}

test("off by default: nothing shows and the explanations are not fetched; turned on, a hover explains without covering the control, and a reload keeps it on", async ({ page }) => {
  await putWorkspace(server, { mixers: MIXERS });
  const requests = catalogueRequests(page);
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  const fader = page.getByTestId("fader-6");
  await expect(fader).toBeVisible();
  await expect(toggle(page)).toHaveAttribute("aria-pressed", "false");

  await fader.hover();
  await page.waitForTimeout(700);
  await expect(panel(page)).toBeHidden();
  expect(requests, "nothing is fetched while the mode is off").toEqual([]);

  await toggle(page).click();
  await expect(toggle(page)).toHaveAttribute("aria-pressed", "true");
  await expect.poll(() => requests.length, "turning it on fetches the explanations").toBe(1);

  await page.mouse.move(5, 5);
  await fader.hover();
  await expect(panel(page)).toBeVisible();
  await expect(panel(page)).toContainText("Vox");
  await expect(panel(page)).toContainText(/fader/i);
  expect(overlaps(await box(panel(page)), await box(fader)), "the panel does not cover the fader").toBe(false);
  expect(await panel(page).evaluate((el) => getComputedStyle(el).pointerEvents), "and takes no pointer events").toBe("none");

  // A control's own tooltip says what it is doing now: the panel shows it last, and borrows it from
  // the control while it is up, so the browser's tooltip does not land on top; it goes back after.
  const mono = page.getByTestId("mix-mono-0");
  const own = await mono.getAttribute("title");
  expect(own).toMatch(/mono/i);
  await mono.hover();
  await expect(panel(page)).toContainText(own ?? "");
  await expect(mono).not.toHaveAttribute("title", /./);
  await page.mouse.move(5, 5);
  await expect(panel(page)).toBeHidden();
  await expect(mono).toHaveAttribute("title", own ?? "");

  await page.reload();
  await expect(page.getByTestId("fader-6")).toBeVisible();
  await expect(toggle(page), "remembered per browser").toHaveAttribute("aria-pressed", "true");
  await page.getByTestId("fader-6").hover();
  await expect(panel(page)).toBeVisible();

  await toggle(page).click();
  await expect(toggle(page)).toHaveAttribute("aria-pressed", "false");
  await expect(panel(page)).toBeHidden();
  await page.mouse.move(5, 5);
  await page.getByTestId("fader-6").hover();
  await page.waitForTimeout(700);
  await expect(panel(page), "off again, nothing shows").toBeHidden();
});

// The user, 2026-09-19: the version should be findable from the tray or the app, "to the left of the
// USB/backend indicator in a dark font for minimal intrusiveness". It is the server's own version, the
// one `/api/v1/health` reports and the tray menu names.
test("the running version reads out left of the backend badge while the explain mode is on, and nothing shows while it is off", async ({ page }) => {
  const running = ((await (await fetch(`${server.url}/api/v1/health`)).json()) as { version: string }).version;
  expect(running, "the server names a version").toMatch(/^\d+\.\d+\.\d+/);

  await page.goto(server.url);
  const version = page.getByTestId("version");
  const backend = page.getByTestId("backend");
  // Measured once the server has said hello, so what moves afterwards is the explain mode's doing.
  await expect(backend).toHaveText("loopback");
  await expect(version, "out of the way until the explanations are asked for").toBeHidden();
  // The readout keeps its place in the line whether it is shown or not, so it is measured through the
  // header's shadow root: a hidden element has no box of Playwright's.
  const places = () =>
    page.evaluate(() => {
      // The header lives in the app's shadow root, and its own root holds the bar.
      const bar = document.querySelector("ga-app")?.shadowRoot?.querySelector("ga-header")?.shadowRoot;
      const rect = (selector: string) => {
        const found = bar?.querySelector(selector);
        if (found === null || found === undefined) throw new Error(`no ${selector} in the header`);
        const { x, width } = found.getBoundingClientRect();
        return { x, width };
      };
      return { version: rect('[data-testid="version"]'), backend: rect('[data-testid="backend"]') };
    });
  const before = await places();
  expect(before.version.width, "it takes its place before it is shown").toBeGreaterThan(0);

  await toggle(page).click();
  await expect(version).toBeVisible();
  await expect(version).toHaveText(running);
  const after = await places();
  expect(after.version.x + after.version.width, "it sits to the left of the backend badge").toBeLessThanOrEqual(after.backend.x);
  expect(after.version.width, "showing it changes no width, so nothing slides along").toBe(before.version.width);
  expect(after.backend.x - after.version.x, "the badge keeps its place beside the readout").toBe(before.backend.x - before.version.x);

  await version.hover();
  await expect(panel(page)).toBeVisible();
  await expect(panel(page)).toContainText("Version");
  await expect(panel(page)).toContainText(/which version of gazelle/i);

  await toggle(page).click();
  await expect(version, "off again, it goes").toBeHidden();
});

test("a drag on a fader moves it and hides the panel while it lasts; the wheel still moves it", async ({ page }) => {
  await putWorkspace(server, { mixers: MIXERS });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  await toggle(page).click();
  const fader = page.getByTestId("fader-6");
  await fader.hover();
  await expect(panel(page)).toBeVisible();

  const at = await box(fader);
  const before = await fader.getAttribute("aria-valuenow");
  await page.mouse.move(at.x + at.width / 2, at.y + 20);
  await page.mouse.down();
  await expect(panel(page), "hidden as soon as the press lands").toBeHidden();
  await page.mouse.move(at.x + at.width / 2, at.y + at.height / 2, { steps: 5 });
  await page.waitForTimeout(600);
  await expect(panel(page), "and not shown while the drag lasts").toBeHidden();
  await page.mouse.move(at.x + at.width / 2, at.y + at.height - 10, { steps: 5 });
  await page.mouse.up();
  await expect(fader).not.toHaveAttribute("aria-valuenow", before ?? "");

  const dragged = Number(await fader.getAttribute("aria-valuenow"));
  const scroll = await page.evaluate(() => window.scrollY);
  await page.mouse.move(at.x + at.width / 2, at.y + at.height / 2);
  await page.mouse.wheel(0, -100);
  await expect.poll(async () => Number(await fader.getAttribute("aria-valuenow")), "the wheel still reaches the fader").toBeGreaterThan(dragged);
  expect(await page.evaluate(() => window.scrollY)).toBe(scroll);
});

test("the keyboard: focus shows the panel, Escape hides it and leaves focus where it was", async ({ page }) => {
  await putWorkspace(server, { mixers: MIXERS });
  await page.goto(`${server.url}/#/outputs/loopback-0`);
  await expect(page.getByTestId("out-mute-0")).toBeVisible();
  await toggle(page).click();
  await page.mouse.move(5, 5);

  await page.getByTestId("out-volume-0").focus();
  await page.keyboard.press("Shift+Tab");
  await page.keyboard.press("Tab");
  await expect(panel(page)).toBeVisible();
  await expect(panel(page)).toContainText("Monitor");
  await page.keyboard.press("Escape");
  await expect(panel(page)).toBeHidden();
  expect(await page.getByTestId("out-volume-0").evaluate((el) => el.matches(":focus"))).toBe(true);

  await page.keyboard.press("Tab");
  await expect(panel(page), "the next control explains itself in turn").toBeVisible();
  await expect(panel(page)).toContainText(/mute/i);
  const mute = await box(page.getByTestId("out-mute-0"));
  expect(overlaps(await box(panel(page)), mute)).toBe(false);
});

test("a preamp's controls are explained by name, with what to watch for on 48V", async ({ page }) => {
  await page.goto(`${server.url}/#/inputs/loopback-1`);
  await toggle(page).click();
  await page.getByTestId("pre-gain-2").hover();
  await expect(panel(page)).toContainText("Preamp 3");
  await page.getByTestId("pre-48v-0").hover();
  await expect(panel(page)).toContainText("Preamp 1");
  await expect(panel(page)).toContainText(/ribbon/i);
});

test.describe("on a phone", () => {
  test.use({ viewport: { width: 375, height: 812 }, isMobile: true, hasTouch: true, deviceScaleFactor: 2 });

  // The header's top line is already full on a phone: the brand, the badges, the connection dot and
  // two buttons. The version stays off it; the tray menu and /api/v1/health still say it.
  test("the version readout stays off the header, even with the explain mode on", async ({ page }) => {
    await page.goto(server.url);
    await expect(page.getByTestId("backend")).toBeVisible();
    await expect(page.getByTestId("version")).toBeHidden();
    await toggle(page).tap();
    await expect(page.getByRole("button", { name: "Explain by tapping" })).toBeVisible();
    await expect(page.getByTestId("version")).toBeHidden();
  });

  test("a tap on the info button makes the next taps explain instead of act, and a tap on it again gives the controls back", async ({ page }) => {
    const frames = recordFrames(page);
    await page.goto(`${server.url}/#/outputs/loopback-0`);
    const mute = page.getByTestId("out-mute-0");
    await expect(mute).toBeVisible();
    const pick = page.getByRole("button", { name: "Explain by tapping" });
    await expect(pick, "only while the mode is on").toBeHidden();
    await toggle(page).tap();
    await expect(pick).toBeVisible();

    await pick.tap();
    await expect(pick).toHaveAttribute("aria-pressed", "true");
    await mute.tap();
    await expect(panel(page)).toBeVisible();
    await expect(panel(page)).toContainText(/mute/i);
    await expect(mute, "the tap explained the button and did not press it").toHaveAttribute("aria-pressed", "false");
    await page.waitForTimeout(300);
    expect(frames.filter((f) => f.command === "set_mute"), "nothing was sent").toEqual([]);
    expect(overlaps(await box(panel(page)), await box(mute))).toBe(false);
    const volume = page.getByTestId("out-volume-0");
    const level = await volume.getAttribute("aria-valuenow");
    await volume.tap();
    await expect(panel(page)).toContainText(/volume/i);
    await expect(volume, "a slider tapped is explained, not moved").toHaveAttribute("aria-valuenow", level ?? "");

    await pick.tap();
    await expect(pick).toHaveAttribute("aria-pressed", "false");
    await expect(panel(page)).toBeHidden();
    await mute.tap();
    await expect(mute, "the controls are back").toHaveAttribute("aria-pressed", "true");
    await expect.poll(() => frames.filter((f) => f.command === "set_mute").length).toBe(1);
  });
});
