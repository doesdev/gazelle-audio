// The effect parameter editor on the Effects page (specs/2026-09-17-effects-and-reverb.md, "Effect
// parameters"). The server runs in dry run, which answers no reads, so the device's replies are supplied
// through the WebSocket as effects.spec.ts does; every write still goes to the server, and what the page
// would send is compared with the bytes the server gives for the same command and arguments. Where one
// change sends two writes (a linked partner), their replies can return in either order, so the order is
// checked on the frames the page sends.

import { expect, test, type Page } from "@playwright/test";

import { startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { resetWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--dry-run"], { webUi: true });
});

test.beforeEach(() => resetWorkspace(server));

test.afterAll(async () => {
  await server?.stop();
});

const slots = (...effects: [number, number][]) => Array.from({ length: 8 }, (_, i) => ({ type: effects[i]?.[0] ?? 0, inst: effects[i]?.[1] ?? 0 }));
/** AFX IN 1 and 2 linked, each a PowerGate then a PowerFFC; AFX IN 3 a FET-A76 and a Brainiac; AFX IN 4 a ClearQ. */
const QUADRO_CHAINS: Record<number, [number, number][]> = { 0: [[39, 2], [2, 0]], 1: [[39, 3], [2, 1]], 2: [[9, 0], [85, 1]], 3: [[1, 0]] };

type Frame = { id?: number; device_id?: string; command?: string; ext3?: number; args?: Record<string, number> };

const gate = { enabled: 1, threshold: 60, range: 4, attack: 250, decay: 80, hold: 1200, gain: 244 };

/** Answers loopback-0's (Quadro) and loopback-1's (Studio+) reads as a device would; returns every frame the page sends. */
async function answerReads(page: Page): Promise<Frame[]> {
  const sent: Frame[] = [];
  const replies: Record<string, (frame: Frame) => unknown> = {
    "loopback-0|get_afx_strip_order": (frame) => ({ entries: [{ slots: slots(...(QUADRO_CHAINS[frame.ext3 ?? -1] ?? [])) }] }),
    "loopback-0|get_afx_links": () => ({ entries: [1, 0, 0, 0, 0, 0, 0].map((linked) => ({ linked })) }),
    "loopback-0|get_reverb_config": () => ({ mixer_id: 0, room_size: 0, color: 0, predelay: 0, density: 100, early_ref_gain: 0, late_ref_delay: 0, richness: 0, reverb_time: 0, reverb_level: 25, on: 1 }),
    "loopback-0|get_reverb_returns": () => ({ entries: Array.from({ length: 4 }, () => ({ level: 0, mute: 0 })) }),
    "loopback-0|get_reverb_sends": () => ({ entries: Array.from({ length: 33 }, () => ({ level: 96, pan: 32, mute: 0, solo: 0 })) }),
    "loopback-0|get_powergate_conf": (frame) => ({ entries: [{ ...gate, enabled: frame.args?.["id"] === 3 ? 0 : 1 }] }),
    "loopback-0|get_compressor_configs": () => ({ entries: [{ enabled: 1, attack: 12500, release: 10000, taw: 65535, ratio: 400, gain: 150, ctrl: 0, threshold: 24, knee: 0, linked: 0 }] }),
    "loopback-0|get_uad_1176_conf": () => ({ entries: [{ enabled: 1, input: 35, output: 46, attack: 78, release: 21, ratio: 1 }] }),
    "loopback-0|get_Brainiac_conf": () => ({ entries: [{ enabled: 1, release: 0, attack: 0, range: 0, ratio: 8, thresh: 0, linlog: 0, mode: 0, sideSource: 7, sideChanN: 3 }] }),
    "loopback-1|get_afx_order": () => ({ entries: Array.from({ length: 16 }, (_, i) => ({ slots: i === 0 ? slots([39, 7]) : slots() })) }),
    "loopback-1|get_afx_links": () => ({ entries: Array.from({ length: 8 }, () => ({ linked: 0 })) }),
    "loopback-1|get_reverb_config": () => ({ mixer_id: 0, room_size: 0, color: 0, predelay: 0, density: 100, early_ref_gain: 0, late_ref_delay: 0, richness: 0, reverb_time: 0, reverb_level: 25, on: 1 }),
    "loopback-1|get_powergate_configs": () => ({ entries: Array.from({ length: 16 }, (_, i) => ({ ...gate, threshold: 100 + i, gain: -6, linked: 0 })) }),
  };
  await page.routeWebSocket(/\/ws$/, (socket) => {
    const upstream = socket.connectToServer();
    socket.onMessage((message) => {
      const frame: Frame = typeof message === "string" ? (JSON.parse(message) as Frame) : {};
      sent.push(frame);
      const reply = replies[`${frame.device_id}|${frame.command}`];
      if (reply === undefined) return upstream.send(message);
      socket.send(JSON.stringify({ type: "rpc_response", id: frame.id, result: { device_id: frame.device_id, command: frame.command, sent_hex: "74", sent_len: 16, dry_run: false, response: reply(frame), response_error: null } }));
    });
  });
  return sent;
}

