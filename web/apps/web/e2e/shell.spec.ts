// Spec §10 row 3 smoke test, plus the agreed theme and font checks (P16, P19).

import { expect, test } from "@playwright/test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { resetWorkspace } from "./workspace.ts";

let server: RunningServer;
let themesDir: string;

test.beforeAll(async () => {
  themesDir = mkdtempSync(join(tmpdir(), "gazelle-e2e-themes-"));
  writeFileSync(join(themesDir, "ember.json"), JSON.stringify({ name: "Ember", extends: "gazelle-dark", colors: { accent: "#e08a2e" } }));
  writeFileSync(join(themesDir, "broken.json"), "{ not json");
  server = await startServer(["--backend", "loopback", "--dry-run", "--themes-dir", themesDir, "--loopback-cyclic-ms", "50"], { webUi: true });
});

test.afterAll(async () => {
  await server?.stop();
  rmSync(themesDir, { recursive: true, force: true });
});

test.beforeEach(() => resetWorkspace(server));

const deviceName = (page: import("@playwright/test").Page, id: string) => page.locator(`ga-device-list a[data-device-id="${id}"] .name`);

test("the served page lists both loopback devices under the safety header", async ({ page }) => {
  await page.goto(server.url);
  await expect(page.locator("ga-device-list a[data-device-id]")).toHaveCount(2);
  await expect(deviceName(page, "loopback-0")).toHaveText("Zen Quadro Synergy Core");
  await expect(page.getByTestId("backend")).toHaveText("loopback");
  await expect(page.getByTestId("dry-run")).toBeVisible();
  await expect(page.getByTestId("connection")).toHaveText("Connected");
  await expect(page.locator('ga-device-status [data-field="current_preset"]')).not.toHaveText("—");
});

test("a renamed device keeps its name after a reload", async ({ page }) => {
  await page.goto(`${server.url}/#/devices/loopback-0`);
  const name = page.getByTestId("device-name");
  await name.fill("Desk Quadro");
  await name.press("Enter");
  await expect(deviceName(page, "loopback-0")).toHaveText("Desk Quadro");
  await expect
    .poll(async () => ((await (await fetch(`${server.url}/api/v1/workspace`)).json()) as { aliases: Record<string, string> }).aliases["loopback-0"])
    .toBe("Desk Quadro");

  await page.reload();
  await expect(deviceName(page, "loopback-0")).toHaveText("Desk Quadro");
  await expect(page.getByTestId("device-name")).toHaveValue("Desk Quadro");
});

