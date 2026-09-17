// The Control Room in dry run: trims and a read-only mono badge on the Outputs page and Studio+
// talkback there (decision P56), and the right zone's Control Room panel, which follows the device
// on the page: the outputs chosen on the Outputs page, talkback on the Studio+, and mono for the mix
// that feeds each output. Expected bytes are the protocol crate's ground-truth vectors with the payload
// fields set: a one-byte payload header, then the fields from byte 17 (set_trim_config's two-byte
// header puts trim_id at 18).

import { expect, test, type Page, type WebSocketRoute } from "@playwright/test";
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
  // Line out is not a Control Room output until chosen.
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

const controlRoomSaved = async (): Promise<unknown> => ((await (await fetch(`${server.url}/api/v1/workspace`)).json()) as { control_room?: unknown }).control_room;

test("the Outputs page chooses which outputs the Quadro's Control Room shows, saved in the workspace, without sending anything", async ({ page }) => {
  const quadro = (command: string, fields: Record<number, number>) => sentText("ground_truth.json", command, fields);
  const frames = recordFrames(page);
  await page.goto(`${server.url}/#/outputs/loopback-0`);
  const shown = () => panel(page).locator('[data-testid^="cr-output-"]').evaluateAll((groups) => groups.map((g) => (g as HTMLElement).dataset["testid"]));
  // Monitor, HP1 and HP2 until chosen.
  for (const [id, on] of [[0, true], [1, true], [2, true], [3, false]] as const) {
    await expect(page.getByTestId(`out-in-cr-${id}`)).toBeChecked({ checked: on });
  }
  await expect.poll(shown).toEqual(["cr-output-0", "cr-output-1", "cr-output-2"]);
  await expect(page.getByTestId("output-3").getByLabel("Line out in the Control Room")).not.toBeChecked();
  const before = frames.length;

  // Line out joins, after HP2 in the device's order, with its volume, mute and dim.
  await page.getByTestId("out-in-cr-3").check();
  await expect.poll(shown).toEqual(["cr-output-0", "cr-output-1", "cr-output-2", "cr-output-3"]);
  await expect(panel(page).getByTestId("cr-output-3")).toContainText("Line out");
  await expect.poll(controlRoomSaved).toEqual({ "loopback-0": { outputs: [0, 1, 2, 3] } });
  await page.getByTestId("out-in-cr-1").uncheck();
  await expect.poll(shown).toEqual(["cr-output-0", "cr-output-2", "cr-output-3"]);
  await expect.poll(controlRoomSaved).toEqual({ "loopback-0": { outputs: [0, 2, 3] } });
  await page.waitForTimeout(300);
  expect(frames.slice(before).filter((f) => f.command !== undefined), "choosing sends nothing to the device").toEqual([]);

  const volume = panel(page).getByTestId("cr-volume-3");
  await volume.focus();
  await volume.press("Home");
  await expect(lastSent(page)).toContainText(quadro("set_volume", { 17: 3, 18: 96 }));
  await expect(page.getByTestId("out-volume-3"), "the Outputs page follows the panel").toHaveAttribute("aria-valuetext", "-inf");
  await panel(page).getByTestId("cr-mute-3").click();
  await expect(lastSent(page)).toContainText(quadro("set_mute", { 17: 3, 18: 1 }));
  await panel(page).getByTestId("cr-dim-3").click();
  await expect(lastSent(page)).toContainText(quadro("set_dim", { 17: 3, 18: 1 }));

  // Kept across a reload, and per device: the Studio+ still has the default.
  await page.reload();
  await expect.poll(shown).toEqual(["cr-output-0", "cr-output-2", "cr-output-3"]);
  await expect(page.getByTestId("out-in-cr-1")).not.toBeChecked();
  await page.goto(`${server.url}/#/outputs/loopback-1`);
  await expect(panel(page)).toContainText("Zen Studio+");
  await expect.poll(shown).toEqual(["cr-output-0", "cr-output-1", "cr-output-2"]);
});