/** What the server says it would send for a command. */
async function wouldSend(deviceId: string, command: string, args: Record<string, unknown>): Promise<string> {
  const response = await fetch(`${server.url}/api/v1/devices/${deviceId}/command/${encodeURIComponent(command)}?dry_run=true`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(args) });
  const body = (await response.json()) as { sent_hex: string };
  return `Dry run, would send ${command}: ${body.sent_hex}`;
}

const lastSent = (page: Page) => page.getByTestId("last-sent");
const of = (sent: Frame[], command: string) => () => sent.filter((f) => f.command === command).map((f) => f.args);

test("choosing an effect opens its editor, which reads that instance once and shows a control per parameter", async ({ page }) => {
  const sent = await answerReads(page);
  await page.goto(`${server.url}/#/effects/loopback-0`);
  const editor = page.getByTestId("effect-editor");
  await expect(editor).toHaveCount(0);

  await page.getByTestId("edit-0-0").click();
  await expect(editor).toBeVisible();
  await expect(editor.getByRole("heading")).toContainText("PowerGate #3");
  await expect(page.getByTestId("param-threshold")).toHaveAttribute("aria-valuetext", "60");
  await expect(page.getByTestId("param-attack")).toHaveAttribute("aria-valuetext", "25.0", { timeout: 1000 });
  await expect(page.getByTestId("param-gain")).toHaveAttribute("aria-valuetext", "-12");
  await expect(editor.locator("[data-testid^='param-']")).toHaveCount(6);
  await expect.poll(of(sent, "get_powergate_conf")).toEqual([{ id: 2 }]);

  await page.getByTestId("editor-close").click();
  await expect(editor).toHaveCount(0);
  await page.getByTestId("edit-0-0").click();
  await expect(page.getByTestId("param-threshold")).toHaveAttribute("aria-valuetext", "60");
  expect(of(sent, "get_powergate_conf")(), "read once").toHaveLength(1);

  // The linked partner's gate reads as bypassed (enabled 0), which the chain shows too.
  await page.getByTestId("edit-1-0").click();
  await expect(editor.getByRole("heading")).toContainText("PowerGate #4");
  await expect(page.getByTestId("editor-bypass")).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("bypass-1-0")).toHaveAttribute("aria-pressed", "true");
});

test("a change sends every parameter with the type and instance, as the server would; the linked partner follows; double-click resets", async ({ page }) => {
  const sent = await answerReads(page);
  await page.goto(`${server.url}/#/effects/loopback-0`);
  await page.getByTestId("edit-0-0").click();
  const threshold = page.getByTestId("param-threshold");
  await expect(threshold).toHaveAttribute("aria-valuetext", "60");

  await threshold.focus();
  await threshold.press("ArrowUp");
  await expect(threshold).toHaveAttribute("aria-valuetext", "61");
  const own = { type_id: 39, inst_id: 2, threshold: 61, range: 4, attack: 250, decay: 80, hold: 1200, gain: 244 };
  await expect.poll(of(sent, "set_powergate_conf")).toEqual([own, { ...own, inst_id: 3 }]);
  const either = [await wouldSend("loopback-0", "set_powergate_conf", own), await wouldSend("loopback-0", "set_powergate_conf", { ...own, inst_id: 3 })];
  await expect(lastSent(page)).toHaveText(new RegExp(`^(${either.join("|")})$`));

  const gain = page.getByTestId("param-gain");
  await gain.focus();
  await gain.press("Home");
  await expect(gain).toHaveAttribute("aria-valuetext", "-24");
  await expect.poll(() => of(sent, "set_powergate_conf")().at(-2)).toEqual({ ...own, gain: 232 });

  await threshold.dblclick();
  await expect(threshold).toHaveAttribute("aria-valuetext", "90");
  await expect.poll(() => of(sent, "set_powergate_conf")().at(-2)?.["threshold"]).toBe(90);
});

