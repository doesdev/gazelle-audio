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
  server = await startServer(["--dry-run", "--themes-dir", themesDir, "--loopback-cyclic-ms", "50"], { webUi: true });
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
  const own = await startServer([], { webUi: true });
  try {
    await page.goto(`${own.url}/#/devices/loopback-0`);
    const name = page.getByTestId("device-name");
    await expect(name).toBeEnabled();
    await own.stop();
    await expect(page.getByTestId("connection")).toHaveText("Reconnecting…");
    await expect(name).toBeDisabled();
    await expect(page.getByRole("alert")).toContainText("not connected");
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
