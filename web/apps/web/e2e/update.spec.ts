// What the header says about updating, in the real app.
//
// The updater's own behaviour (checking, verifying, staging, the one restart path) is covered
// against a fake release source in crates/gazelle-audio-server/tests/update.rs. What is left to
// check here is the page: that it says nothing when there is nothing to say, that a ready version
// is a button which asks twice before it restarts, that a download says so while it runs, that a
// failure is not silent, and that a phone still gets the prompt.
//
// So the update routes are answered in the browser, the way driver.spec.ts answers the driver's
// (`page.route`), rather than by pointing a real server at a fake release source: the server here
// is the one every other spec starts, this spec drives each state directly instead of arranging
// for it, and, most of all, no test can restart the server that Playwright is talking to.

import { expect, test, type Page, type Route } from "@playwright/test";

import { startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { resetWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--backend", "loopback", "--dry-run"], { webUi: true });
});

test.beforeEach(() => resetWorkspace(server));

test.afterAll(async () => {
  await server?.stop();
});

type State =
  | { state: "unknown" | "checking" | "up_to_date" }
  | { state: "available"; version: string; page: string }
  | { state: "downloading" | "staged"; version: string }
  | { state: "failed"; message: string; detail: string };

const status = (state: State, auto = true) => ({
  version: "1.1.0",
  target: "x86_64-pc-windows-msvc",
  channel: "stable",
  check: true,
  auto_download: auto,
  can_verify: true,
  last_check_ms: 1_789_700_000_000,
  state,
});

interface Updater {
  /** Every update path the page asked, in order. */
  calls: string[];
  /** What the next read answers. */
  say(state: State, auto?: boolean): void;
}

/**
 * Answers the page's update requests. A POST answers with whatever state is set now, so a test
 * can make a click land anywhere; `POST /update/restart` answers and stops nothing, since the
 * server behind this page is the one the whole spec is using.
 */
async function fakeUpdater(page: Page, initial: State, auto = true): Promise<Updater> {
  let now = initial;
  let automatic = auto;
  const calls: string[] = [];
  await page.route("**/api/v1/update**", async (route: Route) => {
    const path = new URL(route.request().url()).pathname.replace("/api/v1/update", "") || "/status";
    calls.push(path);
    if (path === "/restart") return route.fulfill({ json: { restarting: true, version: "1.2.0" } });
    await route.fulfill({ json: status(now, automatic) });
  });
  return {
    calls,
    say(state: State, auto = true) {
      now = state;
      automatic = auto;
    },
  };
}

const action = (page: Page) => page.getByTestId("update-action");
const note = (page: Page) => page.getByTestId("update-note");

test("an app that is up to date says nothing about updates", async ({ page }) => {
  const updater = await fakeUpdater(page, { state: "up_to_date" });
  await page.goto(`${server.url}/#/devices/loopback-0`);
  await expect(page.getByTestId("backend")).toBeVisible();
  await expect(action(page)).toBeHidden();
  await expect(note(page)).toBeHidden();
  // It did ask, though: silence is an answer, not a missing question.
  await expect.poll(() => updater.calls).toContain("/status");
});

test("a version being fetched says so, and becomes a restart when it is ready", async ({ page }) => {
  const updater = await fakeUpdater(page, { state: "downloading", version: "1.2.0" });
  await page.goto(`${server.url}/#/devices/loopback-0`);
  await expect(note(page)).toHaveText("1.2.0 downloading");
  await expect(action(page)).toBeHidden();

  updater.say({ state: "staged", version: "1.2.0" });
  // The page is polling faster while something is happening, so this lands without a reload.
  await expect(action(page)).toHaveText("1.2.0 ready, restart");
  await expect(note(page)).toBeHidden();
});

test("a ready version asks twice before it restarts, and a wait forgets it", async ({ page }) => {
  const updater = await fakeUpdater(page, { state: "staged", version: "1.2.0" });
  await page.goto(`${server.url}/#/devices/loopback-0`);
  await expect(action(page)).toHaveText("1.2.0 ready, restart");

  // One press only arms it: a restart lets go of the devices for a few seconds.
  await action(page).click();
  await expect(action(page)).toHaveText("Confirm");
  await expect(action(page)).toHaveAttribute("data-armed", "");
  expect(updater.calls).not.toContain("/restart");

  await action(page).click();
  await expect.poll(() => updater.calls).toContain("/restart");
  await expect(page.locator("ga-notices .notice").first()).toContainText("Restarting into Gazelle 1.2.0");
});

test("an armed restart forgets itself if it is left alone", async ({ page }) => {
  const updater = await fakeUpdater(page, { state: "staged", version: "1.2.0" });
  await page.goto(`${server.url}/#/devices/loopback-0`);
  await action(page).click();
  await expect(action(page)).toHaveText("Confirm");
  await expect(action(page)).toHaveText("1.2.0 ready, restart", { timeout: 6000 });
  expect(updater.calls).not.toContain("/restart");
});

test("a failed check is not silent, and offers another try", async ({ page }) => {
  const updater = await fakeUpdater(page, { state: "failed", message: "no connection", detail: "https://api.github.com/x: status 503" });
  await page.goto(`${server.url}/#/devices/loopback-0`);
  await expect(action(page)).toHaveText("Update failed, try again");
  await expect(action(page)).toHaveAttribute("title", /No connection\. https:/);

  // One press, no arming: asking again costs nothing.
  updater.say({ state: "downloading", version: "1.2.0" });
  await action(page).click();
  await expect.poll(() => updater.calls).toContain("/check");
  await expect(note(page)).toHaveText("1.2.0 downloading");
});

test("an install that does not fetch by itself is offered the download in one press", async ({ page }) => {
  const updater = await fakeUpdater(page, { state: "available", version: "1.2.0", page: "https://example.invalid/r" }, false);
  await page.goto(`${server.url}/#/devices/loopback-0`);
  await expect(action(page)).toHaveText("Get 1.2.0");

  updater.say({ state: "staged", version: "1.2.0" }, false);
  await action(page).click();
  await expect.poll(() => updater.calls).toContain("/download");
  await expect(action(page)).toHaveText("1.2.0 ready, restart");
});

test("a phone keeps the update prompt, though it drops the version readout", async ({ page }) => {
  await fakeUpdater(page, { state: "staged", version: "1.2.0" });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${server.url}/#/devices/loopback-0`);

  // The version is a thing to read and the header line has no room for it on a phone; the prompt
  // is a thing to do, and a phone is as good a place to press it from as a desk.
  await expect(action(page)).toBeVisible();
  await expect(action(page)).toHaveText("1.2.0 ready, restart");
  await expect(page.getByTestId("version")).toBeHidden();
  // The badges that must always be readable are still on the line beside it.
  await expect(page.getByTestId("backend")).toBeVisible();
  await expect(page.getByTestId("connection")).toBeVisible();
});
