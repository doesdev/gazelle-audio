// Builds apps/web/dist before any server is started, so the server the harness builds embeds the
// current UI (its build script watches the dist directory).

import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

export default function globalSetup(): void {
  const build = fileURLToPath(new URL("../scripts/build.ts", import.meta.url));
  const result = spawnSync(process.execPath, [build], { stdio: "inherit" });
  if (result.status !== 0) throw new Error(`building the web app failed (${result.error?.message ?? `exit ${result.status}`})`);
}
