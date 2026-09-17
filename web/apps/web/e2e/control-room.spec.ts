// The Control Room in dry run: trims and a read-only mono badge on the Outputs page and Studio+
// talkback there (decision P56), and the right zone's Control Room panel, which follows the device
// on the page: Monitor, HP1 and HP2, talkback on the Studio+, and mono for the device's selected
// mix. Expected bytes are the protocol crate's ground-truth vectors with the payload
// fields set: a one-byte payload header, then the fields from byte 17 (set_trim_config's two-byte
// header puts trim_id at 18).

import { expect, test, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { REPO_ROOT, startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { putWorkspace, resetWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--dry-run"], { webUi: true });
});

test.beforeEach(() => resetWorkspace(server));

test.afterAll(async () => {
  await server?.stop();
});

function sentText(file: "ground_truth.json" | "ground_truth_studio.json", command: string, fields: Record<number, number>): string {
  const vectors = JSON.parse(readFileSync(join(REPO_ROOT, "crates", "gazelle-audio-protocol", "tests", file), "utf8")) as Record<string, string>;
  const bytes = Uint8Array.from(vectors[command]?.match(/../g) ?? [], (pair) => Number.parseInt(pair, 16));
  for (const [at, value] of Object.entries(fields)) bytes[Number(at)] = value;
  return `Dry run, would send ${command}: ${[...bytes].map((b) => b.toString(16).padStart(2, "0")).join("")}`;
}

const lastSent = (page: Page) => page.getByTestId("last-sent");
const panel = (page: Page) => page.locator("ga-control-room");

test("Quadro trims are steps from 20 to 14 dBu sent with set_trim_config", async ({ page }) => {
  await page.goto(`${server.url}/#/outputs/loopback-0`);
  const trim = page.getByTestId("trim-1");
  await expect(trim.locator("option")).toHaveText(["20 dBu", "19 dBu", "18 dBu", "17 dBu", "16 dBu", "15 dBu", "14 dBu"]);
  await trim.selectOption("4");
  await expect(lastSent(page)).toContainText(sentText("ground_truth.json", "set_trim_config", { 18: 1, 19: 1, 20: 4 }));
  await expect(page.getByTestId("trim-2")).toHaveCount(0);
  await expect(page.getByTestId("talk")).toHaveCount(0, { timeout: 1000 });
});

test("Studio+ trims include the ADC, and talkback sends set_talk, set_tbk_enable and set_tbk_vol", async ({ page }) => {
  const studio = (command: string, fields: Record<number, number>) => sentText("ground_truth_studio.json", command, fields);
  await page.goto(`${server.url}/#/outputs/loopback-1`);
  await page.getByTestId("trim-2").selectOption("3");
  await expect(lastSent(page)).toContainText(studio("set_trim", { 17: 2, 18: 3 }));

  // Talk is momentary: on while held, off on release (the user's choice).
  const talk = page.getByTestId("talk");
  await talk.dispatchEvent("pointerdown", { button: 0, pointerId: 1 });
  await expect(lastSent(page)).toContainText(studio("set_talk", { 17: 1 }));
  await expect(talk).toHaveAttribute("aria-pressed", "true");
  await talk.dispatchEvent("pointerup", { button: 0, pointerId: 1 });
  await expect(lastSent(page)).toContainText(studio("set_talk", { 17: 0 }));
  await expect(talk).toHaveAttribute("aria-pressed", "false");
  await talk.focus();
  await talk.press("Space");
  await expect(lastSent(page)).toContainText(studio("set_talk", { 17: 0 }), { timeout: 1000 });

  await page.getByTestId("talk-to-1").click();
  await expect(lastSent(page)).toContainText(studio("set_tbk_enable", { 17: 1, 18: 1 }));
  // The talkback control is a level fader on the output-volume scale: 0 loudest, 96 = -inf.
  const volume = page.getByTestId("talk-volume");
  await volume.focus();
  await volume.press("End");
  await expect(lastSent(page)).toContainText(studio("set_tbk_vol", { 17: 0 }));
  await expect(volume).toHaveAttribute("aria-valuetext", "0 dB");
  await volume.press("Home");
  await expect(lastSent(page)).toContainText(studio("set_tbk_vol", { 17: 96 }));
  await expect(volume).toHaveAttribute("aria-valuetext", "-inf");
});

test("the right zone's monitor panel follows the page's device and shares state with the Outputs page", async ({ page }) => {
  await page.goto(`${server.url}/#/outputs/loopback-0`);
  await expect(panel(page)).toContainText("Zen Quadro");
  const volume = panel(page).getByTestId("cr-volume-0");
  await volume.focus();
  await volume.press("End");
  await expect(lastSent(page)).toContainText(sentText("ground_truth.json", "set_volume", { 17: 0, 18: 0 }));
  await panel(page).getByTestId("cr-dim-0").click();
  await expect(lastSent(page)).toContainText(sentText("ground_truth.json", "set_dim", { 17: 0, 18: 1 }));
  await page.getByTestId("out-mute-0").click();
  await expect(panel(page).getByTestId("cr-mute-0")).toHaveAttribute("aria-pressed", "true");
  await expect(panel(page).getByTestId("cr-talk")).toHaveCount(0);

  await page.goto(`${server.url}/#/inputs/loopback-1`);
  await expect(panel(page)).toContainText("Zen Studio+");
  await expect(panel(page).getByTestId("cr-dim-0")).toHaveCount(0);
  const crTalk = panel(page).getByTestId("cr-talk");
  await crTalk.dispatchEvent("pointerdown", { button: 0, pointerId: 1 });
  await expect(crTalk).toHaveAttribute("aria-pressed", "true");
  await crTalk.dispatchEvent("pointerleave", { pointerId: 1 });
  await expect(crTalk).toHaveAttribute("aria-pressed", "false", { timeout: 2000 });
});

test("the panel sets volume, mute and dim for Monitor, HP1 and HP2 on the Quadro, and shows no talkback", async ({ page }) => {
  const quadro = (command: string, fields: Record<number, number>) => sentText("ground_truth.json", command, fields);
  await page.goto(`${server.url}/#/outputs/loopback-0`);
  await expect(panel(page).getByRole("slider")).toHaveCount(3);
  for (const [id, name] of [[0, "Monitor"], [1, "HP1"], [2, "HP2"]] as const) {
    await expect(panel(page).getByTestId(`cr-output-${id}`)).toContainText(name);
    const volume = panel(page).getByTestId(`cr-volume-${id}`);
    await volume.focus();
    await volume.press("Home");
    await expect(lastSent(page)).toContainText(quadro("set_volume", { 17: id, 18: 96 }));
    await expect(volume).toHaveAttribute("aria-valuetext", "-inf");
    await expect(page.getByTestId(`out-volume-${id}`), "the Outputs page follows the panel").toHaveAttribute("aria-valuetext", "-inf");
    await volume.press("ArrowRight");
    await expect(lastSent(page)).toContainText(quadro("set_volume", { 17: id, 18: 95 }));

    await panel(page).getByTestId(`cr-mute-${id}`).click();
    await expect(lastSent(page)).toContainText(quadro("set_mute", { 17: id, 18: 1 }));
    await expect(page.getByTestId(`out-mute-${id}`)).toHaveAttribute("aria-pressed", "true");
    await panel(page).getByTestId(`cr-dim-${id}`).click();
    await expect(lastSent(page)).toContainText(quadro("set_dim", { 17: id, 18: 1 }));
    await expect(page.getByTestId(`out-dim-${id}`)).toHaveAttribute("aria-pressed", "true");
  }
  // Line out is not a Control Room output.
  await expect(panel(page).getByTestId("cr-volume-3")).toHaveCount(0);
  // The Quadro has no talkback commands, so nothing of it shows.
  await expect(panel(page).getByTestId("cr-talk")).toHaveCount(0);
  await expect(panel(page).getByTestId("cr-talk-volume")).toHaveCount(0);
  await expect(panel(page).locator('[data-testid^="cr-talk-to-"]')).toHaveCount(0);
  await expect(panel(page)).not.toContainText(/talk/i);
});

test("on the Studio+ the panel has Monitor, HP1 and HP2 without dim, and talkback's level and destinations", async ({ page }) => {
  const studio = (command: string, fields: Record<number, number>) => sentText("ground_truth_studio.json", command, fields);
  await page.goto(`${server.url}/#/outputs/loopback-1`);
  await expect(panel(page)).toContainText("Zen Studio+");
  for (const id of [0, 1, 2]) {
    const volume = panel(page).getByTestId(`cr-volume-${id}`);
    await volume.focus();
    await volume.press("End");
    await expect(lastSent(page)).toContainText(studio("set_volume", { 17: id, 18: 0 }));
    await panel(page).getByTestId(`cr-mute-${id}`).click();
    await expect(lastSent(page)).toContainText(studio("set_mute", { 17: id, 18: 1 }));
    await expect(panel(page).getByTestId(`cr-dim-${id}`)).toHaveCount(0);
  }

  const level = panel(page).getByTestId("cr-talk-volume");
  await level.focus();
  await level.press("End");
  await expect(lastSent(page)).toContainText(studio("set_tbk_vol", { 17: 0 }));
  await expect(level).toHaveAttribute("aria-valuetext", "0 dB");
  await level.press("Home");
  await expect(lastSent(page)).toContainText(studio("set_tbk_vol", { 17: 96 }));
  await expect(page.getByTestId("talk-volume"), "the Outputs page follows the panel").toHaveAttribute("aria-valuetext", "-inf");

  // Destinations in the device's order: HP1 0, HP2 1, Monitor 2.
  for (const [id, name] of [[0, "HP1"], [1, "HP2"], [2, "Monitor"]] as const) {
    const to = panel(page).getByTestId(`cr-talk-to-${id}`);
    await expect(to).toHaveText(name);
    await to.click();
    await expect(lastSent(page)).toContainText(studio("set_tbk_enable", { 17: id, 18: 1 }));
    await expect(to).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByTestId(`talk-to-${id}`)).toHaveAttribute("aria-pressed", "true");
  }
  await panel(page).getByTestId("cr-talk-to-1").click();
  await expect(lastSent(page)).toContainText(studio("set_tbk_enable", { 17: 1, 18: 0 }));

  const talk = panel(page).getByTestId("cr-talk");
  await talk.dispatchEvent("pointerdown", { button: 0, pointerId: 1 });
  await expect(lastSent(page)).toContainText(studio("set_talk", { 17: 1 }));
  await talk.dispatchEvent("pointerup", { button: 0, pointerId: 1 });
  await expect(lastSent(page)).toContainText(studio("set_talk", { 17: 0 }));
});

