// The recording widget (#/widget) and the recording hub (#/hub), end to end in the browser, and the
// Recording page's widget, hub and auto-arm settings. In the desktop app the widget and the hub are
// windows of their own; the test server runs --no-tray, so it has no windows, and these are the
// same pages a browser gets. What only the windows do (drag, close, leave full screen) is asked of
// the window over Wry's IPC, which is stubbed here to see what the page asks.
//
// Screenshots for the report are written when asked for (RECORDING_SCREENSHOTS=<directory>).

import { expect, test, type Page } from "@playwright/test";

import { startServer, TEST_PEER_HEADER, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { putWorkspace } from "./workspace.ts";

let server: RunningServer;

const QUADRO = "loopback-0";
const STUDIO = "loopback-1";
const SHOTS = process.env["RECORDING_SCREENSHOTS"];
const WIDGET = { width: 320, height: 140 };
const HUB = { width: 1920, height: 1080 };

test.beforeAll(async () => {
  server = await startServer(["--backend", "loopback", "--loopback-cyclic-ms", "200"], { webUi: true, phoneSeams: true });
});

test.afterAll(async () => {
  await server?.stop();
});

const api = (path: string, init?: RequestInit) => fetch(`${server.url}/api/v1/${path}`, init);
const post = (path: string, body: unknown = {}) => api(path, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
const put = (path: string, body: unknown) => api(path, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });

const BAND = { id: "band", name: "Band", channels: [{ device: 0, channel: 0 }, { device: 0, channel: 1 }, { device: 1, channel: 2 }, { device: 1, channel: 3 }], preroll_max_seconds: 30 };
/** A preset the aggregate refuses at Arm: the Quadro has no input 41. */
const FAR = { id: "far", name: "Far", channels: [{ device: 0, channel: 40 }] };

async function setUp(presets: unknown[] = [BAND]): Promise<void> {
  await put("recording/settings", { auto_arm: false });
  await post("recording/disarm", { confirm: true });
  await putWorkspace(server, {
    aliases: { [QUADRO]: "Quadro", [STUDIO]: "Studio+" },
    aggregate: { devices: [{ key: "Zen Quadro Synergy Core", device_id: QUADRO, input_names: { "0": "Vocal mic", "1": "Guitar" } }, { key: "ZenStudioTB", device_id: STUDIO }], alignment: "aligned" },
    ...({ recording: { presets } } as object),
  });
}

test.beforeEach(async () => {
  await setUp();
});

test.afterEach(async () => {
  await put("recording/settings", { auto_arm: false });
  await post("recording/disarm", { confirm: true });
});

/** Opens a route as its desktop window would, with the window's IPC standing in as a list of what was asked. */
async function open(page: Page, route: "widget" | "hub", options: { explained?: boolean; ipc?: boolean; theme?: string } = {}): Promise<void> {
  await page.addInitScript(
    ([explained, ipc, theme]) => {
      if (explained) localStorage.setItem("gazelle.recording.armExplained", "1");
      else localStorage.removeItem("gazelle.recording.armExplained");
      if (theme) localStorage.setItem("gazelle.theme", theme as string);
      if (ipc) {
        const asked: string[] = [];
        (window as unknown as { asked: string[] }).asked = asked;
        (window as unknown as { ipc: { postMessage(m: string): void } }).ipc = { postMessage: (m) => asked.push(m) };
      }
    },
    [options.explained ?? false, options.ipc ?? false, options.theme ?? ""] as const,
  );
  await page.goto(`${server.url}/#/${route}`);
}

const asked = (page: Page) => page.evaluate(() => (window as unknown as { asked: string[] }).asked.splice(0));

/** Nothing of the widget is cut off or scrolls: every part of it is inside the window. */
async function fits(page: Page): Promise<void> {
  const overflow = await page.evaluate(() => ({ x: document.documentElement.scrollWidth - innerWidth, y: document.documentElement.scrollHeight - innerHeight }));
  expect(overflow).toEqual({ x: 0, y: 0 });
  for (const id of ["widget-state", "widget-info", "widget-disarm"]) {
    const part = page.getByTestId(id);
    if (!(await part.isVisible())) continue;
    const box = await part.boundingBox();
    expect(box, id).not.toBeNull();
    expect((box?.x ?? 0) + (box?.width ?? 0), id).toBeLessThanOrEqual(WIDGET.width);
    expect((box?.y ?? 0) + (box?.height ?? 0), id).toBeLessThanOrEqual(WIDGET.height);
  }
}

/** A screenshot with the pointer out of the way, so no button shows as hovered. */
async function shoot(page: Page, name: string): Promise<void> {
  if (!SHOTS) return;
  await page.mouse.move(1, 1);
  await page.waitForTimeout(150);
  await page.screenshot({ path: `${SHOTS}/${name}.png` });
}

test("the widget shows the state, the preset and the pre-roll, arms after a second press the first time, records and stops, all inside 320 by 140", async ({ page }) => {
  await page.setViewportSize(WIDGET);
  await open(page, "widget");
  const state = page.getByTestId("widget-state");
  await expect(state).toHaveText("Off");
  await expect(page.getByTestId("widget-info")).toHaveText("Band");
  await expect(page.locator("ga-app")).toHaveCount(0);
  await expect(page.getByTestId("widget-close")).toBeHidden();
  await fits(page);

  // The first Arm in a browser is a second press, since there is no room to explain it.
  const arm = page.getByTestId("widget-arm");
  await expect(arm).toHaveAttribute("title", /Arm opens the interfaces/);
  await arm.click();
  await expect(arm).toHaveText("Confirm");
  await expect(state).toHaveText("Off");
  await arm.click();
  await expect(state).toHaveText("Armed");
  await expect(page.locator("ga-recording-widget")).toHaveAttribute("data-state", "armed");
  await expect(page.getByTestId("widget-info")).toContainText(/Band · [\d.]+ of 30 s held/);
  await expect(page.getByTestId("widget-arm")).toBeHidden();
  await fits(page);

  await page.getByTestId("widget-record").click();
  await expect(state).toHaveText("Recording");
  await expect(page.getByTestId("widget-elapsed")).toBeVisible();
  await expect(page.getByTestId("widget-elapsed")).not.toHaveText("0:00.0");
  await expect(page.getByTestId("widget-info")).toContainText("pre-roll");
  await fits(page);

  // Disarm while recording asks first.
  await page.getByTestId("widget-disarm").click();
  await expect(page.getByTestId("widget-disarm")).toHaveText("Confirm");
  await expect(state).toHaveText("Recording");
  await page.getByTestId("widget-stop").click();
  await expect(state).toHaveText("Armed");
  await page.getByTestId("widget-disarm").click();
  await expect(state).toHaveText("Off");
  // A second widget remembers the explanation: one press.
  await page.getByTestId("widget-arm").click();
  await expect(state).toHaveText("Armed");
});

test("the widget is dragged by its body, not its buttons, and its close button asks its window", async ({ page }) => {
  await page.setViewportSize(WIDGET);
  await open(page, "widget", { explained: true, ipc: true });
  await expect(page.getByTestId("widget-state")).toHaveText("Off");
  await asked(page);
  await page.getByTestId("widget-state").dispatchEvent("pointerdown", { button: 0 });
  await page.getByTestId("widget-info").dispatchEvent("pointerdown", { button: 0 });
  expect(await asked(page)).toEqual(["drag", "drag"]);
  await page.getByTestId("widget-arm").dispatchEvent("pointerdown", { button: 0 });
  await page.getByTestId("widget-state").dispatchEvent("pointerdown", { button: 2 });
  expect(await asked(page), "a button is pressed, and a right click is not a drag").toEqual([]);
  await expect(page.getByTestId("widget-close")).toBeVisible();
  await page.getByTestId("widget-close").click();
  expect(await asked(page)).toContain("close");
  await fits(page);
});

test("the widget says what is wrong in its warning line", async ({ page }) => {
  await setUp([{ id: "empty", name: "Empty", channels: [] }]);
  await page.setViewportSize(WIDGET);
  await open(page, "widget", { explained: true });
  await expect(page.getByTestId("widget-warning")).toHaveText("Empty records no channels yet. Tick at least one.");
  await expect(page.getByTestId("widget-arm")).toBeDisabled();
  await fits(page);
});

test("auto-arm shows on the widget, and two presses there turn it off", async ({ page }) => {
  expect((await put("recording/settings", { auto_arm: true, auto_arm_preset: "band" })).status).toBe(200);
  await page.setViewportSize(WIDGET);
  await open(page, "widget", { explained: true });
  // Turned on, it arms at its next look.
  await expect(page.getByTestId("widget-state")).toHaveText("Armed", { timeout: 10_000 });
  const auto = page.getByTestId("widget-auto-arm");
  await expect(auto).toHaveText("Auto: Band");
  await fits(page);
  await auto.click();
  await expect(auto).toHaveText("Confirm");
  await auto.click();
  await expect(auto).toBeHidden();
  expect(((await (await api("recording/settings")).json()) as { auto_arm: boolean }).auto_arm).toBe(false);
  await expect(page.getByTestId("widget-state")).toHaveText("Armed", { timeout: 1000 });
});

test("screenshots of the widget in each state, in both themes", async ({ browser }) => {
  test.skip(!SHOTS, "only when asked for");
  for (const theme of ["gazelle-dark", "gazelle-light"]) {
    await setUp();
    // A page of its own for each theme, as each is a window of its own.
    const context = await browser.newContext({ viewport: WIDGET });
    const page = await context.newPage();
    await open(page, "widget", { explained: true, ipc: true, theme });
    await expect.poll(() => page.evaluate(() => localStorage.getItem("gazelle.theme"))).toBe(theme);
    const state = page.getByTestId("widget-state");
    await expect(state).toHaveText("Off");
    await page.waitForTimeout(300);
    await shoot(page, `widget-off-${theme}`);
    await page.getByTestId("widget-arm").click();
    await expect(state).toHaveText("Armed");
    await page.waitForTimeout(1500);
    await shoot(page, `widget-armed-${theme}`);
    await page.getByTestId("widget-record").click();
    await expect(state).toHaveText("Recording");
    await page.waitForTimeout(2200);
    await shoot(page, `widget-recording-${theme}`);
    await page.getByTestId("widget-stop").click();
    await expect(state).toHaveText("Armed");
    await post("recording/disarm", { confirm: true });
    await expect(state).toHaveText("Off");

    // A warning: auto-arm on with a preset the interfaces cannot record, backing off with the reason.
    await setUp([FAR]);
    expect((await put("recording/settings", { auto_arm: true, auto_arm_preset: "far" })).status).toBe(200);
    await page.reload();
    await expect(page.getByTestId("widget-auto-arm")).toContainText("Auto: retry in", { timeout: 10_000 });
    await expect(page.getByTestId("widget-warning")).toContainText("Auto-arm could not arm with Far");
    await expect(page.getByTestId("widget-info")).toHaveText("Far");
    await page.waitForTimeout(300);
    await shoot(page, `widget-warning-${theme}`);
    await put("recording/settings", { auto_arm: false });
    await context.close();
  }
});

test("auto-arm that is refused backs off, and the widget says why", async ({ page }) => {
  await setUp([FAR]);
  expect((await put("recording/settings", { auto_arm: true, auto_arm_preset: "far" })).status).toBe(200);
  await page.setViewportSize(WIDGET);
  await open(page, "widget", { explained: true });
  await expect(page.getByTestId("widget-auto-arm")).toContainText("Auto: retry in", { timeout: 10_000 });
  await expect(page.getByTestId("widget-warning")).toContainText(/Auto-arm could not arm with Far: .*input 41 is not one of them.* It tries again in/);
  await expect(page.getByTestId("widget-state")).toHaveText("Off");
  await fits(page);
  const status = (await (await api("recording")).json()) as { auto_arm: { phase: string; failures: number } };
  expect(status.auto_arm.phase).toBe("backing_off");
  await page.waitForTimeout(2500);
  const later = (await (await api("recording")).json()) as { auto_arm: { failures: number } };
  expect(later.auto_arm.failures, "no tight retry loop").toBeLessThanOrEqual(status.auto_arm.failures + 1);
});

test("the hub shows the state and the time very large, every channel's meter by its Gazelle name, the disk and the last takes, with Space and Esc", async ({ page }) => {
  await page.setViewportSize(HUB);
  await open(page, "hub", { explained: true, ipc: true });
  const state = page.getByTestId("hub-state");
  await expect(state).toHaveText("Off");
  await expect(page.locator("ga-app")).toHaveCount(0);
  await expect(page.getByTestId("hub-preset")).toHaveText("Band");
  await expect(page.getByTestId("hub-clock")).toHaveText(/^\d\d:\d\d:\d\d$/);
  await expect(page.getByTestId("hub-disk")).toHaveText("Not armed");
  // The takes since the server started: earlier tests' are there already, newest first.
  const before = (await (await api("recording/takes")).json()) as { takes: { number: number }[] };

  await page.getByTestId("hub-arm").click();
  await expect(state).toHaveText("Armed");
  await expect(page.getByTestId("hub-channel-0")).toContainText("Vocal mic (Quadro 1)");
  await expect(page.getByTestId("hub-channel-3")).toContainText("Studio+ 4");
  await expect(page.getByTestId("hub-disk")).toHaveText(/\d/, { timeout: 5000 });
  const size = await state.evaluate((node) => parseFloat(getComputedStyle(node).fontSize));
  expect(size).toBeGreaterThan(120);
  await page.waitForTimeout(1500);

  // Space records and stops, as on the Recording page.
  await page.locator("body").click({ position: { x: 900, y: 300 } });
  await page.keyboard.press("Space");
  await expect(state).toHaveText("Recording");
  await expect(page.getByTestId("hub-elapsed")).not.toHaveText("0:00.0");
  // Disarm while recording asks first.
  await page.getByTestId("hub-disarm").click();
  await expect(page.getByTestId("hub-disarm")).toHaveText("Confirm");
  await expect(state).toHaveText("Recording");
  await page.waitForTimeout(3200);
  await expect(page.getByTestId("hub-disarm")).toHaveText("Disarm", { timeout: 1000 });
  await page.keyboard.press("Space");
  await expect(state).toHaveText("Armed");
  const first = page.getByTestId("hub-takes").locator("li").first();
  await expect.poll(async () => Number(/^T(\d{3}), /.exec((await first.textContent()) ?? "")?.[1] ?? 0), { timeout: 10_000 }).toBeGreaterThan(before.takes[0]?.number ?? 0);
  expect(await page.getByTestId("hub-takes").locator("li").count()).toBeLessThanOrEqual(4);

  // Esc asks the window to leave full screen; the button puts it back.
  await asked(page);
  await page.keyboard.press("Escape");
  expect(await asked(page)).toEqual(["windowed"]);
  await expect(page.getByTestId("hub-full-screen")).toHaveText("Full screen");
  await page.getByTestId("hub-full-screen").click();
  expect(await asked(page)).toEqual(["full-screen"]);
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - innerWidth);
  expect(overflow).toBe(0);
});

