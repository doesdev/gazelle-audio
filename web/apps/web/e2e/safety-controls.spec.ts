// The safety decisions of 2026-09-18, after the documentation overhaul named the risks:
// a two-click confirm, as 48V has, for recalling a device preset, turning a test tone on, turning
// DC coupling on and changing the clock source or sample rate; and a double-click on a level that
// goes to a safe -20 dB, with Ctrl+click for unity and a setting, remembered per browser, that
// makes double-click unity instead. The server runs the loopback in dry run with no reports, so
// every tone and switch reads as off unless a test says otherwise.

import { expect, test, type Locator, type Page } from "@playwright/test";

import { startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { putWorkspace, resetWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--backend", "loopback", "--dry-run"], { webUi: true });
});

test.beforeEach(() => resetWorkspace(server));

test.afterAll(async () => {
  await server?.stop();
});

type Frame = { command?: string; device_id?: string; args?: Record<string, number> };

function recordFrames(page: Page): Frame[] {
  const frames: Frame[] = [];
  page.on("websocket", (socket) =>
    socket.on("framesent", (event) => {
      if (typeof event.payload === "string") frames.push(JSON.parse(event.payload) as Frame);
    }),
  );
  return frames;
}

/** The last value of `field` sent with `command`, among frames `where` accepts. */
const lastArg = (frames: Frame[], command: string, field: string, where: (f: Frame) => boolean = () => true) => () => frames.filter((f) => f.command === command && where(f)).at(-1)?.args?.[field];

/** A click with Ctrl held, where on the control it lands mattering not at all. */
async function ctrlClick(control: Locator, at: { x: number; y: number }): Promise<void> {
  await control.click({ modifiers: ["Control"], position: at });
}

const QUADRO_CHANNEL = { "loopback-0": { channels: [{ id: "a", name: "", slot: 9, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }] } };

