// Phase 5 increment 2: the Inputs page sends each family's preamp and input commands with the
// expected bytes in dry run. Expected bytes are the protocol crate's ground-truth vectors (all
// fields zero) with the two payload fields set: a two-byte payload has a one-byte payload header,
// so `id` is byte 17 and the value byte 18 (signed values as their byte).

import { expect, test, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { REPO_ROOT, startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { resetWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--dry-run"], { webUi: true });
});

test.beforeEach(() => resetWorkspace(server));

test.afterAll(async () => {
  await server?.stop();
});

function expectedHex(file: "ground_truth.json" | "ground_truth_studio.json", command: string, id: number, value: number): string {
  const vectors = JSON.parse(readFileSync(join(REPO_ROOT, "crates", "gazelle-audio-protocol", "tests", file), "utf8")) as Record<string, string>;
  const hex = vectors[command];
  if (hex === undefined) throw new Error(`no ${command} vector in ${file}`);
  const bytes = Uint8Array.from(hex.match(/../g) ?? [], (pair) => Number.parseInt(pair, 16));
  bytes[17] = id;
  bytes[18] = value & 0xff;
  return [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
}

const lastSent = (page: Page) => page.getByTestId("last-sent");

test("Quadro preamps send type, gain, a confirmed 48V and phase, with the panels' ranges", async ({ page }) => {
  const quadro = (command: string, id: number, value: number) => `Dry run, would send ${command}: ${expectedHex("ground_truth.json", command, id, value)}`;
  await page.goto(`${server.url}/#/inputs/loopback-0`);
  await expect(page.getByTestId("preamp-3")).toBeVisible();
  await expect(page.getByTestId("preamp-4")).toHaveCount(0);
  await expect(page.getByTestId("pre-type-1-hi-z")).toBeVisible();
  await expect(page.getByTestId("pre-type-2-hi-z")).toHaveCount(0, { timeout: 1000 });

  await page.getByTestId("pre-type-1-line").click();
  await expect(lastSent(page)).toContainText(quadro("set_pre_type", 1, 1));
  await expect(page.getByTestId("pre-type-1-line")).toHaveAttribute("aria-pressed", "true");
  const gain = page.getByTestId("pre-gain-1");
  await gain.focus();
  await gain.press("Home");
  await expect(gain).toHaveAttribute("aria-valuetext", "-6 dB");
  await expect(lastSent(page)).toContainText(quadro("set_pre_gain", 1, -6));
  await expect(page.getByTestId("pre-48v-1")).toBeDisabled();

  const phantom = page.getByTestId("pre-48v-0");
  await phantom.click();
  await expect(phantom).toHaveText("Confirm");
  await expect(lastSent(page)).not.toContainText("set_pre_phantom");
  await phantom.click();
  await expect(phantom).toHaveAttribute("aria-pressed", "true");
  await expect(lastSent(page)).toContainText(quadro("set_pre_phantom", 0, 1));

  await page.getByTestId("pre-phase-3").click();
  await expect(lastSent(page)).toContainText(quadro("set_pre_phase_inv", 3, 1));
  await expect(page.getByTestId("adat-gain-0")).toHaveText("—", { timeout: 1000 });
});

/** Records each set_pre_gain the page sends, as "device:id". */
function recordGains(page: Page): string[] {
  const gains: string[] = [];
  page.on("websocket", (socket) =>
    socket.on("framesent", (event) => {
      const frame = typeof event.payload === "string" ? (JSON.parse(event.payload) as { device_id?: string; command?: string; args?: { id?: number } }) : {};
      if (frame.command === "set_pre_gain" && frame.args?.id !== undefined) gains.push(`${frame.device_id}:${frame.args.id}`);
    }),
  );
  return gains;
}

test("linking picks inputs then saves; a link of exactly a device pair sets the device's flag, and gain changes go to every member", async ({ page }) => {
  const gains = recordGains(page);
  await page.goto(`${server.url}/#/inputs/loopback-0`);
  await page.getByTestId("pre-link-2").click();
  await expect(page.getByTestId("link-bar")).toBeVisible();
  await page.getByTestId("pre-link-3").click();
  await page.getByTestId("link-save").click();
  await expect(page.getByTestId("link-bar")).toBeHidden();
  // set_stereo_link(periph 0 = preamps, pair 1 = preamps 3 and 4, linked 1): three bytes after a one-byte payload header.
  const vectors = JSON.parse(readFileSync(join(REPO_ROOT, "crates", "gazelle-audio-protocol", "tests", "ground_truth.json"), "utf8")) as Record<string, string>;
  const bytes = Uint8Array.from(vectors["set_stereo_link"]?.match(/../g) ?? [], (pair) => Number.parseInt(pair, 16));
  [bytes[17], bytes[18], bytes[19]] = [0, 1, 1];
  await expect(lastSent(page)).toContainText(`Dry run, would send set_stereo_link: ${[...bytes].map((b) => b.toString(16).padStart(2, "0")).join("")}`);
  await expect(page.getByTestId("pre-link-2")).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("pre-link-3")).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("pre-link-0")).toHaveAttribute("aria-pressed", "false");

  const gain = page.getByTestId("pre-gain-2");
  await gain.focus();
  await gain.press("ArrowRight");
  await expect.poll(() => gains).toEqual(["loopback-0:2", "loopback-0:3"]);

  await page.getByTestId("pre-link-3").click();
  await page.getByTestId("link-unlink").click();
  await expect(page.getByTestId("pre-link-2")).toHaveAttribute("aria-pressed", "false");
});