test("screenshots of the hub armed and recording", async ({ page }) => {
  test.skip(!SHOTS, "only when asked for");
  await page.setViewportSize(HUB);
  // One take to list.
  expect((await post("recording/arm", { preset: "band" })).status).toBe(200);
  await new Promise((resolve) => setTimeout(resolve, 1200));
  await post("recording/record");
  await new Promise((resolve) => setTimeout(resolve, 1500));
  await post("recording/stop");
  expect((await put("recording/settings", { auto_arm: true, auto_arm_preset: "band" })).status).toBe(200);
  await open(page, "hub", { explained: true, ipc: true });
  const state = page.getByTestId("hub-state");
  await expect(state).toHaveText("Armed");
  await expect(page.getByTestId("hub-takes").locator("li").first()).toContainText(/^T\d{3}, /, { timeout: 10_000 });
  await expect(page.getByTestId("hub-disk")).toHaveText(/\d/, { timeout: 5000 });
  await page.waitForTimeout(1500);
  await shoot(page, "hub-armed-gazelle-dark");
  await page.getByTestId("hub-record").click();
  await expect(state).toHaveText("Recording");
  await page.waitForTimeout(3300);
  await shoot(page, "hub-recording-gazelle-dark");
  await page.getByTestId("hub-stop").click();
});

