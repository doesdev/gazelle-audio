// The Effects page (specs/2026-09-17-effects-and-reverb.md): chains read from the device, bypass per
// effect, the reverb, and the Quadro's reverb returns and sends. The server runs in dry run, which
// answers no reads, so the device's replies are supplied through the WebSocket (as the licensing test
// does); every write still goes to the server, and what it would send is compared with the bytes the
// server gives for the same command and arguments.

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
const QUADRO_CHAINS: Record<number, [number, number][]> = { 0: [[39, 2], [1, 0]], 1: [[39, 3], [1, 1]], 4: [[9, 0]] };

type Frame = { id?: number; device_id?: string; command?: string; ext3?: number };

/** Answers the effects reads of loopback-0 (Quadro) and loopback-1 (Studio+) as a device would. */
async function answerReads(page: Page): Promise<void> {
  const replies: Record<string, (frame: Frame) => unknown> = {
    "loopback-0|get_afx_strip_order": (frame) => ({ entries: [{ slots: slots(...(QUADRO_CHAINS[frame.ext3 ?? -1] ?? [])) }] }),
    "loopback-0|get_afx_links": () => ({ entries: [1, 0, 0, 0, 0, 0, 0].map((linked) => ({ linked })) }),
    "loopback-0|get_reverb_config": () => ({ mixer_id: 0, room_size: 40, color: 10, predelay: 20, density: 100, early_ref_gain: 30, late_ref_delay: 50, richness: 60, reverb_time: 70, reverb_level: 25, on: 1 }),
    "loopback-0|get_reverb_returns": () => ({ entries: [{ level: 12, mute: 0 }, { level: 90, mute: 1 }, { level: 0, mute: 0 }, { level: 0, mute: 0 }] }),
    "loopback-0|get_reverb_sends": () => ({ entries: Array.from({ length: 33 }, (_, i) => ({ level: i === 2 ? 40 : 96, pan: 32, mute: 0, solo: 0 })) }),
    "loopback-1|get_afx_order": () => ({ entries: Array.from({ length: 16 }, (_, i) => ({ slots: i === 3 ? slots([2, 5]) : slots() })) }),
    "loopback-1|get_afx_links": () => ({ entries: Array.from({ length: 8 }, () => ({ linked: 0 })) }),
    "loopback-1|get_reverb_config": () => ({ mixer_id: 0, room_size: 0, color: 0, predelay: 0, density: 100, early_ref_gain: 0, late_ref_delay: 0, richness: 0, reverb_time: 0, reverb_level: 50, on: 0 }),
  };
  await page.routeWebSocket(/\/ws$/, (socket) => {
    const upstream = socket.connectToServer();
    socket.onMessage((message) => {
      const frame: Frame = typeof message === "string" ? (JSON.parse(message) as Frame) : {};
      const reply = replies[`${frame.device_id}|${frame.command}`];
      if (reply === undefined) return upstream.send(message);
      socket.send(JSON.stringify({ type: "rpc_response", id: frame.id, result: { device_id: frame.device_id, command: frame.command, sent_hex: "74", sent_len: 16, dry_run: false, response: reply(frame), response_error: null } }));
    });
  });
}

/** What the server says it would send for a command, to compare with the page's last send. */
async function wouldSend(deviceId: string, command: string, args: Record<string, unknown>): Promise<string> {
  const response = await fetch(`${server.url}/api/v1/devices/${deviceId}/command/${command}?dry_run=true`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(args) });
  const body = (await response.json()) as { sent_hex: string };
  return `Dry run, would send ${command}: ${body.sent_hex}`;
}

const lastSent = (page: Page) => page.getByTestId("last-sent");

test("Effects is in the header and opens the first device of known model", async ({ page }) => {
  await page.goto(server.url);
  await page.locator('ga-header a[data-page="effects"]').click();
  await expect(page).toHaveURL(/#\/effects$/);
  await expect(page.getByTestId("chain-0")).toBeVisible();
});

test("Quadro chains show what the device reports: effects by name and instance, links, and nothing for an empty chain", async ({ page }) => {
  await answerReads(page);
  await page.goto(`${server.url}/#/effects/loopback-0`);
  await expect(page.getByTestId("chain-5")).toBeVisible();
  await expect(page.getByTestId("chain-6")).toHaveCount(0);
  await expect(page.getByTestId("slot-0-0")).toContainText("PowerGate");
  await expect(page.getByTestId("slot-0-0")).toContainText("3", { useInnerText: true });
  await expect(page.getByTestId("slot-0-1")).toContainText("ClearQ");
  await expect(page.getByTestId("slot-4-0")).toContainText("FET-A76");
  await expect(page.getByTestId("chain-2")).toContainText("No effects");
  await expect(page.getByTestId("chain-link-0")).toBeVisible();
  await expect(page.getByTestId("chain-link-2")).toBeHidden();
  await expect(page.getByTestId("effects-note")).toHaveText("");
});

test("bypass sends set_afx_bypass per instance, enabled 0, and the linked partner follows; bypass all sends each", async ({ page }) => {
  await answerReads(page);
  await page.goto(`${server.url}/#/effects/loopback-0`);
  const bypass = page.getByTestId("bypass-0-0");
  await expect(bypass).toHaveAttribute("aria-pressed", "false");
  await expect(page.getByTestId("active-0-0")).toHaveAttribute("aria-pressed", "false", { timeout: 1000 });

  await bypass.click();
  await expect(bypass).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("bypass-1-0")).toHaveAttribute("aria-pressed", "true");
  // The partner's instance is sent last.
  await expect(lastSent(page)).toContainText(await wouldSend("loopback-0", "set_afx_bypass", { periph_id: 3, periph_type: 39, enabled: 0 }));

  await page.getByTestId("active-4-0").click();
  await expect(lastSent(page)).toContainText(await wouldSend("loopback-0", "set_afx_bypass", { periph_id: 0, periph_type: 9, enabled: 1 }));
  await page.getByTestId("chain-bypass-all-4").click();
  await expect(page.getByTestId("bypass-4-0")).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("chain-bypass-all-2")).toBeDisabled();
});

