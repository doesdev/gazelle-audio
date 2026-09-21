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
  server = await startServer(["--backend", "loopback", "--dry-run", "--loopback-cyclic-ms", "50"], { webUi: true });
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
/** Picks a mix from the button group in the bar, and waits for it to take. */
async function pickMix(page: Page, mix: number): Promise<void> {
  await page.getByTestId(`mix-${mix}`).click();
  await expect(page.getByTestId(`mix-${mix}`)).toHaveAttribute("aria-checked", "true");
}
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
  // The strip acts on the selected mix, so pick the channel's main mix to work it.
  await pickMix(page, 1);
  await expect(page.locator('ga-mix-master[mix="1"]')).toBeVisible();

  const fader = page.getByTestId("fader-6");
  await expect(fader).toHaveAttribute("aria-disabled", "false");
  await fader.focus();
  for (let i = 0; i < 3; i++) await fader.press("PageDown");
  await expect(page.getByTestId("level-6")).toHaveText("-18 dB");
  await expect(lastSent(page)).toContainText(dryRun("set_mixer", mixerHex("quadro", { mixer: 1, channel: 7, level: 18 })));
});

test("a channel with no name goes by its input's name until one is typed (the user, 2026-09-16)", async ({ page }) => {
  await layout({ "loopback-0": { channels: [{ id: "a", name: "", slot: 6, sends: [] }] } });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  const name = page.getByTestId("name-6");
  // No input yet: known by its mixer input.
  await expect(name).toHaveAttribute("placeholder", "Ch 7");
  await page.getByTestId("in-6").selectOption("0:1");
  const input = (await page.getByTestId("in-6").locator("option:checked").textContent())?.trim() ?? "";
  expect(input).not.toBe("");
  await expect(name).toHaveAttribute("placeholder", input);
  await expect(name).toHaveValue("");
  await expect(page.getByTestId("fader-6")).toHaveAttribute("aria-label", `${input} level`);

  await name.fill("Vox");
  await name.press("Enter");
  await expect(page.getByTestId("fader-6")).toHaveAttribute("aria-label", "Vox level");
});

test("a Studio+ strip's reverb send shows on Mix 1 only, as the vendor panel's does, and sends set_mixer_cfg's send byte", async ({ page }) => {
  await layout({
    "loopback-1": { channels: [{ id: "a", name: "Vox", slot: 0, source: { group: 0, channel: 0 }, main_mix: 0, sends: [1, 2, 3] }] },
    "loopback-0": { channels: [{ id: "q", name: "Kick", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }] },
  });
  await page.goto(`${server.url}/#/mixer/loopback-1`);
  const send = channelIn(page, 0).getByRole("slider", { name: "Vox send" });
  await expect(send).toBeVisible();
  await expect(channelIn(page, 0).locator("ga-strip").getByText("Send", { exact: true })).toBeVisible();
  await send.focus();
  await send.press("End");
  await expect(lastSent(page)).toContainText(dryRun("set_mixer_cfg", mixerHex("studio", { mixer: 0, channel: 1, level: 0, send: 0 })));
  await expect(send).toHaveAttribute("aria-valuetext", "0 dB");

  for (const mix of ["1", "2", "3"]) {
    await pickMix(page, Number(mix));
    await expect(channelIn(page, 0).getByTestId("fader-0")).toHaveAttribute("aria-disabled", "false");
    await expect(channelIn(page, 0).getByRole("slider", { name: "Vox send" }), `no send on Mix ${Number(mix) + 1}`).toHaveCount(0);
    await expect(channelIn(page, 0).locator("ga-strip").getByText("Send", { exact: true })).toHaveCount(0);
  }
  await pickMix(page, 0);
  await expect(channelIn(page, 0).getByRole("slider", { name: "Vox send" })).toHaveAttribute("aria-valuetext", "0 dB");

  await page.goto(`${server.url}/#/mixer/loopback-0`);
  await expect(channelIn(page, 6).getByTestId("fader-6")).toBeVisible();
  await expect(page.getByRole("slider", { name: /send$/ }), "the Quadro's strips have none").toHaveCount(0);
});

