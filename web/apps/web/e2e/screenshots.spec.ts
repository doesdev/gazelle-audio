// Reference screenshots for design review: the devices and workspace pages in each bundled theme,
// written to web/test-results/design. That is outside Playwright's output directory, which every
// run clears. They are for people to look at, not compared.

import { expect, test } from "@playwright/test";

import { REPO_ROOT, startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--dry-run", "--loopback-cyclic-ms", "50"], { webUi: true });
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
    for (const [device, name] of [["loopback-0", "quadro"], ["loopback-1", "studio"]] as const) {
      await page.goto(`${server.url}/#/mixer/${device}/0`);
      const fader = page.getByTestId("fader-2");
      await fader.focus();
      for (let i = 0; i < 2; i++) await fader.press("PageDown");
      await expect(page.locator('ga-strip[strip="0"] .readout').nth(1)).not.toHaveText("—");
      await page.screenshot({ path: `${REPO_ROOT}/web/test-results/design/${file}-mixer-${name}.png` });
    }
    for (const [device, name] of [["loopback-0", "quadro"], ["loopback-1", "studio"]] as const) {
      await page.goto(`${server.url}/#/inputs/${device}`);
      await expect(page.getByTestId("preamp-0")).toBeVisible();
      await page.screenshot({ path: `${REPO_ROOT}/web/test-results/design/${file}-inputs-${name}.png`, fullPage: true });
    }
    await page.goto(`${server.url}/#/mixer/loopback-1/0`);
    // Both side panels collapsed, at a width where auto strips stretch past the floor.
    await page.getByRole("button", { name: "Collapse the devices panel" }).click();
    await page.getByRole("button", { name: "Collapse the meters panel" }).click();
    await page.setViewportSize({ width: 2200, height: 1000 });
    await page.screenshot({ path: `${REPO_ROOT}/web/test-results/design/${file}-mixer-collapsed.png` });
    await page.getByRole("button", { name: "Expand the devices panel" }).click();
    await page.getByRole("button", { name: "Expand the meters panel" }).click();
  });
}