test.describe("level resets", () => {
  test("a mixer fader: double-click goes to -20 dB, Ctrl+click to 0 dB wherever it lands, and the title says so", async ({ page }) => {
    const frames = recordFrames(page);
    const level = lastArg(frames, "set_mixer", "level", (f) => f.args?.["channel"] === 10);
    await putWorkspace(server, { mixers: QUADRO_CHANNEL });
    await page.goto(`${server.url}/#/mixer/loopback-0`);
    const fader = page.getByTestId("fader-9");
    await expect(fader).toHaveAttribute("aria-disabled", "false");
    await expect(fader).toHaveAttribute("title", "Double-click: -20 dB. Ctrl/Cmd+click: 0 dB.");

    await fader.dblclick();
    await expect.poll(level).toBe(20);
    await expect(page.getByTestId("level-9")).toHaveText("-20 dB");

    // Near the bottom of the track, where a plain click would pull the fader nearly all the way down.
    const box = await fader.boundingBox();
    if (box === null) throw new Error("the fader has no box");
    await ctrlClick(fader, { x: box.width / 2, y: box.height - 4 });
    await expect.poll(level).toBe(0);
    await expect(page.getByTestId("level-9")).toHaveText("0 dB");

    // Two Ctrl+clicks make a double-click too; it is still unity, not the safe level.
    await fader.dblclick();
    await expect.poll(level).toBe(20);
    await fader.dblclick({ modifiers: ["Control"] });
    await page.waitForTimeout(150);
    expect(level()).toBe(0);

    // The pan beside it keeps its reset, centre, and its Ctrl+click is an ordinary click.
    const pan = page.getByTestId("pan-9");
    await pan.dblclick();
    await expect(pan).toHaveAttribute("aria-valuetext", "C");
    await expect(pan).not.toHaveAttribute("title", /Ctrl/);
  });

  test("the setting makes double-click unity, is remembered by the browser, and the titles follow it", async ({ page }) => {
    const frames = recordFrames(page);
    const level = lastArg(frames, "set_mixer", "level", (f) => f.args?.["channel"] === 10);
    await putWorkspace(server, { mixers: QUADRO_CHANNEL });
    await page.goto(`${server.url}/#/mixer/loopback-0`);
    const setting = page.getByTestId("double-click-level");
    await expect(setting).toHaveValue("safe");
    await expect(setting.locator("option")).toHaveText(["Double-click: safe level", "Double-click: unity"]);
    await setting.selectOption("unity");

    const fader = page.getByTestId("fader-9");
    await expect(fader).toHaveAttribute("title", "Double-click or Ctrl/Cmd+click: 0 dB.");
    await fader.dblclick();
    await expect.poll(level).toBe(0);
    await expect(page.getByTestId("level-9")).toHaveText("0 dB");

    await page.reload();
    await expect(page.getByTestId("double-click-level")).toHaveValue("unity");
    await expect(page.getByTestId("fader-9")).toHaveAttribute("title", "Double-click or Ctrl/Cmd+click: 0 dB.");
    const box = await page.getByTestId("fader-9").boundingBox();
    if (box === null) throw new Error("the fader has no box");
    await page.getByTestId("fader-9").click({ position: { x: box.width / 2, y: box.height * 0.7 } });
    await expect.poll(level).not.toBe(0);
    await page.getByTestId("fader-9").dblclick();
    await expect.poll(level).toBe(0);

    await page.getByTestId("double-click-level").selectOption("safe");
    await expect(page.getByTestId("fader-9")).toHaveAttribute("title", "Double-click: -20 dB. Ctrl/Cmd+click: 0 dB.");
    await page.getByTestId("fader-9").dblclick();
    await expect.poll(level).toBe(20);
  });

  test("the Studio+ Send: double-click off, Ctrl+click 0 dB (the user, 2026-09-18)", async ({ page }) => {
    const frames = recordFrames(page);
    const send = lastArg(frames, "set_mixer_cfg", "send", (f) => f.args?.["channel"] === 1);
    await putWorkspace(server, { mixers: { "loopback-1": { channels: [{ id: "a", name: "", slot: 0, source: { group: 0, channel: 0 }, main_mix: 0, sends: [] }] } } });
    await page.goto(`${server.url}/#/mixer/loopback-1`);
    const bar = page.locator('ga-channel[data-channel-slot="0"] ga-strip .send');
    await expect(bar).toHaveAttribute("title", "Double-click: off. Ctrl/Cmd+click: 0 dB.");
    await bar.dblclick();
    await expect.poll(send).toBe(95);
    await expect(bar).toHaveAttribute("aria-valuetext", "-inf");
    await ctrlClick(bar, { x: 3, y: 5 });
    await expect.poll(send).toBe(0);
    await expect(bar).toHaveAttribute("aria-valuetext", "0 dB");
  });

  test("output, Control Room and talkback volumes: double-click -30 dB, Ctrl+click 0 dB; a gain keeps its reset (the user, 2026-09-18)", async ({ page }) => {
    const frames = recordFrames(page);
    const volume = (id: number) => lastArg(frames, "set_volume", "volume", (f) => f.device_id === "loopback-0" && f.args?.["id"] === id);
    await page.goto(`${server.url}/#/outputs/loopback-0`);
    const out = page.getByTestId("out-volume-3");
    await expect(out).toHaveAttribute("title", "Double-click: -30 dB. Ctrl/Cmd+click: 0 dB.");
    await out.dblclick();
    await expect.poll(volume(3)).toBe(30);
    await expect(out).toHaveAttribute("aria-valuetext", "-30 dB");
    await ctrlClick(out, { x: 3, y: 5 });
    await expect.poll(volume(3)).toBe(0);
    await expect(out).toHaveAttribute("aria-valuetext", "0 dB");

    const cr = page.locator("ga-control-room").getByTestId("cr-volume-1");
    await expect(cr).toHaveAttribute("title", "Double-click: -30 dB. Ctrl/Cmd+click: 0 dB.");
    await cr.dblclick();
    await expect.poll(volume(1)).toBe(30);
    await ctrlClick(cr, { x: 3, y: 5 });
    await expect.poll(volume(1)).toBe(0);

    await page.goto(`${server.url}/#/outputs/loopback-1`);
    const talk = page.getByTestId("talk-volume");
    await talk.dblclick();
    await expect(talk).toHaveAttribute("aria-valuetext", "-30 dB");
    await ctrlClick(talk, { x: 3, y: 5 });
    await expect(talk).toHaveAttribute("aria-valuetext", "0 dB");
    await expect.poll(lastArg(frames, "set_tbk_vol", "volume")).toBe(0);

    await page.goto(`${server.url}/#/inputs/loopback-0`);
    const gain = page.getByTestId("pre-gain-0");
    await gain.dblclick();
    await expect(gain).toHaveAttribute("aria-valuetext", "0 dB");
    await expect(gain).not.toHaveAttribute("title", /Ctrl/);
  });
});

