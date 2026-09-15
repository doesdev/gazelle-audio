// Reference screenshots for design review: the devices and workspace pages in each bundled theme,
// written to web/test-results/design. That is outside Playwright's output directory, which every
// run clears. They are for people to look at, not compared.

import { expect, test } from "@playwright/test";

import { REPO_ROOT, startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--dry-run", "--loopback-cyclic-ms", "50"], { webUi: true });
  // A plausible session per model: named channels on preamps, playback and digital inputs, some sending to a second mix, one not set up yet.
  const channel = (id: string, name: string, slot: number, group: number | undefined, source: number, main: number | undefined, sends: number[] = []) => ({ id, name, slot, sends, ...(group === undefined ? {} : { source: { group, channel: source } }), ...(main === undefined ? {} : { main_mix: main }) });
  const mixers = {
    "loopback-0": {
      mixes: [{ name: "Monitors" }, { name: "Cue" }],
      channels: [channel("q1", "Vox", 6, 0, 0, 0, [1]), channel("q2", "Guitar DI", 7, 0, 1, 0, [1]), channel("q3", "DAW L", 8, 1, 0, 0, [1]), channel("q4", "DAW R", 9, 1, 1, 0, [1]), channel("q5", "Synth", 10, 3, 0, 0), channel("q6", "Click", 11, 1, 2, 1), channel("q7", "", 12, undefined, 0, undefined)],
    },
    "loopback-1": {
      mixes: [{ name: "Main" }, { name: "Artist" }],
      groups: [{ id: "drums", name: "Drums", collapsed: false, color: "#b5473a" }],
      channels: [{ ...channel("s1", "Kick", 0, 0, 0, 0, [1]), group: "drums" }, { ...channel("s2", "Snare", 1, 0, 1, 0, [1]), group: "drums" }, { ...channel("s3", "OH L", 2, 0, 2, 0), group: "drums" }, { ...channel("s4", "OH R", 3, 0, 3, 0), group: "drums" }, channel("s5", "Bass", 4, 1, 0, 0, [1]), channel("s6", "Keys", 5, 4, 0, 0, [1]), channel("s7", "Tracks", 6, 3, 0, 0, [1]), channel("s8", "Talkback", 7, 0, 4, 1)],
    },
  };
  const response = await fetch(`${server.url}/api/v1/workspace`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ version: 1, groups: [], links: [], aliases: {}, mixers }) });
  if (!response.ok) throw new Error(`workspace PUT failed: ${response.status}`);
});

test.afterAll(async () => {
  await server?.stop();
});

for (const theme of ["gazelle-dark", "gazelle-light", "community:studio-blue"]) {
  test(`screenshots in ${theme}`, async ({ page }) => {
    await page.goto(`${server.url}/#/devices/loopback-0`);
    await page.getByLabel("Theme").selectOption(theme);
    await expect(page.locator('ga-device-status [data-field="current_preset"]')).not.toHaveText("—");
    await page.evaluate(() => document.fonts.ready);
    const file = theme.replace(":", "-");
    await page.screenshot({ path: `${REPO_ROOT}/web/test-results/design/${file}-devices.png` });
    await page.goto(`${server.url}/#/workspace`);
    await expect(page.locator("ga-workspace input").first()).toBeVisible();
    await page.screenshot({ path: `${REPO_ROOT}/web/test-results/design/${file}-workspace.png` });
    for (const [device, name, slot] of [["loopback-0", "quadro", 7], ["loopback-1", "studio", 1]] as const) {
      await page.goto(`${server.url}/#/mixer/${device}`);
      const fader = page.getByTestId(`fader-${slot}`);
      await fader.focus();
      for (let i = 0; i < 2; i++) await fader.press("PageDown");
      await expect(page.locator(`ga-channel[data-channel-slot="${slot}"] ga-strip .readout`).nth(1)).not.toHaveText("—");
      await page.screenshot({ path: `${REPO_ROOT}/web/test-results/design/${file}-mixer-${name}.png` });
    }
    for (const [device, name] of [["loopback-0", "quadro"], ["loopback-1", "studio"]] as const) {
      await page.goto(`${server.url}/#/routing/${device}`);
      await expect(page.getByTestId("source-0-0")).toBeVisible();
      // A couple of routes, so the destinations show sources.
      await page.getByTestId("source-0-0").click();
      await page.getByTestId("source-0-1").click({ modifiers: ["Shift"] });
      await page.getByTestId("dest-1-0").click();
      await page.getByTestId("source-1-0").click();
      await page.getByTestId("source-1-1").click({ modifiers: ["Shift"] });
      await page.getByTestId("dest-3-0").click();
      await expect(page.getByTestId("dest-3-1")).not.toHaveText("?");
      await page.screenshot({ path: `${REPO_ROOT}/web/test-results/design/${file}-routing-${name}.png`, fullPage: true });
    }
    for (const [device, name] of [["loopback-0", "quadro"], ["loopback-1", "studio"]] as const) {
      await page.goto(`${server.url}/#/inputs/${device}`);
      await expect(page.getByTestId("preamp-0")).toBeVisible();
      await page.screenshot({ path: `${REPO_ROOT}/web/test-results/design/${file}-inputs-${name}.png`, fullPage: true });
    }
    await page.goto(`${server.url}/#/mixer/loopback-1`);
    // Both side panels collapsed, at a width where auto strips stretch past the floor.
    await page.getByRole("button", { name: "Collapse the devices panel" }).click();
    await page.getByRole("button", { name: "Collapse the meters panel" }).click();
    await page.setViewportSize({ width: 2200, height: 1000 });
    await page.screenshot({ path: `${REPO_ROOT}/web/test-results/design/${file}-mixer-collapsed.png` });
    await page.getByRole("button", { name: "Expand the devices panel" }).click();
    await page.getByRole("button", { name: "Expand the meters panel" }).click();
  });
}
