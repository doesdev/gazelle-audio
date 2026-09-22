// Dragging sources from the Routing page onto the mixer dock under it: each source becomes a channel
// in the dock's mix, fed by it, made as the Mixer page's "+" and its Input and Main mix menus make
// one. The dock says what a drop would do while a drag is over it, asks first where an input would
// be in the mix twice, refuses a drop while it shows a surface, and opens when a drag rests on it
// folded. Dry run: the layout is what is checked, through the page and the server's workspace.

import { expect, test, type Locator, type Page } from "@playwright/test";

import { startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { putWorkspace, resetWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--backend", "loopback", "--dry-run"], { webUi: true });
});

test.beforeEach(() => resetWorkspace(server));

test.afterAll(async () => {
  await server?.stop();
});

/** The Quadro with one mix, Monitors, holding Vox on PREAMP 1 at mixer input 7. */
const MIXERS = { "loopback-0": { mixes: [{ name: "Monitors" }], groups: [], channels: [{ id: "a", name: "Vox", slot: 6, sends: [], source: { group: 0, channel: 0 }, main_mix: 0 }] } };

const dock = (page: Page) => page.locator("ga-mixer-dock");
/** The dock's channel strips' mixer inputs, in the order shown. */
const slots = (page: Page) => dock(page).locator('ga-strip:not([strip="master"])').evaluateAll((strips) => strips.map((s) => s.getAttribute("strip")));

interface SavedChannel {
  slot: number;
  source?: { group: number; channel: number };
  main_mix?: number;
  sends: number[];
}

/** The Quadro's channels as the server has saved them. */
async function saved(): Promise<SavedChannel[]> {
  const workspace = (await (await fetch(`${server.url}/api/v1/workspace`)).json()) as { mixers?: Record<string, { channels: SavedChannel[] }> };
  return (workspace.mixers?.["loopback-0"]?.channels ?? []).map(({ slot, source, main_mix, sends }) => ({ slot, sends, ...(source === undefined ? {} : { source }), ...(main_mix === undefined ? {} : { main_mix }) }));
}

/** Presses on `from` and moves onto `to`, leaving the button down so the drag can be looked at. */
async function dragOver(page: Page, from: Locator, to: Locator): Promise<void> {
  const start = await from.boundingBox();
  const end = await to.boundingBox();
  if (start === null || end === null) throw new Error("no box to drag");
  await page.mouse.move(start.x + start.width / 2, start.y + start.height / 2);
  await page.mouse.down();
  await page.mouse.move(end.x + end.width / 2, end.y + Math.min(end.height / 2, 20), { steps: 10 });
  // Playwright hands the page its first dragover on the move after the one that started the drag.
  await page.mouse.move(end.x + end.width / 2 + 1, end.y + Math.min(end.height / 2, 20));
}

async function openRouting(page: Page): Promise<void> {
  await page.goto(`${server.url}/#/routing/loopback-0`);
  await expect(page.getByTestId("source-0-0")).toBeEnabled();
  await expect.poll(() => slots(page)).toEqual(["6"]);
}

