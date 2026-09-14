// Reference screenshots for design review: the devices and workspace pages in each bundled theme,
// written to test-results/e2e/screenshots. They are for people to look at, not compared.

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
    await page.screenshot({ path: `${REPO_ROOT}/web/test-results/e2e/screenshots/${file}-devices.png` });
    await page.goto(`${server.url}/#/workspace`);
    await expect(page.locator("ga-workspace input").first()).toBeVisible();
    await page.screenshot({ path: `${REPO_ROOT}/web/test-results/e2e/screenshots/${file}-workspace.png` });
  });
}