test("the Recording page opens the widget and the hub where there are windows, says why not where there are none, and sets auto-arm and starting in the hub", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("gazelle.recording.armExplained", "1"));
  await page.goto(`${server.url}/#/recording`);
  const section = page.getByTestId("recording-section-computer");
  await expect(section).toBeVisible();
  // The test server runs without windows.
  await expect(page.getByTestId("recording-widget-toggle")).toBeDisabled();
  await expect(page.getByTestId("recording-hub-open")).toBeDisabled();
  await expect(page.getByTestId("recording-no-windows")).toContainText("--no-window");

  await expect(page.getByTestId("recording-auto-arm")).not.toBeChecked();
  await expect(page.getByTestId("recording-auto-arm-preset")).toHaveValue("band");
  await expect(section).toContainText("a DAW may not be able to use them");
  await page.getByTestId("recording-auto-arm").check();
  await expect.poll(async () => (await (await api("recording/settings")).json()) as unknown).toEqual({ auto_arm: true, auto_arm_preset: "band", start_in_hub: false });
  // Auto-arm arms, and the transport says it is on.
  await expect(page.getByTestId("recording-state")).toHaveText("Armed", { timeout: 10_000 });
  await expect(page.getByTestId("recording-auto-arm")).toBeChecked();
  await expect(page.getByTestId("recording-auto-arm-state")).toHaveText("Auto-arm is on, with Band.");

  // Disarming by hand pauses it, and the transport says how to resume.
  await page.getByTestId("recording-disarm").click();
  await expect(page.getByTestId("recording-state")).toHaveText("Off");
  await expect(page.getByTestId("recording-auto-arm-state")).toContainText("Auto-arm is paused because you disarmed", { timeout: 5000 });
  await page.waitForTimeout(2500);
  await expect(page.getByTestId("recording-state")).toHaveText("Off");

  // Turned off from the transport, with a second press.
  await page.getByTestId("recording-auto-arm-off").click();
  await page.getByTestId("recording-auto-arm-off").click();
  await expect.poll(async () => ((await (await api("recording/settings")).json()) as { auto_arm: boolean }).auto_arm).toBe(false);
  await expect(page.getByTestId("recording-auto-arm-state")).toBeHidden();

  await page.getByTestId("recording-start-in-hub").check();
  await expect.poll(async () => ((await (await api("recording/settings")).json()) as { start_in_hub: boolean }).start_in_hub).toBe(true);
  await page.getByTestId("recording-start-in-hub").uncheck();
  await expect.poll(async () => ((await (await api("recording/settings")).json()) as { start_in_hub: boolean }).start_in_hub).toBe(false);
});

test("the window routes and the settings are the computer's alone", async () => {
  const phone = { [TEST_PEER_HEADER]: "192.168.1.50:51000" };
  for (const [method, path] of [
    ["GET", "recording/settings"],
    ["PUT", "recording/settings"],
    ["GET", "window/widget"],
    ["POST", "window/hub"],
  ] as const) {
    const answer = await api(path, { method, headers: { ...phone, "content-type": "application/json" }, ...(method === "GET" ? {} : { body: JSON.stringify({ open: true }) }) });
    expect(answer.status, `${method} ${path}`).toBeGreaterThanOrEqual(401);
  }
  const here = (await (await api("window/widget")).json()) as { available: boolean; reason: string };
  expect(here.available).toBe(false);
  const refused = await post("window/widget", { open: true });
  expect(refused.status).toBe(409);
  expect(((await refused.json()) as { error: { code: string } }).error.code).toBe("no_window");
  // A test server keeps its settings in memory, and starts with auto-arm off.
  expect(((await (await api("recording")).json()) as { auto_arm: { on: boolean } }).auto_arm.on).toBe(false);
});