test("a link can join inputs on two devices, in relative mode, and the 48V confirm names how many inputs it turns on", async ({ page }) => {
  const gains = recordGains(page);
  await page.goto(`${server.url}/#/inputs/loopback-0`);
  await page.getByTestId("pre-link-0").click();
  await page.locator('ga-inputs select[aria-label="Device"]').selectOption("loopback-1");
  await expect(page).toHaveURL(/inputs\/loopback-1$/);
  await expect(page.getByTestId("link-bar")).toContainText("Zen Quadro");
  await page.getByTestId("pre-link-1").click();
  await page.getByTestId("link-mode-relative").click();
  await page.getByTestId("link-save").click();
  await expect(page.getByTestId("pre-link-1")).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("pre-link-1")).toHaveAttribute("title", /linked with Zen Quadro.* Preamp 1 \(relative\)/);

  const gain = page.getByTestId("pre-gain-1");
  await gain.focus();
  await gain.press("ArrowRight");
  await expect.poll(() => gains).toEqual(["loopback-1:1", "loopback-0:0"]);

  const phantom = page.getByTestId("pre-48v-1");
  await phantom.click();
  await expect(phantom).toHaveText("Confirm 2");
  await phantom.click();
  await expect(phantom).toHaveAttribute("aria-pressed", "true");

  await page.getByTestId("pre-link-1").click();
  await page.getByTestId("link-unlink").click();
  await expect(page.getByTestId("pre-link-1")).toHaveAttribute("aria-pressed", "false");
});

test("Studio+ sends set_pre_phaseinv and digital input gains; Hi-Z is on preamps 1-4", async ({ page }) => {
  const studio = (command: string, id: number, value: number) => `Dry run, would send ${command}: ${expectedHex("ground_truth_studio.json", command, id, value)}`;
  await page.goto(`${server.url}/#/inputs/loopback-1`);
  await expect(page.getByTestId("pre-type-3-hi-z")).toBeVisible();
  await expect(page.getByTestId("pre-type-4-hi-z")).toHaveCount(0);

  await page.getByTestId("pre-phase-0").click();
  await expect(lastSent(page)).toContainText(studio("set_pre_phaseinv", 0, 1));

  const line = page.getByTestId("line-gain-2");
  await line.focus();
  await line.press("End");
  await expect(lastSent(page)).toContainText(studio("set_line_gain", 2, 12));
  const spdif = page.getByTestId("spdif-gain-1");
  await spdif.focus();
  await spdif.press("Home");
  await expect(lastSent(page)).toContainText(studio("set_spdif_gain", 1, -6));
});