test.describe("confirms", () => {
  test("a device preset recalls on a confirming second click; one click sends nothing, and a wait forgets it", async ({ page }) => {
    const frames = recordFrames(page);
    const recalled = () => frames.filter((f) => f.command === "preset_recall").map((f) => f.args?.["preset_idx"]);
    // The Studio+: the Quadro ignores a recall, so Gazelle does not offer its slots (measured 2026-09-20).
    await page.goto(`${server.url}/#/devices/loopback-1`);
    const three = page.getByTestId("preset-3");
    await expect(three).toHaveText("3");
    await expect(three).toHaveAttribute("title", "Recall preset 3: click twice");
    await three.click();
    await expect(three).toHaveText("Confirm");
    await expect(three).toHaveAttribute("data-armed", "");
    await expect(three).toHaveAttribute("aria-label", "Recall preset 3");
    await page.waitForTimeout(300);
    expect(recalled()).toEqual([]);
    await three.click();
    await expect.poll(recalled).toEqual([3]);
    await expect(three).toHaveText("3");

    const two = page.getByTestId("preset-2");
    await two.click();
    await expect(two).toHaveText("Confirm");
    await expect(two).toHaveText("2", { timeout: 5000 });
    // After the wait a click only asks again.
    await two.click();
    await expect(two).toHaveText("Confirm");
    await page.waitForTimeout(300);
    expect(recalled()).toEqual([3]);

    // The keyboard confirms as the mouse does.
    await expect(two).toHaveText("2", { timeout: 5000 });
    await two.press("Enter");
    await expect(two).toHaveText("Confirm");
    await page.waitForTimeout(300);
    expect(recalled()).toEqual([3]);
    await two.press("Enter");
    await expect.poll(recalled).toEqual([3, 2]);
  });

  test("a test tone turns on only on a confirming second click, and off with one", async ({ page }) => {
    const frames = recordFrames(page);
    const sines = () => frames.filter((f) => f.command === "set_sine_gen").map((f) => f.args?.["mute_left"]);
    await page.goto(`${server.url}/#/devices/loopback-0`);
    const tone = page.getByTestId("osc-on-left");
    await expect(tone).toHaveAttribute("aria-pressed", "false");
    await tone.click();
    await expect(tone).toHaveText("Confirm");
    await expect(tone).toHaveAttribute("aria-label", "Oscillator left on");
    await page.waitForTimeout(300);
    expect(sines()).toEqual([]);
    await tone.click();
    await expect.poll(sines).toEqual([0]);
    await expect(tone).toHaveText("Tone");
    await expect(tone).toHaveAttribute("aria-pressed", "true");

    await tone.click();
    await expect.poll(sines).toEqual([0, 1]);
    await expect(tone).toHaveText("Tone");
    await expect(tone).toHaveAttribute("aria-pressed", "false");
  });

  test("DC coupling turns on only on a confirming second click, and off with one", async ({ page }) => {
    const frames = recordFrames(page);
    const sent = () => frames.filter((f) => f.command === "set_dc_coupled").map((f) => [f.args?.["dc_coupled"], f.args?.["dc_coupled_io"]]);
    // Its own server with reports, rewritten so the device says its inputs are DC coupled and its outputs not.
    const own = await startServer(["--backend", "loopback", "--dry-run", "--loopback-cyclic-ms", "100"], { webUi: true });
    try {
      await page.routeWebSocket(/\/ws$/, (socket) => {
        const upstream = socket.connectToServer();
        upstream.onMessage((message) => {
          const frame = typeof message === "string" ? (JSON.parse(message) as { type?: string; fields?: Record<string, unknown> }) : {};
          if (frame.type === "cyclic" && frame.fields !== undefined && "dc_coupled_in" in frame.fields) {
            frame.fields["dc_coupled_in"] = 1;
            frame.fields["dc_coupled_out"] = 0;
            return socket.send(JSON.stringify(frame));
          }
          socket.send(message);
        });
        socket.onMessage((message) => upstream.send(message));
      });
      await page.goto(`${own.url}/#/devices/loopback-0`);
      const inputs = page.getByTestId("dc-inputs");
      const outputs = page.getByTestId("dc-outputs");
      await expect(inputs).toHaveAttribute("aria-pressed", "true");
      await expect(outputs).toHaveAttribute("aria-pressed", "false");

      await outputs.click();
      await expect(outputs).toHaveText("Confirm");
      await page.waitForTimeout(300);
      expect(sent()).toEqual([]);
      await outputs.click();
      await expect.poll(sent).toEqual([[1, 1]]);
      await expect(outputs).toHaveText("DC coupled");

      await inputs.click();
      await expect.poll(sent).toEqual([[1, 1], [0, 0]]);
      await expect(inputs).toHaveText("DC coupled");
    } finally {
      await own.stop();
    }
  });

  test("the clock source and sample rate change only on a confirming click, and a wait puts the menu back", async ({ page }) => {
    const frames = recordFrames(page);
    const sources = () => frames.filter((f) => f.command === "set_sync_source").map((f) => f.args?.["src_index"]);
    const rates = () => frames.filter((f) => f.command === "set_samp_rate").map((f) => f.args?.["srate_idx"]);
    await page.goto(`${server.url}/#/devices/loopback-0`);
    const source = page.getByTestId("clock-source");
    const sourceConfirm = page.getByTestId("clock-source-confirm");
    await expect(sourceConfirm).toBeHidden();

    await source.selectOption("4");
    await expect(sourceConfirm).toBeVisible();
    await expect(sourceConfirm).toHaveText("Confirm");
    await expect(sourceConfirm).toHaveAttribute("title", /S\/PDIF/);
    await page.waitForTimeout(300);
    expect(sources()).toEqual([]);
    await sourceConfirm.click();
    await expect.poll(sources).toEqual([4]);
    await expect(sourceConfirm).toBeHidden();

    // Choosing and then waiting sends nothing and shows the source the device has again.
    const rate = page.getByTestId("clock-rate");
    const rateConfirm = page.getByTestId("clock-rate-confirm");
    const rateBefore = await rate.inputValue();
    await rate.selectOption(rateBefore === "2" ? "4" : "2");
    await expect(rateConfirm).toBeVisible();
    await expect(rateConfirm).toBeHidden({ timeout: 5000 });
    await expect(rate).toHaveValue(rateBefore);
    expect(rates()).toEqual([]);

    // Choosing the rate it already has asks nothing.
    await rate.selectOption("5");
    await expect(rateConfirm).toBeVisible();
    await rate.selectOption(rateBefore);
    await expect(rateConfirm).toBeHidden();

    // From the keyboard: choose, Tab to Confirm, Enter.
    await rate.selectOption("2" === rateBefore ? "4" : "2");
    await rate.focus();
    await page.keyboard.press("Tab");
    await expect(rateConfirm).toBeFocused();
    await page.keyboard.press("Enter");
    await expect.poll(rates).toEqual(["2" === rateBefore ? 4 : 2]);
  });

  test("48V on a mixer channel asks with the same word as the Inputs page", async ({ page }) => {
    const frames = recordFrames(page);
    await putWorkspace(server, { mixers: QUADRO_CHANNEL });
    await page.goto(`${server.url}/#/mixer/loopback-0`);
    const phantom = page.getByTestId("pre-48v-ch-9");
    await phantom.click();
    await expect(phantom).toHaveText("Confirm");
    await page.waitForTimeout(300);
    expect(frames.filter((f) => f.command === "set_pre_phantom")).toEqual([]);
  });
});