test("the Studio+ Control Room can show Reamp and Line out, without dim, and every output can be left out", async ({ page }) => {
  const studio = (command: string, fields: Record<number, number>) => sentText("ground_truth_studio.json", command, fields);
  await putWorkspace(server, { control_room: { "loopback-1": { outputs: [4, 0] } } });
  await page.goto(`${server.url}/#/outputs/loopback-1`);
  const shown = () => panel(page).locator('[data-testid^="cr-output-"]').evaluateAll((groups) => groups.map((g) => (g as HTMLElement).dataset["testid"]));
  await expect.poll(shown, "in the device's order, not the stored one").toEqual(["cr-output-0", "cr-output-4"]);
  await expect(page.getByTestId("out-in-cr-4")).toBeChecked();
  await expect(panel(page).getByTestId("cr-output-4")).toContainText("Reamp");
  await expect(panel(page).getByTestId("cr-dim-4")).toHaveCount(0);
  const volume = panel(page).getByTestId("cr-volume-4");
  await volume.focus();
  await volume.press("End");
  await expect(lastSent(page)).toContainText(studio("set_volume", { 17: 4, 18: 0 }));
  await panel(page).getByTestId("cr-mute-4").click();
  await expect(lastSent(page)).toContainText(studio("set_mute", { 17: 4, 18: 1 }));

  await page.getByTestId("out-in-cr-3").check();
  await expect.poll(shown).toEqual(["cr-output-0", "cr-output-3", "cr-output-4"]);
  for (const id of [0, 3, 4]) await page.getByTestId(`out-in-cr-${id}`).uncheck();
  await expect.poll(shown).toEqual([]);
  await expect.poll(controlRoomSaved).toEqual({ "loopback-1": { outputs: [] } });
  // Talkback is the Studio+'s whatever outputs are shown.
  await expect(panel(page).getByTestId("cr-talk")).toBeVisible();
});

