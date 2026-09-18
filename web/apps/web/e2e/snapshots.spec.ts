// The Snapshots section on the Workspace page, against a real server with loopback devices that
// answer reads (P95) and push cyclic reports.
//
// This server is **not** in dry run, unlike most specs here: reads answer nothing in dry run
// (decision 0012), so a snapshot could record nothing. It also runs without `--loopback-cyclic-ms`,
// because a cyclic loopback sweeps the values it reports, so every comparison would find dozens of
// differences that nobody made; here the values only change when this spec changes them, and what
// the cyclic report alone carries is recorded as unread, which is worth seeing too.
//
// The only things sent to a device are the handful of sets this spec makes on purpose, to have
// something for the diff to find — and they go to a loopback, never to hardware.

import { expect, test, type Page } from "@playwright/test";

import { REPO_ROOT, startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { resetWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer([], { webUi: true });
});

test.afterAll(async () => {
  await server?.stop();
});

/** Removes every snapshot, so each test starts from none. */
async function clearSnapshots(): Promise<void> {
  const listed = (await (await fetch(`${server.url}/api/v1/snapshots`)).json()) as { snapshots: { id: string }[] };
  for (const snapshot of listed.snapshots) await fetch(`${server.url}/api/v1/snapshots/${snapshot.id}`, { method: "DELETE" });
}

test.beforeEach(async () => {
  await resetWorkspace(server);
  await clearSnapshots();
});

const workspace = (page: Page) => page.locator("ga-workspace");

/** Renames the Quadro and waits for the server to have it, so a comparison sees the saved name. */
async function renameDevice(page: Page, name: string): Promise<void> {
  const field = workspace(page).getByLabel("Name for loopback-0");
  await field.fill(name);
  await field.press("Enter");
  await expect
    .poll(async () => ((await (await fetch(`${server.url}/api/v1/workspace`)).json()) as { aliases: Record<string, string> }).aliases["loopback-0"])
    .toBe(name);
}

async function open(page: Page): Promise<void> {
  await page.goto(`${server.url}/#/workspace`);
  await expect(workspace(page).getByTestId("snapshots")).toBeVisible();
}

async function take(page: Page, name: string): Promise<void> {
  await workspace(page).getByTestId("snapshot-new-name").fill(name);
  await workspace(page).getByTestId("snapshot-take").click();
  // The name is a field, so it is found by its label rather than as text on the page.
  await expect(workspace(page).getByLabel(`Name of ${name}`)).toHaveValue(name);
}

test("a snapshot is taken with a name, listed with when and from which devices, renamed and deleted", async ({ page }) => {
  await open(page);
  await expect(workspace(page).getByTestId("snapshots")).toContainText("No snapshots yet");

  await take(page, "Drum tracking");
  const rows = workspace(page).locator('[data-testid^="snapshot-row-"]');
  await expect(rows).toHaveCount(1);
  // The list says which devices it came from, so a snapshot taken with one interface unplugged is
  // never mistaken for one of the whole rig.
  await expect(rows.first()).toContainText("Zen Quadro Synergy Core and Zen Studio+");

  await take(page, "After the take");
  await expect(rows).toHaveCount(2);
  await expect(rows.first().locator("input")).toHaveValue("After the take", { timeout: 5000 });

  // Rename in place, as a surface is renamed.
  const id = (await rows.last().getAttribute("data-testid"))!.replace("snapshot-row-", "");
  const field = workspace(page).getByTestId(`snapshot-rename-${id}`);
  await field.fill("Take 1");
  await field.press("Enter");
  await expect.poll(async () => ((await (await fetch(`${server.url}/api/v1/snapshots/${id}`)).json()) as { name: string }).name).toBe("Take 1");

  // Delete needs a second click, and nothing else goes with it.
  const remove = workspace(page).getByTestId(`snapshot-delete-${id}`);
  await remove.click();
  await expect(remove).toHaveText("Confirm");
  await remove.click();
  await expect(rows).toHaveCount(1);
  await expect(workspace(page).getByTestId(`snapshot-row-${id}`)).toHaveCount(0);
});

