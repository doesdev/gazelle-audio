// The Recording page and the Remote page's transport, end to end, against the loopback's recorder:
// the real aggregate over interfaces made of data, with a test tone on every input, writing real
// files into the temporary folder. No driver is opened.
//
// Screenshots for the report are written when asked for (RECORDING_SCREENSHOTS=<directory>).

import { existsSync } from "node:fs";

import { expect, test, type Browser, type Page } from "@playwright/test";

import { startServer, TEST_PEER_HEADER, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { putWorkspace } from "./workspace.ts";

let server: RunningServer;

const QUADRO = "loopback-0";
const STUDIO = "loopback-1";
const PHONE = { [TEST_PEER_HEADER]: "192.168.1.50:51000" };
const SHOTS = process.env["RECORDING_SCREENSHOTS"];

test.beforeAll(async () => {
  server = await startServer(["--backend", "loopback", "--loopback-cyclic-ms", "200"], { webUi: true, phoneSeams: true });
});

test.afterAll(async () => {
  await server?.stop();
});

const api = (path: string, init?: RequestInit) => fetch(`${server.url}/api/v1/${path}`, init);
const post = (path: string, body: unknown = {}) => api(path, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });

/** Both interfaces in the aggregate, named as Gazelle names them, and one preset recording from each. */
async function setUp(): Promise<void> {
  await post("recording/disarm", { confirm: true });
  await putWorkspace(server, {
    aliases: { [QUADRO]: "Quadro", [STUDIO]: "Studio+" },
    aggregate: { devices: [{ key: "Zen Quadro Synergy Core", device_id: QUADRO, input_names: { "0": "Vocal mic" } }, { key: "ZenStudioTB", device_id: STUDIO }], alignment: "aligned" },
    ...({ recording: { presets: [{ id: "band", name: "Band", channels: [{ device: 0, channel: 0 }, { device: 0, channel: 1 }, { device: 1, channel: 2 }], preroll_max_seconds: 30 }] } } as object),
  });
}

test.beforeEach(async () => {
  await setUp();
});

async function open(page: Page, hash: string): Promise<void> {
  await page.addInitScript(() => localStorage.removeItem("gazelle.recording.armExplained"));
  await page.goto(`${server.url}/#/${hash}`);
}

