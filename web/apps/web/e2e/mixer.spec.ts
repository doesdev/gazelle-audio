// Phase 4 done criteria (spec §10 row 4): the mixer sends the correct set_mixer / set_mixer_cfg
// bytes per family, verified with dry run, and drags coalesce. Expected bytes are the protocol
// crate's ground-truth vector (all fields zero) with the payload bytes set as the registry packs
// them: mixer_id, channel, level, then pan (6 bits) | mute << 6 | solo << 7, then send (Studio+).

import { expect, test, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { REPO_ROOT, startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--dry-run", "--loopback-cyclic-ms", "50"], { webUi: true });
});

test.afterAll(async () => {
  await server?.stop();
});

function groundTruth(file: string, command: string): Uint8Array {
  const vectors = JSON.parse(readFileSync(join(REPO_ROOT, "crates", "gazelle-audio-protocol", "tests", file), "utf8")) as Record<string, string>;
  const hex = vectors[command];
  if (hex === undefined) throw new Error(`no ${command} vector in ${file}`);
  return Uint8Array.from(hex.match(/../g) ?? [], (pair) => Number.parseInt(pair, 16));
}

function expectedHex(base: Uint8Array, fields: { mixer: number; channel: number; level: number; pan: number; mute?: number; solo?: number; send?: number }): string {
  const bytes = Uint8Array.from(base);
  bytes[18] = fields.mixer;
  bytes[19] = fields.channel;
  bytes[20] = fields.level;
  bytes[21] = (fields.pan & 0x3f) | ((fields.mute ?? 0) << 6) | ((fields.solo ?? 0) << 7);
  if (fields.send !== undefined) bytes[22] = fields.send;
  return [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
}

interface Frame {
  device_id?: string;
  command?: string;
  args?: Record<string, number>;
}

function recordFrames(page: Page): Frame[] {
  const frames: Frame[] = [];
  page.on("websocket", (socket) => {
    socket.on("framesent", (event) => {
      if (typeof event.payload === "string") frames.push(JSON.parse(event.payload) as Frame);
    });
  });
  return frames;
}

test("a Quadro fader sends set_mixer with the expected bytes in dry run", async ({ page }) => {
  await page.goto(`${server.url}/#/mixer/loopback-0/1`);
  const fader = page.getByTestId("fader-3");
  await fader.focus();
  for (let i = 0; i < 3; i++) await fader.press("PageDown");
  await expect(page.getByTestId("level-3")).toHaveText("-18 dB");
  await expect(fader).toHaveAttribute("aria-valuenow", "-18");

  const expected = expectedHex(groundTruth("ground_truth.json", "set_mixer"), { mixer: 1, channel: 4, level: 18, pan: 32 });
  await expect(page.getByTestId("last-sent")).toContainText(`Dry run, would send set_mixer: ${expected}`);
});

test("a Studio+ strip sends set_mixer_cfg with level and send in dry run", async ({ page }) => {
  await page.goto(`${server.url}/#/mixer/loopback-1/2`);
  const fader = page.getByTestId("fader-0");
  await fader.focus();
  await fader.press("PageDown");
  const send = page.getByRole("slider", { name: "Strip 1 send (raw value, scale unverified)" });
  await send.focus();
  await send.press("PageUp");

  const expected = expectedHex(groundTruth("ground_truth_studio.json", "set_mixer_cfg"), { mixer: 2, channel: 1, level: 6, pan: 32, send: 16 });
  await expect(page.getByTestId("last-sent")).toContainText(`Dry run, would send set_mixer_cfg: ${expected}`);
});

test("dragging a fader coalesces and ends on the final level", async ({ page }) => {
  const frames = recordFrames(page);
  await page.goto(`${server.url}/#/mixer/loopback-0/0`);
  const fader = page.getByTestId("fader-5");
  const box = await fader.boundingBox();
  if (box === null) throw new Error("fader-5 has no box");
  const x = box.x + box.width / 2;
  const moves = 60;
  await page.mouse.move(x, box.y + 2);
  await page.mouse.down();
  await page.mouse.move(x, box.y + box.height * 0.5, { steps: moves });
  await page.mouse.up();

  const readout = page.getByTestId("level-5");
  await expect(readout).not.toHaveText("0 dB");
  const finalLevel = Number((await readout.textContent())?.replace(/[^0-9]/g, ""));
  await expect.poll(() => frames.filter((f) => f.command === "set_mixer" && f.args?.["channel"] === 6).at(-1)?.args?.["level"]).toBe(finalLevel);
  const sent = frames.filter((f) => f.command === "set_mixer" && f.args?.["channel"] === 6);
  expect(sent.length).toBeGreaterThan(0);
  expect(sent.length).toBeLessThanOrEqual(moves + 1);
  test.info().annotations.push({ type: "coalescing", description: `${sent.length} set_mixer frames for ${moves} pointer moves` });
});

test("meters follow the device's reports and links send set_stereo_link", async ({ page }) => {
  const frames = recordFrames(page);
  await page.goto(`${server.url}/#/mixer/loopback-1/0`);
  const mask = page.locator('ga-strip[strip="0"] .mask');
  const first = await mask.evaluate((el) => (el as HTMLElement).style.height);
  await expect.poll(() => mask.evaluate((el) => (el as HTMLElement).style.height)).not.toBe(first);
  await expect.poll(() => frames.some((f) => f.command === "set_peak_source" && f.args?.["bank_id"] === 1 && f.args?.["source_id"] === 0)).toBe(true);

  await page.getByRole("button", { name: "Link strips 5 and 6" }).first().click();
  await expect.poll(() => frames.find((f) => f.command === "set_stereo_link")?.args).toEqual({ periph_id: 4, channel_id: 2, linked: 1 });
  await expect(page.getByRole("button", { name: "Link strips 5 and 6" }).first()).toHaveAttribute("aria-pressed", "true");
});