interface Frame {
  command?: string;
  args?: Record<string, number>;
}

/** Records every command frame the page sends over the WebSocket. */
function recordFrames(page: Page): Frame[] {
  const frames: Frame[] = [];
  page.on("websocket", (socket) => {
    socket.on("framesent", (event) => {
      if (typeof event.payload === "string") frames.push(JSON.parse(event.payload) as Frame);
    });
  });
  return frames;
}

const monoMixes = async (deviceId: string) =>
  (((await (await fetch(`${server.url}/api/v1/workspace`)).json()) as { mixers: Record<string, { mixes?: { mono?: unknown }[] }> }).mixers[deviceId]?.mixes ?? []).map((m) => m?.mono !== undefined);

test("the panel's mono switch acts on the device's selected mix and says which mix", async ({ page }) => {
  const frames = recordFrames(page);
  await putWorkspace(server, {
    mixers: { "loopback-1": { mixes: [{}, { name: "Cue" }], channels: [{ id: "a", name: "Vox", slot: 0, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }] } },
  });
  await page.goto(`${server.url}/#/inputs/loopback-1`);
  const mono = panel(page).getByTestId("cr-mono");
  await expect(panel(page).getByTestId("cr-mono-mix")).toHaveText("Mix 1");
  await expect(mono).toHaveAttribute("aria-pressed", "false");

  // Choose Mix 2 on the Mixer page, then leave it: the panel follows the choice.
  await page.goto(`${server.url}/#/mixer/loopback-1`);
  await page.getByTestId("mix-select").selectOption("1");
  await page.goto(`${server.url}/#/inputs/loopback-1`);
  await expect(panel(page).getByTestId("cr-mono-mix")).toHaveText("Mix 2: Cue");

  const panSent = () => frames.filter((f) => f.command === "set_mixer_cfg" && f.args?.["channel"] === 1).map((f) => [f.args?.["mixer_id"], f.args?.["pan"]]);
  await mono.click();
  await expect(mono).toHaveAttribute("aria-pressed", "true");
  await expect.poll(() => panSent().at(-1)).toEqual([1, 32]);
  await expect.poll(() => monoMixes("loopback-1")).toEqual([false, true]);

  await page.goto(`${server.url}/#/mixer/loopback-1`);
  await expect(page.getByTestId("mix-mono-1"), "the mix master shows the same mono").toHaveAttribute("aria-pressed", "true");
  await page.goto(`${server.url}/#/inputs/loopback-1`);
  await panel(page).getByTestId("cr-mono").click();
  await expect(panel(page).getByTestId("cr-mono")).toHaveAttribute("aria-pressed", "false");
  await expect.poll(() => monoMixes("loopback-1")).toEqual([false, false]);
  expect(panSent().every(([mix]) => mix === 1), "only the selected mix's pans are sent").toBe(true);
});

test("before the Mixer page has read the mixes, the panel's mono reads them before keeping the pans", async ({ page }) => {
  const frames = recordFrames(page);
  await putWorkspace(server, { mixers: { "loopback-0": { channels: [{ id: "a", name: "Vox", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }] } } });
  await page.goto(`${server.url}/#/inputs/loopback-0`);
  await expect(panel(page).getByTestId("cr-mono-mix")).toHaveText("Mix 1");
  await page.waitForTimeout(300);
  expect(frames.filter((f) => f.command === "get_mixer"), "the panel reads nothing on its own").toEqual([]);
  await panel(page).getByTestId("cr-mono").click();
  const centred = () => frames.findIndex((f) => f.command === "set_mixer" && f.args?.["channel"] === 7);
  await expect.poll(centred).toBeGreaterThan(-1);
  const read = frames.findIndex((f) => f.command === "get_mixer");
  expect(read, "the mixes are read").toBeGreaterThan(-1);
  expect(read, "before the pans are kept and centred").toBeLessThan(centred());
  expect(frames[centred()]?.args).toMatchObject({ mixer_id: 0, pan: 32 });
});
