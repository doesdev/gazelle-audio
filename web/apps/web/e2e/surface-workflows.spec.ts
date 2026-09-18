// The user's two cross-device workflows (workspace spec §4.3, §4.4) on the loopback devices, with
// digital cables (phase 3): the Quadro is loopback-0, the Studio+ loopback-1.
//   (a) Drums on the Studio+ preamps go out of its ADAT port into the Quadro's ADAT inputs, and are
//       mixed on the Quadro beside its own channels.
//   (b) The Quadro plays music through a mix out of its S/PDIF output into the Studio+'s S/PDIF input,
//       with the two ports side by side.
// A cable routes nothing; the port strip's menu is a real routing change on the device that owns the
// port, read before it is written. With GAZELLE_SURFACE_SCREENSHOTS set to a folder, each workflow is
// saved there at desktop and phone width.

import { readFileSync } from "node:fs";
import { join } from "node:path";

import { expect, test, type Page } from "@playwright/test";

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

const QUADRO = "loopback-0";
const STUDIO = "loopback-1";

interface Frame {
  device_id?: string;
  command?: string;
  args?: Record<string, unknown>;
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

/** set_routing bytes: bank_idx at 18, then 32 (source, channel) pairs, MUTE but `routed` (as mixer.spec.ts builds them). */
function routingHex(family: "quadro" | "studio", destination: number, routed: Record<number, [number, number]>): string {
  const file = family === "quadro" ? "ground_truth.json" : "ground_truth_studio.json";
  const vectors = JSON.parse(readFileSync(join(REPO_ROOT, "crates", "gazelle-audio-protocol", "tests", file), "utf8")) as Record<string, string>;
  const bytes = Uint8Array.from(vectors["set_routing"]?.match(/../g) ?? [], (pair) => Number.parseInt(pair, 16));
  const mute = family === "quadro" ? 10 : 11;
  bytes[18] = destination;
  for (let slot = 0; slot < 32; slot++) {
    const [source, channel] = routed[slot] ?? [mute, 0];
    bytes[19 + 2 * slot] = source;
    bytes[20 + 2 * slot] = channel;
  }
  return [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
}

async function shoot(page: Page, name: string): Promise<void> {
  const folder = process.env["GAZELLE_SURFACE_SCREENSHOTS"];
  if (folder === undefined || folder === "") return;
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(400);
  await page.screenshot({ path: `${folder}/${name}-desktop.png` });
  const size = page.viewportSize();
  await page.setViewportSize({ width: 375, height: 812 });
  await page.waitForTimeout(400);
  await page.screenshot({ path: `${folder}/${name}-phone.png` });
  if (size !== null) await page.setViewportSize(size);
}

const slot = (page: Page, id: string) => page.getByTestId(`surface-strip-${id}`);
const lastSent = (page: Page) => page.locator("ga-surface").getByTestId("last-sent");
const channel = (id: string, name: string, slotIndex: number, group: number, input: number) => ({ id, name, slot: slotIndex, source: { group, channel: input }, main_mix: 0, sends: [] });

test("workflow (a): drums on the Studio+ preamps reach the Quadro over ADAT and are mixed there, each strip saying where it comes from", async ({ page }) => {
  const frames = recordFrames(page);
  await putWorkspace(server, {
    aliases: { [STUDIO]: "Drum rack", [QUADRO]: "Desk" },
    mixers: {
      // The Quadro's cue mix: drums on ADAT IN 1–4 (source group 3), vocal and guitar on its preamps.
      [QUADRO]: { mixes: [{ name: "Cue" }], groups: [], channels: [channel("kick", "Kick in", 6, 3, 0), channel("snare", "Snare in", 7, 3, 1), channel("ohl", "OH L", 8, 3, 2), channel("ohr", "OH R", 9, 3, 3), channel("vox", "Vox", 10, 0, 0), channel("gtr", "Gtr", 11, 0, 1)] },
      [STUDIO]: { mixes: [], groups: [], channels: [{ id: "k", name: "Kick", slot: 0, source: { group: 0, channel: 0 }, sends: [] }, { id: "s", name: "Snare", slot: 1, source: { group: 0, channel: 1 }, sends: [] }] },
    },
    cables: [{ id: "adat", from: { device_id: STUDIO, port: "ADAT_OUT", first: 0 }, to: { device_id: QUADRO, port: "ADAT_IN", first: 0 }, channels: 8 }],
    surfaces: [
      {
        id: "drums",
        name: "Drum tracking",
        mixes: {},
        strips: [
          { id: "pre1", kind: "input", device_id: STUDIO, input: { kind: "preamp", channel: 0 } },
          { id: "pre2", kind: "input", device_id: STUDIO, input: { kind: "preamp", channel: 1 } },
          { id: "adatout", kind: "port", device_id: STUDIO, port: "ADAT_OUT", first: 0 },
          { id: "adat1", kind: "input", device_id: QUADRO, input: { kind: "adat", channel: 0 } },
          { id: "kick", kind: "channel", device_id: QUADRO, channel: "kick" },
          { id: "snare", kind: "channel", device_id: QUADRO, channel: "snare" },
          { id: "vox", kind: "channel", device_id: QUADRO, channel: "vox" },
          { id: "cue", kind: "master", device_id: QUADRO },
        ],
      },
    ],
  });
  await page.goto(`${server.url}/#/surface/drums`);

  // Each device's strips carry its badge: the preamps are the Studio+'s, the cue mix the Quadro's.
  await expect(slot(page, "pre1").getByTestId("device-badge")).toHaveText("Drum rack");
  await expect(slot(page, "kick").getByTestId("device-badge")).toHaveText("Desk");
  await expect(slot(page, "cue").locator('ga-strip[strip="master"]')).toHaveAttribute("label", "Cue");
  // The Quadro's own panel never sets its ADAT gains, so they stay read-only here too.
  await expect(slot(page, "adat1").getByTestId("adat-gain-0")).toHaveClass(/readonly/);
  await expect(slot(page, "adat1").getByTestId("adat-gain-0")).not.toHaveAttribute("role", "slider");

  // The cable says where the Quadro's ADAT inputs come from, before the Studio+'s routing is known (a dry run reads nothing).
  await expect(slot(page, "kick").getByTestId("provenance")).toHaveText("from Drum rack ADAT out 1");
  await expect(slot(page, "kick").getByTestId("provenance")).toBeVisible();
  await expect(slot(page, "adat1").getByTestId("provenance")).toHaveText("from Drum rack ADAT out 1");
  await expect(slot(page, "vox").getByTestId("provenance"), "a preamp channel has no cable").toBeHidden();
  await expect(slot(page, "adatout").getByTestId("port-feed-0")).toHaveText("← not read");
  await expect(page.getByTestId("cable-health-adat")).toContainText("Drum rack ADAT out 1–8 → Desk ADAT in 1–8");

  // Route the drum preamps 1/2 to ADAT out 1/2 from the port strip: one set_routing on the Studio+ (ADAT OUT is its destination 7).
  const sentBefore = writes(frames).length;
  await slot(page, "adatout").getByTestId("port-route-0").selectOption({ label: "PREAMP 1/2" });
  await expect(lastSent(page)).toContainText(`Dry run, would send set_routing to Drum rack: ${routingHex("studio", 7, { 0: [0, 0], 1: [0, 1] })}`);
  expect(writes(frames).slice(sentBefore).map((f) => [f.device_id, f.command])).toEqual([[STUDIO, "set_routing"]]);
  expect(frames.filter((f) => f.command === "get_routing" && f.device_id === STUDIO && f.args === undefined).length, "the group is read before it is written").toBeGreaterThan(0);
  await expect(slot(page, "adatout").getByTestId("port-feed-0")).toHaveText("← PREAMP 1/2");
  await expect(slot(page, "adatout").getByTestId("port-note")).toHaveText("Bit for bit: no level on Drum rack.");
  await expect(slot(page, "adatout").getByTestId("port-route-0"), "the menu is an action, not a value").toHaveValue("");
  // Each pair has its own menu: preamps 3/4 to ADAT out 3/4, on top of what is already routed.
  await slot(page, "adatout").getByTestId("port-route-2").selectOption({ label: "PREAMP 3/4" });
  await expect(lastSent(page)).toContainText(`Dry run, would send set_routing to Drum rack: ${routingHex("studio", 7, { 0: [0, 0], 1: [0, 1], 2: [0, 2], 3: [0, 3] })}`);
  await expect(slot(page, "adatout").getByTestId("port-feed-2")).toHaveText("← PREAMP 3/4");

  // Now the provenance names the Studio+ source and the channel the Studio+ layout gives it.
  await expect(slot(page, "kick").getByTestId("provenance")).toHaveText("from Drum rack ADAT out 1 ← PREAMP 1 (Kick)");
  await expect(slot(page, "snare").getByTestId("provenance")).toHaveText("from Drum rack ADAT out 2 ← PREAMP 2 (Snare)");

  // The drummer's gain goes to the Studio+; the cue level to the Quadro.
  const gain = slot(page, "pre1").getByTestId("pre-gain-0");
  await gain.focus();
  await gain.press("ArrowRight");
  await expect.poll(() => writes(frames).at(-1)).toMatchObject({ device_id: STUDIO, command: "set_pre_gain", args: { id: 0 } });
  const fader = slot(page, "kick").getByTestId("fader-6");
  await fader.focus();
  await fader.press("PageDown");
  await expect.poll(() => writes(frames).at(-1)).toMatchObject({ device_id: QUADRO, command: "set_mixer", args: { mixer_id: 0, channel: 7, level: 6 } });

  await shoot(page, "workflow-a");
});

test("workflow (b): the Quadro plays a mix out of S/PDIF into the Studio+, with both ends side by side and the mix master as the level", async ({ page }) => {
  const frames = recordFrames(page);
  await putWorkspace(server, {
    aliases: { [QUADRO]: "Desk", [STUDIO]: "Drum rack" },
    mixers: {
      [QUADRO]: { mixes: [{}, {}, {}, { name: "Music" }], groups: [], channels: [] },
      // The Studio+ plays S/PDIF IN (source group 5) in its main mix.
      [STUDIO]: { mixes: [{ name: "Main" }], groups: [], channels: [channel("music", "Music", 0, 5, 0)] },
    },
    cables: [{ id: "spdif", from: { device_id: QUADRO, port: "SPDIF_OUT", first: 0 }, to: { device_id: STUDIO, port: "SPDIF_IN", first: 0 }, channels: 2 }],
    surfaces: [{ id: "music", name: "Music via S/PDIF", mixes: {}, strips: [] }],
  });
  await page.goto(`${server.url}/#/surface/music`);

  // First the Studio+ channel the music plays in: the cable says it comes from the Quadro, so the
  // Quadro's S/PDIF routing is read (once) to say what it sends.
  const reads = () => frames.filter((f) => f.command === "get_routing" && f.device_id === QUADRO && (f as { ext3?: number }).ext3 === 6).length;
  await page.getByTestId("strip-kind").selectOption("channel");
  await page.getByTestId("strip-device").selectOption(STUDIO);
  await page.getByTestId("strip-item").selectOption({ label: "Music" });
  await page.getByTestId("strip-add").click();
  await expect(page.locator("ga-surface .slot")).toHaveCount(1);
  const [music] = (await page.locator("ga-surface .slot").evaluateAll((slots) => slots.map((s) => (s as HTMLElement).dataset["stripId"] as string))) as [string];
  await expect(slot(page, music).getByTestId("provenance")).toHaveText("from Desk S/PDIF out 1");
  await expect.poll(reads, "the sender's S/PDIF out group is read").toBe(1);

  // Both ends of the cable in one go: the Quadro's S/PDIF out, then the Studio+'s S/PDIF inputs.
  await page.getByTestId("strip-kind").selectOption("cable");
  await expect(page.getByTestId("strip-device")).toBeDisabled();
  await page.getByTestId("strip-item").selectOption({ label: "Desk S/PDIF out 1–2 → Drum rack S/PDIF in 1–2" });
  await page.getByTestId("strip-add").click();
  await expect(page.locator("ga-surface .slot")).toHaveCount(4);
  const kinds = await page.locator("ga-surface .slot").evaluateAll((slots) => slots.map((s) => (s as HTMLElement).dataset["kind"]));
  expect(kinds).toEqual(["channel", "port", "input", "input"]);
  const ids = await page.locator("ga-surface .slot").evaluateAll((slots) => slots.map((s) => (s as HTMLElement).dataset["stripId"] as string));
  const [, port, left, right] = ids as [string, string, string, string];
  await expect(slot(page, port).getByTestId("device-badge")).toHaveText("Desk");
  await expect(slot(page, port).getByTestId("strip-caption")).toHaveText("S/PDIF out");
  await expect(slot(page, left).getByTestId("device-badge")).toHaveText("Drum rack");
  await expect(slot(page, left).getByTestId("provenance")).toHaveText("from Desk S/PDIF out 1");
  await expect(slot(page, right).getByTestId("provenance")).toHaveText("from Desk S/PDIF out 2");
  // The Studio+'s S/PDIF gain is its own to set, and its SRC switch sits on the input.
  await expect(slot(page, left).getByTestId("spdif-gain-0")).toHaveAttribute("role", "slider");
  await expect(slot(page, left).getByTestId("spdif-src")).toBeVisible();

  // And the Studio+'s Monitor.
  await page.getByTestId("strip-kind").selectOption("output");
  await page.getByTestId("strip-device").selectOption(STUDIO);
  await page.getByTestId("strip-item").selectOption({ label: "Monitor" });
  await page.getByTestId("strip-add").click();
  await expect(page.locator("ga-surface .slot")).toHaveCount(5);

  // Route Mix 4 ("Music") to the S/PDIF out: a set_routing on the Quadro (SPDIF OUT is destination 6, MIX4 L/R source 9).
  const sentBefore = writes(frames).length;
  await slot(page, port).getByTestId("port-route-0").selectOption({ label: "Music" });
  await expect(lastSent(page)).toContainText(`Dry run, would send set_routing to Desk: ${routingHex("quadro", 6, { 0: [9, 0], 1: [9, 1] })}`);
  expect(writes(frames).slice(sentBefore).map((f) => [f.device_id, f.command])).toEqual([[QUADRO, "set_routing"]]);
  await expect(slot(page, port).getByTestId("port-feed-0")).toHaveText("← Music");
  // The mix's master is the output's level, beside the port.
  await expect(slot(page, port).getByTestId("port-master-caption-3")).toHaveText("Music master, feeds S/PDIF out");
  const master = slot(page, port).locator('ga-strip[strip="master"]');
  await expect(master).toHaveAttribute("mixer", "3");
  await expect(master).toHaveAttribute("device-id", QUADRO);
  await expect(slot(page, left).getByTestId("provenance")).toHaveText("from Desk S/PDIF out 1 ← Music L");
  const level = master.getByTestId("fader-master");
  await level.focus();
  await level.press("PageDown");
  await expect.poll(() => writes(frames).at(-1)).toMatchObject({ device_id: QUADRO, command: "set_mixer", args: { mixer_id: 3, channel: 0 } });
  // The Quadro reports its S/PDIF output's level.
  await expect(slot(page, port).getByTestId("port-level-0")).toBeVisible();

  await shoot(page, "workflow-b");

  // Played straight from USB instead: bit for bit, so there is no master to show.
  await slot(page, port).getByTestId("port-route-0").selectOption({ label: "USB 1 PLAY 1/2" });
  await expect(lastSent(page)).toContainText(`Dry run, would send set_routing to Desk: ${routingHex("quadro", 6, { 0: [1, 0], 1: [1, 1] })}`);
  await expect(slot(page, port).getByTestId("port-feed-0")).toHaveText("← USB 1 PLAY 1/2");
  await expect(slot(page, port).getByTestId("port-note")).toHaveText("Bit for bit: no level on Desk.");
  await expect(slot(page, port).locator('ga-strip[strip="master"]')).toHaveCount(0);

  // The SRC switch is the Studio+'s.
  await slot(page, left).getByTestId("spdif-src").click();
  await expect.poll(() => writes(frames).at(-1)).toMatchObject({ device_id: STUDIO, command: "set_spdif_src" });
});

test("cables are declared and removed on the Workspace page, from an output to an input of the same kind, and routing nothing", async ({ page }) => {
  const frames = recordFrames(page);
  await putWorkspace(server, { aliases: { [QUADRO]: "Desk", [STUDIO]: "Drum rack" } });
  await page.goto(`${server.url}/#/workspace`);
  await expect(page.getByText("No cables declared.", { exact: false })).toBeVisible();

  await page.getByTestId("cable-from").selectOption({ label: "Drum rack ADAT out 9–16" });
  // Only inputs of the same kind are offered, and the channels follow the port.
  const offered = await page.getByTestId("cable-to").locator("option:not([hidden])").allTextContents();
  expect(offered).toEqual(["Desk ADAT in", "Drum rack ADAT in 1–8", "Drum rack ADAT in 9–16"]);
  await expect(page.getByTestId("cable-channels")).toHaveValue("8");
  await page.getByTestId("cable-to").selectOption({ label: "Drum rack ADAT in 1–8" });
  await page.getByTestId("cable-declare").click();
  await expect(page.getByTestId("cable-problem")).toHaveText("a cable joins two devices");
  await expect(page.getByTestId("cable-problem")).toBeVisible();

  await page.getByTestId("cable-to").selectOption({ label: "Desk ADAT in" });
  await page.getByTestId("cable-declare").click();
  await expect(page.getByTestId("cable-problem")).toBeHidden();
  const saved = async () => ((await (await fetch(`${server.url}/api/v1/workspace`)).json()) as { cables: { id: string }[] }).cables;
  await expect.poll(saved).toMatchObject([{ from: { device_id: STUDIO, port: "ADAT_OUT", first: 8 }, to: { device_id: QUADRO, port: "ADAT_IN", first: 0 }, channels: 8 }]);
  const id = (await saved())[0]!.id;
  await expect(page.getByTestId(`cable-row-${id}`)).toContainText("Drum rack ADAT out 9–16 → Desk ADAT in 1–8");
  await expect(page.getByTestId(`cable-health-${id}`)).toBeVisible();

  const remove = page.getByTestId(`cable-remove-${id}`);
  await remove.click();
  await expect(remove).toHaveText("Confirm");
  // Reports keep arriving (the loopback sends them every 50 ms); the confirmation waits for its click all the same.
  await page.waitForTimeout(300);
  await expect(remove).toHaveText("Confirm");
  await remove.click();
  await expect.poll(saved).toEqual([]);
  expect(writes(frames), "a cable is a note about the room: nothing is sent").toEqual([]);
});
