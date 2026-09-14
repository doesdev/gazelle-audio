// `pnpm -C web e2e`: Chromium against the real gazelle-audio-server serving the built UI (spec §9,
// §10 row 3). Global setup builds the web app; each spec starts its own server through the client's
// integration harness, which builds the server once so it embeds that build.

import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: ".",
  testMatch: "*.spec.ts",
  globalSetup: "./global-setup.ts",
  outputDir: "../../../test-results/e2e",
  timeout: 90_000,
  workers: 1,
  reporter: [["list"]],
  use: {
    ...devices["Desktop Chrome"],
    viewport: { width: 1400, height: 860 },
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
});
