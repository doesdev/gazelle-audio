// Cross-device mix surfaces (workspace spec §4, phase 2): made on the Workspace page, opened at
// #/surface/<id>, holding strips from both loopback devices (Quadro loopback-0, Studio+ loopback-1),
// each with its device's badge. Every control sends its own device's command, and nothing is sent
// by building or rearranging a surface.

import { expect, test, type Page } from "@playwright/test";

import { startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { putWorkspace, resetWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--dry-run"], { webUi: true });
});

test.beforeEach(() => resetWorkspace(server));

test.afterAll(async () => {
  await server?.stop();
});

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

const writes = (frames: Frame[]) => frames.filter((f) => f.command !== undefined && !f.command.startsWith("get_"));

async function serverWorkspace(): Promise<Record<string, unknown>> {
  return (await (await fetch(`${server.url}/api/v1/workspace`)).json()) as Record<string, unknown>;
}

const MIXERS = {
  "loopback-0": {
    mixes: [{ name: "Monitors" }, { name: "Cue" }],
    groups: [],
    channels: [
      { id: "vox", name: "Vox", slot: 6, source: { group: 0, channel: 0 }, main_mix: 0, sends: [1] },
      { id: "gtr", name: "Gtr", slot: 7, source: { group: 0, channel: 1 }, main_mix: 1, sends: [] },
    ],
  },
  "loopback-1": { mixes: [{ name: "Main" }], groups: [], channels: [{ id: "kick", name: "Kick", slot: 0, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }] },
};

/** With GAZELLE_SURFACE_SCREENSHOTS set to a folder, saves the page there to look at by eye. */
async function shoot(page: Page, name: string): Promise<void> {
  const folder = process.env["GAZELLE_SURFACE_SCREENSHOTS"];
  if (folder === undefined || folder === "") return;
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(300);
  await page.screenshot({ path: `${folder}/${name}` });
}

const slot = (page: Page, id: string) => page.getByTestId(`surface-strip-${id}`);
const stripIds = (page: Page) => page.locator("ga-surface .slot").evaluateAll((slots) => slots.map((s) => (s as HTMLElement).dataset["stripId"]));

test("surfaces are made, renamed, opened and deleted on the Workspace page, and device colours are chosen there", async ({ page }) => {
  const frames = recordFrames(page);
  await page.goto(`${server.url}/#/workspace`);
  await expect(page.getByText("No surfaces yet.", { exact: false })).toBeVisible();

  await page.getByTestId("surface-new-name").fill("Drum tracking");
  await page.getByTestId("surface-create").click();
  await expect.poll(async () => ((await serverWorkspace())["surfaces"] as { name: string }[] | undefined)?.map((s) => s.name)).toEqual(["Drum tracking"]);
  const id = ((await serverWorkspace())["surfaces"] as { id: string }[])[0]!.id;
  await expect(page.getByTestId(`surface-row-${id}`)).toContainText("0 strips");

  const name = page.getByTestId(`surface-rename-${id}`);
  await name.fill("Drums");
  await name.press("Enter");
  await expect.poll(async () => ((await serverWorkspace())["surfaces"] as { name: string }[])[0]?.name).toBe("Drums");

  // Badge colours: the theme's until one is chosen, saved per device, and cleared back.
  const colour = page.getByTestId("device-colour-loopback-1");
  await expect(page.getByTestId("device-colour-clear-loopback-1")).toBeDisabled();
  await colour.fill("#3fae6a");
  await expect.poll(async () => (await serverWorkspace())["device_colors"]).toEqual({ "loopback-1": "#3fae6a" });
  await page.getByTestId("device-colour-clear-loopback-1").click();
  await expect.poll(async () => (await serverWorkspace())["device_colors"]).toEqual({});

  await page.getByTestId(`surface-open-${id}`).click();
  await expect(page).toHaveURL(new RegExp(`#/surface/${id}$`));
  await expect(page.getByTestId("surface-name")).toHaveText("Drums");
  await expect(page.locator('ga-header nav a[data-page="workspace"]')).toHaveAttribute("aria-current", "page");

  await page.locator('ga-header nav a[data-page="workspace"]').click();
  const remove = page.getByTestId(`surface-delete-${id}`);
  await remove.click();
  await expect(remove).toHaveText("Confirm");
  expect((await serverWorkspace())["surfaces"], "one click deletes nothing").toHaveLength(1);
  await remove.click();
  await expect.poll(async () => (await serverWorkspace())["surfaces"]).toEqual([]);
  expect(writes(frames), "a surface is layout only").toEqual([]);

  await page.goto(`${server.url}/#/surface/${id}`);
  await expect(page.getByText("This surface does not exist")).toBeVisible();
});

