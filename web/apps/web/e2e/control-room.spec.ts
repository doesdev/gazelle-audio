// The Control Room first cut (decision P56) in dry run: trims and a read-only mono badge on the
// Outputs page, Studio+ talkback there, and a monitor panel in the right zone that follows the
// device on the page. Expected bytes are the protocol crate's ground-truth vectors with the payload
// fields set: a one-byte payload header, then the fields from byte 17 (set_trim_config's two-byte
// header puts trim_id at 18).

import { expect, test, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { REPO_ROOT, startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { resetWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--dry-run"], { webUi: true });
});

test.beforeEach(() => resetWorkspace(server));

test.afterAll(async () => {
  await server?.stop();
});

function sentText(file: "ground_truth.json" | "ground_truth_studio.json", command: string, fields: Record<number, number>): string {
  const vectors = JSON.parse(readFileSync(join(REPO_ROOT, "crates", "gazelle-audio-protocol", "tests", file), "utf8")) as Record<string, string>;
  const bytes = Uint8Array.from(vectors[command]?.match(/../g) ?? [], (pair) => Number.parseInt(pair, 16));
  for (const [at, value] of Object.entries(fields)) bytes[Number(at)] = value;
  return `Dry run, would send ${command}: ${[...bytes].map((b) => b.toString(16).padStart(2, "0")).join("")}`;
}

const lastSent = (page: Page) => page.getByTestId("last-sent");
const panel = (page: Page) => page.locator("ga-control-room");

test("Quadro trims are steps from 20 to 14 dBu sent with set_trim_config", async ({ page }) => {
  await page.goto(`${server.url}/#/outputs/loopback-0`);
  const trim = page.getByTestId("trim-1");
  await expect(trim.locator("option")).toHaveText(["20 dBu", "19 dBu", "18 dBu", "17 dBu", "16 dBu", "15 dBu", "14 dBu"]);
  await trim.selectOption("4");
  await expect(lastSent(page)).toContainText(sentText("ground_truth.json", "set_trim_config", { 18: 1, 19: 1, 20: 4 }));
  await expect(page.getByTestId("trim-2")).toHaveCount(0);
  await expect(page.getByTestId("talk")).toHaveCount(0, { timeout: 1000 });
});

test("Studio+ trims include the ADC, and talkback sends set_talk, set_tbk_enable and set_tbk_vol", async ({ page }) => {
  const studio = (command: string, fields: Record<number, number>) => sentText("ground_truth_studio.json", command, fields);
  await page.goto(`${server.url}/#/outputs/loopback-1`);
  await page.getByTestId("trim-2").selectOption("3");
  await expect(lastSent(page)).toContainText(studio("set_trim", { 17: 2, 18: 3 }));

  await page.getByTestId("talk").click();
  await expect(lastSent(page)).toContainText(studio("set_talk", { 17: 1 }));
  await expect(page.getByTestId("talk")).toHaveAttribute("aria-pressed", "true");
  await page.getByTestId("talk-to-1").click();
  await expect(lastSent(page)).toContainText(studio("set_tbk_enable", { 17: 1, 18: 1 }));
  const volume = page.getByTestId("talk-volume");
  await volume.focus();
  await volume.press("End");
  await expect(lastSent(page)).toContainText(studio("set_tbk_vol", { 17: 255 }));
});

test("the right zone's monitor panel follows the page's device and shares state with the Outputs page", async ({ page }) => {
  await page.goto(`${server.url}/#/outputs/loopback-0`);
  await expect(panel(page)).toContainText("Zen Quadro");
  const volume = panel(page).getByTestId("cr-volume");
  await volume.focus();
  await volume.press("End");
  await expect(lastSent(page)).toContainText(sentText("ground_truth.json", "set_volume", { 17: 0, 18: 0 }));
  await panel(page).getByTestId("cr-dim").click();
  await expect(lastSent(page)).toContainText(sentText("ground_truth.json", "set_dim", { 17: 0, 18: 1 }));
  await page.getByTestId("out-mute-0").click();
  await expect(panel(page).getByTestId("cr-mute")).toHaveAttribute("aria-pressed", "true");
  await expect(panel(page).getByTestId("cr-talk")).toHaveCount(0);

  await page.goto(`${server.url}/#/inputs/loopback-1`);
  await expect(panel(page)).toContainText("Zen Studio+");
  await expect(panel(page).getByTestId("cr-dim")).toHaveCount(0);
  await panel(page).getByTestId("cr-talk").click();
  await expect(page.getByTestId("last-sent")).toContainText("set_talk");
});
