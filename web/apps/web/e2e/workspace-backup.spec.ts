// Workspace export and import on the Workspace page: an export downloads the server's workspace as
// a dated JSON file; an import reads a chosen file, asks before replacing, and lets the server's own
// validation decide. A workspace is layout only, so none of this sends anything to a device.

import { expect, test, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";

import { startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { putWorkspace, resetWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--dry-run"], { webUi: true });
});

test.afterAll(async () => {
  await server?.stop();
});

test.beforeEach(() => resetWorkspace(server));

const serverWorkspace = async (): Promise<unknown> => (await fetch(`${server.url}/api/v1/workspace`)).json();

/** A workspace using every part the server keeps, including channel colours. */
const SEED = {
  aliases: { "loopback-0": "Desk Quadro", "loopback-1": "Drum Studio" },
  groups: [{ id: "g-drums", name: "Drums", collapsed: false, hidden: false, color: "#b5473a", members: [{ device_id: "loopback-1", channel: 0 }], children: [] }],
  links: [{ id: "l-oh", kind: "preamp", mode: "absolute", members: [{ device_id: "loopback-1", channel: 2 }, { device_id: "loopback-1", channel: 3 }] }],
  mixers: {
    "loopback-0": {
      mixes: [{ name: "Cue" }],
      groups: [{ id: "mg", name: "Kit", collapsed: false, color: "#3a7bb5" }],
      channels: [{ id: "c-kick", name: "Kick", group: "mg", color: "#aabbcc", slot: 6, source: { group: 3, channel: 0 }, main_mix: 0, sends: [1] }],
    },
  },
  layouts: [{ id: "lay", name: "Tracking", family: "studio", mixer: { mixes: [], groups: [], channels: [] } }],
};

/** Records every command the page sends to a device. */
function commandsSent(page: Page): string[] {
  const sent: string[] = [];
  page.on("websocket", (socket) =>
    socket.on("framesent", (event) => {
      if (typeof event.payload !== "string") return;
      const frame = JSON.parse(event.payload) as { command?: string };
      if (frame.command !== undefined) sent.push(frame.command);
    }),
  );
  return sent;
}

const aliasField = (page: Page, device: string) => page.getByLabel(`Name for ${device}`);

test("an exported workspace imports back exactly, after confirming, without touching a device", async ({ page }) => {
  await putWorkspace(server, SEED);
  const saved = await serverWorkspace();
  await page.goto(`${server.url}/#/workspace`);
  await expect(aliasField(page, "loopback-0")).toHaveValue("Desk Quadro");

  const downloading = page.waitForEvent("download");
  await page.getByTestId("workspace-export").click();
  const download = await downloading;
  expect(download.suggestedFilename()).toMatch(/^gazelle-workspace-\d{4}-\d{2}-\d{2}\.json$/);
  const text = readFileSync((await download.path())!, "utf8");
  expect(JSON.parse(text)).toEqual(saved);

  await resetWorkspace(server);
  // Listening before the reload, which opens the page's WebSocket.
  const commands = commandsSent(page);
  await page.reload();
  await expect(aliasField(page, "loopback-0")).toHaveValue("");
  const sentBefore = commands.length;
  await page.getByTestId("workspace-import-file").setInputFiles({ name: download.suggestedFilename(), mimeType: "application/json", buffer: Buffer.from(text) });

  const confirm = page.getByTestId("workspace-import-confirm");
  await expect(confirm).toContainText(download.suggestedFilename());
  await expect(confirm).toContainText("2 device names, 1 group, 1 link, 1 mixer layout and 1 saved layout");
  expect(await serverWorkspace(), "nothing is replaced before confirming").toEqual({ version: 1, groups: [], links: [], aliases: {}, mixers: {}, layouts: [] });

  await page.getByTestId("workspace-import-replace").click();
  await expect(page.getByTestId("workspace-import-status")).toHaveText(`Imported ${download.suggestedFilename()}.`);
  await expect(confirm).toHaveCount(0);
  expect(await serverWorkspace()).toEqual(saved);
  await expect(aliasField(page, "loopback-0")).toHaveValue("Desk Quadro");
  await expect(page.getByText("Drums", { exact: true })).toBeVisible();
  expect(commands.slice(sentBefore), "a workspace is layout only").toEqual([]);
});

test("a file the server refuses is not imported, and the server's reason is shown", async ({ page }) => {
  await putWorkspace(server, { aliases: { "loopback-0": "Kept" } });
  const before = await serverWorkspace();
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(`${server.url}/#/workspace`);
  const file = page.getByTestId("workspace-import-file");
  const problem = page.getByTestId("workspace-import-problem");

  // Not JSON at all: refused on the page, with nothing to confirm.
  await file.setInputFiles({ name: "notes.txt", mimeType: "text/plain", buffer: Buffer.from("drums on ADAT 1-8") });
  await expect(problem).toHaveText("notes.txt was not imported: it is not JSON.");
  await expect(page.getByTestId("workspace-import-confirm")).toHaveCount(0);
  await page.waitForTimeout(200);
  expect(errors, "a refused file stops cleanly").toEqual([]);

  // A well-formed document the server's validation refuses: a link needs two channels.
  const invalid = { version: 1, groups: [], aliases: {}, mixers: {}, links: [{ id: "solo", kind: "preamp", mode: "absolute", members: [{ device_id: "loopback-0", channel: 0 }] }] };
  await file.setInputFiles({ name: "bad.json", mimeType: "application/json", buffer: Buffer.from(JSON.stringify(invalid)) });
  await expect(problem).toHaveCount(0);
  await page.getByTestId("workspace-import-replace").click();
  await expect(problem).toHaveText("bad.json was not imported. The server refused it: bad value: link 'solo': a link needs at least two channels");
  expect(await serverWorkspace()).toEqual(before);

  // A document of the right outline with a part the server cannot read: its reason names the part.
  const unreadable = { version: 1, groups: [{ id: "g" }], links: [], aliases: {}, mixers: {} };
  await file.setInputFiles({ name: "part.json", mimeType: "application/json", buffer: Buffer.from(JSON.stringify(unreadable)) });
  await page.getByTestId("workspace-import-replace").click();
  await expect(problem).toContainText("part.json was not imported. The server refused it: bad value: not a workspace: groups[0]: missing field `name`");
  expect(await serverWorkspace()).toEqual(before);
  await expect(aliasField(page, "loopback-0")).toHaveValue("Kept");
});

test("cancelling an import leaves the workspace unchanged and saves nothing", async ({ page }) => {
  await putWorkspace(server, { aliases: { "loopback-0": "Before" } });
  const before = await serverWorkspace();
  await page.goto(`${server.url}/#/workspace`);
  await expect(aliasField(page, "loopback-0")).toHaveValue("Before");
  const puts: string[] = [];
  page.on("request", (request) => {
    if (request.method() === "PUT") puts.push(request.url());
  });

  const other = { version: 1, groups: [], links: [], mixers: {}, aliases: { "loopback-0": "After" } };
  await page.getByTestId("workspace-import-file").setInputFiles({ name: "other.json", mimeType: "application/json", buffer: Buffer.from(JSON.stringify(other)) });
  await expect(page.getByTestId("workspace-import-confirm")).toContainText("1 device name");
  await page.getByTestId("workspace-import-cancel").click();

  await expect(page.getByTestId("workspace-import-confirm")).toHaveCount(0);
  await page.waitForTimeout(500);
  expect(puts).toEqual([]);
  expect(await serverWorkspace()).toEqual(before);
  await expect(aliasField(page, "loopback-0")).toHaveValue("Before");
});
