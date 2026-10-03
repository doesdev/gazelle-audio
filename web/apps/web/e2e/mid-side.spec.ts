// Monitoring a mid and a side microphone as stereo, on the loopback without dry run so reads answer
// and writes land: the confirm lists every change and nothing reaches the device or the workspace
// until Confirm; the routes into two effect chains, the same effect in both (the second inverting),
// the returns and the side channel muted, the pans, link, group and side strips it makes; the Width
// moving both side strips; the warning when something drifts, with Put it back; the strips in the
// dock; removal putting everything back; and the refusal when no chains are free.
//
// The loopback reports every chain holding the same two effects whatever is written, and an
// effect's starting values whatever is set, so here the page's socket answers the Quadro's chain,
// instance and BAE-1084 reads the way a device would, keeping what `set_afx_order` and
// `set_neve_1084_conf` write (the writes still go on to the server). Every chain starts loaded, as on
// the loopback, and each test frees AFX In 3 and 4 on the Effects page first. Routing and the mixer
// are the loopback's own.

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
// Quadro topology positions: sources, the AFX In destination, and mix 1's input group.
const PREAMP = 0;
const AFX_OUT = 5;
const MUTE = 10;
const AFX_IN = 7;
const MIX_IN = 8;
/** The BAE-1084, the effect with a polarity switch tried first. */
const BAE = 25;
/** A BAE-1084 at its starting values, polarity apart. */
const FLAT = { gain: 0, high_freq: 0, high_gain: 50, peak_freq: 0, peak_gain: 50, low_freq: 0, low_gain: 50, high_pass: 0, low_pass: 0, hi_q: 0 };
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

async function routing(group: number): Promise<[number, number][]> {
  const reply = await command("get_routing", {}, group);
  return (reply["response"] as { bank_configs: { in_periph_id: number; in_chann: number }[] }).bank_configs.slice(0, 32).map((c) => [c.in_periph_id, c.in_chann]);
}

interface Saved {
  links: { kind: string; mode: string; members: { channel: number }[] }[];
  mixers: Record<string, { groups: { id: string; name: string; mid_side?: Record<string, unknown> }[]; channels: { id: string; name: string; slot: number; group?: string; source?: { group: number; channel: number } }[] }>;
}

async function workspace(): Promise<Saved> {
  return (await (await fetch(`${server.url}/api/v1/workspace`)).json()) as Saved;
}

/** A frame the page sends over the WebSocket, as far as the stub reads it. */
interface WsFrame {
  id?: number;
  device_id?: string;
  command?: string;
  ext3?: number;
  args?: Record<string, number | string>;
}

/** The emulated Quadro's chains and BAE-1084 settings by instance, as the stub keeps them. */
interface Effects {
  chains: Record<number, [number, number][]>;
  settings: Record<number, Record<string, number>>;
}

/**
 * Answers the Quadro's chain, chain link, free instance and BAE-1084 reads as a device would: every
 * chain starts with a Guitar Amp and a FET-A76 of its own, and what `set_afx_order` and
 * `set_neve_1084_conf` write is what is read back. Every frame still goes to the server too.
 */