test("the reverb: on/off and level resend the whole config with density 100; the rest is shown as the panel shows it", async ({ page }) => {
  await answerReads(page);
  await page.goto(`${server.url}/#/effects/loopback-0`);
  const on = page.getByTestId("reverb-on");
  await expect(on).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("reverb-params")).toContainText("21 / 12 / 8");
  await expect(page.getByTestId("reverb-params")).toContainText("20");
  const config = { mixer_id: 0, room_size: 40, color: 10, predelay: 20, density: 100, early_ref_gain: 30, late_ref_delay: 50, richness: 60, reverb_time: 70 };

  await on.click();
  await expect(on).toHaveAttribute("aria-pressed", "false");
  await expect(lastSent(page)).toContainText(await wouldSend("loopback-0", "set_reverb_config", { ...config, reverb_level: 25, on: 0 }));

  const level = page.getByTestId("reverb-level");
  await expect(level).toHaveAttribute("aria-valuetext", "0 dB");
  await level.focus();
  await level.press("End");
  await expect(level).toHaveAttribute("aria-valuetext", "+12 dB");
  await expect(lastSent(page)).toContainText(await wouldSend("loopback-0", "set_reverb_config", { ...config, reverb_level: 100, on: 0 }));
});

test("Quadro reverb returns into mixes 1-2 and sends from mix 1's channels", async ({ page }) => {
  await answerReads(page);
  await page.goto(`${server.url}/#/effects/loopback-0`);
  await expect(page.getByTestId("return-level-2")).toHaveCount(0);
  const mute = page.getByTestId("return-mute-1");
  await expect(mute).toHaveAttribute("aria-pressed", "true");
  await mute.click();
  await expect(lastSent(page)).toContainText(await wouldSend("loopback-0", "set_reverb_return", { mixer_id: 1, level: 90, mute: 0 }));

  const returnLevel = page.getByTestId("return-level-0");
  await returnLevel.focus();
  await returnLevel.press("End");
  await expect(lastSent(page)).toContainText(await wouldSend("loopback-0", "set_reverb_return", { mixer_id: 0, level: 0, mute: 0 }));

  await expect(page.getByTestId("send-level-16")).toBeVisible();
  const send = page.getByTestId("send-level-2");
  await expect(send).toHaveAttribute("aria-valuetext", "-40 dB");
  await send.focus();
  await send.press("ArrowUp");
  await expect(lastSent(page)).toContainText(await wouldSend("loopback-0", "set_reverb_send", { mixer_id: 0, channel: 2, level: 39, pan: 32, mute: 0, solo: 0 }));
});

test("the Studio+ has sixteen chains, its own bypass fields, and no reverb returns or sends of its own", async ({ page }) => {
  await answerReads(page);
  await page.goto(`${server.url}/#/effects/loopback-1`);
  await expect(page.getByTestId("chain-15")).toBeVisible();
  await expect(page.getByTestId("slot-3-0")).toContainText("Compressor");
  await expect(page.getByTestId("return-mute-0")).toHaveCount(0);
  await expect(page.getByTestId("send-level-1")).toHaveCount(0);
  await expect(page.getByTestId("reverb-level")).toHaveAttribute("aria-valuetext", "+6 dB");
  await page.getByTestId("bypass-3-0").click();
  await expect(lastSent(page)).toContainText(await wouldSend("loopback-1", "set_afx_bypass", { inst_id: 5, type_id: 2, enabled: 0 }));
});

test("with nothing read (dry run) the page says so, and chains are not shown as empty", async ({ page }) => {
  await page.goto(`${server.url}/#/effects/loopback-0`);
  await expect(page.getByTestId("effects-note")).toContainText("dry run");
  await expect(page.getByTestId("chain-0")).toContainText("Not read");
});

test("on the loopback, not in dry run, chains read back empty and the reverb follows what was set", async ({ page }) => {
  const live = await startServer([], { webUi: true });
  try {
    await page.goto(`${live.url}/#/effects/loopback-0`);
    await expect(page.getByTestId("chain-0")).toContainText("No effects");
    const on = page.getByTestId("reverb-on");
    await expect(on).toHaveAttribute("aria-pressed", "false");
    await expect(page.getByTestId("effects-note")).toHaveText("");
    await on.click();
    await expect(on).toHaveAttribute("aria-pressed", "true");
    await page.reload();
    await expect(page.getByTestId("reverb-on")).toHaveAttribute("aria-pressed", "true");
    await expect(page.locator("ga-notices")).not.toContainText("could not");
  } finally {
    await live.stop();
  }
});