test("the Mix buttons pick the mix every strip controls; every channel shown, one outside it is dimmed and can be added", async ({ page }) => {
  await layout({ "loopback-1": { channels: [{ id: "a", name: "Vox", slot: 0, source: { group: 0, channel: 0 }, main_mix: 2, sends: [] }] } });
  await page.goto(`${server.url}/#/mixer/loopback-1`);
  const channel = channelIn(page, 0);
  const fader = page.getByTestId("fader-0");
  const inMix = page.getByTestId("in-mix-0");

  // Mix 1 is selected, and Vox does not feed it: with only this mix's channels shown, it is not here.
  await expect(page.getByTestId("mix-0")).toHaveAttribute("aria-checked", "true");
  await expect(channel).toBeHidden();
  await expect(page.getByTestId("empty-mix")).toContainText("Nothing is in Mix 1 yet");

  // Showing every channel brings it back, dimmed and out of this mix, with a way to add it.
  await page.getByTestId("show-all-channels").click();
  await expect(channel).toBeVisible();
  await expect(channel).toHaveAttribute("data-out-of-mix", "");
  await expect(fader).toHaveAttribute("aria-disabled", "true");
  await expect(inMix).toHaveText("Add to Mix 1");

  // In its main mix it is live and in the mix, and its fader acts on that mix.
  await pickMix(page, 2);
  await expect(channel).not.toHaveAttribute("data-out-of-mix", "");
  await expect(page.getByTestId("empty-mix")).toBeHidden();
  await expect(inMix).toHaveText("Main mix");
  await expect(inMix).toBeDisabled();
  await expect(fader).toHaveAttribute("aria-disabled", "false");
  await fader.focus();
  await fader.press("PageDown");
  await expect(lastSent(page)).toContainText(dryRun("set_mixer_cfg", mixerHex("studio", { mixer: 2, channel: 1, level: 6 })));
  // Only the selected mix's master shows.
  await expect(page.locator("ga-mix-master")).toHaveCount(1);
  await expect(page.locator('ga-mix-master[mix="2"]')).toBeVisible();

  // Adding it to Mix 2 routes it there, and then its fader acts on Mix 2 and leaves Mix 3 alone.
  await pickMix(page, 1);
  await expect(fader).toHaveAttribute("aria-disabled", "true");
  await inMix.click();
  // Studio+ MIX CH2 is destination 11.
  await expect(lastSent(page)).toContainText(dryRun("set_routing", routingHex("studio", 11, { 0: [0, 0] })));
  await expect(inMix).toHaveText("In Mix 2");
  await expect(inMix).toHaveAttribute("aria-pressed", "true");
  await expect(fader).toHaveAttribute("aria-disabled", "false");
  await fader.focus();
  // A fader's End is its bottom, -90 dB.
  await fader.press("End");
  await expect(lastSent(page)).toContainText(dryRun("set_mixer_cfg", mixerHex("studio", { mixer: 1, channel: 1, level: 90 })));
  await expect(page.getByTestId("level-0")).toHaveText("-90 dB");
  await pickMix(page, 2);
  await expect(page.getByTestId("level-0")).toHaveText("-6 dB");
});

test("the Mix buttons are a radio group: one is checked, arrow keys move between them and the wheel does not", async ({ page }) => {
  await layout({ "loopback-1": { mixes: [{ name: "Main" }, { name: "Cue" }], channels: [{ id: "a", name: "Vox", slot: 0, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }] } });
  await page.goto(`${server.url}/#/mixer/loopback-1`);
  const group = page.getByRole("radiogroup", { name: "Mix" });
  await expect(group.getByRole("radio")).toHaveCount(4);
  await expect(group.getByRole("radio", { name: "Main" })).toHaveAttribute("aria-checked", "true");
  await expect(group.getByRole("radio", { name: "Cue" })).toHaveAttribute("aria-checked", "false");
  // Only the chosen one is in the tab order, as a radio group is.
  expect(await group.getByRole("radio").evaluateAll((els) => els.map((e) => (e as HTMLElement).tabIndex))).toEqual([0, -1, -1, -1]);

  await page.getByTestId("mix-0").focus();
  await page.keyboard.press("ArrowRight");
  await expect(page.getByTestId("mix-1")).toHaveAttribute("aria-checked", "true");
  await expect(page.getByTestId("mix-1")).toBeFocused();
  await expect(page).toHaveURL(/#\/mixer\/loopback-1\/1$/);
  await page.keyboard.press("End");
  await expect(page.getByTestId("mix-3")).toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("ArrowRight");
  await expect(page.getByTestId("mix-0"), "it wraps round").toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("Home");
  await expect(page.getByTestId("mix-0")).toHaveAttribute("aria-checked", "true");

  // The wheel never changes the mix: a notch here would move every strip to another mix.
  await page.getByTestId("mix-0").hover();
  await page.mouse.wheel(0, 200);
  await page.waitForTimeout(200);
  await expect(page.getByTestId("mix-0")).toHaveAttribute("aria-checked", "true");
});

test("Show all channels is remembered per device across a reload, and the master always shows", async ({ page }) => {
  await layout({
    "loopback-1": {
      channels: [
        { id: "a", name: "Vox", slot: 0, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] },
        { id: "b", name: "Gtr", slot: 1, source: { group: 0, channel: 1 }, main_mix: 1, sends: [] },
      ],
    },
    "loopback-0": { channels: [{ id: "q", name: "Kick", slot: 6, source: { group: 0, channel: 0 }, main_mix: 1, sends: [] }] },
  });
  await page.goto(`${server.url}/#/mixer/loopback-1`);
  const shown = () => page.locator("ga-channel:visible").evaluateAll((els) => els.map((e) => e.getAttribute("data-channel-slot")));
  await expect.poll(shown).toEqual(["0"]);
  // The mix's master is beside the strips whatever the filter says.
  await expect(page.locator('ga-mix-master[mix="0"]')).toBeVisible();

  const toggle = page.getByTestId("show-all-channels");
  await expect(toggle).toHaveAttribute("aria-pressed", "false");
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-pressed", "true");
  await expect.poll(shown).toEqual(["0", "1"]);

  await page.reload();
  await expect(page.getByTestId("show-all-channels")).toHaveAttribute("aria-pressed", "true");
  await expect.poll(shown).toEqual(["0", "1"]);

  // Per device: the Quadro keeps its own choice, which is still the default.
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  await expect(page.getByTestId("show-all-channels")).toHaveAttribute("aria-pressed", "false");
});