async function emulateEffects(page: Page): Promise<Effects> {
  const state: Effects = { chains: Object.fromEntries(Array.from({ length: 6 }, (_, k) => [k, [[3, k], [9, k]]])), settings: {} };
  const used = (type: number) => Object.values(state.chains).flat().filter(([t]) => t === type).length;
  const replies: Record<string, (frame: WsFrame) => unknown> = {
    get_afx_strip_order: (frame) => ({ entries: [{ slots: Array.from({ length: 8 }, (_, i) => ({ type: state.chains[frame.ext3 ?? -1]?.[i]?.[0] ?? 0, inst: state.chains[frame.ext3 ?? -1]?.[i]?.[1] ?? 0 })) }] }),
    get_afx_links: () => ({ entries: Array.from({ length: 7 }, () => ({ linked: 0 })) }),
    get_afx_available_instances: () => ({ entries: [3, 9, 7, 24, 25].map((type_id) => ({ type_id, inst_count: 16 - used(type_id) })) }),
    get_afx_remaining_featured_instances: () => ({ entries: [3, 9, 7, 24, 25].map((type_id) => ({ type_id, inst_count: 16 })) }),
    get_neve_1084_conf: (frame) => ({ entries: [{ enabled: 1, ...FLAT, phase_inv: 0, ...state.settings[Number(frame.args?.["id"])] }] }),
  };
  await page.routeWebSocket(/\/ws$/, (socket) => {
    const upstream = socket.connectToServer();
    socket.onMessage((message) => {
      const frame: WsFrame = typeof message === "string" ? (JSON.parse(message) as WsFrame) : {};
      if (frame.device_id === DEVICE && frame.command === "set_afx_order" && typeof frame.args?.["slots"] === "string") {
        const hex = frame.args["slots"];
        const written: [number, number][] = [];
        for (let i = 0; i + 3 < hex.length; i += 4) {
          const type = Number.parseInt(hex.slice(i, i + 2), 16);
          if (type !== 0) written.push([type, Number.parseInt(hex.slice(i + 2, i + 4), 16)]);
        }
        state.chains[Number(frame.args["ch_id"])] = written;
      }
      if (frame.device_id === DEVICE && frame.command === "set_neve_1084_conf" && frame.args !== undefined) {
        const { type_id: _type, inst_id, ...values } = frame.args as Record<string, number>;
        state.settings[Number(inst_id)] = values;
      }
      const reply = frame.device_id === DEVICE ? replies[frame.command ?? ""] : undefined;
      if (reply === undefined) return upstream.send(message);
      socket.send(JSON.stringify({ type: "rpc_response", id: frame.id, result: { device_id: frame.device_id, command: frame.command, sent_hex: "74", sent_len: 16, dry_run: false, response: reply(frame), response_error: null } }));
    });
  });
  return state;
}

/** Frees AFX In 3 and 4 on the Effects page: both effects taken out of each. */
async function freeChains(page: Page, state: Effects): Promise<void> {
  await page.goto(`${server.url}/#/effects/${DEVICE}`);
  for (const chain of [2, 3]) {
    await expect(page.getByTestId(`slot-${chain}-1`)).toBeVisible();
    await page.getByTestId(`remove-${chain}-1`).click();
    await expect(page.getByTestId(`slot-${chain}-1`)).toBeHidden();
    await page.getByTestId(`remove-${chain}-0`).click();
    await expect(page.getByTestId(`chain-${chain}`)).toContainText("No effects");
  }
  await expect.poll(() => [state.chains[2], state.chains[3]]).toEqual([[], []]);
}

/** The Quadro mixer's `set_stereo_link` peripheral id (`mixers.stereoLinkId` in its topology). */
const MIXER_LINK = 3;

/**
 * A mid on Preamp 1 and a side on Preamp 2 in mix 1, panned a little apart, as someone might have
 * left them; the effect returns on slots 1 to 6, as the vendor keeps them; nothing into the chains;
 * and no strip pair linked on the device, in any mix: it keeps the flag a decode's side strips
 * (slots 9 and 10, a pair) set in an earlier test.
 */
