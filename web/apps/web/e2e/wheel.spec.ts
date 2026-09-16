// Feedback item 6 (P73): the wheel over any select steps it to the previous or next option it may
// pick, firing `change` as a pick would, so the page's own handler sends the command. How travel
// becomes steps is unit-tested (test/select-wheel.test.ts); these check the listener in the app.

import { expect, test, type Locator, type Page } from "@playwright/test";

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

type Frame = { command?: string; args?: Record<string, number> };

function recordFrames(page: Page): Frame[] {
  const frames: Frame[] = [];
  page.on("websocket", (socket) =>
    socket.on("framesent", (event) => {
      if (typeof event.payload === "string") frames.push(JSON.parse(event.payload) as Frame);
    }),
  );
  return frames;
}

/** Notes, for each wheel event, whether something kept the page from scrolling. */
async function watchWheel(page: Page): Promise<void> {
  await page.evaluate(() => window.addEventListener("wheel", (event) => ((window as unknown as { wheelKept: boolean }).wheelKept = event.defaultPrevented)));
}

const wheelKept = (page: Page) => page.evaluate(() => (window as unknown as { wheelKept?: boolean }).wheelKept);

async function wheelOver(page: Page, target: Locator, deltaY: number): Promise<void> {
  await target.scrollIntoViewIfNeeded();
  const box = await target.boundingBox();
  if (box === null) throw new Error("the select has no box");
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.wheel(0, deltaY);
}

/** A page scroll holds the wheel for a moment (WHEEL_IDLE_MS); a test moving on from one waits it out. */
const restTheWheel = (page: Page) => page.waitForTimeout(600);

test("the wheel over a select steps it and sends, stays put at the ends, and keeps the page still", async ({ page }) => {
  const frames = recordFrames(page);
  const sent = () => frames.filter((f) => f.command === "set_panning_law").map((f) => f.args?.["panning"]);
  await page.goto(`${server.url}/#/devices/loopback-0`);
  await watchWheel(page);
  const law = page.getByTestId("panning-law");
  await expect(law.locator("option")).toHaveText(["0 dB", "-6 dB", "-3 dB", "-4.5 dB"]);
  await law.selectOption("1");
  await expect.poll(sent).toEqual([1]);

  // Down is the next option, up the previous; the select sits in a shadow root inside another.
  await wheelOver(page, law, 100);
  await expect(law).toHaveValue("2");
  await expect.poll(sent).toEqual([1, 2]);
  expect(await wheelKept(page)).toBe(true);
  await wheelOver(page, law, -100);
  await expect(law).toHaveValue("1");
  await expect.poll(sent).toEqual([1, 2, 1]);

  // Past the last option nothing changes, and the page still does not scroll.
  await wheelOver(page, law, 100);
  await wheelOver(page, law, 100);
  await expect(law).toHaveValue("3");
  await wheelOver(page, law, 100);
  await expect.poll(sent).toEqual([1, 2, 1, 2, 3]);
  await page.waitForTimeout(200);
  expect(sent()).toEqual([1, 2, 1, 2, 3]);
  await expect(law).toHaveValue("3");
  expect(await wheelKept(page)).toBe(true);

  // A trackpad's small deltas step once they add up to a notch.
  await restTheWheel(page);
  await wheelOver(page, law, -40);
  await page.mouse.wheel(0, -40);
  await expect(law).toHaveValue("3");
  await page.mouse.wheel(0, -40);
  await expect(law).toHaveValue("2");
  await expect.poll(sent).toEqual([1, 2, 1, 2, 3, 2]);
});

test("the wheel passes over disabled options, and leaves a disabled select alone", async ({ page }) => {
  const frames = recordFrames(page);
  const emulations = () => frames.filter((f) => f.command === "set_mic_emulation").length;
  await page.goto(`${server.url}/#/inputs/loopback-0`);
  await watchWheel(page);

  // An Edge Quadro at XY, then an Oxford 4038 on the top head: a figure-8 over a cardioid, which is
  // M/S, and the 4038 cannot point, so XY is not on offer and the top head's pattern is disabled.
  await page.getByTestId("mic-target-0").selectOption("4");
  await page.getByTestId("mic-model-0-top").selectOption("3");
  await page.getByTestId("mic-model-0-bottom").selectOption("3");
  const preset = page.getByTestId("mic-preset-0");
  await preset.selectOption("1");
  await page.getByTestId("mic-model-0-top").selectOption("6");
  await expect(preset.locator("option").nth(1)).toBeDisabled();
  await expect(preset).toHaveValue("2");

  // Up from M/S passes over XY to None, and down from None passes over it to M/S, which sends.
  await wheelOver(page, preset, -100);
  await expect(preset).toHaveValue("0");
  const before = emulations();
  await page.mouse.wheel(0, 100);
  await expect.poll(emulations).toBeGreaterThan(before);
  await expect(preset).not.toHaveValue("1");

  const pattern = page.getByTestId("mic-pattern-0-top");
  await expect(pattern).toBeDisabled();
  const value = await pattern.inputValue();
  const settled = emulations();
  await wheelOver(page, pattern, 100);
  await expect.poll(() => wheelKept(page)).toBe(false);
  await page.waitForTimeout(200);
  await expect(pattern).toHaveValue(value);
  expect(emulations()).toBe(settled);
});

test("a select marked data-no-wheel is left to the page", async ({ page }) => {
  const frames = recordFrames(page);
  await page.goto(`${server.url}/#/devices/loopback-0`);
  await watchWheel(page);

  // The sample rate would reclock the device at every step, so it opts out.
  const rate = page.getByTestId("clock-rate");
  const value = await rate.inputValue();
  await wheelOver(page, rate, 100);
  await expect.poll(() => wheelKept(page)).toBe(false);
  await page.waitForTimeout(200);
  await expect(rate).toHaveValue(value);
  expect(frames.filter((f) => f.command === "set_samp_rate")).toEqual([]);
});

test("an option marked data-no-wheel is passed over", async ({ page }) => {
  // "New group…" is an action; stepping onto it would make a group at every notch.
  await putWorkspace(server, { mixers: { "loopback-0": { channels: [{ id: "a", name: "Kick", slot: 6, sends: [] }] } } });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  await watchWheel(page);
  const group = page.getByTestId("group-6");
  await expect(group.locator("option")).toHaveText(["No group", "New group…"]);
  await wheelOver(page, group, 100);
  await expect.poll(() => wheelKept(page)).toBe(true);
  await page.waitForTimeout(200);
  await expect(group).toHaveValue("");
  await expect(page.locator("ga-channel-group")).toHaveCount(0);
});