test("strips from both devices are added from the picker, each with its device's badge, and can be reordered and taken off", async ({ page }) => {
  const frames = recordFrames(page);
  await putWorkspace(server, { mixers: MIXERS, aliases: { "loopback-1": "Drum rack" }, device_colors: { "loopback-0": "#b5473a", "loopback-1": "#3e9fd6" }, surfaces: [{ id: "s", name: "Both", mixes: {}, strips: [] }] });
  await page.goto(`${server.url}/#/surface/s`);
  await expect(page.getByText("No strips yet.", { exact: false })).toBeVisible();

  const add = async (device: string, kind: string, item?: string) => {
    await page.getByTestId("strip-kind").selectOption(kind);
    await page.getByTestId("strip-device").selectOption(device);
    if (item !== undefined) await page.getByTestId("strip-item").selectOption(item);
    await page.getByTestId("strip-add").click();
  };
  await add("loopback-1", "input", "preamp:0");
  await add("loopback-0", "channel", "vox");
  await add("loopback-0", "master");
  await add("loopback-1", "output", "4");
  await page.getByTestId("strip-kind").selectOption("label");
  await page.getByTestId("strip-text").fill("Drums");
  await page.getByTestId("strip-add").click();

  await expect(page.locator("ga-surface .slot")).toHaveCount(5);
  const savedStrips = async () => ((await serverWorkspace())["surfaces"] as { strips: Record<string, unknown>[] }[])[0]!.strips;
  await expect.poll(async () => (await savedStrips()).length).toBe(5);
  const strips = await savedStrips();
  expect(strips.map(({ id: _id, ...rest }) => rest)).toEqual([
    { kind: "input", device_id: "loopback-1", input: { kind: "preamp", channel: 0 } },
    { kind: "channel", device_id: "loopback-0", channel: "vox" },
    { kind: "master", device_id: "loopback-0" },
    { kind: "output", device_id: "loopback-1", output: 4 },
    { kind: "label", text: "Drums" },
  ]);
  const ids = strips.map((s) => s["id"] as string);

  // Every strip names its device in its own colour, so the two devices never read as one mixer.
  const badge = (id: string) => slot(page, id).locator("ga-surface-strip").getByTestId("device-badge");
  await expect(badge(ids[0]!)).toHaveText("Drum rack");
  await expect(badge(ids[0]!)).toHaveAttribute("data-colour", "#3e9fd6");
  await expect(badge(ids[1]!)).toHaveText("Zen Quadro Synergy Core");
  await expect(badge(ids[1]!)).toHaveAttribute("data-colour", "#b5473a");
  await expect(badge(ids[3]!)).toHaveText("Drum rack");
  await expect(badge(ids[4]!)).toBeHidden();
  await expect(slot(page, ids[1]!).locator('ga-strip[strip="6"]')).toHaveAttribute("device-id", "loopback-0");
  await expect(slot(page, ids[1]!).getByTestId("strip-caption")).toHaveText("Monitors");
  await expect(slot(page, ids[2]!).getByTestId("strip-caption")).toHaveText("Monitors master");
  await expect(slot(page, ids[0]!).getByTestId("pre-gain-0")).toBeVisible();
  await expect(slot(page, ids[3]!).getByTestId("output-4")).toContainText("Reamp");
  await expect(slot(page, ids[4]!).getByTestId("strip-label")).toHaveText("Drums");

  // Reorder with the buttons, and by dragging the grip.
  await page.getByTestId(`strip-left-${ids[4]}`).click();
  await expect.poll(() => stripIds(page)).toEqual([ids[0], ids[1], ids[2], ids[4], ids[3]]);
  const grip = slot(page, ids[0]!).locator("[data-grip]");
  const target = await slot(page, ids[2]!).boundingBox();
  const from = await grip.boundingBox();
  await page.mouse.move(from!.x + from!.width / 2, from!.y + from!.height / 2);
  await page.mouse.down();
  await page.mouse.move(target!.x + target!.width - 4, target!.y + 40, { steps: 6 });
  await page.mouse.up();
  await expect.poll(() => stripIds(page)).toEqual([ids[1], ids[2], ids[0], ids[4], ids[3]]);

  // Taking a strip off needs a second click.
  await page.getByTestId(`strip-remove-${ids[3]}`).click();
  await expect(page.locator("ga-surface .slot")).toHaveCount(5);
  await page.getByTestId(`strip-remove-${ids[3]}`).click();
  await expect.poll(() => stripIds(page)).toEqual([ids[1], ids[2], ids[0], ids[4]]);
  await expect.poll(async () => ((await serverWorkspace())["surfaces"] as { strips: unknown[] }[])[0]!.strips).toHaveLength(4);

  // A strip for a device that is not attached, or a channel that is gone, keeps its place and says so.
  await putWorkspace(server, { mixers: MIXERS, surfaces: [{ id: "s", name: "Both", mixes: {}, strips: [{ id: "x", kind: "master", device_id: "usb:gone" }, { id: "y", kind: "channel", device_id: "loopback-0", channel: "removed" }] }] });
  await page.reload();
  await expect(slot(page, "x").getByTestId("strip-gone")).toHaveText("Not connected");
  await expect(slot(page, "x").getByTestId("device-badge")).toHaveText("usb:gone");
  await expect(slot(page, "y").getByTestId("strip-gone")).toHaveText("Channel removed");
  expect(writes(frames), "building a surface sends nothing").toEqual([]);
});

