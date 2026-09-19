// The recall preview on the Workspace page: what putting a snapshot back would send, in order,
// with its guards — and nothing sent.
//
// This server is **not** in dry run (reads answer nothing there) and it *does* push
// cyclic reports, unlike the snapshots spec: recall is mostly about the values that arrive only in
// the 0x73 report — 48V, gains, volumes, mutes, the clock — and without them a plan would be all
// "could not be read". The loopback's report sweeps, so a snapshot taken a moment ago differs from
// the present everywhere, which is exactly the busy plan worth looking at. Nothing here is a
// device, and nothing here applies a plan: there is no control on the page that could.

import { expect, test, type Page } from "@playwright/test";

import { REPO_ROOT, startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { resetWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--backend", "loopback", "--loopback-cyclic-ms", "200"], { webUi: true });
});

test.afterAll(async () => {
  await server?.stop();
});

test.beforeEach(async () => {
  await resetWorkspace(server);
  const listed = (await (await fetch(`${server.url}/api/v1/snapshots`)).json()) as { snapshots: { id: string }[] };
  for (const snapshot of listed.snapshots) await fetch(`${server.url}/api/v1/snapshots/${snapshot.id}`, { method: "DELETE" });
});

const workspace = (page: Page) => page.locator("ga-workspace");

/**
 * Takes a snapshot once both devices have reported their state, so it holds the values recall is
 * about, and returns its id.
 */
async function take(page: Page, name: string): Promise<string> {
  await expect
    .poll(
      async () => {
        const created = (await (await fetch(`${server.url}/api/v1/snapshots`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ name }) })).json()) as { id: string };
        const whole = (await (await fetch(`${server.url}/api/v1/snapshots/${created.id}`)).json()) as { devices: Record<string, { unreadable: unknown[] }> };
        if (Object.values(whole.devices).every((device) => device.unreadable.length === 0)) return created.id;
        await fetch(`${server.url}/api/v1/snapshots/${created.id}`, { method: "DELETE" });
        return undefined;
      },
      { timeout: 20_000 },
    )
    .toBeDefined();
  const listed = (await (await fetch(`${server.url}/api/v1/snapshots`)).json()) as { snapshots: { id: string; name: string }[] };
  const found = listed.snapshots.find((snapshot) => snapshot.name === name);
  if (found === undefined) throw new Error(`no snapshot named ${name}`);
  await page.goto(`${server.url}/#/workspace`);
  await expect(workspace(page).getByLabel(`Name of ${name}`)).toHaveValue(name);
  return found.id;
}

/** Compares, then asks for the plan, and waits for it to be on the page. */
async function preview(page: Page, id: string): Promise<void> {
  await workspace(page).getByTestId(`snapshot-compare-${id}`).click();
  await expect(workspace(page).getByTestId("snapshot-diff-summary")).toBeVisible();
  await workspace(page).getByTestId("snapshot-recall-prepare").click();
  await expect(workspace(page).getByTestId("snapshot-recall-summary")).toBeVisible({ timeout: 15_000 });
}

test("the preview lists what recall would send, in order, and says nothing was sent", async ({ page }) => {
  const id = await take(page, "Drum tracking");
  await preview(page, id);

  const plan = workspace(page).getByTestId("snapshot-recall-plan");
  await expect(plan.getByTestId("snapshot-recall-summary")).toContainText("Recall preview");
  await expect(plan.getByTestId("snapshot-recall-nothing-sent")).toContainText("Nothing has been sent to any device");

  // The parts are drawn in the order recall would run them, silencing first and restoring last.
  const parts = await plan.locator('[data-testid^="snapshot-recall-part-"]').evaluateAll((nodes) => nodes.map((node) => node.getAttribute("data-testid")!.replace("snapshot-recall-part-", "")));
  const order = ["silence", "clock", "settings", "dc_coupling", "inputs", "phantom", "routing", "mixer", "outputs", "restore"];
  expect(parts.length).toBeGreaterThan(3);
  expect(parts).toEqual([...parts].sort((a, b) => order.indexOf(a) - order.indexOf(b)));
  expect(parts[0]).toBe("silence");
  expect(parts.at(-1)).toBe("restore");

  // Each step carries the bytes it would send, which is the whole point of a preview.
  await expect(plan.getByTestId("snapshot-recall-part-silence")).toContainText("set_hard_mute");
  const steps = plan.locator(".step");
  expect(await steps.count()).toBeGreaterThan(5);
  await expect(steps.first()).toContainText("bytes");

  // And no control on the page applies one.
  await expect(page.getByRole("button", { name: /^(Recall|Apply)/ })).toHaveCount(0);
});

test("the dangerous parts are off, and a step they hold back says so", async ({ page }) => {
  const id = await take(page, "Before the session");
  await preview(page, id);
  const plan = workspace(page).getByTestId("snapshot-recall-plan");

  // 48V and the clock are off until they are ticked; the mixer is on.
  await expect(plan.getByTestId("snapshot-recall-part-clock")).toContainText("off by default");
  await expect(plan.getByTestId("snapshot-recall-part-clock")).toContainText("needs its own confirmation");
  await expect(plan.getByTestId("snapshot-recall-part-clock")).toContainText("Held back: Clock is switched off");
  // The inputs are on by default, so nothing holds their steps back.
  await expect(plan.getByTestId("snapshot-recall-part-inputs")).not.toContainText("Held back");

  // What is not recalled is said with its reason, grouped by why.
  await expect(plan.getByTestId("snapshot-recall-excluded-withheld")).toContainText("held back until a session at the hardware");
  await expect(plan.getByTestId("snapshot-recall-excluded-withheld")).toContainText("set_adat_gain");
  // The Quadro reports six volume words and binds four of them; the other two are named, not dropped.
  await expect(plan.getByTestId("snapshot-recall-excluded-no_writer")).toContainText("bound to no command");
});

test("a reference screenshot of the recall preview", async ({ page }) => {
  const id = await take(page, "Drum tracking");
  // Something in every section a person would recognise, so the picture is of a real recall.
  const set = (device: string, command: string, body: unknown) =>
    fetch(`${server.url}/api/v1/devices/${device}/command/${command}`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
  await set("loopback-0", "set_panning_law", { panning: 2 });
  await set("loopback-0", "set_mixer", { channel: 1, level: 23 });
  await set("loopback-0", "set_routing", { bank_idx: 0, bank_configs: "0100".repeat(32) });

  // The loopback's report shifts every value by one each tick, so a few seconds put the output
  // levels more than the 6 dB the user asked to be told about away from the snapshot's.
  await page.waitForTimeout(4_200);

  await preview(page, id);
  const plan = workspace(page).getByTestId("snapshot-recall-plan");
  await expect(plan.getByTestId("snapshot-recall-part-silence")).toBeVisible();
  await expect(plan.getByTestId("snapshot-recall-part-inputs")).toBeVisible();
  await expect(plan.getByTestId("snapshot-recall-part-routing")).toBeVisible();
  // Guards raised: an output raised by more than 6 dB, and a part that is off holding steps back.
  await expect(plan.locator('[data-testid^="snapshot-recall-raised-"]').first()).toContainText("would be raised by");
  await expect(plan.locator(".step-held").first()).toBeVisible();
  await expect(plan.getByTestId("snapshot-recall-nothing-sent")).toBeVisible();

  // The plan itself, not the comparison above it: the parts, their guards and their bytes.
  await plan.getByTestId("snapshot-recall-part-clock").scrollIntoViewIfNeeded();
  await page.evaluate(() => document.fonts.ready);
  await page.screenshot({ path: `${REPO_ROOT}/web/test-results/design/recall-preview.png` });
});
