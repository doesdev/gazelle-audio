// What a mix plays outside its channels, on the loopback without dry run so reads answer and writes
// land. The loopback is seeded with the owner's Quadro as read on 2026-09-30: Mix 1's effect returns
// AFX OUT 5 and 6 soloed (muted, at -2 dB), which silence the rest of Mix 1; and HP2's Mix 4 playing
// USB 1 PLAY 3 and 4 at unity on slots 7 and 8, where the layout's channels are Mix 1's preamps.
// The markers, the notice, the tidy's confirm and what the device holds afterwards are checked,
// and so are the effect return strips.

import { expect, test, type Page } from "@playwright/test";
import { join } from "node:path";

import { startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { putWorkspace } from "./workspace.ts";

let server: RunningServer;

/** Where screenshots for the owner go, when asked for: `GAZELLE_SHOTS=<dir>`. */
const SHOTS = process.env["GAZELLE_SHOTS"];

test.beforeAll(async () => {
  server = await startServer(["--backend", "loopback", "--loopback-cyclic-ms", "200"], { webUi: true });
});

test.afterAll(async () => {
  await server?.stop();
});

// Quadro topology positions: sources, and the mixes' input groups.
const PREAMP = 0;
const USB1 = 1;
const AFX_OUT = 5;
const MUTE = 10;
const MIX_IN = [8, 9, 10, 11] as const;
const FLOOR = 90;

async function command(name: string, body: Record<string, unknown>, ext3?: number): Promise<Record<string, unknown>> {
  const response = await fetch(`${server.url}/api/v1/devices/loopback-0/command/${name}${ext3 === undefined ? "" : `?ext3=${ext3}`}`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
  const text = await response.text();
  expect(response.ok, text).toBe(true);
  return JSON.parse(text) as Record<string, unknown>;
}

/** One mix's 32 input slots: MUTE except where given. */
function route(mix: number, slots: Record<number, [number, number]>): Promise<unknown> {
  return command("set_routing", { bank_idx: MIX_IN[mix], bank_configs: Array.from({ length: 32 }, (_, at) => slots[at] ?? [MUTE, 0]).flat() });
}

function strip(mix: number, slot: number, values: { level?: number; mute?: boolean; solo?: boolean }): Promise<unknown> {
  return command("set_mixer", { mixer_id: mix, channel: slot + 1, level: values.level ?? 0, pan: 32, mute: values.mute ? 1 : 0, solo: values.solo ? 1 : 0 });
}

async function readRouting(mix: number): Promise<[number, number][]> {
  const reply = await command("get_routing", {}, MIX_IN[mix]);
  const configs = (reply["response"] as { bank_configs: { in_periph_id: number; in_chann: number }[] }).bank_configs;
  return configs.slice(0, 32).map((c) => [c.in_periph_id, c.in_chann]);
}

async function readStrips(mix: number): Promise<{ level: number; mute: number; solo: number }[]> {
  const reply = await command("get_mixer", {}, mix);
  return (reply["response"] as { entries: { level: number; mute: number; solo: number }[] }).entries.slice(1);
}

const AFX: Record<number, [number, number]> = Object.fromEntries(Array.from({ length: 6 }, (_, k) => [k, [AFX_OUT, k] as [number, number]]));

/** The owner's Quadro and its layout. */
async function seedOwnersQuadro(): Promise<void> {
  await route(0, { ...AFX, 6: [PREAMP, 0], 7: [PREAMP, 1] });
  await route(1, AFX);
  await route(2, AFX);
  await route(3, { ...AFX, 6: [USB1, 2], 7: [USB1, 3], 8: [USB1, 0] });
  for (let mix = 0; mix < 4; mix++) for (let slot = 0; slot < 32; slot++) await strip(mix, slot, {});
  // Mix 1: AFX OUT 1 to 4 at the floor, 5 and 6 soloed and muted at -2 dB.
  for (const slot of [0, 1, 2, 3]) await strip(0, slot, { level: FLOOR });
  for (const slot of [4, 5]) await strip(0, slot, { level: 2, mute: true, solo: true });
  // Mix 2: its effect returns play, 1 to 4 at 0 dB and 5 and 6 at -10 dB.
  for (const slot of [4, 5]) await strip(1, slot, { level: 10 });
  // Mix 3 is clean. Mix 4: returns 1 to 4 at the floor, 5 and 6 at unity.
  for (const slot of [0, 1, 2, 3, 4, 5]) await strip(2, slot, { level: FLOOR });
  for (const slot of [0, 1, 2, 3]) await strip(3, slot, { level: FLOOR });
  await putWorkspace(server, {
    mixers: {
      "loopback-0": {
        mixes: [{ name: "HP Amp Tracking" }, {}, {}, { name: "HP2 Tracking" }],
        groups: [],
        channels: [
          { id: "a", name: "", slot: 6, source: { group: PREAMP, channel: 0 }, main_mix: 0, sends: [] },
          { id: "b", name: "", slot: 7, source: { group: PREAMP, channel: 1 }, main_mix: 0, sends: [] },
          { id: "c", name: "", slot: 8, source: { group: USB1, channel: 0 }, main_mix: 3, sends: [] },
        ],
      },
    },
  });
}

// Each test has a browser of its own, so the effect returns' choice starts unmade: shown while they play.
test.beforeEach(() => seedOwnersQuadro());

async function pickMix(page: Page, mix: number): Promise<void> {
  await page.getByTestId(`mix-${mix}`).click();
  await expect(page.getByTestId(`mix-${mix}`)).toHaveAttribute("aria-checked", "true");
}

const notice = (page: Page) => page.locator("ga-mix-notice");
const shot = async (page: Page, name: string, target?: ReturnType<Page["locator"]>) => {
  if (SHOTS === undefined) return;
  await (target ?? page).screenshot({ path: join(SHOTS, `${name}.png`) });
};

test("the mix buttons mark the mixes with hidden solos and leftover routes, and the notice lists them", async ({ page }) => {
  await page.goto(`${server.url}/#/mixer/loopback-0/0`);
  await expect(page.getByTestId("mix-0")).toHaveAttribute("data-warning", "");
  await expect(page.getByTestId("mix-0")).toHaveAttribute("title", "Show HP Amp Tracking: 2 soloed strips");
  await expect(page.getByTestId("mix-3")).toHaveAttribute("data-warning", "");
  await expect(page.getByTestId("mix-3")).toHaveAttribute("title", "Show HP2 Tracking: 2 strips play outside this mix's channels");
  await expect(page.getByTestId("mix-1"), "effect returns alone are not warned about").not.toHaveAttribute("data-warning", "");
  await expect(page.getByTestId("mix-2")).not.toHaveAttribute("data-warning", "");

  await expect(notice(page).getByTestId("mix-notice-items").locator("li")).toHaveText([
    "AFX OUT 5 (slot 5) is soloed, which silences every other channel in this mix. It is an effect return strip.",
    "AFX OUT 6 (slot 6) is soloed, which silences every other channel in this mix. It is an effect return strip.",
  ]);
  // A soloed return shows the effect return strips by itself.
  await expect(page.getByTestId("effect-returns-toggle")).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator("ga-effect-returns ga-strip")).toHaveCount(6);
  await expect(page.locator('ga-effect-returns ga-strip[strip="4"]')).toHaveAttribute("label", "AFX OUT 5");
  await shot(page, "mix-health-1-mix1-notice-and-markers");

  await pickMix(page, 3);
  await expect(notice(page).getByTestId("mix-notice-items").locator("li")).toHaveText([
    "Slot 7: USB 1 PLAY 3 plays at 0 dB but is not one of this mix's channels.",
    "Slot 8: USB 1 PLAY 4 plays at 0 dB but is not one of this mix's channels.",
  ]);
  await expect(notice(page).getByTestId("mix-notice-returns")).toHaveText(/^Effect returns playing: AFX OUT 5 at 0 dB, AFX OUT 6 at 0 dB\./);
  await shot(page, "mix-health-2-mix4-notice");

  await pickMix(page, 1);
  await expect(notice(page).getByTestId("mix-notice-items")).toHaveCount(0);
  await expect(notice(page).getByTestId("mix-tidy"), "nothing to tidy").toHaveCount(0);
  await expect(notice(page).getByTestId("mix-notice-returns")).toHaveText(/AFX OUT 1 at 0 dB, AFX OUT 2 at 0 dB, AFX OUT 3 at 0 dB, AFX OUT 4 at 0 dB, AFX OUT 5 at -10 dB, AFX OUT 6 at -10 dB/);

  await pickMix(page, 2);
  await expect(notice(page), "a clean mix has no notice").toBeHidden();
  await expect(page.locator("ga-effect-returns ga-strip"), "nor effect return strips").toHaveCount(0);
});

test("tidying Mix 4 mutes only the two leftover slots, after a confirm listing them, and leaves its effect returns alone", async ({ page }) => {
  await page.goto(`${server.url}/#/mixer/loopback-0/3`);
  await expect(page.getByTestId("mix-3")).toHaveAttribute("aria-checked", "true");
  await notice(page).getByTestId("mix-tidy").click();
  await expect(notice(page).getByTestId("mix-tidy-plan").locator("li")).toHaveText([
    "Slot 7: route MUTE in place of USB 1 PLAY 3, which is not one of this mix's channels",
    "Slot 8: route MUTE in place of USB 1 PLAY 4, which is not one of this mix's channels",
  ]);
  await expect(notice(page).getByTestId("mix-tidy-returns")).not.toBeChecked();
  await shot(page, "mix-health-3-mix4-confirm", notice(page));
  // Nothing has been written yet.
  expect((await readRouting(3))[6]).toEqual([USB1, 2]);

  await notice(page).getByTestId("mix-tidy-confirm").click();
  await expect(notice(page).getByTestId("mix-tidied")).toHaveText("Tidied: 2 changes.");
  await expect(page.getByTestId("mix-3")).not.toHaveAttribute("data-warning", "");
  await expect(notice(page).getByTestId("mix-notice-items")).toHaveCount(0);

  const routed = await readRouting(3);
  expect(routed.slice(0, 9), "slots 7 and 8 MUTE; the returns and the layout's channel kept").toEqual([...Array.from({ length: 6 }, (_, k) => [AFX_OUT, k]), [MUTE, 0], [MUTE, 0], [USB1, 0]]);
  const strips = await readStrips(3);
  expect(strips.slice(4, 6).map((s) => [s.level, s.mute, s.solo]), "the effect returns untouched").toEqual([
    [0, 0, 0],
    [0, 0, 0],
  ]);
  await shot(page, "mix-health-4-mix4-tidied");
});

test("tidying Mix 1 clears both hidden solos, and with the box ticked a tidy also mutes the effect returns", async ({ page }) => {
  await page.goto(`${server.url}/#/mixer/loopback-0/0`);
  await notice(page).getByTestId("mix-tidy").click();
  await expect(notice(page).getByTestId("mix-tidy-plan").locator("li")).toHaveText(["AFX OUT 5 (slot 5): clear its solo", "AFX OUT 6 (slot 6): clear its solo"]);
  await notice(page).getByTestId("mix-tidy-confirm").click();
  await expect(notice(page).getByTestId("mix-tidied")).toHaveText("Tidied: 2 changes.");
  const mix1 = await readStrips(0);
  expect(mix1.slice(4, 6).map((s) => [s.level, s.mute, s.solo]), "solo cleared, level and mute kept").toEqual([
    [2, 1, 0],
    [2, 1, 0],
  ]);
  await expect(page.getByTestId("mix-0")).not.toHaveAttribute("data-warning", "");

  await pickMix(page, 3);
  await expect(notice(page).getByTestId("mix-tidied"), "an outcome belongs to its mix").toHaveCount(0);
  await notice(page).getByTestId("mix-tidy").click();
  await notice(page).getByTestId("mix-tidy-returns").check();
  await expect(notice(page).getByTestId("mix-tidy-plan").locator("li")).toHaveCount(4);
  await expect(notice(page).getByTestId("mix-tidy-plan").locator("li").nth(3)).toHaveText("AFX OUT 6 (slot 6): mute this effect return");
  await notice(page).getByTestId("mix-tidy-confirm").click();
  await expect(notice(page).getByTestId("mix-tidied")).toHaveText("Tidied: 4 changes.");
  expect((await readStrips(3)).slice(4, 6).map((s) => [s.level, s.mute])).toEqual([
    [0, 1],
    [0, 1],
  ]);
  await expect(notice(page), "nothing left to say but the outcome").toBeVisible();
});

test("Cancel closes the confirm and changes nothing", async ({ page }) => {
  await page.goto(`${server.url}/#/mixer/loopback-0/3`);
  await notice(page).getByTestId("mix-tidy").click();
  await notice(page).getByTestId("mix-tidy-cancel").click();
  await expect(notice(page).getByTestId("mix-tidy-plan")).toHaveCount(0);
  await page.waitForTimeout(300);
  expect((await readRouting(3))[6]).toEqual([USB1, 2]);
});

test("the effect return strips show while one plays, hide and show by hand, remember it, and send set_mixer like any strip", async ({ page }) => {
  await page.goto(`${server.url}/#/mixer/loopback-0/3`);
  const toggle = page.getByTestId("effect-returns-toggle");
  const returns = page.locator("ga-effect-returns ga-strip");
  await expect(toggle).toHaveAttribute("aria-pressed", "true");
  await expect(returns).toHaveCount(6);
  await shot(page, "mix-health-5-effect-returns", page.locator("ga-mixer"));

  // Muting AFX OUT 5 in Mix 4 is the strip's own set_mixer.
  await page.locator('ga-effect-returns ga-strip[strip="4"]').getByRole("button", { name: "AFX OUT 5 mute" }).click();
  await expect.poll(async () => (await readStrips(3))[4]?.mute).toBe(1);

  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-pressed", "false");
  await expect(returns).toHaveCount(0);
  await page.reload();
  await expect(page.getByTestId("mix-3")).toHaveAttribute("aria-checked", "true");
  await expect(page.getByTestId("effect-returns-toggle"), "hidden is remembered").toHaveAttribute("aria-pressed", "false");
  await expect(returns).toHaveCount(0);
  await page.getByTestId("effect-returns-toggle").click();
  await expect(returns).toHaveCount(6);

  // A clean mix hides them unless asked: with the choice made, they stay.
  await pickMix(page, 2);
  await expect(returns, "shown by hand, so shown in every mix").toHaveCount(6);
});
