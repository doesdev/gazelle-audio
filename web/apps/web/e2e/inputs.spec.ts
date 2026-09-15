// Phase 5 increment 2: the Inputs page sends each family's preamp and input commands with the
// expected bytes in dry run. Expected bytes are the protocol crate's ground-truth vectors (all
// fields zero) with the two payload fields set: a two-byte payload has a one-byte payload header,
// so `id` is byte 17 and the value byte 18 (signed values as their byte).

import { expect, test, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { REPO_ROOT, startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--dry-run"], { webUi: true });
});

test.afterAll(async () => {
  await server?.stop();
});

function expectedHex(file: "ground_truth.json" | "ground_truth_studio.json", command: string, id: number, value: number): string {
  const vectors = JSON.parse(readFileSync(join(REPO_ROOT, "crates", "gazelle-audio-protocol", "tests", file), "utf8")) as Record<string, string>;
  const hex = vectors[command];
  if (hex === undefined) throw new Error(`no ${command} vector in ${file}`);
  const bytes = Uint8Array.from(hex.match(/../g) ?? [], (pair) => Number.parseInt(pair, 16));
  bytes[17] = id;
  bytes[18] = value & 0xff;
  return [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
}

const lastSent = (page: Page) => page.getByTestId("last-sent");

test("Quadro preamps send type, gain, a confirmed 48V and phase, with the panels' ranges", async ({ page }) => {
  const quadro = (command: string, id: number, value: number) => `Dry run, would send ${command}: ${expectedHex("ground_truth.json", command, id, value)}`;
  await page.goto(`${server.url}/#/inputs/loopback-0`);
  await expect(page.getByTestId("preamp-3")).toBeVisible();
  await expect(page.getByTestId("preamp-4")).toHaveCount(0);
  await expect(page.getByTestId("pre-type-1-hi-z")).toBeVisible();
  await expect(page.getByTestId("pre-type-2-hi-z")).toHaveCount(0, { timeout: 1000 });

  await page.getByTestId("pre-type-1-line").click();
  await expect(lastSent(page)).toContainText(quadro("set_pre_type", 1, 1));
  await expect(page.getByTestId("pre-type-1-line")).toHaveAttribute("aria-pressed", "true");
  const gain = page.getByTestId("pre-gain-1");
  await gain.focus();
  await gain.press("Home");
  await expect(gain).toHaveAttribute("aria-valuetext", "-6 dB");
  await expect(lastSent(page)).toContainText(quadro("set_pre_gain", 1, -6));
  await expect(page.getByTestId("pre-48v-1")).toBeDisabled();

  const phantom = page.getByTestId("pre-48v-0");
  await phantom.click();
  await expect(phantom).toHaveText("Confirm");
  await expect(lastSent(page)).not.toContainText("set_pre_phantom");
  await phantom.click();
  await expect(phantom).toHaveAttribute("aria-pressed", "true");
  await expect(lastSent(page)).toContainText(quadro("set_pre_phantom", 0, 1));

  await page.getByTestId("pre-phase-3").click();
  await expect(lastSent(page)).toContainText(quadro("set_pre_phase_inv", 3, 1));
  await expect(page.getByTestId("adat-gain-0")).toHaveText("—", { timeout: 1000 });
});

test("Studio+ sends set_pre_phaseinv and digital input gains; Hi-Z is on preamps 1-4", async ({ page }) => {
  const studio = (command: string, id: number, value: number) => `Dry run, would send ${command}: ${expectedHex("ground_truth_studio.json", command, id, value)}`;
  await page.goto(`${server.url}/#/inputs/loopback-1`);
  await expect(page.getByTestId("pre-type-3-hi-z")).toBeVisible();
  await expect(page.getByTestId("pre-type-4-hi-z")).toHaveCount(0);

  await page.getByTestId("pre-phase-0").click();
  await expect(lastSent(page)).toContainText(studio("set_pre_phaseinv", 0, 1));

  const line = page.getByTestId("line-gain-2");
  await line.focus();
  await line.press("End");
  await expect(lastSent(page)).toContainText(studio("set_line_gain", 2, 12));
  const spdif = page.getByTestId("spdif-gain-1");
  await spdif.focus();
  await spdif.press("Home");
  await expect(lastSent(page)).toContainText(studio("set_spdif_gain", 1, -6));
});

test("Inputs is in the header and opens the first device of known model", async ({ page }) => {
  await page.goto(server.url);
  await page.locator('ga-header a[data-page="inputs"]').click();
  await expect(page).toHaveURL(/#\/inputs$/);
  await expect(page.getByTestId("preamp-0")).toBeVisible();
});
