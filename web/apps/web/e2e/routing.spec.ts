// The routing matrix in dry run: each destination row shows its sources; selecting source channels
// and clicking a cell, or dragging them onto one, fills that cell and those after it with one
// set_routing per row (a run is cut at the row's end); Delete and "Mute row" mute; mixer inputs are
// read-only here. Expected bytes are the protocol crate's ground-truth set_routing vector with
// bank_idx at byte 18 and 32 (source group, channel) pairs after it, MUTE (Quadro source 10) unless set.

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

function routingHex(destination: number, routed: Record<number, [number, number]>): string {
  const vectors = JSON.parse(readFileSync(join(REPO_ROOT, "crates", "gazelle-audio-protocol", "tests", "ground_truth.json"), "utf8")) as Record<string, string>;
  const bytes = Uint8Array.from(vectors["set_routing"]?.match(/../g) ?? [], (pair) => Number.parseInt(pair, 16));
  bytes[18] = destination;
  for (let slot = 0; slot < 32; slot++) {
    const [source, channel] = routed[slot] ?? [10, 0];
    bytes[19 + 2 * slot] = source;
    bytes[20 + 2 * slot] = channel;
  }
  return `Dry run, would send set_routing: ${[...bytes].map((b) => b.toString(16).padStart(2, "0")).join("")}`;
}

const lastSent = (page: Page) => page.getByTestId("last-sent");

test("select a run of sources and click a cell: it fills from there with one write, cut at the row's end; Mute row mutes", async ({ page }) => {
  await page.goto(`${server.url}/#/routing/loopback-0`);
  // HP1 is destination 1 (two channels); PREAMP is source 0.
  await page.getByTestId("source-0-0").click();
  await page.getByTestId("source-0-1").click({ modifiers: ["Shift"] });
  await page.getByTestId("dest-1-0").click();
  await expect(lastSent(page)).toContainText(routingHex(1, { 0: [0, 0], 1: [0, 1] }));
  await expect(page.getByTestId("dest-1-0")).toHaveText("PREAMP 1");
  await expect(page.getByTestId("dest-1-1")).toHaveText("PREAMP 2");

  await page.getByTestId("dest-1-1").click();
  await expect(lastSent(page)).toContainText(routingHex(1, { 0: [0, 0], 1: [0, 0] }));

  await page.getByTestId("mute-row-1").click();
  await expect(lastSent(page)).toContainText(routingHex(1, {}));
  await expect(page.getByTestId("dest-1-0")).toHaveText("—");
});

test("dragging a source onto a cell routes it; Delete mutes a cell; mixer inputs are not edited here", async ({ page }) => {
  await page.goto(`${server.url}/#/routing/loopback-0`);
  // LINE OUT is destination 0; USB 1 PLAY is source 1.
  const chip = await page.getByTestId("source-1-2").boundingBox();
  const cell = await page.getByTestId("dest-0-1").boundingBox();
  if (chip === null || cell === null) throw new Error("no box to drag");
  await page.mouse.move(chip.x + chip.width / 2, chip.y + chip.height / 2);
  await page.mouse.down();
  await page.mouse.move(cell.x + cell.width / 2, cell.y + cell.height / 2, { steps: 8 });
  await expect(page.getByTestId("dest-0-1")).toHaveAttribute("data-drop", "");
  await page.mouse.up();
  await expect(lastSent(page)).toContainText(routingHex(0, { 1: [1, 2] }));
  await expect(page.getByTestId("dest-0-1")).toHaveText("USB 1·3");

  await page.getByTestId("dest-0-1").focus();
  await page.keyboard.press("Delete");
  await expect(lastSent(page)).toContainText(routingHex(0, {}));

  // MIX CH1 is destination 8.
  await expect(page.getByTestId("dest-8-6")).toHaveAttribute("aria-disabled", "true");
  await expect(page.getByTestId("mute-row-8")).toBeDisabled();
});

test("Routing is in the header and opens the first device of known model", async ({ page }) => {
  await page.goto(server.url);
  await page.locator('ga-header a[data-page="routing"]').click();
  await expect(page).toHaveURL(/#\/routing$/);
  await expect(page.getByTestId("source-0-0")).toBeVisible();
});
