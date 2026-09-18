// `pnpm -C web docs:screenshots`: the manual's screenshots, taken from the running app against
// the loopback emulator and written to docs/images. Rerun it when a page changes; the PNGs are
// committed, since the PDF build must not need a server.
//
// The server is started by the e2e suite's own harness, so it is `--backend loopback` by name,
// with GAZELLE_NO_HARDWARE set: it cannot open a real device. `--loopback-cyclic-ms` makes the
// emulated devices report state and meter levels, so the pages show values rather than blanks.
// It is not in dry run, so the emulator answers reads as a device would.

import { spawnSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { chromium, type BrowserContext, type Locator, type Page } from "@playwright/test";

import { startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, "../../../..");
const IMAGES = join(REPO_ROOT, "docs", "images");
const QUADRO = "loopback-0";
const STUDIO = "loopback-1";

type Channel = { id: string; name: string; slot: number; sends: number[]; source?: { group: number; channel: number }; main_mix?: number; group?: string };
const channel = (id: string, name: string, slot: number, group: number | undefined, source: number, main: number | undefined, sends: number[] = []): Channel => ({
  id,
  name,
  slot,
  sends,
  ...(group === undefined ? {} : { source: { group, channel: source } }),
  ...(main === undefined ? {} : { main_mix: main }),
});

/** A plausible session on each model: named channels, a cue mix, a group, a surface and a cable. */
function workspace(extraQuadroChannels: Channel[] = []): object {
  return {
    version: 1,
    aliases: { [QUADRO]: "Desk", [STUDIO]: "Live room" },
    device_colors: {},
    groups: [],
    links: [{ id: "drums", kind: "mixer", mode: "absolute", members: [{ device_id: STUDIO, channel: 0 }, { device_id: STUDIO, channel: 1 }] }],
    mixers: {
      [QUADRO]: {
        mixes: [{ name: "Monitors" }, { name: "Cue" }],
        groups: [],
        channels: [
          channel("q1", "Vox", 6, 0, 0, 0, [1]),
          ...extraQuadroChannels,
          channel("q2", "Guitar DI", 7, 0, 1, 0, [1]),
          channel("q3", "DAW L", 8, 1, 0, 0, [1]),
          channel("q4", "DAW R", 9, 1, 1, 0, [1]),
          channel("q5", "Synth", 10, 3, 0, 0),
          channel("q6", "Click", 11, 1, 2, 1),
        ],
      },
      [STUDIO]: {
        mixes: [{ name: "Main" }, { name: "Artist" }],
        groups: [{ id: "drums", name: "Drums", collapsed: false, color: "#b5473a" }],
        channels: [
          { ...channel("s1", "Kick", 0, 0, 0, 0, [1]), group: "drums" },
          { ...channel("s2", "Snare", 1, 0, 1, 0, [1]), group: "drums" },
          { ...channel("s3", "OH L", 2, 0, 2, 0), group: "drums" },
          { ...channel("s4", "OH R", 3, 0, 3, 0), group: "drums" },
          channel("s5", "Bass", 4, 1, 0, 0, [1]),
          channel("s6", "Keys", 5, 4, 0, 0, [1]),
          channel("s7", "Tracks", 6, 3, 0, 0, [1]),
          channel("s8", "Talkback", 7, 0, 4, 1),
        ],
      },
    },
    layouts: [],
    cables: [{ id: "adat", from: { device_id: STUDIO, port: "ADAT_OUT", first: 0 }, to: { device_id: QUADRO, port: "ADAT_IN", first: 0 }, channels: 8 }],
    surfaces: [
      {
        id: "tracking",
        name: "Drum tracking",
        mixes: { [QUADRO]: 1 },
        strips: [
          { id: "label", kind: "label", text: "Drums" },
          { id: "pre1", kind: "input", device_id: STUDIO, input: { kind: "preamp", channel: 0 } },
          { id: "pre2", kind: "input", device_id: STUDIO, input: { kind: "preamp", channel: 1 } },
          { id: "adatout", kind: "port", device_id: STUDIO, port: "ADAT_OUT", first: 0 },
          { id: "vox", kind: "channel", device_id: QUADRO, channel: "q1" },
          { id: "gtr", kind: "channel", device_id: QUADRO, channel: "q2" },
          { id: "cue", kind: "master", device_id: QUADRO, mix: 1 },
          { id: "hp2", kind: "output", device_id: QUADRO, output: 2 },
        ],
      },
    ],
    control_room: { [QUADRO]: { outputs: [0, 1, 2, 3] } },
  };
}

async function putWorkspace(server: RunningServer, body: object): Promise<void> {
  const response = await fetch(`${server.url}/api/v1/workspace`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`workspace PUT failed: ${response.status} ${await response.text()}`);
}

/**
 * The emulator's status reports are a moving test pattern: every byte sweeps, so a page would show
 * a clock of "3410.7 kHz", a random preset and outputs muting and unmuting. For the pictures, the
 * status fields a reader would take at face value are held at a plausible, quiet state: on, preset
 * 1, the first clock source at 48 kHz and locked, nothing muted, dimmed or summed to mono. Meters
 * and levels keep moving. This changes only what the browser is shown, never the server.
 */
function steadyStatus(frame: string): string {
  let message: { type?: string; report_id?: string; fields?: Record<string, unknown> };
  try {
    message = JSON.parse(frame) as typeof message;
  } catch {
    return frame;
  }
  if (message.type !== "cyclic" || message.report_id !== "0x73" || message.fields === undefined) return frame;
  const fields = message.fields;
  const held: Record<string, number> = { power_on: 1, current_preset: 1, sync_source: 0, base_index: 2, sync_freq_hi: 0x00, sync_freq_mid: 0xbb, sync_freq_low: 0x80, locked: 1, locked_wc: 1, hard_mute: 0 };
  for (const [name, value] of Object.entries(held)) if (name in fields) fields[name] = value;
  // Mutes, dims, mono flags and 48V, wherever they sit: at the top or in a list of outputs or preamps.
  const quiet = (record: Record<string, unknown>) => {
    for (const [name, value] of Object.entries(record)) {
      if (typeof value === "number" && /mute|dim|mono|phantom/.test(name)) record[name] = 0;
      else if (Array.isArray(value)) for (const item of value) if (typeof item === "object" && item !== null) quiet(item as Record<string, unknown>);
    }
  };
  quiet(fields);
  return JSON.stringify(message);
}

async function steady(context: BrowserContext): Promise<void> {
  await context.routeWebSocket(/\/api\/v1\/ws/, (ws) => {
    const server = ws.connectToServer();
    server.onMessage((message) => ws.send(typeof message === "string" ? steadyStatus(message) : message));
  });
}

async function settle(page: Page): Promise<void> {
  await page.evaluate(() => document.fonts.ready.then(() => undefined));
  // Meters move on the emulator's reports; a moment lets them rise and the page finish reading.
  await page.waitForTimeout(900);
}

async function shoot(page: Page, name: string, options: { fullPage?: boolean; clip?: Locator } = {}): Promise<void> {
  await settle(page);
  const path = join(IMAGES, `${name}.png`);
  if (options.clip) await options.clip.screenshot({ path });
  else await page.screenshot({ path, fullPage: options.fullPage ?? false });
  console.log(`  ${name}.png`);
}

/** Takes a snapshot once both emulated devices have reported everything a snapshot records. */
async function takeSnapshot(server: RunningServer, name: string): Promise<string> {
  for (let attempt = 0; attempt < 40; attempt++) {
    const created = (await (await fetch(`${server.url}/api/v1/snapshots`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ name }) })).json()) as { id: string };
    const whole = (await (await fetch(`${server.url}/api/v1/snapshots/${created.id}`)).json()) as { devices: Record<string, { unreadable: unknown[] }> };
    if (Object.values(whole.devices).every((device) => device.unreadable.length === 0)) return created.id;
    await fetch(`${server.url}/api/v1/snapshots/${created.id}`, { method: "DELETE" });
    await new Promise((done) => setTimeout(done, 500));
  }
  throw new Error("the emulated devices never reported everything a snapshot records");
}