test("Arm explains itself once, then Record reaches back into the pre-roll, Stop keeps it armed, and the take is listed with its files", async ({ page }) => {
  await open(page, "recording");
  const state = page.getByTestId("recording-state");
  await expect(state).toHaveText("Off");
  await expect(page.getByTestId("recording-driver-hold")).toContainText("A DAW may not be able to use them");
  await expect(page.getByTestId("recording-preset")).toHaveValue("band");

  // The first Arm in a browser says what it does, and arms from there.
  await page.getByTestId("recording-arm").click();
  await expect(page.getByTestId("recording-arm-explained")).toBeVisible();
  await expect(state).toHaveText("Off", { timeout: 1000 });
  await page.getByTestId("recording-arm-explained-go").click();
  await expect(state).toHaveText("Armed");
  await expect(page.getByTestId("recording-transport")).toHaveAttribute("data-state", "armed");
  await expect(page.getByTestId("recording-preset")).toBeDisabled();
  // The channels, named as a DAW names them, with their levels.
  await expect(page.getByTestId("recording-channel-0")).toContainText("Vocal mic (Quadro 1)");
  await expect(page.getByTestId("recording-channel-2")).toContainText("Studio+ 3");
  await expect(page.getByTestId("recording-preroll")).toContainText("of 30.0 s of pre-roll held");
  await page.waitForTimeout(1500);

  // Space records while the page is shown and nothing else has the focus.
  await page.locator("body").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("Space");
  await expect(state).toHaveText("Recording");
  await expect(page.getByTestId("recording-elapsed")).toBeVisible();
  await expect(page.getByTestId("recording-preroll")).toContainText("Started with");
  await page.waitForTimeout(1200);
  if (SHOTS) {
    for (const theme of ["gazelle-dark", "gazelle-light"]) {
      await page.getByLabel("Theme").selectOption(theme);
      await page.waitForTimeout(400);
      await page.screenshot({ path: `${SHOTS}/recording-desktop-recording-${theme}.png`, fullPage: true });
    }
    await page.getByLabel("Theme").selectOption("gazelle-dark");
  }

  // Disarming while recording asks first.
  await page.getByTestId("recording-disarm").click();
  await expect(page.getByTestId("recording-disarm")).toHaveText("Confirm");
  await page.getByTestId("recording-stop").click();
  await expect(state).toHaveText("Armed");

  const take = page.locator('[data-testid^="recording-take-"]').first();
  await expect(take).toBeVisible({ timeout: 10_000 });
  await expect(take).toContainText(/T\d{3}, /);
  await expect(take).toContainText("Gazelle loopback recordings");
  await expect(take).toContainText("Vocal mic (Quadro 1).wav");
  const { takes } = (await (await api("recording/takes")).json()) as { takes: { files: string[]; log: string; preroll_seconds: number; seconds: number }[] };
  expect(takes[0]?.files.length).toBe(3);
  expect(takes[0]?.preroll_seconds ?? 0).toBeGreaterThan(1);
  for (const file of [...(takes[0]?.files ?? []), takes[0]?.log ?? ""]) expect(existsSync(file), file).toBe(true);
  if (SHOTS) {
    for (const theme of ["gazelle-dark", "gazelle-light"]) {
      await page.getByLabel("Theme").selectOption(theme);
      await page.waitForTimeout(400);
      await page.screenshot({ path: `${SHOTS}/recording-desktop-armed-${theme}.png`, fullPage: true });
    }
    await page.getByLabel("Theme").selectOption("gazelle-dark");
  }

  // Armed, Disarm is one press.
  await page.getByTestId("recording-disarm").click();
  await expect(state).toHaveText("Off");
  await expect(page.getByTestId("recording-preset")).toBeEnabled();
});

test("a preset is made and edited on the page, saved in the workspace, and the armed one cannot change", async ({ page }) => {
  await open(page, "recording");
  await page.getByTestId("recording-preset-new").click();
  const name = page.getByTestId("recording-preset-name");
  await expect(name).toHaveValue("Preset 2");
  await name.fill("Drums");
  await name.press("Enter");
  await page.getByTestId("recording-pick-1-4").check();
  await page.getByTestId("recording-pick-1-5").check();
  await page.getByTestId("recording-preset-format").selectOption("float32");
  await page.getByTestId("recording-preset-percent").selectOption("5");
  await expect
    .poll(async () => ((await (await api("workspace")).json()) as { recording?: { presets: { name: string; channels: unknown[]; format?: string; preroll_percent?: number }[] } }).recording?.presets[1])
    .toEqual(expect.objectContaining({ name: "Drums", channels: [{ device: 1, channel: 4 }, { device: 1, channel: 5 }], format: "float32", preroll_percent: 5 }));

  // Armed with Band, Band's editor is locked and the server refuses a change to it.
  expect((await post("recording/arm", { preset: "band" })).status).toBe(200);
  await page.getByTestId("recording-preset-band").click();
  await expect(page.getByTestId("recording-preset-name")).toBeDisabled();
  await expect(page.getByTestId("recording-state")).toHaveText("Armed");
  const workspace = (await (await api("workspace")).json()) as { recording: { presets: { preroll_percent?: number }[] } };
  (workspace.recording.presets[0] as { preroll_percent?: number }).preroll_percent = 20;
  const refused = await api("workspace", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(workspace) });
  expect(refused.status).toBe(409);
  expect(((await refused.json()) as { error: { code: string } }).error.code).toBe("recording_armed");
});