test("a source dragged onto the dock becomes a channel fed by it in the dock's mix; a run adds one channel each", async ({ page }) => {
  await putWorkspace(server, { mixers: MIXERS });
  await openRouting(page);

  // While the drag is over it the dock is outlined and says what a drop will do.
  await dragOver(page, page.getByTestId("source-0-2"), dock(page));
  await expect(dock(page)).toHaveAttribute("data-drop", "add");
  await expect(dock(page).getByTestId("dock-drop-hint")).toHaveText("Drop to add a channel to Monitors");
  await page.mouse.up();
  await expect(dock(page)).not.toHaveAttribute("data-drop", /.*/);
  await expect(dock(page).getByTestId("dock-drop-hint")).toBeHidden();

  await expect.poll(() => slots(page)).toEqual(["6", "7"]);
  const added = dock(page).locator('ga-strip[strip="7"]');
  await expect(added).toHaveAttribute("label", "PREAMP 3");
  await expect(added).toHaveAttribute("input-group", "0");
  await expect(added).toHaveAttribute("input-channel", "2");
  await expect.poll(saved).toEqual([
    { slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] },
    { slot: 7, source: { group: 0, channel: 2 }, main_mix: 0, sends: [] },
  ]);

  // A selected run: USB 1 PLAY 1 and 2, dragged with the browser's own drag and drop.
  await page.getByTestId("source-1-0").click();
  await page.getByTestId("source-1-1").click({ modifiers: ["Shift"] });
  await page.getByTestId("source-1-1").dragTo(dock(page));
  await expect.poll(() => slots(page)).toEqual(["6", "7", "8", "9"]);
  await expect(dock(page).locator('ga-strip[strip="8"]')).toHaveAttribute("label", "USB 1 PLAY 1");
  await expect(dock(page).locator('ga-strip[strip="9"]')).toHaveAttribute("label", "USB 1 PLAY 2");

  // The Mixer page has them too, with their inputs chosen.
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  await expect(page.locator("ga-channel")).toHaveCount(4);
  await expect(page.getByTestId("in-7")).toHaveValue("0:2");
  await expect(page.getByTestId("out-7")).toHaveValue("0");
});

test("a drop that would put an input into the mix twice waits behind Confirm; Cancel adds nothing", async ({ page }) => {
  await putWorkspace(server, { mixers: MIXERS });
  await openRouting(page);

  await page.getByTestId("source-0-0").dragTo(dock(page));
  const reason = dock(page).getByTestId("dock-drop-reason");
  await expect(reason).toBeVisible();
  await expect(reason).toContainText("Monitors: PREAMP 1 is in this mix twice, so it is summed twice");
  await page.waitForTimeout(300);
  expect(await slots(page)).toEqual(["6"]);
  await dock(page).getByTestId("dock-drop-cancel").click();
  await expect(reason).toBeHidden();
  await page.waitForTimeout(300);
  expect(await slots(page)).toEqual(["6"]);

  await page.getByTestId("source-0-0").dragTo(dock(page));
  await dock(page).getByTestId("dock-drop-confirm").click();
  await expect(reason).toBeHidden();
  await expect.poll(() => slots(page)).toEqual(["6", "7"]);
  await expect(dock(page).locator('ga-strip[strip="7"]')).toHaveAttribute("label", "PREAMP 1");
});

test("a dock showing a surface refuses the drop and says why; a folded dock opens when a drag rests on it", async ({ page }) => {
  await putWorkspace(server, { mixers: MIXERS, surfaces: [{ id: "s", name: "Outs", mixes: {}, strips: [] }] });
  await openRouting(page);

  await dock(page).getByTestId("dock-source-select").selectOption("s");
  await dragOver(page, page.getByTestId("source-0-3"), dock(page));
  await expect(dock(page)).toHaveAttribute("data-drop", "refused");
  await expect(dock(page).getByTestId("dock-drop-hint")).toHaveText("Showing a surface: set Show to This device to add channels");
  await page.mouse.up();
  await page.waitForTimeout(300);
  expect(await saved()).toHaveLength(1);

  // Back to the device, folded: a drag held on the dock's bar opens it, and the drop lands.
  await dock(page).getByTestId("dock-source-select").selectOption("");
  await dock(page).getByRole("button", { name: "Mixer", exact: true }).click();
  await expect(dock(page).getByTestId("dock-strips")).toBeHidden();
  await dragOver(page, page.getByTestId("source-0-3"), dock(page));
  await expect(dock(page).getByTestId("dock-strips")).toBeVisible();
  // A browser repeats dragover while the pointer rests; Playwright only on a move, so move a little
  // over the dock as it now stands.
  const opened = await dock(page).boundingBox();
  if (opened === null) throw new Error("no dock");
  await page.mouse.move(opened.x + opened.width / 2, opened.y + opened.height / 2);
  await page.mouse.up();
  await expect.poll(() => slots(page)).toEqual(["6", "7"]);
  await expect(dock(page).locator('ga-strip[strip="7"]')).toHaveAttribute("label", "PREAMP 4");
});
