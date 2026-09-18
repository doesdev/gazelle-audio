// The Outputs page sends each output's volume, mute and (Quadro) dim with the expected bytes in
// dry run. Expected bytes are the protocol crate's ground-truth vectors with the two payload fields
// set: `id` at byte 17 and the value at byte 18, after a one-byte payload header.

import { expect, test, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { REPO_ROOT, startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { resetWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--backend", "loopback", "--dry-run"], { webUi: true });
});

test.beforeEach(() => resetWorkspace(server));

test.afterAll(async () => {
  await server?.stop();
});

function sentText(file: "ground_truth.json" | "ground_truth_studio.json", command: string, id: number, value: number): string {
  const vectors = JSON.parse(readFileSync(join(REPO_ROOT, "crates", "gazelle-audio-protocol", "tests", file), "utf8")) as Record<string, string>;
  const bytes = Uint8Array.from(vectors[command]?.match(/../g) ?? [], (pair) => Number.parseInt(pair, 16));
  bytes[17] = id;
  bytes[18] = value;
  return `Dry run, would send ${command}: ${[...bytes].map((b) => b.toString(16).padStart(2, "0")).join("")}`;
}

const lastSent = (page: Page) => page.getByTestId("last-sent");

test("Quadro outputs: monitor, HP1, HP2 and line out, each with volume, mute and dim", async ({ page }) => {
  const quadro = (command: string, id: number, value: number) => sentText("ground_truth.json", command, id, value);
  await page.goto(`${server.url}/#/outputs/loopback-0`);
  await expect(page.getByTestId("output-3")).toContainText("Line out");
  await expect(page.getByTestId("output-4")).toHaveCount(0);

  const volume = page.getByTestId("out-volume-1");
  await volume.focus();
  await volume.press("End");
  await expect(volume).toHaveAttribute("aria-valuetext", "0 dB");
  await expect(lastSent(page)).toContainText(quadro("set_volume", 1, 0));
  await volume.press("Home");
  await expect(volume).toHaveAttribute("aria-valuetext", "-inf");
  await expect(lastSent(page)).toContainText(quadro("set_volume", 1, 96));

  await page.getByTestId("out-mute-0").click();
  await expect(lastSent(page)).toContainText(quadro("set_mute", 0, 1));
  await expect(page.getByTestId("out-mute-0")).toHaveAttribute("aria-pressed", "true");
  await page.getByTestId("out-dim-2").click();
  await expect(lastSent(page)).toContainText(quadro("set_dim", 2, 1));
});

test("Studio+ outputs add reamp and have no dim", async ({ page }) => {
  await page.goto(`${server.url}/#/outputs/loopback-1`);
  await expect(page.getByTestId("output-4")).toContainText("Reamp");
  await expect(page.getByTestId("out-dim-0")).toHaveCount(0);
  const volume = page.getByTestId("out-volume-4");
  await volume.focus();
  await volume.press("End");
  await expect(lastSent(page)).toContainText(sentText("ground_truth_studio.json", "set_volume", 4, 0));
});

test("Outputs is in the header and opens the first device of known model", async ({ page }) => {
  await page.goto(server.url);
  await page.locator('ga-header a[data-page="outputs"]').click();
  await expect(page).toHaveURL(/#\/outputs$/);
  await expect(page.getByTestId("output-0")).toBeVisible();
});

test("hard mute is a Quadro-only switch that mutes every output at once", async ({ page }) => {
  await page.goto(`${server.url}/#/outputs/loopback-1`);
  await expect(page.getByTestId("hard-mute")).toHaveCount(0);

  await page.goto(`${server.url}/#/outputs/loopback-0`);
  const hardMute = page.getByTestId("hard-mute");
  await expect(hardMute).toHaveAttribute("aria-pressed", "false");
  await hardMute.click();
  // One payload field, so the value sits at byte 17 where sentText puts the id.
  await expect(lastSent(page)).toContainText(sentText("ground_truth.json", "set_hard_mute", 1, 0));
  await expect(hardMute).toHaveAttribute("aria-pressed", "true");
});