test("a measurement on the Aggregate page is refused while armed, and Arm is refused with a sentence when a preset cannot record", async () => {
  expect((await post("recording/arm", { preset: "band" })).status).toBe(200);
  const measuring = await post("aggregate/calibrate", { direction: "inputs", outputs: [{ device: 0, channel: 0 }, { device: 0, channel: 1 }], inputs: [{ device: 0, channel: 0 }, { device: 1, channel: 0 }] });
  expect(measuring.status).toBe(409);
  expect(((await measuring.json()) as { error: { code: string } }).error.code).toBe("recording_armed");
  await post("recording/disarm");
  await putWorkspace(server, { ...({ recording: { presets: [{ id: "far", name: "Far", channels: [{ device: 0, channel: 40 }] }] } } as object) });
  const refused = await post("recording/arm", { preset: "far" });
  expect(refused.status).toBe(409);
  const said = ((await refused.json()) as { error: { message: string } }).error.message;
  expect(said).toMatch(/input 41 is not one of them/);
  expect(said.endsWith(".")).toBe(true);
});

async function phone(browser: Browser, scheme: "dark" | "light") {
  await api("remote", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ allow_phones: true }) });
  const started = (await (await api("remote/pairing", { method: "POST" })).json()) as { code: string };
  const paired = (await (await api("remote/pair", { method: "POST", headers: { ...PHONE, "content-type": "application/json" }, body: JSON.stringify({ code: started.code, name: "Recorder test" }) })).json()) as { token: string };
  const context = await browser.newContext({ extraHTTPHeaders: PHONE, viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true, deviceScaleFactor: 2, colorScheme: scheme });
  await context.addCookies([{ name: "gazelle_token", value: paired.token, url: server.url, httpOnly: true, sameSite: "Strict" }]);
  return context;
}

test("a phone arms, records, stops and disarms from the Remote page's transport, and cannot edit presets", async ({ browser }) => {
  const context = await phone(browser, "dark");
  await context.addInitScript(() => localStorage.setItem("gazelle.recording.armExplained", "1"));
  const page = await context.newPage();
  await page.goto(`${server.url}/#/remote/${QUADRO}`);
  const remote = page.locator("ga-remote");
  const state = remote.getByTestId("remote-state");
  await expect(state).toHaveText("Off");
  await remote.getByTestId("remote-arm").tap();
  await expect(state).toHaveText("Armed");
  await page.waitForTimeout(1500);
  await expect(remote.getByTestId("remote-preroll")).toContainText("s of pre-roll held");
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/remote-transport-armed-dark.png` });
  await remote.getByTestId("remote-record").tap();
  await expect(state).toHaveText("Recording");
  await page.waitForTimeout(1300);
  await expect(remote.getByTestId("remote-elapsed")).not.toHaveText("0:00.0");
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/remote-transport-recording-dark.png` });
  await remote.getByTestId("remote-stop").tap();
  await expect(state).toHaveText("Armed");
  await remote.getByTestId("remote-disarm").tap();
  await expect(state).toHaveText("Off");

  // The Recording page on a phone shows the presets and does not edit them.
  await page.goto(`${server.url}/#/recording`);
  await expect(page.getByTestId("recording-state")).toHaveText("Off");
  await expect(page.getByTestId("recording-preset-new")).toBeHidden();
  await expect(page.getByTestId("recording-preset-name")).toBeDisabled();
  await context.close();

  if (SHOTS) {
    const light = await phone(browser, "light");
    await light.addInitScript(() => {
      localStorage.setItem("gazelle.recording.armExplained", "1");
      localStorage.setItem("gazelle.theme", "gazelle-light");
    });
    const lit = await light.newPage();
    await lit.goto(`${server.url}/#/remote/${QUADRO}`);
    await lit.locator("ga-remote").getByTestId("remote-arm").tap();
    await expect(lit.locator("ga-remote").getByTestId("remote-state")).toHaveText("Armed");
    await lit.locator("ga-remote").getByTestId("remote-record").tap();
    await lit.waitForTimeout(1500);
    await lit.screenshot({ path: `${SHOTS}/remote-transport-recording-light.png` });
    await lit.locator("ga-remote").getByTestId("remote-stop").tap();
    await lit.locator("ga-remote").getByTestId("remote-disarm").tap();
    await light.close();
  }
});
