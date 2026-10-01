// The metronome end to end, against the loopback's aggregate: interfaces made of data, the real
// session the recorder uses, no driver opened. The Recording page's Metronome section, a count-in
// before a take, the hub, the widget and the phone's Remote page.
//
// Screenshots for the report are written when asked for (METRONOME_SCREENSHOTS=<directory>).

import { readFileSync } from "node:fs";

import { expect, test, type Browser, type Page } from "@playwright/test";

import { startServer, TEST_PEER_HEADER, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { putWorkspace } from "./workspace.ts";

let server: RunningServer;

const QUADRO = "loopback-0";
const STUDIO = "loopback-1";
const PHONE = { [TEST_PEER_HEADER]: "192.168.1.50:51000" };
const SHOTS = process.env["METRONOME_SCREENSHOTS"];

test.beforeAll(async () => {
  server = await startServer(["--backend", "loopback", "--loopback-cyclic-ms", "200"], { webUi: true, phoneSeams: true });
});

test.afterAll(async () => {
  await server?.stop();
});

const api = (path: string, init?: RequestInit) => fetch(`${server.url}/api/v1/${path}`, init);
const post = (path: string, body: unknown = {}) => api(path, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
const put = (path: string, body: unknown) => api(path, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });

const DEFAULTS = { tempo: 120, numerator: 4, denominator: 4, accent: true, subdivision: "none", sound: "click", volume_db: -18, outputs: [], count_in_bars: 0, follow_record: false, latency_offset_ms: 0 };

async function setUp(): Promise<void> {
  await post("metronome/stop");
  await post("recording/disarm", { confirm: true });
  expect((await put("metronome/settings", DEFAULTS)).status).toBe(200);
  await putWorkspace(server, {
    aliases: { [QUADRO]: "Quadro", [STUDIO]: "Studio+" },
    aggregate: {
      devices: [
        { key: "Zen Quadro Synergy Core", device_id: QUADRO, input_names: { "0": "Vocal mic" }, output_names: { "6": "Phones L", "7": "Phones R" } },
        { key: "ZenStudioTB", device_id: STUDIO },
      ],
      alignment: "aligned",
    },
    ...({ recording: { presets: [{ id: "band", name: "Band", channels: [{ device: 0, channel: 0 }, { device: 1, channel: 2 }], preroll_max_seconds: 30 }] } } as object),
  });
}

test.beforeEach(async () => {
  await setUp();
});

test.afterEach(async () => {
  await post("metronome/stop");
  await post("recording/disarm", { confirm: true });
});

async function open(page: Page, hash: string, options: { explained?: boolean; theme?: string } = {}): Promise<void> {
  await page.addInitScript(
    ([explained, theme]) => {
      localStorage.setItem("gazelle.recording.armExplained", "1");
      if (explained) localStorage.setItem("gazelle.metronome.explained", "1");
      else localStorage.removeItem("gazelle.metronome.explained");
      if (theme) localStorage.setItem("gazelle.theme", theme as string);
    },
    [options.explained ?? false, options.theme ?? ""] as const,
  );
  await page.goto(`${server.url}/#/${hash}`);
}

/** A screenshot with the pointer out of the way. */
async function shoot(page: Page, name: string, fullPage = false): Promise<void> {
  if (!SHOTS) return;
  await page.mouse.move(1, 1);
  await page.waitForTimeout(200);
  await page.screenshot({ path: `${SHOTS}/${name}.png`, fullPage });
}

test("the Metronome section picks outputs, explains the first Start, plays, changes tempo at once, locks the outputs while open and stops", async ({ page }) => {
  await open(page, "recording");
  const section = page.getByTestId("recording-section-metronome");
  await expect(section).toBeVisible();
  await expect(page.getByTestId("metronome-plays-to")).toContainText("Choose where it plays");
  await expect(page.getByTestId("metronome-state")).toHaveText("Stopped");

  // A pair, the headphones' two outputs, named as the aggregate names them.
  await page.getByTestId("metronome-output-0-6").check();
  await expect(page.getByTestId("metronome-output-0-6")).toBeChecked();
  await page.getByTestId("metronome-output-0-7").check();
  await expect(page.getByTestId("metronome-plays-to")).toContainText("Phones L");
  await expect(page.getByTestId("metronome-plays-to")).toContainText("Phones R");

  // The first Start says where it plays and that it holds the drivers; then it plays.
  await page.getByTestId("metronome-start").click();
  const explained = page.getByTestId("metronome-explained");
  await expect(explained).toBeVisible();
  await expect(explained).toContainText("holds their audio drivers");
  await expect(explained).toContainText("It plays to");
  await expect(page.getByTestId("metronome-state")).toHaveText("Stopped");
  await page.getByTestId("metronome-explained-go").click();
  await expect(page.getByTestId("metronome-state")).toHaveText("Playing");
  await expect(page.getByTestId("metronome-start")).toHaveText("Stop");
  await expect(page.getByTestId("metronome-beats").locator(".dot[data-on]")).toHaveCount(1);
  await expect(page.getByTestId("metronome-outputs-fixed")).toBeVisible();
  await expect(page.getByTestId("metronome-output-0-6")).toBeDisabled();
  const status = await (await api("metronome")).json();
  expect(status.running).toBe(true);
  expect(status.outputs.map((c: { name: string }) => c.name)).toEqual(["Phones L (Quadro 7)", "Phones R (Quadro 8)"]);

  // Tempo: typed, stepped, and tapped.
  await page.getByTestId("metronome-tempo").fill("97.5");
  await page.getByTestId("metronome-tempo").press("Enter");
  await expect.poll(async () => (await (await api("metronome/settings")).json()).tempo).toBe(97.5);
  await page.getByTestId("metronome-faster").click();
  await expect.poll(async () => (await (await api("metronome/settings")).json()).tempo).toBe(98.5);
  await page.getByTestId("metronome-sound").selectOption("woodblock");
  await page.getByTestId("metronome-numerator").selectOption("3");
  await expect(page.getByTestId("metronome-beats").locator(".dot")).toHaveCount(3);

  if (SHOTS) {
    // Tall enough that the mixer dock sits below the section.
    await page.setViewportSize({ width: 1280, height: 1700 });
    await page.waitForTimeout(600);
    for (const theme of ["gazelle-dark", "gazelle-light"]) {
      await page.getByLabel("Theme").selectOption(theme);
      await page.waitForTimeout(300);
      await section.screenshot({ path: `${SHOTS}/metronome-section-${theme}.png` });
    }
    await page.getByLabel("Theme").selectOption("gazelle-dark");
  }

  await page.getByTestId("metronome-start").click();
  await expect(page.getByTestId("metronome-state")).toHaveText("Stopped");
  await expect.poll(async () => (await (await api("metronome")).json()).open).toBe(false);
  await expect(page.getByTestId("metronome-output-0-6")).toBeEnabled();
});

test("Record with a count-in counts, starts the take on the downbeat, and Stop stops the click it started", async ({ page }) => {
  await put("metronome/settings", { outputs: [{ device: 0, channel: 6 }], tempo: 240, numerator: 2, count_in_bars: 2 });
  await open(page, "recording", { explained: true });
  // This PC's latency offset, typed on the page, kept in its settings and shown again on a reload.
  const offset = page.getByTestId("metronome-offset");
  await expect(offset).toHaveValue("0");
  await offset.fill("5");
  await offset.press("Enter");
  await expect.poll(async () => (await (await api("metronome/settings")).json()).latency_offset_ms).toBe(5);
  await offset.fill("250");
  await offset.press("Enter");
  await expect.poll(async () => (await (await api("metronome/settings")).json()).latency_offset_ms).toBe(100);
  await offset.fill("5");
  await offset.press("Enter");
  await expect.poll(async () => (await (await api("metronome/settings")).json()).latency_offset_ms).toBe(5);
  await page.reload();
  await expect(page.getByTestId("metronome-offset")).toHaveValue("5");
  await page.getByTestId("recording-arm").click();
  const state = page.getByTestId("recording-state");
  await expect(state).toHaveText("Armed");
  await page.getByTestId("recording-record").click();
  await expect(state).toHaveText(/Count-in/);
  await expect(page.getByTestId("metronome-state")).toContainText("Count-in");
  await expect(state).toHaveText("Recording", { timeout: 5000 });
  await expect(page.getByTestId("metronome-state")).toHaveText("Playing for the take");
  await page.waitForTimeout(600);
  await page.getByTestId("recording-stop").click();
  await expect(state).toHaveText("Armed");
  await expect(page.getByTestId("metronome-state")).toHaveText("Stopped");
  await expect
    .poll(async () => ((await (await api("recording/takes")).json()).takes as { downbeat_seconds?: number }[])[0]?.downbeat_seconds, { timeout: 10_000 })
    .toBeGreaterThan(0);
  const take = ((await (await api("recording/takes")).json()).takes as { downbeat_seconds: number; preroll_seconds: number; files: string[]; log: string }[])[0];
  // Two bars of 2/4 at 240 after the press, and then placed where playing with the heard click
  // lands: the round trip the aggregate reports, and the 5 ms set above. The log says by how much.
  const log = readFileSync(take?.log ?? "", "utf8");
  const placed = /Downbeat placed (\d+) samples after the click: output (\d+) \+ input (\d+) reported by the aggregate, offset (\d+) \(5\.00 ms\)\./.exec(log);
  expect(placed, log).not.toBeNull();
  const [samples, output, input, offsetSamples] = (placed ?? []).slice(1).map(Number) as [number, number, number, number];
  expect(output).toBeGreaterThan(0);
  expect(input).toBeGreaterThan(0);
  expect(samples).toBe(output + input + offsetSamples);
  const rate = offsetSamples / 0.005;
  expect(Math.abs((take?.downbeat_seconds ?? 0) - (take?.preroll_seconds ?? 0) - 1 - samples / rate)).toBeLessThan(0.001);

  // Stop during a count-in starts no take.
  const before = ((await (await api("recording/takes")).json()).takes as unknown[]).length;
  await page.getByTestId("recording-record").click();
  await expect(state).toHaveText(/Count-in/);
  await page.getByTestId("recording-stop").click();
  await expect(state).toHaveText("Armed");
  await page.waitForTimeout(1500);
  expect(((await (await api("recording/takes")).json()).takes as unknown[]).length).toBe(before);
});

test("the hub and the widget start and stop the click and show its tempo and beat", async ({ context }) => {
  let page = await context.newPage();
  await put("metronome/settings", { outputs: [{ device: 0, channel: 6 }, { device: 0, channel: 7 }] });
  await page.setViewportSize({ width: 1920, height: 1080 });
  await open(page, "hub", { explained: true });
  await page.getByTestId("hub-metronome-start").click();
  await expect(page.getByTestId("hub-metronome-state")).toHaveText("Playing");
  await expect(page.getByTestId("hub-metronome-beats").locator(".dot[data-on]")).toHaveCount(1);
  await page.getByTestId("hub-metronome-faster").click();
  await expect(page.getByTestId("hub-metronome-tempo")).toHaveValue("121");
  await page.getByTestId("hub-arm").click();
  await expect(page.getByTestId("hub-state")).toHaveText("Armed");
  await page.waitForTimeout(800);
  await shoot(page, "metronome-hub-running");
  await page.getByTestId("hub-metronome-start").click();
  await expect(page.getByTestId("hub-metronome-state")).toHaveText("Stopped");

  // The widget is a window, and a page, of its own.
  await page.close();
  page = await context.newPage();
  await page.setViewportSize({ width: 320, height: 140 });
  await open(page, "widget", { explained: true });
  const button = page.getByTestId("widget-metronome");
  await expect(button).toHaveText("♩ 121");
  await button.click();
  await expect.poll(async () => (await (await api("metronome")).json()).running).toBe(true);
  await page.waitForTimeout(700);
  await shoot(page, "metronome-widget");
  const overflow = await page.evaluate(() => ({ x: document.documentElement.scrollWidth - innerWidth, y: document.documentElement.scrollHeight - innerHeight }));
  expect(overflow).toEqual({ x: 0, y: 0 });
  await button.click();
  await expect.poll(async () => (await (await api("metronome")).json()).running).toBe(false);
});

async function phone(browser: Browser) {
  await api("remote", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ allow_phones: true }) });
  const started = (await (await api("remote/pairing", { method: "POST" })).json()) as { code: string };
  const paired = (await (await api("remote/pair", { method: "POST", headers: { ...PHONE, "content-type": "application/json" }, body: JSON.stringify({ code: started.code, name: "Metronome test" }) })).json()) as { token: string };
  const context = await browser.newContext({ extraHTTPHeaders: PHONE, viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true, deviceScaleFactor: 2, colorScheme: "dark" });
  await context.addCookies([{ name: "gazelle_token", value: paired.token, url: server.url, httpOnly: true, sameSite: "Strict" }]);
  await context.addInitScript(() => localStorage.setItem("gazelle.metronome.explained", "1"));
  return context;
}