test("a mix with nothing routed to it says so, and says how to put something there", async ({ page }) => {
  await layout({ "loopback-1": { mixes: [{ name: "Main" }, { name: "Cue" }], channels: [{ id: "a", name: "Vox", slot: 0, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }] } });
  await page.goto(`${server.url}/#/mixer/loopback-1`);
  const empty = page.getByTestId("empty-mix");
  await expect(empty).toBeHidden();

  await pickMix(page, 1);
  await expect(empty).toContainText("Nothing is in Cue yet");
  await expect(empty).toContainText("Show all channels");
  await expect(empty).toContainText('Add to Cue');

  // With every channel shown it no longer points at the toggle, and the channel is there to add.
  await page.getByTestId("show-all-channels").click();
  await expect(empty).toContainText("Nothing is in Cue yet");
  await expect(empty).not.toContainText("Show all channels");
  await page.getByTestId("in-mix-0").click();
  await expect(empty).toBeHidden();
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

test("the fader has an audio taper: clicking halfway down sets about -23 dB, and the scale is drawn to match", async ({ page }) => {
  await layout({ "loopback-0": { channels: [{ id: "a", name: "", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }] } });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  const fader = page.getByTestId("fader-6");
  await expect(fader).toHaveAttribute("aria-disabled", "false");
  const box = await fader.boundingBox();
  if (box === null) throw new Error("no fader");
  await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
  await expect(page.getByTestId("level-6")).toHaveText("-23 dB");
  // The cap sits where it was clicked, not where -23 dB would be on a linear scale (26%).
  const cap = page.locator('ga-channel[data-channel-slot="6"] ga-strip .cap');
  await expect.poll(async () => Number.parseFloat(await cap.evaluate((el) => (el as HTMLElement).style.getPropertyValue("--position")))).toBeGreaterThan(0.48);

  // The scale is drawn on the same taper, the meters' scale: marks crowd together towards the floor.
  const markAt = async (text: string) => {
    const mark = page.locator('ga-channel[data-channel-slot="6"] ga-strip .scale span', { hasText: new RegExp(`^${text}$`) });
    // Drawn along the cap's travel: calc(half a cap + (100% - a cap) * position).
    return 100 * Number.parseFloat((await mark.getAttribute("style"))?.match(/\* ([\d.]+)\)/)?.[1] ?? "-1");
  };
  expect(await markAt("-10")).toBeCloseTo(22.5, 1);
  expect(await markAt("-40")).toBeCloseTo(76.5, 1);
  expect(await markAt("-60")).toBeCloseTo(90, 1);
});

test("the fader cap's centre line sits on the scale mark for its level, and clicking a mark sets that level (the user, 2026-09-16)", async ({ page }) => {
  await layout({ "loopback-0": { channels: [{ id: "a", name: "", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }] } });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  const fader = page.getByTestId("fader-6");
  await expect(fader).toHaveAttribute("aria-disabled", "false");
  const strip = page.locator('ga-channel[data-channel-slot="6"] ga-strip');
  const centre = async (locator: ReturnType<typeof page.locator>) => {
    const box = await locator.boundingBox();
    if (box === null) throw new Error("not laid out");
    return box.y + box.height / 2;
  };
  const mark = (text: string) => strip.locator(".scale span", { hasText: new RegExp(`^${text}$`) });
  const cap = strip.locator(".cap");
  const level = page.getByTestId("level-6");

  // Set by keyboard, so the check does not depend on the pointer mapping.
  for (const [text, keys] of [["0", ["Home"]], ["-10", Array(10).fill("ArrowDown")], ["-30", Array(5).fill("PageDown")], ["-60", Array(10).fill("PageDown")], ["-90", ["End"]]] as const) {
    await fader.focus();
    await fader.press("Home");
    for (const key of keys) await fader.press(key);
    await expect(level).toHaveText(`${text === "0" ? "0" : text} dB`);
    await expect.poll(async () => Math.abs((await centre(cap)) - (await centre(mark(text)))), `cap on the ${text} mark`).toBeLessThanOrEqual(1.5);
  }

  // And the other way: a click level with a mark sets that mark's level.
  const box = await fader.boundingBox();
  if (box === null) throw new Error("no fader");
  for (const text of ["-5", "-20", "-40"]) {
    await page.mouse.click(box.x + box.width / 2, await centre(mark(text)));
    await expect(level).toHaveText(`${text} dB`);
  }
});

test("channel heads are one height, so faders line up whatever the input and whether or not the channel is set up", async ({ page }) => {
  await layout({ "loopback-0": { channels: [{ id: "a", name: "Vox", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [1] }, { id: "b", name: "DAW", slot: 7, source: { group: 1, channel: 0 }, main_mix: 1, sends: [] }, { id: "c", name: "", slot: 8, sends: [] }] } });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  await expect(page.locator("ga-channel")).toHaveCount(3);
  // Every channel on the row, so the one whose main mix is elsewhere is measured too.
  await page.getByTestId("show-all-channels").click();
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

/** A strip's name bar colour, as the browser computes it. */
const barColour = (page: Page, slot: number) => page.locator(`ga-channel[data-channel-slot="${slot}"] ga-strip .name`).evaluate((el) => getComputedStyle(el).backgroundColor);
const rgb = (hex: string) => `rgb(${[1, 3, 5].map((i) => Number.parseInt(hex.slice(i, i + 2), 16)).join(", ")})`;
const storedChannels = async () => ((await (await fetch(`${server.url}/api/v1/workspace`)).json()) as { mixers: Record<string, { channels: { id: string; color?: string }[] }> }).mixers["loopback-0"]?.channels ?? [];

test("a channel's colour defaults to its input's, is picked from the palette or set freely in a popover, and Clear hands it back (the user, 2026-09-16)", async ({ page }) => {
  // Quadro sources: PREAMP is green (#25a844) and USB 1 PLAY blue (#6ac4f8) on the Routing page.
  await layout({
    "loopback-0": {
      channels: [
        { id: "a", name: "Kick", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] },
        { id: "b", name: "Keys", slot: 7, source: { group: 1, channel: 0 }, main_mix: 0, sends: [] },
      ],
    },
  });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  await expect.poll(() => barColour(page, 6)).toBe(rgb("#25a844"));
  await expect.poll(() => barColour(page, 7)).toBe(rgb("#6ac4f8"));

  const swatch = page.getByTestId("colour-6");
  const popover = page.getByRole("dialog", { name: "Channel 7 colour" });
  await expect(popover).toBeHidden();
  await swatch.click();
  await expect(popover).toBeVisible();
  await expect(swatch).toHaveAttribute("aria-expanded", "true");
  await expect(popover.getByRole("button", { name: "Clear" })).toBeDisabled();

  // A palette swatch: the bar and the workspace take it, and the swatch shows as chosen.
  const first = popover.getByRole("button", { name: "Palette colour 1" });
  const paletteColour = (await first.getAttribute("title")) ?? "";
  expect(paletteColour).toMatch(/^#[0-9a-f]{6}$/i);
  await first.click();
  await expect.poll(() => barColour(page, 6)).toBe(rgb(paletteColour));
  await expect(first).toHaveAttribute("aria-pressed", "true");
  await expect.poll(async () => (await storedChannels()).find((c) => c.id === "a")?.color).toBe(paletteColour);
  await expect.poll(() => barColour(page, 7), "other channels keep their input's colour").toBe(rgb("#6ac4f8"));

  // Any colour, from the colour input.
  await popover.getByLabel("Custom colour").fill("#123456");
  await expect.poll(() => barColour(page, 6)).toBe(rgb("#123456"));
  await expect(first).toHaveAttribute("aria-pressed", "false");

  // Escape closes it and puts focus back on the swatch.
  await page.keyboard.press("Escape");
  await expect(popover).toBeHidden();
  await expect(swatch).toBeFocused();
  await expect(swatch).toHaveAttribute("aria-expanded", "false");

  // From the keyboard: Enter opens it on the palette, and Clear goes back to the input's colour.
  await swatch.press("Enter");
  await expect(popover).toBeVisible();
  await expect(first).toBeFocused();
  const clear = popover.getByRole("button", { name: "Clear" });
  await expect(clear).toBeEnabled();
  await clear.focus();
  await page.keyboard.press("Enter");
  await expect.poll(() => barColour(page, 6)).toBe(rgb("#25a844"));
  await expect.poll(async () => "color" in ((await storedChannels()).find((c) => c.id === "a") ?? {})).toBe(false);
  await expect(clear).toBeDisabled();

  // A click elsewhere closes it.
  await page.getByTestId("name-7").click();
  await expect(popover).toBeHidden();
});

test("a group colour wins over a channel's own, which the popover says is kept for when it leaves the group", async ({ page }) => {
  await layout({
    "loopback-0": {
      groups: [{ id: "drums", name: "Drums", collapsed: false, color: "#b5473a" }],
      channels: [{ id: "a", name: "Kick", group: "drums", color: "#123456", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }],
    },
  });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  await expect.poll(() => barColour(page, 6)).toBe(rgb("#b5473a"));

  await page.getByTestId("colour-6").click();
  const popover = page.getByRole("dialog", { name: "Channel 7 colour" });
  await expect(popover).toBeVisible();
  await expect(popover.getByTestId("colour-note-6")).toBeVisible();
  await expect(popover.getByTestId("colour-note-6")).toContainText("Drums");
  await expect(popover.getByLabel("Custom colour")).toHaveValue("#123456");
  // Choosing a colour in a coloured group saves it, and the group's colour still shows.
  await popover.getByLabel("Custom colour").fill("#654321");
  await expect.poll(async () => (await storedChannels()).find((c) => c.id === "a")?.color).toBe("#654321");
  await expect.poll(() => barColour(page, 6)).toBe(rgb("#b5473a"));
  await page.keyboard.press("Escape");

  await page.getByTestId("group-6").selectOption({ label: "No group" });
  await expect.poll(() => barColour(page, 6)).toBe(rgb("#654321"));
  await page.getByTestId("colour-6").click();
  await expect(popover.getByTestId("colour-note-6")).toBeHidden();
});

test("the colour swatch is disabled, and its popover closes, while the server is away", async ({ page }) => {
  const own = await startServer(["--backend", "loopback", "--dry-run"], { webUi: true });
  try {
    await putWorkspace(own, { mixers: { "loopback-0": { channels: [{ id: "a", name: "Kick", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }] } } });
    await page.goto(`${own.url}/#/mixer/loopback-0`);
    const swatch = page.getByTestId("colour-6");
    await swatch.click();
    await expect(page.getByRole("dialog", { name: "Channel 7 colour" })).toBeVisible();
    await own.stop();
    await expect(page.getByTestId("connection")).toHaveText("Reconnecting…");
    await expect(page.getByRole("dialog", { name: "Channel 7 colour" })).toBeHidden();
    await expect(swatch).toBeDisabled();
  } finally {
    await own.stop();
  }
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

test("a strip meters its channel's input; an input with no meter of its own shows none; links send set_stereo_link", async ({ page }) => {
  const frames = recordFrames(page);
  // Studio+: slot 0 on PREAMP 1 (group 0), slot 1 on USB PLAY 1 (group 3, which reports no meter).
  await layout({ "loopback-1": { channels: [{ id: "a", name: "", slot: 0, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }, { id: "b", name: "", slot: 1, source: { group: 3, channel: 0 }, main_mix: 0, sends: [] }] } });
  await page.goto(`${server.url}/#/mixer/loopback-1`);
  const preampMeter = page.locator('ga-channel[data-channel-slot="0"] ga-strip .mask');
  const first = await preampMeter.evaluate((el) => (el as HTMLElement).style.height);
  await expect.poll(() => preampMeter.evaluate((el) => (el as HTMLElement).style.height)).not.toBe(first);
  // Plasma style (the user, 2026-09-16): a peak marker holds the loudest recent signal above the bar.
  const peakMark = page.locator('ga-channel[data-channel-slot="0"] ga-strip .peak-mark');
  await expect(peakMark).toBeVisible();
  const barTop = async () => 100 - Number.parseFloat(await preampMeter.evaluate((el) => (el as HTMLElement).style.height));
  const markAt = async () => Number.parseFloat(await peakMark.evaluate((el) => (el as HTMLElement).style.bottom));
  await expect.poll(async () => (await markAt()) >= (await barTop()) - 0.001).toBe(true);
  const usbMeter = page.locator('ga-channel[data-channel-slot="1"] ga-strip');
  // USB PLAY has no peak field, so the strip asks the Studio+ to carry this mix in its mixer meter
  // bank. The loopback's report never names a mix there, so the meter stays honestly empty.
  await expect.poll(() => frames.filter((f) => f.command === "set_peak_source").map((f) => f.args)).toEqual([{ bank_id: 1, source_id: 0 }]);
  await expect(usbMeter.locator(".mask")).toHaveAttribute("style", /height: 100%/);
  await expect(usbMeter.locator('[data-testid="meter-1"]')).toHaveAttribute("title", /showing another mix/);
  // It is asked once, not on a timer.
  await page.waitForTimeout(500);
  expect(frames.filter((f) => f.command === "set_peak_source").length).toBe(1);

  // A channel not in the selected mix shows no meter either; in its mix it meters again.
  await pickMix(page, 1);
  await expect(preampMeter).toHaveAttribute("style", /height: 100%/);
  await pickMix(page, 0);
  await expect(page.getByTestId("in-mix-1")).toHaveText("Main mix");

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

test("a strip fed by the test oscillator, which no peak field meters, is metered at the mixer's own input", async ({ page }) => {
  const frames = recordFrames(page);
  // Quadro: slot 2 on OSCILLATOR (group 11), which has no peak field of its own. Its mixer meters
  // carry Mix 1 and cannot be moved off it, so this is where the level comes from.
  await layout({ "loopback-0": { channels: [{ id: "a", name: "", slot: 2, source: { group: 11, channel: 0 }, main_mix: 0, sends: [2] }] } });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  const meter = page.locator('ga-channel[data-channel-slot="2"] ga-strip .mask');
  const first = await meter.evaluate((el) => (el as HTMLElement).style.height);
  await expect.poll(() => meter.evaluate((el) => (el as HTMLElement).style.height)).not.toBe(first);
  await expect(page.getByTestId("meter-2")).toHaveAttribute("title", "The level arriving at this mixer channel, before the fader");
  // The Quadro ignores set_peak_source, so it is never asked (hardware, 2026-09-20).
  await page.waitForTimeout(300);
  expect(frames.filter((f) => f.command === "set_peak_source")).toEqual([]);

  // In another mix the same channel has nothing to meter, and says why.
  await pickMix(page, 2);
  await expect(page.getByTestId("meter-2")).toHaveAttribute("title", /meters its mixer channels for Mix 1 alone/);
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

test("a set-up mixer can be saved as a layout, and an empty mixer offers saved layouts beside the starting ones", async ({ page }) => {
  await layout({ "loopback-0": { mixes: [{ name: "Monitors" }], channels: [{ id: "a", name: "Vox", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }] } });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  await page.getByTestId("layout-save-name").fill("Vocal booth");
  await page.getByTestId("layout-save").click();
  await expect.poll(async () => ((await (await fetch(`${server.url}/api/v1/workspace`)).json()) as { layouts: { name: string; family: string }[] }).layouts.map((l) => [l.name, l.family])).toEqual([["Vocal booth", "quadro"]]);

  const saved = ((await (await fetch(`${server.url}/api/v1/workspace`)).json()) as { layouts: unknown[] }).layouts;
  await putWorkspace(server, { layouts: saved });
  // The same address again would not load a new document, so reload to read the reset workspace.
  await page.reload();
  const select = page.getByTestId("profile-select");
  await expect(select.locator("option")).toContainText(["Tracking", "Podcast", "Playback", "Vocal booth"]);
  const option = await select.locator("option", { hasText: "Vocal booth" }).getAttribute("value");
  await select.selectOption(option ?? "");
  await page.getByTestId("profile-apply").click();
  await expect(page.getByTestId("name-6")).toHaveValue("Vox");
});

test("Mono on a mix master centres its channels' pans, takes 6 dB off the master, keeps pan moves for later and restores both when turned off", async ({ page }) => {
  const frames = recordFrames(page);
  await layout({ "loopback-0": { channels: [{ id: "a", name: "Vox", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }] } });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  const pan = page.getByTestId("pan-6");
  // Centred, the label reads C and the centre ticks go; off centre they show.
  await expect(pan).toHaveAttribute("aria-valuetext", "C");
  await expect(pan).toHaveAttribute("data-centred", "");
  await pan.focus();
  await pan.press("Home");
  const panSent = () => frames.filter((f) => f.command === "set_mixer" && f.args?.["channel"] === 7).map((f) => f.args?.["pan"]);
  await expect.poll(() => panSent().at(-1)).toBe(2);
  await expect(pan).not.toHaveAttribute("data-centred");

  // The Quadro's master is channel 0: level is dB of attenuation, so mono adds 6 and gives it back.
  const masterSent = () => frames.filter((f) => f.command === "set_mixer" && f.args?.["channel"] === 0).map((f) => Number(f.args?.["level"]));
  const mono = page.getByTestId("mix-mono-0");
  await expect(mono).toHaveAttribute("title", /lowers the mix by 6 dB/);
  await mono.click();
  await expect(mono).toHaveAttribute("aria-pressed", "true");
  await expect.poll(() => panSent().at(-1)).toBe(32);
  await expect.poll(() => masterSent().length).toBe(1);
  const before = (masterSent()[0] as number) - 6;
  const count = panSent().length;
  await pan.focus();
  await pan.press("ArrowRight");
  await expect(pan).toHaveAttribute("aria-valuetext", "L 97%");
  expect(panSent().length).toBe(count);

  await mono.click();
  await expect(mono).toHaveAttribute("aria-pressed", "false");
  await expect.poll(() => panSent().at(-1)).toBe(3);
  await expect.poll(() => masterSent().at(-1), "the master gets mono's 6 dB back").toBe(before);
});

test("the wheel over a pan moves it a step at a time, up for right, and keeps the page still (the user, 2026-09-17)", async ({ page }) => {
  const frames = recordFrames(page);
  await layout({ "loopback-0": { channels: [{ id: "a", name: "Vox", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }] } });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  const pan = page.getByTestId("pan-6");
  await expect(pan).toHaveAttribute("aria-valuetext", "C");
  await page.evaluate(() => window.addEventListener("wheel", (event) => ((window as unknown as { wheelKept: boolean }).wheelKept = event.defaultPrevented)));
  await pan.hover();
  // One step (3%) a notch, through centre: the drag detent does not apply to the wheel.
  await page.mouse.wheel(0, -100);
  await expect(pan).toHaveAttribute("aria-valuetext", "R 3%");
  await page.mouse.wheel(0, 100);
  await expect(pan).toHaveAttribute("aria-valuetext", "C");
  await page.mouse.wheel(0, 100);
  await expect(pan).toHaveAttribute("aria-valuetext", "L 3%");
  await page.mouse.wheel(0, 100);
  await expect(pan).toHaveAttribute("aria-valuetext", "L 7%");
  expect(await page.evaluate(() => (window as unknown as { wheelKept?: boolean }).wheelKept), "the page did not scroll").toBe(true);
  const panSent = () => frames.filter((f) => f.command === "set_mixer" && f.args?.["channel"] === 7).map((f) => f.args?.["pan"]);
  await expect.poll(() => panSent().at(-1)).toBe(30);
  // The keys step one at a time the same way.
  await pan.focus();
  await pan.press("ArrowRight");
  await expect(pan).toHaveAttribute("aria-valuetext", "L 3%");
  await pan.press("ArrowRight");
  await expect(pan).toHaveAttribute("aria-valuetext", "C");
  await pan.press("ArrowRight");
  await expect(pan).toHaveAttribute("aria-valuetext", "R 3%");
  // A click near centre still lands on it, as the panel's knob does: 35 is inside the detent.
  const box = await pan.boundingBox();
  if (box === null) throw new Error("no pan");
  await page.mouse.click(box.x + box.width * ((35 - 2) / 60), box.y + box.height / 2);
  await expect(pan).toHaveAttribute("aria-valuetext", "C");
  await page.mouse.click(box.x + box.width * ((41.4 - 2) / 60), box.y + box.height / 2);
  await expect(pan).toHaveAttribute("aria-valuetext", "R 30%");
});

/** A frame the page sends over the WebSocket, as far as the chain stub reads it. */
interface WsFrame {
  id?: number;
  device_id?: string;
  command?: string;
  ext3?: number;
  args?: Record<string, number | string>;
}

/**
 * Answers the Quadro's chain and AFX IN routing reads as a device would, so a strip fed by AFX OUT
 * has something to meter in dry run: chain 1 holds one effect, chain 3 is empty, and AFX IN 3 takes
 * PREAMP 1. A chain written with `set_afx_order` is what the stub then reports, as a device does.
 */
async function answerChains(page: Page): Promise<void> {
  const chains: Record<number, [number, number][]> = { 0: [[39, 2]] };
  const free = { 39: 14, 1: 15, 3: 4 };
  const MUTE = 10;
  const afxIn = (slot: number) => (slot === 2 ? { in_periph_id: 0, in_chann: 0 } : { in_periph_id: MUTE, in_chann: 0 });
  const replies: Record<string, (frame: WsFrame) => unknown> = {
    get_afx_strip_order: (frame) => ({ entries: [{ slots: Array.from({ length: 8 }, (_, i) => ({ type: chains[frame.ext3 ?? -1]?.[i]?.[0] ?? 0, inst: chains[frame.ext3 ?? -1]?.[i]?.[1] ?? 0 })) }] }),
    get_afx_links: () => ({ entries: Array.from({ length: 7 }, () => ({ linked: 0 })) }),
    get_afx_available_instances: () => ({ entries: Object.entries(free).map(([type_id, inst_count]) => ({ type_id: Number(type_id), inst_count })) }),
    get_afx_remaining_featured_instances: () => ({ entries: Object.entries(free).map(([type_id]) => ({ type_id: Number(type_id), inst_count: 16 })) }),
  };
  await page.routeWebSocket(/\/ws$/, (socket) => {
    const upstream = socket.connectToServer();
    socket.onMessage((message) => {
      const frame: WsFrame = typeof message === "string" ? (JSON.parse(message) as WsFrame) : {};
      if (frame.command === "set_afx_order" && typeof frame.args?.["slots"] === "string") {
        const hex = frame.args["slots"];
        const written: [number, number][] = [];
        for (let i = 0; i + 3 < hex.length; i += 4) {
          const type = Number.parseInt(hex.slice(i, i + 2), 16);
          if (type !== 0) written.push([type, Number.parseInt(hex.slice(i + 2, i + 4), 16)]);
        }
        chains[Number(frame.args["ch_id"])] = written;
      }
      // AFX IN is the Quadro's destination group 7; every other read goes to the server as usual.
      const reply = frame.device_id === "loopback-0" ? (frame.command === "get_routing" && frame.ext3 === 7 ? () => ({ bank_configs: Array.from({ length: 32 }, (_, i) => afxIn(i)) }) : replies[frame.command ?? ""]) : undefined;
      if (reply === undefined) return upstream.send(message);
      socket.send(JSON.stringify({ type: "rpc_response", id: frame.id, result: { device_id: frame.device_id, command: frame.command, sent_hex: "74", sent_len: 16, dry_run: false, response: reply(frame), response_error: null } }));
    });
  });
}

test("a strip on an empty AFX OUT chain meters the source routed into the chain, and follows the chain as it is edited", async ({ page }) => {
  await answerChains(page);
  // Slot 6 on AFX OUT 3 (an empty chain fed by PREAMP 1), slot 7 on AFX OUT 1 (a chain with an effect).
  await layout({
    "loopback-0": {
      channels: [
        { id: "a", name: "Through", slot: 6, source: { group: 5, channel: 2 }, main_mix: 0, sends: [] },
        { id: "b", name: "Chain", slot: 7, source: { group: 5, channel: 0 }, main_mix: 0, sends: [] },
      ],
    },
  });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  const through = page.getByTestId("meter-6");
  const loaded = page.getByTestId("meter-7");
  // An empty chain passes its input through, so the strip meters what feeds AFX IN 3 and says so.
  await expect(through).toHaveAttribute("title", "Through an empty chain from PREAMP 1");
  await expect(loaded).toHaveAttribute("title", /last effect/);
  const bar = channelIn(page, 6).locator("ga-strip .mask");
  const first = await bar.evaluate((el) => (el as HTMLElement).style.height);
  await expect.poll(() => bar.evaluate((el) => (el as HTMLElement).style.height)).not.toBe(first);

  // Inserting the first effect into the chain hands the meter to it; removing it gives it back.
  await page.locator('ga-header a[data-page="effects"]').click();
  await page.getByTestId("chain-add-2").selectOption("39");
  await expect(page.getByTestId("slot-2-0")).toContainText("PowerGate");
  await page.locator('ga-header a[data-page="mixer"]').click();
  await expect(page.getByTestId("meter-6")).toHaveAttribute("title", /last effect/);

  await page.locator('ga-header a[data-page="effects"]').click();
  await page.getByTestId("remove-2-0").click();
  await expect(page.getByTestId("chain-2")).toContainText("No effects");
  await page.locator('ga-header a[data-page="mixer"]').click();
  await expect(page.getByTestId("meter-6")).toHaveAttribute("title", "Through an empty chain from PREAMP 1");
});

test("a mix that carries one signal twice says so on both strips, and stops when one is muted", async ({ page }) => {
  await answerChains(page);
  // PREAMP 1 on slot 5, the empty chain fed by it on slot 6, and a chain with an effect on slot 7.
  await layout({
    "loopback-0": {
      channels: [
        { id: "p", name: "Vox", slot: 5, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] },
        { id: "a", name: "Through", slot: 6, source: { group: 5, channel: 2 }, main_mix: 0, sends: [] },
        { id: "b", name: "Chain", slot: 7, source: { group: 5, channel: 0 }, main_mix: 0, sends: [] },
      ],
    },
  });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  const doubled = "PREAMP 1 also reaches this mix through AFX OUT 3, so it is summed twice (about +6 dB)";
  await expect(page.getByTestId("doubled-5")).toHaveAttribute("title", doubled);
  await expect(page.getByTestId("doubled-6")).toHaveAttribute("title", doubled);
  // A chain with an effect in it is a parallel setup, not a doubling.
  await expect(page.getByTestId("doubled-7")).toBeHidden();

  await page.getByLabel("Through mute").click();
  await expect(page.getByTestId("doubled-5")).toBeHidden();
  await expect(page.getByTestId("doubled-6")).toBeHidden();
});

test("two channels on one input in one mix both carry the x2 badge, on the page and in the dock", async ({ page }) => {
  await layout({
    "loopback-0": {
      channels: [
        { id: "a", name: "Vox A", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] },
        { id: "b", name: "Vox B", slot: 7, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] },
        { id: "c", name: "Kick", slot: 8, source: { group: 0, channel: 1 }, main_mix: 0, sends: [] },
      ],
    },
  });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  const doubled = "PREAMP 1 is in this mix twice, so it is summed twice (about +6 dB)";
  await expect(page.getByTestId("doubled-6")).toHaveAttribute("title", doubled);
  await expect(page.getByTestId("doubled-7")).toHaveAttribute("title", doubled);
  await expect(page.getByTestId("doubled-8"), "the only channel on PREAMP 2").toBeHidden();

  // The dock rides the same mix from another page, so it carries the badge too.
  await page.locator('ga-header nav a[data-page="inputs"]').click();
  await expect(page.locator("ga-mixer-dock ga-strip").first()).toBeVisible();
  await expect(page.locator('ga-mixer-dock [data-testid="doubled-6"]')).toHaveAttribute("title", doubled);
});

test("putting an input into a mix that already has it asks first, and a second press does it anyway", async ({ page }) => {
  await layout({
    "loopback-0": {
      mixes: [{ name: "Monitors" }, { name: "Cue" }],
      channels: [
        { id: "a", name: "Vox", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] },
        { id: "b", name: "Spare", slot: 7, sends: [] },
      ],
    },
  });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  const doubled = "PREAMP 1 is in this mix twice, so it is summed twice (about +6 dB)";

  // Its main mix first: Mix 2 has nothing in it, so that goes straight through.
  await page.getByTestId("out-7").selectOption("1");
  await expect(page.getByTestId("out-confirm-7")).toBeHidden();

  // Then its input, which Monitors already has on another channel: the menu waits behind Confirm.
  await page.getByTestId("show-all-channels").click();
  await page.getByTestId("in-7").selectOption("0:0");
  await expect(page.getByTestId("in-confirm-7")).toBeHidden();
  await page.getByTestId("in-mix-7").click();
  const confirm = page.getByTestId("in-mix-7");
  await expect(confirm).toHaveText("Confirm");
  await expect(page.getByTestId("in-mix-7")).toHaveAttribute("data-armed", "");
  // Nothing has been routed into Monitors yet.
  await expect(lastSent(page)).not.toContainText(routingHex("quadro", 8, { 7: [0, 0] }));
  await confirm.click();
  await expect(lastSent(page)).toContainText(dryRun("set_routing", routingHex("quadro", 8, { 7: [0, 0] })));
  await expect(page.getByTestId("doubled-6")).toHaveAttribute("title", doubled);
  await expect(page.getByTestId("doubled-7")).toHaveAttribute("title", doubled);
});

test("choosing a main mix or an input that would double one source waits behind a Confirm beside the menu", async ({ page }) => {
  await layout({
    "loopback-0": {
      channels: [
        { id: "a", name: "Vox", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] },
        { id: "b", name: "Spare", slot: 7, source: { group: 0, channel: 0 }, sends: [] },
        { id: "c", name: "Kick", slot: 8, source: { group: 0, channel: 1 }, main_mix: 1, sends: [] },
      ],
    },
  });
  await page.goto(`${server.url}/#/mixer/loopback-0`);
  await page.getByTestId("show-all-channels").click();

  // Spare carries PREAMP 1, which Mix 1 already has: choosing Mix 1 as its main mix asks first.
  const mixConfirm = page.getByTestId("out-confirm-7");
  await expect(mixConfirm).toBeHidden();
  await page.getByTestId("out-7").selectOption("0");
  await expect(mixConfirm).toBeVisible();
  await expect(mixConfirm).toHaveAttribute("title", "PREAMP 1 is in this mix twice, so it is summed twice (about +6 dB)");
  await mixConfirm.click();
  await expect(mixConfirm).toBeHidden();
  await expect(lastSent(page)).toContainText(dryRun("set_routing", routingHex("quadro", 8, { 7: [0, 0] })));

  // And the other way about: giving Kick the input its mix already has asks before routing it.
  const inputConfirm = page.getByTestId("in-confirm-8");
  await page.getByTestId("out-8").selectOption("0");
  await expect(page.getByTestId("out-confirm-8"), "PREAMP 2 is in no mix, so nothing to ask").toBeHidden();
  await page.getByTestId("in-8").selectOption("0:0");
  await expect(inputConfirm).toBeVisible();
  await expect(inputConfirm).toHaveAttribute("title", /PREAMP 1 is in this mix twice/);
  await inputConfirm.click();
  await expect(inputConfirm).toBeHidden();
  await expect(page.getByTestId("doubled-8")).toBeVisible();
});
