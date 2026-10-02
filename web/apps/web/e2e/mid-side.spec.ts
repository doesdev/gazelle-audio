// Monitoring a mid and a side microphone as stereo, on the loopback without dry run so reads answer
// and writes land: the confirm lists every change and nothing reaches the device or the workspace
// until Confirm; the pans, links, group and extra channel it makes; the Width moving both side
// strips; the warning when one drifts, with Put it back; the strips in the dock; and removal putting
// everything back.
//
// The loopback sends no status reports here, so the preamps' settings are unknown to the app: the
// second preamp way is planned without matching gains, as it is in dry run. Every chain on the
// loopback holds effects, so the effect way is refused with its reason, which is what a device with
// no free chain shows; the effect way itself is covered by test/mid-side.test.ts.

import { expect, test, type Page } from "@playwright/test";
import { join } from "node:path";

import { startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { putWorkspace } from "./workspace.ts";

let server: RunningServer;

/** Where screenshots for the owner go, when asked for: `GAZELLE_SHOTS=<dir>`. */
const SHOTS = process.env["GAZELLE_SHOTS"];
const shot = async (page: Page, name: string) => {
  if (SHOTS !== undefined) await page.screenshot({ path: join(SHOTS, `${name}.png`) });
};

test.beforeAll(async () => {
  server = await startServer(["--backend", "loopback"], { webUi: true });
});

test.afterAll(async () => {
  await server?.stop();
});

const DEVICE = "loopback-0";
// Quadro topology positions: sources, and mix 1's input group.
const PREAMP = 0;
const MUTE = 10;
const MIX_IN = 8;
const O = "Ø";

async function command(name: string, body: Record<string, unknown>, ext3?: number): Promise<Record<string, unknown>> {
  const response = await fetch(`${server.url}/api/v1/devices/${DEVICE}/command/${name}${ext3 === undefined ? "" : `?ext3=${ext3}`}`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
  const text = await response.text();
  expect(response.ok, text).toBe(true);
  return JSON.parse(text) as Record<string, unknown>;
}

function strip(slot: number, values: { level?: number; pan?: number; mute?: boolean }): Promise<unknown> {
  return command("set_mixer", { mixer_id: 0, channel: slot + 1, level: values.level ?? 0, pan: values.pan ?? 32, mute: values.mute ? 1 : 0, solo: 0 });
}

async function strips(): Promise<{ level: number; pan: number; mute: number }[]> {
  const reply = await command("get_mixer", {}, 0);
  return (reply["response"] as { entries: { level: number; pan: number; mute: number }[] }).entries.slice(1);
}

async function routing(): Promise<[number, number][]> {
  const reply = await command("get_routing", {}, MIX_IN);
  return (reply["response"] as { bank_configs: { in_periph_id: number; in_chann: number }[] }).bank_configs.slice(0, 32).map((c) => [c.in_periph_id, c.in_chann]);
}

interface Saved {
  links: { kind: string; mode: string; members: { channel: number }[] }[];
  mixers: Record<string, { groups: { id: string; name: string; mid_side?: Record<string, unknown> }[]; channels: { id: string; name: string; slot: number; group?: string; source?: { group: number; channel: number } }[] }>;
}

async function workspace(): Promise<Saved> {
  return (await (await fetch(`${server.url}/api/v1/workspace`)).json()) as Saved;
}

/** A mid on Preamp 1 and a side on Preamp 2 in mix 1, panned a little apart, as someone might have left them. */
test.beforeEach(async () => {
  await command("set_routing", { bank_idx: MIX_IN, bank_configs: Array.from({ length: 32 }, (_, slot) => (slot === 6 ? [PREAMP, 0] : slot === 7 ? [PREAMP, 1] : [MUTE, 0])).flat() });
  for (let slot = 0; slot < 32; slot++) await strip(slot, {});
  await strip(6, { pan: 20 });
  await strip(7, { pan: 44 });
  await putWorkspace(server, {
    mixers: {
      [DEVICE]: {
        mixes: [{ name: "Tracking" }],
        groups: [],
        channels: [
          { id: "m", name: "Mid", slot: 6, source: { group: PREAMP, channel: 0 }, main_mix: 0, sends: [] },
          { id: "s", name: "Side", slot: 7, source: { group: PREAMP, channel: 1 }, main_mix: 0, sends: [] },
        ],
      },
    },
  });
});

const panel = (page: Page) => page.locator("ga-mid-side");

/** Selects the mid and then the side by their name bars and opens the confirm. */
async function ask(page: Page): Promise<void> {
  await page.getByTestId("select-6").click();
  await page.getByTestId("select-7").click({ modifiers: ["Control"] });
  await page.getByTestId("mid-side-start").click();
  await expect(panel(page).getByTestId("mid-side-confirm")).toBeVisible();
}

/** Sets the decode up through Preamp 3 and waits until the workspace holds it. Resolves to its group's id. */
async function setUp(page: Page): Promise<string> {
  await page.goto(`${server.url}/#/mixer/${DEVICE}/0`);
  await ask(page);
  await panel(page).getByTestId("mid-side-go").click();
  await expect(panel(page).getByTestId("mid-side-status")).toHaveText("Mid and Side are monitored as M/S in Tracking.");
  await expect.poll(async () => (await workspace()).mixers[DEVICE]?.groups[0]?.mid_side?.["inverted"] ?? "").not.toBe("");
  return (await workspace()).mixers[DEVICE]?.groups[0]?.id ?? "";
}

test("Monitor as M/S lists every change and makes none until Confirm; then the pans, links, group and extra channel are there", async ({ page }) => {
  await page.goto(`${server.url}/#/mixer/${DEVICE}/0`);
  await expect(page.getByTestId("mid-side-start"), "out of the way until two channels are selected").toBeHidden();
  await page.getByTestId("select-6").click();
  await expect(page.getByTestId("mid-side-start")).toBeHidden();
  await page.getByTestId("select-7").click({ modifiers: ["Control"] });
  await page.getByTestId("mid-side-start").click();

  const box = panel(page).getByTestId("mid-side-confirm");
  await expect(box).toBeVisible();
  await expect(box.getByTestId("mid-side-roles")).toHaveText("Mid: Mid. Side: Side.");
  await expect(box.getByTestId("mid-side-way")).toHaveValue("preamp:2");
  await expect(box.getByTestId("mid-side-plan").locator("li")).toHaveText([
    "Link Preamp 3 to Preamp 2, so its gain, type and 48V follow",
    `Switch Preamp 3's polarity (${O}) on`,
    "Pan Mid to the centre in Tracking (it is at L 40%)",
    "Pan Side hard left in Tracking (it is at R 40%)",
    `Add a channel, Side ${O}, on Preamp 3 to Tracking: the inverted copy, panned hard right at Side's level, 0 dB`,
    `Link Side and Side ${O}, so their levels, mutes and solos always match`,
    'Group the three as "M/S: Mid"',
  ]);
  // What is recorded is said in so many words, and so is why the effect way is not on offer here.
  await expect(box.getByTestId("mid-side-notes")).toContainText("What is recorded does not change: Preamp 1 and Preamp 2 reach your DAW raw");
  await expect(box.getByTestId("mid-side-notes")).toContainText("Preamp 3 is one more input your DAW can record");
  await expect(box.getByTestId("mid-side-effect-why")).toContainText("An effect chain cannot make the copy here:");

  // Swap makes the other channel the mid, and the list follows; swapped back, it is as it was.
  await box.getByTestId("mid-side-swap").click();
  await expect(box.getByTestId("mid-side-roles")).toHaveText("Mid: Side. Side: Mid.");
  await expect(box.getByTestId("mid-side-plan").locator("li").last()).toHaveText('Group the three as "M/S: Side"');
  await box.getByTestId("mid-side-swap").click();
  await shot(page, "mid-side-1-confirm");
  // Another preamp for the copy changes the list too.
  await box.getByTestId("mid-side-way").selectOption("preamp:3");
  await expect(box.getByTestId("mid-side-plan").locator("li").first()).toHaveText("Link Preamp 4 to Preamp 2, so its gain, type and 48V follow");
  await box.getByTestId("mid-side-way").selectOption("preamp:2");

  // Nothing has been sent or saved: the device and the workspace are as they were.
  const before = await strips();
  expect([before[6]?.pan, before[7]?.pan, before[8]?.pan]).toEqual([20, 44, 32]);
  expect((await routing())[8]).toEqual([MUTE, 0]);
  expect((await workspace()).mixers[DEVICE]?.groups).toEqual([]);
  expect((await workspace()).links).toEqual([]);

  // Cancel changes nothing either.
  await box.getByTestId("mid-side-cancel").click();
  await expect(box).toBeHidden();
  expect((await strips())[6]?.pan).toBe(20);
  expect((await workspace()).mixers[DEVICE]?.channels.length).toBe(2);

  await page.getByTestId("mid-side-start").click();
  await box.getByTestId("mid-side-go").click();
  await expect(panel(page).getByTestId("mid-side-status")).toHaveText("Mid and Side are monitored as M/S in Tracking.");
  await expect(box).toBeHidden();
  await expect(page.getByTestId("mid-side-start"), "the selection is cleared, so the two do not move together").toBeHidden();

  // On the device: centre, hard left, and the copy on Preamp 3 hard right at the side's level.
  await expect.poll(async () => (await strips()).slice(6, 9).map((s) => [s.pan, s.level])).toEqual([[32, 0], [2, 0], [62, 0]]);
  await expect.poll(async () => (await routing())[8]).toEqual([PREAMP, 2]);
  // In the workspace: the group with its decode, the three channels in it, and the two links.
  await expect.poll(async () => (await workspace()).mixers[DEVICE]?.groups.map((g) => g.name)).toEqual(["M/S: Mid"]);
  const saved = await workspace();
  const mixer = saved.mixers[DEVICE];
  const group = mixer?.groups[0];
  expect(mixer?.channels.map((c) => [c.name, c.slot, c.group])).toEqual([["Mid", 6, group?.id], ["Side", 7, group?.id], [`Side ${O}`, 8, group?.id]]);
  expect(mixer?.channels[2]?.source).toEqual({ group: PREAMP, channel: 2 });
  expect(group?.mid_side).toMatchObject({ mid: "m", side: "s", inverted: mixer?.channels[2]?.id, via: "preamp", preamp: 2, pans: { "0": { mid: 20, side: 44 } } });
  expect(saved.links.map((l) => [l.kind, l.mode, l.members.map((m) => m.channel)]).sort()).toEqual([["mixer", "absolute", [7, 8]], ["preamp", "absolute", [1, 2]]]);

  await shot(page, "mid-side-2-decoded");
  // On the page: the three strips marked, the copy's polarity switch on, and the decode's line saying what is recorded.
  await expect(page.locator('ga-channel-group ga-channel')).toHaveCount(3);
  for (const [slot, mark] of [[6, "M"], [7, "S"], [8, "-S"]] as const) await expect(page.getByTestId(`mid-side-${slot}`)).toHaveText(mark);
  await expect(page.getByTestId("mid-side-6")).not.toHaveAttribute("data-warning", "");
  await expect(page.getByTestId("pre-phase-ch-8")).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("pre-phase-ch-7"), "the side preamp, which is recorded, is not inverted").toHaveAttribute("aria-pressed", "false");
  const row = panel(page).getByTestId(`mid-side-row-${group?.id}`);
  await expect(row).not.toHaveAttribute("data-warning", "");
  await expect(row.getByTestId(`mid-side-recorded-${group?.id}`)).toHaveText("Recorded raw: Preamp 1 and Preamp 2. Preamp 3 is the inverted split, one more input that can be ignored.");
});

test("the Width moves both side strips and leaves the mid; a side fader moves its copy too", async ({ page }) => {
  const group = await setUp(page);
  const bar = panel(page).getByTestId(`mid-side-width-${group}`);
  await expect(bar).toHaveAttribute("aria-valuetext", "0 dB");
  await bar.focus();
  for (let i = 0; i < 4; i++) await page.keyboard.press("ArrowLeft");
  await expect(bar).toHaveAttribute("aria-valuetext", "-4 dB");
  await expect.poll(async () => (await strips()).slice(6, 9).map((s) => s.level)).toEqual([0, 4, 4]);

  // The side's own fader, one step quieter: the link takes the copy with it, and the Width follows.
  await page.getByTestId("fader-7").focus();
  await page.keyboard.press("ArrowDown");
  await expect.poll(async () => (await strips()).slice(6, 9).map((s) => s.level)).toEqual([0, 5, 5]);
  await expect(bar).toHaveAttribute("aria-valuetext", "-5 dB");
  // Mute on the copy mutes the side strip as well.
  await page.locator('ga-strip[strip="8"]').getByRole("button", { name: /mute$/ }).click();
  await expect.poll(async () => (await strips()).slice(6, 9).map((s) => s.mute)).toEqual([0, 1, 1]);
  await expect(panel(page).getByTestId(`mid-side-row-${group}`)).not.toHaveAttribute("data-warning", "");
});

test("a side strip that drifts and a pan moved are warned about on the group, and Put it back mends them after a confirm", async ({ page }) => {
  const group = await setUp(page);
  // Changed behind the app's back, as the vendor's panel would: the copy 9 dB down and the side off its pan.
  await strip(8, { level: 9, pan: 62 });
  await strip(7, { level: 0, pan: 30 });
  await page.reload();

  const row = panel(page).getByTestId(`mid-side-row-${group}`);
  await expect(row).toHaveAttribute("data-warning", "");
  await expect(row.getByTestId(`mid-side-problems-${group}`).locator("li")).toHaveText([
    "Side is panned L 7% in Tracking, not hard left, so left and right no longer decode.",
    `Side is at 0 dB and Side ${O} at -9 dB in Tracking: the two must match.`,
  ]);
  await shot(page, "mid-side-3-warning");
  // The strips and the group's band say so too.
  await expect(page.getByTestId("mid-side-8")).toHaveAttribute("data-warning", "");
  await expect(page.getByTestId("mid-side-8")).toHaveAttribute("title", /the two must match/);
  await expect(page.getByTestId(`group-warning-${group}`)).toBeVisible();

  await row.getByTestId(`mid-side-repair-${group}`).click();
  const box = panel(page).getByTestId("mid-side-confirm");
  await expect(box.getByTestId("mid-side-plan").locator("li")).toHaveText(["Pan Side hard left in Tracking", `Set Side ${O} to 0 dB in Tracking`]);
  expect((await strips()).slice(7, 9).map((s) => [s.pan, s.level]), "listed, not yet made").toEqual([[30, 0], [62, 9]]);
  await box.getByTestId("mid-side-go").click();
  await expect(panel(page).getByTestId("mid-side-status")).toHaveText("M/S: Mid is put back.");
  await expect.poll(async () => (await strips()).slice(7, 9).map((s) => [s.pan, s.level])).toEqual([[2, 0], [62, 0]]);
  await expect(row).not.toHaveAttribute("data-warning", "");
  await expect(page.getByTestId("mid-side-8")).not.toHaveAttribute("data-warning", "");
  await expect(page.getByTestId(`group-warning-${group}`)).toBeHidden();
});

test("in the dock the three strips are marked and the side strips still move together", async ({ page }) => {
  await setUp(page);
  await page.goto(`${server.url}/#/inputs/${DEVICE}`);
  const dock = page.locator("ga-mixer-dock");
  await expect(dock.locator("ga-strip[compact]")).toHaveCount(4);
  await expect(dock.locator('ga-strip[strip="8"]')).toHaveAttribute("label", `Side ${O}`);
  for (const [slot, mark] of [[6, "M"], [7, "S"], [8, "-S"]] as const) await expect(dock.getByTestId(`mid-side-${slot}`)).toHaveText(mark);
  await dock.getByTestId("fader-8").focus();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  await expect.poll(async () => (await strips()).slice(6, 9).map((s) => s.level)).toEqual([0, 2, 2]);
  await expect(dock.getByTestId("mid-side-7")).not.toHaveAttribute("data-warning", "");

  // A copy that has drifted shows on the dock's strips as well.
  await strip(8, { level: 12, pan: 62 });
  await page.reload();
  await expect(page.locator("ga-mixer-dock").getByTestId("mid-side-8")).toHaveAttribute("data-warning", "");
});

test("removing the decode lists what it puts back, and puts it back only on Confirm; the group's own remove opens the same list", async ({ page }) => {
  const group = await setUp(page);
  // The band's remove does not drop the group alone: it opens the decode's removal.
  await page.getByRole("button", { name: "Remove group M/S: Mid" }).click();
  const box = panel(page).getByTestId("mid-side-confirm");
  await expect(box).toContainText("Remove M/S: Mid?");
  await box.getByTestId("mid-side-cancel").click();
  expect((await workspace()).mixers[DEVICE]?.groups.length, "cancelled, the group is still there").toBe(1);

  await panel(page).getByTestId(`mid-side-remove-${group}`).click();
  await expect(box.getByTestId("mid-side-plan").locator("li")).toHaveText([
    "Pan Mid back to L 40% in Tracking",
    "Pan Side back to R 40% in Tracking",
    `Unlink Side and Side ${O}`,
    `Remove the channel Side ${O}, the inverted copy`,
    "Unlink Preamp 3 from Preamp 2",
    'Remove the group "M/S: Mid"; its channels stay',
  ]);
  expect((await strips()).slice(6, 9).map((s) => s.pan), "listed, not yet made").toEqual([32, 2, 62]);
  await box.getByTestId("mid-side-go").click();
  await expect(panel(page).getByTestId("mid-side-status")).toHaveText("M/S: Mid is removed: the pans, links and channels are as they were before it.");

  await expect.poll(async () => (await strips()).slice(6, 8).map((s) => s.pan)).toEqual([20, 44]);
  await expect.poll(async () => (await routing())[8]).toEqual([MUTE, 0]);
  await expect.poll(async () => (await workspace()).mixers[DEVICE]?.groups).toEqual([]);
  const saved = await workspace();
  expect(saved.mixers[DEVICE]?.channels.map((c) => [c.id, c.group])).toEqual([["m", undefined], ["s", undefined]]);
  expect(saved.links).toEqual([]);
  await expect(page.locator("ga-channel")).toHaveCount(2);
  await expect(page.getByTestId("mid-side-6")).toBeHidden();
  await expect(page.getByTestId("pre-phase-ch-7")).toHaveAttribute("aria-pressed", "false");
});