test("comparing with now finds the change, in the section it belongs to, and says so in words", async ({ page }) => {
  await open(page);
  await take(page, "Before the change");
  const id = (await workspace(page).locator('[data-testid^="snapshot-row-"]').first().getAttribute("data-testid"))!.replace("snapshot-row-", "");

  await workspace(page).getByTestId(`snapshot-compare-${id}`).click();
  await expect(workspace(page).getByTestId("snapshot-diff-summary")).toContainText("Nothing that could be read differs");
  await expect(workspace(page).getByTestId("snapshot-diff-summary")).toContainText("could not be read on one side or the other");
  await workspace(page).getByTestId("snapshot-diff-close").click();
  await expect(workspace(page).getByTestId("snapshot-diff-summary")).toHaveCount(0);

  // Change one device setting and one workspace field, then compare again.
  await fetch(`${server.url}/api/v1/devices/loopback-0/command/set_panning_law`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ panning: 2 }),
  });
  await renameDevice(page, "Desk Quadro");

  await workspace(page).getByTestId(`snapshot-compare-${id}`).click();
  const settings = workspace(page).getByTestId("snapshot-diff-loopback-0-settings");
  await expect(settings).toContainText("Settings");
  await expect(settings).toContainText("panning law");
  await expect(settings).toContainText("0 → 2");
  await expect(workspace(page).getByTestId("snapshot-diff-workspace-workspace")).toContainText("Desk Quadro");
  await expect(workspace(page).getByTestId("snapshot-diff-summary")).toContainText("differences");
});

test("a device that is not attached is named rather than compared away", async ({ page }) => {
  await open(page);
  await take(page, "Both devices");
  const id = (await workspace(page).locator('[data-testid^="snapshot-row-"]').first().getAttribute("data-testid"))!.replace("snapshot-row-", "");

  // The server's own detach, which is what unplugging looks like to the page.
  await fetch(`${server.url}/api/v1/devices/loopback-1`, { method: "DELETE" }).catch(() => undefined);
  await workspace(page).getByTestId(`snapshot-compare-${id}`).click();
  await expect(workspace(page).getByTestId("snapshot-diff-summary")).toBeVisible();
});

test("reference screenshots of the Snapshots section and of a comparison", async ({ page }) => {
  await open(page);
  await take(page, "Drum tracking");
  await take(page, "After the take");
  await workspace(page).getByTestId("snapshots").scrollIntoViewIfNeeded();
  await page.evaluate(() => document.fonts.ready);
  await page.screenshot({ path: `${REPO_ROOT}/web/test-results/design/snapshots-section.png` });

  // Something differing in several sections at once, which is what a comparison is for.
  const id = (await workspace(page).locator('[data-testid^="snapshot-row-"]').last().getAttribute("data-testid"))!.replace("snapshot-row-", "");
  const set = (command: string, body: unknown) =>
    fetch(`${server.url}/api/v1/devices/loopback-0/command/${command}`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
  await set("set_panning_law", { panning: 2 });
  await set("set_mixer", { channel: 1, level: 23 });
  await set("set_mic_emulation", { preamp_ch: 0, target: 1, emu_model: 3, ch_swap: 0, pattern: 0 });
  // An element array of two-byte slots, as hex: every slot of LINE OUT taking source 1, channel 0.
  await set("set_routing", { bank_idx: 0, bank_configs: "0100".repeat(32) });
  await renameDevice(page, "Desk Quadro");

  await workspace(page).getByTestId(`snapshot-compare-${id}`).click();
  await expect(workspace(page).getByTestId("snapshot-diff-loopback-0-settings")).toBeVisible();
  await expect(workspace(page).getByTestId("snapshot-diff-loopback-0-mixer")).toBeVisible();
  await expect(workspace(page).getByTestId("snapshot-diff-loopback-0-inputs")).toBeVisible();
  await expect(workspace(page).getByTestId("snapshot-diff-loopback-0-routing")).toBeVisible();
  await expect(workspace(page).getByTestId("snapshot-diff-workspace-workspace")).toBeVisible();
  // The app shell scrolls inside itself, so a full-page shot would still show the top of the page.
  await workspace(page).getByTestId("snapshot-diff-summary").scrollIntoViewIfNeeded();
  await page.evaluate(() => document.fonts.ready);
  await page.screenshot({ path: `${REPO_ROOT}/web/test-results/design/snapshots-diff.png` });
});
