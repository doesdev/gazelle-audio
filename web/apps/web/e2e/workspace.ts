// Workspace state for e2e specs. Each spec starts one server with --no-persist, so its workspace
// lives in memory and carries from test to test; a test that fails halfway (before undoing a link,
// say) would otherwise leave state that breaks the tests after it. Specs call `resetWorkspace` in
// beforeEach, and tests that need a layout write the whole workspace with `putWorkspace`.

import type { RunningServer } from "../../../packages/client/test/integration/server.ts";

export interface WorkspaceParts {
  groups?: unknown[];
  links?: unknown[];
  aliases?: Record<string, string>;
  mixers?: Record<string, unknown>;
  layouts?: unknown[];
  device_colors?: Record<string, string>;
  surfaces?: unknown[];
  cables?: unknown[];
}

/** Replaces the server's workspace with these parts; anything not given is empty. */
export async function putWorkspace(server: RunningServer, parts: WorkspaceParts = {}): Promise<void> {
  const body = { version: 1, groups: [], links: [], aliases: {}, mixers: {}, ...parts };
  const response = await fetch(`${server.url}/api/v1/workspace`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`workspace PUT failed: ${response.status} ${await response.text()}`);
}

/** An empty workspace: no aliases, links or mixer layouts. */
export function resetWorkspace(server: RunningServer): Promise<void> {
  return putWorkspace(server);
}