test("read-only digital gains show a filled bar, and preamps and digital inputs share one column grid", async ({ page }) => {
  const reporting = await startServer(["--dry-run", "--loopback-cyclic-ms", "50"], { webUi: true });
  try {
    await page.setViewportSize({ width: 1400, height: 900 });
    await page.goto(`${reporting.url}/#/inputs/loopback-0`);
    const adat = page.getByTestId("adat-gain-0");
    await expect(adat).not.toHaveText("—");
    await expect.poll(() => adat.locator(".fill").evaluate((el) => el.getBoundingClientRect().width)).toBeGreaterThan(0);
    await expect(adat).not.toHaveAttribute("role", "slider");

    // Each preamp spans two columns of the grid the digital cells use, so edges line up.
    const left = async (testId: string) => (await page.getByTestId(testId).locator("xpath=ancestor-or-self::*[contains(@class,'cell') or contains(@class,'preamp')][1]").boundingBox())?.x ?? -1;
    const right = async (testId: string) => {
      const box = await page.getByTestId(testId).locator("xpath=ancestor-or-self::*[contains(@class,'cell') or contains(@class,'preamp')][1]").boundingBox();
      return (box?.x ?? 0) + (box?.width ?? 0);
    };
    expect(Math.abs((await left("preamp-1")) - (await left("adat-gain-2")))).toBeLessThanOrEqual(1);
    expect(Math.abs((await right("preamp-0")) - (await right("adat-gain-1")))).toBeLessThanOrEqual(1);
    expect(Math.abs((await left("preamp-3")) - (await left("adat-gain-6")))).toBeLessThanOrEqual(1);
  } finally {
    await reporting.stop();
  }
});

test("Studio+ line, ADAT and S/PDIF pairs link with their own peripheral id; the Quadro's digital inputs have no link", async ({ page }) => {
  const vectors = JSON.parse(readFileSync(join(REPO_ROOT, "crates", "gazelle-audio-protocol", "tests", "ground_truth_studio.json"), "utf8")) as Record<string, string>;
  const linkHex = (periph: number, pair: number) => {
    const bytes = Uint8Array.from(vectors["set_stereo_link"]?.match(/../g) ?? [], (b) => Number.parseInt(b, 16));
    [bytes[17], bytes[18], bytes[19]] = [periph, pair, 1];
    return `Dry run, would send set_stereo_link: ${[...bytes].map((b) => b.toString(16).padStart(2, "0")).join("")}`;
  };
  await page.goto(`${server.url}/#/inputs/loopback-1`);
  const link = async (kind: string, a: number, b: number) => {
    await page.getByTestId(`${kind}-link-${a}`).click();
    await page.getByTestId(`${kind}-link-${b}`).click();
    await page.getByTestId("link-save").click();
  };
  await link("line", 2, 3); // lines 3 and 4
  await expect(lastSent(page)).toContainText(linkHex(1, 1));
  await expect(page.getByTestId("line-link-3")).toHaveAttribute("aria-pressed", "true");
  await link("adat", 14, 15); // ADAT 15 and 16
  await expect(lastSent(page)).toContainText(linkHex(2, 7));
  await link("spdif", 0, 1);
  await expect(lastSent(page)).toContainText(linkHex(3, 0));
  for (const [kind, first] of [["line", 2], ["adat", 14], ["spdif", 0]] as const) {
    await page.getByTestId(`${kind}-link-${first}`).click();
    await page.getByTestId("link-unlink").click();
  }

  await page.goto(`${server.url}/#/inputs/loopback-0`);
  await expect(page.getByTestId("adat-gain-0")).toBeVisible();
  await expect(page.getByTestId("adat-link-0")).toHaveCount(0);
});

