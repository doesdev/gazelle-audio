// The user-built mixer (plan 2026-09-16) against the loopback in dry run: channels start from the
// device's routing (nothing routed: one inactive channel), a channel works once it has an input and
// a main mix, and its controls send the right bytes: routing for its input, the mixer command for
// its main mix and sends, and its preamp's commands. Expected bytes are the protocol crate's
// ground-truth vectors (all fields zero) with the payload fields set as the registry packs them.

import { expect, test, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { REPO_ROOT, startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { putWorkspace, resetWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--dry-run", "--loopback-cyclic-ms", "50"], { webUi: true });
});

test.beforeEach(() => resetWorkspace(server));

test.afterAll(async () => {
  await server?.stop();
});

type Family = "quadro" | "studio";

function groundTruth(family: Family, command: string): Uint8Array {
  const file = family === "quadro" ? "ground_truth.json" : "ground_truth_studio.json";
  const vectors = JSON.parse(readFileSync(join(REPO_ROOT, "crates", "gazelle-audio-protocol", "tests", file), "utf8")) as Record<string, string>;
  const hex = vectors[command];
  if (hex === undefined) throw new Error(`no ${command} vector in ${file}`);
  return Uint8Array.from(hex.match(/../g) ?? [], (pair) => Number.parseInt(pair, 16));
}

const hexOf = (bytes: Uint8Array) => [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");

/** Mixer command bytes: mixer_id, channel, level, then pan (6 bits) | mute << 6 | solo << 7, then send (Studio+). */
function mixerHex(family: Family, fields: { mixer: number; channel: number; level: number; pan?: number; send?: number }): string {
  const bytes = groundTruth(family, family === "quadro" ? "set_mixer" : "set_mixer_cfg");
  bytes[18] = fields.mixer;
  bytes[19] = fields.channel;
  bytes[20] = fields.level;
  bytes[21] = (fields.pan ?? 32) & 0x3f;
  if (fields.send !== undefined) bytes[22] = fields.send;
  return hexOf(bytes);
}

/** set_routing bytes: bank_idx at 18 (after a two-byte payload header), then 32 (source, channel) pairs, all MUTE but `routed`. */
function routingHex(family: Family, destination: number, routed: Record<number, [number, number]>): string {
  const mute = family === "quadro" ? 10 : 11;
  const bytes = groundTruth(family, "set_routing");
  bytes[18] = destination;
  for (let slot = 0; slot < 32; slot++) {
    const [source, channel] = routed[slot] ?? [mute, 0];
    bytes[19 + 2 * slot] = source;
    bytes[20 + 2 * slot] = channel;
  }
  return hexOf(bytes);
}

/** Replaces the server's workspace with these per-device mixer layouts. */
function layout(mixers: Record<string, unknown>): Promise<void> {
  return putWorkspace(server, { mixers });
}

const channelIn = (page: Page, slot: number) => page.locator(`ga-channel[data-channel-slot="${slot}"]`);
const lastSent = (page: Page) => page.getByTestId("last-sent");
const dryRun = (command: string, hex: string) => `Dry run, would send ${command}: ${hex}`;

interface Frame {
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

test("with nothing routed the mixer starts with one inactive channel, and + adds channels on free inputs", async ({ page }) => {
  await layout({});
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  await expect(page.locator("ga-channel")).toHaveCount(1);
  await expect(channelIn(page, 6)).toHaveAttribute("inactive", "");
  await expect(page.getByTestId("fader-6")).toHaveAttribute("aria-disabled", "true");

  await page.getByTestId("add-channel").click();
  await expect(page.locator("ga-channel")).toHaveCount(2);
  await expect(channelIn(page, 7)).toBeVisible();
});

test("a Quadro channel routes its input to its main mix, then its fader sends set_mixer for that mix and input", async ({ page }) => {
  await layout({ "loopback-0": { channels: [{ id: "a", name: "", slot: 6, sends: [] }] } });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  await page.getByTestId("in-6").selectOption("0:1");
  await expect(page.getByTestId("fader-6")).toHaveAttribute("aria-disabled", "true", { timeout: 1000 });
  await page.getByTestId("out-6").selectOption("1");
  // MIX CH2 is destination 9; slot 6 takes PREAMP (0) channel 2.
  await expect(lastSent(page)).toContainText(dryRun("set_routing", routingHex("quadro", 9, { 6: [0, 1] })));
  await expect(channelIn(page, 6)).not.toHaveAttribute("inactive", "");
  await expect(page.locator('ga-mix-master[mix="1"]')).toBeVisible();

  const fader = page.getByTestId("fader-6");
  await expect(fader).toHaveAttribute("aria-disabled", "false");
  await fader.focus();
  for (let i = 0; i < 3; i++) await fader.press("PageDown");
  await expect(page.getByTestId("level-6")).toHaveText("-18 dB");
  await expect(lastSent(page)).toContainText(dryRun("set_mixer", mixerHex("quadro", { mixer: 1, channel: 7, level: 18 })));
});

test("a Studio+ channel's send routes it into another mix, and the send level sets its level there", async ({ page }) => {
  await layout({ "loopback-1": { channels: [{ id: "a", name: "Vox", slot: 0, source: { group: 0, channel: 0 }, main_mix: 2, sends: [] }] } });
  await page.goto(`${server.url}/#/mixer/loopback-1`);
  const fader = page.getByTestId("fader-0");
  await fader.focus();
  await fader.press("PageDown");
  await expect(lastSent(page)).toContainText(dryRun("set_mixer_cfg", mixerHex("studio", { mixer: 2, channel: 1, level: 6 })));

  await expect(page.getByTestId("send-0-2")).toBeDisabled();
  await page.getByTestId("send-0-1").click();
  // Studio+ MIX CH2 is destination 11.
  await expect(lastSent(page)).toContainText(dryRun("set_routing", routingHex("studio", 11, { 0: [0, 0] })));
  const send = page.getByTestId("send-level-0-1");
  await expect(send).toHaveAttribute("aria-disabled", "false");
  await send.focus();
  await send.press("Home");
  await expect(lastSent(page)).toContainText(dryRun("set_mixer_cfg", mixerHex("studio", { mixer: 1, channel: 1, level: 90 })));
});

test("a channel on a preamp shows that preamp's controls, and they send its commands", async ({ page }) => {
  await layout({ "loopback-0": { channels: [{ id: "a", name: "", slot: 6, source: { group: 0, channel: 2 }, main_mix: 0, sends: [] }, { id: "b", name: "", slot: 7, source: { group: 1, channel: 0 }, main_mix: 0, sends: [] }] } });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  await expect(page.getByTestId("pre-ch-6")).not.toHaveAttribute("data-empty", "");
  await expect(page.getByTestId("pre-ch-7")).toHaveAttribute("data-empty", "");
  const gain = page.getByTestId("pre-gain-ch-6");
  await gain.focus();
  await gain.press("ArrowRight");
  const bytes = groundTruth("quadro", "set_pre_gain");
  bytes[17] = 2;
  bytes[18] = await gain.evaluate((el) => Number.parseInt(el.getAttribute("aria-valuetext") ?? "0", 10) & 0xff);
  await expect(lastSent(page)).toContainText(dryRun("set_pre_gain", hexOf(bytes)));
});

test("channel heads are one height, so faders line up whatever the input and whether or not the channel is set up", async ({ page }) => {
  await layout({ "loopback-0": { channels: [{ id: "a", name: "Vox", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [1] }, { id: "b", name: "DAW", slot: 7, source: { group: 1, channel: 0 }, main_mix: 1, sends: [] }, { id: "c", name: "", slot: 8, sends: [] }] } });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  await expect(page.locator("ga-channel")).toHaveCount(3);
  const top = async (slot: number) => (await page.getByTestId(`fader-${slot}`).boundingBox())?.y ?? -1;
  const [preamp, playback, inactive] = [await top(6), await top(7), await top(8)];
  expect(Math.abs(preamp - playback), "a preamp channel and a playback channel").toBeLessThanOrEqual(1);
  expect(Math.abs(preamp - inactive), "an inactive channel").toBeLessThanOrEqual(1);
});

test("groups: made from a channel's menu, joined, renamed, coloured, collapsed (remembered) and removed, with faders level", async ({ page }) => {
  // Kick is set up, so its name bar takes the group colour; inactive channels keep their grey bar.
  await layout({ "loopback-0": { channels: [{ id: "a", name: "Kick", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }, { id: "b", name: "Bass", slot: 7, sends: [] }, { id: "c", name: "Snare", slot: 8, sends: [] }] } });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  const slots = () => page.locator("ga-channel").evaluateAll((els) => els.map((e) => e.getAttribute("data-channel-slot")));
  const group = page.locator("ga-channel-group");

  await page.getByTestId("group-6").selectOption({ label: "New group…" });
  await expect(group).toHaveCount(1);
  await expect(group.locator('ga-channel[data-channel-slot="6"]')).toHaveCount(1);
  await page.getByTestId("group-8").selectOption({ label: "Group 1" });
  await expect.poll(slots).toEqual(["6", "8", "7"]);
  await expect(group.locator("ga-channel")).toHaveCount(2);

  const top = async (slot: number) => (await page.getByTestId(`fader-${slot}`).boundingBox())?.y ?? -1;
  expect(Math.abs((await top(6)) - (await top(7))), "a grouped and an ungrouped fader").toBeLessThanOrEqual(1);

  const name = page.getByRole("textbox", { name: "Group name" });
  await name.fill("Drums");
  await name.press("Enter");
  await expect(page.getByTestId("group-7").locator("option", { hasText: "Drums" })).toHaveCount(1);
  await page.getByLabel("Drums colour").fill("#b5473a");
  await expect.poll(() => page.locator('ga-channel[data-channel-slot="6"] ga-strip .name').evaluate((el) => getComputedStyle(el).backgroundColor)).toBe("rgb(181, 71, 58)");

  await page.getByRole("button", { name: "Collapse group Drums" }).click();
  await expect(page.locator('ga-channel[data-channel-slot="6"]')).toBeHidden();
  await expect(page.locator('ga-channel[data-channel-slot="7"]')).toBeVisible();
  await expect.poll(async () => ((await (await fetch(`${server.url}/api/v1/workspace`)).json()) as { mixers: Record<string, { groups: { collapsed: boolean }[] }> }).mixers["loopback-0"]?.groups[0]?.collapsed).toBe(true);
  await page.reload();
  await expect(page.getByRole("button", { name: "Expand group Drums" })).toBeVisible();
  await expect(page.locator('ga-channel[data-channel-slot="8"]')).toBeHidden();

  await page.getByRole("button", { name: "Expand group Drums" }).click();
  await page.getByRole("button", { name: "Remove group Drums" }).click();
  await expect(group).toHaveCount(0);
  await expect.poll(slots).toEqual(["6", "8", "7"]);
});

test("a mix master names the mix and sends it to outputs: the menu adds a left/right pair, a chip's × removes it", async ({ page }) => {
  await layout({ "loopback-0": { mixes: [{ name: "Monitors" }], channels: [{ id: "a", name: "Vox", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }] } });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  await expect(page.getByTestId("mix-name-0")).toHaveValue("Monitors");
  await expect(page.getByTestId("mix-outputs-0")).toContainText("Not playing anywhere");

  await page.getByTestId("mix-add-output-0").selectOption({ label: "HP1" });
  // HP1 is destination 1; mix 1's output is source 6 (LOOPBACK HP1): left to channel 1, right to channel 2.
  await expect(lastSent(page)).toContainText(dryRun("set_routing", routingHex("quadro", 1, { 0: [6, 0], 1: [6, 1] })));
  await expect(page.getByTestId("mix-outputs-0")).toContainText("HP1");
  await expect(page.getByTestId("mix-add-output-0").locator("option", { hasText: /^HP1$/ })).toHaveCount(0);

  await page.getByRole("button", { name: "Stop Monitors feeding HP1" }).click();
  await expect(lastSent(page)).toContainText(dryRun("set_routing", routingHex("quadro", 1, {})));
  await expect(page.getByTestId("mix-outputs-0")).toContainText("Not playing anywhere");

  const name = page.getByTestId("mix-name-0");
  await name.fill("Control Room");
  await name.press("Enter");
  await expect(page.getByTestId("out-6").locator("option", { hasText: "Control Room" })).toHaveCount(1);

  const top = async (locator: string) => (await page.locator(locator).boundingBox())?.y ?? -1;
  expect(Math.abs((await top('ga-mix-master[mix="0"] ga-strip')) - (await top('ga-channel[data-channel-slot="6"] ga-strip'))), "the master strip starts level with the channel strips").toBeLessThanOrEqual(1);
});

test("dragging a channel's grip moves it; dropped inside a group it joins, dragged out it leaves; the order is saved", async ({ page }) => {
  await layout({
    "loopback-0": {
      groups: [{ id: "g", name: "Drums", collapsed: false }],
      channels: [{ id: "a", name: "Kick", slot: 6, group: "g", sends: [] }, { id: "b", name: "Snare", slot: 7, group: "g", sends: [] }, { id: "c", name: "Bass", slot: 8, sends: [] }, { id: "d", name: "Keys", slot: 9, sends: [] }],
    },
  });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  const slots = () => page.locator("ga-channel").evaluateAll((els) => els.map((e) => e.getAttribute("data-channel-slot")));
  await expect.poll(slots).toEqual(["6", "7", "8", "9"]);
  const drag = async (slot: number, onto: number, fraction: number) => {
    const grip = await page.locator(`ga-channel[data-channel-slot="${slot}"] [data-grip]`).boundingBox();
    const target = await page.locator(`ga-channel[data-channel-slot="${onto}"]`).boundingBox();
    if (grip === null || target === null) throw new Error("no box to drag");
    await page.mouse.move(grip.x + grip.width / 2, grip.y + grip.height / 2);
    await page.mouse.down();
    await page.mouse.move(target.x + target.width * fraction, target.y + 60, { steps: 8 });
    await expect(page.locator("ga-mixer .drop")).toBeVisible();
    await page.mouse.up();
  };

  await drag(9, 6, 0.9); // Keys onto the right half of Kick: between Kick and Snare
  await expect.poll(slots).toEqual(["6", "9", "7", "8"]);
  await expect(page.locator("ga-channel-group ga-channel")).toHaveCount(3);
  await expect(page.locator("ga-mixer .drop")).toBeHidden();

  await drag(6, 8, 0.9); // Kick past Bass: out of the group
  await expect.poll(slots).toEqual(["9", "7", "8", "6"]);
  await expect(page.locator("ga-channel-group ga-channel")).toHaveCount(2);
  await expect.poll(async () => ((await (await fetch(`${server.url}/api/v1/workspace`)).json()) as { mixers: Record<string, { channels: { id: string }[] }> }).mixers["loopback-0"]?.channels.map((c) => c.id)).toEqual(["d", "b", "c", "a"]);
});

test("dragging a channel's fader coalesces and ends on the final level", async ({ page }) => {
  const frames = recordFrames(page);
  await layout({ "loopback-0": { channels: [{ id: "a", name: "", slot: 9, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }] } });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  const fader = page.getByTestId("fader-9");
  await expect(fader).toHaveAttribute("aria-disabled", "false");
  const box = await fader.boundingBox();
  if (box === null) throw new Error("fader-9 has no box");
  const x = box.x + box.width / 2;
  const moves = 60;
  await page.mouse.move(x, box.y + 2);
  await page.mouse.down();
  await page.mouse.move(x, box.y + box.height * 0.5, { steps: moves });
  await page.mouse.up();

  const readout = page.getByTestId("level-9");
  await expect(readout).not.toHaveText("0 dB");
  const finalLevel = Number((await readout.textContent())?.replace(/[^0-9]/g, ""));
  const sent = () => frames.filter((f) => f.command === "set_mixer" && f.args?.["channel"] === 10);
  await expect.poll(() => sent().at(-1)?.args?.["level"]).toBe(finalLevel);
  expect(sent().length).toBeLessThanOrEqual(moves + 1);
  test.info().annotations.push({ type: "coalescing", description: `${sent().length} set_mixer frames for ${moves} pointer moves` });
});

test("meters show the metered mix and links send set_stereo_link", async ({ page }) => {
  const frames = recordFrames(page);
  await layout({ "loopback-1": { channels: [{ id: "a", name: "", slot: 0, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }, { id: "b", name: "", slot: 1, source: { group: 0, channel: 1 }, main_mix: 1, sends: [] }] } });
  await page.goto(`${server.url}/#/mixer/loopback-1`);
  const metered = page.locator('ga-channel[data-channel-slot="0"] ga-strip .mask');
  const first = await metered.evaluate((el) => (el as HTMLElement).style.height);
  await expect.poll(() => metered.evaluate((el) => (el as HTMLElement).style.height)).not.toBe(first);
  await expect(page.locator('ga-channel[data-channel-slot="1"] ga-strip .mask')).toHaveAttribute("style", /height: 100%/);
  await expect.poll(() => frames.some((f) => f.command === "set_peak_source" && f.args?.["bank_id"] === 1 && f.args?.["source_id"] === 0)).toBe(true);

  await page.getByTestId("metered-mix").selectOption("1");
  await expect.poll(() => frames.some((f) => f.command === "set_peak_source" && f.args?.["bank_id"] === 1 && f.args?.["source_id"] === 1)).toBe(true);

  // Linking channels uses the same badges and bar as the Inputs page; slots 1 and 2 are a device pair.
  await page.getByTestId("mixer-link-0").click();
  await page.getByTestId("mixer-link-1").click();
  await page.getByTestId("link-save").click();
  await expect.poll(() => frames.find((f) => f.command === "set_stereo_link")?.args).toEqual({ periph_id: 4, channel_id: 0, linked: 1 });
  await expect(page.getByTestId("mixer-link-1")).toHaveAttribute("aria-pressed", "true");
  await page.getByTestId("mixer-link-0").click();
  await page.getByTestId("link-unlink").click();
  await expect(page.getByTestId("mixer-link-0")).toHaveAttribute("aria-pressed", "false");
});

test("removing takes a second click and mutes the channel; a new order is saved", async ({ page }) => {
  await layout({ "loopback-0": { channels: [{ id: "a", name: "Kick", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }, { id: "b", name: "Snare", slot: 7, sends: [] }, { id: "c", name: "Hat", slot: 8, sends: [] }] } });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  await expect(page.locator("ga-channel")).toHaveCount(3);

  await page.getByTestId("move-right-7").click();
  await expect.poll(() => page.locator("ga-channel").evaluateAll((els) => els.map((e) => e.getAttribute("data-channel-slot")))).toEqual(["6", "8", "7"]);

  await page.getByTestId("remove-6").click();
  await expect(page.locator("ga-channel")).toHaveCount(3, { timeout: 1000 });
  await page.getByTestId("remove-6").click();
  await expect(page.locator("ga-channel")).toHaveCount(2);
  await expect(lastSent(page)).toContainText(dryRun("set_routing", routingHex("quadro", 8, {})));

  await expect.poll(async () => ((await (await fetch(`${server.url}/api/v1/workspace`)).json()) as { mixers: Record<string, { channels: { id: string }[] }> }).mixers["loopback-0"]?.channels.map((c) => c.id)).toEqual(["c", "b"]);
  await page.reload();
  await expect.poll(() => page.locator("ga-channel").evaluateAll((els) => els.map((e) => e.getAttribute("data-channel-slot")))).toEqual(["8", "7"]);
});

test("a mixer with no channel set up offers starting layouts; applying one builds its channels and routes them", async ({ page }) => {
  const frames = recordFrames(page);
  await layout({});
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  await expect(page.locator("ga-channel")).toHaveCount(1);
  await page.getByTestId("profile-select").selectOption("tracking");
  await page.getByTestId("profile-apply").click();
  await expect(page.locator("ga-channel")).toHaveCount(6);
  await expect(page.getByTestId("name-10")).toHaveValue("DAW L");
  await expect(page.getByTestId("profile-select")).toHaveCount(0);
  await expect.poll(() => frames.filter((f) => f.command === "set_routing").length).toBeGreaterThan(0);
});