test.beforeEach(async () => {
  for (let pair = 0; pair < 64; pair++) await command("set_stereo_link", { periph_id: MIXER_LINK, channel_id: pair, linked: 0 });
  await command("set_routing", { bank_idx: MIX_IN, bank_configs: Array.from({ length: 32 }, (_, slot) => (slot < 6 ? [AFX_OUT, slot] : slot === 6 ? [PREAMP, 0] : slot === 7 ? [PREAMP, 1] : [MUTE, 0])).flat() });
  await command("set_routing", { bank_idx: AFX_IN, bank_configs: Array.from({ length: 32 }, () => [MUTE, 0]).flat() });
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

/** Frees two chains, sets the decode up, and waits until the workspace holds it. Resolves to its group's id and the effects. */
async function setUp(page: Page): Promise<{ group: string; state: Effects }> {
  const state = await emulateEffects(page);
  await freeChains(page, state);
  await page.goto(`${server.url}/#/mixer/${DEVICE}/0`);
  await ask(page);
  await panel(page).getByTestId("mid-side-go").click();
  await expect(panel(page).getByTestId("mid-side-status")).toHaveText("Mid and Side are monitored as M/S in Tracking.");
  await expect.poll(async () => (await workspace()).mixers[DEVICE]?.groups[0]?.mid_side?.["inverted"] ?? "").not.toBe("");
  return { group: (await workspace()).mixers[DEVICE]?.groups[0]?.id ?? "", state };
}

test("with every chain loaded it says why it cannot, and what to free up, and offers nothing else", async ({ page }) => {
  await page.goto(`${server.url}/#/mixer/${DEVICE}/0`);
  await ask(page);
  const box = panel(page).getByTestId("mid-side-confirm");
  await expect(box.getByTestId("mid-side-refused")).toHaveText(
    "The side signal goes through two effect chains, one for each side strip, and none is free: the others hold an effect, are linked, are fed by another input or have a channel on their output. Free two first: on the Effects page take every effect out of a chain and unlink it, route Mute into it, and take any mixer channel off its AFX Out.",
  );
  await expect(box.getByTestId("mid-side-go")).toHaveCount(0);
  await expect(box.locator("select")).toHaveCount(0);
  await box.getByTestId("mid-side-cancel").click();
  expect((await workspace()).mixers[DEVICE]?.groups).toEqual([]);
});

test("Monitor as M/S lists every change and makes none until Confirm; then the chains, mutes, pans, links, group and side strips are there", async ({ page }) => {
  const state = await emulateEffects(page);
  await freeChains(page, state);
  await page.goto(`${server.url}/#/mixer/${DEVICE}/0`);
  await expect(page.getByTestId("mid-side-start"), "out of the way until two channels are selected").toBeHidden();
  await page.getByTestId("select-6").click();
  await expect(page.getByTestId("mid-side-start")).toBeHidden();
  await page.getByTestId("select-7").click({ modifiers: ["Control"] });
  await page.getByTestId("mid-side-start").click();

  const box = panel(page).getByTestId("mid-side-confirm");
  await expect(box).toBeVisible();
  await expect(box.getByTestId("mid-side-roles")).toHaveText("Mid: Mid. Side: Side.");
  await expect(box.locator("select"), "there is nothing to choose").toHaveCount(0);
  await expect(box.getByTestId("mid-side-plan").locator("li")).toHaveText([
    "Route Preamp 2 into AFX In 3 as well, in place of Mute",
    "Route Preamp 2 into AFX In 4 as well, in place of Mute",
    "Add BAE-1084 to AFX In 3, which is empty, with every setting at its starting value",
    "Add BAE-1084 to AFX In 4, which is empty, with the same settings and its polarity switch on",
    "Mute the effect return AFX Out 3 on slot 3 of Tracking, which would play the side signal a second time",
    "Mute the effect return AFX Out 4 on slot 4 of Tracking, which would play the inverted copy a second time",
    "Pan Mid to the centre in Tracking (it is at L 40%)",
    "Add a channel, Side L, on AFX Out 3 to Tracking: the side signal, panned hard left at Side's level, 0 dB",
    `Add a channel, Side ${O}, on AFX Out 4 to Tracking: the inverted copy, panned hard right at the same level`,
    `Mute Side in Tracking while the decode plays there: Side L carries the side signal instead, with the same delay as Side ${O}`,
    `Link Side L and Side ${O}, so their levels, mutes and solos always match`,
    'Group the four as "M/S: Mid"',
  ]);
  // What was measured and how to check by ear, and what is recorded, are said in so many words.
  await expect(box.getByTestId("mid-side-notes")).toContainText("Measured on a Quadro at 96 kHz, at a modest level: BAE-1084 at its starting values delays by 4 samples (42 microseconds) and leaves the level as it is");
  await expect(box.getByTestId("mid-side-notes")).toContainText("mute the mid and switch the mix to mono: the side strips should cancel to near silence");
  await expect(box.getByTestId("mid-side-notes")).toContainText("What is recorded does not change: Preamp 1 and Preamp 2 reach your DAW raw");

  // Swap makes the other channel the mid, and the list follows; swapped back, it is as it was.
  await box.getByTestId("mid-side-swap").click();
  await expect(box.getByTestId("mid-side-roles")).toHaveText("Mid: Side. Side: Mid.");
  await expect(box.getByTestId("mid-side-plan").locator("li").first()).toHaveText("Route Preamp 1 into AFX In 3 as well, in place of Mute");
  await expect(box.getByTestId("mid-side-plan").locator("li").last()).toHaveText('Group the four as "M/S: Side"');
  await box.getByTestId("mid-side-swap").click();
  await shot(page, "mid-side-1-confirm");

  // Nothing has been sent or saved: the device, the chains and the workspace are as they were.
  const before = await strips();
  expect([before[2]?.mute, before[3]?.mute, before[6]?.pan, before[7]?.pan, before[7]?.mute, before[8]?.pan]).toEqual([0, 0, 20, 44, 0, 32]);
  expect((await routing(AFX_IN)).slice(2, 4)).toEqual([[MUTE, 0], [MUTE, 0]]);
  expect([state.chains[2], state.chains[3]]).toEqual([[], []]);
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

  // On the device: the side input into both chains, the same effect in each, flat, the second
  // inverting; both returns and the side muted; the mid centred and the side strips hard left and right.
  await expect.poll(async () => (await routing(AFX_IN)).slice(2, 4)).toEqual([[PREAMP, 1], [PREAMP, 1]]);
  await expect.poll(() => [state.chains[2], state.chains[3]]).toEqual([[[BAE, 0]], [[BAE, 1]]]);
  await expect.poll(() => [state.settings[0], state.settings[1]]).toEqual([{ ...FLAT, phase_inv: 0 }, { ...FLAT, phase_inv: 1 }]);
  await expect.poll(async () => (await strips()).map((s) => s.mute).slice(0, 10)).toEqual([0, 0, 1, 1, 0, 0, 0, 1, 0, 0]);
  await expect.poll(async () => (await strips()).slice(6, 10).map((s) => [s.pan, s.level])).toEqual([[32, 0], [44, 0], [2, 0], [62, 0]]);
  await expect.poll(async () => (await routing(MIX_IN)).slice(8, 10)).toEqual([[AFX_OUT, 2], [AFX_OUT, 3]]);
  // In the workspace: the group with its decode, the four channels in it, and the side strips' link.
  await expect.poll(async () => (await workspace()).mixers[DEVICE]?.groups.map((g) => g.name)).toEqual(["M/S: Mid"]);
  const saved = await workspace();
  const mixer = saved.mixers[DEVICE];
  const group = mixer?.groups[0];
  expect(mixer?.channels.map((c) => [c.name, c.slot, c.group, c.source])).toEqual([
    ["Mid", 6, group?.id, { group: PREAMP, channel: 0 }],
    ["Side", 7, group?.id, { group: PREAMP, channel: 1 }],
    ["Side L", 8, group?.id, { group: AFX_OUT, channel: 2 }],
    [`Side ${O}`, 9, group?.id, { group: AFX_OUT, channel: 3 }],
  ]);
  expect(group?.mid_side).toMatchObject({
    mid: "m",
    side: "s",
    left: mixer?.channels[2]?.id,
    inverted: mixer?.channels[3]?.id,
    via: "effect",
    effect_type: BAE,
    left_chain: 2,
    left_effect_inst: 0,
    chain: 3,
    effect_inst: 1,
    pans: { "0": { mid: 20, side: 44 } },
    left_returns_muted: [0],
    returns_muted: [0],
    side_muted: [0],
  });
  expect(saved.links.map((l) => [l.kind, l.mode, l.members.map((m) => m.channel)])).toEqual([["mixer", "absolute", [8, 9]]]);

  await shot(page, "mid-side-2-decoded");
  // On the page: the four strips marked, and the decode's line saying what is recorded.
  await expect(page.locator("ga-channel-group ga-channel")).toHaveCount(4);
  for (const [slot, mark] of [[6, "M"], [7, "S dry"], [8, "S"], [9, "-S"]] as const) await expect(page.getByTestId(`mid-side-${slot}`)).toHaveText(mark);
  await expect(page.getByTestId("mid-side-6")).not.toHaveAttribute("data-warning", "");
  const row = panel(page).getByTestId(`mid-side-row-${group?.id}`);
  await expect(row).not.toHaveAttribute("data-warning", "");
  await expect(row.getByTestId(`mid-side-recorded-${group?.id}`)).toHaveText("Recorded raw: Preamp 1 and Preamp 2. The two side strips are effect chains' outputs and are not recorded.");

  // The Effects page shows what was put in the chains.
  await page.locator('ga-header a[data-page="effects"]').click();
  await expect(page.getByTestId("slot-2-0")).toContainText("BAE-1084");
  await expect(page.getByTestId("slot-3-0")).toContainText("BAE-1084");
});

test("the Width moves both side strips and leaves the mid; a side strip's fader moves the other too", async ({ page }) => {
  const { group } = await setUp(page);
  const bar = panel(page).getByTestId(`mid-side-width-${group}`);
  await expect(bar).toHaveAttribute("aria-valuetext", "0 dB");
  await bar.focus();
  for (let i = 0; i < 4; i++) await page.keyboard.press("ArrowLeft");
  await expect(bar).toHaveAttribute("aria-valuetext", "-4 dB");
  await expect.poll(async () => (await strips()).slice(6, 10).map((s) => s.level)).toEqual([0, 0, 4, 4]);

  // The hard-left strip's own fader, one step quieter: the link takes the copy with it, and the Width follows.
  await page.getByTestId("fader-8").focus();
  await page.keyboard.press("ArrowDown");
  await expect.poll(async () => (await strips()).slice(6, 10).map((s) => s.level)).toEqual([0, 0, 5, 5]);
  await expect(bar).toHaveAttribute("aria-valuetext", "-5 dB");
  // Mute on the copy mutes the hard-left strip as well; the side channel stays muted.
  await page.locator('ga-strip[strip="9"]').getByRole("button", { name: /mute$/ }).click();
  await expect.poll(async () => (await strips()).slice(6, 10).map((s) => s.mute)).toEqual([0, 1, 1, 1]);
  await expect(panel(page).getByTestId(`mid-side-row-${group}`)).not.toHaveAttribute("data-warning", "");
});

test("a side strip that drifts, a pan moved, a return unmuted and the two effects set apart are warned about, and Put it back mends them after a confirm", async ({ page }) => {
  const { group, state } = await setUp(page);
  // Changed behind the app's back, as the vendor's panel would: the copy 9 dB down, the hard-left
  // strip off its pan, the copy's effect return playing, and the copy's equaliser with more gain.
  await strip(9, { level: 9, pan: 62 });
  await strip(8, { level: 0, pan: 30 });
  await strip(3, {});
  state.settings[1] = { ...FLAT, gain: 3, phase_inv: 1 };
  await page.reload();

  const row = panel(page).getByTestId(`mid-side-row-${group}`);
  await expect(row).toHaveAttribute("data-warning", "");
  await expect(row.getByTestId(`mid-side-problems-${group}`).locator("li")).toHaveText([
    "BAE-1084 in AFX In 4 is not set like the one in AFX In 3 (Gain), so the two side strips do not match.",
    "The effect return AFX Out 4 also plays in Tracking, so the inverted copy is heard twice.",
    "Side L is panned L 7% in Tracking, not hard left, so left and right no longer decode.",
    `Side L is at 0 dB and Side ${O} at -9 dB in Tracking: the two must match.`,
  ]);
  await shot(page, "mid-side-3-warning");
  // The strips and the group's band say so too.
  await expect(page.getByTestId("mid-side-9")).toHaveAttribute("data-warning", "");
  await expect(page.getByTestId("mid-side-9")).toHaveAttribute("title", /the two must match/);
  await expect(page.getByTestId(`group-warning-${group}`)).toBeVisible();

  await row.getByTestId(`mid-side-repair-${group}`).click();
  const box = panel(page).getByTestId("mid-side-confirm");
  await expect(box.getByTestId("mid-side-plan").locator("li")).toHaveText([
    "Set BAE-1084 in AFX In 4 like the one in AFX In 3, polarity apart",
    "Mute the effect return AFX Out 4 in Tracking",
    "Pan Side L hard left in Tracking",
    `Set Side ${O} to 0 dB in Tracking`,
  ]);
  expect((await strips()).slice(8, 10).map((s) => [s.pan, s.level]), "listed, not yet made").toEqual([[30, 0], [62, 9]]);
  expect(state.settings[1]?.["gain"]).toBe(3);
  await box.getByTestId("mid-side-go").click();
  await expect(panel(page).getByTestId("mid-side-status")).toHaveText("M/S: Mid is put back.");
  await expect.poll(async () => (await strips()).slice(8, 10).map((s) => [s.pan, s.level])).toEqual([[2, 0], [62, 0]]);
  await expect.poll(async () => (await strips())[3]?.mute).toBe(1);
  await expect.poll(() => state.settings[1]).toEqual({ ...FLAT, phase_inv: 1 });
  await expect(row).not.toHaveAttribute("data-warning", "");
  await expect(page.getByTestId("mid-side-9")).not.toHaveAttribute("data-warning", "");
  await expect(page.getByTestId(`group-warning-${group}`)).toBeHidden();
});

test("in the dock the four strips are marked and the side strips still move together", async ({ page }) => {
  await setUp(page);
  await page.goto(`${server.url}/#/inputs/${DEVICE}`);
  const dock = page.locator("ga-mixer-dock");
  await expect(dock.locator("ga-strip[compact]")).toHaveCount(5);
  await expect(dock.locator('ga-strip[strip="9"]')).toHaveAttribute("label", `Side ${O}`);
  for (const [slot, mark] of [[6, "M"], [7, "S dry"], [8, "S"], [9, "-S"]] as const) await expect(dock.getByTestId(`mid-side-${slot}`)).toHaveText(mark);
  await dock.getByTestId("fader-9").focus();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  await expect.poll(async () => (await strips()).slice(6, 10).map((s) => s.level)).toEqual([0, 0, 2, 2]);
  await expect(dock.getByTestId("mid-side-8")).not.toHaveAttribute("data-warning", "");

  // A copy that has drifted shows on the dock's strips as well.
  await strip(9, { level: 12, pan: 62 });
  await page.reload();
  await expect(page.locator("ga-mixer-dock").getByTestId("mid-side-9")).toHaveAttribute("data-warning", "");
});

test("removing the decode lists what it puts back, and puts it back only on Confirm; the group's own remove opens the same list", async ({ page }) => {
  const { group, state } = await setUp(page);
  // The band's remove does not drop the group alone: it opens the decode's removal.
  await page.getByRole("button", { name: "Remove group M/S: Mid" }).click();
  const box = panel(page).getByTestId("mid-side-confirm");
  await expect(box).toContainText("Remove M/S: Mid?");
  await box.getByTestId("mid-side-cancel").click();
  expect((await workspace()).mixers[DEVICE]?.groups.length, "cancelled, the group is still there").toBe(1);

  await panel(page).getByTestId(`mid-side-remove-${group}`).click();
  await expect(box.getByTestId("mid-side-plan").locator("li")).toHaveText([
    "Pan Mid back to L 40% in Tracking",
    `Unlink Side L and Side ${O}`,
    "Remove the channel Side L, the side signal through its effect chain",
    `Remove the channel Side ${O}, the inverted copy`,
    "Unmute Side in Tracking",
    "Remove BAE-1084 from AFX In 3",
    "Route Mute into AFX In 3 again",
    "Unmute the effect return AFX Out 3 in Tracking",
    "Remove BAE-1084 from AFX In 4",
    "Route Mute into AFX In 4 again",
    "Unmute the effect return AFX Out 4 in Tracking",
    'Remove the group "M/S: Mid"; its channels stay',
  ]);
  expect((await strips()).slice(6, 10).map((s) => [s.pan, s.mute]), "listed, not yet made").toEqual([[32, 0], [44, 1], [2, 0], [62, 0]]);
  await box.getByTestId("mid-side-go").click();
  await expect(panel(page).getByTestId("mid-side-status")).toHaveText("M/S: Mid is removed: the pans, links and channels are as they were before it.");

  await expect.poll(async () => (await strips()).slice(6, 8).map((s) => [s.pan, s.mute])).toEqual([[20, 0], [44, 0]]);
  await expect.poll(async () => (await strips()).slice(2, 4).map((s) => s.mute)).toEqual([0, 0]);
  await expect.poll(async () => (await routing(AFX_IN)).slice(2, 4)).toEqual([[MUTE, 0], [MUTE, 0]]);
  await expect.poll(() => [state.chains[2], state.chains[3]]).toEqual([[], []]);
  await expect.poll(async () => (await routing(MIX_IN)).slice(8, 10)).toEqual([[MUTE, 0], [MUTE, 0]]);
  await expect.poll(async () => (await workspace()).mixers[DEVICE]?.groups).toEqual([]);
  const saved = await workspace();
  expect(saved.mixers[DEVICE]?.channels.map((c) => [c.id, c.group])).toEqual([["m", undefined], ["s", undefined]]);
  expect(saved.links).toEqual([]);
  await expect(page.locator("ga-channel")).toHaveCount(2);
  await expect(page.getByTestId("mid-side-6")).toBeHidden();
});