test("a surface's output strip has no Control Room choice", async ({ page }) => {
  await putWorkspace(server, { surfaces: [{ id: "s", name: "Outs", mixes: {}, strips: [{ id: "o", kind: "output", device_id: "loopback-0", output: 3 }] }] });
  await page.goto(`${server.url}/#/surface/s`);
  await expect(page.locator("ga-surface").getByTestId("output-3")).toBeVisible();
  await expect(page.locator("ga-surface").getByTestId("out-in-cr-3")).toHaveCount(0);
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


type Slot = [number, number];
type RoutedFrame = Frame & { id?: number; device_id?: string; ext3?: number };

/**
 * Answers `get_routing` for the groups given (device → destination → slots, MUTE elsewhere) as a
 * device would; the dry-run server answers no reads. Every frame the page sends is recorded, in order.
 */
async function answerRouting(page: Page, groups: Record<string, Record<number, Record<number, Slot>>>): Promise<RoutedFrame[] & { drop(): Promise<void> }> {
  const sockets: WebSocketRoute[] = [];
  const frames = Object.assign([] as RoutedFrame[], { drop: async () => sockets.at(-1)?.close() });
  await page.routeWebSocket(/\/ws$/, (socket) => {
    sockets.push(socket);
    const upstream = socket.connectToServer();
    socket.onMessage((message) => {
      const frame: RoutedFrame = typeof message === "string" ? (JSON.parse(message) as RoutedFrame) : {};
      frames.push(frame);
      const routed = frame.command === "get_routing" ? groups[frame.device_id ?? ""]?.[frame.ext3 ?? -1] : undefined;
      if (routed === undefined) return upstream.send(message);
      const mute = frame.device_id === "loopback-0" ? 10 : 11;
      const bank_configs = Array.from({ length: 32 }, (_, slot) => ({ in_periph_id: routed[slot]?.[0] ?? mute, in_chann: routed[slot]?.[1] ?? 0 }));
      socket.send(JSON.stringify({ type: "rpc_response", id: frame.id, result: { device_id: frame.device_id, command: "get_routing", sent_hex: "74", sent_len: 16, dry_run: false, response: { bank_idx: frame.ext3, bank_configs }, response_error: null } }));
    });
  });
  return frames;
}

// Studio+ sources: USB PLAY 3, MIX1 L/R 7, MIX2 L/R 8. Destinations: LINE OUT 0, HP1 1, HP2 2, MONITOR 3, REAMP 4, USB REC 6.
// Mix 1 plays in Monitor, HP1 and USB REC 1/2; Mix 2 in HP2 and Reamp (not shown in the panel); Line out plays USB PLAY 1/2.
const MIX1: Record<number, Slot> = { 0: [7, 0], 1: [7, 1] };
const STUDIO_ROUTING = { "loopback-1": { 3: MIX1, 1: MIX1, 6: { 0: [7, 0], 1: [7, 1] } as Record<number, Slot>, 2: { 0: [8, 0], 1: [8, 1] } as Record<number, Slot>, 0: { 0: [3, 0], 1: [3, 1] } as Record<number, Slot>, 4: { 0: [8, 0], 1: [8, 1] } as Record<number, Slot> } };

test("each Control Room output fed by a mix has a Mono button for that mix, naming the outputs that share it; an output no mix feeds has it disabled with the reason", async ({ page }) => {
  const frames = await answerRouting(page, STUDIO_ROUTING);
  await putWorkspace(server, {
    mixers: { "loopback-1": { mixes: [{}, { name: "Cue" }], channels: [{ id: "a", name: "Vox", slot: 0, source: { group: 0, channel: 0 }, main_mix: 0, sends: [1] }] } },
    control_room: { "loopback-1": { outputs: [0, 1, 2, 3] } },
  });
  // The mixer dock reads the mixes when it is open; collapsed, it reads nothing, leaving the panel on its own.
  await page.addInitScript(() => localStorage.setItem("gazelle.layout.mixerDock", "true"));
  await page.goto(`${server.url}/#/inputs/loopback-1`);
  await expect(panel(page).getByTestId("cr-mono-mix"), "the selected-mix switch is gone").toHaveCount(0);
  await expect(panel(page).getByTestId("cr-mono")).toHaveCount(0);
  const mono = (id: number) => panel(page).getByTestId(`cr-mono-${id}`);
  for (const id of [0, 1, 2, 3]) {
    await expect(mono(id), "beside Mute").toHaveText("Mono");
    await expect(mono(id), "until the routing is read, which mix is not known").toBeEnabled();
    await expect(mono(id)).toHaveAttribute("aria-pressed", "false");
  }
  await expect(mono(0)).toHaveAttribute("title", /Reads the routing first/);
  await page.waitForTimeout(300);
  expect(frames.filter((f) => f.command === "get_routing" || f.command === "get_mixer"), "the panel reads nothing on its own").toEqual([]);

  // Monitor plays Mix 1: its Mono reads the routing and the mixes, then centres Mix 1's pans.
  await mono(0).click();
  await expect(mono(0)).toHaveAttribute("aria-pressed", "true");
  const panSent = () => frames.filter((f) => f.command === "set_mixer_cfg" && f.args?.["channel"] === 1).map((f) => [f.args?.["mixer_id"], f.args?.["pan"]]);
  await expect.poll(panSent).toEqual([[0, 32]]);
  const routingRead = frames.findIndex((f) => f.command === "get_routing" && f.ext3 === 3);
  const mixesRead = frames.findIndex((f) => f.command === "get_mixer");
  const centred = frames.findIndex((f) => f.command === "set_mixer_cfg");
  expect(routingRead, "Monitor's routing is read").toBeGreaterThan(-1);
  expect(mixesRead, "the mixes are read").toBeGreaterThan(-1);
  expect(Math.max(routingRead, mixesRead), "both before the pans are kept and centred").toBeLessThan(centred);
  await expect.poll(() => monoMixes("loopback-1")).toEqual([true, false]);

  // HP1 plays the same mix, so it is mono too, and each names the other; USB REC 1/2 plays it as well.
  await expect(mono(1)).toHaveAttribute("aria-pressed", "true");
  await expect(mono(0)).toHaveAttribute("title", "Sums Mix 1 to mono, so HP1 and USB REC 1/2 go mono too: pans its channels to centre, and restores them when turned off.");
  await expect(mono(1)).toHaveAttribute("title", "Sums Mix 1 to mono, so Monitor and USB REC 1/2 go mono too: pans its channels to centre, and restores them when turned off.");
  await expect(mono(0)).toHaveAttribute("aria-label", "Monitor mono (Mix 1, also HP1 and USB REC 1/2)");
  await expect(panel(page).getByTestId("cr-feed-0")).toHaveText("Mix 1");
  // HP2 plays the named Mix 2, which is not mono; so does Reamp, which the panel does not show.
  await expect(mono(2)).toBeEnabled();
  await expect(mono(2)).toHaveAttribute("aria-pressed", "false");
  await expect(mono(2)).toHaveAttribute("title", "Sums Mix 2: Cue to mono, so Reamp goes mono too: pans its channels to centre, and restores them when turned off.");
  await expect(panel(page).getByTestId("cr-feed-2")).toHaveText("Mix 2: Cue");
  // Line out plays USB straight: no mix to sum.
  await expect(mono(3)).toBeDisabled();
  await expect(mono(3)).toHaveAttribute("title", "No mix feeds Line out: it plays USB PLAY 1 and USB PLAY 2, so there is no mix to sum to mono.");
  await expect(panel(page).getByTestId("cr-feed-3")).toHaveText("USB PLAY 1 and USB PLAY 2");

  // The mix master agrees.
  await page.goto(`${server.url}/#/mixer/loopback-1`);
  await expect(page.getByTestId("mix-mono-0"), "the mix master shows the same mono").toHaveAttribute("aria-pressed", "true");
  await expect(panel(page).getByTestId("cr-mono-1"), "the routing read is kept").toHaveAttribute("aria-pressed", "true");

  // HP1's Mono ends Mix 1's mono for Monitor too, restoring the pan it kept, and touches no other mix.
  await mono(1).click();
  await expect(mono(0)).toHaveAttribute("aria-pressed", "false");
  await expect(page.getByTestId("mix-mono-0")).toHaveAttribute("aria-pressed", "false");
  await expect.poll(() => monoMixes("loopback-1")).toEqual([false, false]);
  await expect.poll(() => panSent().length).toBe(2);
  expect(panSent().every(([mix]) => mix === 0), "only Mix 1's pans are sent").toBe(true);

  // Coming back from a dropped connection enables the controls again, but not a Mono with no mix to sum.
  await frames.drop();
  await expect(panel(page).getByTestId("cr-mute-0")).toBeDisabled();
  await expect(mono(0)).toBeDisabled();
  await expect(page.getByTestId("connection")).toHaveText("Connected", { timeout: 15_000 });
  await expect(panel(page).getByTestId("cr-mute-0")).toBeEnabled();
  await expect(mono(0)).toBeEnabled();
  await expect(mono(3)).toBeDisabled();
});

test("on the Quadro, Mono reads the routing and the mixes before keeping the pans; an output whose routing gets no reply says so", async ({ page }) => {
  // Quadro: MONITOR is destination 3, fed by mix 1 (LOOPBACK HP1, source 6). HP1 (1) is not answered.
  const frames = await answerRouting(page, { "loopback-0": { 3: { 0: [6, 0], 1: [6, 1] }, 0: {}, 2: {} } });
  await putWorkspace(server, { mixers: { "loopback-0": { channels: [{ id: "a", name: "Vox", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }] } } });
  await page.addInitScript(() => localStorage.setItem("gazelle.layout.mixerDock", "true"));
  await page.goto(`${server.url}/#/inputs/loopback-0`);
  const mono = (id: number) => panel(page).getByTestId(`cr-mono-${id}`);
  await expect(mono(0)).toBeEnabled();
  await page.waitForTimeout(300);
  expect(frames.filter((f) => f.command === "get_routing" || f.command === "get_mixer"), "the panel reads nothing on its own").toEqual([]);

  await mono(0).click();
  const centred = () => frames.findIndex((f) => f.command === "set_mixer" && f.args?.["channel"] === 7);
  await expect.poll(centred).toBeGreaterThan(-1);
  const routingRead = frames.findIndex((f) => f.command === "get_routing" && f.ext3 === 3);
  const mixesRead = frames.findIndex((f) => f.command === "get_mixer");
  expect(routingRead, "the routing is read").toBeGreaterThan(-1);
  expect(routingRead, "before the mixes").toBeLessThan(mixesRead);
  expect(mixesRead, "and both before the pans are kept and centred").toBeLessThan(centred());
  expect(frames[centred()]?.args).toMatchObject({ mixer_id: 0, pan: 32 });
  await expect(mono(0)).toHaveAttribute("aria-pressed", "true");
  await expect(mono(0)).toHaveAttribute("title", "Sums Mix 1 to mono: pans its channels to centre, and restores them when turned off.");

  await expect(mono(1)).toBeDisabled();
  await expect(mono(1)).toHaveAttribute("title", "The routing to HP1 could not be read, so the mix that feeds it is not known.");
  await expect(mono(2)).toBeDisabled();
  await expect(mono(2)).toHaveAttribute("title", "No mix feeds HP2: it is muted in routing, so there is no mix to sum to mono.");
  await expect(panel(page).getByTestId("cr-feed-2")).toHaveText("Muted");
});

test("an output the Studio+ plays from two mixes has Mono disabled, naming both", async ({ page }) => {
  await answerRouting(page, { "loopback-1": { 0: { 0: [7, 0], 1: [7, 1], 2: [8, 0], 3: [8, 1] } } });
  await putWorkspace(server, { control_room: { "loopback-1": { outputs: [3] } } });
  await page.goto(`${server.url}/#/outputs/loopback-1`);
  const mono = panel(page).getByTestId("cr-mono-3");
  await mono.click();
  await expect(mono).toBeDisabled();
  await expect(mono).toHaveAttribute("title", "Line out plays Mix 1 and Mix 2: sum each to mono with the Mono on its master, on the Mixer page.");
  await expect(panel(page).getByTestId("cr-feed-3")).toHaveText("Mix 1 and Mix 2");
});