async function main(): Promise<void> {
  // The server embeds the web app's build, so build it first, as the e2e suite's setup does.
  const build = spawnSync(process.execPath, [join(REPO_ROOT, "web/apps/web/scripts/build.ts")], { stdio: "inherit" });
  if (build.status !== 0) throw new Error("building the web app failed");
  mkdirSync(IMAGES, { recursive: true });

  const server = await startServer(["--backend", "loopback", "--loopback-cyclic-ms", "60"], { webUi: true });
  const browser = await chromium.launch();
  try {
    await putWorkspace(server, workspace());
    const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, colorScheme: "dark" });
    await steady(context);
    const page = await context.newPage();
    const go = async (hash: string, ready: Locator) => {
      // A new document each time: a change of hash alone would keep the workspace the page read.
      await page.goto("about:blank");
      await page.goto(`${server.url}/${hash}`);
      await ready.waitFor({ state: "visible", timeout: 20_000 });
    };
    console.log("taking screenshots:");

    await go(`#/mixer/${QUADRO}`, page.getByTestId("fader-7"));
    await shoot(page, "mixer-quadro");
    await go(`#/mixer/${STUDIO}`, page.getByTestId("fader-1"));
    await shoot(page, "mixer-studio");

    await go(`#/devices/${QUADRO}`, page.locator("ga-device-status"));
    await shoot(page, "devices-quadro");

    await go(`#/inputs/${STUDIO}`, page.getByTestId("preamp-0"));
    await shoot(page, "inputs-studio");
    await go(`#/inputs/${QUADRO}`, page.getByTestId("preamp-0"));
    await shoot(page, "inputs-quadro", { fullPage: true });

    await go(`#/outputs/${STUDIO}`, page.getByTestId("output-0"));
    await shoot(page, "outputs-studio");

    await go(`#/routing/${QUADRO}`, page.getByTestId("source-0-0"));
    await shoot(page, "routing-quadro");

    await go(`#/effects/${QUADRO}`, page.getByTestId("chain-0"));
    await page.getByTestId("edit-0-0").click();
    await page.getByTestId("effect-editor").waitFor({ state: "visible" });
    await page.evaluate(() => window.scrollTo(0, 0));
    await shoot(page, "effects-quadro");
    await shoot(page, "effect-editor", { clip: page.getByTestId("effect-editor") });

    await go(`#/workspace`, page.locator("ga-workspace input").first());
    await shoot(page, "workspace");

    await go(`#/surface/tracking`, page.getByTestId("surface-strip-vox"));
    await shoot(page, "surface");

    // The sidebar's Control Room, beside the Outputs page it is chosen from.
    await go(`#/outputs/${QUADRO}`, page.getByTestId("output-0"));
    await shoot(page, "control-room", { clip: page.locator("ga-control-room") });

    // The same signal twice in one mix: a second channel on the Vox preamp.
    await putWorkspace(server, workspace([channel("q7", "Vox again", 12, 0, 0, 0)]));
    await go(`#/mixer/${QUADRO}`, page.getByTestId("fader-12"));
    // A channel with its fader on the floor adds nothing to the mix, so bring both up a little.
    for (const slot of [6, 12]) {
      await page.getByTestId(`fader-${slot}`).focus();
      for (let i = 0; i < 4; i++) await page.getByTestId(`fader-${slot}`).press("PageUp");
    }
    await page.getByTestId("doubled-6").waitFor({ state: "visible", timeout: 20_000 });
    await settle(page);
    const boxes = await Promise.all([6, 12].map((slot) => page.locator(`ga-channel[data-channel-slot="${slot}"]`).boundingBox()));
    if (boxes.some((box) => box === null)) throw new Error("the doubled channels are not on the page");
    const [a, b] = boxes as [{ x: number; y: number; width: number; height: number }, { x: number; y: number; width: number; height: number }];
    const left = Math.min(a.x, b.x) - 16;
    const top = Math.min(a.y, b.y) - 12;
    await page.screenshot({ path: join(IMAGES, "doubled-badge.png"), clip: { x: left, y: top, width: Math.max(a.x + a.width, b.x + b.width) + 4 - left, height: Math.max(a.y + a.height, b.y + b.height) + 12 - top } });
    console.log("  doubled-badge.png");
    await putWorkspace(server, workspace());

    // Snapshots: one taken, then compared with a device that has moved on, then a recall preview.
    const id = await takeSnapshot(server, "Evening session");
    await go(`#/workspace`, page.getByTestId(`snapshot-compare-${id}`));
    await page.getByTestId(`snapshot-compare-${id}`).click();
    await page.getByTestId("snapshot-diff-summary").waitFor({ state: "visible", timeout: 20_000 });
    await page.getByTestId("snapshot-diff-summary").evaluate((el) => el.scrollIntoView({ block: "start" }));
    await page.evaluate(() => window.scrollBy(0, -80));
    await shoot(page, "snapshot-compare");
    await page.getByTestId("snapshot-recall-prepare").click();
    await page.getByTestId("snapshot-recall-summary").waitFor({ state: "visible", timeout: 20_000 });
    await page.getByTestId("snapshot-recall-summary").evaluate((el) => el.scrollIntoView({ block: "start" }));
    await page.evaluate(() => window.scrollBy(0, -80));
    await shoot(page, "recall-preview");

    // A phone: the same app, the sidebar as a drawer.
    const phone = await browser.newContext({ viewport: { width: 375, height: 812 }, deviceScaleFactor: 2, isMobile: true, hasTouch: true, colorScheme: "dark" });
    await steady(phone);
    const small = await phone.newPage();
    await small.goto(`${server.url}/#/mixer/${QUADRO}`);
    await small.getByTestId("fader-7").waitFor({ state: "visible", timeout: 20_000 });
    await settle(small);
    await small.screenshot({ path: join(IMAGES, "phone-mixer.png") });
    console.log("  phone-mixer.png");
    await phone.close();
    await context.close();
  } finally {
    await browser.close();
    await server.stop();
  }
}

await main();
