// `pnpm -C web e2e`: Chromium against the real gazelle-audio-server serving the built UI.
// Global setup builds the web app; each spec starts its own server through the client's
// integration harness, which builds the server once so it embeds that build.

import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: ".",
  testMatch: "*.spec.ts",
  globalSetup: "./global-setup.ts",
  outputDir: "../../../test-results/e2e",
  timeout: 90_000,
  workers: 1,
  // On CI (GitHub sets CI=true) a failure is tried twice more: the Windows runners have shown
  // transient socket errors (ERR_NO_BUFFER_SPACE) and slow saves no local run reproduces. A test
  // that passes on a retry is still reported as flaky, with its first failure's trace kept, and
  // the github reporter puts it on the run's summary, so a flake stays visible. Locally: no retries.
  retries: process.env["CI"] ? 2 : 0,
  reporter: process.env["CI"] ? [["list"], ["github"]] : [["list"]],
  use: {
    ...devices["Desktop Chrome"],
    viewport: { width: 1400, height: 860 },
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
});