test("themes include user themes, apply on selection and are remembered; fonts are bundled", async ({ page }) => {
  const hosts = new Set<string>();
  page.on("request", (request) => hosts.add(new URL(request.url()).host));
  await page.goto(server.url);

  const picker = page.getByLabel("Theme");
  await expect(picker.locator("option")).toHaveText(["Gazelle Dark", "Gazelle Light", "Studio Blue", "Ember"]);
  const accent = () => page.evaluate(() => document.documentElement.style.getPropertyValue("--ga-accent"));

  await picker.selectOption("user:ember");
  await expect.poll(accent).toBe("#e08a2e");
  await picker.selectOption("gazelle-light");
  await expect.poll(() => page.evaluate(() => document.documentElement.style.getPropertyValue("color-scheme"))).toBe("light");

  await page.reload();
  await expect(page.getByLabel("Theme")).toHaveValue("gazelle-light");

  await expect
    .poll(() =>
      page.evaluate(async () => {
        await document.fonts.ready;
        return [...new Set([...document.fonts].filter((face) => face.status === "loaded").map((face) => face.family.replace(/"/g, "")))].sort();
      }),
    )
    .toEqual(["Inter Variable", "Josefin Sans Variable"]);
  expect([...hosts]).toEqual([new URL(server.url).host]);
});

test("controls are disabled and the header says so when the server stops", async ({ page }) => {
  const own = await startServer(["--backend", "loopback"], { webUi: true });
  try {
    await page.goto(`${own.url}/#/devices/loopback-0`);
    const name = page.getByTestId("device-name");
    await expect(name).toBeEnabled();
    await own.stop();
    await expect(page.getByTestId("connection")).toHaveText("Reconnecting…");
    await expect(name).toBeDisabled();
    // The mixer dock reads the device's mixes as the page opens, so a read cut off by the stop can
    // add an error notice of its own beside the banner.
    await expect(page.getByRole("alert").filter({ hasText: "not connected" })).toBeVisible();
  } finally {
    await own.stop();
  }
});

test("the Devices page powers a device on, and to standby behind a confirm", async ({ page }) => {
  const frames: { command?: string; args?: Record<string, number> }[] = [];
  page.on("websocket", (socket) =>
    socket.on("framesent", (event) => {
      if (typeof event.payload === "string") frames.push(JSON.parse(event.payload) as { command?: string; args?: Record<string, number> });
    }),
  );
  const powers = () => frames.filter((f) => f.command === "set_power").map((f) => f.args?.["power"]);
  await page.goto(`${server.url}/#/devices/loopback-0`);

  await page.getByTestId("device-power-on").click();
  await expect.poll(powers).toEqual([1]);

  // Standby stops the audio, so it takes a confirming second click.
  const standby = page.getByTestId("device-standby");
  await standby.click();
  await expect(standby).toHaveText(/Confirm/);
  await expect.poll(powers).toEqual([1]);
  await standby.click();
  await expect.poll(powers).toEqual([1, 0]);
});

test("the Devices page sets the front-panel brightness", async ({ page }) => {
  const frames: { command?: string; args?: Record<string, number> }[] = [];
  page.on("websocket", (socket) =>
    socket.on("framesent", (event) => {
      if (typeof event.payload === "string") frames.push(JSON.parse(event.payload) as { command?: string; args?: Record<string, number> });
    }),
  );
  await page.goto(`${server.url}/#/devices/loopback-0`);
  const brightness = page.getByTestId("device-brightness");
  await brightness.focus();
  await brightness.press("End");
  // The slider shows what the device reports, so on the loopback it does not move: the value sent
  // is what matters here. On hardware the device echoes the change within about 30 ms.
  await expect.poll(() => frames.filter((f) => f.command === "set_brightness").map((f) => f.args?.["brightness"]).at(-1)).toBe(100);
});

test("the Devices page sets the clock source and sample rate, and shows the measured rate", async ({ page }) => {
  const frames: { command?: string; args?: Record<string, number> }[] = [];
  page.on("websocket", (socket) =>
    socket.on("framesent", (event) => {
      if (typeof event.payload === "string") frames.push(JSON.parse(event.payload) as { command?: string; args?: Record<string, number> });
    }),
  );
  await page.goto(`${server.url}/#/devices/loopback-0`);

  const source = page.getByTestId("clock-source");
  await expect(source.locator("option")).toHaveText(["Internal", "ADAT x1", "ADAT x2", "ADAT x4", "S/PDIF", "USB"]);
  await source.selectOption("4");
  await expect.poll(() => frames.filter((f) => f.command === "set_sync_source").map((f) => f.args?.["src_index"])).toEqual([4]);

  const rate = page.getByTestId("clock-rate");
  await expect(rate.locator("option")).toHaveText(["32 kHz", "44.1 kHz", "48 kHz", "88.2 kHz", "96 kHz", "176.4 kHz", "192 kHz"]);
  await rate.selectOption("2");
  await expect.poll(() => frames.filter((f) => f.command === "set_samp_rate").map((f) => f.args?.["srate_idx"])).toEqual([2]);

  // The measured rate and lock come from the device's report, whatever the loopback is sending.
  await expect(page.getByTestId("clock-measured")).not.toHaveText("—");
});

test("the Devices page switches the Studio+'s S/PDIF sample-rate converter; the Quadro has none", async ({ page }) => {
  const frames: { command?: string; args?: Record<string, number> }[] = [];
  page.on("websocket", (socket) =>
    socket.on("framesent", (event) => {
      if (typeof event.payload === "string") frames.push(JSON.parse(event.payload) as { command?: string; args?: Record<string, number> });
    }),
  );
  // Its own server, whose loopback sends no reports. The shared one cycles every report byte every
  // 50 ms, and `spdif_src` is a single bit, so the switch would flip on its own under the test.
  const own = await startServer(["--backend", "loopback", "--dry-run"], { webUi: true });
  try {
    await page.goto(`${own.url}/#/devices/loopback-0`);
    await expect(page.getByTestId("clock-source")).toBeVisible();
    await expect(page.getByTestId("spdif-src")).toHaveCount(0);

    await page.goto(`${own.url}/#/devices/loopback-1`);
    const src = page.getByTestId("spdif-src");
    // It sits with the clock: with the converter on, an S/PDIF input need not follow the device's clock.
    await expect(page.locator('ga-section[heading="Clock"]').getByTestId("spdif-src")).toHaveCount(1);
    // With nothing reported the switch is off, so a click turns it on.
    await expect(src).toHaveAttribute("aria-pressed", "false");
    await src.click();
    await expect.poll(() => frames.filter((f) => f.command === "set_spdif_src").map((f) => f.args?.["spdif_src"])).toEqual([1]);
  } finally {
    await own.stop();
  }
});

test("the Devices page recalls a device preset, and saves into one behind a confirm", async ({ page }) => {
  const frames: { command?: string; args?: Record<string, number> }[] = [];
  page.on("websocket", (socket) =>
    socket.on("framesent", (event) => {
      if (typeof event.payload === "string") frames.push(JSON.parse(event.payload) as { command?: string; args?: Record<string, number> });
    }),
  );
  const sent = (command: string) => frames.filter((f) => f.command === command).map((f) => f.args?.["preset_idx"]);
  await page.goto(`${server.url}/#/devices/loopback-0`);

  await page.getByTestId("preset-3").click();
  await expect.poll(() => sent("preset_recall")).toEqual([3]);

  // Saving overwrites the slot, so it takes a confirming second click.
  await page.getByTestId("preset-save-slot").selectOption("5");
  const save = page.getByTestId("preset-save");
  await save.click();
  await expect(save).toHaveText(/Confirm/);
  await expect.poll(() => sent("preset_save")).toEqual([]);
  await save.click();
  await expect.poll(() => sent("preset_save")).toEqual([5]);
});

test("the Devices page sets the Quadro's panning law; the Studio+ has none", async ({ page }) => {
  const frames: { command?: string; args?: Record<string, number> }[] = [];
  page.on("websocket", (socket) =>
    socket.on("framesent", (event) => {
      if (typeof event.payload === "string") frames.push(JSON.parse(event.payload) as { command?: string; args?: Record<string, number> });
    }),
  );
  await page.goto(`${server.url}/#/devices/loopback-1`);
  await expect(page.getByTestId("panning-law")).toHaveCount(0);

  await page.goto(`${server.url}/#/devices/loopback-0`);
  const law = page.getByTestId("panning-law");
  await expect(law.locator("option")).toHaveText(["0 dB", "-6 dB", "-3 dB", "-4.5 dB"]);
  await law.selectOption("2");
  await expect.poll(() => frames.filter((f) => f.command === "set_panning_law").map((f) => f.args?.["panning"])).toEqual([2]);
});

test("the Devices page switches DC coupling per side on the Quadro; the Studio+ has none", async ({ page }) => {
  const frames: { command?: string; args?: Record<string, number> }[] = [];
  page.on("websocket", (socket) =>
    socket.on("framesent", (event) => {
      if (typeof event.payload === "string") frames.push(JSON.parse(event.payload) as { command?: string; args?: Record<string, number> });
    }),
  );
  await page.goto(`${server.url}/#/devices/loopback-1`);
  await expect(page.getByTestId("dc-inputs")).toHaveCount(0);

  await page.goto(`${server.url}/#/devices/loopback-0`);
  await page.getByTestId("dc-inputs").click();
  await page.getByTestId("dc-outputs").click();
  // The switches show what the device reports, so on the loopback they stay put: what is sent is
  // what matters. `dc_coupled_io` names the side, inputs 0 and outputs 1.
  await expect.poll(() => frames.filter((f) => f.command === "set_dc_coupled").map((f) => [f.args?.["dc_coupled"], f.args?.["dc_coupled_io"]])).toEqual([
    [1, 0],
    [1, 1],
  ]);
});

test("the Devices page runs the test oscillator: a tone per side over a shared level", async ({ page }) => {
  const frames: { command?: string; args?: Record<string, number> }[] = [];
  page.on("websocket", (socket) =>
    socket.on("framesent", (event) => {
      if (typeof event.payload === "string") frames.push(JSON.parse(event.payload) as { command?: string; args?: Record<string, number> });
    }),
  );
  // Its own server, whose loopback is not sending reports: the oscillator's five fields share one
  // byte, so every change carries the other four, and a device changing under the test would make
  // what is sent unreadable. With nothing reported, both tones read as off.
  const own = await startServer(["--backend", "loopback", "--dry-run"], { webUi: true });
  try {
  await page.goto(`${own.url}/#/devices/loopback-0`);
  await expect(page.getByTestId("osc-freq-left").locator("option")).toHaveText(["1 kHz", "440 Hz"]);
  await expect(page.getByTestId("osc-level").locator("option")).toHaveText(["0 dBFS", "-6 dBFS", "-12 dBFS", "-18 dBFS"]);

  await page.getByTestId("osc-level").selectOption("3");
  await page.getByTestId("osc-freq-right").selectOption("1");
  await page.getByTestId("osc-on-left").click();
  // All five fields travel together, and each change keeps the ones before it.
  await expect.poll(() => frames.filter((f) => f.command === "set_sine_gen").at(-1)?.args).toEqual({
    freq_left: 0,
    freq_right: 1,
    level: 3,
    mute_left: 0,
    mute_right: 1,
  });
  await expect(page.getByTestId("osc-on-left")).toHaveAttribute("aria-pressed", "true");
  } finally {
    await own.stop();
  }
});

test("a page keeps its element while the address stays the same, so a half-made change is not thrown away", async ({ page }) => {
  // Rebuilding a page throws away whatever is half-done in it: an armed confirm, a half-typed
  // name. Only a change of address is a reason to do that — not a report arriving. This caught the
  // page being rebuilt on every cyclic report, which silently dropped a confirming second click.
  await page.goto(`${server.url}/#/devices/loopback-0`);
  await expect(page.getByTestId("device-standby")).toBeVisible();
  await page.evaluate(() => {
    const window_ = window as unknown as { __built: string[] };
    window_.__built = [];
    const root = document.querySelector("ga-app")?.shadowRoot;
    if (root === undefined || root === null) throw new Error("the app has no shadow root");
    new MutationObserver((records) => {
      for (const record of records) {
        for (const node of record.addedNodes) if (node instanceof Element) window_.__built.push(node.tagName);
      }
    }).observe(root, { childList: true, subtree: true });
  });
  await page.waitForTimeout(1500);
  expect(await page.evaluate(() => (window as unknown as { __built: string[] }).__built.filter((tag) => tag === "GA-DEVICE-STATUS"))).toEqual([]);
});