test("each device's strips follow that device's mix on the surface, a pinned strip keeps its own, and every control sends to its own device", async ({ page }) => {
  const frames = recordFrames(page);
  const strips = [
    { id: "vox", kind: "channel", device_id: "loopback-0", channel: "vox" },
    { id: "gtr", kind: "channel", device_id: "loopback-0", channel: "gtr", mix: 1 },
    { id: "qm", kind: "master", device_id: "loopback-0" },
    { id: "kick", kind: "channel", device_id: "loopback-1", channel: "kick" },
    { id: "pre", kind: "input", device_id: "loopback-1", input: { kind: "preamp", channel: 2 } },
    { id: "hp1", kind: "output", device_id: "loopback-1", output: 1 },
  ];
  await putWorkspace(server, { mixers: MIXERS, surfaces: [{ id: "s", name: "Cue", mixes: {}, strips }] });
  await page.goto(`${server.url}/#/surface/s`);
  const strip = (id: string, sel: string) => slot(page, id).locator(sel);
  await shoot(page, "surface-mixes.png");

  await expect(strip("vox", "ga-strip")).toHaveAttribute("mixer", "0");
  await expect(strip("gtr", "ga-strip")).toHaveAttribute("mixer", "1");
  await expect(slot(page, "gtr").getByTestId("strip-caption")).toHaveText("Cue (pinned)");
  await page.getByTestId("surface-mix-loopback-0").selectOption("1");
  await expect(strip("vox", "ga-strip")).toHaveAttribute("mixer", "1");
  await expect(slot(page, "vox").getByTestId("strip-caption")).toHaveText("Cue");
  await expect(strip("qm", "ga-strip")).toHaveAttribute("label", "Cue");
  await expect(strip("kick", "ga-strip"), "the Studio+ keeps its own mix").toHaveAttribute("mixer", "0");
  await expect.poll(async () => ((await serverWorkspace())["surfaces"] as { mixes: unknown }[])[0]!.mixes).toEqual({ "loopback-0": 1 });
  await page.getByTestId("surface-mix-loopback-1").selectOption("2");
  await expect(strip("kick", "ga-strip")).toHaveAttribute("mixer", "2");
  await expect(strip("vox", "ga-strip"), "and the Quadro keeps its own").toHaveAttribute("mixer", "1");

  // Unpinned, the guitar follows the Quadro's mix too; pinned again, it stays.
  await page.getByTestId("strip-pin-gtr").selectOption("");
  await expect(slot(page, "gtr").getByTestId("strip-caption")).toHaveText("Cue");
  await page.getByTestId("strip-pin-gtr").selectOption("0");
  await expect(strip("gtr", "ga-strip")).toHaveAttribute("mixer", "0");
  expect(writes(frames), "choosing mixes sends nothing").toEqual([]);

  // The Quadro Vox fader in Cue: set_mixer on loopback-0, mix 2, channel 7.
  const vox = strip("vox", "ga-strip").getByTestId("fader-6");
  await vox.focus();
  await vox.press("PageDown");
  await expect.poll(() => writes(frames).at(-1)).toMatchObject({ device_id: "loopback-0", command: "set_mixer", args: { mixer_id: 1, channel: 7, level: 6 } });
  // The Studio+ preamp 3's gain and HP1's mute go to loopback-1.
  const gain = strip("pre", "ga-surface-strip").getByTestId("pre-gain-2");
  await gain.focus();
  await gain.press("ArrowRight");
  await expect.poll(() => writes(frames).at(-1)).toMatchObject({ device_id: "loopback-1", command: "set_pre_gain", args: { id: 2 } });
  await strip("hp1", "ga-surface-strip").getByTestId("out-mute-1").click();
  await expect.poll(() => writes(frames).at(-1)).toMatchObject({ device_id: "loopback-1", command: "set_mute", args: { id: 1, mute: 1 } });
  await expect(page.getByTestId("last-sent")).toContainText("set_mute to Zen Studio+");
  expect(writes(frames).map((f) => f.command)).toEqual(["set_mixer", "set_pre_gain", "set_mute"]);
});