test("Inputs is in the header and opens the first device of known model", async ({ page }) => {
  await page.goto(server.url);
  await page.locator('ga-header a[data-page="inputs"]').click();
  await expect(page).toHaveURL(/#\/inputs$/);
  await expect(page.getByTestId("preamp-0")).toBeVisible();
});

test("mic emulation: a microphone per preamp and one of its emulations; the Studio+ has none", async ({ page }) => {
  const frames: { command?: string; args?: Record<string, number> }[] = [];
  page.on("websocket", (socket) =>
    socket.on("framesent", (event) => {
      if (typeof event.payload === "string") frames.push(JSON.parse(event.payload) as { command?: string; args?: Record<string, number> });
    }),
  );
  await page.goto(`${server.url}/#/inputs/loopback-1`);
  await expect(page.getByTestId("mic-target-0")).toHaveCount(0);

  await page.goto(`${server.url}/#/inputs/loopback-0`);
  const target = page.getByTestId("mic-target-0");
  await expect(target.locator("option")).toHaveText(["None", "Edge Duo", "Verge", "Edge Solo", "Edge Quadro", "Accord", "Edge Note"]);
  // With no microphone there is nothing to emulate, so the catalogue is empty until one is named.
  await expect(page.getByTestId("mic-model-0")).toBeDisabled();

  await target.selectOption("3");
  await expect.poll(() => frames.filter((f) => f.command === "set_mic_emulation").at(-1)?.args).toEqual({ preamp_ch: 0, target: 3, emu_model: 0, ch_swap: 0, pattern: 0 });
  const model = page.getByTestId("mic-model-0");
  await expect(model).toBeEnabled();
  await expect(model.locator("option").nth(2)).toHaveText("Berlin 47 FT");
  await model.selectOption("2");
  await expect.poll(() => frames.filter((f) => f.command === "set_mic_emulation").at(-1)?.args).toEqual({ preamp_ch: 0, target: 3, emu_model: 2, ch_swap: 0, pattern: 0 });
});

test("an Edge Duo covers two preamps and an Edge Quadro four, linked, with an emulation per head", async ({ page }) => {
  const frames: { command?: string; args?: Record<string, number> }[] = [];
  page.on("websocket", (socket) =>
    socket.on("framesent", (event) => {
      if (typeof event.payload === "string") frames.push(JSON.parse(event.payload) as { command?: string; args?: Record<string, number> });
    }),
  );
  const emulations = () => frames.filter((f) => f.command === "set_mic_emulation").map((f) => f.args?.["preamp_ch"]);
  await page.goto(`${server.url}/#/inputs/loopback-0`);

  // An Edge Duo is one microphone on two preamps, so the second preamp's row steps aside.
  await page.getByTestId("mic-target-0").selectOption("1");
  await expect.poll(emulations).toEqual([0, 1]);
  await expect(page.getByTestId("mic-row-0")).toContainText("Preamps 1–2");
  await expect(page.getByTestId("mic-row-1")).toBeHidden();
  // Its preamps are linked, absolute, so their gain and 48V move together.
  await expect(page.getByTestId("pre-link-0")).toHaveAttribute("aria-pressed", "true");

  // The Edge Quadro is two heads on four preamps, and each head takes its own emulation.
  await page.getByTestId("mic-target-0").selectOption("4");
  await expect(page.getByTestId("mic-row-0")).toContainText("Preamps 1–4");
  await expect(page.getByTestId("mic-row-2")).toBeHidden();
  await page.getByTestId("mic-model-0-top").selectOption("3");
  await expect.poll(() => frames.filter((f) => f.command === "set_mic_emulation").slice(-2).map((f) => [f.args?.["preamp_ch"], f.args?.["emu_model"]])).toEqual([
    [2, 3],
    [3, 3],
  ]);
});