test("a phone starts and stops the click and changes its tempo and volume, and nothing else", async ({ browser }) => {
  await put("metronome/settings", { outputs: [{ device: 0, channel: 6 }, { device: 0, channel: 7 }] });
  const context = await phone(browser);
  const page = await context.newPage();
  await page.goto(`${server.url}/#/remote/${QUADRO}`);
  const remote = page.locator("ga-remote");
  await remote.getByTestId("remote-metronome-start").tap();
  await expect(remote.getByTestId("remote-metronome-state")).toHaveText("Playing");
  await remote.getByTestId("remote-metronome-slower").tap();
  await expect.poll(async () => (await (await api("metronome/settings")).json()).tempo).toBe(119);
  await remote.getByTestId("remote-metronome-volume").fill("-24");
  await expect.poll(async () => (await (await api("metronome/settings")).json()).volume_db).toBe(-24);
  const refused = await fetch(`${server.url}/api/v1/metronome/settings`, { method: "PUT", headers: { ...PHONE, "content-type": "application/json", cookie: (await context.cookies()).map((c) => `${c.name}=${c.value}`).join("; ") }, body: JSON.stringify({ outputs: [] }) });
  expect(refused.status).toBe(403);
  await remote.getByTestId("remote-section-metronome").scrollIntoViewIfNeeded();
  await page.waitForTimeout(700);
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/metronome-remote-phone.png` });
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - innerWidth);
  expect(overflow).toBe(0);
  await remote.getByTestId("remote-metronome-start").tap();
  await expect(remote.getByTestId("remote-metronome-state")).toHaveText("Stopped");
  await context.close();
});