test("a surface fits a phone: the page does not scroll sideways, the strips do", async ({ page }) => {
  const strips = [
    { id: "a", kind: "input", device_id: "loopback-1", input: { kind: "preamp", channel: 0 } },
    { id: "b", kind: "input", device_id: "loopback-1", input: { kind: "preamp", channel: 1 } },
    { id: "c", kind: "channel", device_id: "loopback-0", channel: "vox" },
    { id: "d", kind: "master", device_id: "loopback-0" },
  ];
  await putWorkspace(server, { mixers: MIXERS, surfaces: [{ id: "s", name: "Phone", mixes: {}, strips }] });
  await page.setViewportSize({ width: 375, height: 812 });
  await page.goto(`${server.url}/#/surface/s`);
  await expect(page.locator("ga-surface .slot")).toHaveCount(4);
  const doc = await page.evaluate(() => ({ scroll: document.documentElement.scrollWidth, client: document.documentElement.clientWidth }));
  expect(doc.scroll).toBeLessThanOrEqual(doc.client);
  const row = await page.getByTestId("surface-strips").evaluate((el) => ({ scroll: el.scrollWidth, client: el.clientWidth, overflow: getComputedStyle(el).overflowX }));
  expect(row.overflow, "the person can scroll them").toBe("auto");
  expect(row.scroll).toBeGreaterThan(row.client);
});