test("menus, switches and bit masks: PowerFFC's detector, FET-A76's ratio buttons, and a sidechain source kept as read", async ({ page }) => {
  const sent = await answerReads(page);
  await page.goto(`${server.url}/#/effects/loopback-0`);

  await page.getByTestId("edit-0-1").click();
  await expect(page.getByTestId("param-attack")).toHaveAttribute("aria-valuetext", "12.5");
  await expect(page.getByTestId("param-ratio")).toHaveAttribute("aria-valuetext", "4.0");
  const taw = page.getByTestId("param-taw");
  await expect(taw).toHaveValue("65535");
  await taw.selectOption({ label: "RMS 50" });
  const compressor = { type_id: 2, inst_id: 0, attack: 12500, release: 10000, taw: 5000, ratio: 400, gain: 150, ctrl: 0, threshold: 24, knee: 0, linked: 0 };
  await expect.poll(() => of(sent, "set_compressor_cfg")()[0]).toEqual(compressor);
  await expect(lastSent(page)).toHaveText(new RegExp(`^(${[await wouldSend("loopback-0", "set_compressor_cfg", compressor), await wouldSend("loopback-0", "set_compressor_cfg", { ...compressor, inst_id: 1 })].join("|")})$`));

  await page.getByTestId("edit-2-0").click();
  const eight = page.getByTestId("param-ratio-1");
  await expect(eight).toHaveAttribute("aria-pressed", "false");
  await eight.click();
  await expect(eight).toHaveAttribute("aria-pressed", "true");
  await expect.poll(() => of(sent, "set_uad_1176_conf")()).toEqual([{ type_id: 9, inst_id: 0, input: 35, output: 46, attack: 78, release: 21, ratio: 3 }]);
  await expect(lastSent(page)).toContainText(await wouldSend("loopback-0", "set_uad_1176_conf", { type_id: 9, inst_id: 0, input: 35, output: 46, attack: 78, release: 21, ratio: 3 }));

  await page.getByTestId("edit-2-1").click();
  await expect(page.getByTestId("editor-note")).toContainText("sidechain");
  const linlog = page.getByTestId("param-linlog");
  await expect(linlog).toHaveAttribute("aria-pressed", "false");
  await linlog.click();
  await expect.poll(() => of(sent, "set_Brainiac_conf")()).toEqual([{ type_id: 85, inst_id: 1, release: 0, attack: 0, range: 0, ratio: 8, thresh: 0, linlog: 1, mode: 0, sideSource: 7, sideChanN: 3 }]);
  await expect(lastSent(page)).toContainText(await wouldSend("loopback-0", "set_Brainiac_conf", { type_id: 85, inst_id: 1, release: 0, attack: 0, range: 0, ratio: 8, thresh: 0, linlog: 1, mode: 0, sideSource: 7, sideChanN: 3 }));
});

test("an effect whose parameters are left out says why, and reads nothing", async ({ page }) => {
  const sent = await answerReads(page);
  await page.goto(`${server.url}/#/effects/loopback-0`);
  await page.getByTestId("edit-3-0").click();
  await expect(page.getByTestId("editor-note")).toContainText("instance");
  await expect(page.getByTestId("effect-editor").locator("[data-testid^='param-']")).toHaveCount(0);
  expect(sent.filter((f) => f.command === "get_eq_configs")).toHaveLength(0);
});

test("the Studio+ reads every instance of the type at once and addresses the one chosen", async ({ page }) => {
  const sent = await answerReads(page);
  await page.goto(`${server.url}/#/effects/loopback-1`);
  await page.getByTestId("edit-0-0").click();
  const threshold = page.getByTestId("param-threshold");
  await expect(threshold).toHaveAttribute("aria-valuetext", "107");
  await expect(page.getByTestId("param-gain")).toHaveAttribute("aria-valuetext", "-6");
  expect(sent.filter((f) => f.command === "get_powergate_configs").map((f) => f.args ?? null)).toEqual([null]);
  await threshold.focus();
  await threshold.press("ArrowDown");
  const args = { type_id: 39, inst_id: 7, threshold: 106, range: 4, attack: 250, decay: 80, hold: 1200, gain: -6 };
  await expect.poll(() => of(sent, "set_powergate_conf")()).toEqual([args]);
  await expect(lastSent(page)).toContainText(await wouldSend("loopback-1", "set_powergate_conf", args));
});

test.describe("on a phone", () => {
  test.use({ viewport: { width: 375, height: 812 }, isMobile: true, hasTouch: true, deviceScaleFactor: 2 });

  test("the editor fits the width and nothing scrolls sideways", async ({ page }) => {
    await answerReads(page);
    await page.goto(`${server.url}/#/effects/loopback-0`);
    await page.getByTestId("edit-0-1").tap();
    const editor = page.getByTestId("effect-editor");
    await expect(page.getByTestId("param-knee")).toBeVisible();
    await editor.scrollIntoViewIfNeeded();
    const box = await editor.boundingBox();
    expect(box?.width ?? 999).toBeLessThanOrEqual(375);
    const sideways = await page.evaluate(() => {
      const main = document.querySelector("ga-app")?.shadowRoot?.querySelector("main");
      return { page: document.documentElement.scrollWidth - window.innerWidth, main: main === null || main === undefined ? -1 : main.scrollWidth - main.clientWidth };
    });
    expect(sideways).toEqual({ page: 0, main: 0 });
  });
});
